from __future__ import annotations

import importlib.util
import tempfile
import unittest
import subprocess
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "coverage-production.py"
SPEC = importlib.util.spec_from_file_location("coverage_production", SCRIPT)
assert SPEC and SPEC.loader
coverage_production = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage_production)


class CoverageProductionTests(unittest.TestCase):
    def test_cli_reads_crate_floor_from_threshold_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            lcov = root / "coverage.lcov"
            thresholds = root / "thresholds.toml"
            lcov.write_text(
                f"SF:{Path.cwd() / 'crates/bifrost-proxy/src/lib.rs'}\nDA:1,1\nend_of_record\n",
                encoding="utf-8",
            )
            thresholds.write_text(
                "[settings]\ndefault = 99.0\n"
                "[crates.bifrost-proxy]\nmin = 90.0\nmetric = \"production\"\n",
                encoding="utf-8",
            )
            result = subprocess.run(
                ["python3", str(SCRIPT), str(lcov), "--crate", "bifrost-proxy",
                 "--thresholds", str(thresholds)],
                check=False, capture_output=True, text=True,
            )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("for 90% [PASS]", result.stdout)

    def test_parse_lcov_merges_duplicate_line_records_by_max_hits(self) -> None:
        report = coverage_production.parse_lcov(
            "SF:crates/a/src/lib.rs\nDA:1,0\nDA:1,3\nDA:2,0\nend_of_record\n"
        )
        self.assertEqual(report["crates/a/src/lib.rs"], {1: 3, 2: 0})

    def test_evaluate_excludes_inline_test_module(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "crates/a/src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "pub fn live() {}\n#[cfg(test)]\nmod tests {\n fn helper() {}\n}\n",
                encoding="utf-8",
            )
            result = coverage_production.evaluate(
                {"crates/a/src/lib.rs": {1: 1, 2: 1, 3: 1, 4: 1, 5: 1}},
                root,
            )
        self.assertEqual(result["crates"]["a"]["covered"], 1)
        self.assertEqual(result["crates"]["a"]["total"], 1)

    def test_evaluate_excludes_external_exact_cfg_test_module(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "crates/a/src/lib.rs"
            external = root / "crates/a/src/tests.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "pub fn live() {}\n#[cfg(test)]\nmod tests;\n",
                encoding="utf-8",
            )
            external.write_text("fn helper() {}\n", encoding="utf-8")
            result = coverage_production.evaluate(
                {
                    "crates/a/src/lib.rs": {1: 1, 2: 1, 3: 1},
                    "crates/a/src/tests.rs": {1: 1},
                },
                root,
            )
        self.assertEqual(result["crates"]["a"]["covered"], 1)
        self.assertEqual(result["crates"]["a"]["total"], 1)

    def test_external_test_module_paths_resolve_nested_module_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            parent = root / "crates/a/src/nested.rs"
            external = root / "crates/a/src/nested/checks/mod.rs"
            external.parent.mkdir(parents=True)
            parent.write_text("#[cfg(test)]\npub(crate) mod checks;\n", encoding="utf-8")
            external.write_text("fn helper() {}\n", encoding="utf-8")
            resolved = coverage_production.external_test_module_paths(
                parent, parent.read_text(encoding="utf-8")
            )
        self.assertEqual(resolved, {external.resolve()})

    def test_crate_filter_and_uncovered_production_lines(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for crate in ("a", "b"):
                source = root / f"crates/{crate}/src/lib.rs"
                source.parent.mkdir(parents=True)
                source.write_text("pub fn one() {}\npub fn two() {}\n", encoding="utf-8")
            result = coverage_production.evaluate(
                {
                    "crates/a/src/lib.rs": {1: 2, 2: 0},
                    "crates/b/src/lib.rs": {1: 1, 2: 1},
                },
                root,
                crate_filter="a",
            )
        self.assertEqual(set(result["crates"]), {"a"})
        self.assertEqual(result["crates"]["a"]["percent"], 50.0)

    def test_evaluate_excludes_test_only_function_import_and_constant(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "crates/a/src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "#[cfg(test)]\n"
                "use std::path::Path;\n"
                "#[cfg(test)]\n"
                "const FIXTURE: &str = \"fixture\";\n"
                "#[cfg(test)]\n"
                "fn helper() {\n"
                "  let _ = \"{paired}\";\n"
                "}\n"
                "pub fn live() {}\n",
                encoding="utf-8",
            )
            result = coverage_production.evaluate(
                {"crates/a/src/lib.rs": {line: 0 for line in range(1, 10)}},
                root,
            )
        self.assertEqual(result["crates"]["a"]["total"], 1)
        self.assertEqual(result["crates"]["a"]["covered"], 0)


if __name__ == "__main__":
    unittest.main()
