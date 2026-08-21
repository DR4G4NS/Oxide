#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6578}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeJoin158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-join-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-world-158.json" "$save_file"

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
java -cp "$desktop_jar:$class_dir" SmokeJoin158 "$smoke_port"

if ! grep -q "finished loading the world" "$server_log"; then
    echo "Server did not confirm the joined state. Log: $server_log" >&2
    exit 1
fi
if ! grep -q "smoke-chat-158" "$server_log"; then
    echo "Server did not decode the 158.1 chat packet. Log: $server_log" >&2
    exit 1
fi
if ! grep -q '"x": 328.0' "$save_file"; then
    echo "Server did not accept movement from the replacement unit. Save: $save_file" >&2
    exit 1
fi
if grep -q '"block": 216' "$save_file" || ! grep -q '9998' "$save_file"; then
    echo "Server did not deconstruct/refund/mine as expected. Save: $save_file" >&2
    exit 1
fi
if grep -q '"id": 3000000' "$save_file" \
        || ! grep -q '"id": 3000001' "$save_file"; then
    echo "Server did not persist the intended Alpha kill. Save: $save_file" >&2
    exit 1
fi
if ! grep -q '"unit_id": 2500000' "$save_file" \
        || ! grep -q '"dead": false' "$save_file"; then
    echo "Server did not persist the player respawn. Save: $save_file" >&2
    exit 1
fi

java -cp "$desktop_jar:$class_dir" SmokeJoin158 "$smoke_port" join-only
joined_count="$(grep -c "finished loading the world" "$server_log")"
if (( joined_count < 2 )); then
    echo "Server did not complete the persisted reconnection. Log: $server_log" >&2
    exit 1
fi

echo "Server confirmed gameplay, lifecycle and reconnection. Artifacts: $run_dir"
