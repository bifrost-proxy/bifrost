"""PR code review skeleton — PAT-only (reads $GITHUB_TOKEN, targets $GH_REPO).

Default: generate structured Markdown review (do NOT post).
With --post: POST the review back to the PR (requires pull_requests:write on the PAT).

Usage:
  python3 scripts/gh_review.py <PR_NUM> [--focus GLOB]... [--max-diff-lines N]
                               [--post] [--event COMMENT|APPROVE|REQUEST_CHANGES]
                               [--body-file PATH]
"""
from __future__ import annotations

import argparse
import fnmatch
import sys
from typing import Any, Dict, List, Optional

from common import REPO, http_get, http_post_json, paginate, token_or_die

LANG_BY_EXT = {
    ".rs": "rust", ".ts": "ts", ".tsx": "tsx", ".js": "js", ".jsx": "jsx",
    ".py": "python", ".go": "go", ".sh": "bash", ".md": "md",
    ".toml": "toml", ".yaml": "yaml", ".yml": "yaml", ".sql": "sql",
}


def _lang_for(path: str) -> str:
    for ext, lang in LANG_BY_EXT.items():
        if path.endswith(ext):
            return lang
    return ""


def _fetch_pr(token: str, n: int) -> Dict[str, Any]:
    _, _, pr = http_get(f"/repos/{REPO}/pulls/{n}", token)
    return pr


def _fetch_files(token: str, n: int) -> List[Dict[str, Any]]:
    return list(paginate(f"/repos/{REPO}/pulls/{n}/files", token, per_page=100, max_pages=30))


def _match_focus(path: str, focus: List[str]) -> bool:
    if not focus:
        return True
    return any(fnmatch.fnmatch(path, pat) for pat in focus)


def _chunk_patch(patch: str, max_lines: int = 200) -> List[str]:
    if not patch:
        return []
    lines = patch.splitlines()
    if len(lines) <= max_lines:
        return [patch]
    chunks: List[str] = []
    for i in range(0, len(lines), max_lines):
        chunks.append("\n".join(lines[i:i + max_lines]))
    return chunks


def render_markdown(pr: Dict[str, Any], files: List[Dict[str, Any]],
                    focus: List[str], max_diff_lines: int) -> str:
    out: List[str] = []
    n = pr["number"]
    title = pr.get("title", "")
    head = pr.get("head", {}).get("ref", "?")
    base = pr.get("base", {}).get("ref", "?")
    commits = pr.get("commits", 0)
    add = pr.get("additions", 0)
    dele = pr.get("deletions", 0)
    nf = pr.get("changed_files", 0)

    out.append(f"## PR Review #{n} — {title}")
    out.append("")
    out.append(
        f"`{head}` → `{base}`  ·  {commits} commits · +{add}/-{dele} lines across {nf} files"
    )
    out.append("")
    out.append(f"URL: {pr.get('html_url','')}")
    out.append("")

    body = (pr.get("body") or "").strip()
    if body:
        out.append("### PR description (excerpt)")
        out.append("")
        out.append("> " + body[:1200].replace("\n", "\n> "))
        if len(body) > 1200:
            out.append("> … (truncated)")
        out.append("")

    considered = [f for f in files if _match_focus(f.get("filename", ""), focus)]
    skipped = len(files) - len(considered)
    if skipped:
        out.append(f"_Filtered by --focus: {skipped} file(s) skipped._")
        out.append("")

    total_lines = 0
    out.append("### Diff excerpts (for agent review)")
    out.append("")
    for f in considered:
        path = f.get("filename", "?")
        status = f.get("status", "?")
        additions = f.get("additions", 0)
        deletions = f.get("deletions", 0)
        patch = f.get("patch") or ""

        if total_lines >= max_diff_lines:
            out.append(f"_… remaining files truncated (reached --max-diff-lines={max_diff_lines})._")
            break

        chunks = _chunk_patch(patch, max_lines=200)
        if not chunks:
            out.append(f"#### `{path}` — {status} (+{additions}/-{deletions})  _(binary or no patch)_")
            out.append("")
            continue

        out.append(f"#### `{path}` — {status} (+{additions}/-{deletions})")
        out.append("")
        for idx, ch in enumerate(chunks):
            lc = ch.count("\n") + 1
            if total_lines + lc > max_diff_lines:
                out.append(f"_… chunk {idx+1}/{len(chunks)} skipped — diff budget reached._")
                break
            total_lines += lc
            out.append("```diff")
            out.append(ch)
            out.append("```")
            out.append("")

    # Review scaffold — agent fills in
    out.append("### 总体判断")
    out.append("")
    out.append("_Agent: 基于上面 diff 填写整体评价（方向是否合理 / 是否应拆分 / 阻塞点）_")
    out.append("")
    out.append("### 分层建议")
    out.append("")
    out.append("**Must-fix (阻塞)**")
    out.append("- _agent-fill_")
    out.append("")
    out.append("**Should-fix (强烈建议)**")
    out.append("- _agent-fill_")
    out.append("")
    out.append("**Nits (可选)**")
    out.append("- _agent-fill_")
    out.append("")
    out.append("### 测试覆盖")
    out.append("")
    out.append("_agent-fill_")
    out.append("")
    out.append("### 安全/性能点")
    out.append("")
    out.append("_agent-fill (没有就写 N/A)_")
    out.append("")

    return "\n".join(out)


def post_review(token: str, n: int, body: str, event: str) -> Dict[str, Any]:
    payload = {"body": body, "event": event}
    _, _, data = http_post_json(f"/repos/{REPO}/pulls/{n}/reviews", token, payload)
    return data if isinstance(data, dict) else {}


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="PR review (PAT)")
    p.add_argument("pr_number", type=int)
    p.add_argument("--focus", action="append", default=[],
                   help="glob to restrict review scope; may repeat")
    p.add_argument("--max-diff-lines", type=int, default=4000)
    p.add_argument("--post", action="store_true",
                   help="actually POST the review to the PR")
    p.add_argument("--event", default="COMMENT",
                   choices=["COMMENT", "APPROVE", "REQUEST_CHANGES"])
    p.add_argument("--body-file",
                   help="when --post: read review body from this file instead of the scaffold")
    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    token = token_or_die()
    pr = _fetch_pr(token, args.pr_number)
    files = _fetch_files(token, args.pr_number)
    md = render_markdown(pr, files, args.focus, args.max_diff_lines)

    if not args.post:
        print(md)
        return 0

    if args.body_file:
        with open(args.body_file, "r", encoding="utf-8") as fh:
            body = fh.read()
    else:
        body = md
    result = post_review(token, args.pr_number, body=body, event=args.event)
    rid = result.get("id")
    url = result.get("html_url")
    sys.stderr.write(f"Posted review id={rid} event={args.event}\n{url}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
