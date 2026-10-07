#!/usr/bin/env python3
"""Strict readback assertions for the disposable-macOS proxy acceptance test.

Importing this module never runs native commands. Unit tests inject recorded text.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import signal
import time
import tomllib


def reject_native_errors(text: str) -> None:
    for line in text.splitlines():
        lowered = line.strip().lower()
        if lowered.startswith(("**", "error:")) or any(
            marker in lowered for marker in ("permission denied", "must be root", "not authorized")
        ):
            raise ValueError("networksetup reported an error despite its exit status")


def parse_protocol(text: str) -> dict:
    reject_native_errors(text)
    required = ("Enabled", "Server", "Port", "Authenticated Proxy Enabled")
    fields = {}
    for line in text.splitlines():
        key, separator, value = line.partition(":")
        if separator and key in required:
            if key in fields:
                raise ValueError("duplicate networksetup proxy field")
            fields[key] = value.strip()
    if any(key not in fields for key in required):
        raise ValueError("incomplete networksetup proxy readback")
    if fields["Enabled"] not in ("Yes", "No") or fields["Authenticated Proxy Enabled"] not in ("0", "1"):
        raise ValueError("invalid networksetup proxy flags")
    port = int(fields["Port"])
    if not 0 <= port <= 65535:
        raise ValueError("invalid networksetup proxy port")
    return {"enabled": fields["Enabled"] == "Yes", "server": fields["Server"],
            "port": port, "authenticated": fields["Authenticated Proxy Enabled"] == "1"}


def parse_bypass(text: str) -> list[str]:
    reject_native_errors(text)
    lines = text.strip().splitlines()
    if len(lines) == 1 and lines[0].startswith("There aren't any bypass domains set on "):
        return []
    if not lines or any(not line or line.startswith("**") for line in lines):
        raise ValueError("invalid networksetup bypass readback")
    return lines


def native_read(*args: str) -> str:
    result = subprocess.run(["/usr/sbin/networksetup", *args], check=True, text=True,
                            capture_output=True, timeout=15,
                            env={**os.environ, "LC_ALL": "C"})
    reject_native_errors(result.stdout + "\n" + result.stderr)
    return result.stdout


def capture(read=native_read) -> dict:
    lines = read("-listallnetworkservices").splitlines()
    if not lines or "asterisk" not in lines[0]:
        raise ValueError("invalid network service list")
    services = [line for line in lines[1:] if line and not line.startswith("*")]
    if not services or len(services) != len(set(services)):
        raise ValueError("no unique enabled network services to validate")
    return {name: {"http": parse_protocol(read("-getwebproxy", name)),
                   "https": parse_protocol(read("-getsecurewebproxy", name)),
                   "bypass": parse_bypass(read("-getproxybypassdomains", name)),
                   "untouched": {key: opaque_readback(read(command, name)) for key, command in (
                       ("socks", "-getsocksfirewallproxy"),
                       ("pac", "-getautoproxyurl"),
                       ("autodiscovery", "-getproxyautodiscovery"))}}
            for name in services}


def opaque_readback(text: str) -> str:
    reject_native_errors(text)
    if not text.strip():
        raise ValueError("empty native out-of-scope configuration readback")
    return text


def assert_untouched(name: str, original: dict, actual: dict) -> None:
    for field in ("socks", "pac", "autodiscovery"):
        if actual["untouched"][field] != original["untouched"][field]:
            raise AssertionError(f"{name}: out-of-scope {field} configuration changed")


def assert_disabled(original: dict, actual: dict, *, allow_empty_metadata=False) -> None:
    if original.keys() != actual.keys():
        raise AssertionError("network service set changed during acceptance test")
    for name, before in original.items():
        now = actual[name]
        assert_untouched(name, before, now)
        for field in ("http", "https"):
            old, observed = before[field], now[field]
            if observed["enabled"]:
                raise AssertionError(f"{name} {field}: product left native routing enabled")
            if observed["authenticated"] != old["authenticated"]:
                raise AssertionError(f"{name} {field}: authentication flag changed")
            if old["server"] and old["port"] > 0:
                if (observed["server"], observed["port"]) != (old["server"], old["port"]):
                    raise AssertionError(f"{name} {field}: present original endpoint was not restored")
            elif (observed["server"], observed["port"]) != (old["server"], old["port"]):
                message = f"{name} {field}: INCOMPLETE RESTORATION: disabled empty/zero dormant endpoint is not exactly restored"
                if not allow_empty_metadata:
                    raise AssertionError(message)
                print("NONROOT LIMITATION: " + message)
        if now["bypass"] != before["bypass"]:
            raise AssertionError(f"{name}: original bypass was not restored by the product")


def assert_active(original: dict, actual: dict, port: int) -> None:
    if original.keys() != actual.keys():
        raise AssertionError("network service set changed during acceptance test")
    for name, service in actual.items():
        assert_untouched(name, original[name], service)
        for field in ("http", "https"):
            observed = service[field]
            if (not observed["enabled"] or observed["authenticated"]
                    or (observed["server"], observed["port"]) != ("127.0.0.1", port)):
                raise AssertionError(f"{name} {field}: healthy runtime did not automatically resume native proxy")


def intent(path: Path) -> dict:
    value = tomllib.loads(path.read_text())["system_proxy"]
    if value.get("enabled") is not True:
        raise AssertionError("configured enable intent was lost")
    return {key: value[key] for key in ("enabled", "intent_revision", "recovery_mode", "recovery_grace_secs")}


def lease_identity(path: Path) -> dict:
    state = json.loads(path.read_text())
    if not state.get("generation") or not state.get("macos_services"):
        raise AssertionError("native recovery lost its active ownership journal")
    return {"generation": state["generation"],
            "baselines": [{"name": service["name"], "fields": [
                {"field": field["field"], "before": field["before"]}
                for field in service["fields"]]} for service in state["macos_services"]]}


def check_lease(path: Path, snapshot: Path, phase: str) -> None:
    state = json.loads(path.read_text())
    if lease_identity(path) != json.loads(snapshot.read_text()):
        raise AssertionError("native recovery changed generation or recaptured its original baseline")
    if state.get("phase") != phase or state.get("applied") is not (phase == "applied"):
        raise AssertionError("native recovery did not retain the expected active lease phase")


def process_identity(pid: int) -> str:
    result = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "ppid=", "-o", "lstart="],
                            text=True, capture_output=True, timeout=5,
                            env={**os.environ, "LC_ALL": "C"})
    if result.returncode == 1 and not result.stdout.strip() and not result.stderr.strip():
        return ""  # ps's documented no-selected-process result, not an I/O error.
    if result.returncode != 0 or result.stderr.strip() or not result.stdout.strip():
        raise AssertionError("could not verify fixture core process identity")
    return " ".join(result.stdout.split())


def process_stopped(pid: int) -> bool:
    result = subprocess.run(["/bin/ps", "-p", str(pid), "-o", "state="],
                            text=True, capture_output=True, timeout=5,
                            env={**os.environ, "LC_ALL": "C"})
    if result.returncode != 0 or result.stderr.strip() or not result.stdout.strip():
        raise AssertionError("could not confirm fixture core stopped state")
    return result.stdout.strip().startswith("T")


def control_owned_core(data: Path, pid: int, expected: str, action: str, *,
                       read_identity=process_identity, send_signal=os.kill,
                       read_stopped=process_stopped) -> None:
    """Pause/retire only a live fixture child while its native writer is idle.

    Holding the existing flock prevents STOP from freezing a transaction and
    prevents fixture-only KILL from orphaning a setter or retiring the journal.
    Unexpected prior exit is unsafe: an ordinary native child could outlive its
    core without retaining the flock. Keep evidence rather than guessing.
    """
    import fcntl
    if action not in ("pause", "retire"):
        raise AssertionError("unsupported native fixture process control")
    if pid <= 1 or not expected or expected.split()[0] != str(os.getppid()):
        raise AssertionError("refusing a core outside this fixture's direct children")
    observed = read_identity(pid)
    if observed != expected:
        raise AssertionError("refusing absent or reused core PID")
    fd = os.open(data / ".system_proxy.lock", os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW)
    try:
        deadline = time.monotonic() + 180
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise AssertionError("native writer did not release the ownership lock")
                time.sleep(0.1)
        observed = read_identity(pid)
        if observed != expected:
            raise AssertionError("core disappeared or changed identity while waiting for native writer quiescence")
        send_signal(pid, signal.SIGSTOP if action == "pause" else signal.SIGKILL)
        if action == "pause":
            deadline = time.monotonic() + 10
            while True:
                if read_identity(pid) != expected:
                    raise AssertionError("core identity changed while confirming pause")
                if read_stopped(pid) and read_identity(pid) == expected:
                    return  # Keep flock until STOP has actually taken effect.
                if time.monotonic() >= deadline:
                    raise AssertionError("fixture core did not enter stopped state")
                time.sleep(0.05)
        deadline = time.monotonic() + 10
        while read_identity(pid) == expected:
            if time.monotonic() >= deadline:
                raise AssertionError("fixture core did not exit after controlled termination")
            time.sleep(0.1)
    finally:
        os.close(fd)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("capture", "disabled", "nonroot-suspended", "active", "intent", "check-intent", "restore-bypass", "lease", "check-lease", "pause-core", "retire-core"))
    parser.add_argument("paths", nargs="+")
    args = parser.parse_args()
    paths = [Path(path) for path in args.paths]
    if args.mode == "capture":
        paths[0].write_text(json.dumps(capture(), indent=2) + "\n")
    elif args.mode in ("disabled", "nonroot-suspended", "active"):
        before, after = (json.loads(path.read_text()) for path in paths[:2])
        if args.mode == "active":
            assert_active(before, after, int(args.paths[2]))
        else:
            assert_disabled(before, after, allow_empty_metadata=args.mode == "nonroot-suspended")
    elif args.mode == "intent":
        paths[1].write_text(json.dumps(intent(paths[0]), indent=2) + "\n")
    elif args.mode == "check-intent":
        if intent(paths[0]) != json.loads(paths[1].read_text()):
            raise AssertionError("recovery or stop changed persisted proxy intent")
    elif args.mode == "lease":
        paths[1].write_text(json.dumps(lease_identity(paths[0]), indent=2) + "\n")
    elif args.mode == "check-lease":
        check_lease(paths[0], paths[1], args.paths[2])
    elif args.mode in ("pause-core", "retire-core"):
        control_owned_core(paths[0], int(args.paths[1]), args.paths[2], args.mode.split("-")[0])
    else:
        # Fixture cleanup only; never SOCKS/PAC or an endpoint setter. Assertions
        # happen before this command so it cannot conceal product bypass drift.
        for name, service in json.loads(paths[0].read_text()).items():
            native_read("-setproxybypassdomains", name, *(service["bypass"] or ["Empty"]))


if __name__ == "__main__":
    main()
