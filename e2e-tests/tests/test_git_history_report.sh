#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
REPORT_FILE="$(mktemp)"
trap 'rm -f "$REPORT_FILE"' EXIT

python3 "$REPO_ROOT/scripts/analyze_git_history.py" \
  --repo "$REPO_ROOT" \
  --revision HEAD \
  --year 2026 \
  --month 2 \
  --timezone Asia/Shanghai \
  --date-field author \
  --initial-days 30 \
  --output "$REPORT_FILE"

expected_total="$({
  git -C "$REPO_ROOT" log HEAD --format='%aI'
} | python3 -c '
import datetime
import sys
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

try:
    timezone = ZoneInfo("Asia/Shanghai")
except ZoneInfoNotFoundError:
    timezone = datetime.timezone(datetime.timedelta(hours=8))
count = 0
for raw in sys.stdin:
    value = raw.strip()
    if not value:
        continue
    if value.endswith("Z"):
        value = value[:-1] + "+00:00"
    day = datetime.datetime.fromisoformat(value).astimezone(timezone).date()
    count += day.year == 2026 and day.month == 2
print(count)
')"

reported_total="$(awk -F'|' '$2 ~ /^ 全部 commit $/ {gsub(/ /, "", $3); print $3; exit}' "$REPORT_FILE")"
daily_total="$(awk -F'|' '$2 ~ /^ 2026-02-[0-9][0-9] $/ {gsub(/ /, "", $4); sum += $4} END {print sum + 0}' "$REPORT_FILE")"
resolved_head="$(git -C "$REPO_ROOT" rev-parse HEAD)"

test "$reported_total" = "$expected_total"
test "$daily_total" = "$expected_total"
grep -Fq "解析为 \`$resolved_head\`" "$REPORT_FILE"
grep -Fq '| 2026-02-01 | 周日 |' "$REPORT_FILE"
test "$(grep -Ec '^\| 2026-02-[0-9]{2} \|' "$REPORT_FILE")" = "28"

echo "git history report E2E passed: February total=$expected_total, daily rows=28"
