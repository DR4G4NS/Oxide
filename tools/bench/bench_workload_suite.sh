#!/usr/bin/env bash
# Workload e2e suite: Java 159.7 vs Oxide at multiple player scales
# with simultaneous build/break traffic and enemy pressure.
#
# Each bot sends ClientSnapshots that cycle place copper-wall -> shoot -> break.
# Servers receive forced waves / spawned enemies during the steady window.
#
# Usage:
#   tools/bench/bench_workload_suite.sh
#   SCALES="10 100 500" DURATION_SEC=25 tools/bench/bench_workload_suite.sh
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$project_dir"

scales="${SCALES:-10 100 500}"
duration_sec="${DURATION_SEC:-25}"
warmup_sec="${WARMUP_SEC:-5}"
snapshot_hz="${SNAPSHOT_HZ:-10}"
build="${BUILD:-159}"
rust_port_base="${RUST_PORT:-6701}"
java_port_base="${JAVA_PORT:-6801}"
workload="${WORKLOAD:-build}"

desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/dr4g4ns/Escritorio/mindustry-159.7/jre/159.7.jar}"
java_server_jar="${MINDUSTRY_SERVER_JAR:-$project_dir/tools/bench/server-release-159.7.jar}"
class_dir="$project_dir/target/bench-load-classes"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out_dir="${OUT_DIR:-$project_dir/target/bench-workload-$stamp}"
report_md="${REPORT_MD:-$project_dir/benchmark.md}"
mkdir -p "$out_dir" "$class_dir" "$project_dir/tools/bench"

if [[ ! -f "$desktop_jar" ]]; then
  echo "error: desktop jar not found: $desktop_jar" >&2
  exit 1
fi
if [[ ! -f "$java_server_jar" ]]; then
  echo "error: java server jar not found: $java_server_jar" >&2
  exit 1
fi

echo "== workload bench suite =="
echo "scales=$scales duration=${duration_sec}s warmup=${warmup_sec}s workload=$workload"
echo "out_dir=$out_dir"
echo "report_md=$report_md"
echo "available_mem_kb=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)"

# Avoid Cursor rustup proxy breakage when present.
export PATH="/home/dr4g4ns/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:${PATH:-}"
unset RUSTC RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER 2>/dev/null || true

echo "== build rust release =="
cargo build --release

echo "== compile BenchLoadN =="
javac --release 17 -d "$class_dir" -cp "$desktop_jar" tools/bench/BenchLoadN.java

ramp_for() {
  local n="$1"
  if (( n <= 10 )); then echo 40
  elif (( n <= 100 )); then echo 25
  else echo 18
  fi
}

wait_port() {
  local port="$1"
  local seconds="${2:-45}"
  local i
  for ((i=0; i<seconds*10; i++)); do
    if ss -ltn "( sport = :$port )" 2>/dev/null | grep -q ":$port"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

sample_metrics() {
  local pid="$1"
  local out_csv="$2"
  local interval_sec="${3:-1}"
  echo "ts_epoch,elapsed_s,rss_kb,vsz_kb,cpu_pct,threads,fd_count" >"$out_csv"
  local start_epoch now elapsed first=1 prev_proc=0 prev_total=0
  start_epoch="$(date +%s)"
  while kill -0 "$pid" 2>/dev/null; do
    [[ -r "/proc/$pid/stat" && -r "/proc/$pid/status" ]] || break
    local rss_kb vsz_kb threads fd_count proc_jiffies total_jiffies cpu_pct
    rss_kb="$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")"
    vsz_kb="$(awk '/^VmSize:/ {print $2}' "/proc/$pid/status")"
    threads="$(awk '/^Threads:/ {print $2}' "/proc/$pid/status")"
    fd_count=0
    if [[ -d "/proc/$pid/fd" ]]; then
      fd_count="$(find "/proc/$pid/fd" 2>/dev/null | wc -l | tr -d ' ')"
    fi
    proc_jiffies="$(python3 - "$pid" <<'PY'
import pathlib, sys
text = pathlib.Path(f"/proc/{sys.argv[1]}/stat").read_text()
rest = text[text.rfind(")") + 2:].split()
print(int(rest[11]) + int(rest[12]))
PY
)"
    total_jiffies="$(awk '/^cpu / {s=0; for (i=2;i<=NF;i++) s+=$i; print s; exit}' /proc/stat)"
    cpu_pct="0.00"
    if [[ "$first" -eq 0 && "$total_jiffies" -gt "$prev_total" ]]; then
      cpu_pct="$(awk -v pj="$proc_jiffies" -v pp="$prev_proc" -v tj="$total_jiffies" -v tp="$prev_total" \
        'BEGIN { printf "%.2f", (100.0 * (pj-pp)) / (tj-tp) }')"
    fi
    first=0
    prev_proc="$proc_jiffies"
    prev_total="$total_jiffies"
    now="$(date +%s)"
    elapsed=$((now - start_epoch))
    echo "$now,$elapsed,${rss_kb:-0},${vsz_kb:-0},$cpu_pct,${threads:-0},$fd_count" >>"$out_csv"
    sleep "$interval_sec"
  done
}

summarize_metrics_csv() {
  local csv="$1"
  python3 - "$csv" <<'PY'
import csv, json, sys
rows = list(csv.DictReader(open(sys.argv[1])))
if not rows:
    print(json.dumps({"samples": 0}))
    raise SystemExit
def f(key):
    return [float(r[key]) for r in rows if r.get(key) not in (None, "")]
def stats(xs):
    if not xs: return None
    xs = sorted(xs)
    def pct(p):
        i = min(len(xs)-1, max(0, int(round(p*(len(xs)-1)))))
        return xs[i]
    return {"min": xs[0], "avg": sum(xs)/len(xs), "p50": pct(0.50), "p95": pct(0.95), "max": xs[-1]}
rss, cpu, thr, fds = f("rss_kb"), f("cpu_pct"), f("threads"), f("fd_count")
third = max(0, len(rss)//3)
print(json.dumps({
    "samples": len(rows),
    "rss_kb": stats(rss),
    "rss_mb": {k: v/1024.0 for k,v in (stats(rss) or {}).items()} if rss else None,
    "cpu_pct": stats(cpu),
    "threads": stats(thr),
    "fd_count": stats(fds),
    "steady_rss_kb": stats(rss[third:]) if rss else None,
    "steady_cpu_pct": stats(cpu[third:]) if cpu else None,
}))
PY
}

run_load_client() {
  local port="$1"
  local client_log="$2"
  local json_out="$3"
  local players="$4"
  local uuid_bytes="$5"
  local ramp_ms="$6"
  set +e
  java -Xms512m -Xmx6g --enable-native-access=ALL-UNNAMED \
    -cp "$desktop_jar:$class_dir" BenchLoadN \
    --host 127.0.0.1 \
    --port "$port" \
    --players "$players" \
    --duration-sec "$duration_sec" \
    --warmup-sec "$warmup_sec" \
    --ramp-ms "$ramp_ms" \
    --snapshot-hz "$snapshot_hz" \
    --build "$build" \
    --uuid-bytes "$uuid_bytes" \
    --workload "$workload" \
    --block-id 216 \
    >"$client_log" 2>"${client_log%.log}.err.log"
  local rc=$?
  set -e
  if grep -q '^BENCH_JSON ' "$client_log"; then
    grep '^BENCH_JSON ' "$client_log" | tail -1 | sed 's/^BENCH_JSON //' >"$json_out"
  else
    echo '{"error":"missing BENCH_JSON","exit_code":'"$rc"'}' >"$json_out"
  fi
}

wave_injector_rust() {
  local fifo_fd="$1"
  local players="$2"
  local seconds=$((warmup_sec + duration_sec))
  local elapsed=0
  # Seed enemies immediately, then keep pressure.
  echo "spawn dagger $(( players < 40 ? 20 : 40 )) 55 95" >&"$fifo_fd" || true
  echo "spawn crawler $(( players < 40 ? 10 : 20 )) 60 90" >&"$fifo_fd" || true
  echo "waves" >&"$fifo_fd" || true
  while (( elapsed < seconds )); do
    sleep 5
    elapsed=$((elapsed + 5))
    echo "waves" >&"$fifo_fd" || true
    echo "spawn dagger 15 52 98" >&"$fifo_fd" || true
    if (( elapsed % 10 == 0 )); then
      echo "spawn flare 8 70 110" >&"$fifo_fd" || true
    fi
  done
}

wave_injector_java() {
  local fifo_fd="$1"
  local seconds=$((warmup_sec + duration_sec))
  local elapsed=0
  echo "fillitems" >&"$fifo_fd" || true
  echo "runwave" >&"$fifo_fd" || true
  while (( elapsed < seconds )); do
    sleep 5
    elapsed=$((elapsed + 5))
    echo "runwave" >&"$fifo_fd" || true
  done
}

run_rust_case() {
  local players="$1"
  local port="$2"
  local case_dir="$out_dir/p${players}/rust"
  mkdir -p "$case_dir"
  local save_file="$case_dir/world.json"
  cp "$project_dir/tools/smoke/smoke-multiplayer-world-158.json" "$save_file"
  # Rich core inventory for build spam.
  python3 - "$save_file" <<'PY'
import json, sys
path = sys.argv[1]
doc = json.load(open(path))
doc["core_items"] = [50000] * 20
doc["wave"] = 1
doc["wave_time"] = 60.0
json.dump(doc, open(path, "w"))
PY
  cat >"$case_dir/admin-data.json" <<EOF
{"banned_ips":[],"banned_uuids":[],"dos_banned_ips":[],"subnet_bans":[],"whitelist":[],"whitelist_enabled":false,"admins":[],"player_limit":0,"server_name":"bench-rust","server_description":"bench","server_build":$build,"server_version_type":"official"}
EOF
  local server_log="$case_dir/server.log"
  local metrics_csv="$case_dir/metrics.csv"
  local client_log="$case_dir/client.log"
  local client_json="$case_dir/client.json"
  local fifo="$case_dir/server.fifo"
  rm -f "$fifo"
  mkfifo "$fifo"
  local ramp_ms
  ramp_ms="$(ramp_for "$players")"

  echo "== rust players=$players port=$port =="
  (
    cd "$case_dir"
    RUST_LOG="${RUST_LOG:-info,develop=info}" \
    "$project_dir/target/release/oxide" \
      --no-tui \
      --port "$port" \
      --max-players "$players" \
      --tps 60 \
      --build "$build" \
      --version-type official \
      --name "bench-rust" \
      --mode survival \
      --map-name maze \
      --save-file "$save_file" \
      --develop \
      --develop-interval-ms 2000 \
      <"$fifo" >"$server_log" 2>&1 &
    echo $! >"$case_dir/server.pid"
    wait || true
  ) &
  local wrapper_pid=$!
  exec 9>"$fifo"

  local server_pid=""
  local _
  for _ in $(seq 1 100); do
    if [[ -f "$case_dir/server.pid" ]]; then
      server_pid="$(cat "$case_dir/server.pid")"
      kill -0 "$server_pid" 2>/dev/null && break
    fi
    sleep 0.1
  done
  [[ -n "$server_pid" ]] || { echo "rust pid missing" >&2; return 1; }

  cleanup_rust() {
    echo "exit" >&9 || true
    sleep 0.3
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
    exec 9>&- || true
    wait "$wrapper_pid" 2>/dev/null || true
    rm -f "$fifo"
  }
  trap cleanup_rust RETURN

  if ! wait_port "$port" 30; then
    echo "rust failed to bind :$port" >&2
    tail -40 "$server_log" >&2 || true
    return 1
  fi
  sleep 0.5

  sample_metrics "$server_pid" "$metrics_csv" 1 &
  local sampler_pid=$!
  wave_injector_rust 9 "$players" &
  local wave_pid=$!

  run_load_client "$port" "$client_log" "$client_json" "$players" 16 "$ramp_ms"

  kill "$wave_pid" 2>/dev/null || true
  wait "$wave_pid" 2>/dev/null || true
  kill "$sampler_pid" 2>/dev/null || true
  wait "$sampler_pid" 2>/dev/null || true
  cleanup_rust
  trap - RETURN

  summarize_metrics_csv "$metrics_csv" >"$case_dir/metrics_summary.json"
  python3 - "$server_log" "$case_dir/server_derived.json" <<'PY'
import json, re, sys
text = open(sys.argv[1], errors="replace").read()
ups = re.findall(r"ups=([0-9.]+)", text)
tick_avg = re.findall(r"tick_avg_us\(win\)=([0-9]+)", text)
tick_max = re.findall(r"tick_max_us=([0-9]+)", text)
players = re.findall(r"players=([0-9]+)", text)
enemies = re.findall(r"enemies=([0-9]+)", text)
rss = re.findall(r"rss=([0-9]+)mb", text)
json.dump({
    "develop_ups_last": float(ups[-1]) if ups else None,
    "develop_ups_min": min(map(float, ups)) if ups else None,
    "develop_tick_avg_us_last": int(tick_avg[-1]) if tick_avg else None,
    "develop_tick_max_us_last": int(tick_max[-1]) if tick_max else None,
    "develop_players_last": int(players[-1]) if players else None,
    "develop_enemies_last": int(enemies[-1]) if enemies else None,
    "develop_enemies_max": max(map(int, enemies)) if enemies else None,
    "develop_rss_mb_last": int(rss[-1]) if rss else None,
    "joined_log_lines": text.count("finished loading the world"),
    "spawn_commands": text.count("spawn"),
}, open(sys.argv[2], "w"), indent=2)
print(open(sys.argv[2]).read())
PY
}

run_java_case() {
  local players="$1"
  local port="$2"
  local case_dir="$out_dir/p${players}/java"
  mkdir -p "$case_dir/workdir"
  rm -rf "$case_dir/workdir/config" 2>/dev/null || true
  local server_log="$case_dir/server.log"
  local metrics_csv="$case_dir/metrics.csv"
  local client_log="$case_dir/client.log"
  local client_json="$case_dir/client.json"
  local fifo="$case_dir/server.fifo"
  rm -f "$fifo"
  mkfifo "$fifo"
  local ramp_ms
  ramp_ms="$(ramp_for "$players")"

  echo "== java players=$players port=$port =="
  (
    cd "$case_dir/workdir"
    java -Xms512m -Xmx3g --enable-native-access=ALL-UNNAMED \
      -jar "$java_server_jar" <"$fifo" >"$server_log" 2>&1 &
    echo $! >"$case_dir/server.pid"
    wait || true
  ) &
  local wrapper_pid=$!
  exec 9>"$fifo"

  local server_pid=""
  local _
  for _ in $(seq 1 150); do
    if [[ -f "$case_dir/server.pid" ]]; then
      server_pid="$(cat "$case_dir/server.pid")"
      kill -0 "$server_pid" 2>/dev/null && break
    fi
    sleep 0.1
  done
  [[ -n "$server_pid" ]] || { echo "java pid missing" >&2; return 1; }

  cleanup_java() {
    echo "exit" >&9 || true
    sleep 0.4
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
    exec 9>&- || true
    wait "$wrapper_pid" 2>/dev/null || true
    rm -f "$fifo"
  }
  trap cleanup_java RETURN

  for _ in $(seq 1 150); do
    grep -q "Server loaded" "$server_log" 2>/dev/null && break
    sleep 0.1
  done
  {
    echo "config port $port"
    echo "config uuidChangeLimit 0"
    echo "playerlimit $players"
    echo "unban 127.0.0.1"
    echo "host maze survival"
  } >&9

  if ! wait_port "$port" 60; then
    echo "java failed to bind :$port" >&2
    tail -60 "$server_log" >&2 || true
    return 1
  fi
  sleep 1
  echo "fillitems" >&9
  echo "status" >&9

  sample_metrics "$server_pid" "$metrics_csv" 1 &
  local sampler_pid=$!
  wave_injector_java 9 &
  local wave_pid=$!

  run_load_client "$port" "$client_log" "$client_json" "$players" 8 "$ramp_ms"

  echo "status" >&9
  sleep 1
  kill "$wave_pid" 2>/dev/null || true
  wait "$wave_pid" 2>/dev/null || true
  kill "$sampler_pid" 2>/dev/null || true
  wait "$sampler_pid" 2>/dev/null || true
  cleanup_java
  trap - RETURN

  summarize_metrics_csv "$metrics_csv" >"$case_dir/metrics_summary.json"
  python3 - "$server_log" "$case_dir/server_derived.json" <<'PY'
import json, re, sys
text = open(sys.argv[1], errors="replace").read()
plain = re.sub(r"\x1b\[[0-9;]*m", "", text)
fps = re.findall(r"(\d+)\s+FPS,\s+(\d+)\s+MB used", plain)
players = re.findall(r"Players:\s*(\d+)", plain)
units = re.findall(r"(\d+)\s+units\s*/\s*(\d+)\s+enemies", plain)
json.dump({
    "status_fps_mb_samples": [{"fps": int(a), "heap_mb": int(b)} for a,b in fps],
    "status_fps_last": int(fps[-1][0]) if fps else None,
    "status_fps_min": min(int(a) for a,_ in fps) if fps else None,
    "status_heap_mb_last": int(fps[-1][1]) if fps else None,
    "status_players_last": int(players[-1]) if players else None,
    "status_units_last": int(units[-1][0]) if units else None,
    "status_enemies_last": int(units[-1][1]) if units else None,
    "map_loaded": ("Map loaded." in plain),
    "runwave_count": plain.count("Wave") + plain.lower().count("runwave"),
}, open(sys.argv[2], "w"), indent=2)
print(open(sys.argv[2]).read())
PY
}

scale_idx=0
for players in $scales; do
  rust_port=$((rust_port_base + scale_idx))
  java_port=$((java_port_base + scale_idx))
  run_rust_case "$players" "$rust_port"
  run_java_case "$players" "$java_port"
  scale_idx=$((scale_idx + 1))
  # Give the machine a breath between scales.
  sleep 2
done

python3 - "$out_dir" "$report_md" "$scales" "$duration_sec" "$warmup_sec" "$workload" "$snapshot_hz" <<'PY'
import json, pathlib, sys, datetime
out_dir = pathlib.Path(sys.argv[1])
report_md = pathlib.Path(sys.argv[2])
scales = [int(x) for x in sys.argv[3].split()]
duration = int(sys.argv[4])
warmup = int(sys.argv[5])
workload = sys.argv[6]
snapshot_hz = int(sys.argv[7])

def load(path):
    p = out_dir / path
    return json.loads(p.read_text()) if p.exists() else None

def fmt(v, nd=1):
    if v is None: return "—"
    if isinstance(v, float): return f"{v:.{nd}f}"
    return str(v)

def side_block(players, side):
    base = f"p{players}/{side}"
    c = load(f"{base}/client.json") or {}
    m = load(f"{base}/metrics_summary.json") or {}
    d = load(f"{base}/server_derived.json") or {}
    rss = m.get("rss_mb") or {}
    cpu = m.get("cpu_pct") or {}
    steady_cpu = m.get("steady_cpu_pct") or {}
    steady_rss = m.get("steady_rss_kb") or {}
    thr = m.get("threads") or {}
    lines = [f"#### {side.upper()} — {players} players"]
    if c.get("error"):
        lines.append(f"- error: `{c}`")
        return "\n".join(lines)
    lines.append(
        f"- joined: **{c.get('joined')}/{c.get('players_requested')}** "
        f"(confirmed={c.get('confirmed')}, kicked={c.get('kicked')}, "
        f"still_connected={c.get('still_connected')})"
    )
    lines.append(
        f"- join_ms: avg={fmt(c.get('join_ms_avg'))} p50={c.get('join_ms_p50')} "
        f"p95={c.get('join_ms_p95')} p99={c.get('join_ms_p99')} max={c.get('join_ms_max')}"
    )
    lines.append(
        f"- ping_rtt_ms: avg={fmt(c.get('ping_rtt_ms_avg'))} max={c.get('ping_rtt_ms_max')} "
        f"samples={c.get('ping_samples')}"
    )
    lines.append(
        f"- workload tx: build_snap={c.get('build_snapshots_tx')} "
        f"break_snap={c.get('break_snapshots_tx')} shoot_snap={c.get('shoot_snapshots_tx')}"
    )
    lines.append(
        f"- workload rx: begin_place={c.get('begin_place_rx')} "
        f"construct_finish={c.get('construct_finish_rx')} "
        f"begin_break={c.get('begin_break_rx')} "
        f"deconstruct_finish={c.get('deconstruct_finish_rx')} "
        f"unit_death={c.get('unit_death_rx')} bullets={c.get('create_bullet_rx')}"
    )
    lines.append(
        f"- traffic: snapshots_tx={c.get('snapshots_tx')} "
        f"entity_rx={c.get('entity_snapshots_rx')} state_rx={c.get('state_snapshots_rx')} "
        f"packets_rx={c.get('packets_rx')}"
    )
    if rss:
        lines.append(
            f"- rss_mb: avg={fmt(rss.get('avg'))} p50={fmt(rss.get('p50'))} "
            f"p95={fmt(rss.get('p95'))} max={fmt(rss.get('max'))}"
        )
    if steady_rss:
        lines.append(
            f"- steady_rss_mb: avg={fmt(steady_rss.get('avg',0)/1024)} "
            f"max={fmt(steady_rss.get('max',0)/1024)}"
        )
    if cpu:
        lines.append(
            f"- cpu_pct: avg={fmt(cpu.get('avg'))} p50={fmt(cpu.get('p50'))} "
            f"p95={fmt(cpu.get('p95'))} max={fmt(cpu.get('max'))}"
        )
    if steady_cpu:
        lines.append(
            f"- steady_cpu_pct: avg={fmt(steady_cpu.get('avg'))} max={fmt(steady_cpu.get('max'))}"
        )
    if thr:
        lines.append(f"- threads: avg={fmt(thr.get('avg'),0)} max={fmt(thr.get('max'),0)}")
    if side == "rust":
        lines.append(
            f"- develop: ups_last={d.get('develop_ups_last')} ups_min={d.get('develop_ups_min')} "
            f"tick_avg_us={d.get('develop_tick_avg_us_last')} "
            f"tick_max_us={d.get('develop_tick_max_us_last')} "
            f"players={d.get('develop_players_last')} "
            f"enemies_last={d.get('develop_enemies_last')} "
            f"enemies_max={d.get('develop_enemies_max')} "
            f"rss_mb={d.get('develop_rss_mb_last')}"
        )
    else:
        lines.append(
            f"- status: fps_last={d.get('status_fps_last')} fps_min={d.get('status_fps_min')} "
            f"heap_mb={d.get('status_heap_mb_last')} players={d.get('status_players_last')} "
            f"units={d.get('status_units_last')} enemies={d.get('status_enemies_last')}"
        )
    return "\n".join(lines)

comparison = {
    "generated_utc": datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ"),
    "workload": workload,
    "duration_sec": duration,
    "warmup_sec": warmup,
    "snapshot_hz": snapshot_hz,
    "scales": {},
}
for n in scales:
    comparison["scales"][str(n)] = {
        "rust": {
            "client": load(f"p{n}/rust/client.json"),
            "metrics": load(f"p{n}/rust/metrics_summary.json"),
            "server_derived": load(f"p{n}/rust/server_derived.json"),
        },
        "java": {
            "client": load(f"p{n}/java/client.json"),
            "metrics": load(f"p{n}/java/metrics_summary.json"),
            "server_derived": load(f"p{n}/java/server_derived.json"),
        },
    }
(out_dir / "comparison.json").write_text(json.dumps(comparison, indent=2) + "\n")

# Summary table rows
rows = []
for n in scales:
    for side in ("rust", "java"):
        c = (comparison["scales"][str(n)][side]["client"] or {})
        m = (comparison["scales"][str(n)][side]["metrics"] or {})
        d = (comparison["scales"][str(n)][side]["server_derived"] or {})
        rss = (m.get("rss_mb") or {})
        cpu = (m.get("steady_cpu_pct") or m.get("cpu_pct") or {})
        if side == "rust":
            sim = d.get("develop_ups_last")
            enemies = d.get("develop_enemies_max")
        else:
            sim = d.get("status_fps_last")
            enemies = d.get("status_enemies_last")
        rows.append(
            f"| {n} | {side} | {c.get('joined')}/{c.get('players_requested')} | "
            f"{fmt(c.get('join_ms_avg'))} / {c.get('join_ms_p95')} | "
            f"{fmt(rss.get('avg'))} / {fmt(rss.get('max'))} | "
            f"{fmt(cpu.get('avg'))} / {fmt(cpu.get('max'))} | "
            f"{sim} | {enemies} | "
            f"{c.get('construct_finish_rx')}/{c.get('deconstruct_finish_rx')} | "
            f"{c.get('unit_death_rx')} |"
        )

md = []
md.append("# Benchmark: Java 159.7 vs Oxide")
md.append("")
md.append(f"- generated_utc: `{comparison['generated_utc']}`")
md.append(f"- workload: `{workload}` (ClientSnapshot place copper-wall → shoot → break)")
md.append(f"- enemies: Rust `waves` + `spawn dagger/crawler/flare`; Java `runwave` + `fillitems`")
md.append(f"- map/mode: maze survival @ 60 TPS")
md.append(f"- duration_sec: {duration} (warmup_sec: {warmup}, snapshot_hz: {snapshot_hz})")
md.append(f"- scales: {', '.join(map(str, scales))}")
md.append(f"- artifacts: `{out_dir}`")
md.append("")
md.append("## Summary table")
md.append("")
md.append("| players | server | joined | join_ms avg/p95 | rss_mb avg/max | steady_cpu% avg/max | sim (UPS/FPS) | enemies | construct/deconstruct rx | unit_deaths |")
md.append("|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|")
md.extend(rows)
md.append("")
md.append("## Interpretation notes")
md.append("")
md.append("- Low Java CPU% with collapsed FPS usually means the simulation loop is starved, not that Java is cheaper.")
md.append("- Rust CPU% under load reflects real tick + replication work while trying to hold UPS near 60.")
md.append("- UUID wire quirk: harness uses `--uuid-bytes 16` for Rust and `8` for Java (official ConnectPacket read skips CRC).")
md.append("- Build/break acknowledgements (`construct_finish_rx` / `deconstruct_finish_rx`) and `unit_death_rx` validate that the workload was not idle.")
md.append("")
for n in scales:
    md.append(f"## Scale {n}")
    md.append("")
    md.append(side_block(n, "rust"))
    md.append("")
    md.append(side_block(n, "java"))
    md.append("")

report_md.write_text("\n".join(md) + "\n")
print(report_md.read_text())
print(f"Wrote {report_md}")
print(f"Wrote {out_dir / 'comparison.json'}")
PY

echo "DONE out_dir=$out_dir report=$report_md"
