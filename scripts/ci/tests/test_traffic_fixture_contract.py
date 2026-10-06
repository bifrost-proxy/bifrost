from __future__ import annotations

import json
import http.server
import os
import shutil
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
HELPER = ROOT / "e2e-tests/test_utils/admin_client.sh"
ATTRIBUTION = ROOT / "e2e-tests/tests/test_client_process_transport_attribution.sh"
BASH = shutil.which("bash")


@unittest.skipUnless(BASH and shutil.which("jq"), "bash and jq are required")
class TrafficLookupTests(unittest.TestCase):
    def lookup(self, records: list[dict], pattern: str, explicit: bool = False,
               full_records: bool = False) -> str:
        with tempfile.TemporaryDirectory(prefix="bifrost-traffic-fixture-") as tmp:
            curl = Path(tmp) / "curl"
            curl.write_text('#!/bin/sh\nprintf "%s\\n" "$TRAFFIC_JSON"\n')
            curl.chmod(0o755)
            env = dict(os.environ, TRAFFIC_JSON=json.dumps({"records": records}),
                       BIFROST_DATA_DIR=str(Path(tmp) / "data"),
                       BIFROST_E2E_SANDBOX_DIR=tmp)
            env["PATH"] = tmp + os.pathsep + env["PATH"]
            invocation = ('get_traffic_by_url "$2"' if full_records else
                          'find_traffic_id_by_url 127.0.0.1 12345 "$2" 50' if explicit else
                          'find_traffic_id_by_url "$2" 50')
            result = subprocess.run(
                [BASH, "-c", 'set -uo pipefail; source "$1"; '
                 'get_traffic_list() { printf "%s\\n" "$TRAFFIC_JSON"; }; '
                 + invocation, "fixture", str(HELPER), pattern],
                env=env, text=True, capture_output=True, timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            return result.stdout.strip()

    def test_compact_host_and_path_match_both_call_styles(self) -> None:
        records = [{"id": "compact", "h": "http-attr.local", "p": "/test"}]
        for explicit in (False, True):
            for pattern in ("http-attr.local", "/test", "http-attr.local/test"):
                with self.subTest(explicit=explicit, pattern=pattern):
                    self.assertEqual(self.lookup(records, pattern, explicit), "compact")

    def test_legacy_full_url_and_first_match_are_preserved(self) -> None:
        records = [{"id": "first", "url": "https://host.test/path"},
                   {"id": "second", "url": "https://host.test/path"}]
        self.assertEqual(self.lookup(records, "https://host.test/path"), "first")

    def test_pattern_is_literal_and_missing_records_are_empty(self) -> None:
        records = [{"id": "quoted", "h": "host.test", "p": '/say/"hello"'}]
        self.assertEqual(self.lookup(records, '"hello"'), "quoted")
        self.assertEqual(self.lookup(records, "unrelated"), "")
        self.assertEqual(self.lookup([], "anything"), "")

    def test_full_record_lookup_also_matches_compact_host(self) -> None:
        record = {"id": "compact", "h": "host.test", "p": "/path"}
        self.assertEqual(json.loads(self.lookup([record], "host.test", full_records=True)), record)


@unittest.skipUnless(BASH, "bash is required")
class AttributionFailurePropagationTests(unittest.TestCase):
    CASES = ("test_http_proxy_attribution", "test_websocket_attribution",
             "test_https_tunnel_attribution", "test_socks5_tls_attribution")

    def run_main(self, failed: str | None = None) -> subprocess.CompletedProcess:
        source = ATTRIBUTION.read_text()
        main = "main() {" + source.split("\nmain() {", 1)[1]
        stubs = "set -uo pipefail\nPASSED_ASSERTIONS=9 TOTAL_ASSERTIONS=9 FAILED_ASSERTIONS=0\n"
        for name in ("start_mock_servers", "build_bifrost", "write_rules", "start_proxy", "sleep", *self.CASES):
            stubs += f"{name}() {{ return {1 if name == failed else 0}; }}\n"
        return subprocess.run([BASH, "-c", stubs + main], text=True,
                              capture_output=True, timeout=10)

    def test_each_early_case_failure_prevents_false_success(self) -> None:
        for name in self.CASES:
            with self.subTest(case=name):
                result = self.run_main(name)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("1 case(s) failed", result.stdout)
                self.assertNotIn("All client process attribution tests passed", result.stdout)

    def test_setup_failure_prevents_success(self) -> None:
        for name in ("start_mock_servers", "build_bifrost", "write_rules", "start_proxy"):
            with self.subTest(setup=name):
                result = self.run_main(name)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("All client process attribution tests passed", result.stdout)

    def test_all_success_still_passes(self) -> None:
        result = self.run_main()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("All client process attribution tests passed", result.stdout)


@unittest.skipUnless(BASH and shutil.which("curl"), "bash and curl are required")
class ExplicitProxyRoutingTests(unittest.TestCase):
    def test_temporary_port_requests_ignore_inherited_proxy_exclusions(self) -> None:
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                self.send_response(200)
                self.end_headers()
                self.wfile.write(self.server.response_body)

            def log_message(self, *_args) -> None:
                pass

        servers = []
        try:
            for body in (b"direct-origin", b"via-proxy"):
                server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
                server.response_body = body
                threading.Thread(target=server.serve_forever, daemon=True).start()
                servers.append(server)
            source = (ROOT / "e2e-tests/tests/test_temporary_port_bindings.sh").read_text()
            function = "proxy_body() {" + source.split("proxy_body() {", 1)[1].split("\n}", 1)[0] + "\n}"
            for exclusion in ("127.0.0.1,localhost", "*"):
                with self.subTest(no_proxy=exclusion):
                    result = subprocess.run(
                        [BASH, "-c", function + '\nproxy_body "$1" "$2"', "fixture",
                         str(servers[1].server_port),
                         f"http://127.0.0.1:{servers[0].server_port}/direct-temp"],
                        env=dict(os.environ, NO_PROXY=exclusion, no_proxy=exclusion),
                        text=True, capture_output=True, timeout=10,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout, "via-proxy")
        finally:
            for server in servers:
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    unittest.main()
