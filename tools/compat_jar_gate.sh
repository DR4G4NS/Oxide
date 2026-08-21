#!/usr/bin/env bash
# tools/compat_jar_gate.sh — exact JAR-backed certification (layer B).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

python3 - <<'PY'
import hashlib, json, os, subprocess, sys, tempfile, zipfile
from pathlib import Path
sys.path.insert(0, "tools")
from compatlib.current import load_current, current_build, current_jar_sha256, current_source_commit

doc = load_current()
build = current_build(doc)
want_sha = current_jar_sha256(doc)
jar = os.environ.get("MINDUSTRY_CURRENT_JAR") or os.environ.get("MINDUSTRY_DESKTOP_JAR")
if not jar or not Path(jar).exists():
    sys.exit("Set MINDUSTRY_CURRENT_JAR to the official current-target desktop JAR")
data = Path(jar).read_bytes()
got = hashlib.sha256(data).hexdigest()
if got != want_sha:
    sys.exit(f"JAR SHA mismatch: {got} != {want_sha}")
with zipfile.ZipFile(jar) as z:
    props = z.read("version.properties").decode()
if f"build={build}" not in props.replace(" ", ""):
    # properties uses build=159.7
    if f"build={build}" not in props and f"build = {build}" not in props:
        if f"build={build}" not in props.replace("\r", ""):
            if not any(line.strip() == f"build={build}" for line in props.splitlines()):
                sys.exit(f"JAR version.properties does not report {build}:\n{props}")
print(f"JAR identity OK for {build} sha={got}")
tmpdir = tempfile.mkdtemp(prefix="compat-regen-")
# regenerate into tmp by copying tree after generation — generator writes compat/<build>
# Use --check against committed tree with this JAR.
rc = subprocess.call([
    sys.executable, "tools/mindustry_manifest.py",
    "--build", build,
    "--jar", jar,
    "--source-ref", doc["target"]["source_tag"],
    "--source-commit", current_source_commit(doc),
    "--check",
])
sys.exit(rc)
PY

python3 tools/compat_jar_probes.py
