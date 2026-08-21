#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6594}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeRebuild158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-rebuild-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-rebuild-world-158.json" "$save_file"

target/release/oxide --port "$smoke_port" --save-file "$save_file" \
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
java -cp "$desktop_jar:$class_dir" SmokeRebuild158 "$smoke_port"
sleep 1.2

grep -q '"block": 216' "$save_file"
grep -q '"command": 2' "$save_file"
grep -q '9994' "$save_file"
grep -q 'finished loading the world' "$server_log"

echo "Server confirmed persisted autonomous rebuild. Artifacts: $run_dir"
