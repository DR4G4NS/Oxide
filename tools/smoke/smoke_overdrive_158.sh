#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6587}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeOverdrive158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-overdrive-158.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
cp "$project_dir/tools/smoke/smoke-overdrive-world-158.json" "$save_file"

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
java -cp "$desktop_jar:$class_dir" SmokeOverdrive158 "$smoke_port"

boosted_graphite="$(jq '[.tiles[] | select(.position == 3407972)
    | .inventory[] | select(.[0] == 3) | .[1]][0] // 0' "$save_file")"
control_graphite="$(jq '[.tiles[] | select(.position == 5242980)
    | .inventory[] | select(.[0] == 3) | .[1]][0] // 0' "$save_file")"
boosted_coal="$(jq '[.tiles[] | select(.position == 3407972)
    | .inventory[] | select(.[0] == 5) | .[1]][0] // 0' "$save_file")"
boosted_progress="$(jq '.tiles[] | select(.position == 3407972)
    | .production_progress' "$save_file")"
control_progress="$(jq '.tiles[] | select(.position == 5242980)
    | .production_progress' "$save_file")"
boosted_work="$(awk -v items="$boosted_graphite" -v progress="$boosted_progress" \
    'BEGIN { print items * 90 + progress }')"
control_work="$(awk -v items="$control_graphite" -v progress="$control_progress" \
    'BEGIN { print items * 90 + progress }')"

if ! grep -q "finished loading the world" "$server_log" \
        || ! awk -v boosted="$boosted_work" -v control="$control_work" \
            'BEGIN { exit boosted > control * 1.25 ? 0 : 1 }' \
        || (( boosted_coal != 10 - boosted_graphite * 2 )); then
    echo "Overdrive did not persist accelerated graphite production. Artifacts: $run_dir" >&2
    exit 1
fi

echo "Server confirmed exact Overdrive sync and accelerated production" \
    "(boostedWork=$boosted_work controlWork=$control_work)." \
    "Artifacts: $run_dir"
