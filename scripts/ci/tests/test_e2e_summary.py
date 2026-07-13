from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "e2e-summary.py"
SPEC = importlib.util.spec_from_file_location("e2e_summary", SCRIPT)
assert SPEC and SPEC.loader
e2e_summary = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(e2e_summary)


class E2eSummaryTests(unittest.TestCase):
    def test_report_preserves_results_and_aggregates_counts(self) -> None:
        report = e2e_summary.build_report(
            [
                ["passed", "rules:core", "12", "rules.log", ""],
                ["failed", "shell:remote", "7", "remote.log", "exit 1"],
                ["skipped", "ui:playwright", "0", "", "not selected"],
            ],
            {"platform": "Linux", "mode": "ci"},
        )
        self.assertEqual(
            report["counts"],
            {"total": 3, "passed": 1, "failed": 1, "skipped": 1},
        )
        self.assertEqual(report["suites"][1]["reason"], "exit 1")
        self.assertEqual(report["metadata"]["platform"], "Linux")

    def test_invalid_status_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "unknown suite status"):
            e2e_summary.build_report([["unknown", "suite", "1", "", ""]], {})

    def test_invalid_duration_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "invalid duration"):
            e2e_summary.build_report([["passed", "suite", "soon", "", ""]], {})


if __name__ == "__main__":
    unittest.main()
