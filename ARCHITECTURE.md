# Architecture

Where gameplay changes belong, and which invariants are non-negotiable. The **compatibility target** is Mindustry desktop **160.5** (`compat/current.toml`). Older smoke/codec artifacts are historical evidence; a 160.5 layout is never inferred from earlier source order.

Read the sections relevant to the changed behaviour. For finding code or probing the target JAR, use [navigation.md](navigation.md); for local checks, use [CONTRIBUTING.md](CONTRIBUTING.md#validation).

## Authoritative flow

```text
client packet
  → listener/session: bounded decode, authenticate, validate action
  → buildings/config: TypeIO → typed value
  → buildings/placement: shared created/placed lifecycle
  → domain (power, sandbox, reactor, economy, erekir, combat, units)
  → DynamicWorld: single authoritative state
  → writeSync / RPC codec: adapt state to the official client
```

The Mindustry client predicts buildings between snapshots. Every resource, link, progress field, and `Building.writeSync` animation field must exist and advance in `DynamicWorld`. Raising snapshot frequency does **not** fix missing simulation.

## Modules

| Surface | Responsibility |
|---|---|
| `network/listener` | Framing, session, actor authority, tick, emit. Not per-block rules. |
| `network/wire` | Encode/decode. Must not import listener. |
| `network/buildings/config` | TypeIO boundary. `DynamicTile.config` is a **complete object** (tag + payload). |
| `network/buildings/placement` | `after_placement` once: player, unit, `ucontrol build`. |
| `network/buildings/power` | PowerNode/PowerSource specs, `power_links`; range ≠ live graph. |
| `network/buildings/sandbox` | Item/liquid sources and voids on the server. |
| `network/buildings/reactor` | Pure transition; tick emits `ReactorOverheat`. |
| `network/economy` | Serpulo orchestration + `economy/erekir`. |
| `network/simulation` | World-loop phases (logic/waves/units/payloads/power). |
| `network/session` | Connections, replay, unknown packets. |
| `tui/` | Operator UI; **out of** vanilla parity. |

Domains must not import listener, runtime, console, or TUI (**ARCH001–ARCH002**). Emit through `&dyn FrameEmit`, not connection maps or outbound helpers (**ARCH003–ARCH006**).

Valid handler: decode with limits → authenticate → delegate **without** holding a `DashMap::Ref` → persist/broadcast after a valid mutation.

## Protocol contract

1. A packet has a fixed ID, direction, variant, priority, and transport. Do not reuse an S→C layout for C→S.
2. IDs come from the target JAR registry (`compat/<build>/packets.json`). Never from local enum order.
3. Field order and width are confirmed with `javap` + JAR `TypeIO`. `byte`/`short`/`int` are not interchangeable.
4. Each `TypeIO.writeObject` is one known tag + payload. Never concatenate `extra_data` or Building tails.
5. Block config, `Building.write` tail, `writeSync`, and `BuildPlan` config are separate domains.
6. Inbound decoders limit before allocate, validate IDs/counts/finite floats, and consume the payload exactly. Do not mutate the world mid-decode.
7. Player/Unit/Building references use the exact TypeIO codec, not bare IDs.
8. `reliable`/`unreliable` includes channel, ordering, loss, coalescing, and fallback.
9. Unknown packets are diagnosed (id, peer, phase, length). **This server logs a warning and keeps the session** (official 158.1 closes). Documented operational deviation.
10. A baseline change needs an explicit version plus new JAR, hash, registry, and fixtures.
11. `GameMode` is applied **after** map rules; the world-stream `Rules` JSON must match simulation.
12. Interpolated `writeSync` fields (drill `progress`/`warmup`, conveyor items) are gameplay state.
13. Cadence: entity snapshots ~50 ms; `blockSyncTime` = **6 s**. Do not speed snapshots to hide rollback.
14. A loaded building is simulated on one authoritative path only.
15. Client prediction is not authority. New or changed predicted block behaviour needs a test spanning **> 360** game ticks and crossing a `BlockSnapshot`.
16. Domain services never treat TypeIO tag 5/14 as a raw ID.
17. Every construction path runs the post-placement hook.
18. Reactor heat/warmup must not appear as cryofluid in `LiquidModule`.

Sandbox preset (target bytecode): `infiniteResources`, `allowEditRules`, `waves=true`, `waveTimer=false`. Builds complete immediately when `infiniteResources` is set.

Conveyor: Rust keeps FIFO front at index 0; the official client treats `ids[len-1]` as front — reverse on serialize.

NuclearReactor (JAR): 30 thorium / 30 liquid; one thorium every 360 ticks; heat `fullness * 0.02`; cool 0.5 per cryo; overheat `heat >= 0.999`; explosion radius 19 tiles, damage 5000.

## Legacy `DynamicTile` fields

Until subclass-typed state exists:

| Domain | Field | Meaning |
|---|---|---|
| NuclearReactor 315 | `production_progress` | fuel timer |
| NuclearReactor 315 | `output_liquid_amount` | heat; never cryofluid |
| ImpactReactor 316 | `output_liquid_amount` | warmup |
| ItemSource 412 | `transport_progress` / `unloader_offset` | emit counter / round-robin |
| HeatSource 418 | `mass_driver_rotation` | heat output |
| Door 228/229 / AutoDoor 239 | `door_open` | `Building.checkSolid() == !open`; tap-toggle (228/229) and AutoDoor proximity (239) |
| AutoDoor 239 | `production_progress` | `checkInterval` (20 tick) scan accumulator |
| Turret (logic control, H18) | `logic_control` | `Some((aim_x, aim_y, shooting, unit_id))`: `control shoot/shootp` aim state; `None` = automatic targeting |
| Conveyor | `conveyor_items` | FIFO `(item, progress)`, front at index 0 |
| UnitFactory 377-379 / 386-388, Reconstructor 380-383 / 389-392 | `payload` | held `UnitPayload` (incoming or completed); released only after `moveOutPayload` |
| UnitFactory / Reconstructor | `payload_accum` | official `payVector.x`, `payVector.y` (length 2); empty means the origin |
| UnitFactory / Reconstructor | `payload_rotation` | official `payRotation` (degrees) |
| Reconstructor | `stored_amount` | legacy held-unit type + 1 marker; never authoritative for consumption or snapshot efficiency (use `payload`) |
| ConstructBlock 5..=20 | `production_progress` | `ConstructBuild.progress` (0..=1) |
| ConstructBlock 5..=20 | `stored_item` | `ConstructBuild.previous.id` (air = 0) |
| ConstructBlock 5..=20 | `stored_amount` | `ConstructBuild.current.id` |
| ConstructBlock 5..=20 | `payload_accum` | triples of accumulator / totalAccumulator / itemsLeft |

## Recipe for porting a building

1. Pin class, ID, and revision on the target JAR.
2. Extract `updateTile`, accept/handle, config, and `write`/`read`; confirm wire with the JAR.
3. Put state and transition in the domain module. Do not reuse a field without updating the table above.
4. Integrate once in the scheduler and the accept boundary.
5. Add a byte-exact `writeSync` codec and a full TypeIO config case.
6. Add negative tests and, if the client predicts the block, a case spanning **> 360** game ticks and crossing a `BlockSnapshot`.
7. Complete the [applicable validation](CONTRIBUTING.md#validation), including the matching JAR smoke.
8. Record residual product limitations in [README.md](README.md), and located divergences in [gaps.md](gaps.md).

## Concurrency

- Do not hold `DashMap::Ref`/`RefMut` while querying or mutating another tile.
- Snapshot keys, drop guards, then apply.
- Do not take `persistence_lock` from a tick path that already holds it.
- Resolve destructive events after the domain transition.
- A tile in both `base_buildings` and `tiles` is simulated once.

Enforced by `tools/dashmap_guard` (DM001–DM005) and `tools/architecture_guard.sh`.

## Explicit debt

- `DynamicTile.config` still shares a physical type with legacy tails.
- `listener` / `economy` remain large; extraction belongs in an explicit architecture task.
- StackConveyor: documented quirk on exactly-2-SC chains.
- Product gaps (Erekir, units, projectiles, AI): [README.md](README.md).

## Minimum evidence for a wire change

Exact payload fixture; decode with the JAR class and `handled()` when safe; framing test (packet ID, length, no trailing bytes); mutation cases (ally/enemy, range, replay); and the [Rust checks](CONTRIBUTING.md#validation). The label “byte-exact” requires the target JAR to have read the full fixture.
