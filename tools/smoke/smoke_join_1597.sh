#!/usr/bin/env bash
set -euo pipefail

# Real-socket join smoke against the official Mindustry v8 159.7 desktop JAR.
# Uses the same battle-tested client flow as smoke_join_158.sh, compiled
# against the 159.7 JAR so every packet is produced by the official
# serializer, and announces Version.build = 159 like a real 159.7 client.

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_1597_JAR:-/home/dr4g4ns/Escritorio/mindustry-159.7/jre/desktop.jar}"
smoke_port="${1:-6597}"
class_dir="$project_dir/target/protocol-1597-classes"

cd "$project_dir"
case "${OXIDE_SMOKE_PROFILE:-release}" in
    release) cargo build --release; oxide_binary=target/release/oxide ;;
    debug) cargo build; oxide_binary=target/debug/oxide ;;
    *) echo "OXIDE_SMOKE_PROFILE must be release or debug" >&2; exit 2 ;;
esac
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeJoin1597.java
run_dir="$(mktemp -d "$project_dir/target/smoke-join-1597.XXXXXX")"
server_log="$run_dir/server.log"
save_file="$run_dir/world.json"
server_args=(--no-tui)
client_args=()
if [[ -n "${OXIDE_SMOKE_MAP_FILE:-}" ]]; then
    server_args+=(--map-file "$OXIDE_SMOKE_MAP_FILE" --mode "${OXIDE_SMOKE_MODE:-survival}")
    client_args+=(join-only)
else
    cp "$project_dir/tools/smoke/smoke-world-158.json" "$save_file"
    # The lifecycle probe needs a living respawn core. The historical fixture's
    # 240 HP core can lose the game before the Horizon kills the test player.
    python3 - "$save_file" <<'PY'
import json, sys
from pathlib import Path
path = Path(sys.argv[1])
world = json.loads(path.read_text())
world["core_health"] = 6000
path.write_text(json.dumps(world))
PY
fi

"$oxide_binary" \
    --port "$smoke_port" \
    --save-file "$save_file" \
    "${server_args[@]}" \
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
java -cp "$desktop_jar:$class_dir" SmokeJoin1597 "$smoke_port" "${client_args[@]}"
java -cp "$desktop_jar:$class_dir" SmokeJoin1597 "$smoke_port" join-only

if ! grep -q "finished loading the world" "$server_log"; then
    echo "Server did not confirm the joined state. Log: $server_log" >&2
    exit 1
fi
echo "159.7 join smoke passed."
