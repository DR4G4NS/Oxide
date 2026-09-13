//! Building construction lifecycle: build plans, requirements/refund,
//! footprint helpers and the build/break scheduler + finish envelopes.
//! The listener adapter re-exports these through crate::network::listener::*.

use crate::network::codec::Writes;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::network::world::*;

use crate::network::economy::inventory::items_for_team;

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::network::decoders::BuildPlan;
use tracing::{debug, info, warn};

use crate::network::buildings::placement as building_placement;
use crate::network::buildings::plans::sync_unit_build_plans;
use crate::network::combat::enemy::reregister_team_core;
use crate::network::combat::unit_combat::invalidate_navigation_for_block;
use crate::network::economy::TeamItemsMut;
use crate::network::units::unit_orders::unit_has_place_plan;
use crate::network::units::unit_orders::{unit_construction_work, unit_within_build_range};
use crate::network::wire::auth::{actor_action_allowed, player_team};
use crate::network::wire::encode::frame_generated_packet;
use crate::network::wire::persistence::{
    encode_construct_finish, outbound_typeio_object, valid_build_position,
};
use crate::network::wire::tile_config::broadcast_placement_power_configs;
use crate::state::game_state::{GameMode, GameState};
use dashmap::DashMap;
use std::time::Instant;

/// Last accept-to-enqueue sample for a sandbox break. Tests read this;
/// production only traces (off by default).
#[derive(Clone, Debug, Default)]
pub(crate) struct BreakTiming {
    pub plan_seen: Option<Instant>,
    pub finish_entered: Option<Instant>,
    pub finish_enqueued: Option<Instant>,
    pub rejected_snapshots: u32,
}

static BREAK_TIMING: parking_lot::Mutex<BreakTiming> = parking_lot::Mutex::new(BreakTiming {
    plan_seen: None,
    finish_entered: None,
    finish_enqueued: None,
    rejected_snapshots: 0,
});

pub(crate) fn reset_break_timing() {
    *BREAK_TIMING.lock() = BreakTiming::default();
}

pub(crate) fn last_break_timing() -> BreakTiming {
    BREAK_TIMING.lock().clone()
}

pub(crate) fn record_break_plan_seen() {
    let now = Instant::now();
    let mut sample = BREAK_TIMING.lock();
    if sample.plan_seen.is_none() {
        sample.plan_seen = Some(now);
    }
    tracing::trace!(target: "oxide::break_timing", "break plan seen");
}

pub(crate) fn record_snapshot_rejected() {
    BREAK_TIMING.lock().rejected_snapshots += 1;
    tracing::trace!(target: "oxide::break_timing", "client snapshot rejected");
}

fn record_finish_entered() {
    BREAK_TIMING.lock().finish_entered = Some(Instant::now());
    tracing::trace!(target: "oxide::break_timing", "finish_pending_break entered");
}

fn record_finish_enqueued() {
    BREAK_TIMING.lock().finish_enqueued = Some(Instant::now());
    tracing::trace!(target: "oxide::break_timing", "DeconstructFinish enqueued");
}

/// Official ConstructBlock.construct/deconstruct finish immediately when
/// `state.rules.infiniteResources` or `team.rules().infiniteResources`
/// holds (159.7 javap). `instantBuild` is the BuilderComp client drain
/// (`instant && infiniteResources`), not this server-side finish gate —
/// it is still honoured here so an explicit rules override can match
/// editor-like placement. `GameMode::Sandbox` is not a short-circuit:
/// the preset seeds these flags via `apply_game_mode_to_wave_rules`,
/// and an admin with `allowEditRules` can turn `infiniteResources` off.
pub(crate) fn construction_is_instant(world: &DynamicWorld, team: u8) -> bool {
    let rules = world.wave_rules.read();
    rules.instant_build
        || rules.infinite_resources
        || world.game_state.infinite_resources.load(Ordering::Relaxed)
        || rules.team_rule(team).infinite_resources
}

/// Official `ConstructBlock` content ids: `build1`..`build16` are 5..=20.
pub(crate) fn is_construct_block(block: i16) -> bool {
    (5..=20).contains(&block)
}

/// `ConstructBlock.get(size)` — `buildN` id is `4 + size` (159.7 content.json).
pub(crate) fn construct_block_id(target: i16) -> i16 {
    4 + i16::from(crate::game::content::block_size(target).clamp(1, 16))
}

/// Vanilla `setConstruct`: keep `previous` only when it shares the ConstructBlock size.
fn construct_previous_id(previous: i16, current: i16) -> i16 {
    if previous > 0
        && crate::game::content::block_size(previous) == crate::game::content::block_size(current)
    {
        previous
    } else {
        0
    }
}

fn construct_items_left(current: i16, progress: f32, build_cost_multiplier: f32) -> Vec<f32> {
    let requirements = crate::game::content::block_requirements(current);
    if requirements.is_empty() {
        return Vec::new();
    }
    let left = (1.0 - progress.clamp(0.0, 1.0)).max(0.0);
    let mut accum = Vec::with_capacity(requirements.len() * 3);
    for (_item, amount) in requirements {
        accum.push(0.0);
        accum.push(0.0);
        accum.push(
            (*amount as f32 * build_cost_multiplier.max(0.0) * left)
                .round()
                .max(0.0),
        );
    }
    accum
}

/// Authoritative ConstructBlock tile so `writeSync`, late joiners and a
/// hot-swap restream replay the same ghost the 159.7 client draws
/// (`ConstructBuild.draw` uses `progress` / `previous` / `current`).
fn upsert_construct_tile(
    world: &DynamicWorld,
    pending: &PendingBuild,
    previous: i16,
    progress: f32,
) {
    let progress = progress.clamp(0.0, 1.0);
    let previous = construct_previous_id(previous, pending.block);
    let construct = construct_block_id(pending.block);
    let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
    if let Some(existing) = dynamic_at(world, pending.position) {
        if !is_construct_block(existing.block) && existing.position == pending.position {
            building_placement::remove_building_from_world(world, existing.position);
        }
    }
    let mut tile = DynamicTile {
        position: pending.position,
        block: construct,
        rotation: pending.rotation % 4,
        team: pending.team,
        occupied: pending.occupied.clone(),
        health: crate::game::content::block_health(construct).max(1.0),
        production_progress: progress,
        stored_item: previous,
        stored_amount: i32::from(pending.block),
        payload_accum: construct_items_left(pending.block, progress, cost_mult),
        generation: crate::network::world::next_building_generation(),
        ..DynamicTile::default()
    };
    tile.enabled = true;
    world.tiles.insert(pending.position, tile);
    for cell in &pending.occupied {
        world.tile_footprint.insert(*cell, pending.position);
    }
    world.persistence_dirty.store(true, Ordering::Relaxed);
}

fn sync_construct_tile_progress(world: &DynamicWorld, position: i32, current: i16, progress: f32) {
    let progress = progress.clamp(0.0, 1.0);
    let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
    let accum = construct_items_left(current, progress, cost_mult);
    if let Some(mut tile) = world.tiles.get_mut(&position) {
        if is_construct_block(tile.block) {
            tile.production_progress = progress;
            tile.stored_amount = i32::from(current);
            tile.payload_accum = accum;
        }
    }
}

fn remove_construct_tile(world: &DynamicWorld, position: i32, occupied: &[i32]) {
    let is_construct = world
        .tiles
        .get(&position)
        .is_some_and(|tile| is_construct_block(tile.block));
    if !is_construct {
        return;
    }
    world.tiles.remove(&position);
    for cell in occupied {
        world.tile_footprint.remove(cell);
    }
}

/// Returns the network template with the CURRENT live team build plans
/// spliced into the team-blocks section. The official server writes the
/// current `TeamData.plans` of every active team on each connection, so a
/// late joiner must see ghost builds that appeared after hosting, not just
/// the plans baked into the template at host time.
///
/// NOTE: must NOT take `persistence_lock` — `host_map` re-streams while
/// already holding it (parking_lot mutexes are not reentrant). The plans are
pub fn network_template_with_plans(world: &DynamicWorld) -> std::io::Result<Arc<Vec<u8>>> {
    let rules = world.wave_rules.read().clone();
    network_template_with_plans_and_rules(world, &rules)
}

/// Like `network_template_with_plans`, but projects `rules` instead of the
/// live `world.wave_rules`. Host-map restreams use this so a failure can
/// abort before the first mutation without sending the old mode's Rules
/// JSON. Live `SetMode` sends `Call.setRules` instead of rebuilding the
/// world stream.
pub(crate) fn network_template_with_plans_and_rules(
    world: &DynamicWorld,
    rules: &crate::network::units::WaveRules,
) -> std::io::Result<Arc<Vec<u8>>> {
    let plans = world.team_build_plans.read().clone();
    let patched =
        crate::engine::world_stream::replace_team_blocks(&world.network_template, &plans)?;
    // The Gamemode preset and the `rules` overrides only ever reached the
    // authority's own WaveRules; the streamed template still carried the map
    // file's rules, so a sandbox client kept `infiniteResources = false` and
    // never predicted instant construction (ConstructBlock.construct only
    // short-circuits under that flag).
    let patched = crate::engine::world_stream::replace_rules(
        &patched,
        &crate::network::wire::bootstrap::client_visible_rules_json(world, rules)?,
    )?;
    Ok(Arc::new(patched))
}

/// Mirrors the official `TeamData.plans.addFirst` when a construction begins
/// (BuildingComp.java:364): the ghost plan is visible to every client until
/// the build completes or the tile is broken. Plans belong to the PLACING
/// player's team (PvP ghost plans render in the team color; survival/attack
/// is team 1).
pub(crate) fn add_team_plan(
    world: &DynamicWorld,
    team: u8,
    plan: crate::engine::typeio::TeamBlockPlan,
) {
    let mut plans = world.team_build_plans.write();
    let teams = &mut plans.teams;
    if let Some(entry) = teams.iter_mut().find(|entry| entry.team == i32::from(team)) {
        if !entry.plans.iter().any(|existing| {
            existing.x == plan.x && existing.y == plan.y && existing.block == plan.block
        }) {
            entry.plans.push(plan);
        }
    } else {
        teams.push(crate::engine::typeio::TeamPlans {
            team: i32::from(team),
            plans: vec![plan],
        });
    }
    drop(plans);
    world.persistence_dirty.store(true, Ordering::Relaxed);
}

/// Removes the ghost plan at (x, y) from `team`'s plan list and reports
/// whether anything was removed. Pure (lock-free) so the team-scoped deletion
/// rule is directly unit-testable.
pub(crate) fn remove_team_plan_from(
    blocks: &mut crate::engine::typeio::TeamBlocks,
    team: u8,
    x: i16,
    y: i16,
) -> bool {
    let Some(entry) = blocks
        .teams
        .iter_mut()
        .find(|entry| entry.team == i32::from(team))
    else {
        return false;
    };
    let before = entry.plans.len();
    entry.plans.retain(|plan| !(plan.x == x && plan.y == y));
    entry.plans.len() != before
}

/// Mirrors the official removal of a `BlockPlan` once its building completes
/// (BuildingComp.java:355-361) or the tile is broken (InputHandler break).
/// The removal is scoped to a single `team`: every official caller iterates
/// `player.team().data().plans` (InputHandler.deletePlans deletes only the
/// acting player's own team plans), so one actor must never be able to erase
/// another team's ghost plans.
pub(crate) fn remove_team_plan(world: &DynamicWorld, team: u8, x: i16, y: i16) {
    let mut plans = world.team_build_plans.write();
    let removed = remove_team_plan_from(&mut plans, team, x, y);
    drop(plans);
    if removed {
        world.persistence_dirty.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn apply_build_plans(
    player: &mut SessionPlayer,
    plans: &[BuildPlan],
    world: &Arc<DynamicWorld>,
    out: &dyn crate::network::outbound::FrameEmit,
    admin: &crate::state::administration::Administration,
    update_building: bool,
) -> std::io::Result<()> {
    let _persistence_guard = world.persistence_lock.lock();
    player.building = update_building;
    let current: HashSet<_> = plans
        .iter()
        .map(|plan| (plan.breaking, plan.position, plan.block))
        .collect();
    player.active_plans.retain(|key| current.contains(key));
    // ClientSnapshot carries at most 20 plans. Aborting every pending that
    // is not in that window (BeginPlace then the next 20, or a long conveyor
    // line) RemoveTile+BeginPlace storms the outbound queue and spikes ping.
    // Official clearBuilding only empties the unit queue; Q is an empty
    // snapshot. Drop this player's pendings only then.
    if current.is_empty() {
        abort_dropped_player_plans(world, player, &current, out)?;
    }
    for plan in plans {
        let key = (plan.breaking, plan.position, plan.block);
        let mut covered = false;
        if plan.breaking {
            if let Some(mut pending) = world.pending_breaks.iter_mut().find(|pending| {
                pending.position == plan.position || pending.occupied.contains(&plan.position)
            }) {
                pending.last_seen = std::time::Instant::now();
                pending.builder = player.clone();
                covered = true;
            }
        } else if let Some(mut pending) = world.pending_builds.get_mut(&plan.position) {
            if pending.block == plan.block {
                pending.rotation = plan.rotation % 4;
                pending.config = plan.config.clone();
                pending.last_seen = std::time::Instant::now();
                pending.builder = player.clone();
                covered = true;
            }
        }
        if covered {
            continue;
        }
        // A completed plan may remain in one or more late ClientSnapshots.
        // NetServer skips it silently: ConstructFinish was already sent by
        // finish_pending_build. Re-broadcasting a reliable finish on every
        // snapshot creates an O(plans * snapshot-rate) queue storm that also
        // delays the framework ping handled by this same connection task.
        let tile_done = !plan.breaking
            && world
                .tiles
                .get(&plan.position)
                .is_some_and(|tile| tile.block == plan.block && tile.rotation == plan.rotation % 4);
        if tile_done {
            player.active_plans.remove(&key);
            remove_team_plan(
                world,
                player_team(world, player),
                (plan.position >> 16) as i16,
                plan.position as i16,
            );
            continue;
        }
        if player.active_plans.contains(&key) {
            // Not covered and not done: re-register below.
            player.active_plans.remove(&key);
        }
        if !valid_build_position(world, plan.position) {
            continue;
        }

        // Official InputHandler.build/beginBreak run
        // `admins.allowAction(player, ActionType.placeBlock/breakBlock)`
        // before any plan is accepted (P0-4). The action carries the packed
        // tile and the requested block for filter context.
        let action_allowed = actor_action_allowed(
            admin,
            player,
            if plan.breaking {
                crate::state::administration::ActionType::BreakBlock
            } else {
                crate::state::administration::ActionType::PlaceBlock
            },
            Some(plan.position),
            Some(plan.block),
            None,
        );
        if !action_allowed {
            // M5: official NetServer.clientSnapshot sends
            // `Call.removeQueueBlock(con, x, y, breaking)` and records the
            // plan in rejectedRequests when allowAction rejects it (JAR
            // bytecode offsets 550-586).
            {
                let mut payload = Vec::new();
                crate::network::codec::Writes::write_i(
                    &mut payload,
                    (plan.position >> 16) as i16 as i32,
                )
                .ok();
                crate::network::codec::Writes::write_i(&mut payload, plan.position as i16 as i32)
                    .ok();
                crate::network::codec::Writes::write_bool(&mut payload, plan.breaking).ok();
                if let Ok(frame) =
                    frame_generated_packet(REMOVE_QUEUE_BLOCK_PACKET_ID, &payload, false)
                {
                    out.enqueue_to(player.id - 1_000_000, frame, true);
                }
            }
            continue;
        }

        if plan.breaking {
            // Vanilla BuilderComp (UnitEntity, 159.7): if tile.build is already
            // a ConstructBuild (BeginPlace), the unit calls ConstructBuild.deconstruct
            // and does NOT Call.beginBreak. At progress <= deconstructThreshold
            // (Block default 0f) or infiniteResources that immediately
            // Call.deconstructFinish → tile.remove(). Skipping the breaking
            // plan left the client's striped ConstructBlock in place with the
            // red beam stuck on it.
            if let Some(pending) = pending_build_covering(world, plan.position) {
                abort_pending_construct(world, out, pending)?;
                continue;
            }
            if world.pending_breaks.iter().any(|pending| {
                pending.position == plan.position || pending.occupied.contains(&plan.position)
            }) {
                continue;
            }
            let removed_block = effective_block(world, plan.position);
            if removed_block != 0 {
                // SOL-002: only the owning team may demolish a building.
                let owner = effective_building_team(world, plan.position);
                if owner != 0 && owner != player_team(world, player) {
                    continue;
                }
                let dynamic = dynamic_at(world, plan.position);
                let origin = dynamic
                    .as_ref()
                    .map_or_else(|| base_origin(world, plan.position), |tile| tile.position);
                let occupied = dynamic.as_ref().map_or_else(
                    || {
                        block_footprint(world, origin, removed_block)
                            .unwrap_or_else(|| vec![origin])
                    },
                    |tile| tile.occupied.clone(),
                );
                let pending = PendingBreak {
                    position: origin,
                    block: removed_block,
                    occupied,
                    dynamic: dynamic.is_some_and(|tile| tile.block != 0),
                    team: player_team(world, player),
                    builder: player.clone(),
                    last_seen: std::time::Instant::now(),
                    remaining_ticks: 0.0,
                };
                world.pending_breaks.insert(origin, pending.clone());
                // Official Build.beginBreak always creates a ConstructBlock on
                // the client before deconstructFinish. Skipping it leaves the
                // local plan/beam stuck with no ConstructBuild to complete.
                let payload = encode_begin_break(player, origin)?;
                out.broadcast_critical(frame_generated_packet(
                    BEGIN_BREAK_PACKET_ID,
                    &payload,
                    false,
                )?);
                record_break_plan_seen();
                if construction_is_instant(world, pending.team) {
                    if let Err(err) = finish_pending_break(world, out, pending) {
                        warn!("Could not finish instant sandbox deconstruction: {}", err);
                    }
                    continue;
                }
                schedule_break(world, &pending);
                // A break removes the ghost plan of the block being destroyed
                // (official InputHandler iterates `player.team().data().plans`).
                remove_team_plan(
                    world,
                    player_team(world, player),
                    (plan.position >> 16) as i16,
                    plan.position as i16,
                );
                player.active_plans.insert(key);
            }
        } else if world.wave_rules.read().block_banned(plan.block) {
            debug!(
                "Rejected banned block {} at {:?}",
                plan.block,
                (plan.position >> 16, plan.position as i16)
            );
            continue;
        } else if crate::game::content::is_player_buildable(
            plan.block,
            *world.game_state.mode.read() == GameMode::Sandbox,
        ) {
            let Some(occupied) = block_footprint(world, plan.position, plan.block) else {
                continue;
            };
            // The plan belongs to the PLACING PLAYER'S team (official
            // `TeamData.plans`); the finished tile, the consumed build cost
            // and the ghost plan all use this team (survival/attack == 1).
            let placing_team = player_team(world, player);
            let mut team_building_count = live_team_building_count(world, placing_team, plan.block);
            let replacing_same = occupied.iter().any(|&pos| {
                world
                    .tiles
                    .get(&pos)
                    .is_some_and(|t| t.block == plan.block && t.team == placing_team)
            }) || occupied.iter().any(|&pos| {
                let origin = base_origin(world, pos);
                world
                    .base_buildings
                    .get(&origin)
                    .is_some_and(|b| b.block == plan.block && b.team == placing_team)
            });
            if replacing_same && team_building_count > 0 {
                team_building_count -= 1;
            }
            if world.wave_rules.read().is_over_placement_limit(
                plan.block,
                team_building_count,
                placing_team,
            ) {
                debug!(
                    "Rejected block {} placement: team {} is at or over placement limit (count {})",
                    plan.block, placing_team, team_building_count
                );
                continue;
            }
            if placement_blocked_by_cores(world, plan.position, plan.block, placing_team) {
                debug!(
                    "Rejected block {} placement: enemy core protection / placeRangeCheck",
                    plan.block
                );
                continue;
            }
            let rotation = plan.rotation % 4;
            let existing_block = effective_block(world, plan.position);
            let existing_team = effective_building_team(world, plan.position);
            let same_block_and_team = existing_block == plan.block && existing_team == placing_team;
            if same_block_and_team && crate::game::content::block_placement(plan.block).quick_rotate
            {
                let existing_rotation = dynamic_at(world, plan.position)
                    .map(|t| t.rotation)
                    .or_else(|| {
                        let x = (plan.position >> 16) as i16 as i32;
                        let y = plan.position as i16 as i32;
                        if x >= 0 && y >= 0 && x < world.width && y < world.height {
                            let index = (y * world.width + x) as usize;
                            world.tile_data.get(index).map(|d| d & 3)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                if existing_rotation != rotation {
                    if let Some(mut tile) = world.tiles.get_mut(&plan.position) {
                        tile.rotation = rotation;
                    } else {
                        let origin = base_origin(world, plan.position);
                        world.base_buildings.remove(&origin);
                        let generation = crate::network::world::assign_new_building_generation(
                            world,
                            plan.position,
                        );
                        world.tiles.insert(
                            plan.position,
                            DynamicTile {
                                logic_control: None,
                                payload_inventory: Vec::new(),
                                position: plan.position,
                                block: plan.block,
                                rotation,
                                team: placing_team,
                                config: plan.config.clone(),
                                enabled: true,
                                message: None,
                                occupied: occupied.clone(),
                                stored_item: -1,
                                stored_amount: 0,
                                production_progress: 0.0,
                                transport_progress: 0.0,
                                ammo_units: 0.0,
                                inventory: Vec::new(),
                                power_stored: 0.0,
                                power_links: Vec::new(),
                                liquid_inventory: Vec::new(),
                                stored_liquid: -1,
                                liquid_amount: 0.0,
                                output_liquid_amount: 0.0,
                                junction_items: Vec::new(),
                                mass_driver_incoming: Vec::new(),
                                mass_driver_rotation: 90.0,
                                mass_driver_waiting: Vec::new(),
                                payload: None,
                                payload_progress: 0.0,
                                payload_rotation: 0.0,
                                payload_accum: Vec::new(),
                                health: crate::game::content::block_health(plan.block),
                                door_open: false,
                                shield: 0.0,
                                light_color: -1_900_545,
                                memory: Vec::new(),
                                duct_rec_dir: 0,
                                unloader_offset: 0,
                                conveyor_items: Vec::new(),
                                factory_command: None,
                                stack_state: 0,
                                stack_link: -1,
                                stack_cooldown: 0.0,
                                generation,
                            },
                        );
                    }
                    building_placement::after_placement(world, plan.position, &plan.config);
                    let payload = encode_begin_place_for_unit(
                        session_builder_unit_id(player),
                        plan.position,
                        plan.block,
                        rotation,
                        placing_team,
                        &[0],
                    )?;
                    out.broadcast_critical(frame_generated_packet(
                        BEGIN_PLACE_PACKET_ID,
                        &payload,
                        false,
                    )?);
                    remove_team_plan(
                        world,
                        placing_team,
                        (plan.position >> 16) as i16,
                        plan.position as i16,
                    );
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                continue;
            }
            let replaceable =
                placement_footprint_is_replaceable(world, &occupied, plan.block, placing_team);
            if !replaceable {
                let existing = dynamic_at(world, plan.position);
                debug!(
                    "Rejected non-replaceable placement: requested={}, dynamic={:?}, base={}",
                    plan.block,
                    existing,
                    base_block(world, plan.position)
                );
                continue;
            }
            let previous_block = effective_block(world, plan.position);
            let sub_size = crate::game::content::block_size(plan.block);
            let prev_size = crate::game::content::block_size(previous_block);
            let previous = if prev_size == sub_size {
                previous_block
            } else {
                0
            };
            let pending = PendingBuild {
                position: plan.position,
                block: plan.block,
                previous_block: previous,
                rotation,
                config: plan.config.clone(),
                occupied,
                team: placing_team,
                builder: player.clone(),
                last_seen: std::time::Instant::now(),
                assist_progress: 0.0,
                remaining_ticks: 0.0,
                applied_assist: 0.0,
            };
            world.pending_builds.insert(plan.position, pending.clone());
            // Official Build.beginPlace always places a ConstructBlock, even
            // when rules.instantBuild finishes the same tick. The client
            // BuilderComp waits for that tile; ConstructFinish alone leaves
            // the build beam stuck on a plan that never initializes.
            let payload = encode_begin_place(player, &pending)?;
            out.broadcast_critical(frame_generated_packet(
                BEGIN_PLACE_PACKET_ID,
                &payload,
                false,
            )?);
            if construction_is_instant(world, placing_team) {
                if let Err(err) = finish_pending_build(world, out, pending) {
                    warn!("Could not finish instant sandbox placement: {}", err);
                }
                continue;
            }
            schedule_build(world, &pending);
            upsert_construct_tile(world, &pending, pending.previous_block, 0.0);
            // The official server mirrors every started construction into the
            // team's live build plans so all clients render the ghost.
            add_team_plan(
                world,
                placing_team,
                crate::engine::typeio::TeamBlockPlan {
                    x: (plan.position >> 16) as i16,
                    y: plan.position as i16,
                    rotation: rotation as i16,
                    block: plan.block,
                    config: plan.config.clone(),
                },
            );
            player.active_plans.insert(key);
        }
    }
    sync_unit_build_plans(world, player, plans, update_building);
    Ok(())
}

fn snapshot_covers_break(
    current: &HashSet<(bool, i32, i16)>,
    origin: i32,
    occupied: &[i32],
) -> bool {
    current.iter().any(|(breaking, position, _block)| {
        *breaking && (*position == origin || occupied.contains(position))
    })
}

fn another_worker_has_place(
    world: &DynamicWorld,
    except_player_id: i32,
    position: i32,
    block: i16,
) -> bool {
    world.player_sessions.iter().any(|session| {
        session.id != except_player_id && session.active_plans.contains(&(false, position, block))
    }) || world
        .enemies
        .iter()
        .any(|unit| unit.update_building && unit_has_place_plan(&unit, position))
}

fn another_worker_has_break(
    world: &DynamicWorld,
    except_player_id: i32,
    origin: i32,
    occupied: &[i32],
) -> bool {
    world.player_sessions.iter().any(|session| {
        session.id != except_player_id
            && session.active_plans.iter().any(|(breaking, position, _)| {
                *breaking && (*position == origin || occupied.contains(position))
            })
    }) || world.enemies.iter().any(|unit| {
        unit.update_building
            && unit.build_plans.iter().any(|plan| {
                plan.breaking && (plan.position == origin || occupied.contains(&plan.position))
            })
    })
}

/// Origin pending construct whose footprint covers `position`.
fn pending_build_covering(world: &DynamicWorld, position: i32) -> Option<PendingBuild> {
    if let Some(build) = world.pending_builds.get(&position) {
        return Some(build.value().clone());
    }
    world.pending_builds.iter().find_map(|build| {
        build
            .occupied
            .contains(&position)
            .then(|| build.value().clone())
    })
}

/// Cancel an in-progress / stuck ConstructBlock the way vanilla
/// `ConstructBuild.deconstruct` does when progress is already at or below
/// `deconstructThreshold` (0): `Call.deconstructFinish(tile, current, unit)`
/// then `tile.remove()`. Items were never consumed (`checkRequired` failed
/// or the plan never finished), so there is no refund.
fn abort_pending_construct(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    pending: PendingBuild,
) -> std::io::Result<()> {
    if world.pending_builds.remove(&pending.position).is_none() {
        return Ok(());
    }
    remove_team_plan(
        world,
        pending.team,
        (pending.position >> 16) as i16,
        pending.position as i16,
    );
    world.persistence_dirty.store(true, Ordering::Relaxed);
    remove_construct_tile(world, pending.position, &pending.occupied);
    let payload = encode_deconstruct_finish(
        &pending.builder,
        &PendingBreak {
            position: pending.position,
            block: pending.block,
            occupied: pending.occupied,
            dynamic: false,
            team: pending.team,
            builder: pending.builder.clone(),
            last_seen: pending.last_seen,
            remaining_ticks: 0.0,
        },
    )?;
    out.broadcast_critical(frame_generated_packet(
        DECONSTRUCT_FINISH_PACKET_ID,
        &payload,
        false,
    )?);
    Ok(())
}

fn abort_dropped_player_plans(
    world: &DynamicWorld,
    player: &SessionPlayer,
    current: &HashSet<(bool, i32, i16)>,
    out: &dyn crate::network::outbound::FrameEmit,
) -> std::io::Result<()> {
    let dropped_builds: Vec<i32> = world
        .pending_builds
        .iter()
        .filter(|build| {
            build.builder.id == player.id
                && !current.contains(&(false, build.position, build.block))
                && !another_worker_has_place(world, player.id, build.position, build.block)
        })
        .map(|build| build.position)
        .collect();
    for position in dropped_builds {
        let Some((_, dropped)) = world.pending_builds.remove(&position) else {
            continue;
        };
        remove_construct_tile(world, position, &dropped.occupied);
        remove_team_plan(
            world,
            player_team(world, player),
            (position >> 16) as i16,
            position as i16,
        );
        let mut payload = Vec::new();
        crate::network::codec::Writes::write_i(&mut payload, position)?;
        out.broadcast_critical(frame_generated_packet(
            REMOVE_TILE_PACKET_ID,
            &payload,
            false,
        )?);
    }

    let dropped_breaks: Vec<i32> = world
        .pending_breaks
        .iter()
        .filter(|pending| {
            pending.builder.id == player.id
                && !snapshot_covers_break(current, pending.position, &pending.occupied)
                && !another_worker_has_break(world, player.id, pending.position, &pending.occupied)
        })
        .map(|pending| pending.position)
        .collect();
    for position in dropped_breaks {
        // The server tile is still the original block (BeginBreak only
        // creates a ConstructBuild on the client). Dropping the pending
        // without DeconstructFinish leaves the building in place.
        world.pending_breaks.remove(&position);
    }
    Ok(())
}

/// Official `InputHandler.unitControl` server gate (158.1): the unit must be
/// a standard unit (type 2), `state.rules.possessionAllowed` must hold,
/// Consumes `block`'s build cost from the TEAM's core inventory (official
pub(crate) fn consume_requirements(state: &GameState, team: u8, block: i16) -> bool {
    // A7: the live `WaveRules` carries the per-team TeamRule; the global-only
    // path below is used by the AI-construction caller (simulation.rs, team 1)
    // which has no rules handle. Network build plans go through
    // `consume_requirements_for` with the full official gate.
    consume_requirements_impl(state, None, team, block)
}

/// A7: official `ConstructBlock.checkRequired` gate — `team.rules().
/// infiniteResources || state.rules.infiniteResources` (JAR offsets 0-33).
pub(crate) fn consume_requirements_for(
    state: &GameState,
    rules: &crate::network::units::WaveRules,
    team: u8,
    block: i16,
) -> bool {
    consume_requirements_impl(state, Some(rules), team, block)
}

pub(crate) fn consume_requirements_impl(
    state: &GameState,
    rules: Option<&crate::network::units::WaveRules>,
    team: u8,
    block: i16,
) -> bool {
    // Official ConstructBlock: infiniteResources builds without requiring or
    // consuming core items (`progress >= 1f || state.rules.infiniteResources`).
    if state.infinite_resources.load(Ordering::Relaxed)
        || rules.is_some_and(|rules| {
            rules.infinite_resources || rules.team_rule(team).infinite_resources
        })
    {
        return true;
    }
    let requirements = crate::game::content::block_requirements(block);
    let cost_multiplier = rules
        .map(|rules| rules.build_cost_multiplier.max(0.0))
        .unwrap_or(1.0);
    let scaled: Vec<(usize, i32)> = requirements
        .iter()
        .map(|(item, amount)| {
            (
                *item,
                (*amount as f32 * cost_multiplier).round().max(0.0) as i32,
            )
        })
        .collect();
    let mut items = if team == 1 {
        TeamItemsMut::Legacy(state.core_items.write())
    } else {
        TeamItemsMut::Team(state.team_items.entry(team).or_insert_with(|| vec![0; 22]))
    };
    if scaled
        .iter()
        .any(|(item, amount)| items.get(*item).is_none_or(|stored| stored < amount))
    {
        return false;
    }
    for (item, amount) in scaled {
        items[item] -= amount;
    }
    true
}

/// Refunds half of `block`'s build cost into the TEAM's core inventory
/// (official deconstruction refunds the player's team).
pub(crate) fn refund_requirements(state: &GameState, team: u8, block: i16) {
    refund_requirements_impl(state, None, team, block);
}

pub(crate) fn refund_requirements_for(world: &DynamicWorld, team: u8, block: i16) {
    let rules = world.wave_rules.read();
    if world.game_state.infinite_resources.load(Ordering::Relaxed)
        || rules.infinite_resources
        || rules.team_rule(team).infinite_resources
    {
        return;
    }
    drop(rules);
    let refund_multiplier = world
        .wave_rules
        .read()
        .deconstruct_refund_multiplier
        .max(0.0);
    for (item, amount) in crate::game::content::block_requirements(block) {
        crate::network::core_inventory::deposit_core_items(
            world,
            team,
            *item as i16,
            (*amount as f32 * refund_multiplier).round().max(0.0) as i32,
        );
    }
}

pub(crate) fn refund_requirements_impl(
    state: &GameState,
    rules: Option<&crate::network::units::WaveRules>,
    team: u8,
    block: i16,
) {
    // The same infiniteResources gate that makes deconstruction free must
    // suppress its refund; otherwise a TeamRule/global infinite game mints
    // half the build cost on every break outside the Sandbox enum variant.
    if state.infinite_resources.load(Ordering::Relaxed)
        || rules
            .map(|rules| rules.infinite_resources || rules.team_rule(team).infinite_resources)
            .unwrap_or(false)
    {
        return;
    }
    let mut items = if team == 1 {
        TeamItemsMut::Legacy(state.core_items.write())
    } else {
        TeamItemsMut::Team(state.team_items.entry(team).or_insert_with(|| vec![0; 22]))
    };
    for (item, amount) in crate::game::content::block_requirements(block) {
        if let Some(stored) = items.get_mut(*item) {
            let refund_multiplier = rules
                .map(|rules| rules.deconstruct_refund_multiplier.max(0.0))
                .unwrap_or(0.5);
            *stored =
                stored.saturating_add((*amount as f32 * refund_multiplier).round().max(0.0) as i32);
        }
    }
}

pub(crate) fn base_block(world: &DynamicWorld, position: i32) -> i16 {
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    if x < 0 || y < 0 || x >= world.width || y >= world.height {
        return 0;
    }
    world.base_blocks[(y * world.width + x) as usize]
}

pub(crate) fn base_origin(world: &DynamicWorld, position: i32) -> i32 {
    let block = base_block(world, position);
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    let index = (y * world.width + x) as usize;
    if world.base_centers.get(index).copied().unwrap_or(true) {
        return position;
    }
    let size = i32::from(crate::game::content::block_size(block));
    for cy in y - size..=y + size {
        for cx in x - size..=x + size {
            if cx < 0 || cy < 0 || cx >= world.width || cy >= world.height {
                continue;
            }
            let candidate = (cx << 16) | (cy as u16 as i32);
            let candidate_index = (cy * world.width + cx) as usize;
            if world.base_centers[candidate_index]
                && base_block(world, candidate) == block
                && block_footprint(world, candidate, block)
                    .is_some_and(|positions| positions.contains(&position))
            {
                return candidate;
            }
        }
    }
    position
}

pub(crate) fn block_footprint(world: &DynamicWorld, origin: i32, block: i16) -> Option<Vec<i32>> {
    block_footprint_in(world.width, world.height, origin, block)
}

pub(crate) fn block_footprint_in(
    width: i32,
    height: i32,
    origin: i32,
    block: i16,
) -> Option<Vec<i32>> {
    let x = (origin >> 16) as i16 as i32;
    let y = origin as i16 as i32;
    let size = i32::from(crate::game::content::block_size(block));
    let offset = -(size - 1) / 2;
    let mut positions = Vec::with_capacity((size * size) as usize);
    for dy in 0..size {
        for dx in 0..size {
            let px = x + offset + dx;
            let py = y + offset + dy;
            if px < 0 || py < 0 || px >= width || py >= height {
                return None;
            }
            positions.push((px << 16) | (py as u16 as i32));
        }
    }
    Some(positions)
}

pub(crate) fn dynamic_at(world: &DynamicWorld, position: i32) -> Option<DynamicTile> {
    // Round 74d: O(1) path — exact origin first, then the per-tick
    // footprint index. The linear scan only remains as a fallback for
    // worlds that were never ticked (unit tests) or for tiles placed since
    // the last index rebuild. The legacy version scanned every live tile
    // per lookup, which made the logistics tick 20-32 ms with ~360 tiles.
    if let Some(tile) = world.tiles.get(&position) {
        // Exact origin match (pass 1 and pass 2 of the legacy scan both
        // returned it, live or destroyed).
        return Some(tile.clone());
    }
    if let Some(origin) = world.tile_footprint.get(&position) {
        if let Some(tile) = world.tiles.get(&origin) {
            if tile.block != 0 {
                return Some(tile.clone());
            }
        }
    }
    world.tiles.iter().find_map(|tile| {
        (tile.position == position || tile.occupied.contains(&position))
            .then(|| tile.value().clone())
    })
}

pub(crate) fn effective_building_team(world: &DynamicWorld, position: i32) -> u8 {
    if let Some(build) = world.pending_builds.get(&position) {
        return build.team;
    }
    if !world.pending_builds.is_empty() {
        if let Some(build) = world
            .pending_builds
            .iter()
            .find(|build| build.occupied.contains(&position))
        {
            return build.team;
        }
    }
    dynamic_at(world, position)
        .map(|tile| tile.team)
        .or_else(|| {
            let origin = base_origin(world, position);
            world
                .base_buildings
                .get(&origin)
                .map(|building| building.team)
        })
        .unwrap_or(0)
}

/// Official `team.data().getBuildings(block).size` equivalent: live buildings
/// of `block` owned by `team`, counting dynamic origins plus surviving
/// prebuilt/base buildings without double-counting replacements.
pub(crate) fn live_team_building_count(world: &DynamicWorld, team: u8, block: i16) -> usize {
    let mut count = world
        .tiles
        .iter()
        .filter(|tile| tile.block == block && tile.team == team)
        .count();
    for building in world.base_buildings.iter() {
        if building.block != block || building.team != team {
            continue;
        }
        let origin = building.position;
        if !world
            .tiles
            .get(&origin)
            .is_some_and(|tile| tile.block == block && tile.team == team)
        {
            count += 1;
        }
    }
    count
}

pub(crate) fn placement_footprint_is_replaceable(
    world: &DynamicWorld,
    occupied: &[i32],
    new_block: i16,
    team: u8,
) -> bool {
    let derelict_repair = world.wave_rules.read().derelict_repair;
    occupied.iter().all(|position| {
        if dynamic_at(world, *position).is_some_and(|tile| is_construct_block(tile.block)) {
            return true;
        }
        let existing = effective_block(world, *position);
        let existing_team = effective_building_team(world, *position);
        existing == 0
            || ((existing_team == team || (existing_team == 0 && derelict_repair))
                && crate::game::content::block_can_replace(new_block, existing))
    })
}

fn building_world_center(position: i32, block: i16) -> (f32, f32) {
    let tx = (position >> 16) as i16 as f32;
    let ty = position as i16 as f32;
    let size = f32::from(crate::game::content::block_size(block));
    let extra = ((size as i32 + 1) % 2) as f32 * 4.0;
    (tx * 8.0 + extra, ty * 8.0 + extra)
}

/// Official `Build.validPlace` core-protection + `placeRangeCheck` (Build.java).
pub(crate) fn placement_blocked_by_cores(
    world: &DynamicWorld,
    position: i32,
    block: i16,
    team: u8,
) -> bool {
    let rules = world.wave_rules.read();
    if rules.editor {
        return false;
    }
    let (px, py) = building_world_center(position, block);
    if rules.polygon_core_protection {
        let mut closest: Option<(u8, f32)> = None;
        for other in crate::network::world::registered_core_teams(world) {
            if !rules.team_rule(other).protect_cores {
                continue;
            }
            for core in crate::network::world::team_core_snapshot(world, other) {
                let (cx, cy) = building_world_center(core.position, core.block);
                let dist2 = (cx - px).hypot(cy - py);
                if closest.is_none_or(|(_, best)| dist2 < best) {
                    closest = Some((other, dist2));
                }
            }
        }
        if closest.is_some_and(|(owner, _)| owner != team) {
            return true;
        }
    } else {
        for other in crate::network::world::registered_core_teams(world) {
            if other == team {
                continue;
            }
            let radius = rules.enemy_core_radius_for(other);
            if radius <= 0.0 {
                continue;
            }
            for core in crate::network::world::team_core_snapshot(world, other) {
                let (cx, cy) = building_world_center(core.position, core.block);
                if (cx - px).hypot(cy - py) <= radius + 8.0 {
                    return true;
                }
            }
        }
    }
    if rules.place_range_check {
        const PLACE_OVERLAP: f32 = 54.0;
        for tile in world.tiles.iter() {
            if tile.team == team || tile.team == 0 || tile.block == 0 {
                continue;
            }
            if !rules.team_rule(tile.team).check_placement {
                continue;
            }
            let (ex, ey) = building_world_center(tile.position, tile.block);
            if (ex - px).hypot(ey - py) <= PLACE_OVERLAP {
                return true;
            }
        }
    }
    false
}

pub(crate) fn effective_block(world: &DynamicWorld, position: i32) -> i16 {
    // Round 74 fix: O(1) exact-position lookup first. The previous linear
    // scan over every pending build ran once per tile checked by the mining
    // search, compounding with the number of active builds.
    if let Some(build) = world.pending_builds.get(&position) {
        return build.block;
    }
    // Multi-tile build footprints: a pending build covers `position` even
    // when it is not the origin tile.
    if !world.pending_builds.is_empty() {
        if let Some(build) = world
            .pending_builds
            .iter()
            .find(|build| build.occupied.contains(&position))
        {
            return build.block;
        }
    }
    dynamic_at(world, position)
        .map(|tile| tile.block)
        .unwrap_or_else(|| base_block(world, position))
}

/// Registers a construction plan whose work is advanced by the world loop
/// (SOL-003). Official BuilderComp.update: progress scales with
/// `state.rules.buildSpeed(team)`; instantBuild completes immediately.
/// The build only advances while the placing player is active
/// (`last_seen` refreshed by their updates) or builder units assist.
pub(crate) fn schedule_build(world: &DynamicWorld, pending: &PendingBuild) {
    const ALPHA_BUILD_SPEED: f32 = 0.5;
    // Vanilla `ConstructBuild.buildCost = block.buildTime * buildCostMultiplier`.
    // `simulate_constructions` applies `Rules.buildSpeed(team)` as work per
    // tick, so remaining must NOT be pre-divided by that multiplier.
    let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
    let remaining = if construction_is_instant(world, pending.team) {
        0.0
    } else {
        (crate::game::content::block_build_time(pending.block) * cost_mult / ALPHA_BUILD_SPEED)
            .max(1.0)
    };
    if let Some(mut build) = world.pending_builds.get_mut(&pending.position) {
        build.remaining_ticks = remaining;
    }
}

/// `ConstructBuild.progress` for a RequestBlockSnapshot. Vanilla
/// `BuilderComp` drops the plan when `cb.current != plan.block`; the
/// snapshot must therefore carry the target id and a 0..=1 progress.
pub(crate) fn pending_construct_progress(world: &DynamicWorld, pending: &PendingBuild) -> f32 {
    construct_progress_from_remaining(world, pending.block, pending.remaining_ticks)
}

fn construct_progress_from_remaining(
    world: &DynamicWorld,
    block: i16,
    remaining_ticks: f32,
) -> f32 {
    const ALPHA_BUILD_SPEED: f32 = 0.5;
    let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
    let total =
        (crate::game::content::block_build_time(block) * cost_mult / ALPHA_BUILD_SPEED).max(1.0);
    (1.0 - remaining_ticks / total).clamp(0.0, 1.0)
}

/// Deconstruct progress starts at 1 and falls as `remaining_ticks` drops.
pub(crate) fn pending_break_progress(world: &DynamicWorld, pending: &PendingBreak) -> f32 {
    const ALPHA_BUILD_SPEED: f32 = 0.5;
    let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
    let total = (crate::game::content::block_build_time(pending.block) * cost_mult
        / ALPHA_BUILD_SPEED)
        .max(1.0);
    (pending.remaining_ticks / total).clamp(0.0, 1.0)
}

/// Advances every registered construction by `delta` game ticks (SOL-003):
/// the placing player's active presence (`last_seen` within 300 ms) and any
/// builder-unit assist both contribute. Completed plans finish through
/// `finish_pending_build`. Runs inside the world loop, so pause and --tps
/// govern construction like the official server.
pub(crate) fn simulate_constructions(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let keys: Vec<i32> = world
        .pending_builds
        .iter()
        .map(|build| *build.key())
        .collect();
    let mut changed = false;
    let rules = world.wave_rules.read().clone();
    for key in keys {
        let any_unit_plan = world
            .enemies
            .iter()
            .any(|unit| unit.update_building && unit_has_place_plan(&unit, key));
        let unit_work: f32 = world
            .enemies
            .iter()
            .filter(|unit| {
                unit.update_building
                    && unit_has_place_plan(unit, key)
                    && unit_within_build_range(unit, key)
            })
            .map(|unit| unit_construction_work(&unit, delta_ticks))
            .sum();
        let mut ready = false;
        let Some((build_team, builder, last_seen)) = world
            .pending_builds
            .get(&key)
            .map(|build| (build.team, build.builder.clone(), build.last_seen))
        else {
            continue;
        };
        let infinite_now = construction_is_instant(world, build_team);
        let speed = delta_ticks.max(0.0) * rules.build_speed_for(build_team).max(0.0);
        let player_work = player_plan_work(world, &builder, key, last_seen, infinite_now, speed);
        if let Some(mut build) = world.pending_builds.get_mut(&key) {
            if infinite_now {
                build.remaining_ticks = 0.0;
            }
            let mut work = if any_unit_plan {
                unit_work + player_work
            } else {
                player_work
            };
            let assist_work = (build.assist_progress - build.applied_assist).max(0.0);
            build.applied_assist += assist_work;
            work += assist_work;
            build.remaining_ticks = (build.remaining_ticks - work).max(0.0);
            changed = true;
            if build.remaining_ticks <= 0.0 {
                ready = true;
            }
            let remaining = build.remaining_ticks;
            let current = build.block;
            drop(build);
            sync_construct_tile_progress(
                world,
                key,
                current,
                construct_progress_from_remaining(world, current, remaining),
            );
        }
        if ready {
            if let Some(pending) = world.pending_builds.get(&key).map(|b| b.clone()) {
                if let Err(err) = finish_pending_build(world, out, pending) {
                    warn!("Could not finish pending construction: {}", err);
                }
            }
        }
    }
    changed
}

/// Work the placing player contributes this tick. A live `player_sessions`
/// row used to zero this when `ClientSnapshot.isBuilding` decoded false while
/// the plan was still in the queue — the Alpha beam stayed on a conveyor
/// ghost. `last_seen` is refreshed by every snapshot that still carries the
/// plan. A live row must not be stricter than the no-session fallback.
fn player_plan_work(
    world: &DynamicWorld,
    builder: &SessionPlayer,
    position: i32,
    last_seen: Instant,
    infinite_now: bool,
    speed: f32,
) -> f32 {
    const ALPHA_BUILD_RANGE: f32 = 220.0;
    if infinite_now {
        return speed;
    }
    let now = Instant::now();
    let active = now.saturating_duration_since(last_seen) <= std::time::Duration::from_secs(5);
    if active {
        return speed;
    }
    let presence = world
        .player_sessions
        .get(&builder.unit_id)
        .map(|session| (session.x, session.y, session.building))
        .or_else(|| {
            world.player_sessions.iter().find_map(|session| {
                (session.id == builder.id).then_some((session.x, session.y, session.building))
            })
        });
    let Some((x, y, building)) = presence else {
        return 0.0;
    };
    if !building {
        return 0.0;
    }
    let (bx, by) = unit_plan_world(position);
    if (x - bx).hypot(y - by) <= ALPHA_BUILD_RANGE {
        speed
    } else {
        0.0
    }
}

pub(crate) fn finish_pending_build(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    pending: PendingBuild,
) -> std::io::Result<()> {
    // No persistence_lock here: every caller (the world loop, apply_build_plans)
    // already holds it, and parking_lot mutexes are not reentrant — taking it
    // again deadlocked the loop the first time a plan completed (SOL-003).
    if world.pending_builds.remove(&pending.position).is_none() {
        return Ok(());
    }
    let mut finished_count = live_team_building_count(world, pending.team, pending.block);
    let replacing_same = pending.occupied.iter().any(|&pos| {
        world
            .tiles
            .get(&pos)
            .is_some_and(|t| t.block == pending.block && t.team == pending.team)
    }) || pending.occupied.iter().any(|&pos| {
        let origin = base_origin(world, pos);
        world
            .base_buildings
            .get(&origin)
            .is_some_and(|b| b.block == pending.block && b.team == pending.team)
    });
    if replacing_same && finished_count > 0 {
        finished_count -= 1;
    }
    if world
        .wave_rules
        .read()
        .is_over_placement_limit(pending.block, finished_count, pending.team)
    {
        world.pending_builds.insert(pending.position, pending);
        return Ok(());
    }
    let replaceable =
        placement_footprint_is_replaceable(world, &pending.occupied, pending.block, pending.team);
    if !replaceable
        || !consume_requirements_for(
            &world.game_state,
            &world.wave_rules.read(),
            pending.team,
            pending.block,
        )
    {
        // Official ConstructBuild stays on the tile when checkRequired
        // fails (`canFinish = false`); REMOVE_TILE here destroyed the
        // client's ConstructBlock and left the beam on a ghost.
        world.pending_builds.insert(pending.position, pending);
        return Ok(());
    }
    let mut dynamic_origins = HashSet::new();
    let mut base_origins = HashSet::new();
    for position in &pending.occupied {
        if effective_block(world, *position) == 0 {
            continue;
        }
        if let Some(tile) = dynamic_at(world, *position).filter(|tile| tile.block != 0) {
            dynamic_origins.insert(tile.position);
        } else {
            base_origins.insert(base_origin(world, *position));
        }
    }
    let generation = crate::network::world::assign_new_building_generation(world, pending.position);
    for origin in dynamic_origins {
        building_placement::remove_building_from_world(world, origin);
    }
    for origin in base_origins {
        world.base_buildings.remove(&origin);
        world.building_commands.remove(&origin);
    }

    let mut initial_config = pending.config.clone();
    if pending.block == FORCE_PROJECTOR_BLOCK {
        if initial_config.is_empty() {
            initial_config.push(1);
        } else {
            initial_config[0] = 1;
        }
    }
    world.tiles.insert(
        pending.position,
        DynamicTile {
            logic_control: None,
            payload_inventory: Vec::new(),
            position: pending.position,
            block: pending.block,
            rotation: pending.rotation,
            team: pending.team,
            config: initial_config,
            enabled: true,
            message: None,
            occupied: pending.occupied,
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: Vec::new(),
            power_stored: 0.0,
            power_links: Vec::new(),
            liquid_inventory: Vec::new(),
            stored_liquid: -1,
            liquid_amount: 0.0,
            output_liquid_amount: 0.0,
            junction_items: Vec::new(),
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 90.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: crate::game::content::block_health(pending.block),
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation,
        },
    );
    let placement_changes =
        building_placement::after_placement(world, pending.position, &pending.config);
    let final_config = world
        .tiles
        .get(&pending.position)
        .map(|tile| tile.config.clone())
        .unwrap_or_else(|| pending.config.clone());
    invalidate_navigation_for_block(world, pending.block);
    // The construction completed; the ghost plan disappears (official
    // BuildingComp.java:355-361 removes plans whose building is done). The
    // plan was registered under the placing player's team (`pending.team`).
    remove_team_plan(
        world,
        pending.team,
        (pending.position >> 16) as i16,
        pending.position as i16,
    );
    // Round 74e: the world loop persists dirty state every <=1 s
    // through the async PersistenceWorker — a synchronous
    // serde+fsync save here ran INSIDE the tick under
    // persistence_lock and stalled the world (tick_max up to
    // ~99 ms on the user's machine during builds).
    world.persistence_dirty.store(true, Ordering::Relaxed);
    let plan = BuildPlan {
        breaking: false,
        position: pending.position,
        block: pending.block,
        rotation: pending.rotation,
        config: final_config,
    };
    let payload = encode_construct_finish(&pending.builder, &plan, pending.rotation, pending.team)?;
    {
        // Official GameStats: friendly buildings fully built + placed count.
        let mut stats = world.game_state.game_stats.write();
        stats.buildings_built += 1;
        crate::state::game_state::GameStats::bump_block(
            &mut stats.placed_block_count,
            pending.block,
        );
    }
    // Register a freshly built core (339-344) so the team gains a real core:
    // projectiles hit it via damage_team_core, items route to its team and it
    // survives save/load (official TeamData.cores() adds it on build).
    if matches!(pending.block, 339..=344) {
        crate::network::world::register_team_core(
            world,
            pending.team,
            TeamCore {
                position: pending.position,
                block: pending.block,
                health: crate::game::content::block_health(pending.block),
                max_health: crate::game::content::block_health(pending.block),
            },
        );
    }
    out.broadcast_critical(frame_generated_packet(
        CONSTRUCT_FINISH_PACKET_ID,
        &payload,
        false,
    )?);
    broadcast_placement_power_configs(out, pending.builder.id, &placement_changes)?;
    Ok(())
}

pub(crate) fn schedule_break(world: &DynamicWorld, pending: &PendingBreak) {
    // SOL-003: deconstruction work is advanced by the world loop; this just
    // records the total work in game ticks (ALPHA_BUILD_SPEED, like builds).
    const ALPHA_BUILD_SPEED: f32 = 0.5;
    // ConstructBlock.deconstruct also finishes immediately under
    // state.rules.infiniteResources or instantBuild.
    let remaining = if construction_is_instant(world, pending.team) {
        0.0
    } else {
        let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
        (crate::game::content::block_build_time(pending.block) * cost_mult / ALPHA_BUILD_SPEED)
            .max(1.0)
    };
    if let Some(mut operation) = world.pending_breaks.get_mut(&pending.position) {
        operation.remaining_ticks = remaining;
    }
}

/// Advances every registered deconstruction by `delta` while the acting
/// player is active, completing through `finish_pending_break`.
pub(crate) fn simulate_breaks(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let keys: Vec<i32> = world
        .pending_breaks
        .iter()
        .map(|operation| *operation.key())
        .collect();
    let mut changed = false;
    let rules = world.wave_rules.read().clone();
    for key in keys {
        let mut ready = false;
        let Some((break_team, builder, last_seen)) =
            world.pending_breaks.get(&key).map(|operation| {
                (
                    operation.team,
                    operation.builder.clone(),
                    operation.last_seen,
                )
            })
        else {
            continue;
        };
        let infinite_now = construction_is_instant(world, break_team);
        let speed = delta_ticks.max(0.0) * rules.build_speed_for(break_team).max(0.0);
        let player_work = player_plan_work(world, &builder, key, last_seen, infinite_now, speed);
        if let Some(mut operation) = world.pending_breaks.get_mut(&key) {
            if infinite_now {
                operation.remaining_ticks = 0.0;
            }
            operation.remaining_ticks = (operation.remaining_ticks - player_work).max(0.0);
            changed = true;
            if operation.remaining_ticks <= 0.0 {
                ready = true;
            }
        }
        if ready {
            if let Some(pending) = world.pending_breaks.get(&key).map(|b| b.clone()) {
                if let Err(err) = finish_pending_break(world, out, pending) {
                    warn!("Could not finish pending deconstruction: {}", err);
                }
            }
        }
    }
    changed
}

pub(crate) fn finish_pending_break(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    pending: PendingBreak,
) -> std::io::Result<()> {
    record_finish_entered();
    // No persistence_lock here (callers hold it; parking_lot is not
    // reentrant — see finish_pending_build).
    if world.pending_breaks.remove(&pending.position).is_none()
        || effective_block(world, pending.position) != pending.block
    {
        return Ok(());
    }
    let removed_core = matches!(pending.block, 339..=344)
        .then(|| crate::network::world::core_team_at_position(world, pending.position));
    if pending.dynamic {
        building_placement::remove_building_from_world(world, pending.position);
        let original = base_block(world, pending.position);
        if original != 0 {
            let original_position = base_origin(world, pending.position);
            let occupied = block_footprint(world, original_position, original)
                .unwrap_or_else(|| vec![original_position]);
            world.tiles.entry(original_position).or_insert(DynamicTile {
                logic_control: None,
                payload_inventory: Vec::new(),
                position: original_position,
                block: 0,
                rotation: 0,
                team: 0,
                config: vec![0],
                enabled: true,
                message: None,
                occupied,
                stored_item: -1,
                stored_amount: 0,
                production_progress: 0.0,
                transport_progress: 0.0,
                ammo_units: 0.0,
                inventory: Vec::new(),
                power_stored: 0.0,
                power_links: Vec::new(),
                liquid_inventory: Vec::new(),
                stored_liquid: -1,
                liquid_amount: 0.0,
                output_liquid_amount: 0.0,
                junction_items: Vec::new(),
                mass_driver_incoming: Vec::new(),
                mass_driver_rotation: 90.0,
                mass_driver_waiting: Vec::new(),
                payload: None,
                payload_progress: 0.0,
                payload_rotation: 0.0,
                payload_accum: Vec::new(),
                health: f32::MAX,
                door_open: false,
                shield: 0.0,
                light_color: -1_900_545,
                memory: Vec::new(),
                duct_rec_dir: 0,
                unloader_offset: 0,
                conveyor_items: Vec::new(),
                factory_command: None,
                stack_state: 0,
                stack_link: -1,
                stack_cooldown: 0.0,
                generation: 0,
            });
        }
    } else {
        world
            .base_buildings
            .remove(&base_origin(world, pending.position));
        world.tiles.insert(
            pending.position,
            DynamicTile {
                logic_control: None,
                payload_inventory: Vec::new(),
                position: pending.position,
                block: 0,
                rotation: 0,
                team: 0,
                config: vec![0],
                enabled: true,
                message: None,
                occupied: pending.occupied.clone(),
                stored_item: -1,
                stored_amount: 0,
                production_progress: 0.0,
                transport_progress: 0.0,
                ammo_units: 0.0,
                inventory: Vec::new(),
                power_stored: 0.0,
                power_links: Vec::new(),
                liquid_inventory: Vec::new(),
                stored_liquid: -1,
                liquid_amount: 0.0,
                output_liquid_amount: 0.0,
                junction_items: Vec::new(),
                mass_driver_incoming: Vec::new(),
                mass_driver_rotation: 90.0,
                mass_driver_waiting: Vec::new(),
                payload: None,
                payload_progress: 0.0,
                payload_rotation: 0.0,
                payload_accum: Vec::new(),
                health: f32::MAX,
                door_open: false,
                shield: 0.0,
                light_color: -1_900_545,
                memory: Vec::new(),
                duct_rec_dir: 0,
                unloader_offset: 0,
                conveyor_items: Vec::new(),
                factory_command: None,
                stack_state: 0,
                stack_link: -1,
                stack_cooldown: 0.0,
                generation: 0,
            },
        );
    }
    if let Some(Some(team)) = removed_core {
        crate::network::world::unregister_team_core(world, team, pending.position);
        reregister_team_core(world, team);
    }
    crate::network::core_inventory::clamp_core_inventories(world);
    refund_requirements_for(world, pending.team, pending.block);
    // The broken block's ghost plan disappears (official InputHandler break
    // iterates team plans and removes the matching position).
    remove_team_plan(
        world,
        pending.team,
        (pending.position >> 16) as i16,
        pending.position as i16,
    );
    // Round 74e: the world loop persists dirty state every <=1 s
    // through the async PersistenceWorker — a synchronous
    // serde+fsync save here ran INSIDE the tick under
    // persistence_lock and stalled the world (tick_max up to
    // ~99 ms on the user's machine during builds).
    world.persistence_dirty.store(true, Ordering::Relaxed);
    let payload = encode_deconstruct_finish(&pending.builder, &pending)?;
    world.game_state.game_stats.write().buildings_deconstructed += 1;
    out.broadcast_critical(frame_generated_packet(
        DECONSTRUCT_FINISH_PACKET_ID,
        &payload,
        false,
    )?);
    record_finish_enqueued();
    Ok(())
}

pub(crate) fn session_builder_unit_id(player: &SessionPlayer) -> i32 {
    player
        .controlled_unit
        .standard_id()
        .unwrap_or(player.unit_id)
}

pub(crate) fn encode_begin_place(
    player: &SessionPlayer,
    pending: &PendingBuild,
) -> std::io::Result<Vec<u8>> {
    encode_begin_place_for_unit(
        session_builder_unit_id(player),
        pending.position,
        pending.block,
        pending.rotation,
        pending.team,
        &[0],
    )
}

pub(crate) fn encode_begin_place_for_unit(
    unit_id: i32,
    position: i32,
    block: i16,
    rotation: u8,
    team: u8,
    config: &[u8],
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    let mut payload = Vec::new();
    payload.write_b(2)?; // normal unit reference
    payload.write_i(unit_id)?;
    payload.write_s(block)?;
    payload.write_b(team)?;
    payload.write_i(x)?;
    payload.write_i(y)?;
    payload.write_i(i32::from(rotation))?;
    payload.extend_from_slice(&outbound_typeio_object(config));
    Ok(payload)
}

pub(crate) fn encode_begin_break(
    player: &SessionPlayer,
    position: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    let mut payload = Vec::new();
    payload.write_b(2)?;
    payload.write_i(session_builder_unit_id(player))?;
    payload.write_b(1)?;
    payload.write_i(x)?;
    payload.write_i(y)?;
    Ok(payload)
}

pub(crate) fn encode_deconstruct_finish(
    player: &SessionPlayer,
    pending: &PendingBreak,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let mut payload = Vec::new();
    payload.write_i(pending.position)?;
    payload.write_s(pending.block)?;
    payload.write_b(2)?;
    payload.write_i(session_builder_unit_id(player))?;
    Ok(payload)
}
