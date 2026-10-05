from __future__ import annotations

import json
import os
import subprocess
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
CONTRACT = ROOT / "e2e-tests/tests/test_coverage_pipeline_contract.sh"


class CoverageFixtureEnvironmentTests(unittest.TestCase):
    def test_nested_fixture_clears_instrumentation_without_changing_outer_env(self):
        source = CONTRACT.read_text(encoding="utf-8")
        prefix = source.split("run_changed_coverage_fixture() (\n", 1)[1]
        reset = prefix.split('  fixture_dir="$(mktemp -d)"', 1)[0]
        instrumentation = {
            name: "outer-coverage-sentinel"
            for name in (
                "CARGO_LLVM_COV",
                "CARGO_LLVM_COV_TARGET_DIR",
                "__CARGO_LLVM_COV_RUSTC_WRAPPER_PRE_EXISTING",
                "__CARGO_LLVM_COV_RUSTC_WRAPPER_TEST",
                "LLVM_PROFILE_FILE",
                "CARGO_TARGET_DIR",
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "RUSTDOCFLAGS",
                "CARGO_ENCODED_RUSTDOCFLAGS",
            )
        }
        preserved = {
            "PATH": os.environ["PATH"],
            "CARGO_HOME": "fixture-cargo-home",
            "RUSTUP_HOME": "fixture-rustup-home",
            "BIFROST_COVERAGE_E2E": "1",
        }
        env = {**preserved, **instrumentation}
        keys = list(env)
        probe = (
            "import json, os; "
            f"print(json.dumps({{k: os.environ[k] for k in {keys!r} if k in os.environ}}))"
        )
        quoted_probe = "'" + probe.replace("'", "'\\''") + "'"
        synthetic = source.split('  chmod +x "$sentinel_wrapper"\n', 1)[1]
        synthetic = synthetic.split('\nelse\n  echo "Coverage changed-lines', 1)[0]
        for invocation in ("run_changed_coverage_fixture", synthetic):
            with self.subTest(synthetic=invocation == synthetic):
                script = (
                    "set -eu\n"
                    "sentinel_wrapper=sentinel-wrapper\npartition_dir=sentinel-partition\n"
                    f"run_changed_coverage_fixture() (\n{reset}\npython3 -c {quoted_probe}\n)\n"
                    f"{invocation}\npython3 -c {quoted_probe}"
                )
                result = subprocess.run(
                    ["bash", "-c", script],
                    env=env,
                    text=True,
                    capture_output=True,
                    check=True,
                )
                inner, outer = map(json.loads, result.stdout.splitlines())
                self.assertEqual(inner, preserved)
                self.assertEqual(outer, env)


if __name__ == "__main__":
    unittest.main()
