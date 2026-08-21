"""Size-registry validator (ARCH007). Fail-closed on TBD / missing / malformed."""
from __future__ import annotations

import os
import re
import sys

KB100 = 102400
KB150 = 153600
ALLOWED_STATES = {"OK", "SPLIT_COMPLETE", "COHESIVE_EXCEPTION"}
UNRESOLVED = {
    "",
    "tbd",
    "todo",
    "unresolved",
    "unknown",
    "pending",
    "wip",
    "⏳",
}
HEADER = ["path", "bytes", "state", "rationale", "reviewer", "evidence"]


def _unresolved(value: str) -> bool:
    v = value.strip().lower()
    if v in UNRESOLVED:
        return True
    for token in ("tbd", "todo", "unknown", "pending"):
        if token in v:
            return True
    return False


def is_production_rs(path: str, root: str = ".") -> bool:
    if not path.startswith("src/") or not path.endswith(".rs"):
        return False
    base = os.path.basename(path)
    if "/tests/" in path:
        return False
    if base == "tests.rs":
        parent = os.path.join(root, os.path.dirname(path))
        for candidate in ("mod.rs", "lib.rs"):
            mod_path = os.path.join(parent, candidate)
            if not os.path.isfile(mod_path):
                continue
            try:
                raw = open(mod_path, encoding="utf-8", errors="replace").read()
            except OSError:
                continue
            if re.search(r"#\[cfg\s*\(\s*test\s*\)\]\s*(?:pub\s+)?mod\s+tests\s*;", raw):
                return False
        return True
    return True


def validate_size_registry(root: str, registry_path: str | None = None) -> list[str]:
    root = os.path.abspath(root)
    path = registry_path or os.path.join(root, "migration-reports/architecture-size-registry.tsv")
    failures: list[str] = []

    def fail(msg: str) -> None:
        failures.append(msg)

    if not os.path.isfile(path):
        fail(f"size registry missing: {path}")
        return failures

    with open(path, encoding="utf-8") as fh:
        header = fh.readline().rstrip("\n")
        if header.split("\t") != HEADER:
            fail(f"size registry header must be {'/'.join(HEADER)}, got {header!r}")
            return failures
        rows: list[tuple[int, str, int, str, str, str, str]] = []
        for lineno, line in enumerate(fh, start=2):
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            parts = line.split("\t")
            if len(parts) != 6:
                fail(f"{path}:{lineno} malformed (need 6 tab-separated fields)")
                continue
            file_path, size_s, state, rationale, reviewer, evidence = parts
            if any(field.strip() == "" for field in (file_path, size_s, state, rationale, reviewer, evidence)):
                fail(f"{path}:{lineno} blank field (fail closed)")
                continue
            for label, value in (
                ("state", state),
                ("rationale", rationale),
                ("reviewer", reviewer),
                ("evidence", evidence),
            ):
                if _unresolved(value):
                    fail(
                        f"{file_path} {label} {value!r} is unresolved "
                        "(TBD/TODO/UNKNOWN/pending cannot satisfy the registry)"
                    )
            if state.strip() not in ALLOWED_STATES:
                fail(
                    f"{file_path} state {state!r} is not allowed "
                    f"(need OK, SPLIT_COMPLETE, or COHESIVE_EXCEPTION)"
                )
            abs_file = file_path if os.path.isabs(file_path) else os.path.join(root, file_path)
            if not os.path.isfile(abs_file):
                fail(f"{path}:{lineno} registered file does not exist: {file_path}")
                continue
            try:
                registered = int(size_s.strip())
            except ValueError:
                fail(f"{file_path} bytes {size_s!r} is not an integer")
                continue
            actual = os.path.getsize(abs_file)
            if actual != registered:
                fail(f"{file_path} size registry stale: registered {registered} actual {actual}")
            if registered >= KB100 and len(rationale.strip()) < 40:
                fail(f"{file_path} is >100 KB but rationale is too short")
            production = is_production_rs(file_path, root)
            if registered > KB150:
                if production and state.strip() != "COHESIVE_EXCEPTION":
                    fail(
                        f"{file_path} production file is >150 KB and must be "
                        "COHESIVE_EXCEPTION with independent review"
                    )
                if production and (
                    "independent" not in evidence.lower()
                    and "independently" not in evidence.lower()
                    and "audit" not in evidence.lower()
                ):
                    fail(
                        f"{file_path} >150 KB cohesive exception needs evidence of independent review"
                    )
            rows.append((lineno, file_path, actual, state, rationale, reviewer, evidence))

    registered_paths = {p for _, p, _, _, _, _, _ in rows}
    src = os.path.join(root, "src")
    if os.path.isdir(src):
        for dirpath, _, files in os.walk(src):
            for name in files:
                if not name.endswith(".rs"):
                    continue
                fpath = os.path.join(dirpath, name)
                rel_path = os.path.relpath(fpath, root).replace(os.sep, "/")
                size = os.path.getsize(fpath)
                if size >= KB100 and is_production_rs(rel_path, root) and rel_path not in registered_paths:
                    fail(
                        f"production file {rel_path} ({size} B >= 100 KB) is missing from size registry"
                    )
    return failures


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    root = argv[0] if argv and not argv[0].startswith("-") else "."
    registry = None
    if "--registry" in argv:
        registry = argv[argv.index("--registry") + 1]
    failures = validate_size_registry(root, registry)
    if failures:
        for msg in failures:
            print(f"ARCH007 {msg}", file=sys.stderr)
        return 1
    print("ARCH007 size registry ok", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
