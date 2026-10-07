#!/usr/bin/env python3
"""Linux-only CLI regression: real listeners/intent handoff, no OS proxy writes."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.request

if sys.platform != "linux":
    print("SKIP: use isolated Linux; this test must not change host proxy settings")
    raise SystemExit(0)

binary = Path(os.environ.get("BIFROST_BIN", "target/debug/bifrost")).resolve()
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def request(number, path, body=None):
    payload = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{number}/_bifrost/api/{path}", data=payload,
        method="GET" if body is None else "PUT",
        headers={"Content-Type": "application/json"},
    )
    with opener.open(req, timeout=2) as response:
        return json.load(response)


def canary(number):
    with socket.create_connection(("127.0.0.1", number), timeout=2) as stream:
        stream.sendall(b"GET http://bifrost-runtime-canary.invalid/__bifrost_runtime_canary HTTP/1.1\r\nHost: bifrost-runtime-canary.invalid\r\nConnection: close\r\n\r\n")
        return stream.recv(128).startswith(b"HTTP/1.1 204")


def wait_for(predicate, child, log):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if child.poll() is not None:
            raise AssertionError(f"core exited {child.returncode}: {log.read_text()}")
        try:
            if predicate():
                return
        except (OSError, ValueError):
            pass
        time.sleep(0.1)
    raise AssertionError(f"readiness deadline: {log.read_text()}")


with tempfile.TemporaryDirectory(prefix="bifrost-recovery-e2e-") as temp:
    root = Path(temp)
    data, home = root / "data", root / "home"
    data.mkdir()
    home.mkdir()
    (data / "config.toml").write_text(
        '[system_proxy]\nenabled = true\nintent_revision = 2\n'
        '[tray]\nenabled = false\n[sync]\nenabled = false\nauto_sync = false\n'
    )
    env = dict(os.environ, HOME=str(home), BIFROST_DATA_DIR=str(data),
               XDG_CONFIG_HOME=str(home / ".config"), BIFROST_DISABLE_TRAY="1",
               BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="1",
               BIFROST_SYSTEM_PROXY_DISABLE_LAUNCHD_INSTALL="1")
    env.pop("BIFROST_SYSTEM_PROXY_INTENT_REVISION_INTERNAL", None)
    env.pop("BIFROST_SYSTEM_PROXY_RECOVERY_GENERATION_INTERNAL", None)
    for automatic in (False, True):
        first, second = port(), port()
        while second == first:
            second = port()
        launch_env = env.copy()
        if automatic:
            launch_env["BIFROST_SYSTEM_PROXY_INTENT_REVISION_INTERNAL"] = "1"
            launch_env["BIFROST_SYSTEM_PROXY_RECOVERY_GENERATION_INTERNAL"] = ""
        log = root / f"core-{automatic}.log"
        with log.open("w") as output:
            child = subprocess.Popen(
                [str(binary), "-H", "127.0.0.1", "-p", str(first), "start",
                 "--no-system-proxy", "--skip-cert-check"],
                env=launch_env, stdin=subprocess.DEVNULL, stdout=output, stderr=output,
            )
            try:
                wait_for(lambda: canary(first), child, log)
                runtime = json.loads((data / "runtime.json").read_text())
                assert runtime["system_proxy_enabled"] is automatic, runtime
                assert runtime["system_proxy_config_revision"] == 2, runtime
                response = request(first, "config/server", {"port": second})
                assert response["actual_port"] == second, response
                wait_for(lambda: canary(second), child, log)
                runtime = json.loads((data / "runtime.json").read_text())
                assert runtime["port"] == second, runtime
                assert runtime["system_proxy_enabled"] is automatic, runtime
                assert runtime["system_proxy_config_revision"] == 2, runtime
                def old_closed():
                    try:
                        with socket.create_connection(("127.0.0.1", first), timeout=0.2):
                            return False
                    except OSError:
                        return True
                wait_for(old_closed, child, log)
                config = tomllib.loads((data / "config.toml").read_text())
                assert config["system_proxy"]["enabled"] is True, config["system_proxy"]
                assert config["system_proxy"]["intent_revision"] == 2, config["system_proxy"]
                assert not (data / "proxy_state.json").exists()
                print(f"PASS: {'stale automatic flags' if automatic else 'explicit session override'}, rebind, old-listener retirement, persisted intent")
            finally:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
print("PASS: real CLI handoff regression; no host system proxy operations")
