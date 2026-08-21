"""Certification ledger: single machine-readable source of truth."""

from __future__ import annotations

import json
import re
import subprocess
from functools import lru_cache
from pathlib import Path

from . import (
    NON_TERMINAL_STATUSES,
    SERVER_AUTHORITATIVE_CATEGORIES,
    TERMINAL_STATUSES,
)
from .schema import contains_local_path

LEDGER_FIELDS = (
    "id",
    "category",
    "upstream_files",
    "upstream_symbols",
    "source_change",
    "rust_owner",
    "risk",
    "implementation_required",
    "evidence_required",
    "status",
    "source_evidence",
    "jar_probe",
    "rust_tests",
    "notes",
)

_RUST_TEST_TOKEN = re.compile(r"^[a-z][a-z0-9_]*$")
_RUST_TEST_SELFTEST = re.compile(r"^tools/compat_selftest\.py(?: [a-z0-9_]+)?$")
_LISTED_TEST_LINE = re.compile(r"^(.+):\s*(test|bench)\s*$")


class LedgerError(ValueError):
    pass


def rust_tests_tokens_valid(rust_tests: str) -> bool:
    """Each comma-separated token must be a cargo test filter or compat_selftest hook."""
    text = (rust_tests or "").strip()
    if not text:
        return True
    for part in text.split(","):
        token = part.strip()
        if not token:
            return False
        if token.startswith("tools/"):
            if not _RUST_TEST_SELFTEST.match(token):
                return False
        elif not _RUST_TEST_TOKEN.match(token):
            return False
    return True


def iter_rust_test_tokens(rust_tests: str) -> list[str]:
    """Cargo-filter tokens only (compat_selftest hooks excluded)."""
    out: list[str] = []
    for part in (rust_tests or "").split(","):
        token = part.strip()
        if not token or token.startswith("tools/"):
            continue
        out.append(token)
    return out


@lru_cache(maxsize=1)
def list_rust_tests(repo_root: str) -> frozenset[str]:
    """Return full cargo test names via `cargo test -- --list` (no execution)."""
    import os

    root = Path(repo_root)
    cargo = _resolve_cargo_bin()
    # CI sets RUSTFLAGS=-D warnings globally. An early `cargo test --list` can
    # compile dependency build-scripts under that flag while rustup is still
    # fetching components, which fails closed spuriously. Listing must not
    # inherit deny-warnings from the gate env.
    env = os.environ.copy()
    env.pop("RUSTFLAGS", None)
    env.pop("CARGO_ENCODED_RUSTFLAGS", None)
    res = subprocess.run(
        [cargo, "test", "--all-targets", "--", "--list"],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
        env=env,
    )
    if res.returncode != 0:
        raise LedgerError(
            f"cargo test --list failed (rc={res.returncode}): {res.stderr[-2000:]}"
        )
    names: set[str] = set()
    for line in (res.stdout or "").splitlines():
        m = _LISTED_TEST_LINE.match(line.strip())
        if not m:
            continue
        names.add(m.group(1).strip())
    if not names:
        raise LedgerError("cargo test --list returned no test names")
    return frozenset(names)


def _resolve_cargo_bin() -> str:
    """Prefer a real toolchain cargo when rustup proxies break under Cursor argv0."""
    import os
    import shutil

    override = os.environ.get("CARGO")
    if override and Path(override).exists():
        return override

    # CI / normal shells: honor PATH first. Hardcoded rustup paths can point at a
    # stale toolchain (e.g. cached stable-1.83) while dtolnay/rust-toolchain put
    # the intended cargo earlier on PATH.
    if os.environ.get("GITHUB_ACTIONS") == "true" or os.environ.get("CI") == "true":
        found = shutil.which("cargo")
        if found:
            return found

    home = Path.home()
    for candidate in (
        home / ".rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo",
    ):
        if candidate.exists():
            return str(candidate)
    found = shutil.which("cargo")
    if found:
        return found
    raise LedgerError("cargo binary not found")


def rust_test_filter_matches(token: str, listed: frozenset[str] | set[str]) -> bool:
    """Cargo filter semantics: token is a substring of a listed test name."""
    if not token:
        return False
    return any(token in name for name in listed)


def validate_rust_test_evidence(
    doc: dict,
    listed: frozenset[str] | set[str] | None = None,
    *,
    repo_root: Path | None = None,
) -> list[str]:
    """Fail-closed: every VERIFIED_* rust_tests cargo filter must match >=1 real test."""
    errors: list[str] = []
    if listed is None:
        if repo_root is None:
            raise LedgerError("validate_rust_test_evidence requires listed tests or repo_root")
        listed = list_rust_tests(str(repo_root))
    for row in doc.get("rows") or []:
        status = row.get("status")
        if status not in {"VERIFIED_IMPLEMENTED", "VERIFIED_UNCHANGED"}:
            continue
        rust_tests = (row.get("rust_tests") or "").strip()
        if not rust_tests:
            continue
        if not rust_tests_tokens_valid(rust_tests):
            continue  # syntax errors reported by validate_ledger
        rid = row.get("id")
        for token in iter_rust_test_tokens(rust_tests):
            if not rust_test_filter_matches(token, listed):
                errors.append(
                    f"{rid}: rust_tests token {token!r} matches no listed cargo test"
                )
    return errors


def load_ledger(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def validate_ledger(doc: dict) -> list[str]:
    errors = []
    if doc.get("schema_version") != 2:
        errors.append("ledger schema_version must be 2")
    for key in ("build", "baseline", "overall_status", "rows"):
        if key not in doc:
            errors.append(f"ledger missing {key}")
    errors.extend(f"local path leaked: {h}" for h in contains_local_path(doc))
    rows = doc.get("rows") or []
    ids = set()
    for i, row in enumerate(rows):
        prefix = f"row[{i}]"
        if not isinstance(row, dict):
            errors.append(f"{prefix} not an object")
            continue
        for field in LEDGER_FIELDS:
            if field not in row:
                errors.append(f"{prefix} missing {field}")
        rid = row.get("id")
        if rid in ids:
            errors.append(f"duplicate id {rid}")
        ids.add(rid)
        status = row.get("status")
        category = row.get("category")
        source_evidence = (row.get("source_evidence") or "").strip()
        jar_probe = (row.get("jar_probe") or "").strip()
        rust_tests = (row.get("rust_tests") or "").strip()
        notes = (row.get("notes") or "").strip()
        risk = row.get("risk")
        if status not in TERMINAL_STATUSES and status not in NON_TERMINAL_STATUSES:
            errors.append(f"{rid}: illegal status {status}")
        if row.get("implementation_required") and not row.get("evidence_required"):
            errors.append(f"{rid}: implementation_required rows must set evidence_required=true")
        if row.get("implementation_required") and status in {"CLIENT_ONLY", "EDITOR_ONLY"}:
            errors.append(
                f"{rid}: implementation_required rows cannot use terminal status {status}"
            )
        if (
            category in SERVER_AUTHORITATIVE_CATEGORIES
            and status in {"CLIENT_ONLY", "EDITOR_ONLY"}
            and row.get("implementation_required")
        ):
            errors.append(
                f"{rid}: server-authoritative row cannot claim {status} while implementation_required"
            )
        if status in {"VERIFIED_IMPLEMENTED", "VERIFIED_UNCHANGED"} and not rust_tests_tokens_valid(
            rust_tests
        ):
            errors.append(
                f"{rid}: rust_tests must be comma-separated cargo test filters or tools/compat_selftest.py hooks"
            )
        if not row.get("evidence_required"):
            continue
        if status == "VERIFIED_UNCHANGED":
            if not source_evidence and not jar_probe and not rust_tests:
                errors.append(f"{rid}: VERIFIED_UNCHANGED requires source/probe/test evidence")
        elif status == "VERIFIED_IMPLEMENTED":
            if not source_evidence:
                errors.append(f"{rid}: VERIFIED_IMPLEMENTED requires source_evidence")
            elif not jar_probe and not rust_tests:
                errors.append(f"{rid}: VERIFIED_IMPLEMENTED requires jar_probe or rust_tests")
            elif (
                risk == "HIGH"
                and category
                in {
                    "WIRE_PACKET",
                    "STREAMING",
                    "SAVE",
                    "LOGIC",
                    "AI",
                    "PLACEMENT",
                    "RULES",
                    "TYPEIO",
                    "INPUT_AUTHORITY",
                }
                and not jar_probe
                and not rust_tests
            ):
                errors.append(f"{rid}: HIGH-risk VERIFIED_IMPLEMENTED requires jar_probe or rust_tests")
        elif status == "OUT_OF_SCOPE_EXPLICIT":
            if len(notes) < 20:
                errors.append(f"{rid}: OUT_OF_SCOPE_EXPLICIT requires explicit scope/rejection notes")
    if doc.get("overall_status") in {"PASS", "CERTIFIED"}:
        for row in rows:
            if row.get("evidence_required") and row.get("status") in {
                "VERIFIED_IMPLEMENTED",
                "VERIFIED_UNCHANGED",
            }:
                source_evidence = (row.get("source_evidence") or "").strip()
                jar_probe = (row.get("jar_probe") or "").strip()
                rust_tests = (row.get("rust_tests") or "").strip()
                if row.get("status") == "VERIFIED_IMPLEMENTED" and (
                    not source_evidence or (not jar_probe and not rust_tests)
                ):
                    errors.append(
                        f"{row.get('id')}: PASS blocked — VERIFIED_IMPLEMENTED lacks required evidence"
                    )
    return errors


def unresolved_server_rows(doc: dict) -> list[dict]:
    out = []
    for row in doc.get("rows") or []:
        status = row.get("status")
        category = row.get("category")
        if status in TERMINAL_STATUSES:
            if row.get("implementation_required") and status in {"CLIENT_ONLY", "EDITOR_ONLY"}:
                out.append(row)
            continue
        if category in {"CLIENT_ONLY", "EDITOR_ONLY"}:
            continue
        if category in SERVER_AUTHORITATIVE_CATEGORIES or row.get("implementation_required"):
            out.append(row)
        elif status in NON_TERMINAL_STATUSES:
            out.append(row)
    return out


def certification_may_pass(doc: dict) -> bool:
    if doc.get("overall_status") not in {"PASS", "CERTIFIED"}:
        return False
    if validate_ledger(doc):
        return False
    return not unresolved_server_rows(doc)


def render_markdown(doc: dict) -> str:
    runtime_sha = (doc.get("certified_runtime_sha") or "").strip()
    lines = [
        f"# Mindustry Build {doc.get('build')} Certification Ledger",
        "",
        f"- **Overall status**: `{doc.get('overall_status')}`",
        f"- **Baseline**: `{doc.get('baseline')}`",
        f"- **Source**: `{doc.get('source_ref')}` `{doc.get('source_commit')}`",
        f"- **CERTIFIED_RUNTIME_SHA**: `{runtime_sha or 'MISSING'}`",
        "- **Final CI head**: recorded externally in the PR / GitHub Actions run "
        "(not required to equal the runtime checkpoint)",
        "",
        "| ID | Category | Status | Risk | Rust owner | Evidence |",
        "|---|---|---|---|---|---|",
    ]
    for row in doc.get("rows") or []:
        evidence = "; ".join(
            x for x in (row.get("source_evidence"), row.get("jar_probe"), row.get("rust_tests")) if x
        )
        lines.append(
            f"| `{row['id']}` | {row['category']} | {row['status']} | {row.get('risk','')} | `{row.get('rust_owner','')}` | {evidence} |"
        )
    unresolved = unresolved_server_rows(doc)
    lines += ["", f"Unresolved server-authoritative rows: **{len(unresolved)}**", ""]
    return "\n".join(lines) + "\n"
