#!/usr/bin/env python3
import json
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "coverage-gate.py"


class CoverageGateTests(unittest.TestCase):
    def test_production_metric_is_delegated_to_lcov_gate(self):
        document = {
            "data": [{"files": [{
                "filename": "/repo/crates/bifrost-proxy/src/lib.rs",
                "summary": {"lines": {"covered": 10, "count": 100}},
            }]}]
        }
        with tempfile.TemporaryDirectory() as temp_dir:
            coverage = Path(temp_dir) / "coverage.json"
            thresholds = Path(temp_dir) / "thresholds.toml"
            coverage.write_text(json.dumps(document), encoding="utf-8")
            thresholds.write_text(
                "[settings]\ndefault = 90.0\nworkspace_min = 0.0\n"
                "[crates.bifrost-proxy]\nmin = 90.0\nmetric = \"production\"\n",
                encoding="utf-8",
            )
            result = subprocess.run(
                ["python3", str(SCRIPT), str(coverage), "--thresholds", str(thresholds)],
                check=False, capture_output=True, text=True,
            )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PRODUCTION-LCOV", result.stdout)

    def test_crate_filter_ignores_instrumented_dependencies(self):
        document = {
            "data": [{
                "files": [
                    {
                        "filename": "/repo/crates/bifrost-proxy/src/lib.rs",
                        "summary": {"lines": {"covered": 90, "count": 100}},
                    },
                    {
                        "filename": "/repo/crates/bifrost-admin/src/lib.rs",
                        "summary": {"lines": {"covered": 0, "count": 100}},
                    },
                ]
            }]
        }
        with tempfile.TemporaryDirectory() as temp_dir:
            coverage = Path(temp_dir) / "coverage.json"
            thresholds = Path(temp_dir) / "thresholds.toml"
            coverage.write_text(json.dumps(document), encoding="utf-8")
            thresholds.write_text(
                "[settings]\ndefault = 90.0\nworkspace_min = 90.0\n"
                "[crates.bifrost-proxy]\nmin = 90.0\n"
                "[crates.bifrost-admin]\nmin = 90.0\n",
                encoding="utf-8",
            )
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    str(coverage),
                    "--thresholds",
                    str(thresholds),
                    "--single-crate",
                    "--crate",
                    "bifrost-proxy",
                ],
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("bifrost-proxy", result.stdout)
        self.assertNotIn("bifrost-admin", result.stdout)


if __name__ == "__main__":
    unittest.main()
