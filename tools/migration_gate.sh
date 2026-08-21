#!/usr/bin/env bash
# Migration gate — the mandatory, pre-merge checklist for the Migration branch.
#
# Usage:
#   bash tools/migration_gate.sh [--static-only] [--skip-tests]
#
# Exit code 0 only when every gate passes.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STATIC_ONLY=0
SKIP_TESTS=0
for arg in "$@"; do
  case "$arg" in
    --static-only) STATIC_ONLY=1 ;;
    --skip-tests)  SKIP_TESTS=1 ;;
    --slow)        ;; # accepted for compatibility; low-core always runs in full mode
  esac
done

pass()  { echo "  [ok] $1"; }
fail()  { echo "  [FAIL] $1" >&2; }
step()  { echo; echo "== $1 =="; }

step "1/10 dashmap_guard self-tests"
if cargo test --manifest-path tools/dashmap_guard/Cargo.toml --quiet; then
  pass "dashmap_guard crate tests"
else
  fail "dashmap_guard crate tests"
  exit 1
fi

step "2/10 dashmap_guard (deterministic DashMap guard analyzer)"
grep -q 'name = "dashmap"' Cargo.lock || { echo "Cargo.lock has no dashmap entry" >&2; exit 2; }
RESOLVED=$(grep -A1 'name = "dashmap"' Cargo.lock | grep version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
echo "  resolved dashmap: $RESOLVED"
if cargo run --quiet --manifest-path tools/dashmap_guard/Cargo.toml -- check . --paths src,tests --deny-warnings; then
  pass "dashmap_guard: zero blocking diagnostics"
else
  fail "dashmap_guard: blocking DashMap guard hazard(s) found — fix them, do not weaken the gate"
  exit 1
fi

if [ "$STATIC_ONLY" = "1" ]; then
  echo
  echo "migration gate (static-only) passed."
  exit 0
fi

step "3/10 cargo fmt"
if cargo fmt --all -- --check; then
  pass "fmt"
else
  fail "cargo fmt --check"
  exit 1
fi

step "4/10 cargo clippy (all targets, -D warnings)"
if cargo clippy --all-targets -- -D warnings; then
  pass "clippy"
else
  fail "clippy"
  exit 1
fi

if [ "$SKIP_TESTS" = "1" ]; then
  echo
  echo "migration gate passed (fmt/clippy; tests skipped)."
  exit 0
fi

step "5/10 parity tests"
if cargo test --lib parity -- --test-threads=1; then
  pass "parity"
else
  fail "parity"
  exit 1
fi

step "6/10 MSAV world-entity framing"
if cargo test --test msav_world_entity_framing -- --test-threads=1; then
  pass "msav framing"
else
  fail "msav framing"
  exit 1
fi

step "7/10 cargo test (all targets, --test-threads=1)"
if cargo test --all-targets -- --test-threads=1; then
  pass "tests"
else
  fail "cargo test"
  exit 1
fi

step "8/10 low-core watchdog self-test"
if python3 tools/low_core_test.py --self-test --cpus 0,1; then
  pass "watchdog self-test"
else
  fail "watchdog self-test"
  exit 1
fi

step "9/10 low-core full suite"
if python3 tools/low_core_test.py \
    --cpus 0,1 \
    --timeout-seconds 1800 \
    -- cargo test --all-targets -- --test-threads=1; then
  pass "low-core full suite"
else
  fail "low-core full suite"
  exit 1
fi

step "10/10 git cleanliness"
if [ -n "$(git status --porcelain)" ]; then
  git status --porcelain >&2
  fail "working tree is dirty"
  exit 1
fi
git diff --check
pass "git diff --check / clean tree"

echo
echo "migration gate passed."
