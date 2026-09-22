//! Enemy AI / wave spawn / base handling. The collision leaf lookups and
//! support/effect helpers live here with their consumers; the listener
//! adapter re-exports them through crate::network::listener::*.

use crate::network::buildings::power as power_nodes;
use dashmap::DashMap;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::network::buildings::snapshot::dynamic_tile_health;

use tracing::{info, warn};

use crate::network::buildings::placement as building_placement;
use crate::network::economy::*;
use crate::network::units::*;
use crate::network::world::*;

use crate::network::combat::damage::apply_unit_armor;
use crate::network::combat::unit_combat::invalidate_navigation_for_block;
use crate::network::units::mining::heal_building_for_team;
use crate::network::wire::encode_build_health_update_frame;

pub(crate) fn hostile_unit_count(world: &DynamicWorld) -> u32 {
    let wave_team = world.wave_rules.read().wave_team;
    world
        .enemies
        .iter()
        .filter(|unit| {
            unit.team == wave_team && crate::game::unit_types::unit_type_is_enemy(unit.unit_type)
        })
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

pub(crate) fn nearest_player_building(
    world: &DynamicWorld,
    x: f32,
    y: f32,
) -> Option<(i32, f32, f32)> {
    let dynamic = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == 1)
        .map(|tile| {
            let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let target_y = tile.position as i16 as f32 * 8.0;
            (
                (target_x - x).hypot(target_y - y),
                tile.position,
                target_x,
                target_y,
            )
        });
    let base = world
        .base_buildings
        .iter()
        .filter(|building| building.team == 1)
        .map(|building| {
            let target_x = (building.position >> 16) as i16 as f32 * 8.0;
            let target_y = building.position as i16 as f32 * 8.0;
            (
                (target_x - x).hypot(target_y - y),
                building.position,
                target_x,
                target_y,
            )
        });
    dynamic
        .chain(base)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, position, target_x, target_y)| (position, target_x, target_y))
}

fn packed_tile(tx: i32, ty: i32) -> i32 {
    (tx << 16) | (ty & 0xFFFF)
}

/// Floor content id under a world position (0 when out of bounds).
/// Uses official `World.toTile` (ASTRA E04), not `floor(x/8)`.
pub(crate) fn floor_at(world: &DynamicWorld, x: f32, y: f32) -> i16 {
    floor_at_tile(world, world_to_tile(x), world_to_tile(y))
}

pub(crate) fn floor_at_tile(world: &DynamicWorld, tx: i32, ty: i32) -> i16 {
    if tx < 0 || ty < 0 || tx >= world.width || ty >= world.height {
        return 0;
    }
    if let Some(over) = world
        .game_state
        .extras
        .floor_overrides
        .get(&packed_tile(tx, ty))
    {
        return *over;
    }
    let idx = (ty * world.width + tx) as usize;
    world.floors.get(idx).copied().unwrap_or(0)
}

/// Overlay (ore) content id under a world position.
/// Uses official `World.toTile` (ASTRA E04), not `floor(x/8)`.
pub(crate) fn overlay_at(world: &DynamicWorld, x: f32, y: f32) -> i16 {
    overlay_at_tile(world, world_to_tile(x), world_to_tile(y))
}

pub(crate) fn overlay_at_tile(world: &DynamicWorld, tx: i32, ty: i32) -> i16 {
    if tx < 0 || ty < 0 || tx >= world.width || ty >= world.height {
        return 0;
    }
    if let Some(over) = world
        .game_state
        .extras
        .overlay_overrides
        .get(&packed_tile(tx, ty))
    {
        return *over;
    }
    let idx = (ty * world.width + tx) as usize;
    world.overlays.get(idx).copied().unwrap_or(0)
}

pub(crate) fn navigation_index(world: &DynamicWorld, x: i32, y: i32) -> Option<usize> {
    (x >= 0 && y >= 0 && x < world.width && y < world.height)
        .then_some((y * world.width + x) as usize)
}

/// Official `World.toTile`: `Math.round(coord / tilesize)` via `floor(x+0.5)`.
pub(crate) fn world_to_tile(coord: f32) -> i32 {
    (coord / 8.0 + 0.5).floor() as i32
}

pub(crate) fn world_to_tile_in_map(coord: f32, extent: i32) -> i32 {
    world_to_tile(coord).clamp(0, extent.saturating_sub(1))
}

/// Pathfinder cost-field variants. Ground uses `costGround` (ASTRA E04):
/// only `allDeep` liquid is a wall; shallow water is costly. Naval prefers
/// liquid and pays a high cost on land rather than treating every land tile
/// as equally impassable when a channel exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavigationClass {
    Ground,
    Legs,
    Naval,
}

pub(crate) fn navigation_field_class(
    world: &DynamicWorld,
    class: NavigationClass,
) -> Arc<Vec<u32>> {
    let revision = world.navigation_revision.load(Ordering::Relaxed);
    let cache = match class {
        NavigationClass::Ground => &world.ground_navigation,
        NavigationClass::Legs => &world.leg_navigation,
        NavigationClass::Naval => &world.naval_navigation,
    };
    let mut cached = cache.lock();
    if let Some(field) = cached.as_ref().filter(|field| field.revision == revision) {
        return field.costs.clone();
    }
    let costs = Arc::new(build_navigation_field_class(world, class));
    *cached = Some(NavigationField {
        revision,
        costs: costs.clone(),
    });
    costs
}

/// Pathfinder `PositionTarget` field (audit H13): cost-to-go toward an
/// arbitrary tile instead of the team's core. Cached per (class, team, goal, revision).
pub(crate) fn navigation_field_toward(
    world: &DynamicWorld,
    class: NavigationClass,
    goal_x: i32,
    goal_y: i32,
    agent_team: u8,
) -> Arc<Vec<u32>> {
    let revision = world.navigation_revision.load(Ordering::Relaxed);
    let packed = (goal_x << 16) | (goal_y as u16 as i32);
    let class_id = match class {
        NavigationClass::Ground => 0u8,
        NavigationClass::Legs => 1,
        NavigationClass::Naval => 2,
    };
    type TowardCache = Vec<(u64, u8, u8, i32, Arc<Vec<u32>>)>;
    thread_local! {
        static CACHE: std::cell::RefCell<TowardCache> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, _, _, _, costs)) = cache.iter().find(|(rev, cid, team, goal, _)| {
            *rev == revision && *cid == class_id && *team == agent_team && *goal == packed
        }) {
            return costs.clone();
        }
        let costs = Arc::new(build_navigation_field_toward(
            world, class, goal_x, goal_y, agent_team,
        ));
        cache.retain(|(rev, _, _, _, _)| *rev == revision);
        if cache.len() >= 16 {
            cache.remove(0);
        }
        cache.push((revision, class_id, agent_team, packed, costs.clone()));
        costs
    })
}

/// Legacy two-variant wrapper (ground/legs).
pub(crate) fn navigation_field(world: &DynamicWorld, legs: bool) -> Arc<Vec<u32>> {
    navigation_field_class(
        world,
        if legs {
            NavigationClass::Legs
        } else {
            NavigationClass::Ground
        },
    )
}

pub(crate) fn tile_is_leg_solid(block: i16, floor: i16, data: u8) -> bool {
    let block_navigation = crate::game::content::block_navigation(block);
    let pathing = crate::game::content::block_pathing(block);
    let natural_filled_wall =
        block_navigation.solid && !pathing.synthetic && pathing.fills_tile && data >= 2;
    let solid_floor_without_wall =
        crate::game::content::block_navigation(floor).solid && block == 0;
    natural_filled_wall || solid_floor_without_wall
}

pub(crate) fn build_navigation_field_class(
    world: &DynamicWorld,
    class: NavigationClass,
) -> Vec<u32> {
    let (gx, gy) = core_tile(world);
    build_navigation_field_toward(
        world,
        class,
        i32::from(gx),
        i32::from(gy),
        world.wave_rules.read().wave_team,
    )
}

pub(crate) fn build_navigation_field_toward(
    world: &DynamicWorld,
    class: NavigationClass,
    goal_x: i32,
    goal_y: i32,
    agent_team: u8,
) -> Vec<u32> {
    let legs = class == NavigationClass::Legs;
    let naval = class == NavigationClass::Naval;
    const IMPASSABLE: u32 = u32::MAX / 4;

    let total = (world.width * world.height).max(0) as usize;
    let mut dynamic_cells = HashMap::new();
    for tile in world.tiles.iter().filter(|tile| tile.block != 0) {
        let health = dynamic_tile_health(&tile);
        let solid = crate::game::content::building_check_solid(tile.block, tile.door_open);
        for position in &tile.occupied {
            dynamic_cells.insert(*position, (tile.block, tile.team, health, solid));
        }
        dynamic_cells
            .entry(tile.position)
            .or_insert((tile.block, tile.team, health, solid));
    }
    let mut base_cells = HashMap::new();
    for building in world.base_buildings.iter() {
        for position in &building.occupied {
            base_cells.insert(*position, (building.block, building.team, building.health));
        }
    }
    let Some(target) = navigation_index(world, goal_x, goal_y) else {
        return vec![IMPASSABLE; total];
    };
    let neighbor_flags = |x: i32, y: i32| {
        let mut near_solid = false;
        let mut near_liquid = false;
        let mut near_ground = false;
        let mut all_deep = crate::game::content::block_navigation(floor_at_tile(world, x, y)).deep;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            if nx < 0 || ny < 0 || nx >= world.width || ny >= world.height {
                continue;
            }
            let nfloor_id = floor_at_tile(world, nx, ny);
            let nfloor = crate::game::content::block_navigation(nfloor_id);
            if crate::game::content::floor_is_liquid(nfloor_id) && nfloor.deep {
                near_liquid = true;
            }
            if !crate::game::content::floor_is_liquid(nfloor_id) {
                near_ground = true;
            }
            if !nfloor.deep {
                all_deep = false;
            }
            let nindex = (ny * world.width + nx) as usize;
            let npos = (nx << 16) | (ny as u16 as i32);
            let nblock = dynamic_cells
                .get(&npos)
                .map(|(block, _, _, _)| *block)
                .or_else(|| base_cells.get(&npos).map(|(block, _, _)| *block))
                .unwrap_or(world.base_blocks[nindex]);
            let nsolid = dynamic_cells.get(&npos).map_or_else(
                || crate::game::content::building_check_solid(nblock, false),
                |(_, _, _, solid)| *solid,
            );
            let npassable = crate::game::content::block_navigation(nblock).team_passable;
            if nsolid && !npassable {
                near_solid = true;
            }
        }
        (near_solid, near_liquid, near_ground, all_deep)
    };
    let tile_cost = |x: i32, y: i32| {
        if x == goal_x && y == goal_y {
            return 1;
        }
        let index = (y * world.width + x) as usize;
        let floor_id = floor_at_tile(world, x, y);
        let floor = crate::game::content::block_navigation(floor_id);
        let (near_solid, near_liquid, near_ground, all_deep) = neighbor_flags(x, y);
        // costGround: allDeep is a wall; a deep shore tile is +6000 (ASTRA E04).
        if !legs && !naval && all_deep {
            return IMPASSABLE;
        }
        let terrain = 1 + u32::from(floor.deep) * 6000 + u32::from(floor.damages) * 30;
        let position = (x << 16) | (y as u16 as i32);
        let effective_block = dynamic_cells
            .get(&position)
            .map(|(block, _, _, _)| *block)
            .or_else(|| base_cells.get(&position).map(|(block, _, _)| *block))
            .unwrap_or(world.base_blocks[index]);
        if legs
            && tile_is_leg_solid(
                effective_block,
                floor_id,
                world.tile_data.get(index).copied().unwrap_or(0),
            )
        {
            return IMPASSABLE;
        }
        let own_or_derelict_wall = |team: u8, team_passable: bool, check_solid: bool| {
            check_solid && !team_passable && (team == agent_team || team == 0)
        };
        if let Some((block, team, health, check_solid)) = dynamic_cells.get(&position).copied() {
            let navigation = crate::game::content::block_navigation(block);
            if check_solid {
                if legs {
                    return terrain + 5;
                }
                if own_or_derelict_wall(team, navigation.team_passable, true) {
                    return IMPASSABLE;
                }
                let scaled_health = ((health / 40.0) as u32).min(80);
                return terrain
                    + scaled_health * 5
                    + u32::from(near_solid) * 2
                    + u32::from(near_liquid) * 6;
            }
        }
        if let Some((block, team, health)) = base_cells.get(&position).copied() {
            let navigation = crate::game::content::block_navigation(block);
            if navigation.solid {
                if legs {
                    return terrain + 5;
                }
                if own_or_derelict_wall(team, navigation.team_passable, true) {
                    return IMPASSABLE;
                }
                let scaled_health = ((health / 40.0) as u32).min(80);
                return terrain
                    + scaled_health * 5
                    + u32::from(near_solid) * 2
                    + u32::from(near_liquid) * 6;
            }
        }
        let base = crate::game::content::block_navigation(world.base_blocks[index]);
        if naval {
            let team = dynamic_cells
                .get(&position)
                .map(|(_, team, _, _)| *team)
                .or_else(|| base_cells.get(&position).map(|(_, team, _)| *team))
                .unwrap_or(0);
            let solid = crate::game::content::building_check_solid(effective_block, false);
            if !crate::game::content::floor_is_liquid(floor_id)
                || (solid && (team == agent_team || team == 0))
            {
                7000 + u32::from(near_ground || near_solid) * 14
            } else {
                1 + u32::from(near_ground || near_solid) * 14
                    + u32::from(!floor.deep)
                    + u32::from(floor.damages) * 35
            }
        } else if base.solid {
            if legs {
                terrain + 5
            } else {
                IMPASSABLE
            }
        } else {
            terrain + u32::from(near_solid) * 2 + u32::from(near_liquid) * 6
        }
    };

    let mut costs = vec![IMPASSABLE; total];
    let mut pending = BinaryHeap::new();
    costs[target] = 0;
    pending.push((Reverse(0u32), target));
    while let Some((Reverse(cost), index)) = pending.pop() {
        if cost != costs[index] {
            continue;
        }
        let x = index as i32 % world.width;
        let y = index as i32 / world.width;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            let Some(next) = navigation_index(world, nx, ny) else {
                continue;
            };
            let step = tile_cost(nx, ny);
            if step >= IMPASSABLE {
                continue;
            }
            let candidate = cost.saturating_add(step).min(IMPASSABLE);
            if candidate < costs[next] {
                costs[next] = candidate;
                pending.push((Reverse(candidate), next));
            }
        }
    }
    costs
}

pub(crate) fn base_building_at(world: &DynamicWorld, position: i32) -> Option<BaseBuildingState> {
    world
        .base_buildings
        .iter()
        .find(|building| building.position == position || building.occupied.contains(&position))
        .map(|building| building.value().clone())
}

/// Returns true when the hit destroyed and removed the building.
/// After a team's registered core is destroyed, promote the next surviving
/// core of the same team (official `TeamData.cores()` rebuilds from the
/// world). If none survive, the team is left coreless: `damage_team_core`
/// already emitted the game-over for the destroyed core, and `core_health`
/// stays 0 for the team-1 mirror.
pub(crate) fn reregister_team_core(world: &DynamicWorld, team: u8) {
    let existing = crate::network::world::team_core_snapshot(world, team);
    for core in existing {
        if world
            .tiles
            .get(&core.position)
            .is_none_or(|tile| !(339..=344).contains(&tile.block))
        {
            crate::network::world::unregister_team_core(world, team, core.position);
        }
    }
    let positions: Vec<_> = world
        .tiles
        .iter()
        .filter_map(|tile| {
            (tile.team == team && (339..=344).contains(&tile.block)).then(|| {
                let max = crate::game::content::block_health(tile.block);
                TeamCore {
                    position: tile.position,
                    block: tile.block,
                    health: tile.health.min(max),
                    max_health: max,
                }
            })
        })
        .collect();
    for core in positions {
        crate::network::world::register_team_core(world, team, core);
    }
}

/// RtsAI.damagedSet (BuildDamageEvent listener): remember that `position`
/// took damage this tick so squads can pick defend targets for the next
/// ~two assignSquads windows (120-tick cadence each).
pub(crate) fn record_damaged_building(world: &DynamicWorld, position: i32) {
    let tick = world
        .game_state
        .world_ticks
        .load(std::sync::atomic::Ordering::Relaxed);
    let mut window = world.damaged_window.lock();
    window.retain(|(pos, seen)| *pos != position && tick.saturating_sub(*seen) < 240);
    window.push((position, tick));
}

/// Whether `position` was damaged within the current defend window.
pub(crate) fn recently_damaged(world: &DynamicWorld, position: i32) -> bool {
    let tick = world
        .game_state
        .world_ticks
        .load(std::sync::atomic::Ordering::Relaxed);
    world
        .damaged_window
        .lock()
        .iter()
        .any(|(pos, seen)| *pos == position && tick.saturating_sub(*seen) <= 240)
}

/// Phase walls 224/225: `chanceDeflect = 10` → P(deflect) = min(1, 10/damage).
fn wall_deflects(world: &DynamicWorld, position: i32, block: i16, damage: f32) -> bool {
    if !matches!(block, 224 | 225) {
        return false;
    }
    let chance = (10.0 / damage.max(1.0)).min(1.0);
    let tick = *world.game_state.simulation_time.read() as i64;
    let mut rand = crate::engine::arc_rand::ArcRand::new((i64::from(position) << 16) ^ tick);
    rand.next_float() < chance
}

/// Surge walls 226/227: `lightningChance = 0.05`, 20 damage, length 17 tiles.
fn wall_surge_lightning(world: &DynamicWorld, position: i32, block: i16, team: u8) {
    if !matches!(block, 226 | 227) {
        return;
    }
    let tick = *world.game_state.simulation_time.read() as i64;
    let mut rand =
        crate::engine::arc_rand::ArcRand::new((i64::from(position) << 16) ^ tick ^ 0x5A17);
    if rand.next_float() >= 0.05 {
        return;
    }
    let wx = (position >> 16) as i16 as f32 * 8.0;
    let wy = position as i16 as f32 * 8.0;
    world.game_state.extras.queue_wall_lightning(wx, wy, team);
}

pub(crate) fn damage_building(
    world: &DynamicWorld,
    position: i32,
    damage: f32,
) -> Option<(bool, f32)> {
    // RtsAI.damagedSet: BuildDamageEvent feeds the squad defend window.
    record_damaged_building(world, position);
    // BuildingComp.lastDamageTime: feeds RepairBeamWeapon's
    // wasRecentlyDamaged heal modifier.
    world
        .building_last_damage
        .insert(position, *world.game_state.simulation_time.read());
    if let Some(tile) = world.tiles.get(&position) {
        if wall_deflects(world, position, tile.block, damage) {
            return Some((false, tile.health));
        }
    }
    if let Some(mut tile) = world.tiles.get_mut(&position) {
        let max_health = crate::game::content::block_health(tile.block);
        if tile.health <= 0.0 || tile.health > max_health {
            tile.health = max_health;
        }
        // BulletType.damageMultiplier already applied the shooter's rule at
        // creation. Only the victim's blockHealth divides incoming damage.
        let building_team = tile.team;
        let rules = world.wave_rules.read();
        let team_rule = rules.team_rule(building_team);
        let health_mult = rules.block_health_multiplier * team_rule.block_health_multiplier;
        let effective = if health_mult.abs() <= 0.0001 {
            tile.health + 1.0
        } else {
            damage / health_mult
        };
        tile.health -= apply_unit_armor(effective, crate::game::content::block_armor(tile.block));
        let destroyed = tile.health <= 0.0;
        let health = tile.health.max(0.0);
        let block = tile.block;
        let destroyed_state = destroyed.then(|| tile.clone());
        drop(tile);
        wall_surge_lightning(world, position, block, building_team);
        if let Some(building) = destroyed_state {
            building_placement::teardown_building_in_place(world, position);
            world
                .tiles
                .insert(position, dynamic_building_tombstone(&building));
            // A destroyed core must be re-registered: if another core of the
            // same team survives, it becomes the team's core; otherwise the
            // team is left coreless (game-over path via damage_team_core).
            if matches!(building.block, 339..=344) {
                crate::network::world::unregister_team_core(world, building.team, position);
                reregister_team_core(world, building.team);
            }
            crate::network::core_inventory::clamp_core_inventories(world);
        }
        return Some((destroyed, health));
    }
    if let Some(building) = world.base_buildings.get(&position) {
        if wall_deflects(world, position, building.block, damage) {
            return Some((false, building.health));
        }
    }
    let mut building = world.base_buildings.get_mut(&position)?;
    let building_team = building.team;
    let rules = world.wave_rules.read();
    let team_rule = rules.team_rule(building_team);
    let health_mult = rules.block_health_multiplier * team_rule.block_health_multiplier;
    let effective = if health_mult.abs() <= 0.0001 {
        building.health + 1.0
    } else {
        damage / health_mult
    };
    building.health -=
        apply_unit_armor(effective, crate::game::content::block_armor(building.block));
    let destroyed = building.health <= 0.0;
    let health = building.health.max(0.0);
    let destroyed_state = destroyed.then(|| building.clone());
    drop(building);
    if let Some(building) = destroyed_state {
        world.base_buildings.remove(&position);
        world
            .tiles
            .insert(position, base_building_tombstone(&building));
        invalidate_navigation_for_block(world, building.block);
        crate::network::core_inventory::clamp_core_inventories(world);
        power_nodes::relink_after_insulated_removed(
            world,
            building.position,
            building.block,
            &building.occupied,
        );
    }
    Some((destroyed, health))
}

pub(crate) fn base_building_tombstone(building: &BaseBuildingState) -> DynamicTile {
    DynamicTile {
        logic_control: None,
        payload_inventory: Vec::new(),
        position: building.position,
        block: 0,
        rotation: 0,
        team: building.team,
        config: vec![0],
        enabled: true,
        message: None,
        occupied: building.occupied.clone(),
        stored_item: -1,
        stored_amount: i32::from(building.block) + 1,
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
        health: 0.0,
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
    }
}

pub(crate) fn dynamic_building_tombstone(building: &DynamicTile) -> DynamicTile {
    DynamicTile {
        logic_control: None,
        payload_inventory: Vec::new(),
        position: building.position,
        block: 0,
        rotation: building.rotation,
        team: building.team,
        config: building.config.clone(),
        enabled: true,
        message: None,
        occupied: building.occupied.clone(),
        stored_item: -1,
        stored_amount: i32::from(building.block) + 1,
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
        health: 0.0,
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
    }
}

pub(crate) fn enemy_circle_radius(unit_type: i16) -> Option<f32> {
    match unit_type {
        15 => Some(60.0), // Flare
        16 => Some(40.0), // Horizon bomber
        _ => None,
    }
}

pub(crate) fn move_enemy_in_attack_orbit(
    enemy: &mut EnemyUnit,
    target_x: f32,
    target_y: f32,
    radius: f32,
    delta_ticks: f32,
) {
    let outward_x = enemy.x - target_x;
    let outward_y = enemy.y - target_y;
    let distance = outward_x.hypot(outward_y).max(0.001);
    let tangent_x = -outward_y / distance;
    let tangent_y = outward_x / distance;
    let radial = ((distance - radius) / radius.max(1.0)).clamp(-1.0, 1.0);
    let desired_x = tangent_x - outward_x / distance * radial;
    let desired_y = tangent_y - outward_y / distance * radial;
    let desired_length = desired_x.hypot(desired_y).max(0.001);
    let velocity_x = desired_x / desired_length * enemy.move_speed;
    let velocity_y = desired_y / desired_length * enemy.move_speed;
    enemy.velocity_x = velocity_x;
    enemy.velocity_y = velocity_y;
    enemy.x += velocity_x * delta_ticks;
    enemy.y += velocity_y * delta_ticks;
    enemy.rotation = velocity_y.atan2(velocity_x).to_degrees();
}

/// AI stand-in for FlyingFollowAI (quell 52 / disrupt 54) and HugAI
/// (renale 56 / latum 57): shadow the nearest friendly unit whose
/// `EnemySpec.health` ("large" proxy; hitSize is not tabulated for the
/// Erekir ids) is strictly greater than the follower's own, staying within
/// `TETHER_FOLLOW_DISTANCE` world units. Movement-only preference: attacks,
/// abilities and player/logic authority are unchanged. Deterministic:
/// distance ties break on unit id.
pub(crate) const TETHER_FOLLOW_UNITS: &[i16] = &[52, 54, 56, 57];
pub(crate) const TETHER_FOLLOW_DISTANCE: f32 = 40.0;

/// Precomputed follower -> ally-position map for one simulation tick. Runs
/// as a read-only pass so callers never hold an enemies write guard while it
/// iterates (DashMap DM rule).
pub(crate) fn tether_follow_targets(world: &DynamicWorld) -> HashMap<i32, (f32, f32)> {
    let followers: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| TETHER_FOLLOW_UNITS.contains(&unit.unit_type))
        .filter(|unit| {
            !crate::network::units::unit_is_player_controlled(world, unit.id)
                && !crate::network::units::unit_bound_to_logic(world, unit.id)
        })
        .map(|unit| (unit.id, unit.team, unit.x, unit.y, unit.unit_type))
        .collect();
    let mut targets = HashMap::new();
    for (id, team, x, y, unit_type) in followers {
        let Some(own) = enemy_spec(unit_type) else {
            continue;
        };
        let mut best: Option<(f32, i32, f32, f32)> = None;
        let mut already_close = false;
        for other in world.enemies.iter() {
            if other.team != team || other.id == id || other.entity_class == 39 {
                continue;
            }
            let Some(spec) = enemy_spec(other.unit_type) else {
                continue;
            };
            if spec.health <= own.health {
                continue;
            }
            let distance = (other.x - x).hypot(other.y - y);
            if distance <= TETHER_FOLLOW_DISTANCE {
                // Already shadowing a large ally: hold this spot.
                already_close = true;
                break;
            }
            let better = match best {
                None => true,
                Some((best_distance, best_id, _, _)) => {
                    distance < best_distance || (distance == best_distance && other.id < best_id)
                }
            };
            if better {
                best = Some((distance, other.id, other.x, other.y));
            }
        }
        if already_close {
            targets.insert(id, (x, y));
        } else if let Some((_, _, ally_x, ally_y)) = best {
            targets.insert(id, (ally_x, ally_y));
        }
    }
    targets
}

#[derive(Clone, Copy)]
pub(crate) enum SupportRepairTarget {
    Unit(i32),
    Building(i32),
}

pub(crate) fn apply_enemy_support_abilities(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) {
    let simulation_time = *world.game_state.simulation_time.read();
    let crossed = |period: f32| {
        (simulation_time / period).floor()
            > ((simulation_time - delta_ticks.max(0.0)).max(0.0) / period).floor()
    };
    let supports: Vec<_> = world
        .enemies
        .iter()
        .filter(|enemy| matches!(enemy.unit_type, 3 | 5..=8 | 21 | 24 | 27 | 30..=32))
        .map(|enemy| (enemy.unit_type, enemy.team, enemy.x, enemy.y, enemy.id))
        .collect();
    for (unit_type, team, x, y, id) in supports {
        if unit_type == 3 {
            // Scepter ShieldRegenFieldAbility(25, 250, 60, 60): every 60 ticks
            // add 25 shield (cap 250) to allies within 60 tiles.
            if crossed(60.0) {
                let allies: Vec<_> = world
                    .enemies
                    .iter()
                    .filter(|ally| ally.team == team && (ally.x - x).hypot(ally.y - y) <= 60.0)
                    .map(|ally| ally.id)
                    .collect();
                for ally_id in allies {
                    if let Some(mut ally) = world.enemies.get_mut(&ally_id) {
                        ally.shield = (ally.shield + 25.0).min(250.0);
                    }
                }
            }
            continue;
        }
        if unit_type == 8 {
            // Vela RepairBeamWeapon("repair-beam-weapon-center-large", 158.1):
            // repairSpeed 1.4, bullet maxRange 120, reload 1. Official weapon
            // defaults keep targetUnits=true / targetBuildings=false, so the
            // beam continuously heals the nearest damaged allied unit by
            // 1.4 HP/tick; buildings are NOT repaired (documented deviation
            // from the earlier task description).
            let unit_targets: Vec<_> = world
                .enemies
                .iter()
                .filter(|ally| ally.id != id && ally.team == team)
                .filter_map(|ally| {
                    let maximum = enemy_max_health(&ally);
                    let distance = (ally.x - x).hypot(ally.y - y);
                    (distance <= 120.0 && ally.health < maximum)
                        .then_some((distance, ally.id, maximum))
                })
                .collect();
            if let Some((_, target_id, maximum)) =
                unit_targets.into_iter().min_by(|l, r| l.0.total_cmp(&r.0))
            {
                if let Some(mut ally) = world.enemies.get_mut(&target_id) {
                    ally.health = (ally.health + 1.4 * delta_ticks.max(0.0)).min(maximum);
                }
            }
            continue;
        }
        // Nova / poly / oct RepairFieldAbility table (spawn.rs, desktop.jar
        // 159.7 dump): every `reload` ticks heal `amount` flat HP to EVERY
        // damaged allied unit within `range` (Units.nearby includes the
        // owner; buildings are not healed).
        if let Some((amount, reload, range)) = repair_field_spec(unit_type) {
            if crossed(reload) {
                let allies: Vec<_> = world
                    .enemies
                    .iter()
                    .filter(|ally| {
                        ally.team == team
                            && ally.health < enemy_max_health(ally)
                            && (ally.x - x).hypot(ally.y - y) <= range
                    })
                    .map(|ally| ally.id)
                    .collect();
                for ally_id in allies {
                    if let Some(mut ally) = world.enemies.get_mut(&ally_id) {
                        ally.health = (ally.health + amount).min(enemy_max_health(&ally));
                    }
                }
                if unit_type == 24 && team == 1 {
                    for mut player in world.players.iter_mut() {
                        if possessed_unit_id(world, player.unit_id).is_some() {
                            continue;
                        }
                        if !player.dead
                            && player.health < 150.0
                            && (player.x - x).hypot(player.y - y) <= range
                        {
                            player.health = (player.health + amount).min(150.0);
                        }
                    }
                }
            }
            continue;
        }
        if unit_type == 31 {
            if crossed(360.0) {
                let allies: Vec<_> = world
                    .enemies
                    .iter()
                    .filter(|ally| ally.team == team && (ally.x - x).hypot(ally.y - y) <= 60.0)
                    .map(|ally| ally.id)
                    .collect();
                for ally_id in allies {
                    if let Some(mut ally) = world.enemies.get_mut(&ally_id) {
                        let overdrive_duration = ally
                            .statuses
                            .iter()
                            .find(|entry| entry.effect == 14)
                            .map(|entry| entry.time.max(360.0))
                            .unwrap_or(360.0);
                        crate::network::units::StatusContainer::apply_status(
                            &mut *ally,
                            14,
                            overdrive_duration,
                        );
                    }
                }
                if team == 1 {
                    for mut player in world.players.iter_mut() {
                        if possessed_unit_id(world, *player.key()).is_some() {
                            continue;
                        }
                        if !player.dead && (player.x - x).hypot(player.y - y) <= 60.0 {
                            let overdrive_duration = player
                                .statuses
                                .iter()
                                .find(|entry| entry.effect == 14)
                                .map(|entry| entry.time.max(360.0))
                                .unwrap_or(360.0);
                            crate::network::units::StatusContainer::apply_status(
                                &mut *player,
                                14,
                                overdrive_duration,
                            );
                        }
                    }
                }
            }
            continue;
        }
        if matches!(unit_type, 30 | 32) {
            let (repair_range, repair_speed) = if unit_type == RETUSA.unit_type {
                (120.0, 0.75)
            } else {
                (130.0, 0.7)
            };
            let unit_targets = world
                .enemies
                .iter()
                .filter(|ally| ally.id != id && ally.team == team)
                .filter_map(|ally| {
                    let maximum = enemy_max_health(&ally);
                    let distance = (ally.x - x).hypot(ally.y - y);
                    (distance <= repair_range && ally.health < maximum).then_some((
                        distance,
                        SupportRepairTarget::Unit(ally.id),
                        maximum,
                    ))
                });
            let dynamic_targets = world.tiles.iter().filter_map(|tile| {
                if tile.block == 0 || tile.team != team {
                    return None;
                }
                let maximum = crate::game::content::block_health(tile.block);
                let health = dynamic_tile_health(&tile);
                let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
                let target_y = tile.position as i16 as f32 * 8.0;
                let distance = (target_x - x).hypot(target_y - y);
                (distance <= repair_range
                    && health < maximum
                    && !building_heal_suppressed(world, tile.position, tile.block))
                .then_some((
                    distance,
                    SupportRepairTarget::Building(tile.position),
                    maximum,
                ))
            });
            let base_targets = world.base_buildings.iter().filter_map(|building| {
                if building.team != team {
                    return None;
                }
                let maximum = crate::game::content::block_health(building.block);
                let target_x = (building.position >> 16) as i16 as f32 * 8.0;
                let target_y = building.position as i16 as f32 * 8.0;
                let distance = (target_x - x).hypot(target_y - y);
                (distance <= repair_range
                    && building.health < maximum
                    && !building_heal_suppressed(world, building.position, building.block))
                .then_some((
                    distance,
                    SupportRepairTarget::Building(building.position),
                    maximum,
                ))
            });
            let target = unit_targets
                .chain(dynamic_targets)
                .chain(base_targets)
                .min_by(|left, right| left.0.total_cmp(&right.0));
            if let Some((_, target, maximum)) = target {
                let amount = repair_speed * delta_ticks.max(0.0);
                match target {
                    SupportRepairTarget::Unit(target_id) => {
                        if let Some(mut ally) = world.enemies.get_mut(&target_id) {
                            ally.health = (ally.health + amount).min(maximum);
                        }
                    }
                    SupportRepairTarget::Building(position) => {
                        if let Some(health) =
                            heal_building_for_team(world, position, team, 0.0, amount)
                        {
                            if let Ok(frame) =
                                encode_build_health_update_frame(&[(position, health)])
                            {
                                out.broadcast(frame);
                            }
                        }
                    }
                }
            }
            continue;
        }
        if unit_type == QUASAR.unit_type {
            // ForceFieldAbility(60, 0.4, 500, 360): regen on unit.shield.
            // Bullet absorption lives in quasar_force_field_absorb; oct's
            // area shield (world.force_fields) is a separate unit and is
            // not rewritten here.
            if let Some(mut quasar) = world.enemies.get_mut(&id) {
                quasar.shield = (quasar.shield + 0.4 * delta_ticks).min(500.0);
            }
            continue;
        }
        // Pulsar/Bryde ShieldRegenFieldAbility(20, 40, reload, 60). Nova's
        // RepairFieldAbility lives in the repair_field_spec table above.
        let (period, heal, shield_amount, shield_cap): (f32, Option<f32>, Option<f32>, f32) =
            match unit_type {
                6 => (300.0, None, Some(20.0), 40.0),
                27 => (240.0, None, Some(20.0), 40.0),
                _ => continue,
            };
        if !crossed(period) {
            continue;
        }
        let allies: Vec<_> = world
            .enemies
            .iter()
            .filter(|ally| ally.team == team && (ally.x - x).hypot(ally.y - y) <= 60.0)
            .map(|ally| ally.id)
            .collect();
        for ally_id in allies {
            if let Some(mut ally) = world.enemies.get_mut(&ally_id) {
                if let Some(amount) = heal {
                    ally.health = (ally.health + amount).min(enemy_max_health(&ally));
                } else if let Some(amount) = shield_amount {
                    if ally.shield < shield_cap {
                        ally.shield = (ally.shield + amount).min(shield_cap);
                    }
                }
            }
        }
    }
}

pub(crate) fn enemy_max_health(enemy: &EnemyUnit) -> f32 {
    enemy_spec(enemy.unit_type)
        .map(|spec| spec.health)
        .unwrap_or(enemy.health)
}

/// Save slots contain durable world state only. Loading cancels transient actions
/// so clients cannot receive invisible in-flight damage or half-finished builds
/// after they reconnect and rebuild their world stream.
pub(crate) fn cancel_transient_world_actions(world: &DynamicWorld) {
    world.projectiles.clear();
    world.pending_builds.clear();
    world.pending_breaks.clear();
    // Team plans are the visual half of PendingBuild, not durable work by
    // themselves. Keeping them after cancelling pending work creates ghosts
    // that can never advance (plans>0 while pending=0).
    world.team_build_plans.write().teams.clear();
    for mut session in world.player_sessions.iter_mut() {
        session.active_plans.clear();
    }
}

pub(crate) fn restore_base_buildings(
    world: &DynamicWorld,
    saved_health: &[PersistedBaseBuildingHealth],
) {
    let saved: HashMap<_, _> = saved_health
        .iter()
        .map(|entry| (entry.position, entry.health))
        .collect();
    world.base_buildings.clear();
    for template in &world.base_building_templates {
        if world
            .tiles
            .get(&template.position)
            .is_some_and(|tile| tile.block == 0)
        {
            continue;
        }
        let maximum = crate::game::content::block_health(template.block);
        let health = saved
            .get(&template.position)
            .copied()
            .filter(|health| health.is_finite() && (0.0..=maximum).contains(health))
            .unwrap_or(template.health);
        world.base_buildings.insert(
            template.position,
            BaseBuildingState {
                health,
                inventory: Vec::new(),
                ..template.clone()
            },
        );
    }
}

/// WaveSpawner ground spread (`tilesize * 2`).
const WAVE_GROUND_SPREAD: f32 = 16.0;
/// WaveSpawner.spawnEffect: unmoving then invincible.
const SPAWN_UNMOVING_TICKS: f32 = 30.0;
const SPAWN_INVINCIBLE_TICKS: f32 = 60.0;
/// WaveSpawner.doShockwave damage (Damage.damage with air=true).
const SPAWN_SHOCKWAVE_DAMAGE: f32 = 99_999_999.0;

pub(crate) fn spawn_wave(world: &DynamicWorld, out: &dyn crate::network::outbound::FrameEmit) {
    // ASTRA W06: Logic.runWave increments the wave even when nobody appears.
    let wave = world.game_state.wave.fetch_add(1, Ordering::Relaxed);
    world.game_state.game_stats.write().waves_lasted += 1;
    let now = *world.game_state.simulation_time.read();
    world
        .game_state
        .extras
        .spawner_until
        .store((now + 121.0) as u32, Ordering::Relaxed);
    let groups = if world.wave_rules.read().is_default() {
        initial_official_wave_groups(wave - 1)
    } else {
        map_wave_spawns(wave - 1, &world.wave_rules.read())
    };
    let spawn_points = wave_spawn_points(world);
    let (wave_team, drop_zone) = {
        let rules = world.wave_rules.read();
        (rules.wave_team, rules.drop_zone_radius)
    };
    // ASTRA W03: one shockwave per overlay, not per unit and not at cores.
    let overlays = world.enemy_spawns.read().clone();
    for &(tile_x, tile_y) in &overlays {
        crate::network::combat::apply_allied_splash_damage_for_team(
            world,
            out,
            wave_team,
            f32::from(tile_x) * 8.0,
            f32::from(tile_y) * 8.0,
            SPAWN_SHOCKWAVE_DAMAGE,
            drop_zone,
            1.0,
            -1,
            0.0,
            1.0,
        );
    }
    let mut spawned = 0u32;
    for group in groups {
        // ASTRA W02: amount is per eligible overlay. A missing filter yields
        // zero units for that group — never fall back to every overlay.
        let spawns: Vec<(i16, i16)> = if group.spawn >= 0 {
            let (sx, sy) = ((group.spawn >> 16) as i16, (group.spawn & 0xffff) as i16);
            spawn_points
                .iter()
                .copied()
                .filter(|(x, y)| *x == sx && *y == sy)
                .collect()
        } else {
            spawn_points.clone()
        };
        let team = group.team.unwrap_or(wave_team);
        for &(tile_x, tile_y) in &spawns {
            for _ in 0..group.amount {
                let id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
                insert_wave_unit(world, &group, team, tile_x, tile_y, id, wave);
                spawned += 1;
            }
        }
    }
    world
        .game_state
        .enemies_count
        .store(hostile_unit_count(world), Ordering::Relaxed);
    if spawn_points.is_empty() {
        warn!("Wave {wave} incremented with no spawn overlays or cores");
    }
    info!(
        "Spawned official wave {} with {} supported units",
        wave, spawned
    );
}

/// ASTRA W03/W05: geometry, spawnEffect statuses, base stats (not baked).
pub(crate) fn insert_wave_unit(
    world: &DynamicWorld,
    group: &WaveSpawn,
    team: u8,
    tile_x: i16,
    tile_y: i16,
    id: i32,
    wave: u32,
) {
    let flying = crate::game::content::unit_movement(group.spec.unit_type).flying;
    let (base_x, base_y) = crate::network::simulation::remaining::flyer_spawn_world(
        world,
        tile_x,
        tile_y,
        group.spec.unit_type,
    );
    let (x, y) = if flying {
        (base_x, base_y)
    } else {
        let (jx, jy) = ground_spawn_jitter(id, wave);
        (base_x + jx, base_y + jy)
    };
    let cx = world.width as f32 * 4.0;
    let cy = world.height as f32 * 4.0;
    let rotation = (cy - y).atan2(cx - x).to_degrees();
    let payloads = group
        .payloads
        .iter()
        .filter_map(|unit_type| payload_unit_for_wave(*unit_type, team))
        .map(CarriedPayload::Unit)
        .collect();
    let mut unit = EnemyUnit {
        id,
        unit_type: group.spec.unit_type,
        entity_class: group.spec.entity_class,
        team,
        x,
        y,
        rotation,
        health: group.spec.health,
        shield: group.shield,
        status_effect: group.status_effect,
        status_duration: f32::MAX,
        statuses: Vec::new(),
        velocity_x: 0.0,
        velocity_y: 0.0,
        elevation: if crate::game::content::unit_movement(group.spec.unit_type).flying {
            1.0
        } else {
            0.0
        },
        payloads,
        flag: 0.0,
        items: group.items.clone(),
        mine_progress: 0.0,
        attack_reload: 0.0,
        secondary_attack_reload: 0.0,
        tertiary_attack_reload: 0.0,
        quaternary_attack_reload: 0.0,
        move_speed: group.spec.speed,
        attack_damage: group.spec.attack_damage,
        attack_reload_time: group.spec.attack_reload,
        attack_range: group.spec.attack_range,
        authority: UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        missile_time: 0.0,
        status_agg: Default::default(),
        drown_progress: 0.0,
    };
    if group.status_effect >= 0 {
        crate::network::units::StatusContainer::apply_status(
            &mut unit,
            group.status_effect,
            f32::MAX,
        );
    }
    apply_wave_spawn_effect(&mut unit);
    world.enemies.insert(id, unit);
    world.register_unit_group(id);
}

fn apply_wave_spawn_effect(unit: &mut EnemyUnit) {
    crate::network::units::StatusContainer::apply_status(
        unit,
        crate::game::status::STATUS_UNMOVING,
        SPAWN_UNMOVING_TICKS,
    );
    crate::network::units::StatusContainer::apply_status(
        unit,
        crate::game::status::STATUS_INVINCIBLE,
        SPAWN_INVINCIBLE_TICKS,
    );
}

fn ground_spawn_jitter(unit_id: i32, wave: u32) -> (f32, f32) {
    let mut rng = crate::network::combat::DetRand::new(
        ((unit_id as u64) << 32) ^ u64::from(wave) ^ 0xA24B_A9D7,
    );
    let angle = rng.unit_f32() * std::f32::consts::TAU;
    let len = rng.unit_f32() * WAVE_GROUND_SPREAD;
    (angle.cos() * len, angle.sin() * len)
}

fn wave_spawn_points(world: &DynamicWorld) -> Vec<(i16, i16)> {
    let mut points = world.enemy_spawns.read().clone();
    let (at_cores, team) = {
        let rules = world.wave_rules.read();
        (rules.waves_spawn_at_cores, rules.wave_team)
    };
    if !at_cores {
        return points;
    }
    if let Some(list) = world.team_core_lists.get(&team) {
        for core in list.iter() {
            let spawn = ((core.position >> 16) as i16, core.position as i16);
            if !points.contains(&spawn) {
                points.push(spawn);
            }
        }
    }
    points
}

fn payload_unit_for_wave(unit_type: i16, team: u8) -> Option<EnemyUnit> {
    let spec = enemy_spec(unit_type)?;
    Some(EnemyUnit {
        id: 0,
        unit_type: spec.unit_type,
        entity_class: spec.entity_class,
        team,
        x: 0.0,
        y: 0.0,
        rotation: -90.0,
        health: spec.health,
        shield: 0.0,
        status_effect: -1,
        status_duration: f32::MAX,
        statuses: Vec::new(),
        velocity_x: 0.0,
        velocity_y: 0.0,
        elevation: if crate::game::content::unit_movement(spec.unit_type).flying {
            1.0
        } else {
            0.0
        },
        payloads: Vec::new(),
        flag: 0.0,
        items: Vec::new(),
        mine_progress: 0.0,
        attack_reload: 0.0,
        secondary_attack_reload: 0.0,
        tertiary_attack_reload: 0.0,
        quaternary_attack_reload: 0.0,
        move_speed: spec.speed,
        attack_damage: spec.attack_damage,
        attack_reload_time: spec.attack_reload,
        attack_range: spec.attack_range,
        authority: UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        missile_time: 0.0,
        status_agg: Default::default(),
        drown_progress: 0.0,
    })
}

/// Official `UnitComp.updateDrowning`: deep liquid floors accumulate
/// `drownTime` and kill the unit at 0.999 (audit H4).
pub(crate) fn simulate_drowning(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let delta = delta_ticks.max(0.0);
    if delta <= 0.0 {
        return false;
    }
    let mut drowned = Vec::new();
    let mut changed = false;
    for mut unit in world.enemies.iter_mut() {
        let movement = crate::game::content::unit_movement(unit.unit_type);
        let flying = movement.flying || unit.elevation >= 0.09;
        let can_drown = !flying && !movement.naval;
        let floor = floor_at(world, unit.x, unit.y);
        let drown_time = crate::game::content::floor_drown_time(floor);
        if can_drown && drown_time > 0.0 {
            let hit = movement.hit_size.max(0.0001);
            let multiplier =
                crate::game::content::drown_time_multiplier(unit.unit_type).max(0.0001);
            unit.drown_progress += delta / (hit / 8.0 * drown_time * multiplier);
            if unit.drown_progress >= 0.999 {
                drowned.push(unit.id);
            }
            changed = true;
        } else if unit.drown_progress > 0.0 {
            unit.drown_progress = (unit.drown_progress - delta / 50.0).max(0.0);
            changed = true;
        }
        unit.drown_progress = unit.drown_progress.clamp(0.0, 1.0);
    }
    for id in drowned {
        crate::network::combat::kill_enemy(world, out, id);
        changed = true;
    }
    // Core-alpha is flying; possessed grounded units drown via `enemies`.
    // Any leftover grounded player avatar still accumulates drownTime (H4).
    let mut drowned_players = Vec::new();
    for session in world.player_sessions.iter() {
        if matches!(session.controlled_unit, ControlledUnit::Standard(_)) {
            continue;
        }
        if world.enemies.contains_key(&session.unit_id) {
            continue;
        }
        let movement = crate::game::content::unit_movement(35);
        let flying = movement.flying;
        let floor = floor_at(world, session.x, session.y);
        let drown_time = crate::game::content::floor_drown_time(floor);
        let key = session.unit_id;
        if !flying && drown_time > 0.0 {
            let hit = movement.hit_size.max(0.0001);
            let multiplier = crate::game::content::drown_time_multiplier(35).max(0.0001);
            let mut progress = world
                .game_state
                .extras
                .player_drown
                .get(&key)
                .map(|entry| *entry)
                .unwrap_or(0.0);
            progress += delta / (hit / 8.0 * drown_time * multiplier);
            if progress >= 0.999 {
                drowned_players.push(key);
                world.game_state.extras.player_drown.remove(&key);
            } else {
                world
                    .game_state
                    .extras
                    .player_drown
                    .insert(key, progress.min(1.0));
            }
            changed = true;
        } else if let Some(progress) = world.game_state.extras.player_drown.get(&key).map(|e| *e) {
            let next = (progress - delta / 50.0).max(0.0);
            if next <= 0.0 {
                world.game_state.extras.player_drown.remove(&key);
            } else {
                world.game_state.extras.player_drown.insert(key, next);
            }
            changed = true;
        }
    }
    for unit_id in drowned_players {
        crate::network::combat::damage_player(world, out, unit_id, 9999.0, -1, 0.0);
        changed = true;
    }
    changed
}
