import importlib.util
import stat
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "coverage-sanitize-profraw.py"
COVERAGE_ALL = SCRIPT.with_name("coverage-all.sh")
SPEC = importlib.util.spec_from_file_location("coverage_sanitize_profraw", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class CoverageProfileSanitizerTests(unittest.TestCase):
    def test_unified_coverage_sanitizes_unit_profiles_before_first_report(self):
        source = COVERAGE_ALL.read_text(encoding="utf-8")
        unit_sanitize = source.index(
            'sanitize_profiles "$PROFILE_ROOT" "unit-integration-profile-sanitizer.json"'
        )
        first_report = source.index(
            'cargo llvm-cov report --json --output-path "$OUTPUT_DIR/unit-integration.json"'
        )
        self.assertLess(unit_sanitize, first_report)
        self.assertIn(
            'sanitize_profiles "$e2e_profiles" "e2e-profile-sanitizer.json"',
            source,
        )

    def test_invalid_profiles_are_quarantined_without_touching_valid_profiles(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            valid = root / "valid.profraw"
            invalid = root / "invalid.profraw"
            valid.write_bytes(b"valid")
            invalid.write_bytes(b"truncated")
            tool = root / "llvm-profdata"
            tool.write_text(
                "#!/bin/sh\ncase \"$2\" in *invalid*) exit 1;; *) exit 0;; esac\n",
                encoding="utf-8",
            )
            tool.chmod(tool.stat().st_mode | stat.S_IXUSR)

            result = MODULE.sanitize(root, str(tool))

            self.assertEqual(result["checked"], 2)
            self.assertEqual(result["quarantined"], 1)
            self.assertTrue(valid.exists())
            self.assertFalse(invalid.exists())
            self.assertTrue((root / "corrupt" / "invalid.profraw").exists())

    def test_existing_quarantine_is_not_revalidated(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            quarantine = root / "corrupt"
            quarantine.mkdir()
            (quarantine / "old.profraw").write_bytes(b"old")
            tool = root / "llvm-profdata"
            tool.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
            tool.chmod(tool.stat().st_mode | stat.S_IXUSR)

            result = MODULE.sanitize(root, str(tool))

            self.assertEqual(result["checked"], 1)
            self.assertEqual(result["quarantined"], 0)


if __name__ == "__main__":
    unittest.main()
