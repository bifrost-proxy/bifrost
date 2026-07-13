#!/usr/bin/env python3
"""Convert the unified E2E runner's TSV ledger into an auditable JSON report."""

from __future__ import annotations

import argparse
import csv
import json
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path


VALID_STATUSES = {"passed", "failed", "skipped"}


def build_report(rows: list[list[str]], metadata: dict[str, str]) -> dict[str, object]:
    suites: list[dict[str, object]] = []
    counts: Counter[str] = Counter()
    for row in rows:
        if len(row) != 5:
            raise ValueError(f"expected 5 TSV columns, got {len(row)}")
        status, name, duration, log_path, reason = row
        if status not in VALID_STATUSES:
            raise ValueError(f"unknown suite status: {status}")
        try:
            duration_seconds = int(duration)
        except ValueError as exc:
            raise ValueError(f"invalid duration for {name}: {duration}") from exc
        counts[status] += 1
        suites.append(
            {
                "name": name,
                "status": status,
                "duration_seconds": duration_seconds,
                "log": log_path or None,
                "reason": reason or None,
            }
        )
    return {
        "schema_version": 1,
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "metadata": metadata,
        "counts": {
            "total": len(suites),
            "passed": counts["passed"],
            "failed": counts["failed"],
            "skipped": counts["skipped"],
        },
        "suites": suites,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("ledger")
    parser.add_argument("output")
    parser.add_argument("--metadata", action="append", default=[])
    args = parser.parse_args()

    metadata: dict[str, str] = {}
    for item in args.metadata:
        key, separator, value = item.partition("=")
        if not separator or not key:
            parser.error(f"invalid metadata value: {item}")
        metadata[key] = value

    with Path(args.ledger).open(newline="", encoding="utf-8") as handle:
        rows = list(csv.reader(handle, delimiter="\t"))
    report = build_report(rows, metadata)
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"E2E JSON summary: {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
