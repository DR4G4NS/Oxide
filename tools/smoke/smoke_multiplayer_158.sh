#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6579}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeMultiplayer158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-multiplayer-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-multiplayer-world-158.json" "$save_file"

target/release/oxide \
    --port "$smoke_port" \
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
java -cp "$desktop_jar:$class_dir" SmokeMultiplayer158 "$smoke_port"

joined_count="$(grep -c "finished loading the world" "$server_log")"
if (( joined_count < 2 )); then
    echo "Both clients did not reach joined state. Log: $server_log" >&2
    exit 1
fi
if ! grep -q "multi-alpha" "$server_log" || ! grep -q "multi-beta" "$server_log"; then
    echo "Server did not register both client identities. Log: $server_log" >&2
    exit 1
fi
if ! grep -q '"uuid": "AQIDBAUGBwg="' "$save_file" \
        || ! grep -q '"uuid": "CAcGBQQDAgE="' "$save_file" \
        || ! grep -q '"x": 324.0' "$save_file" \
        || ! grep -q '"x": 328.0' "$save_file"; then
    echo "Server did not persist both player profiles/movements. Save: $save_file" >&2
    exit 1
fi

echo "Server confirmed simultaneous replication and cleanup. Artifacts: $run_dir"
