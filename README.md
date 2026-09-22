# Oxide

<p align="center">
  <img src="assets/oxide-logo.png" alt="Oxide" width="220">
</p>

<p align="center">
  <strong>Headless Mindustry server in Rust</strong><br/>
  <em>independently authored · authoritative · built for high load</em>
</p>

<p align="center">
  <img alt="version" src="https://img.shields.io/badge/version-0.2.0-orange">
  <img alt="channel" src="https://img.shields.io/badge/channel-alpha-yellow">
  <img alt="target" src="https://img.shields.io/badge/Mindustry-v8%20160.5-2ea44f">
  <img alt="license" src="https://img.shields.io/badge/license-Apache--2.0-blue">
  <a href="https://discord.gg/AQ6Awkk48w"><img alt="Discord" src="https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white"></a>
  <img alt="lang" src="https://img.shields.io/badge/Rust-2021-dea584">
</p>

<p align="center">
  <a href="#english">English</a> · <a href="#español">Español</a>
  · <strong><a href="https://discord.gg/AQ6Awkk48w">Community Discord</a></strong>
</p>

| | |
|---|---|
| **Product** | `0.2.0` |
| **Crate** (`Cargo.toml`) | `0.2.0` |
| **Compatibility target** | Mindustry v8 **160.5** (`compat/current.toml`) |
| **Historical smoke baseline** | desktop.jar **158.1** (many harnesses still named `*_158`) |
| **Original code license** | Apache-2.0 — [LICENSE](LICENSE), [NOTICE](NOTICE), [THIRD_PARTY.md](THIRD_PARTY.md) |
| **Official campaign maps** | GPLv3 (Anuken) — [third_party/mindustry-maps](third_party/mindustry-maps/) |

Find documentation by task:

- this README — pitch, scope, gaps, how to run
- [ARCHITECTURE.md](ARCHITECTURE.md) — modules, protocol, building recipe *(English)*
- [CONTRIBUTING.md](CONTRIBUTING.md) — pull requests *(English + Español)*
- [AGENTS.md](AGENTS.md) — coding-agent constraints and task-specific reading *(English)*
- [navigation.md](navigation.md) — locate code, query the target JAR, run client harnesses *(English)*
- [gaps.md](gaps.md) — open divergences with code anchors and closure evidence *(English)*
- [SECURITY.md](SECURITY.md) — vulnerability reports *(English)*
- [benchmark.md](benchmark.md) — Rust vs Java 159.7 under load *(English)*
- [THIRD_PARTY.md](THIRD_PARTY.md) — provenance *(English)*

---

# English

**Oxide** is an experimental Mindustry server port: ArcNet handshake, world stream, tick simulation, economy, combat, and console/TUI. **Not the official server** and not affiliated with Anuken.

## What it is (and is not)

**It is** an authoritative server: the client predicts; Rust validates and advances the state that is then replicated (`EntitySnapshot`, `BlockSnapshot`, RPC). This alpha aims to be **experimentally hostable in production** for vanilla Serpulo (survival / sandbox / pvp / attack) with official 160.5 clients. It is not byte-for-byte behavioral parity of all vanilla.

**It is not**

- a 100% drop-in for `server-release.jar` (full Erekir campaign, exact AI behaviour, mods…)
- compatible with **mods, plugins, scripts, custom packets, or an event bus** — modded clients are rejected. This is an explicit product decision: zip/jar/hjson/scripts require a dedicated product change.
- a multi-build runtime: one binary, one target (`160.5`)

Wire/save compatibility is checked with the pinned 160.5 manifests, Rust fixtures and target-JAR probes. There is no runtime-SHA certification gate; passing these checks does **not** mean the entire Java game is ported.

## Alpha status

Playable today (Serpulo critical path): TCP+UDP, LAN discovery, join and world stream; movement, timed build/break, mining, chat and ping; survival waves, main Serpulo turrets and authoritative projectiles; transport, power, liquids, factories and reconstructors; Logic `ubind`/`ucontrol`; operator TUI (`--tui` / `--no-tui`, outside vanilla parity). The current compatibility target is **160.5**; older smoke artifacts are historical evidence.

The tables below summarize coverage and remaining product limitations. Concrete behavioural divergences, their code anchors, and required regression evidence live in [gaps.md](gaps.md). Implemented content does not imply exact vanilla parity. The Serpulo audit
has incomplete closure evidence and remaining behaviour gaps; see
[gaps.md](gaps.md). It does not certify bug-free sandbox
or survival.

## Gaps — Serpulo

| Area | Coverage and remaining limits |
|---|---|
| **Units and AI** | Vanilla unit IDs have specs and sync layouts; missile lifetime, cargo delivery, and builder repair are simulated. Poly/Mega repair commands aim at damaged allied buildings and heal on projectile impact; idle weapons stop firing. Assembler drones reach perimeter slots but do not ferry payload stacks. Squad/follow behaviour remains simplified; full `BaseBuilderAI` base-plan construction and requirements-driven mining/prioritisation are missing. |
| **Unit production** | Factories and reconstructors retain payload/progress across snapshots and pauses; consumption follows the actual payload and team costs. Recipes and input capacities use the JAR-checked [production table](src/game/unit_production.tsv), including graphite for the additive reconstructor and metaglass for the multiplicative reconstructor and Risso. Carried units count toward the unit cap. Explicit factory deselection survives sync. Full higher-tier liquid-supply and retained-payload save/rally matrices remain open in [gaps.md](gaps.md). |
| **Projectiles** | Physical bullets use swept hitboxes, air/ground filters and pierce history; a cached target does not cause remote damage. Homing, mirrored volleys, scathe explosions and deterministic lightning exist, but full trajectories beyond the aim point, homing and nonphysical beam/rail paths still have approximations. Inspect [projectiles.rs](src/network/combat/projectiles.rs) before claiming parity. |
| **Logic and status** | `ucontrol` commands use a central 600-tick control lease. Status multipliers and reactions are implemented in [status.rs](src/game/status.rs), with values in [status_effects.tsv](src/game/status_effects.tsv). Logic `set` accepts a single value token, matching the parser behaviour recorded by the 159.7 probes; it is not an expression assembler. |
| **Liquids** | Authoritative puddle and conduit flow is implemented, including reinforced bridge conduit pressure flow. Implementation: [liquids.rs](src/network/economy/liquids.rs). |

## Gaps — Erekir

Erekir is **not** at Serpulo level. `.msav` loading is not planet-specific; loading a map does not establish full campaign support.

| Area | Coverage and remaining limits |
|---|---|
| **Ducts, heat, beam drills and crafters** | Domain simulation in [erekir.rs](src/network/economy/erekir.rs). Erekir turret support remains partial. |
| **Assemblers, fabricators and refabricators** | Production includes resource consumption, unit-cap/output gating and upgrade recipes. Assemblers consume payload stacks and cyanogen; when stacks are absent they still use equivalent core items as a drone-ferry proxy. Drones do not transport the stacks. |
| **Units** | Sync layouts cover vanilla IDs, including missile lifetime fields. Follow/formation AI remains simplified despite implemented repair and missile-expiry behaviour. |
| **Payloads, displays and canvas** | Payload transport and canvas data have sync implementations. Logic-display sync follows the official headless behaviour, which discards draw graphics server-side. These codecs do not establish campaign-wide parity. |

Unit production is implemented, but the remaining simulation and AI limits mean this alpha is still not a replacement for Java on a full Erekir campaign.

## Benchmark vs Java 159.7

From `tools/bench/bench_workload_suite.sh` on **2026-08-21**, maze survival @ 60 TPS, 25 s + 5 s warmup. RSS below is the recorded average. Full numbers and caveats: [benchmark.md](benchmark.md).

| Players | RSS avg Rust / Java | Join avg Rust / Java | Simulation |
|---:|---|---|---|
| 10 | **15 MB** / 280 MB | **6.6 ms** / 30 ms | both ~60 UPS/FPS |
| 100 | **23.4 MB** / 347.4 MB | **4.2 ms** / 12 ms | Rust 60 UPS; Java ~57 FPS |
| 500 | **65.2 MB** / 430.9 MB | **20 ms** / 120 ms | Rust ~33 UPS; **Java 1 FPS** |

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

Use the [validation matrix](CONTRIBUTING.md#validation) for the changed surface. It contains the Rust commands, module/concurrency guards, and fixture requirements. Documentation-only edits have their own checks; CI gates remain mandatory.

For current-target JAR setup and runnable 160.5 join/gameplay scenarios, see [the client harness guide](navigation.md#client-in-the-loop-harness). Historical script names are not proof of compatibility with older targets.

Differential parity using captured fixtures:

```bash
cargo test --lib parity -- --test-threads=1
```

To remove build artifacts, `bash tools/clean_artifacts.sh` deletes `target/` and compiled Java classes under `tools/`; preserve any diagnostic outputs you need first.

## Tools

```
tools/
  smoke/         ArcNet smokes + E2E + VerifyProtocol (Java + bash + world JSON)
  inspect/       JAR manifest extractors (Inspect*.java, export_*.py)
  parity/        tests/parity/probes runner → fixtures
  bench/         BenchLoadN + Rust vs Java 159.7 suite
  archguard/     module dependency guard
  dashmap_guard/ DashMap analyzer DM001–DM005
  compatlib/     current-target ledger and manifests
```

CI gates (stay at `tools/` root): `architecture_guard.sh`, `compat_gate.sh`, `compat_jar_gate.sh`, `low_core_test.py`, `mindustry_manifest.py`.

Content manifests (tabular authority, not markdown): `src/game/*.tsv`.

## Out of scope

Mods, plugins, RequestAssets / asset streams, full interactive Rules editing (`SetRules` inbound), protocol fuzzing, and exact vanilla of every Erekir block.

Architecture and wire contract: [ARCHITECTURE.md](ARCHITECTURE.md).

## Contributing

Human PRs: [CONTRIBUTING.md](CONTRIBUTING.md). If you use a coding agent on the change, it must follow [AGENTS.md](AGENTS.md).

---

# Español

**Oxide** es un port experimental del servidor de Mindustry: handshake ArcNet, world stream, simulación de tick, economía, combate y consola/TUI. **No es el servidor oficial** y no está afiliado a Anuken.

## Qué es (y qué no)

**Es** un servidor autoritativo: el cliente predice; Rust valida y avanza el estado que luego se replica (`EntitySnapshot`, `BlockSnapshot`, RPC). Objetivo de esta alpha: **hosteable en producción experimental** para partidas vanilla Serpulo (survival / sandbox / pvp / attack) con clientes oficiales 160.5. No es paridad conductual byte-a-byte de todo vanilla.

**No es**

- un drop-in 100 % del `server-release.jar` (campaña Erekir completa, IA exacta, mods…)
- compatible con **mods, plugins, scripts, packets custom o event bus** — los clientes con mods se rechazan. Es una decisión explícita del producto: zip/jar/hjson/scripts requieren un cambio de producto dedicado.
- un runtime multi-build: un binario, un target (`160.5`)

La compatibilidad wire/save se comprueba con los manifiestos 160.5 fijados, fixtures Rust y probes del JAR objetivo. No hay un gate de certificación por SHA del runtime; pasar esas pruebas **no** significa que todo el juego Java esté portado.

## Estado de la alpha

Jugable hoy (Serpulo): TCP+UDP, LAN, conexión y world stream; movimiento, construcción/deconstrucción temporizada, minería, chat y ping; oleadas, torretas principales y proyectiles autoritativos; transporte, energía, líquidos, fábricas y reconstructores; Logic `ubind`/`ucontrol`; TUI de operador (`--tui` / `--no-tui`, fuera de paridad vanilla). El objetivo actual es **160.5**; los artefactos de smokes anteriores son evidencia histórica.

Las tablas resumen cobertura y limitaciones del producto. Las divergencias concretas, sus anclas de código y la evidencia necesaria para cerrarlas están en [gaps.md](gaps.md). Contenido implementado no implica paridad exacta. La auditoría de Serpulo
conserva contratos pendientes y evidencia de cierre incompleta; consulta
[gaps.md](gaps.md). No certifica sandbox ni survival sin bugs.

## Huecos — Serpulo

| Área | Cobertura y límites pendientes |
|---|---|
| **Unidades e IA** | Los IDs vanilla tienen especificaciones y layouts de sync; se simulan la vida de misiles, entregas de carga y reparación constructora. Las órdenes de reparación de Poly/Mega apuntan a edificios aliados dañados y curan al impactar; las armas en reposo dejan de disparar. Los drones de assembler llegan al perímetro pero no transportan stacks de payload. Escuadras y seguimiento siguen simplificados; faltan los planes de base de `BaseBuilderAI` y la minería/priorización según requisitos. |
| **Producción de unidades** | Fábricas y reconstructores conservan payload/progreso entre snapshots y pausas; el consumo consulta el payload real y los costes del equipo. Las recetas y capacidades de entrada usan la [tabla de producción](src/game/unit_production.tsv) comprobada contra el JAR, incluido grafito para el aditivo y metacristal para el multiplicativo y Risso. Las unidades transportadas cuentan para el límite de unidades. La deselección explícita de fábrica se conserva en sync. Siguen pendientes las matrices completas de líquidos de niveles superiores y guardado/rally con payload retenido en [gaps.md](gaps.md). |
| **Proyectiles** | Los proyectiles físicos usan barrido de hitboxes, filtros aire/suelo e historial de penetración; el objetivo guardado no causa daño remoto. Existen homing, volleys espejados, explosiones scathe y rayos deterministas, pero siguen aproximados el recorrido más allá del punto apuntado, el homing y las rutas no físicas de haces/rail. Consulta [projectiles.rs](src/network/combat/projectiles.rs) antes de afirmar paridad. |
| **Logic y estados** | Los comandos `ucontrol` usan una concesión central de control de 600 ticks. Los multiplicadores y reacciones están en [status.rs](src/game/status.rs), con valores en [status_effects.tsv](src/game/status_effects.tsv). `set` acepta un único token de valor, como el parser documentado por los probes 159.7; no interpreta expresiones. |
| **Líquidos** | Hay simulación autoritativa de charcos y conductos, incluido el flujo por presión del reinforced bridge conduit. Implementación: [liquids.rs](src/network/economy/liquids.rs). |

## Huecos — Erekir

Erekir **no** está al nivel de Serpulo. La carga de `.msav` no depende del planeta; cargar un mapa no demuestra soporte de campaña completa.

| Área | Cobertura y límites pendientes |
|---|---|
| **Ductos, calor, beam drills y crafters** | Simulación en [erekir.rs](src/network/economy/erekir.rs). Las torretas Erekir siguen parcialmente soportadas. |
| **Assemblers, fabricators y refabricators** | La producción incluye consumo de recursos, límite de unidades/salida libre y recetas de mejora. Los assemblers consumen stacks de payload y cyanogen; sin stacks usan ítems equivalentes del core como sustituto del transporte por drones. Los drones no transportan esos stacks. |
| **Unidades** | Los layouts de sync cubren IDs vanilla, incluidos los campos de vida de misiles. La IA de seguimiento/formación sigue simplificada, aunque se simulan reparación y expiración de misiles. |
| **Payloads, displays y canvas** | El transporte de payloads y los datos de canvas tienen sync. Logic-display sigue el comportamiento headless oficial, que descarta los gráficos de draw en el servidor. Estos codecs no establecen paridad de campaña. |

La producción de unidades está implementada, pero los límites de simulación e IA hacen que esta alpha todavía no sustituya a Java en una campaña Erekir completa.

## Benchmark vs Java 159.7

Suite `tools/bench/bench_workload_suite.sh` el **2026-08-21**, maze survival @ 60 TPS, 25 s + 5 s warmup. RSS indica el promedio registrado. Detalle en [benchmark.md](benchmark.md) *(inglés)*.

| Jugadores | RSS avg Rust / Java | Join avg Rust / Java | Simulación |
|---:|---|---|---|
| 10 | **15 MB** / 280 MB | **6.6 ms** / 30 ms | ambos ~60 UPS/FPS |
| 100 | **23.4 MB** / 347.4 MB | **4.2 ms** / 12 ms | Rust 60 UPS; Java ~57 FPS |
| 500 | **65.2 MB** / 430.9 MB | **20 ms** / 120 ms | Rust ~33 UPS; **Java 1 FPS** |

Rust mantiene construct/deconstruct y oleadas (pico ~120–180 enemigos). Java acepta el mismo TX; el harness **no** ve `ConstructFinish`/`UnitDeath` (`rx = 0`): hueco de observabilidad, no prueba de que ignore los planes. La presión de enemigos tampoco es idéntica (`spawn`+`waves` vs `runwave`).

## Ejecutar / verificar / tools

Ver la sección [English — Run](#run) más arriba: mismos comandos.

Fuera de alcance: mods, plugins, RequestAssets, `SetRules` inbound completo, fuzz de protocolo, exact vanilla de cada bloque Erekir.

## Contribuir

PRs: [CONTRIBUTING.md](CONTRIBUTING.md). Si el cambio lo hace un agente de código, tiene que seguir [AGENTS.md](AGENTS.md).
