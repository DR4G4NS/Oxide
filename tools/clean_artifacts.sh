#!/usr/bin/env bash
# P2: remove build artifacts and transient smoke outputs without touching
# user work (saves, fixtures, source). Rebuild reproducibly with:
#   cargo build --release
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_dir"

target_size="$(du -sh target 2>/dev/null | cut -f1 || echo 0)"
echo "Removing: target/ ($target_size), tools/**/*.class"
rm -rf target
find tools -name '*.class' -delete
echo "Done. Rebuild with: cargo build --release"
echo "JAR-less CI gates: cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets -- --test-threads=1"
