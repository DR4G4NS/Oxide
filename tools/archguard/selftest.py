#!/usr/bin/env python3
"""Architecture-guard self-tests. Isolated fixtures; every rule class must fire.

Exit 0 only when every must-fail fixture produces its expected codes, every
must-pass fixture is clean, size fixtures behave, and all configured
rule classes were exercised.
"""
from __future__ import annotations

import os
import shutil
import sys
import tempfile
import traceback

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.insert(0, _HERE)

from scan import RULE_CLASSES, scan_root  # noqa: E402
from size import validate_size_registry  # noqa: E402
from textscan import count_dashmap_suppressions, find_god_contexts  # noqa: E402

FIXTURES = os.path.join(_HERE, "fixtures")

MUST_FAIL = {
    "arch001_direct_listener": ["ARCH001"],
    "arch004_wire_broadcast_facade": ["ARCH004"],
    "arch003_dashmap_pending": ["ARCH003"],
    "arch003_arc_dashmap_pending": ["ARCH003"],
    "arch002_domain_runtime": ["ARCH002"],
    "arch002_domain_console": ["ARCH002"],
    "arch005_wire_to_listener": ["ARCH005"],
    "arch006_reexport_outbound": ["ARCH006"],
    "arch006_alias_outbound": ["ARCH006"],
    "pr5_type_alias_pending": ["ARCH003"],
    "pr5_struct_wrap_registry": ["ARCH003"],
    "pr5_fq_listener": ["ARCH001"],
    "pr5_wire_brace_listener": ["ARCH005"],
    "pr5_wire_broadcast_connections": ["ARCH003", "ARCH004"],
    "pr5_cross_file_type_alias": ["ARCH003"],
    "arch006_glob_outbound": ["ARCH006"],
    "pr5_glob_type_alias": ["ARCH003"],
    "arch006_super_outbound": ["ARCH006"],
    "pr5_tuple_struct_registry": ["ARCH003"],
    "pr5_self_method_registry": ["ARCH003"],
    "pr5_generic_type_alias": ["ARCH003"],
    "arch004_method_facade": ["ARCH004"],
    "arch005_super_listener": ["ARCH005"],
    "pr5_ungated_tests_rs": ["ARCH001"],
    "arch004_fn_pointer": ["ARCH004"],
}

MUST_PASS = [
    "domain_pure_encoder",
    "domain_dynamic_world",
    "simulation_outbound",
    "listener_to_economy",
    "session_wire_encoder",
    "unrelated_dashmap",
    "frame_emit_method",
    "frame_emit_ufcs",
    "wire_reexport_encode",
]

failures = 0


def fail(msg: str) -> None:
    global failures
    failures += 1
    print(f"  [FAIL] {msg}", file=sys.stderr)


def ok(msg: str) -> None:
    print(f"  [ok] {msg}")


def codes_from(root: str) -> tuple[set[str], str]:
    diags, classes = scan_root(root)
    got = {d.code for d in diags}
    detail = "\n".join(d.tsv() for d in diags)
    return got, detail


def run_rust_must_fail() -> set[str]:
    exercised: set[str] = set()
    base = os.path.join(FIXTURES, "must_fail")
    expected_dirs = set(MUST_FAIL)
    actual_dirs = {name for name in os.listdir(base) if os.path.isdir(os.path.join(base, name))}
    missing = expected_dirs - actual_dirs
    extra = actual_dirs - expected_dirs
    if missing:
        fail(f"must_fail fixtures missing from disk: {sorted(missing)}")
    if extra:
        fail(f"must_fail fixtures lack expectation table entries: {sorted(extra)}")
    for name, expected in MUST_FAIL.items():
        root = os.path.join(base, name)
        try:
            got, detail = codes_from(root)
        except Exception:
            fail(f"{name} scanner raised:\n{traceback.format_exc()}")
            continue
        missing_codes = [c for c in expected if c not in got]
        if missing_codes:
            fail(f"{name} expected {expected}, missing {missing_codes}\n{detail}")
        else:
            ok(f"{name} -> {sorted(got)}")
            exercised.update(c for c in expected if c in got)
    return exercised


def run_rust_must_pass() -> None:
    base = os.path.join(FIXTURES, "must_pass")
    actual = {name for name in os.listdir(base) if os.path.isdir(os.path.join(base, name))}
    missing = set(MUST_PASS) - actual
    extra = actual - set(MUST_PASS)
    if missing:
        fail(f"must_pass fixtures missing from disk: {sorted(missing)}")
    if extra:
        fail(f"must_pass fixtures lack table entries: {sorted(extra)}")
    for name in MUST_PASS:
        root = os.path.join(base, name)
        try:
            got, detail = codes_from(root)
        except Exception:
            fail(f"{name} scanner raised:\n{traceback.format_exc()}")
            continue
        if got:
            fail(f"{name} must emit no diagnostics, got {sorted(got)}\n{detail}")
        else:
            ok(f"{name} clean")


def write_registry(dirpath: str, rows: list[str], files: dict[str, bytes]) -> str:
    src = os.path.join(dirpath, "src")
    os.makedirs(src, exist_ok=True)
    for rel, data in files.items():
        path = os.path.join(dirpath, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as fh:
            fh.write(data)
    reg = os.path.join(dirpath, "migration-reports/architecture-size-registry.tsv")
    os.makedirs(os.path.dirname(reg), exist_ok=True)
    with open(reg, "w", encoding="utf-8") as fh:
        fh.write("path\tbytes\tstate\trationale\treviewer\tevidence\n")
        for row in rows:
            fh.write(row + "\n")
    return dirpath


def run_size_fixtures() -> set[str]:
    exercised: set[str] = set()
    rationale = "Fixture-gated official codec; splitting would risk persist-format drift."
    reviewer = "architecture-guard selftest"
    evidence = "selftest generated; independent review recorded in fixture"

    with tempfile.TemporaryDirectory() as tmp:
        blob = b"x" * 110_000
        write_registry(
            tmp,
            [
                f"src/engine/save.rs\t{len(blob)}\tOK\t{rationale}\t{reviewer}\t{evidence}",
            ],
            {"src/engine/save.rs": blob},
        )
        fails = validate_size_registry(tmp)
        if fails:
            fail("resolved >100KB rationale must pass:\n" + "\n".join(fails))
        else:
            ok("size >100KB rationale passes")
            exercised.add("ARCH007")

    with tempfile.TemporaryDirectory() as tmp:
        blob = b"x" * 160_000
        write_registry(
            tmp,
            [
                f"src/engine/huge.rs\t{len(blob)}\tCOHESIVE_EXCEPTION\t"
                f"{rationale} Independently reviewed cohesive persist codec.\t"
                f"{reviewer}\tindependent review: selftest fixture",
            ],
            {"src/engine/huge.rs": blob},
        )
        fails = validate_size_registry(tmp)
        if fails:
            fail("valid >150KB cohesive exception must pass:\n" + "\n".join(fails))
        else:
            ok("size >150KB independently reviewed exception passes")

    with tempfile.TemporaryDirectory() as tmp:
        blob = b"x" * 50_000
        write_registry(
            tmp,
            [
                f"src/engine/save.rs\t{len(blob)}\tTBD\t{rationale}\t{reviewer}\t{evidence}",
            ],
            {"src/engine/save.rs": blob},
        )
        fails = validate_size_registry(tmp)
        if not any("unresolved" in f.lower() or "TBD" in f for f in fails):
            fail("TBD size registry must fail ARCH007, got:\n" + "\n".join(fails))
        else:
            ok("size TBD rejected")
            exercised.add("ARCH007")

    with tempfile.TemporaryDirectory() as tmp:
        blob = b"x" * 160_000
        write_registry(
            tmp,
            [
                f"src/engine/huge.rs\t{len(blob)}\tOK\t{rationale}\t{reviewer}\t{evidence}",
            ],
            {"src/engine/huge.rs": blob},
        )
        fails = validate_size_registry(tmp)
        if not any("150" in f for f in fails):
            fail(">150KB without cohesive exception must fail, got:\n" + "\n".join(fails))
        else:
            ok(">150KB missing independent cohesive exception rejected")
            exercised.add("ARCH007")

    with tempfile.TemporaryDirectory() as tmp:
        os.makedirs(os.path.join(tmp, "src/engine"), exist_ok=True)
        os.makedirs(os.path.join(tmp, "migration-reports"), exist_ok=True)
        path = os.path.join(tmp, "src/engine/missing.rs")
        with open(path, "wb") as fh:
            fh.write(b"x" * 110_000)
        with open(
            os.path.join(tmp, "migration-reports/architecture-size-registry.tsv"),
            "w",
            encoding="utf-8",
        ) as fh:
            fh.write("path\tbytes\tstate\trationale\treviewer\tevidence\n")
        fails = validate_size_registry(tmp)
        if not any("missing from size registry" in f for f in fails):
            fail("missing registry row for >100KB production must fail, got:\n" + "\n".join(fails))
        else:
            ok("missing oversized production registry row rejected")

    return exercised


def run_textscan_fixtures() -> None:
    """G2/G4 must not depend on ripgrep and must fail closed without src/."""
    with tempfile.TemporaryDirectory() as tmp:
        src = os.path.join(tmp, "src/network")
        os.makedirs(src)
        with open(os.path.join(src, "world.rs"), "w", encoding="utf-8") as fh:
            fh.write("pub struct World {}\n")
        if find_god_contexts(tmp):
            fail("clean tree must not report god contexts")
        else:
            ok("god-context scan clean tree")

        with open(os.path.join(src, "god.rs"), "w", encoding="utf-8") as fh:
            fh.write("pub(crate) struct ServerContext {\n    pub x: u8,\n}\n")
        hits = find_god_contexts(tmp)
        if "src/network/god.rs" not in hits:
            fail(f"pub(crate) struct ServerContext must be G2, got {hits}")
        else:
            ok("god-context scan finds ServerContext")

        with open(os.path.join(src, "allow.rs"), "w", encoding="utf-8") as fh:
            fh.write(
                "fn tick() {\n"
                '    // dashmap-guard: allow DM900 reason="fixture"\n'
                "}\n"
            )
        if count_dashmap_suppressions(tmp) != 1:
            fail("suppression line count must match dashmap-guard: allow hits")
        else:
            ok("suppression count is portable (no rg)")

    with tempfile.TemporaryDirectory() as tmp:
        raised = False
        try:
            find_god_contexts(tmp)
        except FileNotFoundError:
            raised = True
        if not raised:
            fail("missing src/ must fail closed for G2")
        else:
            ok("missing src/ fails closed for G2")


def run_tool_fail_does_not_empty_ledger() -> None:
    """Scanner/parser failure must not replace the canonical ledger."""
    with tempfile.TemporaryDirectory() as tmp:
        reports = os.path.join(tmp, "migration-reports")
        os.makedirs(reports)
        ledger = os.path.join(reports, "architecture-violations.tsv")
        sentinel = "code\tsource\tline\ttarget\tcapability\tchain\towning_phase\tevidence\nSENTINEL\n"
        with open(ledger, "w", encoding="utf-8") as fh:
            fh.write(sentinel)
        # Malformed exceptions must exit 2 and leave the canonical file untouched.
        with open(os.path.join(reports, "architecture-exceptions.tsv"), "w", encoding="utf-8") as fh:
            fh.write("not-a-header\n")
        os.makedirs(os.path.join(tmp, "src/network/economy"), exist_ok=True)
        with open(os.path.join(tmp, "src/network/economy/mod.rs"), "w", encoding="utf-8") as fh:
            fh.write("pub fn tick() {}\n")
        from scan import load_exceptions

        raised = False
        try:
            load_exceptions(tmp)
        except SystemExit as exc:
            raised = exc.code == 2
        if not raised:
            fail("malformed exceptions must SystemExit 2")
            return
        with open(ledger, encoding="utf-8") as fh:
            body = fh.read()
        if body != sentinel:
            fail("malformed scan must not rewrite the canonical ledger")
        else:
            ok("tool/parser failure leaves canonical ledger untouched")


def main() -> int:
    print("== architecture_guard self-tests ==")
    exercised: set[str] = set()
    print("\n-- rust must-fail --")
    exercised |= run_rust_must_fail()
    print("\n-- rust must-pass --")
    run_rust_must_pass()
    print("\n-- size registry --")
    exercised |= run_size_fixtures()
    print("\n-- fail-closed ledger --")
    run_tool_fail_does_not_empty_ledger()
    print("\n-- portable text scans (no rg) --")
    run_textscan_fixtures()

    required = set(RULE_CLASSES)
    # ARCH900 is exercised if a fixture emits it; add a dedicated unresolved case if missing.
    if "ARCH900" not in exercised:
        with tempfile.TemporaryDirectory() as tmp:
            eco = os.path.join(tmp, "src/network/economy")
            os.makedirs(eco)
            with open(os.path.join(eco, "mod.rs"), "w", encoding="utf-8") as fh:
                fh.write(
                    "use crate::network::outbound::*;\n"
                    "pub fn tick() { enqueue_outbound_routed(); }\n"
                )
            out = os.path.join(tmp, "src/network")
            # glob of outbound is ARCH006; also create a call we cannot resolve
            with open(os.path.join(eco, "mod.rs"), "w", encoding="utf-8") as fh:
                fh.write("pub fn tick() { enqueue_outbound_routed(); }\n")
            got, detail = codes_from(tmp)
            if "ARCH900" in got:
                ok(f"ARCH900 unresolved delivery call -> {sorted(got)}")
                exercised.add("ARCH900")
            else:
                fail(f"unresolved enqueue_outbound_routed must be ARCH900, got {sorted(got)}\n{detail}")

    missing = required - exercised
    extra_ok = exercised - required
    if missing:
        fail(f"rule classes never exercised by fixtures: {sorted(missing)}")
    else:
        ok("every configured rule class was exercised: " + ",".join(RULE_CLASSES))
    if extra_ok:
        ok("fixtures also produced: " + ",".join(sorted(extra_ok)))

    if failures:
        print(f"\narchitecture_guard self-tests FAILED ({failures})", file=sys.stderr)
        return 1
    print("\narchitecture_guard self-tests passed.")
    print("ARCHGUARD_RULE_CLASS_COVERAGE\t" + ",".join(RULE_CLASSES))
    return 0


if __name__ == "__main__":
    sys.exit(main())
