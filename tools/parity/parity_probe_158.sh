#!/usr/bin/env bash
# Backward-compatible wrapper for parity_probe_158.sh
# Delegates to tools/parity/parity_probe.sh --build 158.1
exec "$(dirname "$0")/parity_probe.sh" --build 158.1 "$@"
