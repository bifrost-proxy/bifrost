from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
FIXTURE = ROOT / "e2e-tests/tests/test_socks5_tls_routing_exceptions.sh"
BASH = shutil.which("bash")
PORT_NAMES = (
    "PROXY_PORT", "SOCKS5_PORT", "DOWNSTREAM_PROXY_PORT", "ECHO_HTTP_PORT",
    "ECHO_HTTPS_PORT", "SECONDARY_HTTPS_PORT",
)


def fixture_function(name: str) -> str:
    source = FIXTURE.read_text()
    return name + "() {" + source.split(name + "() {", 1)[1].split("\n}", 1)[0] + "\n}"


def port_assignments() -> str:
    return re.search(r'^PROXY_HOST=.*?(?=^\s*$)', FIXTURE.read_text(), re.M | re.S).group()


@unittest.skipUnless(BASH, "bash is required")
class SecondaryHttpsPortTests(unittest.TestCase):
    def ports(self, **overrides: str) -> dict[str, int]:
        result = subprocess.run(
            [BASH, "-c", "set -eu\n" + port_assignments() + "\n"
             + 'printf "%s\\n" ' + " ".join(f'"${{{name}}}"' for name in PORT_NAMES)],
            env={"PATH": os.environ["PATH"], **overrides},
            text=True, capture_output=True, check=True, timeout=5,
        )
        return dict(zip(PORT_NAMES, map(int, result.stdout.splitlines())))

    def test_assigned_secondary_stays_in_own_stride_without_collisions(self) -> None:
        # Same assignments as all three runner launch paths (isolated, setsid,
        # and plain background). Include the failing CI job's assigned base.
        for base in (15000, 15134, 34000):
            with self.subTest(base=base):
                ports = self.ports(
                    PROXY_PORT=str(base), ECHO_HTTP_PORT=str(base + 1),
                    ECHO_HTTPS_PORT=str(base + 2), SOCKS5_PORT=str(base + 6),
                    ECHO_PROXY_PORT=str(base + 7),
                )
                self.assertEqual(ports["SECONDARY_HTTPS_PORT"], base + 8)
                self.assertEqual(len(set(ports.values())), len(PORT_NAMES))
                for sibling in (base - 10, base + 10):
                    self.assertNotIn(ports["SECONDARY_HTTPS_PORT"], range(sibling, sibling + 10))

    def test_standalone_defaults_are_distinct(self) -> None:
        ports = self.ports()
        self.assertEqual(ports["SECONDARY_HTTPS_PORT"], ports["PROXY_PORT"] + 8)
        self.assertEqual(len(set(ports.values())), len(PORT_NAMES))

    def test_explicit_secondary_and_downstream_alias_precedence_are_preserved(self) -> None:
        for overrides, expected in (
            ({}, 32007),
            ({"MOCK_ECHO_PROXY_PORT": "33007"}, 33007),
            ({"MOCK_ECHO_PROXY_PORT": "33007", "ECHO_PROXY_PORT": "34007"}, 34007),
            ({"MOCK_ECHO_PROXY_PORT": "33007", "ECHO_PROXY_PORT": "34007",
              "DOWNSTREAM_PROXY_PORT": "35007"}, 35007),
        ):
            with self.subTest(overrides=overrides):
                ports = self.ports(PROXY_PORT="32000", SECONDARY_HTTPS_PORT="36008", **overrides)
                self.assertEqual(ports["SECONDARY_HTTPS_PORT"], 36008)
                self.assertEqual(ports["DOWNSTREAM_PROXY_PORT"], expected)
                self.assertEqual(len(set(ports.values())), len(PORT_NAMES))


@unittest.skipUnless(BASH, "bash is required")
class SecondaryHttpsStartupTests(unittest.TestCase):
    def test_log_exists_before_asynchronous_child_or_readiness_probe(self) -> None:
        startup = fixture_function("start_mock_servers")
        self.assertLess(startup.index(': >"$SECONDARY_HTTPS_LOG_FILE"'),
                        startup.index('python3 -u '))

    def run_startup(self, mode: str) -> tuple[subprocess.CompletedProcess, list[str]]:
        # Execute the real fixture functions with only local process/file stubs.
        # No sockets, Bifrost, system-proxy APIs, or certificate stores are used.
        with tempfile.TemporaryDirectory(prefix="bifrost-secondary-fixture-") as tmp:
            root = Path(tmp)
            servers = root / "mock_servers"
            servers.mkdir()
            manager = servers / "start_servers.sh"
            manager.write_text('#!/bin/sh\nprintf "manager %s\\n" "$1" >> "$TRACE"\n')
            manager.chmod(0o755)
            script = r'''
set -euo pipefail
PROXY_PORT=31000 SOCKS5_PORT=31006 DOWNSTREAM_PROXY_PORT=31007
ECHO_HTTP_PORT=31001 ECHO_HTTPS_PORT=31002 SECONDARY_HTTPS_PORT=31008
TEST_DATA_DIR="$E2E_DIR/data"
SECONDARY_HTTPS_LOG_FILE="$TEST_DATA_DIR/secondary-https.log"
PROXY_PID="" DOWNSTREAM_PROXY_PID="" SECONDARY_HTTPS_PID=""
LIVE_CHECKS=0
log_section() { :; }
_log_pass() { echo "$1"; }
python3() {
    printf 'python %s\n' "$*" >> "$TRACE"
    echo "fixture stderr: simulated secondary startup diagnostic" >&2
    if [[ "$MODE" == ready_crlf ]]; then
        printf 'READY\r\n'
    elif [[ "$MODE" != missing_ready ]]; then
        echo READY
    fi
    if [[ "$MODE" == exited* ]]; then return 73; fi
}
kill() {
    [[ "$1" == -0 && "$2" == "$SECONDARY_HTTPS_PID" ]] || return 99
    LIVE_CHECKS=$((LIVE_CHECKS + 1))
    # Synchronize the fake child without depending on scheduler timing.
    builtin wait "$SECONDARY_HTTPS_PID" || true
    if [[ "$MODE" == exited* ]]; then return 1; fi
    if [[ "$MODE" == dies_after_health && "$LIVE_CHECKS" -gt 1 ]]; then return 1; fi
    return 0
}
curl() {
    printf 'curl %s\n' "$*" >> "$TRACE"
    case "$MODE:${*: -1}" in
        timeout_http:*:31001/health|timeout_https:*:31002/health|timeout_secondary:*:31008/health)
            return 1 ;;
    esac
    return 0
}
sleep() { echo sleep >> "$TRACE"; }
kill_pid() { printf 'kill_pid %s\n' "$1" >> "$TRACE"; }
safe_cleanup_proxy() { echo unexpected_proxy_cleanup >> "$TRACE"; }
kill_bifrost_on_port() { printf 'kill_port %s\n' "$1" >> "$TRACE"; }
'''
            script += "\n".join(fixture_function(name) for name in (
                "cleanup", "report_mock_startup_failure", "start_mock_servers"))
            script += '\ntrap cleanup EXIT\nstart_mock_servers\nprintf "ready_pid %s\\n" "$SECONDARY_HTTPS_PID" >> "$TRACE"\n'
            result = subprocess.run(
                [BASH, "-c", script],
                env={"PATH": os.environ["PATH"], "E2E_DIR": tmp,
                     "TRACE": str(root / "trace"), "MODE": mode},
                text=True, capture_output=True, timeout=5,
            )
            trace = (root / "trace").read_text().splitlines()
            self.assertFalse((root / "data").exists(), "EXIT cleanup must remove fixture data")
            self.assertIn("manager stop", trace)
            self.assertNotIn("kill_port 31008", trace, "do not kill an unrelated listener after bind failure")
            self.assertNotIn("unexpected_proxy_cleanup", trace)
            return result, trace

    def test_early_exit_reports_status_and_stderr_before_cleanup(self) -> None:
        result, trace = self.run_startup("exited")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exited before readiness (exit 73)", result.stderr)
        self.assertIn("secondary HTTPS=31008", result.stderr)
        self.assertIn("fixture stderr: simulated secondary startup diagnostic", result.stderr)
        self.assertNotIn("sleep", trace)
        self.assertFalse(any(line.startswith(("curl ", "kill_pid ")) for line in trace))
        self.assertNotIn("servers are ready", result.stdout)

    def test_each_unhealthy_endpoint_fails_and_preserves_secondary_diagnostics(self) -> None:
        for mode in ("timeout_http", "timeout_https", "timeout_secondary"):
            with self.subTest(mode=mode):
                result, trace = self.run_startup(mode)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("failed to become ready after 30 attempts", result.stderr)
                self.assertIn("HTTP=31001, HTTPS=31002, secondary HTTPS=31008", result.stderr)
                self.assertIn("fixture stderr: simulated secondary startup diagnostic", result.stderr)
                self.assertEqual(trace.count("sleep"), 30)
                self.assertEqual(sum(line.startswith("kill_pid ") for line in trace), 1)
                self.assertNotIn("servers are ready", result.stdout)

    def test_ready_requires_all_health_checks_and_live_owned_child(self) -> None:
        for mode in ("ready", "ready_crlf"):
            with self.subTest(mode=mode):
                result, trace = self.run_startup(mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("Mock HTTP/HTTPS servers are ready", result.stdout)
                requests = [line for line in trace if line.startswith("curl ")]
                self.assertEqual(len(requests), 3)
                for port, request in zip((31001, 31002, 31008), requests):
                    self.assertTrue(request.endswith(f":{port}/health"))
                    self.assertIn("--noproxy * --connect-timeout 1 --max-time 2", request)
                self.assertTrue(any(line.startswith("python -u ") and line.endswith("31008") for line in trace))
                ready_pid = next(line.split()[1] for line in trace if line.startswith("ready_pid "))
                self.assertIn(f"kill_pid {ready_pid}", trace)

    def test_successful_health_does_not_hide_secondary_child_exit(self) -> None:
        result, trace = self.run_startup("dies_after_health")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exited before readiness (exit 0)", result.stderr)
        self.assertIn("fixture stderr: simulated secondary startup diagnostic", result.stderr)
        self.assertNotIn("servers are ready", result.stdout)
        self.assertEqual(len([line for line in trace if line.startswith("curl ")]), 3)

    def test_live_child_must_finish_binding_before_accepting_health(self) -> None:
        result, trace = self.run_startup("missing_ready")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("failed to become ready after 30 attempts", result.stderr)
        self.assertIn("fixture stderr: simulated secondary startup diagnostic", result.stderr)
        self.assertFalse(any(line.startswith("curl ") for line in trace))
        self.assertEqual(sum(line.startswith("kill_pid ") for line in trace), 1)


if __name__ == "__main__":
    unittest.main()
