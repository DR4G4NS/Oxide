#!/usr/bin/env python3
"""Differential compatibility analysis and non-certifying master canary."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from compatlib.atomic import canonical_dumps
from compatlib.classifier import enumerate_delta, resolve_commit
from compatlib.current import load_current
from compatlib.diff import diff_builds, load_build

REPO_ROOT = Path(__file__).resolve().parent.parent


def print_summary(from_build: str, to_build: str, delta: dict) -> None:
    print(f"=== COMPATIBILITY DELTA: {from_build} -> {to_build} ===")
    pk = delta["packets"]
    print(f"Packets added: {len(pk['added'])} removed: {len(pk['removed'])} shifted: {len(pk['id_shifted'])} order_changed: {pk['registration_order_changed']}")
    for p in pk["added"]:
        print(f"  + [{p.get('id')}] {p.get('name')}")
    print(f"Streams added: {len(delta['streams']['added'])}")
    print(f"RPC added: {len(delta['rpc']['added'])} changed: {len(delta['rpc']['changed'])}")
    print(f"TypeIO changed: {len(delta['typeio']['changed'])}")
    print(f"Saves added: {len(delta['saves']['added'])} class_changed: {len(delta['saves']['class_changed'])}")
    print(f"Rules added: {len(delta['rules']['added'])} changed: {len(delta['rules']['changed'])}")
    print(f"Logic access added: {len(delta['logic']['access']['added'])}")
    print(f"Entities changed: {len(delta['entities']['changed'])} entity_sync_changed: {delta['entity_sync']['changed']}")


def write_canary(source_repo: Path, from_ref: str, to_ref: str, md_out: Path) -> int:
    current = load_current()
    if to_ref in {"current", current["target"]["build"]}:
        print("error: canary must not target the certified current build as a promotion", file=sys.stderr)
        return 4
    from_sha = resolve_commit(source_repo, from_ref)
    to_sha = resolve_commit(source_repo, to_ref)
    certified = current["target"]["source_commit"]
    if from_ref in {current["target"]["build"], current["target"]["source_tag"]} and from_sha != certified:
        print(
            f"error: source tag {from_ref} resolved to {from_sha}, expected certified {certified}",
            file=sys.stderr,
        )
        return 5
    rows = enumerate_delta(source_repo, from_ref, to_ref)
    interesting = [
        r
        for r in rows
        if r.category
        not in {"CLIENT_ONLY", "EDITOR_ONLY"}
    ]
    lines = [
        f"# Master canary (NON-CERTIFYING)",
        "",
        f"- **From**: `{from_ref}` `{from_sha}`",
        f"- **To**: `{to_ref}` `{to_sha}`",
        f"- **Certified current target**: `{current['target']['build']}` (unchanged)",
        f"- **Status**: DETECTOR ONLY — master is **not** certified",
        "",
        f"Changed non-client files: {len(interesting)} / {len(rows)}",
        "",
        "| Path | Category | Symbols |",
        "|---|---|---|",
    ]
    for r in interesting:
        lines.append(f"| `{r.path}` | {r.category} | {', '.join(r.symbols[:5])} |")
    md_out.parent.mkdir(parents=True, exist_ok=True)
    md_out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"wrote {md_out} ({len(interesting)} server-relevant candidates); current.toml not modified")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--from", dest="from_build", required=True)
    parser.add_argument("--to", dest="to_build")
    parser.add_argument("--to-source-ref")
    parser.add_argument("--from-source-ref")
    parser.add_argument("--mode", choices=["diff", "canary"], default="diff")
    parser.add_argument("--source-repo", type=Path)
    parser.add_argument("--json-out", type=Path)
    parser.add_argument("--md-out", type=Path)
    args = parser.parse_args()

    if args.mode == "canary":
        if not args.source_repo or not args.to_source_ref:
            print("error: canary requires --source-repo and --to-source-ref", file=sys.stderr)
            return 2
        from_ref = args.from_source_ref or args.from_build
        if from_ref.replace("v", "") == args.from_build or not args.from_source_ref:
            current = load_current()
            from_ref = current["target"]["source_tag"] if args.from_build == current["target"]["build"] else args.from_build
        md_out = args.md_out or (REPO_ROOT / "target" / "master-canary.md")
        return write_canary(args.source_repo, from_ref, args.to_source_ref, md_out)

    to_build = args.to_build
    if not to_build:
        print("error: --to is required in diff mode", file=sys.stderr)
        return 2
    from_data = load_build(REPO_ROOT / "compat", args.from_build)
    to_data = load_build(REPO_ROOT / "compat", to_build)
    delta = diff_builds(from_data, to_data)
    delta["from_build"] = args.from_build
    delta["to_build"] = to_build
    print_summary(args.from_build, to_build, delta)
    if args.json_out:
        args.json_out.write_text(canonical_dumps(delta), encoding="utf-8")
        print(f"wrote JSON delta to {args.json_out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
