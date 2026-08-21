#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6590}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeShockwave158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-shockwave-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-shockwave-world-158.json" "$save_file"

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
java -cp "$desktop_jar:$class_dir" SmokeShockwave158 "$smoke_port"

tower_health="$(jq '.tiles[] | select(.position == 3276900) | .health' "$save_file")"
cyanogen="$(jq '.tiles[] | select(.position == 3276900) | .liquid_amount' "$save_file")"
reload="$(jq '.tiles[] | select(.position == 3276900)
    | .production_progress' "$save_file")"
if ! grep -q "finished loading the world" "$server_log" \
        || ! awk -v health="$tower_health" -v liquid="$cyanogen" -v reload="$reload" \
            'BEGIN { exit health > 0 && liquid < 15 && reload < 80 ? 0 : 1 }'; then
    echo "Shockwave Tower did not intercept/consume/persist. Artifacts: $run_dir" >&2
    exit 1
fi

echo "Server confirmed exact ShockwaveTower sync and projectile interception" \
    "(health=$tower_health cyanogen=$cyanogen reload=$reload). Artifacts: $run_dir"
