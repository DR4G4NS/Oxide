#!/usr/bin/env bash
# E2E performance compare: official Mindustry Java headless vs Oxide.
#
# Metrics collected for each server under the same BenchLoadN client workload:
#   - process RSS / VSZ / CPU% (1 Hz sampling via /proc)
#   - client join latency (avg/p50/p95/p99), confirmed/joined/kicked
#   - ping RTT, snapshot TX/RX counters
#   - Rust --develop UPS / tick_us when available
#   - Java `status` FPS / heap when available
#
# UUID note: official ConnectPacket.write emits uuid_bytes+CRC32, but Java's
# ConnectPacket.read consumes a fixed 16-byte blob and skips the CRC. Real
# clients use 8-byte IDs so (8+CRC) accidentally aligns. Rust implements the
# write layout correctly (16 uuid + CRC). The harness therefore uses
# --uuid-bytes 16 against Rust and --uuid-bytes 8 against Java.
#
# Usage:
#   tools/bench/bench_e2e_compare.sh
#   PLAYERS=500 DURATION_SEC=30 tools/bench/bench_e2e_compare.sh
#   PLAYERS=100 SKIP_JAVA=1 tools/bench/bench_e2e_compare.sh
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$project_dir"

players="${PLAYERS:-500}"
duration_sec="${DURATION_SEC:-30}"
warmup_sec="${WARMUP_SEC:-5}"
ramp_ms="${RAMP_MS:-25}"
snapshot_hz="${SNAPSHOT_HZ:-10}"
build="${BUILD:-159}"
rust_port="${RUST_PORT:-6591}"
java_port="${JAVA_PORT:-6592}"
skip_rust="${SKIP_RUST:-0}"
skip_java="${SKIP_JAVA:-0}"

desktop_jar="${MINDUSTRY_DESKTOP_JAR:-/home/dr4g4ns/Escritorio/mindustry-159.7/jre/159.7.jar}"
java_server_jar="${MINDUSTRY_SERVER_JAR:-$project_dir/tools/bench/server-release-159.7.jar}"
class_dir="$project_dir/target/bench-load-classes"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out_dir="${OUT_DIR:-$project_dir/target/bench-e2e-$stamp}"
mkdir -p "$out_dir" "$class_dir" "$project_dir/tools/bench"

if [[ ! -f "$desktop_jar" ]]; then
  echo "error: desktop jar not found: $desktop_jar" >&2
  exit 1
fi
if [[ "$skip_java" != "1" && ! -f "$java_server_jar" ]]; then
  echo "error: java server jar not found: $java_server_jar" >&2
  echo "download: https://github.com/Anuken/Mindustry/releases/download/v159.7/server-release.jar" >&2
  exit 1
fi

echo "== bench e2e compare =="
echo "players=$players duration=${duration_sec}s warmup=${warmup_sec}s ramp_ms=$ramp_ms"
echo "desktop_jar=$desktop_jar"
echo "java_server_jar=$java_server_jar"
echo "out_dir=$out_dir"
echo "available_mem_kb=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)"

echo "== build rust release =="
cargo build --release

echo "== compile BenchLoadN =="
javac --release 17 -d "$class_dir" -cp "$desktop_jar" tools/bench/BenchLoadN.java

sample_metrics() {
  local pid="$1"
  local out_csv="$2"
  local interval_sec="${3:-1}"
  echo "ts_epoch,elapsed_s,rss_kb,vsz_kb,cpu_pct,threads,fd_count" >"$out_csv"
  local start_epoch
  start_epoch="$(date +%s)"
  local prev_proc=0
  local prev_total=0
  local first=1
  while kill -0 "$pid" 2>/dev/null; do
    if [[ ! -r "/proc/$pid/stat" || ! -r "/proc/$pid/status" ]]; then
      break
    fi
    local rss_kb vsz_kb threads fd_count proc_jiffies total_jiffies cpu_pct now elapsed
    rss_kb="$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")"
    vsz_kb="$(awk '/^VmSize:/ {print $2}' "/proc/$pid/status")"
    threads="$(awk '/^Threads:/ {print $2}' "/proc/$pid/status")"
    fd_count=0
    if [[ -d "/proc/$pid/fd" ]]; then
      fd_count="$(find "/proc/$pid/fd" 2>/dev/null | wc -l | tr -d ' ')"
    fi
    proc_jiffies="$(python3 - "$pid" <<'PY'
import pathlib, sys
pid = sys.argv[1]
text = pathlib.Path(f"/proc/{pid}/stat").read_text()
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
path = sys.argv[1]
rows = list(csv.DictReader(open(path)))
if not rows:
    print(json.dumps({"samples": 0}))
    raise SystemExit
def f(key):
    return [float(r[key]) for r in rows if r.get(key) not in (None, "")]
rss = f("rss_kb")
cpu = f("cpu_pct")
thr = f("threads")
fds = f("fd_count")
def stats(xs):
    if not xs:
        return None
    xs = sorted(xs)
    def pct(p):
        i = min(len(xs)-1, max(0, int(round(p*(len(xs)-1)))))
        return xs[i]
    return {
        "min": xs[0],
        "avg": sum(xs)/len(xs),
        "p50": pct(0.50),
        "p95": pct(0.95),
        "max": xs[-1],
    }
out = {
    "samples": len(rows),
    "rss_kb": stats(rss),
    "rss_mb": {k: (v/1024.0 if isinstance(v, (int,float)) else v) for k,v in (stats(rss) or {}).items()} if rss else None,
    "cpu_pct": stats(cpu),
    "threads": stats(thr),
    "fd_count": stats(fds),
    "steady_rss_kb": stats(rss[max(0,len(rss)//3):]) if rss else None,
    "steady_cpu_pct": stats(cpu[max(0,len(cpu)//3):]) if cpu else None,
}
print(json.dumps(out))
PY
}

wait_port() {
  local port="$1"
  local seconds="${2:-30}"
  local i
  for ((i=0; i<seconds*10; i++)); do
    if ss -ltn "( sport = :$port )" 2>/dev/null | grep -q ":$port"; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

run_load_client() {
  local port="$1"
  local client_log="$2"
  local json_out="$3"
  local uuid_bytes="${4:-16}"
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
    >"$client_log" 2>"${client_log%.log}.err.log"
  local rc=$?
  set -e
  if grep -q '^BENCH_JSON ' "$client_log"; then
    grep '^BENCH_JSON ' "$client_log" | tail -1 | sed 's/^BENCH_JSON //' >"$json_out"
  else
    echo '{"error":"missing BENCH_JSON","exit_code":'"$rc"'}' >"$json_out"
  fi
  return 0
}

run_rust_case() {
  local case_dir="$out_dir/rust"
  mkdir -p "$case_dir"
  local save_file="$case_dir/world.json"
  cp "$project_dir/tools/smoke/smoke-multiplayer-world-158.json" "$save_file"
  # Isolated admin state: avoid repo admin-data.json player_limit/bans leaking in.
  cat >"$case_dir/admin-data.json" <<EOF
{"banned_ips":[],"banned_uuids":[],"dos_banned_ips":[],"subnet_bans":[],"whitelist":[],"whitelist_enabled":false,"admins":[],"player_limit":0,"server_name":"bench-rust","server_description":"bench","server_build":$build,"server_version_type":"official"}
EOF
  local server_log="$case_dir/server.log"
  local metrics_csv="$case_dir/metrics.csv"
  local client_log="$case_dir/client.log"
  local client_json="$case_dir/client.json"

  echo "== rust server on :$rust_port =="
  (
    cd "$case_dir"
    RUST_LOG="${RUST_LOG:-info,develop=info}" \
    "$project_dir/target/release/oxide" \
      --no-tui \
      --port "$rust_port" \
      --max-players "$players" \
      --tps 60 \
      --build "$build" \
      --version-type official \
      --name "bench-rust" \
      --save-file "$save_file" \
      --develop \
      --develop-interval-ms 2000 \
      >"$server_log" 2>&1 &
    echo $! >"$case_dir/server.pid"
    wait || true
  ) &
  local wrapper_pid=$!
  local server_pid=""
  local _
  for _ in $(seq 1 100); do
    if [[ -f "$case_dir/server.pid" ]]; then
      server_pid="$(cat "$case_dir/server.pid")"
      if kill -0 "$server_pid" 2>/dev/null; then
        break
      fi
    fi
    sleep 0.1
  done
  if [[ -z "$server_pid" ]]; then
    echo "rust server pid not captured" >&2
    return 1
  fi

  cleanup_rust() {
    if kill -0 "$server_pid" 2>/dev/null; then
      kill "$server_pid" 2>/dev/null || true
      wait "$server_pid" 2>/dev/null || true
    fi
    wait "$wrapper_pid" 2>/dev/null || true
  }
  trap cleanup_rust RETURN

  if ! wait_port "$rust_port" 20; then
    echo "rust server failed to bind :$rust_port" >&2
    tail -50 "$server_log" >&2 || true
    return 1
  fi

  sample_metrics "$server_pid" "$metrics_csv" 1 &
  local sampler_pid=$!

  run_load_client "$rust_port" "$client_log" "$client_json" 16

  kill "$sampler_pid" 2>/dev/null || true
  wait "$sampler_pid" 2>/dev/null || true
  cleanup_rust
  trap - RETURN

  summarize_metrics_csv "$metrics_csv" >"$case_dir/metrics_summary.json"
  python3 - "$server_log" "$case_dir/server_derived.json" <<'PY'
import json, re, sys
log, out = sys.argv[1], sys.argv[2]
text = open(log, errors="replace").read()
ups = re.findall(r"ups=([0-9.]+)", text)
tick_avg = re.findall(r"tick_avg_us\(win\)=([0-9]+)", text)
tick_max = re.findall(r"tick_max_us=([0-9]+)", text)
players = re.findall(r"players=([0-9]+)", text)
rss = re.findall(r"rss=([0-9]+)mb", text)
json.dump({
    "develop_ups_last": float(ups[-1]) if ups else None,
    "develop_tick_avg_us_last": int(tick_avg[-1]) if tick_avg else None,
    "develop_tick_max_us_last": int(tick_max[-1]) if tick_max else None,
    "develop_players_last": int(players[-1]) if players else None,
    "develop_rss_mb_last": int(rss[-1]) if rss else None,
    "joined_log_lines": text.count("finished loading the world"),
}, open(out, "w"), indent=2)
print(open(out).read())
PY
}

run_java_case() {
  local case_dir="$out_dir/java"
  mkdir -p "$case_dir"
  local work_dir="$case_dir/workdir"
  # Fresh admin state so prior uuid-change bans do not leak into the run.
  rm -rf "$work_dir/config" "$work_dir/settings.bin" 2>/dev/null || true
  mkdir -p "$work_dir"
  local server_log="$case_dir/server.log"
  local metrics_csv="$case_dir/metrics.csv"
  local client_log="$case_dir/client.log"
  local client_json="$case_dir/client.json"
  local fifo="$case_dir/server.fifo"
  rm -f "$fifo"
  mkfifo "$fifo"

  echo "== java server on :$java_port =="
  (
    cd "$work_dir"
    java -Xms512m -Xmx2g --enable-native-access=ALL-UNNAMED \
      -jar "$java_server_jar" <"$fifo" >"$server_log" 2>&1 &
    echo $! >"$case_dir/server.pid"
    wait || true
  ) &
  local wrapper_pid=$!

  # Keep the FIFO write-end open for the whole case.
  exec 9>"$fifo"

  local server_pid=""
  local _
  for _ in $(seq 1 100); do
    if [[ -f "$case_dir/server.pid" ]]; then
      server_pid="$(cat "$case_dir/server.pid")"
      if kill -0 "$server_pid" 2>/dev/null; then
        break
      fi
    fi
    sleep 0.1
  done
  if [[ -z "$server_pid" ]]; then
    echo "java server pid not captured" >&2
    return 1
  fi

  cleanup_java() {
    echo "exit" >&9 || true
    sleep 0.5
    if kill -0 "$server_pid" 2>/dev/null; then
      kill "$server_pid" 2>/dev/null || true
      wait "$server_pid" 2>/dev/null || true
    fi
    exec 9>&- || true
    wait "$wrapper_pid" 2>/dev/null || true
    rm -f "$fifo"
  }
  trap cleanup_java RETURN

  for _ in $(seq 1 150); do
    if grep -q "Server loaded" "$server_log" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  {
    echo "config port $java_port"
    echo "config uuidChangeLimit 0"
    echo "playerlimit $players"
    echo "unban 127.0.0.1"
    echo "host maze survival"
  } >&9

  if ! wait_port "$java_port" 60; then
    echo "java server failed to bind :$java_port" >&2
    tail -80 "$server_log" >&2 || true
    return 1
  fi
  sleep 1
  echo "status" >&9

  sample_metrics "$server_pid" "$metrics_csv" 1 &
  local sampler_pid=$!

  run_load_client "$java_port" "$client_log" "$client_json" 8

  echo "status" >&9
  sleep 1
  kill "$sampler_pid" 2>/dev/null || true
  wait "$sampler_pid" 2>/dev/null || true
  cleanup_java
  trap - RETURN

  summarize_metrics_csv "$metrics_csv" >"$case_dir/metrics_summary.json"
  python3 - "$server_log" "$case_dir/server_derived.json" <<'PY'
import json, re, sys
log, out = sys.argv[1], sys.argv[2]
text = open(log, errors="replace").read()
fps = re.findall(r"(\d+)\x1b\[[0-9;]*m*\s*FPS,\s*\x1b\[[0-9;]*m*\s*(\d+)\x1b\[[0-9;]*m*\s*MB used", text)
if not fps:
    # Fallback without ANSI, or with partial escapes stripped.
    plain = re.sub(r"\x1b\[[0-9;]*m", "", text)
    fps = re.findall(r"(\d+)\s+FPS,\s+(\d+)\s+MB used", plain)
players = re.findall(r"Players:\s*(?:\x1b\[[0-9;]*m*)*(\d+)", text)
if not players:
    plain = re.sub(r"\x1b\[[0-9;]*m", "", text)
    players = re.findall(r"Players:\s*(\d+)", plain)
json.dump({
    "status_fps_mb_samples": [{"fps": int(a), "heap_mb": int(b)} for a,b in fps],
    "status_fps_last": int(fps[-1][0]) if fps else None,
    "status_heap_mb_last": int(fps[-1][1]) if fps else None,
    "status_players_last": int(players[-1]) if players else None,
    "map_loaded": ("Map loaded." in text),
}, open(out, "w"), indent=2)
print(open(out).read())
PY
}

if [[ "$skip_rust" != "1" ]]; then
  run_rust_case
fi
if [[ "$skip_java" != "1" ]]; then
  run_java_case
fi

python3 - "$out_dir" "$players" "$duration_sec" "$warmup_sec" <<'PY'
import json, pathlib, sys
out_dir = pathlib.Path(sys.argv[1])
players = int(sys.argv[2])
duration = int(sys.argv[3])
warmup = int(sys.argv[4])

def load(path):
    p = out_dir / path
    if not p.exists():
        return None
    return json.loads(p.read_text())

report = {
    "players": players,
    "duration_sec": duration,
    "warmup_sec": warmup,
    "rust": {
        "client": load("rust/client.json"),
        "metrics": load("rust/metrics_summary.json"),
        "server_derived": load("rust/server_derived.json"),
    },
    "java": {
        "client": load("java/client.json"),
        "metrics": load("java/metrics_summary.json"),
        "server_derived": load("java/server_derived.json"),
    },
}
(out_dir / "comparison.json").write_text(json.dumps(report, indent=2) + "\n")

def fmt_metrics(side):
    m = (report.get(side) or {}).get("metrics") or {}
    c = (report.get(side) or {}).get("client") or {}
    d = (report.get(side) or {}).get("server_derived") or {}
    rss = (m.get("rss_mb") or {})
    cpu = (m.get("cpu_pct") or {})
    steady_cpu = (m.get("steady_cpu_pct") or {})
    steady_rss = (m.get("steady_rss_kb") or {})
    lines = []
    lines.append(f"### {side.upper()}")
    if c:
        lines.append(
            f"- clients: joined={c.get('joined')}/{c.get('players_requested')} "
            f"confirmed={c.get('confirmed')} kicked={c.get('kicked')} "
            f"still_connected={c.get('still_connected')}"
        )
        lines.append(
            f"- join_ms: avg={c.get('join_ms_avg')} p50={c.get('join_ms_p50')} "
            f"p95={c.get('join_ms_p95')} p99={c.get('join_ms_p99')} max={c.get('join_ms_max')}"
        )
        lines.append(
            f"- ping_rtt_ms: avg={c.get('ping_rtt_ms_avg')} max={c.get('ping_rtt_ms_max')} "
            f"samples={c.get('ping_samples')}"
        )
        lines.append(
            f"- traffic: snapshots_tx={c.get('snapshots_tx')} entity_rx={c.get('entity_snapshots_rx')} "
            f"state_rx={c.get('state_snapshots_rx')} packets_rx={c.get('packets_rx')}"
        )
    if rss:
        lines.append(
            f"- rss_mb: avg={rss.get('avg'):.1f} p50={rss.get('p50'):.1f} "
            f"p95={rss.get('p95'):.1f} max={rss.get('max'):.1f}"
        )
    if steady_rss:
        lines.append(
            f"- steady_rss_mb: avg={steady_rss.get('avg',0)/1024:.1f} max={steady_rss.get('max',0)/1024:.1f}"
        )
    if cpu:
        lines.append(
            f"- cpu_pct: avg={cpu.get('avg'):.1f} p50={cpu.get('p50'):.1f} "
            f"p95={cpu.get('p95'):.1f} max={cpu.get('max'):.1f}"
        )
    if steady_cpu:
        lines.append(
            f"- steady_cpu_pct: avg={steady_cpu.get('avg'):.1f} max={steady_cpu.get('max'):.1f}"
        )
    thr = m.get("threads") or {}
    if thr:
        lines.append(f"- threads: avg={thr.get('avg'):.0f} max={thr.get('max'):.0f}")
    if d:
        if side == "rust":
            lines.append(
                f"- develop: ups={d.get('develop_ups_last')} "
                f"tick_avg_us={d.get('develop_tick_avg_us_last')} "
                f"tick_max_us={d.get('develop_tick_max_us_last')} "
                f"players={d.get('develop_players_last')} rss_mb={d.get('develop_rss_mb_last')}"
            )
        else:
            lines.append(
                f"- status: fps={d.get('status_fps_last')} heap_mb={d.get('status_heap_mb_last')} "
                f"players={d.get('status_players_last')} map_loaded={d.get('map_loaded')}"
            )
    if not lines:
        return "- (no data)\n"
    return "\n".join(lines) + "\n"

md = []
md.append(f"# Bench E2E Java vs Rust — {players} players\n")
md.append(f"- duration_sec: {duration}")
md.append(f"- warmup_sec: {warmup}")
md.append(f"- artifacts: `{out_dir}`\n")
md.append(fmt_metrics("rust"))
md.append(fmt_metrics("java"))
md.append("## Raw\n")
md.append(f"- comparison.json")
md.append(f"- rust/: server.log metrics.csv client.json")
md.append(f"- java/: server.log metrics.csv client.json\n")
(out_dir / "REPORT.md").write_text("\n".join(md) + "\n")
print("\n".join(md))
print(f"Wrote {out_dir / 'comparison.json'}")
print(f"Wrote {out_dir / 'REPORT.md'}")
PY

echo "DONE out_dir=$out_dir"
