#!/usr/bin/env bash
# gen_architecture_ledger.sh — regenerate the violation/size/suppression
# records from the CURRENT source tree.
#
# Root-maintained. Run only at a green checkpoint to re-baseline after a phase
# deliberately and provably removes violations (rows disappear) or when newly
# introduced violations are unavoidable and must be explicitly recorded with
# evidence + owning phase. Not a way to hide violations: architecture_guard.sh
# fails if any present violation is NOT in the ledger.
#
# Atomic ledger write: the canonical TSV is replaced only after a successful
# scan that completed every rule class. A tool failure leaves the previous
# ledger untouched (never truncate-then-scan).
#
# Size registry: NEVER writes TBD. Existing rows keep state/rationale/reviewer/
# evidence; only the bytes column is refreshed from disk. Unknown oversized
# production files are NOT auto-inserted — the guard fails closed until a
# human writes a real OK / SPLIT_COMPLETE / COHESIVE_EXCEPTION row.

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

mkdir -p migration-reports
LEDGER="${ROOT}/migration-reports/architecture-violations.tsv"
SIZE_REG="${ROOT}/migration-reports/architecture-size-registry.tsv"
SUPP_BASE="${ROOT}/migration-reports/architecture-suppressions.count"

if ! python3 tools/archguard/ledger.py "$ROOT" "$LEDGER"; then
  echo "capability scan failed; canonical ledger left unchanged" >&2
  exit 1
fi

if ! python3 - "$SIZE_REG" "$ROOT" <<'PY'
import os, sys
reg_path, root = sys.argv[1], sys.argv[2]
HEADER = ["path", "bytes", "state", "rationale", "reviewer", "evidence"]
if not os.path.isfile(reg_path):
    print("size registry missing; write a real registry before generating", file=sys.stderr)
    sys.exit(1)
with open(reg_path, encoding="utf-8") as fh:
    header = fh.readline().rstrip("\n")
    if header.split("\t") != HEADER:
        print(f"size registry header invalid: {header!r}", file=sys.stderr)
        sys.exit(1)
    rows = []
    for lineno, line in enumerate(fh, start=2):
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) != 6:
            print(f"{reg_path}:{lineno} malformed; refusing to rewrite", file=sys.stderr)
            sys.exit(1)
        path, _size, state, rationale, reviewer, evidence = parts
        abs_path = os.path.join(root, path)
        if not os.path.isfile(abs_path):
            print(f"{path} missing on disk; refusing to rewrite registry", file=sys.stderr)
            sys.exit(1)
        size = os.path.getsize(abs_path)
        rows.append((path, size, state, rationale, reviewer, evidence))
tmp = reg_path + ".tmp"
with open(tmp, "w", encoding="utf-8") as fh:
    fh.write("\t".join(HEADER) + "\n")
    for row in rows:
        fh.write("\t".join(str(c) for c in row) + "\n")
    fh.flush()
    os.fsync(fh.fileno())
os.replace(tmp, reg_path)
print(f"size registry     : {reg_path} ({len(rows)} rows)")
PY
then
  echo "size registry refresh failed; canonical registry left unchanged" >&2
  exit 1
fi

python3 tools/archguard/textscan.py suppressions "$ROOT" > "$SUPP_BASE"

echo "suppression base  : $(cat "$SUPP_BASE")"
