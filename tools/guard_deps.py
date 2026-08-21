#!/usr/bin/env python3
"""Compatibility wrapper for tools/archguard/scan.py.

Stdout: TSV diagnostics (new ledger columns). A crash cannot look like
an empty-success scan: the process exits 2 and prints ARCHGUARD_TOOL_FAIL.
"""
from __future__ import annotations

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(_HERE, "archguard"))

from scan import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
