//! Unit-orders domain. The listener adapter re-exports these through
//! crate::network::listener::*.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::placement as building_placement;
use crate::network::combat::*;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::network::world::*;
use dashmap::DashMap;

use crate::network::buildings::construction::dynamic_at;
use crate::network::combat::enemy::{base_building_at, navigation_index, tile_is_leg_solid};
use crate::network::combat::unit_combat::{
    effective_unit_build_speed, effective_unit_speed, invalidate_navigation_for_block,
};
use crate::network::units::mining::choose_navigation_step;
use crate::network::wire::{
    broadcast_placement_power_configs, encode_block_snapshot, nearest_opposing_unit,
};

pub(crate) struct PathfindResult {
    pub should_move: bool,
    pub dest_x: f32,
    pub dest_y: f32,
    pub next: Option<(i32, i32)>,
    pub unreachable: bool,
}

pub(crate) fn ordered_unit_step(
    world: &DynamicWorld,
    unit: &EnemyUnit,
    target_x: f32,
    target_y: f32,
) -> Option<(f32, f32)> {
    let result = ordered_unit_path(world, unit, target_x, target_y);
    result.should_move.then_some((result.dest_x, result.dest_y))
}

/// Observable ControlPathfinder.getPathPosition / PathfindResult (v159.7).
pub(crate) fn ordered_unit_path(
    world: &DynamicWorld,
    unit: &EnemyUnit,
    target_x: f32,
    target_y: f32,
) -> PathfindResult {
    const IMPASSABLE: u32 = u32::MAX / 4;
    let flying = unit.elevation >= 0.09;
    let naval = matches!(unit.unit_type, 25..=29);
    if flying {
        return PathfindResult {
            should_move: true,
            dest_x: target_x,
            dest_y: target_y,
            next: None,
            unreachable: false,
        };
    }
    let legs = matches!(unit.unit_type, 11..=14);
    let start_x = (unit.x / 8.0).floor() as i32;
    let start_y = (unit.y / 8.0).floor() as i32;
    let goal_x = (target_x / 8.0).floor() as i32;
    let goal_y = (target_y / 8.0).floor() as i32;
    let Some(start) = navigation_index(world, start_x, start_y) else {
        return PathfindResult {
            should_move: false,
            dest_x: target_x,
            dest_y: target_y,
            next: None,
            unreachable: true,
        };
    };
    let Some(goal) = navigation_index(world, goal_x, goal_y) else {
        return PathfindResult {
            should_move: false,
            dest_x: target_x,
            dest_y: target_y,
            next: None,
            unreachable: true,
        };
    };
    if start == goal {
        return PathfindResult {
            should_move: true,
            dest_x: target_x,
            dest_y: target_y,
            next: None,
            unreachable: false,
        };
    }
    let passable_cost = |x: i32, y: i32, goal_cell: bool| {
        let Some(index) = navigation_index(world, x, y) else {
            return IMPASSABLE;
        };
        if goal_cell {
            return 1;
        }
        let floor_id = world.floors[index];
        let floor = crate::game::content::block_navigation(floor_id);
        let position = (x << 16) | (y as u16 as i32);
        let dynamic = dynamic_at(world, position);
        let base = base_building_at(world, position);
        let (block, team) = dynamic
            .as_ref()
            .map(|tile| (tile.block, tile.team))
            .or_else(|| {
                base.as_ref()
                    .map(|building| (building.block, building.team))
            })
            .unwrap_or((world.base_blocks[index], 0));
        if legs
            && tile_is_leg_solid(
                block,
                floor_id,
                world.tile_data.get(index).copied().unwrap_or(0),
            )
        {
            return IMPASSABLE;
        }
        let navigation = crate::game::content::block_navigation(block);
        if !legs && navigation.solid && !(team == unit.team && navigation.team_passable) {
            return IMPASSABLE;
        }
        1 + u32::from(floor.deep) * 6000 + u32::from(floor.damages) * 30
    };

    let total = (world.width * world.height).max(0) as usize;
    let mut costs = vec![IMPASSABLE; total];
    let mut pending = BinaryHeap::new();
    let heuristic = |x: i32, y: i32| (x - start_x).unsigned_abs() + (y - start_y).unsigned_abs();
    costs[goal] = 0;
    pending.push((Reverse(heuristic(goal_x, goal_y)), Reverse(0u32), goal));
    while let Some((_, Reverse(cost), index)) = pending.pop() {
        if cost != costs[index] {
            continue;
        }
        if index == start {
            break;
        }
        let x = index as i32 % world.width;
        let y = index as i32 / world.width;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            let Some(next) = navigation_index(world, nx, ny) else {
                continue;
            };
            let step = passable_cost(nx, ny, next == goal);
            if step >= IMPASSABLE {
                continue;
            }
            let candidate = cost.saturating_add(step).min(IMPASSABLE);
            if candidate < costs[next] {
                costs[next] = candidate;
                let priority = candidate.saturating_add(heuristic(nx, ny));
                pending.push((Reverse(priority), Reverse(candidate), next));
            }
        }
    }
    if costs[start] >= IMPASSABLE {
        return PathfindResult {
            should_move: false,
            dest_x: target_x,
            dest_y: target_y,
            next: None,
            unreachable: true,
        };
    }
    let Some(next) = choose_navigation_step(&costs, world.width, world.height, start) else {
        return PathfindResult {
            should_move: false,
            dest_x: target_x,
            dest_y: target_y,
            next: None,
            unreachable: true,
        };
    };
    let next_x = next as i32 % world.width;
    let next_y = next as i32 / world.width;
    let dest_x = (next_x as f32 + 0.5) * 8.0;
    let dest_y = (next_y as f32 + 0.5) * 8.0;
    let mut should_move = true;
    if naval {
        let can_pass_next = passable_cost(next_x, next_y, next == goal) < IMPASSABLE;
        let on_dest_tile =
            start_x == (dest_x / 8.0).floor() as i32 && start_y == (dest_y / 8.0).floor() as i32;
        if !can_pass_next && on_dest_tile {
            should_move = false;
        }
    }
    PathfindResult {
        should_move,
        dest_x,
        dest_y,
        next: Some((next_x, next_y)),
        unreachable: false,
    }
}

pub(crate) fn route_unit_movement(
    world: &DynamicWorld,
    snapshot: &EnemyUnit,
    target_x: f32,
    target_y: f32,
    delta_ticks: f32,
    stop_distance: f32,
) -> Option<(f32, f32, f32, f32, f32)> {
    let speed = effective_unit_speed(snapshot);
    let mut remaining = speed * delta_ticks.max(0.0);
    let mut routed = snapshot.clone();
    let mut velocity_x = 0.0;
    let mut velocity_y = 0.0;
    let mut rotation = routed.rotation;
    let maximum_steps = ((world.width + world.height).max(1) * 2) as usize;
    for _ in 0..maximum_steps {
        let target_distance = (target_x - routed.x).hypot(target_y - routed.y);
        if remaining <= 0.001 || target_distance <= stop_distance {
            break;
        }
        let (waypoint_x, waypoint_y) = ordered_unit_step(world, &routed, target_x, target_y)?;
        let dx = waypoint_x - routed.x;
        let dy = waypoint_y - routed.y;
        let distance = dx.hypot(dy);
        if distance <= 0.001 {
            break;
        }
        let step = remaining.min(distance).min(target_distance - stop_distance);
        velocity_x = dx / distance * speed;
        velocity_y = dy / distance * speed;
        routed.x += dx / distance * step;
        routed.y += dy / distance * step;
        rotation = dy.atan2(dx).to_degrees();
        remaining -= step;
        if step + 0.001 < distance {
            break;
        }
    }
    Some((routed.x, routed.y, velocity_x, velocity_y, rotation))
}

pub(crate) fn unit_mining(world: &DynamicWorld, id: i32) -> bool {
    world
        .unit_orders
        .get(&id)
        .map(|order| order.target_kind == 6)
        .unwrap_or(false)
}

/// Logic miner: moves to the mine target, drills the ore overlay into the
/// unit's item inventory (1 item/second, cap 2 carried like poly/mega).
pub(crate) fn unit_logic_firing(world: &DynamicWorld, id: i32) -> bool {
    world
        .unit_orders
        .get(&id)
        .map(|order| matches!(order.target_kind, 7 | 8))
        .unwrap_or(false)
}

/// Logic aim/fire: the bound unit stops, aims at the ordered point and (for
/// target_kind 7) fires its weapons toward it.
pub(crate) fn unit_logic_building(world: &DynamicWorld, id: i32) -> bool {
    world
        .unit_orders
        .get(&id)
        .map(|order| order.target_kind == 9)
        .unwrap_or(false)
}

/// Logic builder: moves to the site, accumulates progress and places the
/// building (authoritative BlockSnapshot broadcast).
pub(crate) fn clear_logic_build_order(world: &DynamicWorld, unit_id: i32) {
    if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
        order.target_kind = 0;
        order.target_x = None;
        order.target_y = None;
        order.target_id = -1;
    }
}

/// Places a logic-built building: inserts the tile and broadcasts the final
/// BlockSnapshot to every client (same layout as player builds).
pub(crate) fn place_logic_building(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    position: i32,
    block: i16,
    rotation: u8,
    occupied: Vec<i32>,
) {
    let generation = crate::network::world::assign_new_building_generation(world, position);
    world.tiles.insert(
        position,
        DynamicTile {
            position,
            block,
            rotation,
            team: 1,
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
            health: crate::game::content::block_health(block),
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
    let placement_changes = building_placement::after_placement(world, position, &[0]);
    invalidate_navigation_for_block(world, block);
    if let Some(tile) = world.tiles.get(&position) {
        let power = std::collections::HashMap::new();
        // dashmap-guard: allow DM900 reason="encode_block_snapshot serializes the already-borrowed tile and performs no exclusive world.tiles operation"
        if let Ok(Some(frame)) = encode_block_snapshot(world, &tile, &power) {
            out.broadcast(frame);
        }
    }
    let actor_id = world
        .player_sessions
        .iter()
        .next()
        .map(|session| session.id)
        .unwrap_or(1);
    let _ = broadcast_placement_power_configs(out, actor_id, &placement_changes);
}

pub(crate) fn apply_ordered_unit_movement(
    world: &DynamicWorld,
    snapshot: &EnemyUnit,
    delta_ticks: f32,
) -> bool {
    // CommandAI.updateUnit's invalidation sweep (CommandAI.java:136-139)
    // runs every update, before anything consumes a target: queued Healthc
    // entries (buildings, units) that are no longer valid leave the queue.
    prune_invalid_queued_targets(world, snapshot.id);
    let Some(order) = world
        .unit_orders
        .get(&snapshot.id)
        .map(|order| order.clone())
    else {
        return false;
    };
    if order.command != 0 {
        return false;
    }
    let (mut target_x, mut target_y) = match (order.target_x, order.target_y) {
        (Some(x), Some(y)) => (x, y),
        _ => return false,
    };
    if order.target_kind == 2 {
        // Read the target's team through a short-lived guard: advancing the
        // order may reset the unit's authority, which takes an `enemies`
        // write lock below.
        let target_team = world
            .enemies
            .get(&order.target_id)
            .map(|target| target.team);
        if target_team == Some(snapshot.team) {
            advance_unit_order(world, snapshot.id);
            return true;
        }
        let Some(target) = world.enemies.get(&order.target_id) else {
            advance_unit_order(world, snapshot.id);
            return true;
        };
        target_x = target.x;
        target_y = target.y;
        let within_range = !unit_has_stance(world, snapshot.id, 4)
            && (target_x - snapshot.x).hypot(target_y - snapshot.y) <= snapshot.attack_range
            && snapshot.attack_damage > 0.0;
        drop(target);
        if within_range {
            if let Some(mut live_order) = world.unit_orders.get_mut(&snapshot.id) {
                live_order.target_x = Some(target_x);
                live_order.target_y = Some(target_y);
            }
            return false;
        }
    }
    if order.target_kind == 1 && !order_target_building_exists(world, order.target_id) {
        advance_unit_order(world, snapshot.id);
        return true;
    }
    let dx = target_x - snapshot.x;
    let dy = target_y - snapshot.y;
    let distance = dx.hypot(dy);
    if !unit_has_stance(world, snapshot.id, 4)
        && order.target_kind == 1
        && ordered_opposing_building(world, snapshot).is_some()
        && distance <= snapshot.attack_range
        && snapshot.attack_damage > 0.0
    {
        return false;
    }
    let stop_distance = if unit_has_stance(world, snapshot.id, 4) {
        1.0
    } else {
        10.0
    };
    if distance <= stop_distance {
        // Reached the destination — official `finishPath()`
        // (CommandAI.java:391-396): the active target is dropped and the
        // next queue entry promoted; with an exhausted queue the unit ends
        // up with NO command (hasCommand() false, logic-controllable
        // again). This deliberately fires for an empty queue too.
        advance_unit_order(world, snapshot.id);
        return true;
    }
    let routed = route_unit_movement(
        world,
        snapshot,
        target_x,
        target_y,
        delta_ticks,
        stop_distance,
    );
    let Some(mut unit) = world.enemies.get_mut(&snapshot.id) else {
        return false;
    };
    if distance <= stop_distance {
        unit.velocity_x = 0.0;
        unit.velocity_y = 0.0;
    } else if let Some((x, y, velocity_x, velocity_y, rotation)) = routed {
        unit.x = x;
        unit.y = y;
        unit.velocity_x = velocity_x;
        unit.velocity_y = velocity_y;
        unit.rotation = rotation;
    } else {
        unit.velocity_x = 0.0;
        unit.velocity_y = 0.0;
    }
    true
}

pub(crate) fn order_target_building_exists(world: &DynamicWorld, position: i32) -> bool {
    dynamic_at(world, position).is_some_and(|tile| tile.block != 0)
        || base_building_at(world, position).is_some()
}

pub(crate) fn ordered_opposing_building(
    world: &DynamicWorld,
    unit: &EnemyUnit,
) -> Option<(i32, f32, f32)> {
    let order = world.unit_orders.get(&unit.id)?;
    if order.target_kind != 1 {
        return None;
    }
    let position = order.target_id;
    drop(order);
    if let Some(tile) = dynamic_at(world, position) {
        return (tile.block != 0 && tile.team != unit.team).then_some((
            tile.position,
            (tile.position >> 16) as i16 as f32 * 8.0,
            tile.position as i16 as f32 * 8.0,
        ));
    }
    base_building_at(world, position).and_then(|building| {
        (building.team != unit.team).then_some((
            building.position,
            (building.position >> 16) as i16 as f32 * 8.0,
            building.position as i16 as f32 * 8.0,
        ))
    })
}

/// `CommandAI.finishPath()` restricted to the queue bookkeeping the port
/// models (CommandAI.java:412-486): drops the active target, pops the next
/// queue entry into it (Teamc -> `commandTarget`, Vec2 -> `commandPosition`,
/// CommandAI.java:465-471) and — while PATROLLING (stance bit 3) — re-queues
/// the previous targetPos as a plain POSITION (CommandAI.java:473-475:
/// `commandQueue.add(prev.cpy())` where `prev` is the Vec2 targetPos, never
/// the attack target).
pub(crate) fn advance_unit_order(world: &DynamicWorld, unit_id: i32) {
    let Some(mut order) = world.unit_orders.get_mut(&unit_id) else {
        return;
    };
    if order.queue.is_empty() {
        // Exhausted: `finishPath` leaves targetPos/attackTarget null, so
        // hasCommand() becomes false. Only RTS kinds release Command
        // authority; logic kinds 6-9 belong to the (later) LogicAI model
        // and stay untouched.
        let was_active_rts =
            order.target_kind <= 2 && (order.target_x.is_some() || order.target_id >= 0);
        clear_order_active_target(&mut order);
        drop(order);
        if was_active_rts {
            release_command_control(world, unit_id);
        }
        return;
    }
    if order.stances & (1_u32 << 3) != 0 {
        if let (Some(x), Some(y)) = (order.target_x, order.target_y) {
            // prev is targetPos — a Vec2 — so the re-queued entry is a
            // position target, not the original attack target.
            order.queue.push(UnitOrderTarget {
                kind: 0,
                id: -1,
                x,
                y,
            });
        }
    }
    let next = order.queue.remove(0);
    set_order_active_target(&mut order, next);
}

/// `CommandAI.updateUnit`'s per-update queue invalidation
/// (CommandAI.java:136-139): `commandQueue.removeAll(e -> e instanceof
/// Healthc h && !h.isValid())`. Buildings (kind 1) are valid while their
/// tile still holds the building (`Building.isValid`), units (kind 2) while
/// alive; queued positions (kind 0) are plain Vec2s and never expire.
/// Lazily called from [`apply_ordered_unit_movement`] — the Rust
/// counterpart of the CommandAI update tick.
///
/// Guard discipline (project rule): the validity decision touches
/// `world.tiles`/`world.enemies`, so it runs WITHOUT any `unit_orders`
/// guard. Pass 1 copies the queue out under a short read guard, pass 2
/// decides which entries died, pass 3 re-acquires a short write guard and
/// removes exactly those entries — concurrent queue appends survive.
pub(crate) fn prune_invalid_queued_targets(world: &DynamicWorld, unit_id: i32) {
    // Pass 1: short-lived read guard, copy the queue out.
    let queue = world
        .unit_orders
        .get(&unit_id)
        .map(|order| order.queue.clone())
        .unwrap_or_default();
    // Pass 2: no guards held — inspect tiles and enemies.
    let invalid: Vec<(u8, i32)> = queue
        .iter()
        .filter_map(|target| match target.kind {
            1 => (!order_target_building_exists(world, target.id)).then_some((1, target.id)),
            2 => {
                let alive = world
                    .enemies
                    .get(&target.id)
                    .is_some_and(|unit| unit.health > 0.0);
                (!alive).then_some((2, target.id))
            }
            _ => None,
        })
        .collect();
    if invalid.is_empty() {
        return;
    }
    // Pass 3: short-lived write guard; remove only the entries found dead.
    if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
        order
            .queue
            .retain(|target| !invalid.contains(&(target.kind, target.id)));
    }
}

pub(crate) fn unit_has_stance(world: &DynamicWorld, unit_id: i32, stance: u8) -> bool {
    world
        .unit_orders
        .get(&unit_id)
        .is_some_and(|order| order.stances & (1_u32 << stance) != 0)
}

pub(crate) fn boost_should_land_near_target(world: &DynamicWorld, unit: &EnemyUnit) -> bool {
    if let Some(order) = world.unit_orders.get(&unit.id) {
        let target = match order.target_kind {
            // dashmap-guard: allow DM900 reason="ordered_opposing_building does not mutate unit_orders while this shared order guard is live"
            1 => ordered_opposing_building(world, unit).map(|(_, x, y)| (x, y)),
            2 => world
                .enemies
                .get(&order.target_id)
                .and_then(|target| (target.team != unit.team).then_some((target.x, target.y))),
            _ if order.stances & (1_u32 << 3) != 0 => order.target_x.zip(order.target_y),
            _ => None,
        };
        if let Some((x, y)) = target {
            return (x - unit.x).hypot(y - unit.y) <= unit.attack_range;
        }
    }
    nearest_opposing_unit(world, unit.team, unit.x, unit.y)
        .is_some_and(|(_, x, y)| (x - unit.x).hypot(y - unit.y) <= unit.attack_range)
}

pub(crate) const BUILDER_RANGE: f32 = 220.0;

pub(crate) fn builder_unit_hit_size(unit_type: i16) -> Option<f32> {
    match unit_type {
        5 => Some(8.0),
        6 => Some(11.0),
        7 => Some(13.0),
        8 => Some(24.0),
        21 => Some(9.0),
        22 => Some(16.05),
        23 => Some(36.0),
        24 => Some(66.0),
        _ => None,
    }
}

pub(crate) fn unit_build_speed(unit_type: i16) -> Option<f32> {
    match unit_type {
        5 => Some(0.3),
        6 | 21 => Some(0.5),
        7 => Some(1.1),
        8 => Some(3.0),
        22 => Some(2.6),
        23 => Some(2.5),
        24 => Some(4.0),
        35 => Some(0.5),  // alpha
        36 => Some(0.75), // beta
        37 => Some(1.0),  // gamma
        _ => None,
    }
}

/// Official `UnitType.buildRange` — vanilla 158.1 never overrides the
/// default `Vars.buildingRange` (220).
pub(crate) fn unit_build_range(_unit_type: i16) -> f32 {
    BUILDER_RANGE
}

pub(crate) fn unit_plan_world(position: i32) -> (f32, f32) {
    (
        (position >> 16) as i16 as f32 * 8.0,
        position as i16 as f32 * 8.0,
    )
}

pub(crate) fn unit_within_build_range(unit: &EnemyUnit, position: i32) -> bool {
    let (x, y) = unit_plan_world(position);
    (unit.x - x).hypot(unit.y - y) <= unit_build_range(unit.unit_type)
}

/// Remaining-tick work for one constructing unit. `PendingBuild.remaining_ticks`
/// is initialized as `buildTime / 0.5 / teamSpeed`, so a 0.5-speed unit
/// contributes `delta` per tick (Java `type.buildSpeed / buildCost`).
pub(crate) fn unit_construction_work(unit: &EnemyUnit, delta_ticks: f32) -> f32 {
    effective_unit_build_speed(unit).unwrap_or(0.0) / 0.5 * delta_ticks.max(0.0)
}

pub(crate) fn unit_has_place_plan(unit: &EnemyUnit, position: i32) -> bool {
    unit.build_plans
        .iter()
        .any(|plan| !plan.breaking && plan.position == position)
}

use crate::network::units::*;

use crate::network::combat::*;
use crate::network::economy::*;
