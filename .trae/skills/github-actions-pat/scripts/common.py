"""Common helpers for github-actions-pat skill.

- Token loading (env only, never persisted)
- HTTP with GitHub headers
- Pagination
- Rate-limit handling
- Log slicing / key-error extraction

Repo selection:
- Reads `GH_REPO` env var (format `owner/repo`); falls back to `bifrost-proxy/bifrost`.
  Override by `export GH_REPO=owner/repo` before invoking scripts.
"""
from __future__ import annotations

import io
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from typing import Any, Dict, Iterator, List, Optional, Tuple

REPO = os.environ.get("GH_REPO", "bifrost-proxy/bifrost")
API = "https://api.github.com"
UA = "bifrost-ci-inspector/1.0"


def token_or_die() -> str:
    t = os.environ.get("GITHUB_TOKEN", "").strip()
    if not t:
        sys.stderr.write("ERROR: set GITHUB_TOKEN before running this skill\n")
        sys.exit(2)
    return t


def _base_headers(token: str, accept: str = "application/vnd.github+json") -> Dict[str, str]:
    return {
        "User-Agent": UA,
        "Accept": accept,
        "X-GitHub-Api-Version": "2022-11-28",
        "Authorization": f"Bearer {token}",
    }


def _die_http(resp_status: int, headers: Dict[str, str], body_preview: str) -> None:
    rem = headers.get("x-ratelimit-remaining", "?")
    reset = headers.get("x-ratelimit-reset", "?")
    sys.stderr.write(
        f"HTTP {resp_status} — x-ratelimit-remaining={rem} reset={reset}\n"
    )
    sys.stderr.write(body_preview[:200] + ("\n" if not body_preview.endswith("\n") else ""))
    if resp_status in (403, 429):
        sys.stderr.write(
            "Rate-limited. Wait until x-ratelimit-reset (epoch seconds) before retrying.\n"
        )
    sys.exit(1)


def http_get(
    path_or_url: str,
    token: str,
    *,
    accept: str = "application/vnd.github+json",
    params: Optional[Dict[str, Any]] = None,
    binary: bool = False,
) -> Tuple[int, Dict[str, str], Any]:
    """GET JSON (default) or raw bytes (binary=True).

    Returns (status, headers, parsed-or-bytes). On HTTP error → prints and sys.exit(1).
    """
    url = path_or_url if path_or_url.startswith("http") else API + path_or_url
    if params:
        qs = urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
        url = f"{url}{'&' if '?' in url else '?'}{qs}"
    req = urllib.request.Request(url, headers=_base_headers(token, accept=accept))
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read()
            status = resp.status
            hdrs = {k.lower(): v for k, v in resp.headers.items()}
    except urllib.error.HTTPError as e:
        raw = e.read() or b""
        hdrs = {k.lower(): v for k, v in (e.headers or {}).items()}
        _die_http(e.code, hdrs, raw.decode("utf-8", errors="replace"))
        return 0, {}, None  # unreachable
    except urllib.error.URLError as e:
        sys.stderr.write(f"NETWORK ERROR: {e}\n")
        sys.exit(1)

    if binary:
        return status, hdrs, raw
    if not raw:
        return status, hdrs, None
    try:
        return status, hdrs, json.loads(raw.decode("utf-8"))
    except json.JSONDecodeError:
        return status, hdrs, raw.decode("utf-8", errors="replace")


def http_post_json(
    path_or_url: str,
    token: str,
    payload: Dict[str, Any],
) -> Tuple[int, Dict[str, str], Any]:
    url = path_or_url if path_or_url.startswith("http") else API + path_or_url
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(url, data=data, method="POST")
    for k, v in _base_headers(token).items():
        req.add_header(k, v)
    req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read()
            status = resp.status
            hdrs = {k.lower(): v for k, v in resp.headers.items()}
    except urllib.error.HTTPError as e:
        raw = e.read() or b""
        hdrs = {k.lower(): v for k, v in (e.headers or {}).items()}
        _die_http(e.code, hdrs, raw.decode("utf-8", errors="replace"))
        return 0, {}, None  # unreachable
    try:
        return status, hdrs, json.loads(raw.decode("utf-8"))
    except Exception:
        return status, hdrs, raw.decode("utf-8", errors="replace")


def paginate(
    path: str,
    token: str,
    *,
    per_page: int = 100,
    max_pages: int = 20,
    params: Optional[Dict[str, Any]] = None,
) -> Iterator[Any]:
    """Iterate list-typed GitHub endpoints (e.g. /pulls/{n}/files)."""
    page = 1
    base_params = dict(params or {})
    base_params.setdefault("per_page", per_page)
    while page <= max_pages:
        base_params["page"] = page
        _, _, data = http_get(path, token, params=base_params)
        if not data:
            break
        if isinstance(data, dict) and "items" in data:
            items = data["items"]
        elif isinstance(data, list):
            items = data
        else:
            yield data
            break
        for it in items:
            yield it
        if len(items) < per_page:
            break
        page += 1


# ---------- Log extraction ----------

# Keywords to flag; case-insensitive
ERROR_PATTERNS = [
    re.compile(r"^error\b", re.I),
    re.compile(r"\bFAILED\b"),
    re.compile(r"thread '.*' panicked", re.I),
    re.compile(r"\bpanicked at\b", re.I),
    re.compile(r"^\s*error\[E\d+\]", re.I),  # rustc errors
    re.compile(r"compilation failed", re.I),
    re.compile(r"\bfatal:\b", re.I),
    re.compile(r"segmentation fault", re.I),
    re.compile(r"\btest result: FAILED\b"),
    re.compile(r"##\[error\]"),
    re.compile(r"\bAssertion failed\b", re.I),
    re.compile(r"\bSyntaxError\b"),
    re.compile(r"\bTypeError\b"),
]

TAIL_WINDOW = 500     # only scan last N lines of log
CONTEXT_BEFORE = 6    # lines of context before an error hit
CONTEXT_AFTER = 14    # lines of context after an error hit
MAX_SNIPPETS = 5      # cap snippets per step
MAX_SNIPPET_LINES = 40  # cap size of each snippet


def slice_log(text: str) -> List[Tuple[int, str]]:
    """Return (line_number_1based, line) for lines flagged within tail window."""
    lines = text.splitlines()
    total = len(lines)
    start = max(0, total - TAIL_WINDOW)
    window = lines[start:]
    hits: List[Tuple[int, str]] = []
    for i, ln in enumerate(window):
        for p in ERROR_PATTERNS:
            if p.search(ln):
                hits.append((start + i + 1, ln))
                break
    return hits


def extract_snippets(text: str) -> List[str]:
    """Return up to MAX_SNIPPETS context-wrapped snippets around error hits."""
    lines = text.splitlines()
    hits = slice_log(text)
    if not hits:
        # fall back: last 40 lines
        tail = lines[-MAX_SNIPPET_LINES:]
        return [f"(no keyword match, tail {len(tail)} lines)\n" + "\n".join(tail)]

    used_ranges: List[Tuple[int, int]] = []
    for lineno, _ in hits:
        s = max(1, lineno - CONTEXT_BEFORE)
        e = min(len(lines), lineno + CONTEXT_AFTER)
        # merge if overlaps previous range
        if used_ranges and s <= used_ranges[-1][1] + 2:
            prev_s, prev_e = used_ranges[-1]
            used_ranges[-1] = (prev_s, max(prev_e, e))
            continue
        used_ranges.append((s, e))
        if len(used_ranges) >= MAX_SNIPPETS:
            break

    snippets: List[str] = []
    for s, e in used_ranges:
        chunk = lines[s - 1:e]
        if len(chunk) > MAX_SNIPPET_LINES:
            chunk = chunk[:MAX_SNIPPET_LINES] + [f"... (+{len(chunk) - MAX_SNIPPET_LINES} more)"]
        header = f"--- log L{s}-L{e} ---"
        snippets.append(header + "\n" + "\n".join(chunk))
    return snippets


def classify_error(text: str) -> str:
    """Best-effort root-cause bucket."""
    t = text.lower()
    if "github-actions-log-unavailable" in t or "hosted runner loses communication" in t:
        return "github actions runner/log unavailable"
    if "panicked at" in t or ("thread '" in t and "panicked" in t):
        return "runtime panic (likely test failure)"
    if re.search(r"error\[e\d+\]", t):
        return "rust compile error (rustc)"
    if "cargo fmt" in t or "rustfmt" in t:
        return "fmt violation"
    if "clippy" in t:
        return "clippy lint"
    if "test result: failed" in t:
        return "test failure"
    if "timed out" in t or "timeout" in t:
        return "timeout"
    if "enotfound" in t or "could not resolve host" in t or "network is unreachable" in t:
        return "network"
    if "out of memory" in t or "oom" in t:
        return "OOM"
    if "permission denied" in t or "eacces" in t:
        return "permission"
    if re.search(r"\berror\b", t):
        return "generic error"
    return "unknown"


# ---------- Log archive handling ----------

def unzip_logs(blob: bytes) -> List[Tuple[str, str]]:
    """GitHub run-level logs come as a zip. Returns list of (path, text)."""
    out: List[Tuple[str, str]] = []
    with zipfile.ZipFile(io.BytesIO(blob)) as zf:
        for name in zf.namelist():
            if name.endswith("/"):
                continue
            try:
                out.append((name, zf.read(name).decode("utf-8", errors="replace")))
            except Exception:
                continue
    return out
