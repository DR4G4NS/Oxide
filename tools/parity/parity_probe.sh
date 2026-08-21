#!/usr/bin/env bash
# tools/parity/parity_probe.sh — Build-parameterized parity probe runner.
#
# Compiles and executes a Java probe from tests/parity/probes/ against an
# official Mindustry desktop/server JAR and writes its normalized JSON output
# into tests/parity/fixtures/<probe>.json (or custom output path).
#
# Usage:
#   tools/parity/parity_probe.sh [--build <build>] [--jar <desktop.jar>] <probe-class> [fixture-out.json]
#
# Environment:
#   MINDUSTRY_TARGET_BUILD   Target build version (default: from compat/current.toml, or 159.7)
#   MINDUSTRY_DESKTOP_JAR    Path to official desktop/server jar for target build

set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Parse optional flags
target_build="${MINDUSTRY_TARGET_BUILD:-}"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-}"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --build)
            target_build="$2"
            shift 2
            ;;
        --jar)
            desktop_jar="$2"
            shift 2
            ;;
        -*)
            echo "error: unknown flag $1" >&2
            exit 1
            ;;
        *)
            break
            ;;
    esac
done

# If target_build not specified, read from compat/current.toml or default to 159.7
if [[ -z "$target_build" ]]; then
    if [[ -f "$project_dir/compat/current.toml" ]]; then
        target_build="$(grep '^build = ' "$project_dir/compat/current.toml" | head -1 | cut -d'"' -f2 || true)"
    fi
    target_build="${target_build:-159.7}"
fi

# Determine default JAR if not provided
if [[ -z "$desktop_jar" ]]; then
    if [[ "$target_build" == "159.7" ]]; then
        for candidate in \
            "/home/dr4g4ns/Escritorio/mindustry-159.7/jre/159.7.jar" \
            "/home/dr4g4ns/Escritorio/mindustry-159.7/Mindustry.jar" \
            "/home/ubuntu/Mindustry/desktop.jar"; do
            if [[ -f "$candidate" ]]; then
                desktop_jar="$candidate"
                break
            fi
        done
    elif [[ "$target_build" == "158.1" ]]; then
        for candidate in \
            "/home/dr4g4ns/Escritorio/mindustry-linux-64-bit/jre/desktop.jar" \
            "/home/ubuntu/Mindustry/desktop.jar"; do
            if [[ -f "$candidate" ]]; then
                desktop_jar="$candidate"
                break
            fi
        done
    fi
fi

if [[ $# -lt 1 ]]; then
    echo "Usage: tools/parity/parity_probe.sh [--build <build>] [--jar <desktop.jar>] <probe-class> [fixture-out.json]" >&2
    exit 1
fi

probe="$1"
out="${2:-$project_dir/tests/parity/fixtures/$probe.json}"
class_dir="$project_dir/target/parity-${target_build//./_}-classes"

# --- version gate: only an official jar matching target_build is accepted ---
if [[ -z "$desktop_jar" || ! -f "$desktop_jar" ]]; then
    echo "error: desktop jar not found for build $target_build (set MINDUSTRY_DESKTOP_JAR or --jar)" >&2
    exit 1
fi

jar_version="$(unzip -p "$desktop_jar" version.properties 2>/dev/null | tr -d '\r' || true)"
jar_build="$(printf '%s\n' "$jar_version" | sed -n 's/^build=//p')"
jar_type="$(printf '%s\n' "$jar_version" | sed -n 's/^type=//p')"

if [[ "$jar_build" != "$target_build" || "$jar_type" != "official" ]]; then
    echo "error: probe target must be official build $target_build, but $desktop_jar reports build=$jar_build type=$jar_type" >&2
    exit 2
fi

mkdir -p "$class_dir" "$(dirname "$out")"
javac -d "$class_dir" -cp "$desktop_jar" "$project_dir/tests/parity/probes/$probe.java"
tmp_out="$(mktemp "$(dirname "$out")/.$probe.XXXXXX")"
trap 'rm -f "$tmp_out"' EXIT
java -cp "$desktop_jar:$class_dir" "$probe" > "$tmp_out"

# Drop anything before the opening '{' of the JSON object
awk 'BEGIN{keep=0} /^[[:space:]]*\{/{keep=1} keep' "$tmp_out" > "$out"
if ! python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$out" 2>/dev/null; then
    echo "error: probe '$probe' did not emit a parseable JSON document:" >&2
    sed -n '1,10p' "$out" >&2
    exit 3
fi

echo "fixture written: $out (from $desktop_jar, build=$jar_build type=$jar_type)"
