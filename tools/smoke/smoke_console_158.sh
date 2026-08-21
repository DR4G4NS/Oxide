#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
smoke_port="${1:-6580}"
class_dir="$project_dir/target/protocol-158-classes"

cd "$project_dir"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeJoin158.java
run_dir="$(mktemp -d "$project_dir/target/smoke-console-158.XXXXXX")"
server_log="$run_dir/server.log"
client_log="$run_dir/client.log"
save_file="$run_dir/world.json"
console_fifo="$run_dir/console.fifo"
cp "$project_dir/tools/smoke/smoke-multiplayer-world-158.json" "$save_file"
mkfifo "$console_fifo"
exec 3<>"$console_fifo"

target/release/oxide \
    --port "$smoke_port" \
    --save-file "$save_file" \
    <&3 >"$server_log" 2>&1 &
server_pid=$!
client_pid=""

cleanup() {
    exec 3>&-
    if [[ -n "$client_pid" ]] && kill -0 "$client_pid" 2>/dev/null; then
        kill "$client_pid"
        wait "$client_pid" 2>/dev/null || true
    fi
    if kill -0 "$server_pid" 2>/dev/null; then
        kill "$server_pid"
        wait "$server_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

sleep 0.5
java -cp "$desktop_jar:$class_dir" SmokeJoin158 "$smoke_port" console \
    >"$client_log" 2>&1 &
client_pid=$!

joined=false
for _ in {1..100}; do
    if grep -q "finished loading the world" "$server_log"; then
        joined=true
        break
    fi
    sleep 0.1
done
if [[ "$joined" != true ]]; then
    echo "Console client did not join. Server log: $server_log" >&2
    exit 1
fi

printf '%s\n' "save console_before" >&3
slot_file="$run_dir/world-console_before.json"
for _ in {1..50}; do
    [[ -f "$slot_file" ]] && break
    sleep 0.1
done
if [[ ! -f "$slot_file" ]]; then
    echo "Console save slot was not created. Server log: $server_log" >&2
    exit 1
fi

printf '%s\n' "say smoke-console-158" >&3
sleep 0.2
printf '%s\n' "gameover" >&3
# Baseline contract (desktop 158.1 / this port):
# - `gameover` sets in-memory GameState.game_over and emits Call.gameOver to
#   connected clients. That is what consoleGameOver on the JAR client checks.
# - The flag is ephemeral: persist_tiles omits `game_over` (see
#   tests/hot_host_tests.rs::game_over_is_ephemeral_never_persisted_nor_restored
#   and the load path in listener). Official Config.autosaveSpacing is 300 s;
#   roundExtraTime / round_wait_ticks is 12 s for map rotation, not JSON persist.
# The previous assertion (`"game_over": true` in $save_file within 1.2 s) was
# therefore stale even at d028480b — it tested a persist cadence the server
# never had. Do not change runtime autosave/gameover timing to appease it.
sleep 0.5
if grep -q '"game_over": true' "$save_file"; then
    echo "Console gameover unexpectedly persisted the ephemeral flag. Save: $save_file" >&2
    exit 1
fi
printf '%s\n' "load console_before" >&3
sleep 0.5
printf '%s\n' "kick smoke-158" >&3

wait "$client_pid"
client_pid=""
if ! grep -q "consoleSay=true consoleGameOver=true consoleKick=true" "$client_log"; then
    echo "Client missed one or more console RPCs. Client log: $client_log" >&2
    exit 1
fi
if grep -q '"game_over": true' "$slot_file"; then
    echo "Console save slot persisted ephemeral game_over=true. Slot: $slot_file" >&2
    exit 1
fi

echo "Server confirmed save, say, gameover, load and kick. Artifacts: $run_dir"
