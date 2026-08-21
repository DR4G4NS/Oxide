"""Atomic architecture-violations.tsv generation.

Never truncates the canonical ledger before a successful scan finishes.
"""
from __future__ import annotations

import os
import sys

# Allow `python3 tools/archguard/ledger.py` and imports from tools/.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE not in sys.path:
    sys.path.insert(0, _HERE)

from scan import RULE_CLASSES, scan_root, write_ledger_atomic  # noqa: E402


def generate(root: str, ledger_path: str) -> int:
    try:
        diags, classes = scan_root(root)
    except SystemExit as exc:
        print(str(exc), file=sys.stderr)
        return 2
    except Exception as exc:
        print(f"ARCHGUARD_TOOL_FAIL {type(exc).__name__}: {exc}", file=sys.stderr)
        return 2
    if list(classes) != list(RULE_CLASSES):
        print("scanner did not complete every configured rule class", file=sys.stderr)
        return 2
    write_ledger_atomic(ledger_path, diags, classes)
    print(f"violations ledger : {ledger_path} ({len(diags)} rows)")
    print("ARCHGUARD_SCAN_COMPLETE\t" + ",".join(classes), file=sys.stderr)
    return 0


def main() -> int:
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    ledger = (
        sys.argv[2]
        if len(sys.argv) > 2
        else os.path.join(root, "migration-reports/architecture-violations.tsv")
    )
    return generate(os.path.abspath(root), os.path.abspath(ledger))


if __name__ == "__main__":
    sys.exit(main())
