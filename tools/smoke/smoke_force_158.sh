#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6588}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeForce158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-force-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-force-world-158.json" "$save_file"

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
java -cp "$desktop_jar:$class_dir" SmokeForce158 "$smoke_port"

force_health="$(jq '.tiles[] | select(.position == 3276900) | .health' "$save_file")"
shield_buildup="$(jq '.tiles[] | select(.position == 3276900)
    | .production_progress' "$save_file")"
if ! grep -q "finished loading the world" "$server_log" \
        || ! awk -v health="$force_health" -v buildup="$shield_buildup" \
            'BEGIN { exit health == 360 && buildup > 0 ? 0 : 1 }'; then
    echo "Force Projector did not absorb/persist enemy fire. Artifacts: $run_dir" >&2
    exit 1
fi

echo "Server confirmed exact ForceBuild sync and authoritative absorption" \
    "(health=$force_health buildup=$shield_buildup). Artifacts: $run_dir"
