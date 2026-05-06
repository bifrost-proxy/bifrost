#!/usr/bin/env python3

import json
import os
import sys
from pathlib import Path


def resolve_home() -> Path:
    home = os.environ.get("HOME")
    if not home:
        raise RuntimeError("HOME is not set; cannot resolve Codex default data dir")
    return Path(os.path.abspath(os.path.expanduser(home)))


def normalize(path: Path) -> Path:
    return Path(os.path.abspath(os.path.expanduser(str(path))))


def candidate_from_env(home: Path):
    raw = os.environ.get("CODEX_HOME")
    if not raw:
        return None
    path = Path(raw).expanduser()
    if not path.is_absolute():
        path = home / path
    path = normalize(path)
    return {
        "source": "env:CODEX_HOME",
        "path": path,
        "exists": path.exists(),
    }


def candidate_from_default(home: Path):
    path = normalize(home / ".codex")
    return {
        "source": "default:$HOME/.codex",
        "path": path,
        "exists": path.exists(),
    }


def inspect(path: Path):
    return {
        "config_toml": (path / "config.toml").exists(),
        "sessions_dir": (path / "sessions").is_dir(),
        "session_index": (path / "session_index.jsonl").is_file(),
        "history": (path / "history.jsonl").is_file(),
        "state_db": any(path.glob("state_*.sqlite")),
    }


def choose(candidates):
    if candidates and candidates[0]["source"] == "env:CODEX_HOME":
        return candidates[0]
    for candidate in candidates:
        if candidate["exists"]:
            return candidate
    return candidates[0]


def main() -> int:
    home = resolve_home()
    candidates = []

    env_candidate = candidate_from_env(home)
    if env_candidate:
        candidates.append(env_candidate)
    candidates.append(candidate_from_default(home))

    selected = choose(candidates)
    selected["markers"] = inspect(selected["path"])

    payload = {
        "selected_path": str(selected["path"]),
        "selected_source": selected["source"],
        "selected_exists": selected["exists"],
        "markers": selected["markers"],
        "candidates": [
            {
                "source": item["source"],
                "path": str(item["path"]),
                "exists": item["exists"],
            }
            for item in candidates
        ],
    }

    json.dump(payload, sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
