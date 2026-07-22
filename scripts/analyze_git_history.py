#!/usr/bin/env python3
"""Generate a reproducible Markdown report from a Git revision's history."""

from __future__ import annotations

import argparse
import calendar
import re
import subprocess
import sys
from collections import Counter
from dataclasses import dataclass
from datetime import date, datetime, timedelta, timezone as fixed_timezone, tzinfo
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError


COMMIT_FORMAT = "%H%x09%P%x09%aI%x09%cI%x09%aN%x09%aE%x09%s"
CONVENTIONAL_TYPE_RE = re.compile(r"^([A-Za-z][A-Za-z0-9_-]*)(?:\([^)]*\))?!?:\s")


@dataclass(frozen=True)
class Commit:
    sha: str
    parents: tuple[str, ...]
    author_at: datetime
    committer_at: datetime
    author_name: str
    author_email: str
    subject: str

    @property
    def is_merge(self) -> bool:
        return len(self.parents) > 1

    @property
    def author(self) -> str:
        return f"{self.author_name} <{self.author_email}>"


def run_git(repo: Path, args: list[str]) -> str:
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"git {' '.join(args)} failed: {detail}")
    return result.stdout


def parse_iso_datetime(value: str) -> datetime:
    normalized = value[:-1] + "+00:00" if value.endswith("Z") else value
    parsed = datetime.fromisoformat(normalized)
    if parsed.tzinfo is None:
        raise ValueError(f"Git date has no timezone: {value}")
    return parsed


def load_commits(repo: Path, revision: str) -> tuple[str, list[Commit]]:
    resolved = run_git(repo, ["rev-parse", "--verify", f"{revision}^{{commit}}"]).strip()
    raw = run_git(repo, ["log", "--topo-order", f"--format={COMMIT_FORMAT}", resolved])
    commits: list[Commit] = []
    for line_number, line in enumerate(raw.splitlines(), start=1):
        fields = line.split("\t", 6)
        if len(fields) != 7:
            raise ValueError(f"Unexpected git log row {line_number}: {line!r}")
        sha, parents, author_at, committer_at, name, email, subject = fields
        commits.append(
            Commit(
                sha=sha,
                parents=tuple(parents.split()),
                author_at=parse_iso_datetime(author_at),
                committer_at=parse_iso_datetime(committer_at),
                author_name=name,
                author_email=email,
                subject=subject,
            )
        )
    if not commits:
        raise ValueError(f"Revision {revision!r} has no commits")
    return resolved, commits


def resolve_timezone(name: str) -> tzinfo:
    try:
        return ZoneInfo(name)
    except ZoneInfoNotFoundError:
        fallbacks = {
            "Asia/Shanghai": fixed_timezone(timedelta(hours=8), "Asia/Shanghai"),
            "UTC": fixed_timezone.utc,
            "Etc/UTC": fixed_timezone.utc,
        }
        if name in fallbacks:
            return fallbacks[name]
        raise


def commit_datetime(commit: Commit, field: str, timezone: tzinfo) -> datetime:
    source = commit.author_at if field == "author" else commit.committer_at
    return source.astimezone(timezone)


def percent(part: int, total: int) -> str:
    return f"{part / total * 100:.1f}%" if total else "0.0%"


def markdown_table(headers: list[str], rows: list[list[object]]) -> list[str]:
    def cell(value: object) -> str:
        return str(value).replace("|", "\\|").replace("\n", " ")

    rendered = ["| " + " | ".join(cell(header) for header in headers) + " |"]
    rendered.append("| " + " | ".join("---" for _ in headers) + " |")
    rendered.extend("| " + " | ".join(cell(value) for value in row) + " |" for row in rows)
    return rendered


def date_range(start: date, end: date):
    current = start
    while current <= end:
        yield current
        current += timedelta(days=1)


def select_between(
    commits: list[Commit],
    field: str,
    timezone: tzinfo,
    start: date,
    end: date,
) -> list[Commit]:
    return [
        commit
        for commit in commits
        if start <= commit_datetime(commit, field, timezone).date() <= end
    ]


def render_report(
    commits: list[Commit],
    resolved_revision: str,
    revision_label: str,
    field: str,
    timezone: tzinfo,
    timezone_name: str,
    year: int,
    month: int,
    initial_days: int,
) -> str:
    dated = sorted(commits, key=lambda item: commit_datetime(item, field, timezone))
    first = dated[0]
    latest = dated[-1]
    first_day = commit_datetime(first, field, timezone).date()
    latest_day = commit_datetime(latest, field, timezone).date()
    initial_end = first_day + timedelta(days=initial_days - 1)
    initial = select_between(commits, field, timezone, first_day, initial_end)

    month_start = date(year, month, 1)
    month_end = date(year, month, calendar.monthrange(year, month)[1])
    focused = select_between(commits, field, timezone, month_start, month_end)
    focused_daily = Counter(commit_datetime(item, field, timezone).date() for item in focused)
    merge_daily = Counter(
        commit_datetime(item, field, timezone).date() for item in focused if item.is_merge
    )
    active_days = sum(count > 0 for count in focused_daily.values())

    monthly = Counter(
        commit_datetime(item, field, timezone).strftime("%Y-%m") for item in commits
    )
    month_rows = [[key, monthly[key], percent(monthly[key], len(commits))] for key in sorted(monthly)]

    weekday_names = ("周一", "周二", "周三", "周四", "周五", "周六", "周日")
    daily_rows = []
    for day in date_range(month_start, month_end):
        total = focused_daily[day]
        merges = merge_daily[day]
        daily_rows.append(
            [day.isoformat(), weekday_names[day.weekday()], total, total - merges, merges]
        )

    contributors = Counter(item.author for item in focused)
    contributor_rows = [
        [author, count, percent(count, len(focused))]
        for author, count in contributors.most_common()
    ]

    types = Counter()
    for commit in focused:
        match = CONVENTIONAL_TYPE_RE.match(commit.subject)
        types[match.group(1).lower() if match else "other"] += 1
    type_rows = [[kind, count, percent(count, len(focused))] for kind, count in types.most_common()]

    weekday_counts = Counter(
        weekday_names[commit_datetime(item, field, timezone).weekday()] for item in focused
    )
    weekday_rows = [[name, weekday_counts[name]] for name in weekday_names]

    peak_count = max(focused_daily.values(), default=0)
    peak_days = [day.isoformat() for day, count in sorted(focused_daily.items()) if count == peak_count]
    focused_merges = sum(item.is_merge for item in focused)
    initial_active_days = len(
        {commit_datetime(item, field, timezone).date() for item in initial}
    )
    last_three_start = max(month_start, month_end - timedelta(days=2))
    last_three_count = sum(
        count for day, count in focused_daily.items() if last_three_start <= day <= month_end
    )

    lines = [
        f"# Git 提交历史报告：{year}-{month:02d}",
        "",
        "## 统计口径",
        "",
        f"- 仓库 revision：`{revision_label}`，解析为 `{resolved_revision}`。",
        "- 历史范围：只统计该 revision 可达的提交，不包含未合并分支。",
        f"- 日期口径：使用 `{field}` date，并转换到 `{timezone_name}`。",
        "- Merge 口径：merge commit 计入总数，同时单独列出。",
        f"- 初始阶段：从最早提交日起连续 {initial_days} 个自然日。",
        "",
        "## 仓库概览",
        "",
        *markdown_table(
            ["指标", "值"],
            [
                ["首个提交", f"`{first.sha[:12]}` · {first_day} · {first.subject}"],
                ["该 revision 最新提交", f"`{latest.sha[:12]}` · {latest_day} · {latest.subject}"],
                ["可达提交数", len(commits)],
                ["Merge commit 数", sum(item.is_merge for item in commits)],
                ["不同作者身份数", len({item.author for item in commits})],
            ],
        ),
        "",
        "### 月度提交趋势",
        "",
        *markdown_table(["月份", "提交数", "占可达历史比例"], month_rows),
        "",
        "## 初始阶段",
        "",
        f"初始阶段窗口为 **{first_day} 至 {initial_end}**，共 **{len(initial)} 个 commit**，"
        f"其中 **{initial_active_days}/{initial_days} 天有提交**，平均每个自然日 "
        f"**{len(initial) / initial_days:.1f} 个 commit**。",
        "",
        f"## {year}-{month:02d} 每日提交数",
        "",
        *markdown_table(["日期", "星期", "全部 commit", "非合并", "合并"], daily_rows),
        "",
        "### 本月汇总",
        "",
        *markdown_table(
            ["指标", "值"],
            [
                ["全部 commit", len(focused)],
                ["非合并 commit", len(focused) - focused_merges],
                ["Merge commit", focused_merges],
                ["有提交的自然日", f"{active_days}/{len(daily_rows)}"],
                ["平均每个自然日", f"{len(focused) / len(daily_rows):.2f}"],
                ["平均每个活跃日", f"{len(focused) / active_days:.2f}" if active_days else "0.00"],
                ["峰值日期", f"{', '.join(peak_days) or '无'} ({peak_count})"],
                [f"最后三天（{last_three_start}..{month_end}）", f"{last_three_count} ({percent(last_three_count, len(focused))})"],
            ],
        ),
        "",
        "### 贡献者",
        "",
        *markdown_table(["作者身份", "提交数", "占比"], contributor_rows),
        "",
        "### Conventional Commit 类型",
        "",
        *markdown_table(["类型", "提交数", "占比"], type_rows),
        "",
        "### 星期分布",
        "",
        *markdown_table(["星期", "提交数"], weekday_rows),
        "",
        "## 结论",
        "",
        (
            f"- 仓库始于 **{first_day}**，就在目标月份内，因此 {year}-{month:02d} 就是项目创始阶段。"
            if first_day.year == year and first_day.month == month
            else f"- 仓库始于 **{first_day}**，早于目标月份 {year}-{month:02d}。"
        ),
        f"- 目标月份共有 **{len(focused)} 个 commit**、**{active_days} 个活跃日**；峰值为 "
        f"**{', '.join(peak_days) or '无'} 的 {peak_count} 个 commit**。",
        f"- 最后三个自然日贡献 **{last_three_count}/{len(focused)} 个 commit "
        f"({percent(last_three_count, len(focused))})**，可见月末开发活动的集中程度。",
        f"- Merge commit 占 **{focused_merges}/{len(focused)} ({percent(focused_merges, len(focused))})**；"
        "跨时期比较时可优先参考“非合并”列，避免合并策略变化造成误判。",
        "",
        "## 复现命令",
        "",
        "```bash",
        f"python3 scripts/analyze_git_history.py --revision {revision_label} --year {year} --month {month} \\",
        f"  --timezone {timezone_name} --date-field {field} --initial-days {initial_days}",
        "```",
        "",
        "报告记录了完整 SHA，因为 `origin/main` 这类分支会在报告生成后继续移动。",
        "",
    ]
    return "\n".join(lines)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="Git repository path")
    parser.add_argument("--revision", default="origin/main", help="Commit or ref to analyze")
    parser.add_argument("--year", type=int, required=True, help="Focus year")
    parser.add_argument("--month", type=int, required=True, choices=range(1, 13), help="Focus month")
    parser.add_argument("--timezone", default="Asia/Shanghai", help="IANA timezone")
    parser.add_argument("--date-field", choices=("author", "committer"), default="author")
    parser.add_argument("--initial-days", type=int, default=30, help="Initial-stage calendar days")
    parser.add_argument("--output", type=Path, help="Write Markdown to this file instead of stdout")
    args = parser.parse_args(argv)
    if args.initial_days < 1:
        parser.error("--initial-days must be at least 1")
    return args


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        timezone = resolve_timezone(args.timezone)
        resolved, commits = load_commits(args.repo.resolve(), args.revision)
        report = render_report(
            commits,
            resolved,
            args.revision,
            args.date_field,
            timezone,
            args.timezone,
            args.year,
            args.month,
            args.initial_days,
        )
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(report, encoding="utf-8")
        else:
            print(report, end="")
    except (OSError, RuntimeError, ValueError, ZoneInfoNotFoundError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
