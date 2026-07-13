from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check-e2e-capabilities.py"
SPEC = importlib.util.spec_from_file_location("check_e2e_capabilities", SCRIPT)
assert SPEC and SPEC.loader
capabilities = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(capabilities)


class CapabilityContractTests(unittest.TestCase):
    def test_valid_p0_contract(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "unit.rs").write_text("test", encoding="utf-8")
            (root / "e2e.sh").write_text("test", encoding="utf-8")
            errors = capabilities.validate(
                {
                    "schema_version": 1,
                    "capabilities": [
                        {
                            "id": "proxy",
                            "criticality": "p0",
                            "owner": "team",
                            "layers": ["unit", "integration", "e2e"],
                            "platforms": ["linux", "macos", "windows"],
                            "failure_modes": ["reset"],
                            "evidence": ["unit.rs", "e2e.sh"],
                        }
                    ],
                },
                root,
            )
        self.assertEqual(errors, [])

    def test_p0_missing_dimension_and_evidence_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            errors = capabilities.validate(
                {
                    "schema_version": 1,
                    "capabilities": [
                        {
                            "id": "proxy",
                            "criticality": "p0",
                            "owner": "team",
                            "layers": ["unit"],
                            "platforms": ["linux"],
                            "failure_modes": [],
                            "evidence": ["missing.rs"],
                        }
                    ],
                },
                Path(tmp),
            )
        self.assertTrue(any("requires unit, integration" in error for error in errors))
        self.assertTrue(any("requires linux, macos" in error for error in errors))
        self.assertTrue(any("failure mode" in error for error in errors))
        self.assertTrue(any("missing evidence" in error for error in errors))

    def test_duplicate_ids_are_rejected(self) -> None:
        entry = {
            "id": "duplicate",
            "criticality": "p2",
            "owner": "team",
            "layers": ["unit"],
            "platforms": ["linux"],
            "failure_modes": ["error"],
            "evidence": ["a", "b"],
        }
        errors = capabilities.validate(
            {"schema_version": 1, "capabilities": [entry, entry]}, Path("/")
        )
        self.assertIn("duplicate: duplicate id", errors)


if __name__ == "__main__":
    unittest.main()
