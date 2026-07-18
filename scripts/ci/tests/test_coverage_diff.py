from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "coverage-diff.py"
SPEC = importlib.util.spec_from_file_location("coverage_diff", SCRIPT)
assert SPEC and SPEC.loader
coverage_diff = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage_diff)


class CoverageDiffTests(unittest.TestCase):
    def test_parse_diff_tracks_only_added_ranges(self) -> None:
        diff = """diff --git a/crates/a/src/lib.rs b/crates/a/src/lib.rs
+++ b/crates/a/src/lib.rs
@@ -10,0 +11,3 @@
+one
+two
+three
@@ -30,2 +34 @@
+four
"""
        self.assertEqual(
            coverage_diff.parse_diff(diff),
            {"crates/a/src/lib.rs": {11, 12, 13, 34}},
        )

    def test_parse_lcov_normalizes_absolute_workspace_paths(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            report = coverage_diff.parse_lcov(
                f"SF:{root}/crates/a/src/lib.rs\nDA:4,1\nDA:5,0\nend_of_record\n",
                root,
            )
        self.assertEqual(report, {"crates/a/src/lib.rs": {4: 1, 5: 0}})

    def test_evaluate_counts_only_instrumentable_production_lines(self) -> None:
        changed = {
            "crates/a/src/lib.rs": {4, 5, 6},
            "crates/a/tests/integration.rs": {1},
            "docs/guide.md": {1},
        }
        coverage = {"crates/a/src/lib.rs": {4: 1, 5: 0}}
        result = coverage_diff.evaluate_changed_coverage(changed, coverage)
        self.assertEqual(result["covered"], 1)
        self.assertEqual(result["total"], 2)
        self.assertEqual(result["percent"], 50.0)
        self.assertEqual(result["files"][0]["missed_lines"], [5])

    def test_no_coverable_changes_reports_full_coverage(self) -> None:
        result = coverage_diff.evaluate_changed_coverage(
            {"design/coverage-90.md": {1, 2}}, {}
        )
        self.assertEqual(result["total"], 0)
        self.assertEqual(result["percent"], 100.0)

    def test_inline_cfg_test_module_lines_are_excluded(self) -> None:
        source = """pub fn production() {\n    println!(\"live\");\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn production_works() {\n        super::production();\n    }\n}\n"""
        self.assertEqual(
            coverage_diff.rust_test_module_lines(source), set(range(5, 12))
        )

    def test_exclude_inline_modules_keeps_production_changes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "crates/a/src/lib.rs"
            path.parent.mkdir(parents=True)
            path.write_text(
                "pub fn production() {}\n#[cfg(test)]\nmod tests {\n fn test() {}\n}\n",
                encoding="utf-8",
            )
            filtered = coverage_diff.exclude_inline_test_modules(
                {"crates/a/src/lib.rs": {1, 2, 3, 4, 5}}, root
            )
        self.assertEqual(filtered, {"crates/a/src/lib.rs": {1}})

    def test_substantial_unchanged_moved_block_is_excluded(self) -> None:
        base = """fn download() {
    let client = client();
    let response = client.send();
    if response.is_ok() {
        save(response);
    } else {
        retry(response);
    }
}
"""
        current = base + "new_behavior();\n"
        moved = coverage_diff.unchanged_moved_block_lines(current, [(base, "")])
        self.assertEqual(moved, set(range(1, 10)))
        self.assertNotIn(10, moved)

    def test_small_boilerplate_match_remains_changed(self) -> None:
        source = """if ready {
    return Ok(());
}
"""
        self.assertEqual(
            coverage_diff.unchanged_moved_block_lines(source, [(source, "")]), set()
        )

    def test_exclude_moved_blocks_reports_only_changed_intersection(self) -> None:
        base = """fn restart() {
    let runtime = read_runtime();
    validate(runtime);
    stop(runtime);
    wait_for_exit(runtime);
    install(runtime);
    start(runtime);
    verify(runtime);
}
"""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "crates/a/src/restart.rs"
            path.parent.mkdir(parents=True)
            path.write_text(base + "new_behavior();\n", encoding="utf-8")
            filtered, count = coverage_diff.exclude_unchanged_moved_blocks(
                {"crates/a/src/restart.rs": set(range(1, 11))}, [(base, "")], root
            )
        self.assertEqual(filtered, {"crates/a/src/restart.rs": {10}})
        self.assertEqual(count, 9)

    def test_copied_block_still_present_in_source_remains_changed(self) -> None:
        base = "\n".join(f"let value_{index} = {index};" for index in range(12))
        moved = coverage_diff.unchanged_moved_block_lines(base, [(base, base)])
        self.assertEqual(moved, set())

    def test_retained_middle_block_does_not_create_small_moved_fragments(self) -> None:
        lines = [f"let value_{index} = {index};" for index in range(20)]
        base = "\n".join(lines)
        original_current = "\n".join(lines[6:14])
        moved = coverage_diff.unchanged_moved_block_lines(
            base, [(base, original_current)]
        )
        self.assertEqual(moved, set())


if __name__ == "__main__":
    unittest.main()
