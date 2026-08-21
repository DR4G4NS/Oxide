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
/// Returns the network template with the CURRENT live team build plans
/// spliced into the team-blocks section. The official server writes the
/// current `TeamData.plans` of every active team on each connection, so a
/// late joiner must see ghost builds that appeared after hosting, not just
/// the plans baked into the template at host time.
///
/// NOTE: must NOT take `persistence_lock` — `host_map` re-streams while
/// already holding it (parking_lot mutexes are not reentrant). The plans are
pub(crate) fn network_template_with_plans(world: &DynamicWorld) -> std::io::Result<Arc<Vec<u8>>> {
    let plans = world.team_build_plans.read().clone();
    let patched =
        crate::engine::world_stream::replace_team_blocks(&world.network_template, &plans)?;
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
    let current: HashSet<_> = plans
        .iter()
        .map(|plan| (plan.breaking, plan.position, plan.block))
        .collect();
    player.active_plans.retain(|key| current.contains(key));
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
                .is_some_and(|tile| tile.block == plan.block);
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
            if world.pending_builds.iter().any(|build| {
                build.position == plan.position || build.occupied.contains(&plan.position)
            }) || world.pending_breaks.iter().any(|pending| {
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
                let payload = encode_begin_break(player, origin)?;
                out.broadcast(frame_generated_packet(
                    BEGIN_BREAK_PACKET_ID,
                    &payload,
                    false,
                )?);
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
            let placing_team = world
                .players
                .get(&player.unit_id)
                .map(|combat| combat.team)
                .unwrap_or(1);
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
            let rotation = plan.rotation % 4;
            let pending = PendingBuild {
                position: plan.position,
                block: plan.block,
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
            let payload = encode_begin_place(player, &pending)?;
            out.broadcast(frame_generated_packet(
                BEGIN_PLACE_PACKET_ID,
                &payload,
                false,
            )?);
            schedule_build(world, &pending);
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

/// Restream payload produced by the transactional `SetMode` path (P0-6):
/// the shared WorldDataBegin frame plus one personalized world stream per
/// connected player id.
pub(crate) type ModeRestream = (Vec<u8>, Vec<(i32, Vec<u8>)>);

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
    if *state.mode.read() == GameMode::Sandbox {
        return true;
    }
    // Official ConstructBlock: infiniteResources builds without requiring or
    // consuming core items (`progress >= 1f || state.rules.infiniteResources`).
    if state.infinite_resources.load(Ordering::Relaxed)
        || rules
            .map(|rules| rules.team_rule(team).infinite_resources)
            .unwrap_or(false)
    {
        return true;
    }
    let requirements = crate::game::content::block_requirements(block);
    let mut items = if team == 1 {
        TeamItemsMut::Legacy(state.core_items.write())
    } else {
        TeamItemsMut::Team(state.team_items.entry(team).or_insert_with(|| vec![0; 22]))
    };
    if requirements
        .iter()
        .any(|(item, amount)| items.get(*item).is_none_or(|stored| stored < amount))
    {
        return false;
    }
    for (item, amount) in requirements {
        items[*item] -= amount;
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
    if *world.game_state.mode.read() == GameMode::Sandbox
        || world.game_state.infinite_resources.load(Ordering::Relaxed)
        || rules.infinite_resources
        || rules.team_rule(team).infinite_resources
    {
        return;
    }
    drop(rules);
    for (item, amount) in crate::game::content::block_requirements(block) {
        crate::network::core_inventory::deposit_core_items(
            world,
            team,
            *item as i16,
            (amount + 1) / 2,
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
    if *state.mode.read() == GameMode::Sandbox
        || state.infinite_resources.load(Ordering::Relaxed)
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
            *stored = stored.saturating_add((amount + 1) / 2);
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
    occupied.iter().all(|position| {
        let existing = effective_block(world, *position);
        let existing_team = effective_building_team(world, *position);
        existing == 0
            || ((existing_team == team || existing_team == 0)
                && crate::game::content::block_can_replace(new_block, existing))
    })
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
    let rules = &world.wave_rules.read();
    // P0-5: Rules.buildSpeed(team) = global buildSpeedMultiplier * the
    // team's TeamRule.buildSpeedMultiplier (Rules.java:327).
    let build_speed = rules.build_speed_for(pending.team).max(0.0001);
    // ConstructBlock.construct completes when progress reaches 1 OR
    // state.rules.infiniteResources. Sandbox sets infiniteResources without
    // setting instantBuild, so checking instantBuild alone introduced the
    // visible multi-second "ghost build" regression.
    // ConstructBlock.construct (desktop 158.1 bytecode offsets 87-117):
    // `team.rules().infiniteResources || state.rules.infiniteResources`.
    let infinite = rules.infinite_resources
        || world.game_state.infinite_resources.load(Ordering::Relaxed)
        || rules.team_rule(pending.team).infinite_resources;
    let remaining = if rules.instant_build || infinite {
        0.0
    } else {
        (crate::game::content::block_build_time(pending.block) / ALPHA_BUILD_SPEED / build_speed)
            .max(1.0)
    };
    if let Some(mut build) = world.pending_builds.get_mut(&pending.position) {
        build.remaining_ticks = remaining;
    }
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
    let now = std::time::Instant::now();
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
        if let Some(mut build) = world.pending_builds.get_mut(&key) {
            // Core-player plans never live on an EnemyUnit. They still use
            // the last_seen window. As soon as a unit queue owns the tile,
            // only in-range `updateBuilding` units advance it.
            let active = now.duration_since(build.last_seen) <= std::time::Duration::from_secs(5);
            let mut work = if any_unit_plan {
                unit_work
            } else if active {
                delta_ticks.max(0.0)
            } else {
                0.0
            };
            let assist_work = (build.assist_progress - build.applied_assist).max(0.0);
            build.applied_assist += assist_work;
            work += assist_work;
            build.remaining_ticks = (build.remaining_ticks - work).max(0.0);
            changed = true;
            if build.remaining_ticks <= 0.0 {
                ready = true;
            }
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
        let mut payload = Vec::new();
        crate::network::codec::Writes::write_i(&mut payload, pending.position)?;
        out.broadcast(frame_generated_packet(
            REMOVE_TILE_PACKET_ID,
            &payload,
            false,
        )?);
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
    out.broadcast(frame_generated_packet(
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
    // state.rules.infiniteResources; no refund is emitted in that mode.
    let rules = world.wave_rules.read();
    let infinite = rules.infinite_resources
        || world.game_state.infinite_resources.load(Ordering::Relaxed)
        || rules.team_rule(pending.team).infinite_resources;
    let remaining = if infinite {
        0.0
    } else {
        (crate::game::content::block_build_time(pending.block) / ALPHA_BUILD_SPEED).max(1.0)
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
    let now = std::time::Instant::now();
    for key in keys {
        let mut ready = false;
        if let Some(mut operation) = world.pending_breaks.get_mut(&key) {
            let active =
                now.duration_since(operation.last_seen) <= std::time::Duration::from_secs(5);
            if active {
                operation.remaining_ticks =
                    (operation.remaining_ticks - delta_ticks.max(0.0)).max(0.0);
                changed = true;
                if operation.remaining_ticks <= 0.0 {
                    ready = true;
                }
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
    out.broadcast(frame_generated_packet(
        DECONSTRUCT_FINISH_PACKET_ID,
        &payload,
        false,
    )?);
    Ok(())
}

pub(crate) fn encode_begin_place(
    player: &SessionPlayer,
    pending: &PendingBuild,
) -> std::io::Result<Vec<u8>> {
    encode_begin_place_for_unit(
        player.unit_id,
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
    payload.write_i(player.unit_id)?;
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
    payload.write_i(player.unit_id)?;
    Ok(payload)
}
