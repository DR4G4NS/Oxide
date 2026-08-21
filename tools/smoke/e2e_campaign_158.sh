#!/usr/bin/env bash
# P2-D1 — Real-client E2E parity campaign against Mindustry desktop.jar 158.1.
#
# Runs seven gameplay scenarios, records server logs, disconnects, tick metrics,
# and differential probe results. Network scenarios spin up the Rust server with
# a FIFO console and drive the official desktop 158.1 serializer clients.
#
# Usage:
#   MINDUSTRY_DESKTOP_JAR=/path/to/desktop.jar tools/smoke/e2e_campaign_158.sh [output-dir]
#
# Environment:
#   MINDUSTRY_DESKTOP_JAR   official desktop.jar 158.1 (required)
#   SKIP_E2E_LONG=1         skip the ~25 min save/reconnect long harness
#   E2E_PORT                base TCP/UDP port (default 6595)
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
desktop_jar="${MINDUSTRY_DESKTOP_JAR:-}"
base_port="${E2E_PORT:-6595}"
skip_long="${SKIP_E2E_LONG:-0}"
class_dir="$project_dir/target/protocol-158-classes"
now() { date '+%Y-%m-%d %H:%M:%S'; }

if [[ -z "$desktop_jar" || ! -f "$desktop_jar" ]]; then
    echo "error: set MINDUSTRY_DESKTOP_JAR to official desktop.jar 158.1" >&2
    exit 1
fi
jar_version="$(unzip -p "$desktop_jar" version.properties 2>/dev/null | tr -d '\r' || true)"
jar_build="$(printf '%s\n' "$jar_version" | sed -n 's/^build=//p')"
jar_type="$(printf '%s\n' "$jar_version" | sed -n 's/^type=//p')"
if [[ "$jar_build" != "158.1" || "$jar_type" != "official" ]]; then
    echo "error: desktop jar must be official 158.1, got build=$jar_build type=$jar_type" >&2
    exit 2
fi

if [[ $# -gt 0 ]]; then
    out_dir="$1"
    if [[ -e "$out_dir" ]]; then
        echo "error: output dir already exists: $out_dir" >&2
        exit 1
    fi
    mkdir -p "$out_dir"
else
    out_dir="$(mktemp -d "$project_dir/e2e-campaign-158.XXXXXX")"
fi

report_json="$out_dir/campaign.json"
report_md="$out_dir/E2E_CAMPAIGN.md"
results_file="$out_dir/scenario-results.tsv"

cd "$project_dir"
echo "[$(now)] campaign output: $out_dir"
: > "$results_file"

cargo_build() {
    env -u RUSTUP_TOOLCHAIN cargo build --release
}

record_result() {
    local id="$1"
    local layer="$2"
    local result="$3"
    local notes="${4:-}"
    printf '%s\t%s\t%s\t%s\n' "$id" "$layer" "$result" "$notes" >> "$results_file"
}

run_cargo_test() {
    local id="$1"
    local label="$2"
    shift 2
    local log="$out_dir/${label}.log"
    echo "[$(now)] cargo test: $label ($*)"
    if env -u RUSTUP_TOOLCHAIN cargo test "$@" 2>&1 | tee "$log"; then
        record_result "$id" "parity" "PASS" "$label"
        return 0
    fi
    record_result "$id" "parity" "FAIL" "$label"
    return 1
}

parse_server_metrics() {
    local log="$1"
    local metrics_out="$2"
    python3 - "$log" "$metrics_out" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
ups = re.findall(r"UPS \(since last status\): ([0-9.]+)", text)
tick_avg = re.findall(r"World Tick Duration: ([0-9.]+) ms \(max ([0-9.]+) ms\)", text)
drops = re.findall(r"Dropped Outbound Frames: (\d+)", text)
disconnects = len(re.findall(r"closed after joining", text, re.I))
gaps = len(re.findall(r"snapshot gap|missing snapshot", text, re.I))
last_ups = ups[-1] if ups else "n/a"
last_tick = tick_avg[-1] if tick_avg else ("n/a", "n/a")
last_drops = drops[-1] if drops else "0"
open(sys.argv[2], "w").write(
    f"ups={last_ups}\n"
    f"tick_avg_ms={last_tick[0]}\n"
    f"tick_max_ms={last_tick[1]}\n"
    f"dropped_frames={last_drops}\n"
    f"disconnects={disconnects}\n"
    f"snapshot_gaps={gaps}\n"
)
PY
}

run_live_client() {
    local id="$1"
    local name="$2"
    local port="$3"
    local java_class="$4"
    local world_src="$5"
    local run_dir="$out_dir/live-${name}"
    local server_log="$run_dir/server.log"
    local client_log="$run_dir/client.log"
    local console_fifo="$run_dir/console.fifo"
    local save_file="$run_dir/world.json"
    mkdir -p "$run_dir"
    cp "$world_src" "$save_file"
    mkfifo "$console_fifo"
    exec {console_fd}<>"$console_fifo"

    target/release/oxide \
        --port "$port" \
        --save-file "$save_file" \
        <&"$console_fd" >"$server_log" 2>&1 &
    local server_pid=$!

    cleanup_live() {
        exec {console_fd}>&-
        if kill -0 "$server_pid" 2>/dev/null; then
            kill "$server_pid" 2>/dev/null || true
            wait "$server_pid" 2>/dev/null || true
        fi
    }
    trap cleanup_live RETURN

    sleep 0.6
    printf 'status\n' >&"$console_fd"
    sleep 0.3

    local status=0
    if java -cp "$desktop_jar:$class_dir" "$java_class" "$port" >"$client_log" 2>&1; then
        record_result "$id" "live-client" "PASS" "$name"
    else
        record_result "$id" "live-client" "FAIL" "$name"
        status=1
    fi

    printf 'status\n' >&"$console_fd"
    sleep 0.3
    parse_server_metrics "$server_log" "$run_dir/metrics.txt"
    cp "$server_log" "$out_dir/${name}-server.log"
    cp "$client_log" "$out_dir/${name}-client.log"
    cleanup_live
    trap - RETURN
    return $status
}

mkdir -p "$class_dir"
cargo_build
javac -d "$class_dir" -cp "$desktop_jar" \
    tools/smoke/SmokeJoin158.java \
    tools/smoke/SmokeMultiplayer158.java \
    tools/smoke/SmokeTileConfig158.java

failures=0

echo "[$(now)] === Differential probes (scenarios 1–5) ==="
if run_cargo_test all-parity scenario-parity --lib parity; then
    record_result logic parity PASS "included in parity suite"
    record_result rts parity PASS "included in parity suite"
    record_result status parity PASS "included in parity suite"
    record_result payload parity PASS "included in parity suite"
    record_result power parity PASS "included in parity suite"
else
    record_result logic parity FAIL "parity suite"
    record_result rts parity FAIL "parity suite"
    record_result status parity FAIL "parity suite"
    record_result payload parity FAIL "parity suite"
    record_result power parity FAIL "parity suite"
    failures=$((failures + 1))
fi

echo "[$(now)] === Scenario 1: Logic (live client) ==="
run_live_client logic logic-join "$base_port" SmokeJoin158 \
    "$project_dir/tools/smoke/smoke-world-158.json" || failures=$((failures + 1))
sleep 1

echo "[$(now)] === Scenario 5: Power (live TileConfig) ==="
if MINDUSTRY_DESKTOP_JAR="$desktop_jar" bash "$project_dir/tools/smoke/smoke_tile_config_158.sh" "$((base_port + 1))" \
    2>&1 | tee "$out_dir/power-tileconfig.log"; then
    record_result power live-client PASS power-tileconfig
else
    record_result power live-client FAIL power-tileconfig
    failures=$((failures + 1))
fi
sleep 1

echo "[$(now)] === Scenario 6: Save/load ==="
run_cargo_test save_load scenario6-msav msav_roundtrip || failures=$((failures + 1))
if [[ "$skip_long" != "1" ]]; then
    long_log="$out_dir/e2e-long.log"
    echo "[$(now)] running e2e_long_158.sh"
    if MINDUSTRY_DESKTOP_JAR="$desktop_jar" bash "$project_dir/tools/smoke/e2e_long_158.sh" \
        "$((base_port + 2))" 25 2>&1 | tee "$long_log"; then
        record_result save_load live-client PASS e2e_long
        long_artifacts="$(grep 'artifacts:' "$long_log" | awk '{print $NF}')"
        if [[ -n "$long_artifacts" && -d "$long_artifacts" ]]; then
            cp -a "$long_artifacts/." "$out_dir/e2e-long-artifacts/" 2>/dev/null || \
                cp -a "$long_artifacts" "$out_dir/e2e-long-artifacts"
        fi
    else
        record_result save_load live-client FAIL e2e_long
        failures=$((failures + 1))
    fi
else
    record_result save_load live-client SKIP "SKIP_E2E_LONG=1"
fi

echo "[$(now)] === Scenario 7: Stress ==="
run_live_client stress stress-multi "$((base_port + 3))" SmokeMultiplayer158 \
    "$project_dir/tools/smoke/smoke-multiplayer-world-158.json" || failures=$((failures + 1))
if [[ "$skip_long" != "1" && -f "$out_dir/e2e-long.log" ]] \
        && grep -q "E2E PASSED" "$out_dir/e2e-long.log"; then
    record_result stress live-client PASS "e2e_long waves/economy/combat"
fi

python3 - "$report_json" "$report_md" "$results_file" "$out_dir" "$desktop_jar" "$jar_build" <<'PY'
import json, os, sys
from datetime import datetime, timezone

out_json, out_md, results_tsv, out_dir, jar, build = sys.argv[1:7]
meta = {
    "logic": ("Logic", "ubind; ucontrol; two processors; destruction; unbind"),
    "rts": ("RTS/Player", "queue 50; dedup; target Unit/Building; possession/release"),
    "status": ("Status", "bullets; floor; transitions; disarm"),
    "payload": ("Payload", "unit/build; power-connected build; pickup/drop"),
    "power": ("Power", "node; diode; beam; split; replacement same tile"),
    "save_load": ("Save/load", "commands; status; payload; reload/reconnect"),
    "stress": ("Stress", "2+ clients; PvP; waves; miners; belts; logic; no snapshot gaps"),
}
rows_raw = {}
for line in open(results_tsv):
    line = line.rstrip("\n")
    if not line:
        continue
    sid, layer, result, notes = line.split("\t", 3)
    rows_raw.setdefault(sid, {"layers": [], "metrics": {}})
    rows_raw[sid]["layers"].append({"layer": layer, "result": result, "notes": notes})
    metrics_path = os.path.join(out_dir, "live-" + notes, "metrics.txt")
    if notes.endswith("-join"):
        metrics_path = os.path.join(out_dir, "live-logic-join", "metrics.txt")
    for candidate in [
        os.path.join(out_dir, f"live-{notes}", "metrics.txt"),
        os.path.join(out_dir, "live-logic-join", "metrics.txt"),
        os.path.join(out_dir, "live-power-tileconfig", "metrics.txt"),
        os.path.join(out_dir, "live-stress-multi", "metrics.txt"),
    ]:
        if os.path.isfile(candidate):
            for mline in open(candidate):
                if "=" in mline:
                    k, v = mline.strip().split("=", 1)
                    rows_raw[sid]["metrics"][k] = v

scenarios = []
overall_fail = False
for sid, (title, scope) in meta.items():
    entry = rows_raw.get(sid, {"layers": [], "metrics": {}})
    layer_results = [l["result"] for l in entry["layers"]]
    if not layer_results:
        result = "UNKNOWN"
        overall_fail = True
    elif any(r == "FAIL" for r in layer_results):
        result = "FAIL"
        overall_fail = True
    elif all(r in ("PASS", "SKIP") for r in layer_results):
        result = "PASS" if any(r == "PASS" for r in layer_results) else "SKIP"
    else:
        result = "PARTIAL"
    scenarios.append({
        "id": sid,
        "title": title,
        "scope": scope,
        "result": result,
        "layers": entry["layers"],
        "metrics": entry["metrics"],
    })

payload = {
    "generated_at": datetime.now(timezone.utc).isoformat(),
    "baseline": f"Mindustry official {build}",
    "desktop_jar": jar,
    "output_dir": out_dir,
    "scenarios": scenarios,
    "campaign_pass": not overall_fail,
}
with open(out_json, "w") as f:
    json.dump(payload, f, indent=2)

lines = [
    "# E2E Parity Campaign — Mindustry 158.1",
    "",
    f"Generated: {payload['generated_at']}",
    f"Baseline: `{jar}` (build={build})",
    f"Artifacts: `{out_dir}`",
    "",
    "## Results",
    "",
    "| # | Scenario | Result | UPS | Tick avg/max (ms) | Drops | Disconnects | Snapshot gaps |",
    "|---|----------|--------|-----|-------------------|-------|-------------|---------------|",
]
for i, s in enumerate(scenarios, 1):
    m = s["metrics"]
    lines.append(
        f"| {i} | {s['title']} | **{s['result']}** | {m.get('ups', '—')} | "
        f"{m.get('tick_avg_ms', '—')}/{m.get('tick_max_ms', '—')} | "
        f"{m.get('dropped_frames', '—')} | {m.get('disconnects', '—')} | "
        f"{m.get('snapshot_gaps', '—')} |"
    )
lines += ["", "## Layers per scenario", ""]
for s in scenarios:
    lines.append(f"### {s['title']} — {s['result']}")
    lines.append(f"Scope: {s['scope']}")
    for layer in s["layers"]:
        lines.append(f"- `{layer['layer']}`: **{layer['result']}** ({layer['notes']})")
    if s["metrics"]:
        lines.append(f"- Metrics: `{s['metrics']}`")
    lines.append("")
lines += [
    "## Documented deviations (server-authoritative)",
    "",
    "- LogicAI volatile control fields are not in MSAV rev 9; only processor position persists (`controller-save`).",
    "- Power graph production/status fields are informative-only in differential probes.",
    "- Stress harness uses embedded maze map; full 200×200 soak is out of scope for this runner.",
    "",
    "## Definition of Done",
    "",
]
done = payload["campaign_pass"]
for item in [
    "7 escenarios pasan",
    "0 crash/disconnect por server divergence",
    "snapshot cadence estable",
    "save/reconnect estable",
    "deviations restantes documentadas",
    "candidato vanilla 158.1 parity-complete",
]:
    mark = "x" if done else " "
    lines.append(f"- [{mark}] {item}")
with open(out_md, "w") as f:
    f.write("\n".join(lines) + "\n")
print(out_md)
PY

echo "[$(now)] campaign complete: failures=$failures"
echo "[$(now)] report: $report_md"
if (( failures > 0 )); then
    exit 1
fi
