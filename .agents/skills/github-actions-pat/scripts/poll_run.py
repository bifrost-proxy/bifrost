"""Poll a workflow run until it leaves queued/in_progress state.

PAT-only. Reads $GITHUB_TOKEN and targets $GH_REPO (fallback bifrost-proxy/bifrost).

Usage:
  POLL_SEC=45 MAX_WAIT_SEC=1800 python3 scripts/poll_run.py <run_id>

Exit codes:
  0 — completed & conclusion=success
  2 — completed with non-success conclusion (failure / cancelled / timed_out / ...)
  3 — timed out waiting for the run to complete
"""
from __future__ import annotations

import json
import os
import sys
import time
import urllib.request

REPO = os.environ.get("GH_REPO", "bifrost-proxy/bifrost")


def main() -> int:
    if len(sys.argv) < 2:
        sys.stderr.write("usage: poll_run.py <run_id>\n")
        return 1
    run_id = int(sys.argv[1])
    token = os.environ.get("GITHUB_TOKEN", "").strip()
    if not token:
        sys.stderr.write("ERROR: set GITHUB_TOKEN before running this skill\n")
        return 2

    api = f"https://api.github.com/repos/{REPO}/actions/runs/{run_id}"
    hdrs = {
        "Authorization": f"Bearer {token}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "bifrost-ci-inspector/1.0",
    }

    poll_interval = int(os.environ.get("POLL_SEC", "30"))
    max_wait = int(os.environ.get("MAX_WAIT_SEC", "1500"))
    start = time.time()
    last_status = None
    while True:
        req = urllib.request.Request(api, headers=hdrs)
        with urllib.request.urlopen(req) as r:
            d = json.loads(r.read().decode())
        status = d.get("status")
        conc = d.get("conclusion")
        elapsed = int(time.time() - start)
        if status != last_status:
            print(f"[{elapsed:>4}s] status={status} conclusion={conc}", flush=True)
            last_status = status
        else:
            print(f"[{elapsed:>4}s] ... still {status}", flush=True)
        if status == "completed":
            print(json.dumps({
                "run_id": run_id,
                "status": status,
                "conclusion": conc,
                "head_sha": d.get("head_sha"),
                "html_url": d.get("html_url"),
            }, indent=2))
            return 0 if conc == "success" else 2
        if elapsed > max_wait:
            sys.stderr.write("TIMEOUT waiting for run to complete\n")
            return 3
        time.sleep(poll_interval)


if __name__ == "__main__":
    sys.exit(main())
