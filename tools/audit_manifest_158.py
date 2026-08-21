#!/usr/bin/env python3
"""audit_manifest_158.py — Backward-compatible wrapper for 158.1 manifest extraction."""

import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
JAR = sys.argv[1] if len(sys.argv) > 1 else str(Path.home() / "Escritorio/mindustry-linux-64-bit/jre/desktop.jar")

cmd = [
    sys.executable,
    str(REPO_ROOT / "tools" / "mindustry_manifest.py"),
    "--build",
    "158.1",
    "--jar",
    JAR,
    "--source-ref",
    "v158.1",
]
sys.exit(subprocess.call(cmd))
