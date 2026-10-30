#!/usr/bin/env python3
"""Find the nearest reachable Bifrost SemVer tag, excluding runtime releases."""

import re
import subprocess
import sys


def previous_tag(revision: str) -> str:
    number = r"(?:0|[1-9][0-9]*)"
    identifier = r"(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)"
    pattern = re.compile(
        rf"v{number}\.{number}\.{number}"
        rf"(?:-{identifier}(?:\.{identifier})*)?"
        r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    )
    tags = subprocess.check_output(
        ["git", "tag", "--merged", revision], text=True
    ).splitlines()
    matches = [tag for tag in tags if pattern.fullmatch(tag)]
    if not matches:
        return ""
    arguments = ["git", "describe", "--tags", "--abbrev=0"]
    for tag in matches:
        arguments.extend(["--match", tag])
    return subprocess.check_output([*arguments, revision], text=True).strip()


if __name__ == "__main__":
    print(previous_tag(sys.argv[1] if len(sys.argv) > 1 else "HEAD"))
