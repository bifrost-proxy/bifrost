#!/usr/bin/env python3

from __future__ import annotations

import argparse
import io
import json
import os
import pathlib
import re
import subprocess
import sys
import textwrap
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from typing import Any

UNAUTHENTICATED_MIN_POLL_INTERVAL = 75
COOKIE_INSPECTOR = ".trae/skills/github-actions-ci-inspect/scripts/github-actions-ci"
COOKIE_FILE = ".env/.cookie.github.com"


def eprint(*args: object) -> None:
    print(*args, file=sys.stderr)


def git(args: list[str], cwd: pathlib.Path) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Watch a GitHub Actions CI run, fetch failure logs, and optionally ask Codex to fix and retry."
    )
    parser.add_argument("--repo", default="bifrost-proxy/bifrost", help="GitHub repo in owner/name form")
    parser.add_argument("--workflow", default="ci.yml", help="Workflow filename or workflow id")
    parser.add_argument("--branch", help="Branch to watch; defaults to current git branch")
    parser.add_argument("--run-id", type=int, help="Specific workflow run id to watch first")
    parser.add_argument("--run-url", help="Specific workflow run url to watch first")
    parser.add_argument("--poll-interval", type=int, default=20, help="Polling interval in seconds")
    parser.add_argument("--max-cycles", type=int, default=10, help="Maximum fix/retry cycles")
    parser.add_argument(
        "--cookie-inspector",
        default=COOKIE_INSPECTOR,
        help="Cookie-based GitHub Actions inspector script",
    )
    parser.add_argument(
        "--cookie-file",
        default=COOKIE_FILE,
        help="Cookie file used by the GitHub Actions inspector",
    )
    parser.add_argument(
        "--state-dir",
        default=".git/ci-watch",
        help="Directory for downloaded logs, summaries, and artifacts",
    )
    parser.add_argument(
        "--fix-with-codex",
        action="store_true",
        help="Invoke `codex exec` after a failure bundle is collected",
    )
    parser.add_argument("--codex-model", help="Optional model passed to `codex exec`")
    parser.add_argument(
        "--codex-extra-prompt",
        help="Additional instructions appended to the Codex repair prompt",
    )
    parser.add_argument(
        "--download-artifacts",
        action="store_true",
        help="Download workflow artifacts for failed runs when API access is available",
    )
    parser.add_argument(
        "--push-after-fix",
        action="store_true",
        help="If Codex leaves committed local changes, push `origin HEAD` after the repair step",
    )
    return parser.parse_args()


def parse_run_id_from_url(run_url: str) -> int:
    match = re.search(r"/actions/runs/(\d+)", run_url)
    if not match:
        raise ValueError(f"Unable to parse run id from url: {run_url}")
    return int(match.group(1))


def run_cookie_inspector(
    repo_dir: pathlib.Path,
    inspector_script: pathlib.Path,
    cookie_file: pathlib.Path,
    repo: str,
    workflow: str,
    run: str | int,
    failed_only: bool = False,
    fetch_logs: bool = False,
) -> dict[str, Any]:
    cmd = [
        "bash",
        str(inspector_script),
        "--repo",
        repo,
        "--workflow",
        workflow,
        "--run",
        str(run),
        "--format",
        "json",
        "--cookie-file",
        str(cookie_file),
    ]
    if failed_only:
        cmd.append("--failed-only")
    if fetch_logs:
        cmd.append("--fetch-logs")

    result = subprocess.run(
        cmd,
        cwd=repo_dir,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def cookie_run_status(cookie_data: dict[str, Any]) -> tuple[str, str | None]:
    status_text = str(cookie_data.get("run", {}).get("status") or "").lower()
    failed_jobs_count = sum(
        1
        for job in cookie_data.get("jobs", [])
        if job.get("failedSteps")
        or "failed" in str(job.get("status", "")).lower()
        or "failed" in str(job.get("jobStatus", "")).lower()
    )
    if "success" in status_text:
        return "completed", "success"
    if any(token in status_text for token in ["failure", "failed", "timed out", "cancelled"]):
        return "completed", "failure"
    if failed_jobs_count > 0:
        return "completed", "failure"
    if "queued" in status_text or "waiting" in status_text:
        return "queued", None
    if "in progress" in status_text or "running" in status_text or "pending" in status_text:
        return "in_progress", None
    return "unknown", None


def cookie_jobs_to_watch(cookie_data: dict[str, Any], run_id: int) -> list[dict[str, Any]]:
    jobs = []
    for job in cookie_data.get("jobs", []):
        status = "completed"
        conclusion = "success"
        if job.get("failedSteps") or "failed" in str(job.get("status", "")).lower() or "failed" in str(job.get("jobStatus", "")).lower():
            conclusion = "failure"
        elif "running" in str(job.get("status", "")).lower() or "in_progress" in str(job.get("jobStatus", "")).lower():
            status = "in_progress"
            conclusion = None

        steps = []
        for step in job.get("steps", []):
            step_conclusion = step.get("conclusion")
            step_status = "completed" if step_conclusion not in (None, "", "in_progress") else "in_progress"
            steps.append(
                {
                    "number": step.get("number"),
                    "name": step.get("name"),
                    "status": step_status,
                    "conclusion": step_conclusion,
                    "logPath": step.get("logPath"),
                }
            )

        jobs.append(
            {
                "id": int(job["jobId"]),
                "run_id": run_id,
                "name": job["name"],
                "status": status,
                "conclusion": conclusion,
                "html_url": f"https://github.com{job['jobPath']}",
                "steps": steps,
                "annotations": job.get("annotations", []),
                "relatedRunAnnotations": job.get("relatedRunAnnotations", []),
                "failureSummary": job.get("failureSummary"),
                "context": job.get("context", {}),
                "stepLogs": job.get("stepLogs", {}),
            }
        )
    return jobs


class GitHubClient:
    def __init__(self, repo: str, token: str | None) -> None:
        self.repo = repo
        self.token = token
        self.authenticated = bool(token)

    def api_url(self, path: str, params: dict[str, Any] | None = None) -> str:
        base = f"https://api.github.com/repos/{self.repo}{path}"
        if not params:
            return base
        query = urllib.parse.urlencode(params)
        return f"{base}?{query}"

    def request_json(self, path: str, params: dict[str, Any] | None = None) -> Any:
        url = self.api_url(path, params)
        req = urllib.request.Request(
            url,
            headers=self._headers(),
        )
        with self._open_with_retry(req) as resp:
            return json.load(resp)

    def request_bytes(self, path: str, params: dict[str, Any] | None = None) -> bytes:
        url = self.api_url(path, params)
        req = urllib.request.Request(
            url,
            headers=self._headers(),
        )
        with self._open_with_retry(req) as resp:
            return resp.read()

    def _open_with_retry(self, req: urllib.request.Request):
        while True:
            try:
                return urllib.request.urlopen(req)
            except urllib.error.HTTPError as exc:
                if exc.code == 403 and self._is_rate_limited(exc):
                    wait_seconds = self._rate_limit_wait_seconds(exc)
                    eprint(f"[warn] GitHub API rate limited; sleeping {wait_seconds}s before retry")
                    time.sleep(wait_seconds)
                    continue
                raise

    def _is_rate_limited(self, exc: urllib.error.HTTPError) -> bool:
        remaining = exc.headers.get("X-RateLimit-Remaining")
        if remaining == "0":
            return True
        try:
            body = exc.read().decode("utf-8", errors="ignore").lower()
        except Exception:
            body = ""
        return "rate limit" in body

    def _rate_limit_wait_seconds(self, exc: urllib.error.HTTPError) -> int:
        reset = exc.headers.get("X-RateLimit-Reset")
        if reset and reset.isdigit():
            return max(5, int(reset) - int(time.time()) + 5)
        return 60

    def _headers(self) -> dict[str, str]:
        headers = {
            "Accept": "application/vnd.github+json",
            "User-Agent": "bifrost-ci-watch",
        }
        if self.token:
            headers["Authorization"] = f"Bearer {self.token}"
        return headers


def latest_run(client: GitHubClient, workflow: str, branch: str) -> dict[str, Any] | None:
    data = client.request_json(
        f"/actions/workflows/{workflow}/runs",
        {"branch": branch, "per_page": 10},
    )
    runs = data.get("workflow_runs", [])
    return runs[0] if runs else None


def get_run(client: GitHubClient, run_id: int) -> dict[str, Any]:
    return client.request_json(f"/actions/runs/{run_id}")


def list_jobs(client: GitHubClient, run_id: int) -> list[dict[str, Any]]:
    jobs: list[dict[str, Any]] = []
    page = 1
    while True:
        data = client.request_json(
            f"/actions/runs/{run_id}/jobs",
            {"per_page": 100, "page": page},
        )
        page_jobs = data.get("jobs", [])
        if not page_jobs:
            break
        jobs.extend(page_jobs)
        if len(page_jobs) < 100:
            break
        page += 1
    return jobs


def list_artifacts(client: GitHubClient, run_id: int) -> list[dict[str, Any]]:
    data = client.request_json(f"/actions/runs/{run_id}/artifacts", {"per_page": 100})
    return data.get("artifacts", [])


def fetch_annotations(client: GitHubClient, job_id: int) -> list[dict[str, Any]]:
    try:
        data = client.request_json(f"/check-runs/{job_id}/annotations", {"per_page": 100})
        return data if isinstance(data, list) else []
    except urllib.error.HTTPError as exc:
        eprint(f"[warn] failed to fetch annotations for job {job_id}: {exc}")
        return []


def download_job_log(client: GitHubClient, job_id: int, output_path: pathlib.Path) -> bool:
    try:
        data = client.request_bytes(f"/actions/jobs/{job_id}/logs")
    except urllib.error.HTTPError as exc:
        eprint(f"[warn] failed to download logs for job {job_id}: {exc}")
        return False

    output_path.write_bytes(data)
    return True


def download_artifact_zip(client: GitHubClient, artifact_id: int, output_path: pathlib.Path) -> bool:
    try:
        data = client.request_bytes(f"/actions/artifacts/{artifact_id}/zip")
    except urllib.error.HTTPError as exc:
        eprint(f"[warn] failed to download artifact {artifact_id}: {exc}")
        return False

    output_path.write_bytes(data)
    return True


def print_job_progress(jobs: list[dict[str, Any]], last_seen: dict[int, tuple[str, str | None, str]]) -> dict[int, tuple[str, str | None, str]]:
    current: dict[int, tuple[str, str | None, str]] = {}
    for job in jobs:
        steps = [step for step in job.get("steps", []) if step.get("status") != "pending"]
        current_step = steps[-1]["name"] if steps else "-"
        state = (job["status"], job.get("conclusion"), current_step)
        current[job["id"]] = state
        if last_seen.get(job["id"]) != state:
            print(
                f"[ci] {job['name']}: status={job['status']} conclusion={job.get('conclusion')} step={current_step}",
                flush=True,
            )
    return current


def failed_jobs(jobs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [job for job in jobs if job.get("conclusion") == "failure"]


def ensure_dir(path: pathlib.Path) -> pathlib.Path:
    path.mkdir(parents=True, exist_ok=True)
    return path


def write_json(path: pathlib.Path, data: Any) -> None:
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")


def build_failure_summary(run: dict[str, Any], jobs: list[dict[str, Any]]) -> str:
    lines = [
        f"# CI Failure Summary",
        "",
        f"- workflow run: #{run['run_number']}",
        f"- run id: {run['id']}",
        f"- status: {run['status']}",
        f"- conclusion: {run.get('conclusion')}",
        f"- head sha: {run['head_sha']}",
        f"- url: {run['html_url']}",
        "",
    ]
    for job in jobs:
        lines.append(f"## {job['name']}")
        lines.append(f"- job id: {job['id']}")
        lines.append(f"- url: {job['html_url']}")
        lines.append(f"- conclusion: {job.get('conclusion')}")
        for step in job.get("steps", []):
            if step.get("status") == "pending":
                continue
            lines.append(
                f"- step {step['number']}: {step['name']} ({step['status']}/{step.get('conclusion')})"
            )
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def collect_failure_bundle(
    client: GitHubClient,
    run: dict[str, Any],
    jobs: list[dict[str, Any]],
    state_dir: pathlib.Path,
    download_artifacts: bool,
) -> pathlib.Path:
    bundle_dir = ensure_dir(state_dir / f"run-{run['run_number']}-{run['head_sha'][:7]}")
    write_json(bundle_dir / "run.json", run)
    write_json(bundle_dir / "jobs.json", jobs)

    failed = failed_jobs(jobs)
    (bundle_dir / "summary.md").write_text(build_failure_summary(run, failed), encoding="utf-8")

    for job in failed:
        job_dir = ensure_dir(bundle_dir / f"job-{job['id']}")
        write_json(job_dir / "job.json", job)
        annotations = fetch_annotations(client, job["id"])
        write_json(job_dir / "annotations.json", annotations)
        download_job_log(client, job["id"], job_dir / "job.log")

    artifacts = list_artifacts(client, run["id"])
    write_json(bundle_dir / "artifacts.json", artifacts)
    if download_artifacts:
        artifacts_dir = ensure_dir(bundle_dir / "artifacts")
        for artifact in artifacts:
            artifact_zip = artifacts_dir / f"{artifact['id']}-{artifact['name']}.zip"
            if not download_artifact_zip(client, artifact["id"], artifact_zip):
                continue
            extract_dir = ensure_dir(artifacts_dir / f"{artifact['id']}-{artifact['name']}")
            with zipfile.ZipFile(io.BytesIO(artifact_zip.read_bytes())) as zf:
                zf.extractall(extract_dir)

    return bundle_dir


def collect_cookie_failure_bundle(
    cookie_data: dict[str, Any],
    head_sha: str,
    run_id: int,
    state_dir: pathlib.Path,
) -> pathlib.Path:
    run_number = cookie_data["run"]["runId"]
    bundle_dir = ensure_dir(state_dir / f"run-{run_number}-{head_sha[:7]}")
    write_json(bundle_dir / "cookie-run.json", cookie_data)

    summary_lines = [
        "# CI Failure Summary",
        "",
        f"- workflow run: #{run_number}",
        f"- run id: {run_id}",
        f"- status: {cookie_data['run'].get('status')}",
        f"- head sha: {head_sha}",
        f"- url: https://github.com{cookie_data['run'].get('runPath')}",
        "",
    ]
    for job in cookie_data.get("jobs", []):
        job_failed = (
            job.get("failedSteps")
            or "failed" in str(job.get("status", "")).lower()
            or "failed" in str(job.get("jobStatus", "")).lower()
        )
        if not job_failed:
            continue
        job_dir = ensure_dir(bundle_dir / f"job-{job['jobId']}")
        write_json(job_dir / "job.json", job)
        summary_lines.append(f"## {job['name']}")
        summary_lines.append(f"- job id: {job['jobId']}")
        summary_lines.append(f"- url: https://github.com{job['jobPath']}")
        if job.get("failureSummary"):
            summary_lines.append(f"- summary: {job['failureSummary']}")
        for failed_step in job.get("failedSteps", []):
            summary_lines.append(
                f"- step {failed_step.get('number')}: {failed_step.get('name')} ({failed_step.get('conclusion')})"
            )
        for number, log in job.get("stepLogs", {}).items():
            write_json(job_dir / f"step-{number}.log.json", log)
        summary_lines.append("")

    (bundle_dir / "summary.md").write_text("\n".join(summary_lines).rstrip() + "\n", encoding="utf-8")
    return bundle_dir


def run_codex_fix(
    repo_dir: pathlib.Path,
    branch: str,
    run: dict[str, Any],
    failed: list[dict[str, Any]],
    bundle_dir: pathlib.Path,
    codex_model: str | None,
    codex_extra_prompt: str | None,
) -> None:
    failed_names = ", ".join(job["name"] for job in failed) or "unknown jobs"
    prompt = textwrap.dedent(
        f"""
        The GitHub Actions workflow `ci.yml` failed for branch `{branch}`.

        Failure bundle directory: `{bundle_dir}`
        Workflow run: #{run['run_number']} ({run['html_url']})
        Head SHA: {run['head_sha']}
        Failed jobs: {failed_names}

        Please:
        1. Inspect the downloaded failure summary, annotations, logs, and any artifacts in that bundle.
        2. Reproduce the relevant failing CI issue locally.
        3. Fix the real issue without mocking or skipping the failing tests.
        4. Run the smallest meaningful validation for the fix.
        5. Commit the fix and push `origin HEAD`.

        If one failure blocks others, fix the current root cause first and push.
        """
    ).strip()
    if codex_extra_prompt:
        prompt += "\n\nAdditional instructions:\n" + codex_extra_prompt.strip()

    cmd = [
        "codex",
        "exec",
        "--dangerously-bypass-approvals-and-sandbox",
        "-C",
        str(repo_dir),
        "-o",
        str(bundle_dir / "codex-last-message.txt"),
        prompt,
    ]
    if codex_model:
        cmd[2:2] = ["-m", codex_model]

    print("[ci] invoking Codex repair loop...", flush=True)
    subprocess.run(cmd, check=True)


def wait_for_new_run(
    client: GitHubClient,
    workflow: str,
    branch: str,
    previous_run_id: int,
    expected_head_sha: str,
    poll_interval: int,
    timeout_seconds: int = 900,
) -> dict[str, Any]:
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        run = latest_run(client, workflow, branch)
        if run and run["id"] != previous_run_id and run["head_sha"] == expected_head_sha:
            print(f"[ci] detected new workflow run #{run['run_number']} ({run['id']})", flush=True)
            return run
        time.sleep(poll_interval)
    raise TimeoutError("Timed out waiting for a new workflow run after repair")


def wait_for_new_run_with_cookie(
    repo_dir: pathlib.Path,
    inspector_script: pathlib.Path,
    cookie_file: pathlib.Path,
    repo: str,
    workflow: str,
    previous_run_id: int,
    expected_head_sha: str,
    poll_interval: int,
    timeout_seconds: int = 900,
) -> dict[str, Any]:
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        run = latest_run_with_cookie(
            repo_dir=repo_dir,
            inspector_script=inspector_script,
            cookie_file=cookie_file,
            repo=repo,
            workflow=workflow,
        )
        if run["id"] != previous_run_id:
            current_head = git(["rev-parse", "HEAD"], repo_dir)
            if current_head != expected_head_sha:
                eprint(
                    f"[warn] current HEAD {current_head} differs from expected {expected_head_sha}; "
                    "continuing to watch the newest CI run"
                )
            print(f"[ci] detected new workflow run #{run['run_number']} ({run['id']})", flush=True)
            return run
        time.sleep(poll_interval)
    raise TimeoutError("Timed out waiting for a new workflow run after repair")


def monitor_run(client: GitHubClient, run_id: int, poll_interval: int) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    last_seen: dict[int, tuple[str, str | None, str]] = {}
    while True:
        run = get_run(client, run_id)
        jobs = list_jobs(client, run_id)
        last_seen = print_job_progress(jobs, last_seen)
        if run["status"] == "completed":
            return run, jobs
        time.sleep(poll_interval)


def maybe_push_current_head(repo_dir: pathlib.Path) -> None:
    status = git(["status", "--short"], repo_dir)
    if status:
        raise RuntimeError("Refusing to auto-push with a dirty working tree after Codex fix")
    subprocess.run(["git", "push", "origin", "HEAD"], cwd=repo_dir, check=True)


def monitor_run_with_cookie(
    repo_dir: pathlib.Path,
    inspector_script: pathlib.Path,
    cookie_file: pathlib.Path,
    repo: str,
    workflow: str,
    run_id: int,
    poll_interval: int,
) -> tuple[dict[str, Any], list[dict[str, Any]], dict[str, Any]]:
    last_seen: dict[int, tuple[str, str | None, str]] = {}
    while True:
        cookie_data = run_cookie_inspector(
            repo_dir=repo_dir,
            inspector_script=inspector_script,
            cookie_file=cookie_file,
            repo=repo,
            workflow=workflow,
            run=run_id,
            fetch_logs=True,
        )
        status, conclusion = cookie_run_status(cookie_data)
        run = {
            "id": run_id,
            "run_number": int(cookie_data["run"]["runId"]),
            "status": status,
            "conclusion": conclusion,
            "head_sha": git(["rev-parse", "HEAD"], repo_dir),
            "html_url": f"https://github.com{cookie_data['run']['runPath']}",
        }
        jobs = cookie_jobs_to_watch(cookie_data, run_id)
        last_seen = print_job_progress(jobs, last_seen)
        if status == "completed":
            return run, jobs, cookie_data
        time.sleep(poll_interval)


def latest_run_with_cookie(
    repo_dir: pathlib.Path,
    inspector_script: pathlib.Path,
    cookie_file: pathlib.Path,
    repo: str,
    workflow: str,
) -> dict[str, Any]:
    cookie_data = run_cookie_inspector(
        repo_dir=repo_dir,
        inspector_script=inspector_script,
        cookie_file=cookie_file,
        repo=repo,
        workflow=workflow,
        run="latest",
        fetch_logs=False,
    )
    run_id = int(cookie_data["run"]["runId"])
    status, conclusion = cookie_run_status(cookie_data)
    return {
        "id": run_id,
        "run_number": run_id,
        "status": status,
        "conclusion": conclusion,
        "head_sha": git(["rev-parse", "HEAD"], repo_dir),
        "html_url": f"https://github.com{cookie_data['run']['runPath']}",
    }


def main() -> int:
    args = parse_args()
    repo_dir = pathlib.Path.cwd()
    branch = args.branch or git(["rev-parse", "--abbrev-ref", "HEAD"], repo_dir)
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    state_dir = ensure_dir(repo_dir / args.state_dir)

    client = GitHubClient(args.repo, token)
    inspector_script = repo_dir / args.cookie_inspector
    cookie_file = repo_dir / args.cookie_file
    use_cookie_inspector = inspector_script.exists() and cookie_file.exists()
    if (
        not use_cookie_inspector
        and not client.authenticated
        and args.poll_interval < UNAUTHENTICATED_MIN_POLL_INTERVAL
    ):
        eprint(
            f"[warn] no GITHUB_TOKEN/GH_TOKEN detected; raising poll interval to "
            f"{UNAUTHENTICATED_MIN_POLL_INTERVAL}s to stay within GitHub public API limits"
        )
        args.poll_interval = UNAUTHENTICATED_MIN_POLL_INTERVAL
    if use_cookie_inspector:
        print(f"[ci] using cookie inspector: {inspector_script}", flush=True)

    if args.run_url:
        run_id = parse_run_id_from_url(args.run_url)
    elif args.run_id:
        run_id = args.run_id
    else:
        if use_cookie_inspector:
            run = latest_run_with_cookie(
                repo_dir=repo_dir,
                inspector_script=inspector_script,
                cookie_file=cookie_file,
                repo=args.repo,
                workflow=args.workflow,
            )
            run_id = int(run["id"])
        else:
            run = latest_run(client, args.workflow, branch)
            if not run:
                eprint(f"No workflow runs found for {args.workflow} on branch {branch}")
                return 1
            run_id = int(run["id"])

    for cycle in range(1, args.max_cycles + 1):
        print(f"[ci] cycle {cycle}: watching run {run_id}", flush=True)
        cookie_data = None
        if use_cookie_inspector:
            run, jobs, cookie_data = monitor_run_with_cookie(
                repo_dir=repo_dir,
                inspector_script=inspector_script,
                cookie_file=cookie_file,
                repo=args.repo,
                workflow=args.workflow,
                run_id=run_id,
                poll_interval=args.poll_interval,
            )
        else:
            run, jobs = monitor_run(client, run_id, args.poll_interval)
        if run.get("conclusion") == "success":
            print(f"[ci] workflow #{run['run_number']} succeeded", flush=True)
            return 0

        failed = failed_jobs(jobs)
        if use_cookie_inspector and cookie_data is not None:
            bundle_dir = collect_cookie_failure_bundle(
                cookie_data=cookie_data,
                head_sha=run["head_sha"],
                run_id=run["id"],
                state_dir=state_dir,
            )
        else:
            bundle_dir = collect_failure_bundle(
                client,
                run,
                jobs,
                state_dir,
                download_artifacts=args.download_artifacts,
            )
        print(f"[ci] failure bundle saved to {bundle_dir}", flush=True)

        if not args.fix_with_codex:
            eprint("[ci] run failed and --fix-with-codex is not enabled")
            return 1

        before_head = git(["rev-parse", "HEAD"], repo_dir)
        run_codex_fix(
            repo_dir=repo_dir,
            branch=branch,
            run=run,
            failed=failed,
            bundle_dir=bundle_dir,
            codex_model=args.codex_model,
            codex_extra_prompt=args.codex_extra_prompt,
        )
        after_head = git(["rev-parse", "HEAD"], repo_dir)
        if args.push_after_fix and after_head != before_head:
            maybe_push_current_head(repo_dir)

        if use_cookie_inspector:
            run_id = wait_for_new_run_with_cookie(
                repo_dir=repo_dir,
                inspector_script=inspector_script,
                cookie_file=cookie_file,
                repo=args.repo,
                workflow=args.workflow,
                previous_run_id=run["id"],
                expected_head_sha=after_head,
                poll_interval=args.poll_interval,
            )["id"]
        else:
            run_id = wait_for_new_run(
                client,
                workflow=args.workflow,
                branch=branch,
                previous_run_id=run["id"],
                expected_head_sha=after_head,
                poll_interval=args.poll_interval,
            )["id"]

    eprint(f"[ci] exhausted max cycles ({args.max_cycles}) without success")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
