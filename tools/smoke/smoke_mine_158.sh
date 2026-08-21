#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6591}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeMine158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-mine-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-mine-world-158.json" "$save_file"

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
java -cp "$desktop_jar:$class_dir" SmokeMine158 "$smoke_port"

mine_health="$(jq '[.tiles[] | select(.position == 3801188) | .health][0] // 0' "$save_file")"
minimum_enemy_health="$(jq '[.enemies[].health] | min // 0' "$save_file")"
enemy_count="$(jq '[.enemies[]] | length' "$save_file")"
if ! grep -q "finished loading the world" "$server_log" \
        || ! awk -v mine="$mine_health" -v enemy="$minimum_enemy_health" -v count="$enemy_count" \
            'BEGIN { exit (mine < 50 && (count < 3 || enemy < 150)) ? 0 : 1 }'; then
    echo "Shock Mine did not damage/persist combat. Artifacts: $run_dir" >&2
    exit 1
fi

echo "Server confirmed exact ShockMine sync and ground lightning" \
    "(mineHealth=$mine_health minEnemyHealth=$minimum_enemy_health). Artifacts: $run_dir"
