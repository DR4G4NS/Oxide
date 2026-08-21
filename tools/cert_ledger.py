#!/usr/bin/env python3
"""Validate and render the certification ledger."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from compatlib.current import current_build, load_current
from compatlib.ledger import (
    certification_may_pass,
    load_ledger,
    list_rust_tests,
    render_markdown,
    unresolved_server_rows,
    validate_ledger,
    validate_rust_test_evidence,
)
from compatlib.runtime_checkpoint import validate_ledger_runtime_checkpoint
from compatlib.schema import validate_compat_dir

REPO_ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--build", default="")
    parser.add_argument("--require-certified", action="store_true")
    parser.add_argument("--write-md", action="store_true")
    args = parser.parse_args()
    current = load_current()
    build = args.build or current_build(current)
    ledger_path = REPO_ROOT / "compat" / build / "certification-ledger.json"
    if not ledger_path.exists():
        print(f"error: missing {ledger_path}", file=sys.stderr)
        return 1
    doc = load_ledger(ledger_path)
    errors = validate_ledger(doc)
    errors.extend(validate_compat_dir(REPO_ROOT / "compat" / build, expected_build=build))
    if doc.get("build") != build:
        errors.append(f"ledger build {doc.get('build')} != {build}")
    if doc.get("source_commit") and doc.get("source_commit") != current["target"].get("source_commit") and build == current_build(current):
        errors.append("ledger source_commit does not match current.toml")
    # Fail-closed runtime checkpoint: CERTIFIED_RUNTIME_SHA must resolve,
    # be an ancestor of HEAD, and have no runtime-affecting descendant drift.
    if doc.get("overall_status") in {"PASS", "CERTIFIED"} or args.require_certified:
        errors.extend(validate_ledger_runtime_checkpoint(REPO_ROOT, doc))
    try:
        listed = list_rust_tests(str(REPO_ROOT))
        errors.extend(validate_rust_test_evidence(doc, listed))
    except Exception as exc:  # noqa: BLE001 — fail-closed certification
        errors.append(f"rust test listing failed: {exc}")
    unresolved = unresolved_server_rows(doc)
    if doc.get("overall_status") in {"PASS", "CERTIFIED"} and not certification_may_pass(doc):
        errors.append(
            "overall_status claims PASS/CERTIFIED but certification_may_pass is false"
        )
    if errors:
        print("ledger/schema check FAILED:", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        return 2
    if args.require_certified:
        if not certification_may_pass(doc) or unresolved:
            print(f"error: {len(unresolved)} unresolved server-authoritative rows; certification blocked", file=sys.stderr)
            for row in unresolved:
                print(f"  - {row.get('id')} {row.get('status')} {row.get('category')}", file=sys.stderr)
            return 3
        if "certified_code_sha" in doc:
            print(
                "error: stale certified_code_sha in ledger; use certified_runtime_sha",
                file=sys.stderr,
            )
            return 5
        runtime_sha = (doc.get("certified_runtime_sha") or "").strip()
        if not runtime_sha:
            print("error: ledger missing certified_runtime_sha", file=sys.stderr)
            return 5
    if args.write_md:
        out = REPO_ROOT / "target" / f"{build}-ledger.md"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(render_markdown(doc), encoding="utf-8")
        print(f"wrote {out}")
    print(f"ledger OK for {build}: overall={doc.get('overall_status')} unresolved={len(unresolved)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
