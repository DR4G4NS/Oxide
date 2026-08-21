# Oxide

<p align="center">
  <img src="assets/oxide-logo.png" alt="Oxide" width="220">
</p>

<p align="center">
  <strong>Headless Mindustry server in Rust</strong><br/>
  <em>independently authored · authoritative · built for high load</em>
</p>

<p align="center">
  <img alt="version" src="https://img.shields.io/badge/version-0.1.0--alpha.1-orange">
  <img alt="channel" src="https://img.shields.io/badge/channel-alpha-yellow">
  <img alt="target" src="https://img.shields.io/badge/Mindustry-v8%20159.7-2ea44f">
  <img alt="license" src="https://img.shields.io/badge/license-Apache--2.0-blue">
  <img alt="lang" src="https://img.shields.io/badge/Rust-2021-dea584">
</p>

<p align="center">
  <a href="#english">English</a> · <a href="#español">Español</a>
</p>

| | |
|---|---|
| **Product** | `0.1.0-alpha.1` |
| **Crate** (`Cargo.toml`) | `0.1.0` |
| **Compatibility target** | Mindustry v8 **159.7** (`compat/current.toml`) |
| **Historical smoke baseline** | desktop.jar **158.1** (many harnesses still named `*_158`) |
| **Original code license** | Apache-2.0 — [LICENSE](LICENSE), [NOTICE](NOTICE), [THIRD_PARTY.md](THIRD_PARTY.md) |
| **Official campaign maps** | GPLv3 (Anuken) — [third_party/mindustry-maps](third_party/mindustry-maps/) |

Docs in this repo:

- this README — pitch, scope, gaps, how to run
- [ARCHITECTURE.md](ARCHITECTURE.md) — modules, protocol, building recipe *(English)*
- [CONTRIBUTING.md](CONTRIBUTING.md) — pull requests *(English + Español)*
- [AGENTS.md](AGENTS.md) — rails for coding agents on a PR *(English)*
- [SECURITY.md](SECURITY.md) — vulnerability reports *(English)*
- [benchmark.md](benchmark.md) — Rust vs Java 159.7 under load *(English)*
- [THIRD_PARTY.md](THIRD_PARTY.md) — provenance *(English)*

---

# English

**Oxide** is an experimental Mindustry server port: ArcNet handshake, world stream, tick simulation, economy, combat, and console/TUI. **Not the official server** and not affiliated with Anuken.

## What it is (and is not)

**It is** an authoritative server: the client predicts; Rust validates and advances the state that is then replicated (`EntitySnapshot`, `BlockSnapshot`, RPC). This alpha aims to be **experimentally hostable in production** for vanilla Serpulo (survival / sandbox / pvp / attack) with official 159.7 clients. It is not byte-for-byte behavioral parity of all vanilla.

**It is not**

- a 100% drop-in for `server-release.jar` (Erekir economy, RTS AI, missile units, mods…)
- compatible with **mods, plugins, scripts, custom packets, or an event bus** — modded clients are rejected
- a multi-build runtime: one binary, one target (`159.7`)

Wire/save certification for the documented scope lives in `compat/159.7/certification-ledger.json` (`overall=PASS`, `CERTIFIED_RUNTIME_SHA` = `f0d5cbb7…`). That does **not** mean the entire Java game is ported.

## Alpha status

Playable today (Serpulo critical path): TCP+UDP on the same port, LAN discovery, join, world stream, `ConnectConfirm`; movement, timed build/break, mining, chat, ping; survival waves, main Serpulo turrets, authoritative projectiles; conveyors/routers/bridges, basic power, liquids, Serpulo factories and reconstructors; Logic `ubind` + `ucontrol` subset (move/stop/within/getBlock/flag/boost/mine/itemDrop/itemTake/shoot/target/build/unbind); operator TUI (`--tui` / `--no-tui`, **out of** vanilla parity); real 158.1 JAR smokes and 159.7 CI certification.

Gaps below **do** break a full Erekir campaign or “exact vanilla”. They are not hidden by faster snapshots: if the client predicts a block, Rust must simulate it.

## Gaps — Serpulo

Playable, **not** exact 158.1/159.7 across all content.

| Area | Missing |
|---|---|
| **Units** | 14 vanilla UnitTypes with **no** `enemy_spec` (strict rejects): anthicus + missiles, quell/disrupt missiles, evoke/incite/emanate/block, manifold/assembly-drone, scathe-missiles. Internal: `turret-unit-build-tower`. |
| **Projectiles** | Scepter/pulsar/arkyid lightning not spawned; continuous beams collapse to one impact; missile homing is ballistic; `spawnUnit` is terminal splash (**no** MissileUnit insert); SAP lifesteal on expiry even with no damage. |
| **`ucontrol`** | NoOp: idle, approach, pathfind, autoPathfind, payDrop, payTake, payEnter, targetp, … |
| **Status** | Truly simulated: burning, electrified, overdrive, overclock, shielded. The rest is a tag or lacks multiplier/tick damage. |
| **Team AI** | `rtsAi` only picks CommandAI vs GroundAI. No `BaseBuilderAI`, RtsAI squads, `prebuildAi` / `buildAi`. |
| **Power** | Beam-node link topology incomplete. |
| **Official maps** | `host_map("archipelago")` can fail on MSAV entity chunks (`UnexpectedEof`). |
| **Puddles / liquids** | Authoritative model exists; conduit cadence/leaks can diverge from the JAR. |
| **Logic assembler** | In 158.1 `set a b + c` is the literal `b_+_c`; Rust does interpret the expression. |

## Gaps — Erekir

Erekir is **not** at Serpulo level. `.msav` maps load (the format is not planet-specific); simulation and netcode do not cover an Erekir campaign.

| Area | Status |
|---|---|
| **Ducts / heat / beam drills / crafters 199–202** | Domain simulation (`src/network/economy/erekir.rs`) |
| **Erekir turrets** | Partial (liquid/item ammo) |
| **Assemblers 393–395 / fabricators 386–388** | Codecs / fixed plan; full production is not parity |
| **Erekir units on the wire** | 35 Serpulo classes verified with `readSyncEntity`. CrawlUnit / TankUnit / Erekir missiles **lack** the same snapshot closure |
| **Factory payloads, campaign pads, logic-display/canvas** | Incomplete or empty in sync |
| **Neoplasm / missile parents** | REJECTED or PARTIAL |

A real Erekir map (own factories + units + heat + assemblers) is **not** a Java replacement in this alpha.

## Benchmark vs Java 159.7

From `tools/bench/bench_workload_suite.sh` on **2026-08-21**, maze survival @ 60 TPS, 25 s + 5 s warmup. Full numbers and caveats: [benchmark.md](benchmark.md).

| Players | RSS Rust / Java | Join avg Rust / Java | Simulation |
|---:|---|---|---|
| 10 | **15 MB** / 280 MB | **6.6 ms** / 30 ms | both ~60 UPS/FPS |
| 100 | **24 MB** / 365 MB | **4.2 ms** / 12 ms | Rust 60 UPS; Java ~57 FPS |
| 500 | **80–100 MB** / 476–513 MB | **20 ms** / 120 ms | Rust ~33 UPS; **Java 1 FPS** |

Rust sustains construct/deconstruct traffic and waves (peak ~120–180 enemies). Java accepts the same snapshot TX; this harness **does not** see `ConstructFinish`/`UnitDeath` (`rx = 0`) — an observability gap, not proof the server ignored plans. Enemy pressure is also not identical (`spawn`+`waves` vs `runwave`).

## Run

```bash
cargo run --release -- --port 6567 --tps 60 --max-players 100 \
  --save-file world-delta.json
```

TUI opens automatically on an interactive TTY. `--no-tui` keeps the classic console. Headless when there is no TTY. `--tui` forces the dashboard. The chosen port must be open for **TCP and UDP**.

```bash
cargo run --release -- --port 6567 \
  --map-file path/to/map.msav \
  --save-file archipelago.json
```

Sandbox preset on a Survival map:

```bash
cargo run --release -- --port 6567 --tps 60 \
  --map-file cavesurvival.msav --mode sandbox \
  --save-file target/cavesurvival-sandbox.json
```

Each map needs its own `--save-file`. Do not reuse a save with `game_over=true` for a clean trial.

```bash
cargo run -- --help
```

## Verify

Without a JAR:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=1
git diff --check
```

With desktop.jar (158.1 for historical smokes):

```bash
export MINDUSTRY_DESKTOP_JAR=/path/to/desktop.jar
bash tools/smoke/smoke_join_158.sh
bash tools/smoke/smoke_multiplayer_158.sh
```

Differential parity (fixtures already captured):

```bash
cargo test --lib parity
```

Artifact cleanup: `bash tools/clean_artifacts.sh`.

## Tools

```
tools/
  smoke/         ArcNet smokes + E2E + VerifyProtocol (Java + bash + world JSON)
  inspect/       JAR manifest extractors (Inspect*.java, export_*.py)
  parity/        tests/parity/probes runner → fixtures
  bench/         BenchLoadN + Rust vs Java 159.7 suite
  archguard/     module dependency guard
  dashmap_guard/ DashMap analyzer DM001–DM005
  compatlib/     159.7 ledger and manifests
```

CI gates (stay at `tools/` root): `architecture_guard.sh`, `compat_gate.sh`, `compat_jar_gate.sh`, `cert_ledger.py`, `low_core_test.py`, `mindustry_manifest.py`.

Content manifests (tabular authority, not markdown): `src/game/*.tsv`.

## Out of scope (0.1 line)

Mods, plugins, RequestAssets / asset streams, full interactive Rules editing (`SetRules` inbound), protocol fuzzing, and exact vanilla of every Erekir block.

Architecture and wire contract: [ARCHITECTURE.md](ARCHITECTURE.md).

## Contributing

Human PRs: [CONTRIBUTING.md](CONTRIBUTING.md). If you use a coding agent on the change, it must follow [AGENTS.md](AGENTS.md).

---

# Español

**Oxide** es un port experimental del servidor de Mindustry: handshake ArcNet, world stream, simulación de tick, economía, combate y consola/TUI. **No es el servidor oficial** y no está afiliado a Anuken.

## Qué es (y qué no)

**Es** un servidor autoritativo: el cliente predice; Rust valida y avanza el estado que luego se replica (`EntitySnapshot`, `BlockSnapshot`, RPC). Objetivo de esta alpha: **hosteable en producción experimental** para partidas vanilla Serpulo (survival / sandbox / pvp / attack) con clientes oficiales 159.7. No es paridad conductual byte-a-byte de todo vanilla.

**No es**

- un drop-in 100 % del `server-release.jar` (economía Erekir, AI RTS, misiles-unidad, mods…)
- compatible con **mods, plugins, scripts, packets custom o event bus** — los clientes con mods se rechazan
- un runtime multi-build: un binario, un target (`159.7`)

Certificación de wire/save del scope documentado: ledger `compat/159.7/certification-ledger.json` (`overall=PASS`, `CERTIFIED_RUNTIME_SHA` = `f0d5cbb7…`). Eso **no** significa “todo el juego Java está portado”.

## Estado de la alpha

Jugable hoy (Serpulo, camino crítico): TCP+UDP en el mismo puerto, LAN, join, world stream, `ConnectConfirm`; movimiento, construcción/deconstrucción temporizada, minería, chat, ping; oleadas, torretas Serpulo principales, proyectiles autoritativos; cintas/routers/bridges, power básico, líquidos, fábricas y reconstructores; Logic `ubind` + subset de `ucontrol` (move/stop/within/getBlock/flag/boost/mine/itemDrop/itemTake/shoot/target/build/unbind); TUI de operador (`--tui` / `--no-tui`, **fuera** de paridad vanilla); smokes contra el JAR 158.1 y certificación 159.7 en CI.

Los huecos de abajo **sí rompen** una campaña Erekir completa o “exact vanilla”. No se esconden subiendo la cadencia de snapshots: si el cliente predice un bloque, Rust tiene que simularlo.

## Huecos — Serpulo

Jugable, **no** exact 158.1/159.7 en todo el contenido.

| Área | Qué falta |
|---|---|
| **Unidades** | 14 UnitTypes vanilla **sin** `enemy_spec` (strict las rechaza): anthicus + misiles, quell/disrupt missiles, evoke/incite/emanate/block, manifold/assembly-drone, scathe-missiles. Interno: `turret-unit-build-tower`. |
| **Proyectiles** | Lightning de scepter/pulsar/arkyid no spawnea; beams continuos → un impacto; homing de misiles es balístico; `spawnUnit` es splash terminal (**no** inserta MissileUnit); SAP lifesteal en expiry aunque no haya daño. |
| **`ucontrol`** | NoOp: idle, approach, pathfind, autoPathfind, payDrop, payTake, payEnter, targetp, … |
| **Status** | Simulados de verdad: burning, electrified, overdrive, overclock, shielded. El resto es etiqueta o sin multiplicador/daño por tick. |
| **AI de equipo** | `rtsAi` solo elige CommandAI vs GroundAI. Sin `BaseBuilderAI`, squads RtsAI, `prebuildAi` / `buildAi`. |
| **Power** | Beam-node: topología de links incompleta. |
| **Mapas oficiales** | `host_map("archipelago")` puede fallar en chunks de entidades MSAV (`UnexpectedEof`). |
| **Puddles / líquidos** | Hay modelo autoritativo; cadencia/fugas de conductos pueden desviarse del JAR. |
| **Logic assembler** | En 158.1 `set a b + c` es el literal `b_+_c`; Rust sí interpreta la expresión. |

## Huecos — Erekir

Erekir **no** está al nivel Serpulo. Los `.msav` cargan (el formato no es por planeta); simulación y red no cubren una campaña Erekir.

| Área | Estado |
|---|---|
| **Ductos / heat / beam drills / crafters 199–202** | Simulación de dominio (`src/network/economy/erekir.rs`) |
| **Torretas Erekir** | Parcial (munición líquida/item) |
| **Assemblers 393–395 / fabricators 386–388** | Codecs / plan fijo; producción completa no es paridad |
| **Unidades Erekir en red** | 35 clases Serpulo verificadas con `readSyncEntity`. CrawlUnit / TankUnit / misiles Erekir **sin** el mismo cierre |
| **Payloads, pads de campaña, logic-display/canvas** | Incompleto o vacío en sync |
| **Neoplasm / missile parents** | REJECTED o PARTIAL |

Si el mapa es Erekir de verdad (fábricas + unidades propias + heat + assemblers), esta alpha **no** sustituye a Java.

## Benchmark vs Java 159.7

Suite `tools/bench/bench_workload_suite.sh` el **2026-08-21**, maze survival @ 60 TPS, 25 s + 5 s warmup. Detalle en [benchmark.md](benchmark.md) *(inglés)*.

| Jugadores | RSS Rust / Java | Join avg Rust / Java | Simulación |
|---:|---|---|---|
| 10 | **15 MB** / 280 MB | **6.6 ms** / 30 ms | ambos ~60 UPS/FPS |
| 100 | **24 MB** / 365 MB | **4.2 ms** / 12 ms | Rust 60 UPS; Java ~57 FPS |
| 500 | **80–100 MB** / 476–513 MB | **20 ms** / 120 ms | Rust ~33 UPS; **Java 1 FPS** |

Rust mantiene construct/deconstruct y oleadas (pico ~120–180 enemigos). Java acepta el mismo TX; el harness **no** ve `ConstructFinish`/`UnitDeath` (`rx = 0`): hueco de observabilidad, no prueba de que ignore los planes. La presión de enemigos tampoco es idéntica (`spawn`+`waves` vs `runwave`).

## Ejecutar / verificar / tools

Ver la sección [English — Run](#run) más arriba: mismos comandos.

Fuera de alcance de la línea 0.1: mods, plugins, RequestAssets, `SetRules` inbound completo, fuzz de protocolo, exact vanilla de cada bloque Erekir.

## Contribuir

PRs: [CONTRIBUTING.md](CONTRIBUTING.md). Si el cambio lo hace un agente de código, tiene que seguir [AGENTS.md](AGENTS.md).
