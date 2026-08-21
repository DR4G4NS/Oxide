#!/usr/bin/env bash
# architecture_guard.sh — capability and size architecture guard.
#
#   Gself architecture-guard self-tests (every rule class must fire)
#   G0   dashmap_guard strict scan (0 unsuppressed DM*, 0 TOOL failures)
#   G1   capability scanner vs ledger (must be empty at certification)
#        ARCH001–ARCH006 / ARCH900: semantic capabilities, not module-name greps
#   G2   no god contexts (ServerContext/GameContext/HandlerContext)
#   G3   size registry (ARCH007): OK / SPLIT_COMPLETE / COHESIVE_EXCEPTION;
#        TBD/TODO/UNKNOWN fail closed; missing oversized production rows fail
#   G4   no NEW dashmap_guard suppressions beyond the recorded baseline
#
# Narrow exceptions: migration-reports/architecture-exceptions.tsv
# (closed path set in tools/archguard/scan.py — not a glob allowlist).
#
# Set ARCHITECTURE_GUARD_SKIP_G0=1 only when a caller already ran
# `dashmap_guard check --paths src,tests --deny-warnings` in the same job.
# Set ARCHITECTURE_GUARD_SKIP_SELFTEST=1 only when a caller already ran
# `python3 tools/archguard/selftest.py` in the same job.

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

failures=0
pass() { echo "  [ok] $1"; }
fail() { echo "  [FAIL] $1" >&2; failures=$((failures+1)); }

LEDGER="${ROOT}/migration-reports/architecture-violations.tsv"
SIZE_REG="${ROOT}/migration-reports/architecture-size-registry.tsv"
SUPP_BASE="${ROOT}/migration-reports/architecture-suppressions.count"
REQUIRED_CLASSES="ARCH001,ARCH002,ARCH003,ARCH004,ARCH005,ARCH006,ARCH900"

echo "== architecture_guard =="

step() { echo; echo "-- $1 --"; }

# Gself — prove every rule class before trusting a zero-row repo scan
step "Gself architecture guard self-tests"
if [ "${ARCHITECTURE_GUARD_SKIP_SELFTEST:-}" = "1" ]; then
  pass "self-tests skipped (ARCHITECTURE_GUARD_SKIP_SELFTEST=1; caller already ran them)"
elif python3 tools/archguard/selftest.py; then
  pass "architecture guard self-tests (all rule classes exercised)"
else
  fail "architecture guard self-tests"
fi

# G0 dashmap_guard strict scan
step "G0 dashmap_guard strict scan"
if [ "${ARCHITECTURE_GUARD_SKIP_G0:-}" = "1" ]; then
  pass "dashmap_guard strict skipped (ARCHITECTURE_GUARD_SKIP_G0=1; caller already scanned)"
elif cargo run --quiet --manifest-path tools/dashmap_guard/Cargo.toml -- \
     check . --paths src,tests --deny-warnings >/tmp/architecture_guard.g0 2>&1; then
  pass "dashmap_guard strict (0 unsuppressed DM*, 0 TOOL)"
else
  fail "dashmap_guard strict scan"
  tail -20 /tmp/architecture_guard.g0 | sed 's/^/    /'
fi

# G1 capability scanner vs ledger
step "G1 forbidden reverse capabilities vs ledger"
if [ ! -f "$LEDGER" ]; then
  fail "violations ledger missing — run tools/gen_architecture_ledger.sh"
else
  header=$(head -1 "$LEDGER")
  if [ "$header" != $'code\tsource\tline\ttarget\tcapability\tchain\towning_phase\tevidence' ]; then
    fail "violations ledger header is not the capability-scanner schema"
  fi
  if ! grep -q '^# RULE_CLASSES_COMPLETED:' "$LEDGER"; then
    fail "violations ledger missing RULE_CLASSES_COMPLETED coverage stamp"
  fi
  cur=$(mktemp)
  set +e
  python3 tools/archguard/scan.py . > "$cur" 2>"$cur.err"
  scan_rc=$?
  set -e
  if [ "$scan_rc" -ne 0 ]; then
    fail "capability scanner failed (exit $scan_rc) — refusing empty-success"
    sed 's/^/    /' "$cur.err" "$cur" || true
  elif ! grep -q "^ARCHGUARD_SCAN_COMPLETE	${REQUIRED_CLASSES}$" "$cur.err"; then
    fail "scanner did not report completion of every rule class"
    sed 's/^/    /' "$cur.err"
  elif grep -q '^EXCEPTIONS_' "$cur"; then
    fail "exceptions file rejected"
    grep '^EXCEPTIONS_' "$cur" | sed 's/^/    /'
  else
    if grep -v '^ARCHGUARD_SCAN_COMPLETE' "$cur.err" | grep -q .; then
      # unexpected stderr besides the trailer is a tool problem
      extra=$(grep -v '^ARCHGUARD_SCAN_COMPLETE' "$cur.err" || true)
      if [ -n "$extra" ]; then
        fail "scanner wrote unexpected diagnostics on stderr"
        echo "$extra" | sed 's/^/    /'
      fi
    fi
    missing=0
    while IFS= read -r row; do
      [ -z "${row:-}" ] && continue
      case "$row" in
        \#*) continue ;;
      esac
      code=$(printf '%s' "$row" | cut -f1)
      source=$(printf '%s' "$row" | cut -f2)
      line=$(printf '%s' "$row" | cut -f3)
      target=$(printf '%s' "$row" | cut -f4)
      if ! grep -F -q $'\t'"${source}"$'\t'"${line}"$'\t'"${target}"$'\t' "$LEDGER"; then
        fail "unrecorded forbidden capability: ${row}"
        missing=$((missing+1))
      fi
    done < "$cur"
    if [ "$missing" = "0" ]; then
      sites=$(grep -c . "$cur" || true)
      pass "all present forbidden capabilities are recorded in the ledger (${sites} sites)"
    fi
  fi
  rm -f "$cur" "$cur.err"
fi

# G2 god contexts (Python: GitHub runners do not ship `rg`)
step "G2 god contexts"
god_out=$(mktemp)
set +e
python3 tools/archguard/textscan.py god "$ROOT" > "$god_out" 2>&1
god_rc=$?
set -e
if [ "$god_rc" -eq 0 ]; then
  pass "no ServerContext/GameContext/HandlerContext"
elif [ "$god_rc" -eq 1 ]; then
  fail "god context type(s) found:"
  sed 's/^/    /' "$god_out"
else
  fail "god-context scanner failed (exit $god_rc) — refusing empty-success"
  sed 's/^/    /' "$god_out" || true
fi
rm -f "$god_out"

# G3 size guardrails vs registry
step "G3 size registry (ARCH007; TBD fails closed)"
if python3 tools/archguard/size.py "$ROOT"; then
  pass "size registry is complete, TBD-free, and matches on-disk sizes"
else
  fail "size registry validation (ARCH007)"
fi

# G4 suppression baseline
step "G4 suppression baseline (no new dashmap_guard suppressions)"
if [ ! -f "$SUPP_BASE" ]; then
  fail "suppression baseline missing — run tools/gen_architecture_ledger.sh"
else
  base=$(cat "$SUPP_BASE")
  now=$(python3 tools/archguard/textscan.py suppressions "$ROOT")
  if [ "$now" -gt "$base" ]; then
    fail "dashmap_guard suppressions grew: baseline=$base now=$now"
  else
    pass "suppression count $now <= baseline $base"
  fi
fi

if [ "$failures" != "0" ]; then
  echo
  echo "architecture_guard FAILED with $failures check(s)."
  exit 1
fi
echo
echo "architecture_guard passed."
