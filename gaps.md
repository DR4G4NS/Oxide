# Gaps

Concrete, located divergences from Mindustry v8 **159.7** that are still open in
this tree. This is **not** the product-level gap list — that lives in
[README.md](README.md) (§ Gaps — Serpulo / Gaps — Erekir) and summarizes *content coverage and
product limitations*. This file describes *behaviour that is ported but wrong*,
with the exact vanilla anchor and the exact Oxide anchor, so the next agent can
fix one item without re-deriving the investigation.

Navigation, JAR tooling, and the client-in-the-loop harness recipe:
[navigation.md](navigation.md).

## Ground rules

- Keep each fix focused on the requested divergence; combine gaps only when the task explicitly requires it.
- **Verify against the JAR, not against this file.** Line numbers drift; the
  named method is the stable anchor. If the anchor moved, fix the anchor here in
  the same change.
- Every item lists the **test that must exist** before it is closed. A gap
  closed without that test is not closed. If the client predicts the block, the
  test must span **> 360** game ticks and cross a `BlockSnapshot`
  ([ARCHITECTURE.md](ARCHITECTURE.md) § Protocol contract, rule 15).
- When you close a gap, **delete its section** and, if the behaviour is still
  short of vanilla in a smaller way, replace it with the narrower statement. Do
  not leave a changelog behind — that is what git is for.
- `P0` = actively wrong on the wire or contradicts a documented contract.
  `P1` = observable by a player. `P2` = exactness, not yet observed to matter.

Vanilla source paths are relative to an external Mindustry checkout. The target
identity is pinned in [compat/current.toml](compat/current.toml); see
[navigation.md](navigation.md#compatibility-evidence) for verifying the JAR and
using source as a reference. Java is never copied into `src/`.

## P2 — in-progress ConstructBlock deconstruct is instant

Vanilla `ConstructBuild.deconstruct` (159.7 javap) subtracts build-speed from
`progress` and only `Call.deconstructFinish` when
`progress <= current.deconstructThreshold` (Block default `0f`) or
`state.rules.infiniteResources`. A half-built conveyor therefore takes about
half a break to come off.

Oxide now inserts a ConstructBlock tile for timed places, but a breaking
ClientSnapshot plan still aborts that pending immediately
(`abort_pending_construct` in `src/network/buildings/construction.rs`) so a
**stuck** unpaid plan (client progress stays 0 because `checkRequired` failed)
matches the vanilla threshold-0 path. A construct that had already advanced
past 0 is therefore faster to cancel than 159.7.

**Close when:** a timed survival construct with `0 < progress < 1` takes
`progress * buildTime / alphaBuildSpeed` ticks to `DeconstructFinish`, with a
test spanning **> 360** game ticks.

## P2 — unit plan snapshot always encodes pending constructs as place plans

Vanilla `NetServer` copies `ClientSnapshot` plans onto `player.unit().plans`
(`TypeIO.writePlansQueueNet`). A breaking plan stays `breaking=true`.

Oxide `player_builder_plans` (`src/network/wire/encode.rs`) synthesizes the
queue from `pending_builds` (always `breaking: false`) and `pending_breaks`.
While the player is deconstructing a ConstructBlock, the server copy still
looks like a place plan until `abort_pending_construct` drops it.
`@SyncLocal` discards that queue for the controlling client; it can still
wipe or rewind a non-local observer's beam for one snapshot.

**Close when:** `player_builder_plans` prefers the live ClientSnapshot /
`player.active_plans` breaking bit over `pending_builds`, with a fixture that
encodes `breaking=1` for an in-progress construct the actor is demolishing.

## Remaining Serpulo behaviour and validation gaps

These limitations do not imply that the corresponding implemented paths have
full vanilla parity. The target oracle remains the pinned 159.7 JAR.

| Area and owner | Remaining behaviour or required evidence |
|---|---|
| Projectiles — `src/network/combat/projectiles.rs` | Full trajectories beyond the aim point, homing, and nonphysical beam/rail impacts remain approximate. Close with matching target-JAR trajectories and collision scenarios. |
| Navigation — `src/network/combat/enemy.rs` | Compare the first pathfinding step with Java Pathfinder on a live `Vars.world`; coordinate conversion alone does not validate the path. |
| Weapons — `src/network/simulation/units.rs`, `waves.rs` | Verify per-mount alternation, phase, rotation and delays against the JAR, plus firing while mining/building across the order matrix. |
| Unit death — `src/network/combat/damage.rs` | Stack flammability, fire, carried charge, passengers and drowning still need the complete vanilla lifecycle. |
| Factory configuration — `src/network/wire/tile_config.rs` | Banned-plan deselection needs persistence and observation beyond 360 ticks and a BlockSnapshot. |
| Payload production — `src/network/economy/factories.rs`, `payload.rs` | Complete retained-payload, rally and MSAV cycles, JAR rereading of `UnitPayload.dump`, and fractional liquid-supply/input traces beyond 360 ticks remain unverified. |
| Possession — `src/network/wire/unit_control.rs` | Verify replacing LogicAI with player possession through a real client RPC. |
| Support units — `src/network/units/mining.rs`, `simulation/units.rs` | Passive abilities under every controller and ore-selection behaviour still need the full matrix. |
| Live rules — `src/network/units/rules.rs`, `wire/bootstrap.rs` | Complete NetworkIO world loading and the live-session rules matrix remain unverified; typed Rules decoding and TCP join/rejoin cover narrower contracts. |
