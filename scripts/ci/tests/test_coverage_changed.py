from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "coverage-changed.py"
SPEC = importlib.util.spec_from_file_location("coverage_changed", SCRIPT)
assert SPEC and SPEC.loader
coverage_changed = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage_changed)


class CoverageChangedTests(unittest.TestCase):
    def test_production_path_filter_verifies_split_test_modules(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "crates/a/src"
            (source / "feature/tests").mkdir(parents=True)
            (source / "support/suite").mkdir(parents=True)
            (source / "lib.rs").write_text(
                '#[cfg(test)]\nmod tests;\n#[cfg(test)]\n#[path = "support/suite.rs"]\nmod explicit_tests;\nmod runtime_tests;\n',
                encoding="utf-8",
            )
            (source / "tests.rs").write_text("fn helper() {}\n", encoding="utf-8")
            (source / "support/suite.rs").write_text(
                "fn explicit_helper() {}\n", encoding="utf-8"
            )
            (source / "support/suite/case.rs").write_text(
                "fn nested_helper() {}\n", encoding="utf-8"
            )
            (source / "runtime_tests.rs").write_text(
                "pub fn live() {}\n", encoding="utf-8"
            )
            (source / "feature.rs").write_text(
                '#[cfg(test)]\n#[path = "feature/tests.rs"]\nmod tests;\n',
                encoding="utf-8",
            )
            (source / "feature/tests/group_flow.rs").write_text(
                "fn helper() {}\n", encoding="utf-8"
            )

            self.assertTrue(
                coverage_changed.is_production_rust_path(
                    "crates/a/src/runtime_tests.rs", root
                )
            )
            self.assertFalse(
                coverage_changed.is_production_rust_path("crates/a/src/tests.rs", root)
            )
            self.assertFalse(
                coverage_changed.is_production_rust_path(
                    "crates/a/src/support/suite.rs", root
                )
            )
            self.assertFalse(
                coverage_changed.is_production_rust_path(
                    "crates/a/src/support/suite/case.rs", root
                )
            )
            self.assertFalse(
                coverage_changed.is_production_rust_path(
                    "crates/a/src/feature/tests/group_flow.rs", root
                )
            )

    def test_packages_for_paths_uses_workspace_manifest_roots(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            metadata = {
                "workspace_members": ["alpha-id", "beta-id"],
                "packages": [
                    {
                        "id": "alpha-id",
                        "name": "alpha",
                        "manifest_path": str(root / "crates/alpha/Cargo.toml"),
                    },
                    {
                        "id": "beta-id",
                        "name": "beta",
                        "manifest_path": str(root / "crates/beta/Cargo.toml"),
                    },
                    {
                        "id": "dependency-id",
                        "name": "dependency",
                        "manifest_path": str(root / "vendor/dependency/Cargo.toml"),
                    },
                ],
            }
            selected = coverage_changed.packages_for_paths(
                ["crates/beta/src/nested/mod.rs", "crates/alpha/src/lib.rs"],
                metadata,
                root,
            )
        self.assertEqual(selected, ["alpha", "beta"])

    def test_packages_for_paths_rejects_unmapped_changed_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            with self.assertRaisesRegex(
                coverage_changed.PreflightError, "cannot map changed Rust files"
            ):
                coverage_changed.packages_for_paths(
                    ["crates/missing/src/lib.rs"],
                    {"workspace_members": [], "packages": []},
                    root,
                )

    def test_changed_lines_threshold_reads_authoritative_setting(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "thresholds.toml"
            path.write_text(
                "[settings]\nchanged_lines_min = 95.5 # local and CI\n",
                encoding="utf-8",
            )
            self.assertEqual(coverage_changed.changed_lines_threshold(path), 95.5)

    def test_coverage_command_selects_only_requested_packages_and_filter(self) -> None:
        command = coverage_changed.coverage_command(
            ["alpha", "beta"],
            output_path=Path("target/changed/lcov.info"),
            jobs=3,
            test_filter="new_behavior",
        )
        self.assertEqual(
            command,
            [
                "cargo",
                "llvm-cov",
                "--no-clean",
                "--package",
                "alpha",
                "--package",
                "beta",
                "--all-features",
                "--lcov",
                "--output-path",
                "target/changed/lcov.info",
                "--jobs",
                "3",
                "--",
                "new_behavior",
            ],
        )

    def test_clear_profiles_keeps_compiled_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp)
            profile = target / "nested/run.profraw"
            artifact = target / "debug/libalpha.rlib"
            profile.parent.mkdir(parents=True)
            artifact.parent.mkdir(parents=True)
            profile.write_bytes(b"profile")
            artifact.write_bytes(b"artifact")
            self.assertEqual(coverage_changed.clear_profiles(target), 1)
            self.assertFalse(profile.exists())
            self.assertTrue(artifact.exists())


if __name__ == "__main__":
    unittest.main()
