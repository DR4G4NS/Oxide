#!/usr/bin/env bash
# Long end-to-end test with the official Mindustry desktop 158.1 client:
#   economy build -> waves 1-10 organic + counter jump to 60 -> save -> restart ->
#   continuation.
#
# Usage: tools/smoke/e2e_long_158.sh [port] [minutes]
#   port    server port (default 6590)
#   minutes overall timeout budget (default 25)
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/ubuntu/Mindustry/desktop.jar}"
e2e_port="${1:-6590}"
timeout_minutes="${2:-25}"
class_dir="$project_dir/target/protocol-158-classes"
now() { date '+%Y-%m-%d %H:%M:%S'; }

cd "$project_dir"
echo "[$(now)] building release"
cargo build --release
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/E2ELong158.java

run_dir="$(mktemp -d "$project_dir/target/e2e-long-158.XXXXXX")"
phase1_dir="$run_dir/phase1"
phase2_dir="$run_dir/phase2"
mkdir -p "$phase1_dir" "$phase2_dir"
phase1_log="$phase1_dir/server.log"
phase1_client="$phase1_dir/client.log"
phase1_console="$phase1_dir/console.fifo"
phase1_save="$phase1_dir/world.json"
phase1_slot="$phase1_dir/world-e2e.json"
phase2_log="$phase2_dir/server.log"
phase2_client="$phase2_dir/client.log"
phase2_console="$phase2_dir/console.fifo"
phase2_save="$phase2_dir/world.json"

echo "[$(now)] run dir: $run_dir"
echo "[$(now)] phase 1: fresh server + economy + waves 1-10 + counter 60"

# --- phase 1 server --------------------------------------------------------
mkfifo "$phase1_console"
exec 3<>"$phase1_console"
target/release/oxide \
    --port "$e2e_port" \
    --save-file "$phase1_save" \
    <&3 >"$phase1_log" 2>&1 &
phase1_pid=$!
cleanup() {
    exec 3>&-
    if [[ -n "${client_pid:-}" ]] && kill -0 "$client_pid" 2>/dev/null; then
        kill "$client_pid"
        wait "$client_pid" 2>/dev/null || true
    fi
    if [[ -n "${phase1_pid:-}" ]] && kill -0 "$phase1_pid" 2>/dev/null; then
        kill "$phase1_pid"
        wait "$phase1_pid" 2>/dev/null || true
    fi
    if [[ -n "${phase2_pid:-}" ]] && kill -0 "$phase2_pid" 2>/dev/null; then
        kill "$phase2_pid"
        wait "$phase2_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- phase 1 client: economy build -----------------------------------------
java -cp "$desktop_jar:$class_dir" E2ELong158 "$e2e_port" build \
    >"$phase1_client" 2>&1 &
client_pid=$!

ready=false
for _ in {1..150}; do
    if grep -q "E2E READY" "$phase1_client"; then
        ready=true
        break
    fi
    if ! kill -0 "$client_pid" 2>/dev/null; then
        echo "[$(now)] phase-1 client exited early. Client log:" >&2
        cat "$phase1_client" >&2
        exit 1
    fi
    sleep 0.2
done
if [[ "$ready" != true ]]; then
    echo "[$(now)] economy build did not complete. Client log:" >&2
    cat "$phase1_client" >&2
    exit 1
fi
echo "[$(now)] economy built: $(grep 'E2E READY' "$phase1_client")"

# --- phase 1 wave progression ----------------------------------------------
# The maze's survival roster escalates to 9000 HP scepters at wave 41 and the
# official wave spacing is 250 s; no starter base survives 60 waves no matter
# the dispatch rate. The harness therefore verifies the MECHANICS:
#   1. organic waves 1-10 at a playable 20 s cadence (client builds, turrets
#      kill trickling units, wave counter advances);
#   2. the documented `waves 60` console command re-anchors the shared wave
#      counter to 60 (the save must persist that counter);
#   3. after restart, `waves 63` proves the restored world keeps advancing.
prog() {
    local fd="$1"
    local i
    for ((i = 1; i <= 10; i++)); do
        printf 'time 20 s\n' >&"$fd"
        sleep 20
    done
    printf 'waves 60\n' >&"$fd"
}
prog "$phase1_console" &
accel_pid=$!

# Spawn a few weak enemy squads AT the official maze spawn points (288,280)
# and (190,290) so they must walk the maze like real wave units. Spawning next
# to the core (as an earlier draft did) legitimately destroyed the 6000 HP
# core in seconds (crawler DPS ~200 each), which is correct game behavior but
# not what this harness wants to demonstrate. The client observes combat RPCs
# (CreateBullet/UnitDeath) as the Duo kills the trickling units.
combat() {
    local fd="$1"
    sleep 8
    printf 'spawn dagger 4 288 280\n' >&"$fd"
    sleep 6
    printf 'spawn flare 3 190 290\n' >&"$fd"
    sleep 8
    printf 'spawn dagger 5 288 280\n' >&"$fd"
    sleep 8
    printf 'spawn flare 3 190 290\n' >&"$fd"
}
combat "$phase1_console" &
combat_pid=$!

phase1_ok=false
for _ in {1..300}; do
    if grep -q "E2E PHASE1_OK" "$phase1_client"; then
        phase1_ok=true
        break
    fi
    if grep -q "E2E PHASE1_FAIL" "$phase1_client"; then
        echo "[$(now)] phase-1 FAILED:" >&2
        grep "E2E" "$phase1_client" >&2
        exit 1
    fi
    if ! kill -0 "$client_pid" 2>/dev/null; then
        echo "[$(now)] phase-1 client exited:" >&2
        cat "$phase1_client" >&2
        exit 1
    fi
    sleep 1
done
kill "$accel_pid" 2>/dev/null || true
kill "$combat_pid" 2>/dev/null || true
wait "$accel_pid" 2>/dev/null || true
wait "$combat_pid" 2>/dev/null || true
if [[ "$phase1_ok" != true ]]; then
    echo "[$(now)] phase-1 timed out. Client log:" >&2
    cat "$phase1_client" >&2
    exit 1
fi
phase1_summary="$(grep "E2E PHASE1_OK" "$phase1_client")"
phase1_ready="$(grep "E2E READY" "$phase1_client")"
echo "[$(now)] phase 1 OK: $phase1_summary"

# --- phase 1 save + restart -------------------------------------------------
printf 'save e2e\n' >&3
sleep 2
if [[ ! -f "$phase1_slot" ]]; then
    echo "[$(now)] save slot was not created. Server log tail:" >&2
    tail -40 "$phase1_log" >&2
    exit 1
fi
sleep 1
echo "[$(now)] save slot: $phase1_slot"

# Extract wave/copper from the saved slot for continuity checks.
saved_wave="$(python3 -c "import json;print(json.load(open('$phase1_slot'))['wave'])")"
saved_copper="$(python3 -c "import json;print(json.load(open('$phase1_slot'))['core_items'][0])")"
saved_tiles="$(python3 -c "
import json
d=json.load(open('$phase1_slot'))
stats = d.get('game_stats', {}).get('placed_block_count', [])
counts = {block: n for block, n in stats}
blocks=[t['block'] for t in d.get('tiles', [])]
drill = counts.get(325, blocks.count(325))
conveyor = counts.get(257, blocks.count(257))
duo = counts.get(349, blocks.count(349))
game_over = d.get('game_over', False)
print('drill=%d conveyor=%d duo=%d game_over=%s' % (drill, conveyor, duo, game_over))")"
echo "[$(now)] saved: wave=$saved_wave copper=$saved_copper tiles=[$saved_tiles]"
if (( saved_wave < 60 )); then
    echo "[$(now)] saved wave below 60: $saved_wave" >&2
    exit 1
fi
if [[ "$saved_tiles" != *"game_over=False"* && "$saved_tiles" != *"game_over=false"* ]]; then
    echo "[$(now)] saved game_over is not false: $saved_tiles" >&2
    exit 1
fi
if [[ "$saved_tiles" != *"drill=2"* || "$saved_tiles" != *"conveyor=7"* || "$saved_tiles" != *"duo=1"* ]]; then
    echo "[$(now)] saved economy is incomplete: $saved_tiles" >&2
    exit 1
fi

# Kill the phase-1 server, restart on the saved slot with a fresh client.
kill "$phase1_pid"
wait "$phase1_pid" 2>/dev/null || true
phase1_pid=""
exec 3>&-
echo "[$(now)] phase 1 server stopped"

# --- phase 2: restart + new client + continuation ----------------------------
echo "[$(now)] phase 2: restart on slot save, fresh client"
# The restarted server must boot directly from the saved slot.
cp "$phase1_slot" "$phase2_save"
mkfifo "$phase2_console"
exec 4<>"$phase2_console"
target/release/oxide \
    --port "$e2e_port" \
    --save-file "$phase2_save" \
    <&4 >"$phase2_log" 2>&1 &
phase2_pid=$!

java -cp "$desktop_jar:$class_dir" E2ELong158 "$e2e_port" verify \
    "$saved_wave" "$saved_copper" >"$phase2_client" 2>&1 &
client_pid=$!

verify_ready=false
for _ in {1..150}; do
    if grep -q "E2E VERIFY_READY" "$phase2_client"; then
        verify_ready=true
        break
    fi
    if ! kill -0 "$client_pid" 2>/dev/null; then
        echo "[$(now)] phase-2 client exited early:" >&2
        cat "$phase2_client" >&2
        exit 1
    fi
    sleep 0.2
done
if [[ "$verify_ready" != true ]]; then
    echo "[$(now)] phase-2 client did not join. Client log:" >&2
    cat "$phase2_client" >&2
    exit 1
fi
echo "[$(now)] verify client joined: $(grep 'E2E VERIFY_READY' "$phase2_client")"

# Advance the restored world: re-anchor the counter 3 waves past the saved
# value (60 -> 63) and dispatch one weak squad for combat RPCs.
sleep 2
printf 'waves 63\n' >&4
sleep 1
printf 'spawn dagger 3 288 280\n' >&4

phase2_ok=false
for _ in {1..180}; do
    if grep -q "E2E VERIFY_OK" "$phase2_client"; then
        phase2_ok=true
        break
    fi
    if grep -q "E2E VERIFY_FAIL" "$phase2_client"; then
        echo "[$(now)] phase-2 FAILED:" >&2
        grep "E2E" "$phase2_client" >&2
        exit 1
    fi
    if ! kill -0 "$client_pid" 2>/dev/null; then
        echo "[$(now)] phase-2 client exited:" >&2
        cat "$phase2_client" >&2
        exit 1
    fi
    sleep 1
done
if [[ "$phase2_ok" != true ]]; then
    echo "[$(now)] phase-2 timed out. Client log:" >&2
    cat "$phase2_client" >&2
    exit 1
fi
echo "[$(now)] phase 2 OK: $(grep 'E2E VERIFY_OK' "$phase2_client")"
sleep 2

# Continuity checks on the post-restart save (auto-persisted).
post_wave="$(python3 -c "import json;print(json.load(open('$phase2_save'))['wave'])")"
post_copper="$(python3 -c "import json;print(json.load(open('$phase2_save'))['core_items'][0])")"
post_tiles="$(python3 -c "
import json
d=json.load(open('$phase2_save'))
stats = d.get('game_stats', {}).get('placed_block_count', [])
counts = {block: n for block, n in stats}
blocks=[t['block'] for t in d.get('tiles', [])]
drill = counts.get(325, blocks.count(325))
conveyor = counts.get(257, blocks.count(257))
duo = counts.get(349, blocks.count(349))
game_over = d.get('game_over', False)
print('drill=%d conveyor=%d duo=%d game_over=%s' % (drill, conveyor, duo, game_over))")"
echo "[$(now)] post-restart: wave=$post_wave copper=$post_copper tiles=[$post_tiles]"
if (( post_wave < saved_wave + 3 )); then
    echo "[$(now)] world did not advance after restart: saved=$saved_wave post=$post_wave" >&2
    exit 1
fi
if (( post_copper < saved_copper )); then
    echo "[$(now)] copper did not continue growing: saved=$saved_copper post=$post_copper" >&2
    exit 1
fi
if [[ "$post_tiles" != *"drill=2"* || "$post_tiles" != *"conveyor=7"* || "$post_tiles" != *"duo=1"* ]]; then
    echo "[$(now)] economy was not restored after restart: $post_tiles" >&2
    exit 1
fi
if [[ "$post_tiles" != *"game_over=false"* && "$post_tiles" != *"game_over=False"* ]]; then
    echo "[$(now)] game over after restart: $post_tiles" >&2
    exit 1
fi

# no disconnects observed: the server log must not report a mid-game close
if grep -q "closed after joining" "$phase2_log"; then
    echo "[$(now)] server reported a post-join disconnect during phase 2:" >&2
    grep "closed after joining" "$phase2_log" >&2
    exit 1
fi

kill "$client_pid" 2>/dev/null || true
wait "$client_pid" 2>/dev/null || true
client_pid=""
kill "$phase2_pid" 2>/dev/null || true
wait "$phase2_pid" 2>/dev/null || true
phase2_pid=""
exec 4>&-

echo "[$(now)] E2E PASSED"
echo "[$(now)] phase1: $phase1_ready | $phase1_summary"
echo "[$(now)] phase2: $(grep 'E2E VERIFY_OK' "$phase2_client")"
echo "[$(now)] continuity: saved wave=$saved_wave copper=$saved_copper -> post wave=$post_wave copper=$post_copper"
echo "[$(now)] artifacts: $run_dir"
