#!/usr/bin/env python3
"""Download and verify the exact current-target Mindustry release JAR."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from compatlib.release_jar import main  # noqa: E402


if __name__ == "__main__":
    raise SystemExit(main())
