"""Verify recovery CI wiring without running Bifrost or native proxy tools."""
from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]
WRAPPER = ROOT / "e2e-tests/tests/test_proxy_recovery_handoff.sh"
REGRESSION = WRAPPER.with_suffix(".py")
MANIFEST = ROOT / "scripts/ci/proxy-coverage-shell-tests.txt"
RUNNER = ROOT / "scripts/run_all_e2e.sh"
BASH = shutil.which("bash")
PRESSURE_TEST = "test_runtime_pressure_degradation.sh"


@unittest.skipUnless(BASH, "bash is required")
class ProxyRecoveryEntrypointTests(unittest.TestCase):
    def run_wrapper(
        self, platform: str = "Linux", binary: str | None = "instrumented binary",
        profile: str | None = "coverage profiles/recovery-%p-%16m.profraw",
        status: int = 0,
    ) -> tuple[subprocess.CompletedProcess, list[str]]:
        with tempfile.TemporaryDirectory(prefix="bifrost-recovery-contract-") as tmp:
            directory = Path(tmp)
            calls = directory / "calls"
            for name, body in {
                "uname": 'printf "%s\\n" "$TEST_PLATFORM"\n',
                "python3": (
                    'printf "%s\\n" "$#" "$1" "${BIFROST_BIN-UNSET}" '
                    '"${LLVM_PROFILE_FILE-UNSET}" >>"$CALLS"\n'
                    'echo "regression stdout"\necho "regression stderr" >&2\n'
                    'exit "$TEST_PYTHON_EXIT"\n'
                ),
            }.items():
                command = directory / name
                command.write_text(f"#!{BASH}\nset -euo pipefail\n{body}")
                command.chmod(0o700)
            env = dict(os.environ, PATH=tmp + os.pathsep + os.environ["PATH"],
                       CALLS=str(calls), TEST_PLATFORM=platform,
                       TEST_PYTHON_EXIT=str(status))
            for name, value in (("BIFROST_BIN", binary), ("LLVM_PROFILE_FILE", profile)):
                if value is None:
                    env.pop(name, None)
                else:
                    env[name] = value
            result = subprocess.run(
                [BASH, str(WRAPPER)], cwd=directory, env=env,
                text=True, capture_output=True, timeout=10,
            )
            return result, calls.read_text().splitlines() if calls.exists() else []

    def test_linux_runs_existing_regression_with_injected_binary_and_profile(self) -> None:
        result, calls = self.run_wrapper()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(calls, ["1", str(REGRESSION), "instrumented binary",
                                 "coverage profiles/recovery-%p-%16m.profraw"])
        self.assertIn("regression stdout", result.stdout)
        self.assertIn("regression stderr", result.stderr)

    def test_standalone_uses_release_binary_without_creating_a_profile_override(self) -> None:
        for binary in (None, ""):
            with self.subTest(binary=binary):
                result, calls = self.run_wrapper(binary=binary, profile=None)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(calls, ["1", str(REGRESSION),
                                         str(ROOT / "target/release/bifrost"), "UNSET"])

    def test_python_failure_is_propagated_without_retry_or_fallback(self) -> None:
        result, calls = self.run_wrapper(status=23)
        self.assertEqual(result.returncode, 23, result.stdout + result.stderr)
        self.assertEqual(calls, ["1", str(REGRESSION), "instrumented binary",
                                 "coverage profiles/recovery-%p-%16m.profraw"])
        self.assertIn("regression stderr", result.stderr)
        self.assertNotIn("SKIP", result.stdout)

    def test_unsupported_platforms_skip_before_invoking_python(self) -> None:
        for platform in ("Darwin", "MINGW64_NT-10.0", "FreeBSD"):
            with self.subTest(platform=platform):
                result, calls = self.run_wrapper(platform=platform, status=23)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(calls, [])
                self.assertIn("SKIP: proxy recovery handoff requires isolated Linux", result.stdout)


@unittest.skipUnless(BASH, "bash is required")
class ProxyRecoveryInventoryTests(unittest.TestCase):
    def list_tests(self, selected: list[str] | None = None) -> list[str]:
        env = dict(os.environ, BIFROST_E2E_SHARD_INDEX="0", BIFROST_E2E_SHARD_TOTAL="0",
                   BIFROST_E2E_CAPABILITY_SHARDS="0")
        env.pop("BIFROST_E2E_SHELL_TESTS", None)
        if selected is not None:
            env["BIFROST_E2E_SHELL_TESTS"] = ",".join(selected)
        result = subprocess.run(
            [BASH, str(RUNNER), "--ci", "--full-shell", "--skip-rules", "--skip-runner",
             "--skip-ui", "--skip-build", "--list-shell-tests"],
            cwd=ROOT, env=env, text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout.splitlines()

    def test_full_shell_ci_discovers_recovery_and_pressure_exactly_once(self) -> None:
        self.assertTrue(REGRESSION.is_file())
        selected = self.list_tests()
        for name in (WRAPPER.name, PRESSURE_TEST):
            with self.subTest(name=name):
                self.assertEqual(selected.count(name), 1)

    def test_proxy_coverage_selects_recovery_and_pressure_exactly_once(self) -> None:
        manifest = MANIFEST.read_text().splitlines()
        for name in (WRAPPER.name, PRESSURE_TEST):
            with self.subTest(name=name):
                self.assertEqual(manifest.count(name), 1)
        self.assertEqual(self.list_tests(manifest), manifest)


@unittest.skipUnless(BASH, "bash is required")
class SystemProxyStabilitySignalTests(unittest.TestCase):
    SCRIPT = ROOT / "e2e-tests/tests/test_system_proxy_reconcile_stability.sh"
    SIGNAL = "system proxy transition verified"

    def test_stability_uses_current_signal_and_preserves_ownership_and_snapshot_checks(self) -> None:
        source = self.SCRIPT.read_text()
        driver = (ROOT / "crates/bifrost-cli/src/commands/start/system_proxy_reconcile/driver.rs").read_text()
        self.assertIn(f'"{self.SIGNAL}"', driver)
        self.assertEqual(source.count(self.SIGNAL), 2)
        self.assertNotIn("system proxy full reconcile completed", source)
        self.assertIn('if ! grep -q \'"managed_by_bifrost":true\' <<<"$status"; then', source)
        self.assertIn('if ! cmp -s "$SNAPSHOT_FILE" "$AFTER_SNAPSHOT_FILE"; then', source)
        self.assertIn('diff -u "$SNAPSHOT_FILE" "$AFTER_SNAPSHOT_FILE" || true', source)

    def test_stability_requires_exactly_one_verified_transition(self) -> None:
        source = self.SCRIPT.read_text()
        # Execute only the log assertion, never the native macOS setup/cleanup.
        assertion = "verified_transition_count=" + source.split("verified_transition_count=", 1)[1]
        assertion = assertion.split('\nstatus=', 1)[0]
        for count in (0, 1, 2):
            with self.subTest(count=count), tempfile.TemporaryDirectory(prefix="bifrost-stability-") as tmp:
                logs = Path(tmp) / "logs"
                logs.mkdir()
                log = logs / "bifrost.test.log"
                log.write_text((self.SIGNAL + "\n") * count + "healthy no-op cycle\n" * 2)
                result = subprocess.run(
                    [BASH, "-c", "set -euo pipefail\n" + assertion],
                    env=dict(os.environ, BIFROST_DATA_DIR=tmp, PROXY_LOG=str(log)),
                    text=True, capture_output=True, timeout=10,
                )
                self.assertEqual(result.returncode, 0 if count == 1 else 1,
                                 result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
