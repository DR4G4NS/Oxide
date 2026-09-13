#!/usr/bin/env bash
set -euo pipefail

# 159.7 sandbox build/break smoke. Asserts streamed Rules (inflate + UTF-8,
# not NetworkIO.readWorld) and that BeginPlace/ConstructFinish plus
# BeginBreak/DeconstructFinish all arrive. Skips when the JAR is absent.

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_1597_JAR:-/home/dr4g4ns/Escritorio/mindustry-159.7/jre/159.7.jar}"
smoke_port="${1:-6598}"
class_dir="$project_dir/target/protocol-1597-classes"

if [[ ! -f "$desktop_jar" ]]; then
    echo "skip: 159.7 JAR not found at $desktop_jar (set MINDUSTRY_1597_JAR)"
    exit 0
fi

cd "$project_dir"
cargo() { (exec -a cargo "$HOME/.cargo/bin/cargo" "$@"); }
case "${OXIDE_SMOKE_PROFILE:-release}" in
    release) cargo build --release; oxide_binary=target/release/oxide ;;
    debug) cargo build; oxide_binary=target/debug/oxide ;;
    *) echo "OXIDE_SMOKE_PROFILE must be release or debug" >&2; exit 2 ;;
esac
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeSandboxBuild1597.java
run_dir="$(mktemp -d "$project_dir/target/smoke-sandbox-build-1597.XXXXXX")"
server_log="$run_dir/server.log"
client_log="$run_dir/client.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-sandbox-build-world-1597.json" "$save_file"

"$oxide_binary" \
    --port "$smoke_port" \
    --no-tui \
    --mode sandbox \
    --save-file "$save_file" \
    >"$server_log" 2>&1 &
server_pid=$!

cleanup() {
    if kill -0 "$server_pid" 2>/dev/null; then
        kill "$server_pid"
        wait "$server_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

sleep 0.5
java -cp "$desktop_jar:$class_dir" SmokeSandboxBuild1597 "$smoke_port" | tee "$client_log"

if ! grep -q "finished loading the world" "$server_log"; then
    echo "Server did not confirm the joined state. Log: $server_log" >&2
    exit 1
fi
if ! grep -q "SMOKE_OK sandbox-build" "$client_log"; then
    echo "Sandbox build smoke did not assert success. Artifacts: $run_dir" >&2
    exit 1
fi
echo "159.7 sandbox build smoke passed. Artifacts: $run_dir"
