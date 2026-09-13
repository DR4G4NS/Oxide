#!/usr/bin/env bash
set -euo pipefail

# 159.7 production smoke: enterPayload or finite-resource survival chains,
# client unit counts, plastanium withdrawal, and optional survival waves.
# Reads streamed Rules without NetworkIO.readWorld.
# Skips when the JAR is absent.

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_1597_JAR:-/home/dr4g4ns/Escritorio/mindustry-159.7/jre/159.7.jar}"
smoke_port="${1:-6599}"
class_dir="$project_dir/target/protocol-1597-classes"

if [[ ! -f "$desktop_jar" ]]; then
    echo "skip: 159.7 JAR not found at $desktop_jar (set MINDUSTRY_1597_JAR)"
    exit 0
fi

cd "$project_dir"
cargo() { (exec -a cargo "$HOME/.cargo/bin/cargo" "$@"); }
case "${OXIDE_SMOKE_PROFILE:-release}" in
    release) cargo build --release; oxide_binary=target/release/oxide ;;
    debug) cargo build; oxide_binary=target/debug/oxide ;;
    *) echo "OXIDE_SMOKE_PROFILE must be release or debug" >&2; exit 2 ;;
esac
mkdir -p "$class_dir"
javac -d "$class_dir" -cp "$desktop_jar" tools/smoke/SmokeUnitPayload1597.java
run_dir="$(mktemp -d "$project_dir/target/smoke-unit-payload-1597.XXXXXX")"
server_log="$run_dir/server.log"
client_log="$run_dir/client.log"
save_file="$run_dir/world.json"
smoke_mode=sandbox
java_args=()
success_marker="SMOKE_OK unit-payload"
case "${OXIDE_SMOKE_SCENARIO:-enter-payload}" in
    enter-payload) cp "$project_dir/tools/smoke/smoke-unit-payload-world-1597.json" "$save_file" ;;
    survival-cycle|survival-air-cycle)
        cp "$project_dir/tools/smoke/smoke-survival-production-world-1597.json" "$save_file"
        smoke_mode=survival
        java_args+=(-Doxide.smoke.survivalCycle=true)
        if [[ "${OXIDE_SMOKE_SCENARIO}" == survival-air-cycle ]]; then
            java_args+=(-Doxide.smoke.airCycle=true)
        fi
        success_marker="SMOKE_OK survival-production"
        ;;
    *) echo "OXIDE_SMOKE_SCENARIO must be enter-payload, survival-cycle or survival-air-cycle" >&2; exit 2 ;;
esac

server_args=()
if [[ "${OXIDE_SMOKE_WAVES:-0}" == 1 ]]; then
    java_args+=(-Doxide.smoke.waves=true)
fi
if [[ -n "${OXIDE_SMOKE_MAP_FILE:-}" ]]; then
    server_args+=(--map-file "$OXIDE_SMOKE_MAP_FILE")
fi
python3 - "$save_file" <<'PY'
import json, os, sys
from pathlib import Path
path = Path(sys.argv[1])
world = json.loads(path.read_text())
if os.environ.get("OXIDE_SMOKE_WAVES") == "1":
    world["wave_time"] = 300
if os.environ.get("OXIDE_SMOKE_MAP_FILE"):
    world["map_name"] = Path(os.environ["OXIDE_SMOKE_MAP_FILE"]).stem
def tile(x, y, block, size=1, **fields):
    half = size // 2
    return dict(position=(x << 16) | y, block=block, rotation=0, team=1,
                config=[0], occupied=[(i << 16) | j for j in range(y-half, y+half+1)
                                      for i in range(x-half, x+half+1)], **fields)
if os.environ.get("OXIDE_SMOKE_SCENARIO", "").startswith("survival-"):
    # Feed the line from a multiblock drill. Prefilling belts would hide
    # acceptItem bugs: the client predicts that input independently.
    world["tiles"] += [tile(45, 88, 327, 3, stored_item=9, stored_amount=110)]
    world["tiles"] += [tile(45, y, 259) for y in range(90, 101)]
    for t in world["tiles"]:
        if t["block"] == 259: t["rotation"] = 1
    # Start the test player within transfer range even on maps whose core is
    # far from the fixture (pruebas01 has its core at 2,2).
    world["players"] = [dict(uuid="c21va2UtcHk=", player_id=1000001, unit_id=2000001,
                             x=360.0, y=800.0, health=150.0, shield=0.0,
                             status_effect=-1, status_duration=0.0, statuses=[],
                             dead=False, respawn_timer=0.0, team=1)]
if os.environ.get("OXIDE_SMOKE_SCENARIO") == "survival-air-cycle":
    factory = world["tiles"][0]
    factory.update(block=378, config=[1, 0, 0, 0, 1], inventory=[[9, 30], [1, 15]])
    world["tiles"] = [t for t in world["tiles"] if t["block"] != 0]
    second = tile(62, 100, 381, 5, inventory=[[9, 130], [6, 80], [2, 40]])
    world["tiles"].append(second)
    world["tiles"] += [tile(x, 95, 314, 3) for x in (61, 64, 67)]
    producers = [t["position"] for t in world["tiles"] if t["block"] == 314]
    consumers = [t["position"] for t in world["tiles"] if t["block"] in (378, 380, 381)]
    for t in world["tiles"]:
        if t["block"] == 314: t["power_links"] = consumers
        elif t["block"] in (378, 380, 381): t["power_links"] = producers
path.write_text(json.dumps(world))
PY

"$oxide_binary" \
    --port "$smoke_port" \
    --no-tui \
    --mode "$smoke_mode" \
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
java "${java_args[@]}" -cp "$desktop_jar:$class_dir" SmokeUnitPayload1597 "$smoke_port" | tee "$client_log"

if ! grep -q "finished loading the world" "$server_log"; then
    echo "Server did not confirm the joined state. Log: $server_log" >&2
    exit 1
fi
if ! grep -q "$success_marker" "$client_log"; then
    echo "Unit payload smoke did not assert success. Artifacts: $run_dir" >&2
    exit 1
fi
echo "159.7 unit payload smoke passed. Artifacts: $run_dir"
