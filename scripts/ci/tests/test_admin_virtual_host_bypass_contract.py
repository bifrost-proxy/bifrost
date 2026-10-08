"""Check the virtual-host assertion without Bifrost, sockets or native OS tools."""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "e2e-tests/tests/test_admin_virtual_host_proxy.sh"
BASH = shutil.which("bash")
ASSERTION = "assert_default_system_proxy_bypass_keeps_virtual_host_routable"
LOOPBACKS = "localhost,127.0.0.1,::1"
FOREIGN_BYPASS = "*.local,169.254/16"


def fixture_function(name: str) -> str:
    source = SCRIPT.read_text()
    return name + "() {" + source.split(name + "() {", 1)[1].split("\n}", 1)[0] + "\n}"


@unittest.skipUnless(BASH, "bash is required")
class AdminVirtualHostBypassContractTests(unittest.TestCase):
    def run_assertion(self, status: str, curl_status: int = 0) -> subprocess.CompletedProcess:
        # Load only the real assertion, never the E2E's build/start/cleanup code.
        with tempfile.TemporaryDirectory(prefix="bifrost-bypass-contract-") as tmp:
            directory = Path(tmp)
            response = directory / "status.json"
            response.write_text(status)
            calls = directory / "calls"
            (directory / "proxy.log").write_text("mock proxy diagnostic\n")
            script = r'''
set -euo pipefail
ADMIN_BASE_URL="http://127.0.0.1:18080/_bifrost"
log_info() { printf '[INFO] %s\n' "$*"; }
log_fail() { printf '[FAIL] %s\n' "$*"; }
curl() {
    printf '%s\n' "$@" >> "$CALLS"
    if [[ "$CURL_STATUS" != 0 ]]; then return "$CURL_STATUS"; fi
    cat "$RESPONSE"
}
'''
            result = subprocess.run(
                [BASH, "-c", script + fixture_function(ASSERTION) + "\n" + ASSERTION],
                env={"PATH": os.environ["PATH"], "DATA_DIR": tmp,
                     "RESPONSE": str(response), "CALLS": str(calls),
                     "CURL_STATUS": str(curl_status)},
                text=True, capture_output=True, timeout=5,
            )
            self.assertEqual(response.read_text(), status, "observed state must remain untouched")
            self.assertEqual(calls.read_text().splitlines(), [
                "-fsS", "--connect-timeout", "2", "--max-time", "10",
                "http://127.0.0.1:18080/_bifrost/api/proxy/system",
            ], "the assertion must only read status once")
            return result

    def assert_rejected(self, status: str, diagnostic: str) -> None:
        result = self.run_assertion(status)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(diagnostic, result.stdout + result.stderr)
        self.assertNotIn("[INFO]", result.stdout)

    def test_foreign_observed_wildcard_is_allowed_and_preserved(self) -> None:
        # Match the macOS CI response: unmanaged OS settings can differ from
        # persisted intent because this E2E starts with --no-system-proxy.
        status = {
            "supported": True, "enabled": False, "host": "", "port": 0,
            "bypass": FOREIGN_BYPASS, "managed_by_bifrost": False,
            "configured_enabled": True, "configured_bypass": LOOPBACKS,
            "recovery_mode": "fail_open", "recovery_grace_secs": 5,
        }
        result = self.run_assertion(json.dumps(status))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("keeps bifrost.local proxy-routable", result.stdout)

    def test_default_intent_passes_without_matching_observed_bypass(self) -> None:
        for observed in ("", LOOPBACKS, "corp.example"):
            with self.subTest(observed=observed):
                result = self.run_assertion(json.dumps({
                    "configured_bypass": LOOPBACKS, "bypass": observed,
                    "managed_by_bifrost": False,
                }))
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_configured_wildcard_fails_even_when_observed_bypass_is_safe(self) -> None:
        self.assert_rejected(json.dumps({
            "configured_bypass": LOOPBACKS + ",*.local", "bypass": LOOPBACKS,
            "managed_by_bifrost": False,
        }), "must not include *.local")

    def test_json_escapes_are_decoded_before_checking_configured_bypass(self) -> None:
        status = json.dumps({"configured_bypass": LOOPBACKS}).replace("localhost", r"\u006cocalhost")
        result = self.run_assertion(status)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        status = json.dumps({"configured_bypass": LOOPBACKS + ",*.local"}).replace("*", r"\u002a")
        self.assert_rejected(status, "must not include *.local")

    def test_each_missing_configured_loopback_fails_despite_observed_defaults(self) -> None:
        entries = LOOPBACKS.split(",")
        for missing in entries:
            with self.subTest(missing=missing):
                self.assert_rejected(json.dumps({
                    "configured_bypass": ",".join(entry for entry in entries if entry != missing),
                    "bypass": LOOPBACKS,
                }), "did not include loopback entries")
        self.assert_rejected(json.dumps({"configured_bypass": "", "bypass": LOOPBACKS}),
                             "did not include loopback entries")

    def test_missing_configured_field_cannot_fall_back_to_observed_defaults(self) -> None:
        self.assert_rejected(json.dumps({"bypass": LOOPBACKS}), "Invalid configured_bypass")

    def test_wrong_configured_type_cannot_fall_back_or_stringify(self) -> None:
        for configured in (None, True, 1, [LOOPBACKS], {"value": LOOPBACKS}):
            with self.subTest(configured=configured):
                self.assert_rejected(json.dumps({
                    "configured_bypass": configured, "bypass": LOOPBACKS,
                }), "configured_bypass must be a string")

    def test_malformed_json_and_non_object_status_fail(self) -> None:
        for status in ("", '{"configured_bypass":', "null", "[]", json.dumps(LOOPBACKS)):
            with self.subTest(status=status):
                self.assert_rejected(status, "Invalid configured_bypass")

    def test_status_fetch_failure_keeps_existing_diagnostics(self) -> None:
        result = self.run_assertion(json.dumps({"configured_bypass": LOOPBACKS}), curl_status=22)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Failed to load system proxy status", result.stdout)
        self.assertIn("mock proxy diagnostic", result.stdout)
        self.assertNotIn("[INFO]", result.stdout)

    def test_no_system_proxy_and_existing_routing_checks_remain(self) -> None:
        self.assertIn("--no-system-proxy", fixture_function("start_services"))
        main = fixture_function("main")
        for assertion in (
            'assert_admin_html_via_proxy "http://bifrost.local/"',
            'assert_admin_html_via_proxy "https://bifrost.local/"',
            'assert_admin_html_via_proxy "http://bifrost.local:${PROXY_PORT}/"',
            "assert_admin_static_assets_via_proxy", "assert_direct_host_header_admin_html",
            ASSERTION, "assert_ordinary_proxy_target_still_works",
        ):
            with self.subTest(assertion=assertion):
                self.assertIn(assertion, main)


if __name__ == "__main__":
    unittest.main()
