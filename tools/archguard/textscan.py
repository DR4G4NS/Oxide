"""Portable G2/G4 text scans. CI runners do not ship ripgrep (`rg`).

G2: fail-closed god-context struct detection (no `|| true` on missing tools).
G4: DashMap suppression line count under `src/`, matching `rg -c` + sum.
"""
from __future__ import annotations

import os
import re
import sys

GOD_RE = re.compile(
    r"^\s*(pub(\([^)]*\))?\s+)?struct\s+(ServerContext|GameContext|HandlerContext)\b"
)
SUPPRESSION = "dashmap-guard: allow"


def _iter_rs(root: str, rel_dir: str = "src"):
    base = os.path.join(root, rel_dir)
    if not os.path.isdir(base):
        raise FileNotFoundError(f"{rel_dir}/ is missing under {root}")
    for dirpath, _, files in os.walk(base):
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, root).replace(os.sep, "/")
            yield path, rel


def find_god_contexts(root: str) -> list[str]:
    hits: list[str] = []
    for path, rel in _iter_rs(root):
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if GOD_RE.search(line):
                    hits.append(rel)
                    break
    return hits


def count_dashmap_suppressions(root: str) -> int:
    total = 0
    for path, _rel in _iter_rs(root):
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                if SUPPRESSION in line:
                    total += 1
    return total


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if len(argv) < 1 or argv[0] not in ("god", "suppressions"):
        print("usage: textscan.py god|suppressions [root]", file=sys.stderr)
        return 2
    mode = argv[0]
    root = os.path.abspath(argv[1] if len(argv) > 1 else ".")
    try:
        if mode == "god":
            hits = find_god_contexts(root)
            if hits:
                for rel in hits:
                    print(rel)
                return 1
            return 0
        print(count_dashmap_suppressions(root))
        return 0
    except FileNotFoundError as exc:
        print(str(exc), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
