"""Exercise the proxy fixture/probe without starting a daemon or native OS tools."""
from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "e2e-tests/tests/test_stop_restart_shutdown_marker.sh"
BASH = shutil.which("bash")


@unittest.skipUnless(BASH, "bash is required")
class ProxyFixtureContractTests(unittest.TestCase):
    def run_shell(self, body: str, candidate: str, capable: bool = True) -> tuple[subprocess.CompletedProcess, str]:
        with tempfile.TemporaryDirectory(prefix="bifrost-proxy-io-") as tmp:
            directory = Path(tmp)
            binary = directory / "candidate"
            signature = "# BIFROST_PROXY_TEST_IO_CAPABILITY_V1\n" if capable else ""
            binary.write_text('#!/bin/bash\n' + signature + 'set -euo pipefail\nprintf "%s\\n" "$*" >>"$CALLS"\n' + candidate)
            binary.chmod(0o700)
            calls = directory / "calls"
            calls.touch()
            env = dict(os.environ, CALLS=str(calls), FIXTURE_ROOT=tmp,
                       BIFROST_PROXY_TEST_BIN=str(binary), BIFROST_BIN=str(binary),
                       SKIP_BUILD="true", BIFROST_COVERAGE_E2E="0")
            result = subprocess.run(
                [BASH, "-c", 'source "$1"; ' + body, "fixture-test", str(SCRIPT)],
                env=env, text=True, capture_output=True, timeout=15,
            )
            return result, calls.read_text()

    def test_successful_read_probe_and_fake_write_readback(self) -> None:
        result, calls = self.run_shell('''
            TEST_DATA_DIR="$FIXTURE_ROOT/data"; mkdir "$TEST_DATA_DIR"
            setup_fake_macos_system_proxy
            require_fake_proxy_capability
            export BIFROST_FAKE_SYSTEM_PROXY_STATE="$FAKE_PROXY_STATE"
            export BIFROST_FAKE_SYSTEM_PROXY_COMMAND_LOG="$FAKE_PROXY_COMMAND_LOG"
            "$FAKE_PROXY_BIN/networksetup" -setwebproxy Wi-Fi 127.0.0.1 18888
            "$FAKE_PROXY_BIN/networksetup" -setwebproxystate Wi-Fi on
            [[ "$("$FAKE_PROXY_BIN/networksetup" -getwebproxy Wi-Fi)" == *'Port: 18888'* ]]
            "$FAKE_PROXY_BIN/networksetup" -setproxybypassdomains Wi-Fi localhost '*.test'
            [[ "$("$FAKE_PROXY_BIN/networksetup" -getproxybypassdomains Wi-Fi)" == $'localhost\\n*.test' ]]
            "$FAKE_PROXY_BIN/networksetup" -setproxybypassdomains Wi-Fi Empty
            [[ "$("$FAKE_PROXY_BIN/networksetup" -getproxybypassdomains Wi-Fi)" == *"aren't any"* ]]
            [[ "$("$FAKE_PROXY_BIN/scutil" --proxy)" == *'HTTPEnable : 1'* ]]
        ''', '"$BIFROST_PROXY_TEST_IO_DIR/scutil" --proxy\n')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(calls.splitlines(), ["system-proxy status"])

    def test_wrong_binary_is_rejected_without_any_execution(self) -> None:
        result, calls = self.run_shell('uname() { printf "Darwin\\n"; }; main', "exit 0\n", capable=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, "")
        self.assertIn("refusing execution", result.stdout + result.stderr)

    def test_marked_binary_without_fixture_read_is_rejected_before_start(self) -> None:
        result, calls = self.run_shell('uname() { printf "Darwin\\n"; }; main', "exit 0\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls.splitlines(), ["system-proxy status"])
        self.assertIn("refusing start", result.stdout + result.stderr)

    def test_missing_binary_is_rejected_before_main_installs_cleanup(self) -> None:
        result, calls = self.run_shell('''
            uname() { printf "Darwin\\n"; }
            FAKE_PROXY_TEST_BINARY="$FIXTURE_ROOT/missing"
            main
        ''', "exit 0\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, "")
        self.assertIn("fake proxy test binary is required", result.stdout + result.stderr)

    def test_mutating_or_failed_probe_is_rejected_without_start_or_stop(self) -> None:
        for candidate in (
            '"$BIFROST_PROXY_TEST_IO_DIR/scutil" --proxy\n'
            '"$BIFROST_PROXY_TEST_IO_DIR/networksetup" -setwebproxystate Wi-Fi on\n',
            "exit 2\n",
        ):
            with self.subTest(candidate=candidate):
                result, calls = self.run_shell('uname() { printf "Darwin\\n"; }; main', candidate)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls.splitlines(), ["system-proxy status"])

    def test_fixture_cleanup_before_verified_start_never_invokes_binary(self) -> None:
        result, calls = self.run_shell('''
            TEST_DATA_DIR="$FIXTURE_ROOT/data"; mkdir "$TEST_DATA_DIR"
            setup_fake_macos_system_proxy
            cleanup
            [[ ! -d "$TEST_DATA_DIR" ]]
        ''', "exit 0\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(calls, "")


class ProxyFixtureBuildContractTests(unittest.TestCase):
    def test_ci_keeps_release_and_fixture_artifacts_separate(self) -> None:
        source = (ROOT / ".github/workflows/ci.yml").read_text()
        mac = source.split("  build-cli-macos-aarch64:", 1)[1].split("  build-cli-macos-x86_64:", 1)[0]
        self.assertLess(mac.index("name: bifrost-release-aarch64-apple-darwin"),
                        mac.index('BIFROST_BUILD_PROXY_TEST_IO: "1"'))
        self.assertIn("name: bifrost-proxy-test-io-aarch64-apple-darwin", mac)
        download = source.split("- name: Download fixture-only proxy test binary", 1)[1]
        self.assertIn("if: matrix.shard == 1", download.split("- name:", 1)[0])
        self.assertIn("path: target/proxy-test-io", download)
        self.assertIn('BIFROST_PROXY_TEST_BIN=$GITHUB_WORKSPACE/target/proxy-test-io/bifrost', download)
        self.assertNotIn("bifrost_proxy_test_io", (ROOT / "crates/bifrost-core/Cargo.toml").read_text())


if __name__ == "__main__":
    unittest.main()
