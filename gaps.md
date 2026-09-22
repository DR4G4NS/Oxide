# Gaps

Concrete, located divergences from Mindustry v8 **160.5** that are still open in
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
full vanilla parity. The target oracle remains the pinned 160.5 JAR.

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

## Survival/sandbox closure target and evidence boundary

Implementation baseline for the located findings below:
[`release/0.2.0-slopmaxxing` at `74218a443cbe8554c7921ff933c8575258a3989c`](https://github.com/DR4G4NS/Oxide/tree/74218a443cbe8554c7921ff933c8575258a3989c).
Official reference: [`Anuken/Mindustry` at `c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c`](https://github.com/Anuken/Mindustry/tree/c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c), the commit of `v159.7` and of `compat/current.toml`.
The oracle JAR SHA-256 is
`ce1db5b06fe7326b9d0c1d99b1eb1667cf6f0bf97093293f6674ae294981ff05`.
Historical 158.1 names and comments are not current-target behavioural evidence.

**Evidence status:** the additional findings are a source-level comparison, not
an executed Rust/JAR/client certification. The Rust suite, JAR probes and
gameplay scenarios were not run for this review. Proposed scenario names below
are requirements, not claims that those tests already exist or pass. Existing
narrower checks are preserved. The pre-existing gap matrix above is not being
relabeled as newly discovered defects. Source-confirmed differences still need
runtime regressions; conditional concerns are identified explicitly.

The requested target is vanilla survival/sandbox with every production unit and
its reachable gameplay dependencies, including Erekir production systems. This
expands the README's current experimental Serpulo scope; it does not claim that
Erekir is already supported. Campaign inter-sector progression, mods, plugins,
server scripting and arbitrary asset streams remain outside this target.

### Production denominator

A starting denominator is **50 principal production units**:

| Family | Required unit outputs |
|---|---|
| Serpulo ground | Dagger, Mace, Fortress, Scepter, Reign |
| Serpulo support ground | Nova, Pulsar, Quasar, Vela, Corvus |
| Serpulo crawler/legs | Crawler, Atrax, Spiroct, Arkyid, Toxopid |
| Serpulo attack air | Flare, Horizon, Zenith, Antumbra, Eclipse |
| Serpulo support air | Mono, Poly, Mega, Quad, Oct |
| Serpulo attack naval | Risso, Minke, Bryde, Sei, Omura |
| Serpulo support naval | Retusa, Oxynoe, Cyerce, Aegires, Navanax |
| Erekir tanks | Stell, Locus, Precept; Vanquish, Conquer |
| Erekir mechs | Merui, Cleroi, Anthicus; Tecta, Collaris |
| Erekir ships | Elude, Avert, Obviate; Quell, Disrupt |

The last two units of each Erekir row are assembler outputs, not a continuation
through reconstructors. This inventory follows
[factory plans](src/game/unit_production.tsv), `reconstructor_upgrade` in
[factories](src/network/economy/factories.rs), and `assembler_plan` in
[Erekir](src/network/economy/erekir.rs). Re-extract the complete reachable graph
from the pinned JAR before using the count as a gate. Include core avatars,
cargo/assembly drones, missiles, fragment carriers and unit-spawning abilities
transitively. Classify Renale, Latum and internal block units by actual
production, wave, sandbox and protocol reachability, not as factory products.

A `FULL` row in [unit_inventory.tsv](src/game/unit_inventory.tsv), whose header
still identifies 158.1, is not a passed 159.7 contract. Its `spawnable` label is
not by itself proof of a functional rejection; trace the consumers.

## P1 F01 — assembler core fallback bypasses required payloads

**Oxide:** `simulate_erekir_assemblers` and `assembler_proxy_items` in
[src/network/economy/erekir.rs](src/network/economy/erekir.rs). Missing payload
stacks can be replaced by core items. The base tank plan lists four Stell
payloads and ten large tungsten walls, but the fallback checks core beryllium
and silicon instead. This bypasses the actual unit/wall production chain.

**Official:** [UnitAssembler](https://github.com/Anuken/Mindustry/blob/c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c/core/src/mindustry/world/blocks/units/UnitAssembler.java),
`shouldConsume`, `moveInPayload`, `yeetPayload`, `spawned` and its consumers:
accepted payloads populate local stocks; core items do not replace those stocks.

The README's explanation as a missing **drone ferry** is misleading.
[AssemblerAI](https://github.com/Anuken/Mindustry/blob/c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c/core/src/mindustry/ai/types/AssemblerAI.java)
moves/orients drones around the perimeter; it does not ferry stacks. Do not
implement a fictitious ferry. Correct both README languages when removing the
proxy, preserving genuine residual limitations.

**Close when:** all six assembler outputs require the actual local payloads;
abundant core items alone cannot start production. Exact required stacks create
one unit and are consumed once. Cover incomplete/mixed stacks, conveyors,
module delivery, cancellation and restart. Probe the actual sandbox consumer
rules separately from finite-input survival.

## P1 F02 — assembly modules and output disagree on geometry

**Oxide:** `assembler_tier` uses adjacency to the assembler footprint, without
assembly-perimeter/facing validation. `assembler_output_occupied` checks
`8 * (13 + block_size) / 2` ahead, but completion calls `spawn_factory_unit` in
[factories](src/network/economy/factories.rs), which spawns only **20 world
units** ahead. For the size-5/area-13 geometry described by the helper, the
check is at 72: a 52-unit discrepancy. Occupancy also uses a fixed 24-unit circle.

**Official:** pinned `UnitAssemblerBuild.moduleFits`, `getUnitSpawn`,
`checkSolid` and `spawned` use assembly-area geometry and a consistent spawn
point, with class-sensitive occupancy checks. A compatible building underneath
may accept the new unit as a payload.

**Change:** use one assembly geometry service for module validation, drone
slots, occupancy and spawn/output; do not reuse the fixed-offset factory spawn.

**Close when:** all rotations; valid perimeter versus merely adjacent module;
wrong facing; module removal/re-addition; obstacles/units at the actual output;
compatible output acceptor. Compare tier, spawn coordinates, events and count
with Java. No output may appear at a different point than the one validated.

## P1 F03 — assembler progress omits drones and build-speed rules

**Oxide:** `simulate_erekir_assemblers` adds `delta_ticks * efficiency`, with no
positioned-drone factor and no `rules.unitBuildSpeed(team)` factor. Its local
cyanogen debit is clamped to available liquid without locally reducing progress
by that fraction. **Conditional concern:** probe the complete power/consumer
path before claiming an additional fractional-efficiency bug; an upstream gate
may already affect the supplied efficiency.

**Official:** `UnitAssemblerBuild.updateTile` adds normalized progress
`edelta() * rules.unitBuildSpeed(team) * eff / plan.time`. `eff` is the fraction
of drones satisfying `AssemblerAI.inPosition()`; both position and facing matter.
The lifecycle also handles tier-transition progress resets and tethered drones.

**Close when:** zero/partial/all positioned drones give the oracle progress;
drone destruction/facing changes, global/team build speeds, fractional power
and cyanogen, disabled state and tier transitions match. Run through the real
economy scheduler, not a helper with an injected power map. Compare debits,
progress and creation tick over >360 ticks. Do not apply efficiency twice.

## P1 F04 — Erekir liquid output routing selects multiblock interior cells

**Oxide:** the electrolyzer (200) and atmospheric-concentrator (201) branches in
`simulate_erekir_crafters` use one-tile `offset_position(key, rotation)` and skip
`snapshot.occupied`. Both have size 3 in
[block_sizes.tsv](src/game/block_sizes.tsv). With a complete footprint, all four
candidates are interior cells rather than external ports. The branches also
send outputs directly and discard unaccepted remainder instead of retaining it.

**Official:** [GenericCrafterBuild](https://github.com/Anuken/Mindustry/blob/c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c/core/src/mindustry/world/blocks/production/GenericCrafter.java)
retains each output via `handleLiquid`, applies fullness/backpressure, and calls
`dumpOutputs`/`dumpLiquid`.

**Change:** real footprint-edge/proximity routing and per-liquid inventories,
with explicit current-liquid semantics where applicable. Update economy,
save and sync together; do not reuse reactor heat or a single scalar as a
second liquid.

**Close when:** place actual 3x3 buildings through normal placement, feed finite
water and observe ozone/hydrogen or nitrogen through exterior ports. Test full
and disconnected outputs, reconnect, rotation, tiny input fractions and reload.
Fixtures with empty `occupied` must not hide the defect. Compare stored and
transferred quantities under the official discard/backpressure policy.

## P1 F05 — direct producer handoff misses reconstructor input-face checks

**Oxide:** `front_accepts_payload` in
[src/network/economy/factories.rs](src/network/economy/factories.rs) has no source
position parameter. Its caller `move_out_unit_payload` checks team/capacity/
upgrade but cannot reject delivery through the receiver's output face.

**Official:** [ReconstructorBuild.acceptPayload](https://github.com/Anuken/Mindustry/blob/c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c/core/src/mindustry/world/blocks/units/Reconstructor.java)
requires `relativeTo(source) != rotation`, plus payload/upgrade conditions. Its
`enabled || source == this` exception distinguishes self-entry from ordinary
building delivery.

**Change:** a source-aware acceptance operation shared by producer output,
conveyors and carried/self-entry paths, preserving their distinct exceptions.

**Close when:** facing outputs cannot feed into the reconstructor output side;
valid sides can. Cover every rotation, teams, bans, full/disabled receiver and
self-entry. A failed transfer retains exactly one original payload. Exercise
receiver removal between validation and commit at the real mutation boundary.

## P1 F06 — projectile motion disagrees with emitted angle and shot timing

**Oxide:** [projectiles.rs](src/network/combat/projectiles.rs),
`spawn_enemy_projectile`, `spawn_projectile_for_team`,
`simulate_ballistic_projectile`. Source-to-aim interpolation ends motion at the
aim distance. Enemy shot delay is added to flight duration; `angle_offset` is
sent in `CreateBullet` without rotating the authoritative endpoint. These are
separate early-termination, slow-flight-instead-of-delay and spread divergences.

**Official:** [BulletType.create](https://github.com/Anuken/Mindustry/blob/c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c/core/src/mindustry/entities/bullet/BulletType.java)
initializes velocity from angle and lifetime independently. Specific bullet
classes can scale life; this is not a universal aim-distance rule.
[Weapon.shoot](https://github.com/Anuken/Mindustry/blob/c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c/core/src/mindustry/type/Weapon.java)
schedules delayed creation, not a slower already-existing bullet.

**Change:** position/previous-position, velocity, age/lifetime and scheduled
shots; separate ballistic, instant line/rail, continuous beam and spawn-unit
behaviours. Extract required initialized metadata. Preserve swept collision,
air/ground filters and pierce history; do not return to remote cached-target damage.

**Close when:** an ordinary bullet hits beyond the original aim; spread rotates
both emitted and authoritative paths; delayed shots do not exist/collide early
and then move at the proper speed. Trace moving shooters/targets, collision
order, beams, shields, fragments and expiry, not only final HP.

## P1 F07 — direct spawn-unit launchers create missiles at impact

**Oxide:** `spawn_unit_bullet_payload`, `spawn_projectile_unit` and
`projectile_impact_effects` retain a surrogate launcher and create the missile
at impact. The insertion helper receives no shooter reference. Direct mappings
include Anthicus 92 -> 46, Disrupt 106 -> 55 and Scathe 186/189/192 -> 64/65/66.

**Official:** `BulletType.create` with `spawnUnit` creates the unit immediately
at launch on the server, sets appropriate velocity and `MissileAI.shooter`,
adds/notifies the unit and returns no bullet. Interception, targeting and death
timing therefore differ in the current model.

**Exception:** Quell's 103 -> fragment carrier 104 -> missile 53 is staged. Its
child belongs to the carrier's creation trigger, not necessarily the parent
weapon's firing tick. Do not move every missile spawn to time zero.

**Close when:** direct launchers create one owned missile at the muzzle without
a surrogate bullet; Quell keeps its correct parent/fragment timing. Cover
shooter death, expiry, retargeting, interception, split children, team/rule
inheritance and late join. Generate the reachable spawn/fragment graph.

## P1 F08 — weapon mount state and Navanax cadence differ by controller

**Oxide:** [unit_combat.rs](src/network/combat/unit_combat.rs),
`collect_allied_weapon_fire_n`, `collect_manual_weapon_fire`,
`spawn_weapon_fire_for_team`. Mirrored pairs share a group timer and emit both
mounts together; this does not model per-mount alternation, rotation, cone,
charge/warmup or delayed shots. Equal average DPS does not establish parity.

A narrow direct inconsistency: Navanax AI uses main reload **130** while the
manual path uses **65** for the same primary volley. Do not choose a constant
from the pre-init TSV alone; characterize the initialized JAR mounts.

**Official:** `Weapon.update` maintains per-mount reload/side/rotation/continuous
state. Changing controller does not replace the weapon's reload contract.

**Close when:** identical sustained aim/trigger traces have the same controllable
mount event sequence under command, logic and player control. Start with Navanax
and mirrored Dagger, then all reachable mount families. Autonomous repair/point
defence retain independent targeting. Replace the four-timer representation
where necessary with actual initialized mounts, not another controller special case.

## P1 F09 — Oxynoe repair hardcodes projectile ownership

**Oxide:** `simulate_allied_oxynoe_repair` in
[src/network/simulation/units.rs](src/network/simulation/units.rs) finds a target
for `snapshot.team` but emits using shooter ID **0** and team **1**. Its caller
invokes this branch before the normal hold-fire/can-shoot filter, and the helper
owns a separate reload calculation.

**Official:** weapon/bullet creation uses actual owner/team and firing
eligibility. A repair path must not change ownership or bypass `canShoot`.

**Close when:** non-team-1 Oxynoe repairs its own team using its own ID, does
not gain enemy healing through ownership substitution, and respects disarmed/
hold-fire states. Verify rule/status scaling and mirror timing through
`simulate_allied_units` and the shared mount engine, not only the helper.

## P1 F10 — position-path cache can reuse a different world's field

**Oxide:** `navigation_field_toward` in
[src/network/combat/enemy.rs](src/network/combat/enemy.rs) has a `thread_local`
cache keyed by revision, class, team and goal, without world identity, dimensions
or epoch. Different worlds with the same key on one thread reuse the first
world's `Arc<Vec<u32>>`. This is a source-level counterexample, not a reproduced
runtime crash.

**Change:** world-owned cache, explicit topology/rule invalidations. Dimensions
alone do not distinguish same-sized maps; a raw pointer is not a durable epoch.

**Close when:** query A then B on the same thread, using equal-sized worlds with
equal revision/goal but opposite walls, and compare each with its own uncached
builder. Repeat with different dimensions and map rotation. Also retain the
required official Pathfinder first-step comparison on a live `Vars.world`.

## P1 F11 — homing search origin ignores valid aim coordinates

**Oxide:** `simulate_projectiles` computes an interpolated bullet position and
searches the homing radius there; its comment presents that as universal.

**Official:** pinned `BulletType.updateHoming` uses valid `aimX/aimY`, falling
back to bullet position for negative/unset coordinates, with an explicit
`aimTile` branch and exclusion of already-collided targets. Search origin and
velocity-turn origin are distinct concepts.

**Close when:** with valid aim, choose between targets near the aim and near the
bullet; repeat with unset aim. Compare target/turn traces, aim-tile priority,
healing filters, delay, turn limit and collision-history exclusion. Neither
always-aim nor always-bullet is correct.

## Closure sequence

Capture a failing oracle-backed example, change the smallest shared contract,
then run its cross-system regressions. Avoid a whole-server rewrite or giant
parity patch. The sequence below is planned work, not completion claims or a
calendar estimate.

| Stage | Bounded work and dependency | Exit evidence |
|---|---|---|
| R0 — oracle and denominator | Pinned JAR/source; reuse compat/parity tools; enumerate production, initialized mounts, child bullets/abilities and rule eligibility. | Machine-readable contract/scenario/provenance/result coverage for all 50 main outputs plus reachable auxiliary entities. Deliberately perturbed fixture fails the comparator. |
| R1 — narrow regressions | After R0: F10 cache, F05 source-aware acceptance, F09 owner/eligibility and F08 Navanax characterization, as separate fixes. | Each regression fails on the baseline and passes after its own fix; existing gates unchanged. |
| R2 — real Erekir supply/assembly | After R0: F04 liquid storage/edges; then F01 payload stocks and F02/F03 geometry/modules/drones/progress. Correct the README ferry explanation. | All six assembler outputs from real finite supply; missing, blocked and fractional inputs and tier changes match Java. |
| R3 — complete production lifecycle | After R1, plus R2 for assemblers: accept/move-in/process/hold/move-out/dump, costs, bans, cap and rally. | Every main unit through its actual production route. Retained states survive applicable save/rejoin paths without duplicate resources, entities or orders. |
| R4 — shared combat contracts | After R0, alongside R2: per-mount state, shot queue and typed projectiles for F06–F08/F11. Preserve swept-hit regressions. | Matching shot/event, trajectory and damage traces for all reachable weapons, including direct versus fragment-triggered missiles. |
| R5 — AI, abilities and death | After R1/R4: existing pathfinding, support/mining, control/possession, passenger, death and drowning matrix. Inspect early-return mining/building/firing branches. | Controller transitions neither duplicate nor suppress required independent updates; groups, ownership, cap and payload membership remain consistent. |
| R6 — full world/multiplayer | Integrate R2–R5. Extend clients to actual full NetworkIO loading and event application, beyond decoded fields and hand-preplaced buildings. | Survival/sandbox, two-client observers, ordinary >360-tick block snapshots, rejoin and Java/Rust save exchange pass. |
| R7 — release/performance | After semantic closure, optimize demonstrated hot scans with bounded indexes/broadphase and deterministic ordering. | No required skips/untested contracts; declared tick, latency and memory budgets under soak and map rotation without semantic divergence. |

R1 and R2 can proceed independently after R0; R4 can run alongside R2. Give
shared world/codec changes one owner and an agreed schema. Split each stage
into reviewable failing contracts rather than waiting for a complete migration.

### Targeted architecture changes

Introduce typed state where demonstrated defects require it: source-aware
payload transfer, per-liquid storage, initialized mount state, projectile
behaviour and world-owned navigation cache. Keep wire/save adapters separate
and migrate old saves deliberately. Internal Rust layout is not Java protocol.

For multi-object changes, specify whether one authoritative tick owns all
mutations or queued transactions serialize them. Validate and commit transfers
once. Dropping DashMap guards prevents deadlocks but alone does not establish
atomicity. Test that boundary; no specific concurrent race is claimed here.

Avoid separate AI/player/logic weapon engines, faster snapshots that hide
rollback, weakened cap checks, core-item payload substitutes, or a new runtime
SHA-certification gate. Oracle identity belongs to extraction, tests and CI.

`simulate_unit_collisions` currently scans all unit pairs and player/unit pairs:
O(U² + P*U) candidate work. At 5,000 units that is 12,497,500 unit pairs before
filters; this is a calculated loop count, not a measured benchmark. Later use
spatial broadphase and dirty topology indexes while preserving pair order and
tie-breaking. Compare the same maps, waves, inputs and observations on both
servers; faster load tests do not measure parity.

### Test matrix and oracle design

Run a finite-resource production scenario for **every output**, with full
high-risk intersections for the findings. Use pairwise combinations only for
lower-risk surrounding axes, not instead of end-to-end routes.

| Axis | Required coverage |
|---|---|
| Mode/rules | Finite survival, sandbox preset, explicit infinite-resource overrides, global/team cost and build speed, bans and factory activation. Zero cost is not all sandbox semantics. |
| Controller/team | Default AI, commands, logic, player; transitions both ways; non-default team and configurable wave team; independent autonomous mounts/abilities. |
| Resources/time | Full/zero/fractional power and liquids; disabled, pause/resume, supported scaling, output backpressure, interruption just before completion. |
| Payload/cap | Producer -> conveyor -> reconstructor; carried/self-entry; module/assembler supply; full receiver; per-type cap; carried nested units versus building payloads; failed dump, rally and deselection. |
| Movement/combat | Ground/legs/naval/air; doors/topology changes; moving shooter/target; alternation/cones/delays; instant and continuous attacks; homing, fragments, shields, statuses and death. |
| Persistence/network | Save at retained states, restart/map rotation, late join/observer, ordinary snapshot cadence, full-world load, event application and disconnect/rejoin. |

Extend `tests/parity` and existing smoke tools, not a disconnected second
bootstrap. Proposed regression identities (not existing passing tests):

- `assembler_requires_real_payloads`, `assembler_spawn_uses_checked_area`,
  `assembler_progress_tracks_positioned_drones`.
- `multiblock_liquid_outputs_use_perimeter`,
  `reconstructor_rejects_output_face`, `ballistic_outlives_aim_point`,
  `delayed_shot_does_not_fly_early`.
- `spawn_unit_launcher_preserves_creation_stage`,
  `navanax_controller_cadence`, `oxynoe_preserves_owner_and_hold_fire`,
  `navigation_cache_is_world_owned`, `homing_distinguishes_aim_from_position`.

Compare discrete transitions, quantities, types/teams, event order and
shot/spawn/death counts exactly. Normalize generated IDs only through a
bijection preserving references and creation order; never hide missing or
duplicate entities. Set floating-point tolerances per measured contract and
precision, not one permissive global epsilon. Preserve RNG, seeds, update order
and tie-breaking; a deterministic but different RNG is not Java parity.
Compare intermediate traces, since matching final HP/inventory can conceal
wrong timing or compensating errors.

Test Java -> Rust -> Java and Rust -> Java -> Rust for the supported save
format, then advance the restored world with the same input trace. Decode
success alone does not prove resumed production/control/combat. Runtime JSON
persistence and MSAV exchange are separate labeled contracts.

### Existing gates and release completion

[ci.yml](.github/workflows/ci.yml) already has Rust, architecture, concurrency
and current-target JAR gates. Its JAR is verified before the survival-cycle
smoke. Do not describe the oracle as absent or weaken existing checks. Its
production smoke is narrower than all-unit coverage. The push trigger names
`main`; PR and manual-dispatch triggers also exist. Establish explicit release
execution/protection rather than assuming every direct release push was tested.
Branch-protection settings were not inspected.

[navigation.md](navigation.md#client-in-the-loop-harness) explicitly states that
current probes do not load the entire world through NetworkIO. Rules decoding
and prepared-building observations do not close R6. Optional local missing-JAR
`skip` is not run, not passed; required release lanes must have zero required
skips. Preserve the existing CI `test -f` guard.

Existing commands below are validation instructions, not results of this review:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=1
bash tools/architecture_guard.sh
cargo run --quiet --manifest-path tools/dashmap_guard/Cargo.toml -- check . --paths src,tests --deny-warnings
MINDUSTRY_CURRENT_JAR="$MINDUSTRY_1597_JAR" bash tools/compat_jar_gate.sh
bash tools/smoke/smoke_join_1597.sh 6597
bash tools/smoke/smoke_sandbox_build_1597.sh 6598
OXIDE_SMOKE_SCENARIO=survival-cycle bash tools/smoke/smoke_unit_payload_1597.sh 6599
OXIDE_SMOKE_SCENARIO=survival-air-cycle bash tools/smoke/smoke_unit_payload_1597.sh 6600
git diff --check
```

Resolve and verify `MINDUSTRY_1597_JAR` through the navigation guide first.
Integrate all-unit, full-world, controller and save cases as their stages land.
Keep watchdog-bounded single-threaded Rust tests and concurrency guards;
timeouts must not be retried until green.

Release completion requires no open required behavioural contract, unexplained
oracle mismatch or required skip, with evidence for every reachable output and
behaviour family. Include sustained production/waves/control/death/save/map
rotation sessions. Declare hardware and workload before selecting performance
thresholds. At 60 TPS the entire tick budget is about 16.67 ms, not a separate
budget for each subsystem. Finite tests establish declared coverage, not a
mathematical proof for every possible world; report coverage and residual limits
instead of an unsupported global “100% vanilla” percentage.
