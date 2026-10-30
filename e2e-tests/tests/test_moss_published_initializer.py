#!/usr/bin/env python3
"""Validate public MOSS installs on a fresh disposable GitHub ARM64 runner."""

import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import zipfile


def main():
    assert os.environ.get("GITHUB_ACTIONS") == "true", "requires disposable GitHub runner"
    assert platform.system() == "Darwin" and platform.machine() == "arm64"
    binary = Path(sys.argv[1]).resolve()
    asr_home = Path.home() / ".bifrost/asr"
    assert not asr_home.exists(), "refuse to touch any pre-existing ASR installation"
    process = None
    with tempfile.TemporaryDirectory(prefix="bifrost-moss-install-") as directory:
        work = Path(directory)
        env = dict(os.environ, BIFROST_DATA_DIR=str(work / "data"),
                   BIFROST_DISABLE_TRAY="1", BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT="1")
        assert not any(key.startswith("BIFROST_MOSS_") for key in env), "public default URLs required"
        with socket.socket() as reserved:
            reserved.bind(("127.0.0.1", 0))
            port = reserved.getsockname()[1]
        assert port != 9900
        base = f"http://127.0.0.1:{port}/_bifrost/api/asr/moss"

        def status():
            with urllib.request.urlopen(base + "/status", timeout=60) as response:
                return json.load(response)

        def stop():
            nonlocal process
            if process is not None:
                process.terminate()
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
                process = None

        def start():
            nonlocal process
            # Skip certificate installation and all system-proxy/tray interaction.
            with (work / "server.log").open("ab") as log:
                process = subprocess.Popen(
                    [str(binary), "start", "-p", str(port), "--skip-cert-check", "--no-system-proxy"],
                    env=env, stdout=log, stderr=subprocess.STDOUT,
                )
            for _ in range(120):
                assert process.poll() is None, "isolated server exited during startup"
                try:
                    status()
                    return
                except (urllib.error.URLError, TimeoutError):
                    time.sleep(1)
            raise AssertionError("isolated MOSS management endpoint did not become ready")

        def initialize():
            output = subprocess.check_output(
                ["curl", "--fail", "--silent", "--show-error", "--max-time", "900", base + "/init-stream"],
                text=True, timeout=920,
            )
            events = [json.loads(line[5:].strip()) for line in output.splitlines()
                      if line.startswith("data:") and line[5:].strip().startswith("{")]
            if not any(event.get("status") == "ready" for event in events):
                print(re.sub(r"https?://[^\s\"<>]+", "<URL>", output[-12000:]), flush=True)
            assert any(event.get("status") == "ready" for event in events), "initializer did not report ready"
            current = status()
            assert current["ready"] and current["runtime_ready"] and current["model_ready"], current
            root = Path(current["install_dir"]).resolve()
            assert root == (asr_home / "moss_joint_mlx").resolve(), "unexpected install root"
            marker = json.loads((root / "verification.json").read_text())
            assert marker["runtime_version"] == "1.0.1"
            assert marker["model_bytes"] == 1258427442
            assert marker["model_sha256"] == "469a8969e6b70c8b276411eca54a355a27de9ed6794f738dab53f4ffd3c83190"
            assert "2026.10.18" in (root / "runtime/site-packages/certifi/__init__.py").read_text()
            assert b"emsign" not in (root / "runtime/site-packages/certifi/cacert.pem").read_bytes().lower()
            return root

        try:
            start()
            assert not status()["ready"], "fresh runner unexpectedly reports an installed runtime"
            root = initialize()
            model = root / "model/model.safetensors"
            fingerprint = (model.stat().st_ino, model.stat().st_size, model.stat().st_mtime_ns)
            print("PASS: clean initialization from public 1.0.1 URLs; verified runtime/model and durable 1.0.1 marker", flush=True)
            stop()

            # Seed real 1.0.0 runtime bytes plus its version marker, retaining the
            # already verified model. This tests an installed-layout upgrade,
            # without needing to execute or distribute an old Bifrost program.
            asset = "moss-joint-runtime-v1.0.0-aarch64-apple-darwin.zip"
            release = "https://github.com/bifrost-proxy/bifrost/releases/download/moss-runtime-v1.0.0"
            archive = work / asset
            subprocess.run(["curl", "--fail", "--location", "--retry", "3", "--output", str(archive), f"{release}/{asset}"], check=True, timeout=600)
            manifest = subprocess.check_output(["curl", "--fail", "--location", "--silent", "--show-error", f"{release}/{asset}.sha256"], text=True, timeout=60)
            checksum, filename = manifest.strip().split()
            assert filename.lstrip("*") == asset
            assert hashlib.sha256(archive.read_bytes()).hexdigest() == checksum
            with zipfile.ZipFile(archive) as packaged:
                assert "2026.6.17" in packaged.read("moss-joint-runtime/runtime/site-packages/certifi/__init__.py").decode()
            subprocess.run(["ditto", "-x", "-k", str(archive), str(work / "old")], check=True)
            shutil.rmtree(root / "runtime")
            shutil.move(str(work / "old/moss-joint-runtime/runtime"), str(root / "runtime"))
            marker_path = root / "verification.json"
            marker = json.loads(marker_path.read_text())
            marker["runtime_version"] = "1.0.0"
            marker_path.write_text(json.dumps(marker))
            start()
            assert not status()["runtime_ready"], "old runtime was incorrectly accepted"
            initialize()
            assert fingerprint == (model.stat().st_ino, model.stat().st_size, model.stat().st_mtime_ns), "verified model was needlessly replaced"
            assert not list(root.glob("runtime.invalid-*")), "successful upgrade left managed runtime quarantines"
            print("PASS: actual 1.0.0 runtime layout upgraded to 1.0.1; old CA bundle replaced, verified model reused, quarantines cleaned", flush=True)
        finally:
            stop()
            # Only this job's newly created ASR directory is ever removed.
            shutil.rmtree(asr_home, ignore_errors=True)


if __name__ == "__main__":
    main()
