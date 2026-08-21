# Benchmark: Java 159.7 vs Oxide

Reproducible measurement of the same harness against:

- **Rust:** `target/release/oxide` from this repository
- **Java:** official v159.7 `server-release.jar` (`tools/bench/server-release-159.7.jar`, not committed)

| | |
|---|---|
| generated_utc | `2026-08-21T01:52:53Z` |
| harness | `tools/bench/bench_workload_suite.sh` + `tools/bench/BenchLoadN.java` |
| workload | `build`: ClientSnapshot place copper-wall → shoot → break |
| map / mode | maze survival @ 60 TPS |
| window | duration 25 s, warmup 5 s, snapshot_hz 10 |
| scales | 10, 100, 500 players |
| raw artifacts | `target/bench-workload-full/comparison.json` (local, untracked) |

Table numbers match that JSON (join_ms, rss_mb, construct rx, UPS/FPS). They were not rewritten by hand.

## How to read this (fidelity)

This is **not** “identical simulation, two languages”:

1. **Different enemy pressure.** Rust injects `waves` + `spawn dagger/crawler/flare` (peak ~120–180). Java uses `runwave` (status peaked around ~23 enemies in these runs).
2. **Build ACKs.** Both sides receive the same build/break/shoot TX. Rust returns `BeginPlace` / `ConstructFinish` / `DeconstructFinish` / `UnitDeath` to the load clients. Headless Java did **not** show those Call packets on this harness (`rx = 0`). That is a harness observability gap, not proof that Java ignored the plans.
3. **UUID.** `--uuid-bytes 16` against Rust (official write is 16 + CRC) and `8` against Java (`ConnectPacket.read` skips CRC).
4. **Low Java CPU% with 1 FPS at 500 players** means the simulation loop is starved, not that Java is cheaper.
5. At 500 players, ping rises to ~1.7–2.3 s avg on both sides; joins remain 500/500.

Reproduce:

```bash
# desktop 159.7 JAR + server-release.jar 159.7
export MINDUSTRY_DESKTOP_JAR=/path/to/159.7.jar
export MINDUSTRY_SERVER_JAR=/path/to/server-release.jar
SCALES="10 100 500" DURATION_SEC=25 bash tools/bench/bench_workload_suite.sh
```

Cheap smoke scale: `SCALES=10 DURATION_SEC=15`.

After the tools were reorganized, a shorter 10-player run (12 s) reproduced the same qualitative result (Rust ~15 MB vs Java ~280 MB; Java construct rx still 0). The published table below is still the 25 s full suite.

## Verdict

| Scale | Winner (memory) | Winner (join) | Simulation under load |
|------:|-----------------|---------------|------------------------|
| 10 | **Rust** (~15 MB vs ~280 MB) | **Rust** | Both hold ~60 UPS/FPS |
| 100 | **Rust** (~24 MB vs ~365 MB) | **Rust** | Rust 60 UPS; Java ~57 FPS |
| 500 | **Rust** (~80–100 MB vs ~476–513 MB) | **Rust** (~6× faster avg join) | Rust ~33 UPS; **Java collapses to 1 FPS** |

## Summary table

| players | server | joined | join_ms avg/p95 | rss_mb avg/max | steady_cpu% avg/max | sim (UPS/FPS) | enemies | construct/deconstruct rx | unit_deaths |
|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 10 | rust | 10/10 | 6.6 / 31 | 14.7 / 15.3 | 1.3 / 1.6 | 60.0 | 121 | 250/210 | 40 |
| 10 | java | 10/10 | 30.2 / 108 | 278.6 / 283.3 | 0.5 / 1.1 | 61 | 23 | 0/0 | 0 |
| 100 | rust | 100/100 | 4.2 / 5 | 23.4 / 24.3 | 23.1 / 29.1 | 60.0 | 153 | 42506/37100 | 2500 |
| 100 | java | 100/100 | 12.1 / 39 | 347.4 / 385.1 | 1.2 / 2.6 | 57 | 23 | 0/0 | 0 |
| 500 | rust | 500/500 | 20.2 / 47 | 65.2 / 101.2 | 55.1 / 60.0 | 33.4 | 178 | 67289/2 | 0 |
| 500 | java | 500/500 | 119.6 / 523 | 430.9 / 512.6 | 4.4 / 9.3 | 1 | 23 | 0/0 | 0 |

At 500 players, Rust `still_connected=481` at window end (UPS 33); Java keeps 500 connected at 1 FPS.

## Scale 10

#### RUST — 10 players
- joined: **10/10** (confirmed=10, kicked=0, still_connected=10)
- join_ms: avg=6.6 p50=4 p95=31 p99=31 max=31
- ping_rtt_ms: avg=0.2 max=2 samples=2990
- workload tx: build_snap=1120 break_snap=980 shoot_snap=690
- workload rx: begin_place=240 construct_finish=250 begin_break=210 deconstruct_finish=210 unit_death=40 bullets=15929
- traffic: snapshots_tx=3000 entity_rx=126318 state_rx=6060 packets_rx=160124
- rss_mb: avg=14.7 p50=15.1 p95=15.3 max=15.3
- steady_cpu_pct: avg=1.3 max=1.6
- threads: avg=36 max=36
- develop: ups_last=60.0 ups_min=59.9 tick_avg_us=928 enemies_max=121 rss_mb=15

#### JAVA — 10 players
- joined: **10/10**
- join_ms: avg=30.2 p50=12 p95=108 max=108
- ping_rtt_ms: avg=8.6 max=18 samples=2989
- workload tx: build_snap=1120 break_snap=980 shoot_snap=689
- workload rx: construct/deconstruct/deaths/bullets = 0 (harness)
- traffic: snapshots_tx=2999 entity_rx=4829 state_rx=1501 packets_rx=15039
- rss_mb: avg=278.6 max=283.3
- steady_cpu_pct: avg=0.5 max=1.1
- status: fps_last=61 fps_min=47 heap_mb=103 enemies=23

## Scale 100

#### RUST — 100 players
- joined: **100/100**
- join_ms: avg=4.2 p50=4 p95=5 max=38
- ping_rtt_ms: avg=2.4 max=59 samples=29900
- workload tx: build_snap=11200 break_snap=9800 shoot_snap=6900
- workload rx: begin_place=40863 construct_finish=42506 begin_break=37100 deconstruct_finish=37100 unit_death=2500 bullets=187415
- traffic: snapshots_tx=30000 entity_rx=8594176 packets_rx=9113583
- rss_mb: avg=23.4 max=24.3
- steady_cpu_pct: avg=23.1 max=29.1
- develop: ups_last=60.0 ups_min=59.2 tick_avg_us=1890 enemies_max=153

#### JAVA — 100 players
- joined: **100/100**
- join_ms: avg=12.1 p50=7 p95=39 max=124
- ping_rtt_ms: avg=10.9 max=54 samples=29899
- workload tx: build_snap=11200 break_snap=9800 shoot_snap=6899
- rss_mb: avg=347.4 max=385.1
- steady_cpu_pct: avg=1.2 max=2.6
- status: fps_last=57 fps_min=44 heap_mb=166 enemies=23

## Scale 500

#### RUST — 500 players
- joined: **500/500** (still_connected=481 at cutoff)
- join_ms: avg=20.2 p50=18 p95=47 max=84
- ping_rtt_ms: avg=2279.8 max=15998 samples=5420
- workload tx: build_snap=54623 break_snap=47790 shoot_snap=33144
- workload rx: begin_place=19776 construct_finish=67289 begin_break=54 deconstruct_finish=2 unit_death=0 bullets=525253
- traffic: snapshots_tx=145788 entity_rx=26775971 packets_rx=27636262
- rss_mb: avg=65.2 max=101.2
- steady_cpu_pct: avg=55.1 max=60.0
- develop: ups_last=33.4 ups_min=29.2 tick_avg_us=17145 enemies_max=178 rss_mb=100

#### JAVA — 500 players
- joined: **500/500** (still_connected=500)
- join_ms: avg=119.6 p50=26 p95=523 max=850
- ping_rtt_ms: avg=1683.9 max=2271 samples=8472
- workload tx: build_snap=55737 break_snap=49000 shoot_snap=34332
- rss_mb: avg=430.9 max=512.6
- steady_cpu_pct: avg=4.4 max=9.3
- status: fps_last=1 fps_min=1 heap_mb=104 enemies=23
