#!/usr/bin/env python3
"""Enumerate and classify official Mindustry source deltas."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from compatlib.atomic import canonical_dumps
from compatlib.classifier import enumerate_delta, resolve_commit
from compatlib.current import load_current


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--from", dest="from_ref", required=True)
    parser.add_argument("--to", dest="to_ref", required=True)
    parser.add_argument("--source-repo", type=Path, required=True)
    parser.add_argument("--json-out", type=Path)
    parser.add_argument("--md-out", type=Path)
    args = parser.parse_args()

    from_sha = resolve_commit(args.source_repo, args.from_ref)
    to_sha = resolve_commit(args.source_repo, args.to_ref)
    rows = enumerate_delta(args.source_repo, args.from_ref, args.to_ref)
    unknown = [r for r in rows if r.category == "UNKNOWN_REQUIRES_PROBE"]
    report = {
        "from_ref": args.from_ref,
        "from_commit": from_sha,
        "to_ref": args.to_ref,
        "to_commit": to_sha,
        "file_count": len(rows),
        "unknown_requires_probe": [r.path for r in unknown],
        "files": [
            {
                "path": r.path,
                "status": r.status,
                "category": r.category,
                "symbols": r.symbols,
                "notes": r.notes,
            }
            for r in rows
        ],
    }
    print(f"delta {args.from_ref} ({from_sha[:12]}) -> {args.to_ref} ({to_sha[:12]}): {len(rows)} files, {len(unknown)} UNKNOWN_REQUIRES_PROBE")
    if args.json_out:
        args.json_out.write_text(canonical_dumps(report), encoding="utf-8")
    if args.md_out:
        lines = [
            f"# Source delta {args.from_ref} → {args.to_ref}",
            "",
            f"- from: `{from_sha}`",
            f"- to: `{to_sha}`",
            f"- files: {len(rows)}",
            f"- UNKNOWN_REQUIRES_PROBE: {len(unknown)}",
            "",
            "| Path | Status | Category | Symbols |",
            "|---|---|---|---|",
        ]
        for r in rows:
            lines.append(f"| `{r.path}` | {r.status} | {r.category} | {', '.join(r.symbols[:6])} |")
        args.md_out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
