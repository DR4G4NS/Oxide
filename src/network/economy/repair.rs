//! Repair points/turrets/towers, build-tower construction assist, and
//! Erekir cargo loader/unload-point logistics (audit H3 / M1).

use super::spec::{inventory_add, inventory_count, inventory_remove, inventory_total};
use super::support::building_time_scale;
use crate::game::content::{block_size, unit_movement};
use crate::network::combat::enemy::enemy_max_health;
use crate::network::units::spawn_unit_world;
use crate::network::wire::encode::encode_assembler_drone_spawned_frame;
use crate::network::world::*;
use std::collections::HashMap;

const REPAIR_POINT: i16 = 384;
const REPAIR_TURRET: i16 = 385;
const UNIT_REPAIR_TOWER: i16 = 397;
const BUILD_TOWER: i16 = 252;
const CARGO_LOADER: i16 = 281;
const CARGO_UNLOAD: i16 = 282;
const MANIFOLD: i16 = 62;

const REPAIR_POINT_RADIUS: f32 = 60.0;
const REPAIR_POINT_SPEED: f32 = 0.45;
const REPAIR_TURRET_RADIUS: f32 = 145.0;
const REPAIR_TURRET_SPEED: f32 = 3.0;
const UNIT_REPAIR_TOWER_RANGE: f32 = 100.0;
const UNIT_REPAIR_TOWER_HEAL: f32 = 1.5;
const BUILD_TOWER_RANGE: f32 = 200.0;
const BUILD_TOWER_SPEED: f32 = 1.5;
const CARGO_BUILD_TIME: f32 = 60.0 * 8.0;
const CARGO_LOADER_CAPACITY: i32 = 200;
const CARGO_UNLOAD_CAPACITY: i32 = 100;
const CARGO_TRANSFER_RANGE: f32 = 20.0;
const CARGO_MOVE_RANGE: f32 = 6.0;
const CARGO_ITEM_CAPACITY: i32 = 100;
const CARGO_STALE_TIME: f32 = 60.0 * 6.0;
const ASSEMBLY_DRONE: i16 = 63;
const ASSEMBLER_BLOCKS: std::ops::RangeInclusive<i16> = 393..=395;
const ASSEMBLER_DRONES_CREATED: usize = 4;
const ASSEMBLER_DRONE_CONSTRUCT_TIME: f32 = 60.0 * 4.0;
const ASSEMBLER_AREA_SIZE: f32 = 13.0;
const ASSEMBLER_AI_MOVE_RANGE: f32 = 1.0;
const ASSEMBLER_AI_LOOK_RANGE: f32 = 5.0;

fn tile_center(position: i32, block: i16) -> (f32, f32) {
    let tx = (position >> 16) as i16 as f32;
    let ty = position as i16 as f32;
    let size = f32::from(block_size(block));
    let extra = ((size as i32 + 1) % 2) as f32 * 4.0;
    (tx * 8.0 + 4.0 + extra, ty * 8.0 + 4.0 + extra)
}

fn efficiency_at(power: &HashMap<i32, f32>, position: i32) -> f32 {
    power.get(&position).copied().unwrap_or(0.0).clamp(0.0, 1.0)
}

/// RepairTurret (384/385): heal the nearest damaged allied unit in radius.
pub(crate) fn simulate_repair_points(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &HashMap<i32, f32>,
) -> bool {
    let keys: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, REPAIR_POINT | REPAIR_TURRET) && tile.enabled)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let efficiency = efficiency_at(power, key);
        if efficiency <= 0.0 {
            continue;
        }
        let (radius, speed) = if snapshot.block == REPAIR_TURRET {
            (REPAIR_TURRET_RADIUS, REPAIR_TURRET_SPEED)
        } else {
            (REPAIR_POINT_RADIUS, REPAIR_POINT_SPEED)
        };
        let (cx, cy) = tile_center(snapshot.position, snapshot.block);
        let scale = building_time_scale(world, key);
        let mut best: Option<(i32, f32)> = None;
        for unit in world.enemies.iter() {
            if unit.team != snapshot.team {
                continue;
            }
            let max_hp = enemy_max_health(&unit);
            if unit.health + 0.001 >= max_hp {
                continue;
            }
            let dist = (unit.x - cx).hypot(unit.y - cy);
            if dist - unit_movement(unit.unit_type).hit_size / 2.0 > radius {
                continue;
            }
            if best.is_none_or(|(_, best_dist)| dist < best_dist) {
                best = Some((unit.id, dist));
            }
        }
        let Some((target_id, _)) = best else {
            continue;
        };
        let heal = speed * efficiency * delta_ticks.max(0.0) * scale;
        if let Some(mut unit) = world.enemies.get_mut(&target_id) {
            let max_hp = enemy_max_health(&unit);
            let next = (unit.health + heal).min(max_hp);
            if next > unit.health {
                unit.health = next;
                changed = true;
            }
        }
        let rotation = world
            .enemies
            .get(&target_id)
            .map(|unit| (unit.y - cy).atan2(unit.x - cx).to_degrees());
        if let (Some(rotation), Some(mut tile)) = (rotation, world.tiles.get_mut(&key)) {
            // Rotation is saved as a float degrees tail (RepairTurret rev 1).
            tile.mass_driver_rotation = rotation;
        }
    }
    changed
}

/// RepairTower (397): heal every damaged allied unit in range each tick.
pub(crate) fn simulate_unit_repair_towers(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &HashMap<i32, f32>,
) -> bool {
    let keys: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == UNIT_REPAIR_TOWER && tile.enabled)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let efficiency = efficiency_at(power, key);
        if efficiency <= 0.0 {
            continue;
        }
        let (cx, cy) = tile_center(snapshot.position, snapshot.block);
        let scale = building_time_scale(world, key);
        let heal = UNIT_REPAIR_TOWER_HEAL * efficiency * delta_ticks.max(0.0) * scale;
        let targets: Vec<i32> = world
            .enemies
            .iter()
            .filter(|unit| {
                unit.team == snapshot.team
                    && (unit.x - cx).hypot(unit.y - cy) <= UNIT_REPAIR_TOWER_RANGE
                    && unit.health + 0.001 < enemy_max_health(unit)
            })
            .map(|unit| unit.id)
            .collect();
        for id in targets {
            if let Some(mut unit) = world.enemies.get_mut(&id) {
                let max_hp = enemy_max_health(&unit);
                let next = (unit.health + heal).min(max_hp);
                if next > unit.health {
                    unit.health = next;
                    changed = true;
                }
            }
        }
    }
    changed
}

/// BuildTurret (252): contribute construction work to in-range pending builds
/// of the same team (`buildSpeed` 1.5, range 200).
pub(crate) fn simulate_build_towers(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &HashMap<i32, f32>,
) -> bool {
    let keys: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == BUILD_TOWER && tile.enabled)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let efficiency = efficiency_at(power, key);
        if efficiency <= 0.0 {
            continue;
        }
        let (cx, cy) = tile_center(snapshot.position, snapshot.block);
        let scale = building_time_scale(world, key);
        // PendingBuild.remaining_ticks is initialized so a 0.5-speed builder
        // contributes `delta` per tick (unit_construction_work).
        let work = BUILD_TOWER_SPEED / 0.5 * efficiency * delta_ticks.max(0.0) * scale;
        let pending: Vec<i32> = world
            .pending_builds
            .iter()
            .filter(|build| {
                if build.team != snapshot.team {
                    return false;
                }
                let bx = ((build.position >> 16) as i16 as f32) * 8.0;
                let by = (build.position as i16 as f32) * 8.0;
                (bx - cx).hypot(by - cy) <= BUILD_TOWER_RANGE
            })
            .map(|build| *build.key())
            .collect();
        for position in pending {
            if let Some(mut build) = world.pending_builds.get_mut(&position) {
                build.remaining_ticks = (build.remaining_ticks - work).max(0.0);
                changed = true;
            }
        }
        // BuildTurret's hidden builder unit also repairs damaged allied
        // buildings in range (Unit.updateBuildLogic -> repair).
        let damaged: Vec<i32> = world
            .tiles
            .iter()
            .filter(|tile| {
                if tile.team != snapshot.team || tile.health <= 0.0 {
                    return false;
                }
                let max = crate::game::content::block_health(tile.block);
                if tile.health + 0.001 >= max {
                    return false;
                }
                let (bx, by) = tile_center(tile.position, tile.block);
                (bx - cx).hypot(by - cy) <= BUILD_TOWER_RANGE
            })
            .map(|tile| *tile.key())
            .collect();
        for position in damaged {
            if let Some(mut tile) = world.tiles.get_mut(&position) {
                let max = crate::game::content::block_health(tile.block);
                let next = (tile.health + work * 0.5).min(max);
                if next > tile.health {
                    tile.health = next;
                    changed = true;
                }
            }
        }
    }
    changed
}

/// UnitCargoLoader (281): construct a tethered manifold after `unitBuildTime`.
pub(crate) fn simulate_cargo_loaders(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &HashMap<i32, f32>,
) -> bool {
    let keys: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == CARGO_LOADER && tile.enabled)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let tethered = snapshot.stored_amount;
        if tethered > 0 && world.enemies.contains_key(&tethered) {
            continue;
        }
        let efficiency = efficiency_at(power, key);
        if efficiency <= 0.0 {
            continue;
        }
        let scale = building_time_scale(world, key);
        let next = snapshot.production_progress
            + efficiency * delta_ticks.max(0.0) * scale / CARGO_BUILD_TIME;
        if next < 1.0 {
            if let Some(mut tile) = world.tiles.get_mut(&key) {
                tile.production_progress = next;
                tile.stored_amount = -1;
            }
            changed = true;
            continue;
        }
        let (cx, cy) = tile_center(snapshot.position, snapshot.block);
        let spawned = spawn_unit_world(world, MANIFOLD, snapshot.team, cx, cy, 90.0);
        if let Some(mut tile) = world.tiles.get_mut(&key) {
            tile.production_progress = 0.0;
            tile.stored_amount = spawned.unwrap_or(-1);
        }
        changed = true;
    }
    changed
}

/// UnitCargoUnloadPoint (282): mark stale when full for `staleTime`.
pub(crate) fn simulate_cargo_unload_points(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let keys: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == CARGO_UNLOAD)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let total = inventory_total(&snapshot.inventory);
        if total < CARGO_UNLOAD_CAPACITY {
            if snapshot.ammo_units > 0.0 || snapshot.transport_progress > 0.0 {
                if let Some(mut tile) = world.tiles.get_mut(&key) {
                    tile.ammo_units = 0.0;
                    tile.transport_progress = 0.0;
                }
                changed = true;
            }
            continue;
        }
        let timer = snapshot.transport_progress + delta_ticks.max(0.0);
        if let Some(mut tile) = world.tiles.get_mut(&key) {
            tile.transport_progress = timer;
            tile.ammo_units = if timer >= CARGO_STALE_TIME { 1.0 } else { 0.0 };
        }
        changed = true;
    }
    changed
}

/// CargoAI for manifold units tethered to a cargo loader.
pub(crate) fn simulate_cargo_units(world: &DynamicWorld, delta_ticks: f32) -> bool {
    #[allow(clippy::type_complexity)]
    let loaders: Vec<(i32, u8, f32, f32, Vec<(i16, i32)>)> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == CARGO_LOADER && tile.stored_amount > 0)
        .map(|tile| {
            let (x, y) = tile_center(tile.position, tile.block);
            (tile.stored_amount, tile.team, x, y, tile.inventory.clone())
        })
        .collect();
    let unloads: Vec<(i32, u8, i16, bool, f32, f32, i32)> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == CARGO_UNLOAD)
        .map(|tile| {
            let (x, y) = tile_center(tile.position, tile.block);
            let item = configured_unload_item(&tile);
            (
                tile.position,
                tile.team,
                item,
                tile.ammo_units > 0.5,
                x,
                y,
                inventory_total(&tile.inventory),
            )
        })
        .collect();
    let mut changed = false;
    for (unit_id, team, lx, ly, loader_items) in loaders {
        let Some(snapshot) = world.enemies.get(&unit_id).map(|unit| unit.clone()) else {
            continue;
        };
        if snapshot.unit_type != MANIFOLD || snapshot.team != team {
            continue;
        }
        let carried = snapshot.items.iter().find(|(_, n)| *n > 0).copied();
        if let Some((item, amount)) = carried {
            let target = unloads
                .iter()
                .filter(|entry| entry.1 == team && entry.2 == item)
                .min_by(|a, b| {
                    let da = (a.4 - snapshot.x).hypot(a.5 - snapshot.y);
                    let db = (b.4 - snapshot.x).hypot(b.5 - snapshot.y);
                    match (a.3, b.3) {
                        (false, true) => std::cmp::Ordering::Less,
                        (true, false) => std::cmp::Ordering::Greater,
                        _ => da.total_cmp(&db),
                    }
                });
            let Some(&(pos, _, _, _, tx, ty, stored)) = target else {
                if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
                    unit.items.clear();
                    changed = true;
                }
                continue;
            };
            changed |= move_unit_towards(world, unit_id, tx, ty, delta_ticks);
            let dist = {
                let unit = world.enemies.get(&unit_id).unwrap();
                (unit.x - tx).hypot(unit.y - ty)
            };
            if dist <= CARGO_TRANSFER_RANGE {
                let space = (CARGO_UNLOAD_CAPACITY - stored).max(0);
                let deposited = amount.min(space);
                if deposited > 0 {
                    if let Some(mut tile) = world.tiles.get_mut(&pos) {
                        inventory_add(&mut tile.inventory, item, deposited);
                        tile.ammo_units = 0.0;
                        tile.transport_progress = 0.0;
                    }
                    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
                        if deposited >= amount {
                            unit.items.clear();
                        } else if let Some((_, held)) =
                            unit.items.iter_mut().find(|(held, _)| *held == item)
                        {
                            *held -= deposited;
                        }
                    }
                    changed = true;
                }
            }
        } else {
            changed |= move_unit_towards(world, unit_id, lx, ly, delta_ticks);
            let dist = {
                let unit = world.enemies.get(&unit_id).unwrap();
                (unit.x - lx).hypot(unit.y - ly)
            };
            if dist <= CARGO_TRANSFER_RANGE {
                if let Some(&(item, available)) = loader_items.iter().find(|(_, n)| *n > 0) {
                    let take = available.min(CARGO_ITEM_CAPACITY);
                    let loader_pos = {
                        let found = world.tiles.iter().find(|tile| {
                            tile.block == CARGO_LOADER && tile.stored_amount == unit_id
                        });
                        found.map(|tile| tile.position)
                    };
                    if let Some(pos) = loader_pos {
                        if let Some(mut tile) = world.tiles.get_mut(&pos) {
                            let _ = inventory_remove(&mut tile.inventory, item, take);
                        }
                    }
                    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
                        unit.items = vec![(item, take)];
                    }
                    changed = true;
                }
            }
        }
        let _ = CARGO_LOADER_CAPACITY;
    }
    changed
}

fn configured_unload_item(tile: &DynamicTile) -> i16 {
    if tile.stored_item >= 0 {
        return tile.stored_item;
    }
    // TypeIO item config: tag 0 + i32 id, or a bare id.
    match tile.config.as_slice() {
        [0, rest @ ..] if rest.len() >= 4 => {
            i32::from_be_bytes(rest[..4].try_into().unwrap_or([0; 4])) as i16
        }
        bytes if bytes.len() >= 4 => {
            i32::from_be_bytes(bytes[..4].try_into().unwrap_or([0; 4])) as i16
        }
        _ => -1,
    }
}

fn move_unit_towards(world: &DynamicWorld, id: i32, tx: f32, ty: f32, delta_ticks: f32) -> bool {
    move_unit_to_range(world, id, tx, ty, delta_ticks, CARGO_MOVE_RANGE)
}

fn fly_assembler_drone(
    world: &DynamicWorld,
    id: i32,
    tx: f32,
    ty: f32,
    look: f32,
    delta_ticks: f32,
) -> bool {
    let Some(mut unit) = world.enemies.get_mut(&id) else {
        return false;
    };
    let dx = tx - unit.x;
    let dy = ty - unit.y;
    let dist = dx.hypot(dy);
    let mut changed = false;
    if dist > ASSEMBLER_AI_MOVE_RANGE {
        let step = (unit.move_speed * delta_ticks.max(0.0)).min(dist - ASSEMBLER_AI_MOVE_RANGE);
        if step > 0.0 {
            unit.x += dx / dist * step;
            unit.y += dy / dist * step;
            unit.rotation = dy.atan2(dx).to_degrees();
            changed = true;
        }
    }
    let dist_after = (tx - unit.x).hypot(ty - unit.y);
    if dist_after <= ASSEMBLER_AI_LOOK_RANGE {
        unit.velocity_x = 0.0;
        unit.velocity_y = 0.0;
        unit.rotation = look;
        changed = true;
    }
    changed
}

fn assembly_drone_alive(world: &DynamicWorld, id: i32) -> bool {
    world
        .enemies
        .get(&id)
        .is_some_and(|unit| unit.unit_type == ASSEMBLY_DRONE)
}

fn set_unit_elevation(world: &DynamicWorld, id: i32, elevation: f32) {
    if let Some(mut unit) = world.enemies.get_mut(&id) {
        unit.elevation = elevation;
    }
}

fn move_unit_to_range(
    world: &DynamicWorld,
    id: i32,
    tx: f32,
    ty: f32,
    delta_ticks: f32,
    stop_range: f32,
) -> bool {
    let Some(mut unit) = world.enemies.get_mut(&id) else {
        return false;
    };
    let dx = tx - unit.x;
    let dy = ty - unit.y;
    let dist = dx.hypot(dy);
    if dist <= stop_range {
        return false;
    }
    let step = (unit.move_speed * delta_ticks.max(0.0)).min(dist - stop_range);
    if step <= 0.0 {
        return false;
    }
    unit.x += dx / dist * step;
    unit.y += dy / dist * step;
    unit.rotation = dy.atan2(dx).to_degrees();
    true
}

fn assembler_spawn_point(tile: &DynamicTile) -> (f32, f32) {
    let (cx, cy) = tile_center(tile.position, tile.block);
    let radians = (f32::from(tile.rotation) * 90.0).to_radians();
    let len = 8.0 * (ASSEMBLER_AREA_SIZE + f32::from(block_size(tile.block))) / 2.0;
    (cx + radians.cos() * len, cy + radians.sin() * len)
}

fn assembler_drone_slot(spawn_x: f32, spawn_y: f32, index: usize) -> (f32, f32, f32) {
    let angle = index as f32 * 90.0 + 45.0;
    let radians = angle.to_radians();
    let offset = ASSEMBLER_AREA_SIZE / 2.0 * std::f32::consts::SQRT_2 * 8.0;
    (
        spawn_x + radians.cos() * offset,
        spawn_y + radians.sin() * offset,
        angle + 180.0,
    )
}

/// AssemblerAI for assembly-drone units tethered to UnitAssembler 393-395.
/// Spawns up to `dronesCreated` (4) drones on the official 240-tick construct
/// timer and flies each to its perimeter slot (`targetPos` / `targetAngle`).
/// Production progress on the assembler tile is unchanged (factory sibling).
pub(crate) fn simulate_assembler_drones(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &HashMap<i32, f32>,
) -> bool {
    let assemblers: Vec<DynamicTile> = world
        .tiles
        .iter()
        .filter(|tile| ASSEMBLER_BLOCKS.contains(&tile.block))
        .map(|tile| tile.value().clone())
        .collect();
    let mut changed = false;
    let mut live_positions = std::collections::HashSet::new();
    for assembler in assemblers {
        live_positions.insert(assembler.position);
        changed |= tick_assembler_drones(world, &assembler, delta_ticks, power);
    }
    let stale_keys: Vec<i32> = world
        .game_state
        .extras
        .assembler_drones
        .lock()
        .keys()
        .copied()
        .filter(|position| !live_positions.contains(position))
        .collect();
    for position in stale_keys {
        let ids = world
            .game_state
            .extras
            .assembler_drones
            .lock()
            .remove(&position)
            .map(|bind| bind.unit_ids)
            .unwrap_or_default();
        for id in ids {
            world.game_state.extras.queue_unit_kill(id);
            changed = true;
        }
    }
    changed
}

fn tick_assembler_drones(
    world: &DynamicWorld,
    assembler: &DynamicTile,
    delta_ticks: f32,
    power: &HashMap<i32, f32>,
) -> bool {
    if !assembler.enabled {
        let ids = {
            let mut binds = world.game_state.extras.assembler_drones.lock();
            binds
                .remove(&assembler.position)
                .map(|bind| bind.unit_ids)
                .unwrap_or_default()
        };
        let any = !ids.is_empty();
        for id in ids {
            world.game_state.extras.queue_unit_kill(id);
        }
        return any;
    }
    let mut bind = {
        let mut binds = world.game_state.extras.assembler_drones.lock();
        binds.remove(&assembler.position).unwrap_or_default()
    };
    let live: Vec<i32> = bind
        .unit_ids
        .iter()
        .copied()
        .filter(|&id| assembly_drone_alive(world, id))
        .collect();
    bind.unit_ids = live;
    let mut changed = false;
    let efficiency = efficiency_at(power, assembler.position);
    let build_speed = world.wave_rules.read().unit_build_speed_for(assembler.team);
    if bind.unit_ids.len() < ASSEMBLER_DRONES_CREATED && efficiency > 0.0 {
        bind.progress += delta_ticks.max(0.0) * build_speed.max(0.0) * efficiency
            / ASSEMBLER_DRONE_CONSTRUCT_TIME;
        if bind.progress >= 1.0 {
            let (cx, cy) = tile_center(assembler.position, assembler.block);
            if let Some(id) = spawn_unit_world(world, ASSEMBLY_DRONE, assembler.team, cx, cy, 90.0)
            {
                set_unit_elevation(world, id, 1.0);
                bind.unit_ids.push(id);
                if let Ok(frame) = encode_assembler_drone_spawned_frame(assembler.position, id) {
                    world.game_state.extras.queue_call(frame);
                }
                changed = true;
            }
        }
    } else if bind.unit_ids.len() >= ASSEMBLER_DRONES_CREATED {
        bind.progress = 0.0;
    }
    let (spawn_x, spawn_y) = assembler_spawn_point(assembler);
    for (index, id) in bind.unit_ids.iter().copied().enumerate() {
        let (tx, ty, look) = assembler_drone_slot(spawn_x, spawn_y, index);
        changed |= fly_assembler_drone(world, id, tx, ty, look, delta_ticks);
    }
    world
        .game_state
        .extras
        .assembler_drones
        .lock()
        .insert(assembler.position, bind);
    changed
}

pub(crate) fn simulate_repair_and_cargo(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &HashMap<i32, f32>,
) -> bool {
    let mut changed = false;
    changed |= simulate_repair_points(world, delta_ticks, power);
    changed |= simulate_unit_repair_towers(world, delta_ticks, power);
    changed |= simulate_build_towers(world, delta_ticks, power);
    changed |= simulate_cargo_loaders(world, delta_ticks, power);
    changed |= simulate_cargo_unload_points(world, delta_ticks);
    changed |= simulate_cargo_units(world, delta_ticks);
    changed |= simulate_assembler_drones(world, delta_ticks, power);
    changed
}
