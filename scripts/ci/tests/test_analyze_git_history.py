from __future__ import annotations

import datetime
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock
from zoneinfo import ZoneInfo


SCRIPT = Path(__file__).resolve().parents[2] / "analyze_git_history.py"
SPEC = importlib.util.spec_from_file_location("analyze_git_history", SCRIPT)
assert SPEC and SPEC.loader
history = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = history
SPEC.loader.exec_module(history)


class GitHistoryReportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.repo = Path(self.temp_dir.name)
        self.git("init")
        self.git("config", "user.name", "Test Author")
        self.git("config", "user.email", "test@example.com")
        self.git("branch", "-M", "main")

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def git(self, *args: str, env: dict[str, str] | None = None) -> str:
        merged_env = os.environ.copy()
        if env:
            merged_env.update(env)
        result = subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            capture_output=True,
            text=True,
            env=merged_env,
        )
        return result.stdout.strip()

    def commit(self, filename: str, subject: str, author_at: str, committer_at: str | None = None) -> None:
        (self.repo / filename).write_text(subject, encoding="utf-8")
        self.git("add", filename)
        self.git(
            "commit",
            "-m",
            subject,
            env={
                "GIT_AUTHOR_DATE": author_at,
                "GIT_COMMITTER_DATE": committer_at or author_at,
            },
        )

    def build_history_with_merge(self) -> None:
        self.commit(
            "root.txt",
            "feat: root",
            "2026-01-31T16:30:00+00:00",
            "2026-01-31T14:00:00+00:00",
        )
        self.git("switch", "-c", "feature")
        self.commit("feature.txt", "fix: feature", "2026-02-02T16:00:00+00:00")
        self.git("switch", "main")
        self.commit("main.txt", "docs: main", "2026-02-01T16:00:00+00:00")
        self.git(
            "merge",
            "--no-ff",
            "feature",
            "-m",
            "merge: feature",
            env={
                "GIT_AUTHOR_DATE": "2026-02-03T16:00:00+00:00",
                "GIT_COMMITTER_DATE": "2026-02-03T16:00:00+00:00",
            },
        )

    def test_load_and_render_include_timezone_zero_days_and_merge(self) -> None:
        self.build_history_with_merge()
        resolved, commits = history.load_commits(self.repo, "HEAD")
        report = history.render_report(
            commits,
            resolved,
            "HEAD",
            "author",
            ZoneInfo("Asia/Shanghai"),
            "Asia/Shanghai",
            2026,
            2,
            3,
        )

        self.assertEqual(len(commits), 4)
        self.assertEqual(sum(commit.is_merge for commit in commits), 1)
        self.assertIn("| 2026-02-01 | 周日 | 1 | 1 | 0 |", report)
        self.assertIn("| 2026-02-02 | 周一 | 1 | 1 | 0 |", report)
        self.assertIn("| 2026-02-03 | 周二 | 1 | 1 | 0 |", report)
        self.assertIn("| 2026-02-04 | 周三 | 1 | 0 | 1 |", report)
        self.assertIn("| 2026-02-05 | 周四 | 0 | 0 | 0 |", report)
        self.assertIn("**3 个 commit**", report)
        self.assertIn("**3/3 天有提交**", report)

    def test_committer_date_can_be_selected_independently(self) -> None:
        self.commit(
            "root.txt",
            "feat: root",
            "2026-01-31T16:30:00+00:00",
            "2026-01-31T14:00:00+00:00",
        )
        _, commits = history.load_commits(self.repo, "HEAD")
        timezone = ZoneInfo("Asia/Shanghai")
        self.assertEqual(
            history.commit_datetime(commits[0], "author", timezone).date().isoformat(),
            "2026-02-01",
        )
        self.assertEqual(
            history.commit_datetime(commits[0], "committer", timezone).date().isoformat(),
            "2026-01-31",
        )

    def test_markdown_table_escapes_repository_text(self) -> None:
        table = history.markdown_table(["Name"], [["pipe | value\nnext"]])
        self.assertEqual(table[-1], "| pipe \\| value next |")

    def test_default_timezone_has_fixed_offset_fallback(self) -> None:
        with mock.patch.object(
            history, "ZoneInfo", side_effect=history.ZoneInfoNotFoundError("missing")
        ):
            timezone = history.resolve_timezone("Asia/Shanghai")
        offset = datetime.datetime(2026, 2, 1, tzinfo=timezone).utcoffset()
        self.assertEqual(offset, datetime.timedelta(hours=8))

    def test_invalid_revision_returns_error(self) -> None:
        self.commit("root.txt", "feat: root", "2026-02-01T00:00:00+00:00")
        result = subprocess.run(
            [
                "python3",
                str(SCRIPT),
                "--repo",
                str(self.repo),
                "--revision",
                "missing",
                "--year",
                "2026",
                "--month",
                "2",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("error: git rev-parse", result.stderr)


if __name__ == "__main__":
    unittest.main()
