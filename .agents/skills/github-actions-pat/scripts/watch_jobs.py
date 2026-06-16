"""Fail-fast watcher for a GitHub Actions run.

Polls `/actions/runs/{id}/jobs` and exits the moment ANY job enters a
non-success terminal state (failure / cancelled / timed_out / action_required).
This lets the fix-push-watch loop start remediation before the slow
long-tail jobs (Windows build, macOS bundle, etc.) finish.

PAT-only. Reads $GITHUB_TOKEN and targets $GH_REPO (fallback bifrost-proxy/bifrost).

Usage:
  POLL_SEC=20 MAX_WAIT_SEC=3600 python3 scripts/watch_jobs.py <run_id>

Exit codes:
  0 — all jobs completed with conclusion=success
  2 — at least one job reached a non-success terminal state; the first such
      job's id/name/url + root-cause bucket + key log snippets are printed
      to stdout as Markdown
  3 — timed out waiting

Markdown stdout on exit=2 is directly consumable by the agent to begin
remediation immediately.
"""
from __future__ import annotations

import os
import sys
import time
from typing import Any, Dict, List, Optional, Tuple

from common import (
    REPO,
    classify_error,
    extract_snippets,
    http_get,
    token_or_die,
)
from gh_ci import _fetch_job_log  # reuse the 302-aware log fetcher


TERMINAL_BAD = {"failure", "cancelled", "timed_out", "action_required", "startup_failure"}


def _fetch_jobs(token: str, run_id: int) -> List[Dict[str, Any]]:
    jobs: List[Dict[str, Any]] = []
    page = 1
    while True:
        _, _, data = http_get(
            f"/repos/{REPO}/actions/runs/{run_id}/jobs",
            token,
            params={"per_page": 100, "page": page, "filter": "latest"},
        )
        chunk = (data or {}).get("jobs", []) if isinstance(data, dict) else []
        if not chunk:
            break
        jobs.extend(chunk)
        if len(chunk) < 100:
            break
        page += 1
    return jobs


def _first_bad(jobs: List[Dict[str, Any]]) -> Optional[Dict[str, Any]]:
    """Return the first job whose conclusion is terminal-bad."""
    for j in jobs:
        if j.get("status") == "completed" and j.get("conclusion") in TERMINAL_BAD:
            return j
    return None


def _status_summary(jobs: List[Dict[str, Any]]) -> Tuple[int, int, int, int]:
    total = len(jobs)
    done_ok = sum(1 for j in jobs if j.get("status") == "completed" and j.get("conclusion") == "success")
    done_bad = sum(1 for j in jobs if j.get("status") == "completed" and j.get("conclusion") in TERMINAL_BAD)
    running = sum(1 for j in jobs if j.get("status") != "completed")
    return total, done_ok, done_bad, running


def _render_failure(token: str, run_id: int, job: Dict[str, Any]) -> str:
    job_id = job["id"]
    name = job.get("name", f"job-{job_id}")
    conc = job.get("conclusion")
    url = job.get("html_url", "")
    failed_steps = [s for s in job.get("steps", []) if s.get("conclusion") in TERMINAL_BAD]
    log = _fetch_job_log(token, job_id)
    bucket = classify_error(log)
    snippets = extract_snippets(log)

    out: List[str] = []
    out.append(f"## ⚠️ Fail-fast: run #{run_id} — first bad job detected")
    out.append("")
    out.append(f"**Job**: `{name}`  ({conc})")
    out.append(f"**Job URL**: {url}")
    if failed_steps:
        step_names = ", ".join(f"\"{s.get('name','?')}\"" for s in failed_steps)
        out.append(f"**Failed step(s)**: {step_names}")
    out.append("")
    out.append(f"**Root cause bucket**: {bucket}")
    out.append("")
    out.append("Key log:")
    out.append("```")
    out.append("\n\n".join(snippets))
    out.append("```")
    out.append("")
    out.append(
        f"Run URL: https://github.com/{REPO}/actions/runs/{run_id}"
    )
    return "\n".join(out)


def main() -> int:
    if len(sys.argv) < 2:
        sys.stderr.write("usage: watch_jobs.py <run_id>\n")
        return 1
    run_id = int(sys.argv[1])
    token = token_or_die()

    poll_interval = int(os.environ.get("POLL_SEC", "20"))
    max_wait = int(os.environ.get("MAX_WAIT_SEC", "3600"))
    start = time.time()

    last_summary: Optional[Tuple[int, int, int, int]] = None
    while True:
        jobs = _fetch_jobs(token, run_id)
        elapsed = int(time.time() - start)

        bad = _first_bad(jobs)
        if bad:
            print(_render_failure(token, run_id, bad))
            sys.stderr.write(
                f"[watch] fail-fast exit on job_id={bad['id']} "
                f"conclusion={bad.get('conclusion')} after {elapsed}s\n"
            )
            return 2

        summary = _status_summary(jobs)
        if summary != last_summary:
            total, ok, bad_n, run_n = summary
            sys.stderr.write(
                f"[{elapsed:>4}s] {ok}/{total} ok, {bad_n} bad, {run_n} running\n"
            )
            last_summary = summary

        _, ok, bad_n, run_n = summary
        if run_n == 0 and bad_n == 0 and ok > 0:
            sys.stderr.write(f"[watch] all {ok} jobs green after {elapsed}s\n")
            print(f"## ✅ Run #{run_id} — all {ok} jobs green")
            print("")
            print(f"Run URL: https://github.com/{REPO}/actions/runs/{run_id}")
            return 0

        if elapsed > max_wait:
            sys.stderr.write(
                f"[watch] TIMEOUT after {elapsed}s (running={run_n})\n"
            )
            return 3
        time.sleep(poll_interval)


if __name__ == "__main__":
    sys.exit(main())
