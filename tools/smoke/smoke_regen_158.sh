#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6589}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeRegen158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-regen-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-regen-world-158.json" "$save_file"

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
java -cp "$desktop_jar:$class_dir" SmokeRegen158 "$smoke_port"

wall_health="$(jq '.tiles[] | select(.position == 3539044) | .health' "$save_file")"
hydrogen="$(jq '.tiles[] | select(.position == 3276900) | .liquid_amount' "$save_file")"
if ! grep -q "finished loading the world" "$server_log" \
        || ! awk -v health="$wall_health" -v hydrogen="$hydrogen" \
            'BEGIN { exit health > 100 && hydrogen < 10 ? 0 : 1 }'; then
    echo "Regen Projector did not heal/consume/persist. Artifacts: $run_dir" >&2
    exit 1
fi

echo "Server confirmed exact RegenProjector sync and persistent healing" \
    "(health=$wall_health hydrogen=$hydrogen). Artifacts: $run_dir"
