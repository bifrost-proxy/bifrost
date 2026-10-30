import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SELECTOR = Path(__file__).resolve().parents[1] / "previous-bifrost-release-tag.py"


class ReleaseTagTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git("init", "-q")
        self.git("config", "user.name", "Release Test")
        self.git("config", "user.email", "release-test@example.invalid")
        self.commit("initial")

    def git(self, *arguments):
        return subprocess.check_output(["git", *arguments], cwd=self.root, text=True).strip()

    def commit(self, message):
        self.git("-c", "core.hooksPath=/dev/null", "commit", "--allow-empty", "-qm", message)

    def select(self, revision="HEAD"):
        return subprocess.check_output(
            [os.sys.executable, str(SELECTOR), revision], cwd=self.root, text=True
        ).strip()

    def test_runtime_tag_does_not_change_version_or_changelog_baseline(self):
        self.git("tag", "v0.0.201")
        self.commit("fix: feature from merged PR")
        self.git("tag", "moss-runtime-v1.0.1")
        self.assertEqual(self.git("describe", "--tags", "--abbrev=0"), "moss-runtime-v1.0.1")
        self.assertEqual(self.select(), "v0.0.201")
        self.commit("chore: bump version")
        self.git("tag", "v0.0.202")
        self.assertEqual(self.select("v0.0.202^"), "v0.0.201")
        self.assertIn("feature from merged PR", self.git("log", f"{self.select('v0.0.202^')}..v0.0.202", "--format=%s"))

    def test_rejects_malformed_and_unreachable_tags(self):
        self.git("tag", "v1.2.3")
        self.commit("later")
        for tag in ("v999.2.3junk", "v01.2.3", "v1.2", "vmoss-runtime-v1.0.2"):
            self.git("tag", tag)
        self.assertEqual(self.select(), "v1.2.3")
        self.git("checkout", "-qb", "unmerged")
        self.commit("unmerged release")
        self.git("tag", "v99.0.0")
        self.git("checkout", "-q", "-")
        self.assertEqual(self.select(), "v1.2.3")

    def test_prerelease_and_no_bifrost_tag(self):
        self.git("tag", "moss-runtime-v1.0.1")
        self.assertEqual(self.select(), "")
        self.git("tag", "v1.2.3-rc.1+build.2")
        self.assertEqual(self.select(), "v1.2.3-rc.1+build.2")

    def test_workflow_uses_selector_for_version_and_changelog(self):
        workflow = SELECTOR.parents[1].parent / ".github/workflows/release.yml"
        text = workflow.read_text()
        self.assertIn("LATEST_TAG=$(python3 scripts/ci/previous-bifrost-release-tag.py)", text)
        self.assertIn('PREV_TAG=$(python3 scripts/ci/previous-bifrost-release-tag.py "${TAG}^")', text)
        self.assertNotIn("git describe --tags --abbrev=0", text)


if __name__ == "__main__":
    unittest.main()
