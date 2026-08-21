#!/usr/bin/env bash
# tools/compat_gate.sh — JAR-less always-on compatibility gate (layer A).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STATIC_ONLY=0
SKIP_TESTS=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build) shift 2 ;;
    --static-only) STATIC_ONLY=1; shift ;;
    --skip-tests) SKIP_TESTS=1; shift ;;
    *) echo "error: unknown argument $1" >&2; exit 1 ;;
  esac
done

TARGET_BUILD="$(python3 - <<'PY'
import sys
sys.path.insert(0, "tools")
from compatlib.current import current_build
print(current_build())
PY
)"

echo "============================================================"
echo "== Mindustry Compatibility Gate A (JAR-less, target $TARGET_BUILD) =="
echo "============================================================"

pass() { echo "  [ok] $1"; }
fail() { echo "  [FAIL] $1" >&2; exit 1; }
step() { echo; echo "== $1 =="; }

step "1 Schema / ledger / current.toml"
python3 tools/compat_selftest.py || fail "compat self-tests"
python3 tools/mindustry_manifest.py --build "$TARGET_BUILD" || fail "committed artifacts"
python3 tools/cert_ledger.py --build "$TARGET_BUILD" || fail "certification ledger"
pass "schema + ledger"

step "2 DashMap Guard"
cargo test --manifest-path tools/dashmap_guard/Cargo.toml --quiet
cargo run --quiet --manifest-path tools/dashmap_guard/Cargo.toml -- check . --paths src,tests --deny-warnings
pass "dashmap_guard"

step "3 Architecture Guard"
ARCHITECTURE_GUARD_SKIP_G0=1 bash tools/architecture_guard.sh
pass "architecture_guard"

if [[ "$STATIC_ONLY" == "1" ]]; then
  echo "Compatibility gate (static-only) passed for Build $TARGET_BUILD."
  exit 0
fi

step "4 fmt"
cargo fmt --all -- --check
pass "fmt"

step "5 clippy"
cargo clippy --all-targets -- -D warnings
pass "clippy"

if [[ "$SKIP_TESTS" == "1" ]]; then
  echo "Compatibility gate passed (tests skipped)."
  exit 0
fi

step "6 Focused compatibility tests"
cargo test --lib rust_packet_ids_match_committed_159_7_packets_json -- --test-threads=1
cargo test --lib official_159_7_save13_fixture_reads rust_save12_empty_patches_round_trip_read_map current_personalize_keeps_159_7_data_patch_prefix -- --test-threads=1
cargo test --test msav_world_entity_framing -- --test-threads=1
pass "focused tests"

step "7 One low-core all-targets suite"
python3 tools/low_core_test.py --cpus 0,1 --timeout-seconds 1800 -- cargo test --all-targets -- --test-threads=1
pass "all-targets"

step "8 git cleanliness"
if [ -n "$(git status --porcelain)" ]; then
  git status --porcelain >&2
  fail "working tree is dirty"
fi
git diff --check
pass "clean tree"

echo
echo "== Compatibility Gate A PASSED: Build $TARGET_BUILD (not a JAR certification) =="
