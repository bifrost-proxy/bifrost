"""CI error analyzer — PAT-only (reads $GITHUB_TOKEN, targets $GH_REPO).

Subcommands:
  run <run_id>             — analyze a specific workflow run
  pr <N>                   — most recent failed run on PR #N
  sha <commit_sha>         — most recent run for that sha
  branch <name>            — most recent run on branch (default only failed)
  regression <run_id>      — diff against the previous success on same workflow

All output goes to stdout as Markdown; intended to be surfaced to the user.
Never echoes the token, never writes it to disk.
"""
from __future__ import annotations

import argparse
import sys
from typing import Any, Dict, List, Optional

from common import (
    REPO,
    classify_error,
    extract_snippets,
    http_get,
    token_or_die,
)


def _run_url(run_id: int) -> str:
    return f"https://github.com/{REPO}/actions/runs/{run_id}"


def _fetch_run(token: str, run_id: int) -> Dict[str, Any]:
    _, _, run = http_get(f"/repos/{REPO}/actions/runs/{run_id}", token)
    return run


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


def _fetch_job_log(token: str, job_id: int) -> str:
    """Fetch a single job's text log.

    GitHub returns 302 to a signed Azure Blob URL. If we let urllib auto-follow
    with the Authorization header still attached, Azure returns
    `401 InvalidAuthenticationInfo`. So we block the redirect, grab the
    Location, and do a second request without Authorization.
    """
    import urllib.request
    import urllib.error
    from common import _base_headers  # lazy import to avoid cycles

    api_url = f"https://api.github.com/repos/{REPO}/actions/jobs/{job_id}/logs"

    class _NoRedirect(urllib.request.HTTPRedirectHandler):
        def http_error_302(self, req, fp, code, msg, headers):
            return None
        http_error_301 = http_error_303 = http_error_307 = http_error_302

    opener = urllib.request.build_opener(_NoRedirect())
    req = urllib.request.Request(api_url, headers=_base_headers(token))
    signed_url = None
    try:
        resp = opener.open(req)
        # If no redirect, we already have the text body
        body = resp.read()
        return body.decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        if e.code in (301, 302, 303, 307, 308):
            signed_url = e.headers.get("Location")
        else:
            raise

    if not signed_url:
        return ""
    # Fetch signed URL without auth — Azure Blob refuses Authorization header
    req2 = urllib.request.Request(
        signed_url, headers={"User-Agent": "bifrost-ci-inspector/1.0"}
    )
    with urllib.request.urlopen(req2) as resp2:
        return resp2.read().decode("utf-8", errors="replace")


def _failed_steps(job: Dict[str, Any]) -> List[Dict[str, Any]]:
    return [s for s in job.get("steps", []) if s.get("conclusion") == "failure"]


def _suggest_repro(job_name: str, step_name: str, log_tail: str) -> List[str]:
    sugg: List[str] = []
    jn = job_name.lower()
    sn = (step_name or "").lower()
    lt = log_tail.lower()
    if "fmt" in sn or "fmt" in jn or "rustfmt" in lt:
        sugg.append("cargo fmt --all -- --check")
    if "clippy" in sn or "clippy" in jn or "clippy" in lt:
        sugg.append("cargo clippy --all-targets --all-features -- -D warnings")
    if "test" in sn or "test" in jn or "test result:" in lt:
        sugg.append("cargo test --workspace --all-features")
    if "build" in sn or "cargo build" in lt:
        sugg.append("cargo build --workspace --all-features")
    if "web" in jn or "pnpm" in lt or "npm" in lt:
        sugg.append("pnpm -C web install && pnpm -C web build")
    if not sugg:
        sugg.append("# Re-run the failing step locally with same env vars as CI")
    return sugg


def _render_run(token: str, run: Dict[str, Any]) -> str:
    run_id = run["id"]
    jobs = _fetch_jobs(token, run_id)
    failed = [j for j in jobs if j.get("conclusion") == "failure"]

    out: List[str] = []
    out.append(
        f"## CI Run #{run_id} — {run.get('conclusion') or run.get('status')}  "
        f"{run.get('name') or run.get('display_title', '')}"
    )
    out.append("")
    out.append(
        f"**Branch/SHA**: {run.get('head_branch','?')}  `{(run.get('head_sha') or '')[:8]}`"
    )
    out.append(f"**URL**: {_run_url(run_id)}")
    out.append(f"**Event**: {run.get('event')}  **Attempt**: {run.get('run_attempt')}")
    out.append("")

    if not failed:
        out.append("_No failed jobs found (run may be queued/success/cancelled)._")
        return "\n".join(out)

    out.append("### Failed jobs")
    out.append("")
    for job in failed:
        job_id = job["id"]
        job_name = job.get("name", f"job-{job_id}")
        failed_steps = _failed_steps(job) or [{"name": "(unknown step)"}]
        log = _fetch_job_log(token, job_id)
        snippets = extract_snippets(log)
        cls = classify_error(log)

        for step in failed_steps:
            step_name = step.get("name", "?")
            out.append(f"#### {job_name} — step \"{step_name}\"")
            out.append("")
            out.append(f"Root cause (one-liner): **{cls}**")
            out.append("")
            out.append("Key error:")
            out.append("```")
            out.append("\n\n".join(snippets))
            out.append("```")
            out.append("")
            out.append("Suggested fix:")
            for s in _suggest_repro(job_name, step_name, log[-8000:]):
                out.append(f"- {s}")
            out.append("")
            out.append("Repro locally:")
            out.append("```bash")
            for s in _suggest_repro(job_name, step_name, log[-8000:]):
                if not s.startswith("#"):
                    out.append(s)
                    break
            out.append("```")
            out.append("")
            out.append(f"Job URL: {job.get('html_url', '')}")
            out.append("")
    return "\n".join(out)


def _latest_failed_run(token: str, *, branch: Optional[str] = None,
                       sha: Optional[str] = None,
                       pr: Optional[int] = None,
                       only_failed: bool = True) -> Optional[Dict[str, Any]]:
    params: Dict[str, Any] = {"per_page": 20}
    if branch:
        params["branch"] = branch
    if sha:
        params["head_sha"] = sha
    if only_failed:
        params["status"] = "failure"

    target_sha: Optional[str] = None
    if pr:
        _, _, pr_data = http_get(f"/repos/{REPO}/pulls/{pr}", token)
        if not pr_data:
            return None
        target_sha = pr_data.get("head", {}).get("sha")
        params["branch"] = pr_data.get("head", {}).get("ref")

    _, _, data = http_get(f"/repos/{REPO}/actions/runs", token, params=params)
    runs = (data or {}).get("workflow_runs", []) if isinstance(data, dict) else []
    if pr and target_sha:
        runs = [r for r in runs if r.get("head_sha") == target_sha]
    return runs[0] if runs else None


def _previous_success(token: str, run: Dict[str, Any]) -> Optional[Dict[str, Any]]:
    wf_id = run.get("workflow_id")
    branch = run.get("head_branch")
    _, _, data = http_get(
        f"/repos/{REPO}/actions/workflows/{wf_id}/runs",
        token,
        params={"status": "success", "branch": branch, "per_page": 5},
    )
    runs = (data or {}).get("workflow_runs", []) if isinstance(data, dict) else []
    return runs[0] if runs else None


def cmd_run(args: argparse.Namespace) -> None:
    token = token_or_die()
    run = _fetch_run(token, args.run_id)
    print(_render_run(token, run))


def cmd_pr(args: argparse.Namespace) -> None:
    token = token_or_die()
    run = _latest_failed_run(token, pr=args.pr_number, only_failed=not args.any_status)
    if not run:
        print(f"_No failed runs found for PR #{args.pr_number}._")
        return
    print(_render_run(token, run))


def cmd_sha(args: argparse.Namespace) -> None:
    token = token_or_die()
    run = _latest_failed_run(token, sha=args.sha, only_failed=not args.any_status)
    if not run:
        print(f"_No failed runs found for sha {args.sha[:8]}._")
        return
    print(_render_run(token, run))


def cmd_branch(args: argparse.Namespace) -> None:
    token = token_or_die()
    run = _latest_failed_run(token, branch=args.branch, only_failed=args.only_failed)
    if not run:
        print(f"_No matching runs found for branch {args.branch}._")
        return
    print(_render_run(token, run))


def cmd_regression(args: argparse.Namespace) -> None:
    token = token_or_die()
    failed = _fetch_run(token, args.run_id)
    prev = _previous_success(token, failed)
    print(_render_run(token, failed))
    print("\n---\n")
    if not prev:
        print("_No previous success on same workflow+branch — cannot diff._")
        return
    print(f"### Last green run: #{prev['id']} — `{(prev.get('head_sha') or '')[:8]}`  ")
    print(
        f"Compare: https://github.com/{REPO}/compare/"
        f"{(prev.get('head_sha') or '')[:8]}...{(failed.get('head_sha') or '')[:8]}"
    )
    print("")
    print(f"Suggested: `git log --oneline {prev.get('head_sha')}..{failed.get('head_sha')}`")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="GitHub Actions CI analyzer (PAT)")
    sub = p.add_subparsers(dest="cmd", required=True)

    sp = sub.add_parser("run", help="analyze a specific workflow run")
    sp.add_argument("run_id", type=int)
    sp.set_defaults(func=cmd_run)

    sp = sub.add_parser("pr", help="most recent failed run for a PR")
    sp.add_argument("pr_number", type=int)
    sp.add_argument("--any-status", action="store_true")
    sp.set_defaults(func=cmd_pr)

    sp = sub.add_parser("sha", help="most recent failed run for a commit SHA")
    sp.add_argument("sha")
    sp.add_argument("--any-status", action="store_true")
    sp.set_defaults(func=cmd_sha)

    sp = sub.add_parser("branch", help="most recent run on a branch")
    sp.add_argument("branch")
    sp.add_argument("--only-failed", action="store_true", default=True)
    sp.add_argument("--any-status", dest="only_failed", action="store_false")
    sp.set_defaults(func=cmd_branch)

    sp = sub.add_parser("regression", help="diff a failed run against last green")
    sp.add_argument("run_id", type=int)
    sp.set_defaults(func=cmd_regression)

    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    args.func(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
