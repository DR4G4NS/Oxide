# Navigation

Find the code and evidence for one behaviour. Use the section that matches the
question; the [architecture contract](ARCHITECTURE.md) and
[validation matrix](CONTRIBUTING.md#validation) own the implementation rules and
completion checks. Known divergences live in [gaps.md](gaps.md).

| Starting point | Go to |
|---|---|
| Packet name, action, or snapshot field | [Follow a packet](#follow-a-packet) |
| Block name, class, or balance value | [Find the owning domain](#find-the-owning-domain) |
| Uncertain ID, field width, or vanilla behaviour | [Compatibility evidence](#compatibility-evidence) |
| Rust state looks correct but the client rolls back | [Client prediction](#client-prediction) |
| Reproduce against the official client | [Client harness](#client-in-the-loop-harness) |
| Joining client receives different rules | [Streamed rules](#streamed-rules) |

## Compatibility evidence

The target identity and SHA-256 are in [compat/current.toml](compat/current.toml).
Use these sources in order:

1. **The verified target JAR** is authoritative for 159.7 behaviour and wire
   layout. If bytecode and documentation disagree, update the stale document.
2. **[compat/159.7/](compat/159.7/)** contains extracted facts. Query existing
   manifests before repeating an extraction: `packets.json` (IDs/classes),
   `rpc.json` (`source_remote`), `typeio.json`, `content.json`,
   `entity-sync.json`, `streams.json`, `saves.json`, `rules.json`, `logic.json`.
3. **An external Mindustry source checkout** explains control flow. Compare its
   revision with `source_commit` in `compat/current.toml`; a checkout of master
   may differ from 159.7. Confirm layout and constants against the target JAR.
   Java remains an external reference and is never copied into `src/`.

Set the JAR path explicitly rather than relying on a script's local default:

```bash
export MINDUSTRY_1597_JAR=/absolute/path/to/159.7.jar
test -f "$MINDUSTRY_1597_JAR"
sha256sum "$MINDUSTRY_1597_JAR"
rg -n 'jar_sha256' compat/current.toml
```

The digest must match **`[target].jar_sha256`**, not the historical entry.
For a full identity/manifest/probe check (broader than one gameplay smoke):

```bash
MINDUSTRY_CURRENT_JAR="$MINDUSTRY_1597_JAR" bash tools/compat_jar_gate.sh
```

When a fact is not already extracted:

```bash
# Declared fields and serialization bytecode for a generated packet.
javap -c -p -cp "$MINDUSTRY_1597_JAR" mindustry.gen.UnitBlockSpawnCallPacket

# Quote nested class names so the shell does not expand the dollar sign.
javap -c -p -cp "$MINDUSTRY_1597_JAR" \
  'mindustry.world.blocks.units.Reconstructor$ReconstructorBuild'

unzip -l "$MINDUSTRY_1597_JAR" | rg -i 'reconstructor'
```

A field listing alone does not establish serialization order; inspect the
relevant `write`/`read`/TypeIO bytecode. Reusable extractors live in
[tools/inspect/](tools/inspect/) and [tools/compatlib/](tools/compatlib/).

## Follow a packet

For example, `BeginBreak` is an **outbound event**; a player's break request
arrives as a plan inside `ClientSnapshot`.

| Step | Search or owner |
|---|---|
| Name → ID/direction | `rg -n -C1 'BeginBreakCallPacket' compat/159.7/packets.json`; consult `rpc.json` for the source remote method |
| ID → named constant | `rg -n 'BEGIN_BREAK_PACKET_ID' src/network/protocol.rs` |
| Constant → codec/callers | `rg -n 'BEGIN_BREAK_PACKET_ID' src/network/` |
| Incoming build/break action | `rg -n -e CLIENT_SNAPSHOT_PACKET_ID -e apply_build_plans src/network/session/mod.rs` |
| Action → mutation | `apply_build_plans` in [construction.rs](src/network/buildings/construction.rs); actor checks include `actor_action_allowed` |
| Mutation → state | [DynamicWorld/DynamicTile](src/network/world.rs) and the [legacy-field table](ARCHITECTURE.md#legacy-dynamictile-fields) |
| State → replication/persistence | [building snapshots](src/network/buildings/snapshot.rs), [wire codecs](src/network/wire/), [save codec](src/engine/save_io.rs), and [runtime persistence](src/network/wire/persistence.rs) |

Trace the relevant field through sync and the applicable save format; transient
state need not persist, but that decision must match the format's contract.
Search by symbol rather than a stored line number. Starting from a client
symptom, trace backward from the field the client reads to its server writer.

## Find the owning domain

For `additiveReconstructor`:

```bash
rg -n 'additive-reconstructor' compat/159.7/content.json src/game/block_names.tsv
rg -n '^380\b' src/game/*.tsv
rg -n '\b380\b' src/network/ --glob '*.rs'
```

Use the [module map](ARCHITECTURE.md#modules) to choose where the change belongs.
Inspect the relevant methods and inherited behaviour: `Reconstructor` extends
`UnitBlock`, then `PayloadBlock`, then `Block`. Payload movement and sync
prefixes can live in parents; reading every class in full is unnecessary.

For optional source navigation, set `MINDUSTRY_SOURCE` to the external checkout:

```bash
rg -n 'class Reconstructor|extends' \
  "$MINDUSTRY_SOURCE/core/src/mindustry/world/blocks/units/Reconstructor.java"
```

Balance/content inputs live in `src/game/*.tsv`. Find the table's Rust consumer
and any exporter in `tools/inspect/`; not every table uses the same generation
path. Preserve the JAR-derived input as the source of numeric facts.

## Client prediction

Rust tests prove the properties they exercise. They do not by themselves prove
that the official client's prediction converges. Useful symptom → cause pairs:

| Symptom | Inspect |
|---|---|
| Client predicts different costs, build speed, or wave timing | Simulation `WaveRules` versus the Rules in the world stream |
| Progress rolls back at a 6 s `BlockSnapshot` | Missing multipliers or different progress rates |
| Released payload or cancelled plan lingers | Missing prompt RPC or wrong event order |
| Client keeps steering a removed unit | Removal from all relevant state collections |
| Instant break/build fails on the client | Begin/finish event ordering and client-side building lifecycle |

These causes can also have Rust regression tests. A real-client scenario adds
evidence about decoding, event application, and prediction. For predicted block
changes, keep the **> 360 game ticks across a `BlockSnapshot`** regression; an
on-demand snapshot alone does not exercise the normal cadence.

## Client-in-the-loop harness

### Run an existing scenario

After [verifying the JAR](#compatibility-evidence), choose the scenario:

| Scenario | Command | Client implementation |
|---|---|---|
| Join | `bash tools/smoke/smoke_join_1597.sh 6597` | [SmokeJoin1597.java](tools/smoke/SmokeJoin1597.java) |
| Sandbox rules and build/break events | `bash tools/smoke/smoke_sandbox_build_1597.sh 6598` | [SmokeSandboxBuild1597.java](tools/smoke/SmokeSandboxBuild1597.java) |
| Factory/reconstructor payload and progress | `bash tools/smoke/smoke_unit_payload_1597.sh 6599` | [SmokeUnitPayload1597.java](tools/smoke/SmokeUnitPayload1597.java) |
| Survival factory → conveyor → reconstructor → released Mace, then deselect across snapshots | `OXIDE_SMOKE_SCENARIO=survival-cycle bash tools/smoke/smoke_unit_payload_1597.sh 6600` | [SmokeUnitPayload1597.java](tools/smoke/SmokeUnitPayload1597.java) |
| Survival Mono → Poly → Mega, client unit count and plastanium withdrawal | `OXIDE_SMOKE_SCENARIO=survival-air-cycle bash tools/smoke/smoke_unit_payload_1597.sh 6601` | [SmokeUnitPayload1597.java](tools/smoke/SmokeUnitPayload1597.java) |

The scripts default to a release build; set `OXIDE_SMOKE_PROFILE=debug` to
exercise a local debug build during iteration.

The production smoke also accepts `OXIDE_SMOKE_MAP_FILE=/absolute/path/to/map.msav`.
It places its finite-input production fixture in a separate trial save under
`target/`, preserving the supplied map and existing player saves. The air cycle
uses graphite for the additive reconstructor and metaglass for the multiplicative
reconstructor. Both survival cycles fill a plastanium line from a multiblock drill,
inspect its end, withdraw three of its ten items, and require seven to remain
across snapshots. The released unit must appear once in the official client's
team count. `CheckSupportSnapshots.java`, run by `tools/compat_jar_probes.py`,
also reads complete support-unit/belt snapshots with the target JAR and invokes
the belt's actual draw method across 800 client ticks to detect invisible stacks.
On a map with wave spawns, set `OXIDE_SMOKE_WAVES=1` to start a wave during
production and require both enemy entities and a positive enemy counter on the client.

These scripts compile the client, build Oxide, create a trial save under
`target/`, start the server, assert outcomes, and clean up their server PID.
The `survival-cycle` scenario starts with no units and finite factory inputs,
uses solar power, observes more than 360 reconstructor progress ticks, requires
an upgraded Mace in entity snapshots, and keeps a factory deselected for more
than one 6-second snapshot interval. Its marker is `SMOKE_OK survival-production`.
The join smoke also reconnects and deserializes the streamed Rules with
`JsonIO.read(Rules.class, ...)`. To check a particular map in either mode:

```bash
OXIDE_SMOKE_MAP_FILE=/absolute/path/to/map.msav OXIDE_SMOKE_MODE=survival \
  bash tools/smoke/smoke_join_1597.sh 6597
```

Use `OXIDE_SMOKE_MODE=sandbox` for the second mode. With a custom map this
checks Rules decoding and join/rejoin traffic only; the build/combat scenario
requires the default fixture's coordinates. None of these probes loads the
entire world through `NetworkIO.loadWorld`.

Use an available port for TCP and UDP. Historical `*_158` filenames sometimes
supply fixtures or reusable clients; choose the JAR/build explicitly, since
those names are not a second runtime compatibility target.

The sandbox and payload scripts return exit zero with `skip` if their JAR is
missing. Report that as **not run**, not passed. Success requires the server's
`finished loading the world` assertion and, for gameplay scripts, the client
marker `SMOKE_OK sandbox-build` or `SMOKE_OK unit-payload`.

### Extend a scenario

Start from the closest committed harness instead of maintaining a second Java
bootstrap in this document. In `SmokeUnitPayload1597`, `main`,
`allocateWithoutConstructor`, `clientNet`, `readSnapshot`, and
`readStreamedRules` show the relevant setup and decoding. Locate them with:

```bash
rg -n 'main\(|allocateWithoutConstructor|clientNet|readSnapshot|readStreamedRules' \
  tools/smoke/SmokeUnitPayload1597.java
```

Preserve these non-obvious boundaries when adapting it:

- `handled()` decodes deferred packet fields; `handleClient()` applies an event
  to the client world. Use event application when observing that world, or
  explicitly inspect decoded fields as the committed probes do. Do not swallow
  an exception for the event being tested and then claim success.
- Pre-place observed buildings for scenarios that do not load the whole world;
  `readSync` cannot update a local building that does not exist.
- For the partial sandbox/payload probes, accumulate `StreamBegin`/`StreamChunk`
  to the declared total, then send `ConnectConfirmCallPacket`. Use their
  `readStreamedRules` helper; they do not bootstrap a full `NetworkIO.readWorld`.
- Send official `mindustry.gen.*` packet classes without subclassing. Unit
  commands use `posTarget` and `finalBatch`; build plans use `ClientSnapshot`.
- Assert the observation with a bounded timeout and preserve useful logs. An
  exit-zero run that only prints values does not prove the scenario.

`RequestBlockSnapshotCallPacket` can probe a tile immediately to distinguish
wrong state from delayed delivery. Pack its position as `(tileX << 16) | tileY`
and inspect the reply with the target building's `readSync`. For timing bugs,
also observe the normal snapshot cadence and log time since connection.

Manual trials need a distinct save per map under `target/`; a save with
`game_over=true` is not a clean trial. A map-name mismatch can prevent startup.
Keep the PID of the server you launch and stop that PID during cleanup. Place
blocks on buildable tiles: a non-replaceable wall/floor rejection is not evidence
of a placement bug.

## Streamed rules

Compare `world.wave_rules` with the Rules bytes sent at join when changing a
rule that affects the client. The two are patched through different paths.

- [world_stream.rs](src/engine/world_stream.rs): `inspect_metadata` reads the
  compressed stream; `replace_rules` patches its Rules document.
- [rules.rs](src/network/units/rules.rs): `arc_json_to_strict` allows parsing Arc
  JSON with `serde_json`; `patch_rules_json` preserves unrelated content when
  applying overrides.
- [SmokeUnitPayload1597.java](tools/smoke/SmokeUnitPayload1597.java):
  `readStreamedRules` inflates the stream, checks the 159.7 data-patch prefix,
  skips its 8-byte header, and reads the Rules with Java `readUTF` (modified
  UTF-8, not a plain UTF-8 string).

## Tool troubleshooting

Only if `cargo` fails with `error: unknown proxy name: '…'`, force the rustup
shim's `argv[0]` in the current Bash session:

```bash
cargo() { (exec -a cargo "$HOME/.cargo/bin/cargo" "$@"); }
```
