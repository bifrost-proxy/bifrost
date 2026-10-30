from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "sync-cargo-lockfiles.sh"


class CargoLockfileSyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cargo lock fixture ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "project"
        self.external = self.root / "external"
        self.repo.mkdir()
        self.external.mkdir()
        self.cargo = shutil.which("cargo")
        self.assertIsNotNone(self.cargo, "cargo is required for the real resolver regression")
        self.git("init", "-q")
        self.git("config", "user.name", "Lockfile Test")
        self.git("config", "user.email", "lockfile-test@example.invalid")
        self.write(self.external / "src/lib.rs", "pub fn value() -> u8 { 1 }\n")
        self.external_version("1.0.0")
        self.git("add", ".")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "-qm", "first external release")
        self.write(self.repo / "Cargo.toml", '[workspace]\nmembers=["core"]\nresolver="2"\n[workspace.package]\nversion="1.0.0"\n')
        self.write(self.repo / "core/Cargo.toml", '[package]\nname="local-core"\nversion.workspace=true\nedition="2021"\n[dependencies]\nexternal={git="' + self.external.as_uri() + '",version="1"}\n')
        self.write(self.repo / "core/src/lib.rs", "pub fn value() -> u8 { external::value() }\n")
        self.write(self.repo / "desktop/src-tauri/Cargo.toml", '[package]\nname="local-desktop"\nversion="1.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nlocal-core={path="../../core"}\n')
        self.write(self.repo / "desktop/src-tauri/src/main.rs", "fn main() { assert_eq!(local_core::value(), 1); }\n")
        for manifest in self.manifests():
            self.run_cargo("generate-lockfile", "--manifest-path", str(manifest))

    def write(self, path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.external, text=True).strip()

    def external_version(self, version):
        self.write(self.external / "Cargo.toml", f'[package]\nname="external"\nversion="{version}"\nedition="2021"\n')

    def manifests(self):
        return [self.repo / "Cargo.toml", self.repo / "desktop/src-tauri/Cargo.toml"]

    def locks(self):
        return [p.with_name("Cargo.lock") for p in self.manifests()]

    def run_cargo(self, *args, succeeds=True):
        result = subprocess.run([self.cargo, *args], cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)
        return result

    def helper(self, check=False, succeeds=True):
        args = ["bash", str(SCRIPT)] + (["--check"] if check else []) + [str(self.repo)]
        result = subprocess.run(args, cwd=self.repo, text=True, capture_output=True)
        self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)
        return result

    def test_real_version_bump_checks_both_workspaces_without_upgrading_external_git(self):
        before = [tomllib.loads(p.read_text()) for p in self.locks()]
        external_before = [next(p for p in lock["package"] if p["name"] == "external") for lock in before]
        # A newer compatible external revision exists, but a version-only
        # release bump must leave the previously locked revision untouched.
        self.external_version("1.1.0")
        self.git("add", ".")
        self.git("-c", "core.hooksPath=/dev/null", "commit", "-qm", "new compatible external release")
        for manifest in self.manifests():
            manifest.write_text(manifest.read_text().replace('version="1.0.0"', 'version="2.0.0"'))
        stale = [p.read_bytes() for p in self.locks()]
        for manifest in self.manifests():
            failure = self.run_cargo("check", "--locked", "--offline", "--manifest-path", str(manifest), succeeds=False)
            self.assertIn("lock", failure.stderr.lower())
        self.helper(check=True, succeeds=False)
        self.assertEqual(stale, [p.read_bytes() for p in self.locks()])
        # A fresh root must not conceal a stale standalone desktop lockfile.
        self.run_cargo("update", "--workspace", "--manifest-path", str(self.manifests()[0]))
        failure = self.helper(check=True, succeeds=False)
        self.assertIn("desktop/src-tauri", failure.stdout + failure.stderr)
        self.assertEqual(stale[1], self.locks()[1].read_bytes())
        self.helper()
        self.helper(check=True)
        for index, manifest in enumerate(self.manifests()):
            after = tomllib.loads(self.locks()[index].read_text())
            self.assertEqual(external_before[index], next(p for p in after["package"] if p["name"] == "external"))
            self.assertTrue(all(p["version"] == "2.0.0" for p in after["package"] if p["name"].startswith("local-")))
            self.run_cargo("check", "--locked", "--offline", "--manifest-path", str(manifest))
        self.run_cargo("run", "--locked", "--offline", "--manifest-path", str(self.manifests()[1]))
        stable = [p.read_bytes() for p in self.locks()]
        self.helper()
        self.assertEqual(stable, [p.read_bytes() for p in self.locks()])

    def test_release_sync_precedes_commit_and_tag_and_ci_checks_clean_checkout(self):
        root = SCRIPT.parents[2]
        release = (root / ".github/workflows/release.yml").read_text()
        update = release.index("- name: Update Cargo.toml versions")
        sync = release.index("          bash scripts/ci/sync-cargo-lockfiles.sh\n", update)
        check = release.index("          bash scripts/ci/sync-cargo-lockfiles.sh --check\n", sync)
        self.assertLess(sync, check)
        self.assertLess(check, release.index("git add -A", update))
        self.assertLess(sync, release.index("git add -A", update))
        self.assertLess(sync, release.index("- name: Create and push tag", update))
        ci = (root / ".github/workflows/ci.yml").read_text()
        self.assertIn("bash scripts/ci/sync-cargo-lockfiles.sh --check", ci)


if __name__ == "__main__":
    unittest.main()
