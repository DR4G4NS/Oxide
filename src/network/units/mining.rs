//! Unit mining / repair / heal / navigation helpers plus the small tile/core
//! lookup leaves they depend on. The listener adapter re-exports these
//! through crate::network::listener::*.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};

use crate::network::buildings::snapshot::dynamic_tile_health;
use crate::network::economy::building_heal_suppressed;
use crate::network::units::EnemySpec;
use crate::network::world::{
    core_tile, core_world, DynamicTile, DynamicWorld, EnemyNavigationTarget, EnemyUnit,
    NavigationField,
};

use crate::network::buildings::construction::dynamic_at;
use crate::network::combat::enemy::{
    base_building_at, navigation_field, navigation_index, nearest_player_building,
};
use crate::network::combat::unit_combat::effective_unit_speed;
use crate::network::combat::unit_combat::unit_collision_layer;
use crate::network::wire::{encode_build_health_update_frame, mine_result};

pub(crate) fn move_repair_unit(
    world: &DynamicWorld,
    unit_id: i32,
    x: f32,
    y: f32,
    desired_range: f32,
    delta_ticks: f32,
) -> bool {
    let Some(unit) = world.enemies.get(&unit_id).map(|unit| unit.clone()) else {
        return false;
    };
    let Some((_, target_x, target_y)) =
        crate::network::combat::unit_combat::damaged_allied_building_target(
            world,
            unit.team,
            x,
            y,
            f32::INFINITY,
        )
    else {
        return false;
    };
    if (target_x - x).hypot(target_y - y) > desired_range {
        move_unit_toward(world, unit_id, target_x, target_y, delta_ticks)
    } else if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.velocity_x = 0.0;
        unit.velocity_y = 0.0;
        unit.rotation = (target_y - y).atan2(target_x - x).to_degrees();
        false
    } else {
        false
    }
}

/// Tiles per side of a cell in the static mineable-ore spatial index.
pub(crate) const ORE_CELL_TILES: i32 = 16;

/// Cells at Chebyshev radius `radius` around the origin cell (ring walk).
pub(crate) fn cell_ring(radius: i32) -> Vec<(i32, i32)> {
    if radius == 0 {
        return vec![(0, 0)];
    }
    let mut cells = Vec::with_capacity((radius * 8) as usize);
    for d in -radius..=radius {
        cells.push((d, -radius));
        cells.push((d, radius));
    }
    for d in (-radius + 1)..radius {
        cells.push((-radius, d));
        cells.push((radius, d));
    }
    cells
}

/// Builds the static mineable-ore spatial index for a hosted map: a coarse
/// grid over every base-map tile whose overlay/floor yields ore (same table
/// as `raw_mine_result`). Round 74 fix: `nearest_mineable_ore` used to expand
/// a square over the whole map for every mining unit every tick — with ore
/// far away or absent (18+ monos on a 200x200 map) each tick took seconds
/// and the 158.1 client disconnected after 20 s without entity snapshots.
pub(crate) fn build_mineable_ore_index(
    width: i32,
    height: i32,
    floors: &[i16],
    overlays: &[i16],
) -> crate::network::world::OreIndex {
    let cells_x = (width + ORE_CELL_TILES - 1) / ORE_CELL_TILES;
    let cells_y = (height + ORE_CELL_TILES - 1) / ORE_CELL_TILES;
    let mut grid = vec![Vec::new(); (cells_x * cells_y) as usize];
    let mut per_item = vec![0u32; 18];
    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            let item = match overlays[index] {
                167 => Some((0i16, 1u8)),  // copper
                168 => Some((1, 1)),       // lead
                169 => Some((8, 0)),       // scrap
                170 => Some((5, 2)),       // coal
                171 => Some((6, 3)),       // titanium
                172 | 175 => Some((7, 4)), // thorium
                173 => Some((16, 3)),      // beryllium
                174 => Some((17, 5)),      // tungsten
                _ => match floors[index] {
                    39 | 40 => Some((4, 0)), // sand / darksand
                    _ => None,
                },
            };
            if let Some((item, hardness)) = item {
                let cell = ((y / ORE_CELL_TILES) * cells_x + (x / ORE_CELL_TILES)) as usize;
                grid[cell].push(((x << 16) | y, item, hardness));
                per_item[item as usize] += 1;
            }
        }
    }
    let total = grid.iter().map(Vec::len).sum();
    crate::network::world::OreIndex {
        cells_x,
        cells_y,
        grid,
        per_item,
        total,
    }
}

pub(crate) fn nearest_mineable_ore(
    world: &DynamicWorld,
    x: f32,
    y: f32,
    carried_item: i16,
) -> Option<(i32, i16, u8, f32, f32)> {
    // Fast path (round 74): scan the static ore index instead of expanding a
    // square across the map. Same filtering as `choose_mining_ore`:
    // hardness <= 1, carried-item match, and the tile must be unbuilt.
    if let Some(index) = world.mineable_ore.get() {
        // Round 74: instant negative answer when the map has no ore of the
        // carried item (or no ore at all), instead of walking every cell.
        if carried_item >= 0 {
            let item = carried_item as usize;
            if item >= index.per_item.len() || index.per_item[item] == 0 {
                return None;
            }
        } else if index.total == 0 {
            return None;
        }
        // Round 74: precompute the flat set of blocked positions once per
        // search (live tiles, pending builds and map base buildings) so the
        // per-candidate check is an O(1) set lookup instead of a linear scan
        // over every tile. This is what made each fresh scan take ~200 ms in
        // debug when the mono was inside a dense building cluster.
        let mut blocked: std::collections::HashSet<i32> = world
            .tiles
            .iter()
            .filter(|tile| tile.block != 0)
            .flat_map(|tile| tile.occupied.clone())
            .collect();
        blocked.extend(
            world
                .pending_builds
                .iter()
                .flat_map(|build| build.occupied.clone()),
        );
        blocked.extend(
            world
                .base_buildings
                .iter()
                .flat_map(|building| building.occupied.clone()),
        );
        // Fast path (round 74): search cells in rings of increasing minimum
        // distance and stop once the next ring cannot beat the current best.
        let tile_x = (x / 8.0).round() as i32;
        let tile_y = (y / 8.0).round() as i32;
        let cell_x = tile_x.div_euclid(ORE_CELL_TILES);
        let cell_y = tile_y.div_euclid(ORE_CELL_TILES);
        let mut best: Option<(f32, i32, i16, u8, f32, f32)> = None;
        let max_radius = index.cells_x.max(index.cells_y);
        'rings: for radius in 0..=max_radius {
            let ring_min = (radius.saturating_sub(1).max(0) * ORE_CELL_TILES * 8) as f32;
            if best
                .as_ref()
                .is_some_and(|candidate| ring_min > candidate.0)
            {
                break 'rings;
            }
            for (cell_dx, cell_dy) in cell_ring(radius) {
                let cx = cell_x + cell_dx;
                let cy = cell_y + cell_dy;
                if cx < 0 || cy < 0 || cx >= index.cells_x || cy >= index.cells_y {
                    continue;
                }
                for (position, item, hardness) in &index.grid[(cy * index.cells_x + cx) as usize] {
                    if *hardness > 1 || (carried_item >= 0 && item != &carried_item) {
                        continue;
                    }
                    if blocked.contains(position) {
                        continue;
                    }
                    let target_x = (*position >> 16) as i16 as f32 * 8.0;
                    let target_y = *position as i16 as f32 * 8.0;
                    let distance = (target_x - x).hypot(target_y - y);
                    if best.as_ref().is_none_or(|candidate| distance < candidate.0) {
                        best = Some((distance, *position, *item, *hardness, target_x, target_y));
                    }
                }
            }
        }
        return best.map(|(_, position, item, hardness, target_x, target_y)| {
            (position, item, hardness, target_x, target_y)
        });
    }
    // Legacy expanding-square scan, kept for test worlds built without the
    // index (OnceLock unset).
    let center_x = (x / 8.0).round() as i32;
    let center_y = (y / 8.0).round() as i32;
    let maximum = world.width.max(world.height);
    for radius in 0..=maximum {
        let mut best = None;
        for dx in -radius..=radius {
            for dy in [-radius, radius] {
                best = choose_mining_ore(
                    world,
                    x,
                    y,
                    center_x + dx,
                    center_y + dy,
                    carried_item,
                    best,
                );
            }
        }
        if radius > 0 {
            for dy in (-radius + 1)..radius {
                for dx in [-radius, radius] {
                    best = choose_mining_ore(
                        world,
                        x,
                        y,
                        center_x + dx,
                        center_y + dy,
                        carried_item,
                        best,
                    );
                }
            }
        }
        if let Some((_, position, item, hardness, target_x, target_y)) = best {
            return Some((position, item, hardness, target_x, target_y));
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn choose_mining_ore(
    world: &DynamicWorld,
    source_x: f32,
    source_y: f32,
    tile_x: i32,
    tile_y: i32,
    carried_item: i16,
    current: Option<(f32, i32, i16, u8, f32, f32)>,
) -> Option<(f32, i32, i16, u8, f32, f32)> {
    if tile_x < 0 || tile_y < 0 || tile_x >= world.width || tile_y >= world.height {
        return current;
    }
    let position = (tile_x << 16) | tile_y;
    let Some((item, hardness)) = mine_result(world, position) else {
        return current;
    };
    if hardness > 1 || (carried_item >= 0 && item != carried_item) {
        return current;
    }
    let target_x = tile_x as f32 * 8.0;
    let target_y = tile_y as f32 * 8.0;
    let distance = (target_x - source_x).hypot(target_y - source_y);
    let candidate = (distance, position, item, hardness, target_x, target_y);
    if current.is_none_or(|best| distance < best.0) {
        Some(candidate)
    } else {
        current
    }
}

pub(crate) fn move_unit_toward(
    world: &DynamicWorld,
    unit_id: i32,
    target_x: f32,
    target_y: f32,
    delta_ticks: f32,
) -> bool {
    let Some(mut unit) = world.enemies.get_mut(&unit_id) else {
        return false;
    };
    let dx = target_x - unit.x;
    let dy = target_y - unit.y;
    let distance = dx.hypot(dy);
    if distance <= 0.001 {
        return false;
    }
    let speed = effective_unit_speed(&unit);
    let step = (speed * delta_ticks.max(0.0)).min(distance);
    unit.velocity_x = dx / distance * speed;
    unit.velocity_y = dy / distance * speed;
    unit.x += dx / distance * step;
    unit.y += dy / distance * step;
    unit.rotation = dy.atan2(dx).to_degrees();
    true
}

/// Whether `team` has at least one standing damaged building (RepairAI target).
pub(crate) fn team_has_damaged_building(world: &DynamicWorld, team: u8) -> bool {
    world.tiles.iter().any(|tile| {
        tile.block != 0
            && tile.team == team
            && dynamic_tile_health(&tile) < crate::game::content::block_health(tile.block) - 0.0001
    }) || world.base_buildings.iter().any(|building| {
        building.team == team
            && building.health < crate::game::content::block_health(building.block) - 0.0001
    })
}

/// RepairAI / BuilderAI idle retreat: if an enemy is inside `flee_range` and
/// the unit is farther than `retreat_dst` from its core, fly home.
pub(crate) fn unit_idle_retreat(
    world: &DynamicWorld,
    unit: &EnemyUnit,
    flee_range: f32,
    retreat_dst: f32,
    delta_ticks: f32,
) -> bool {
    if world
        .unit_orders
        .get(&unit.id)
        .is_some_and(|order| order.stances & (1_u32 << 6) != 0)
    {
        return false;
    }
    let threatened = world.enemies.iter().any(|other| {
        other.team != unit.team
            && other.id != unit.id
            && other.health > 0.0
            && (other.x - unit.x).hypot(other.y - unit.y) <= flee_range
    });
    if !threatened {
        return false;
    }
    let (core_x, core_y) = crate::network::world::core_world_for_team(world, unit.team);
    if (core_x - unit.x).hypot(core_y - unit.y) <= retreat_dst {
        return false;
    }
    move_unit_toward(world, unit.id, core_x, core_y, delta_ticks)
}

pub(crate) fn heal_buildings_in_radius(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    x: f32,
    y: f32,
    range: f32,
    percent: f32,
) -> bool {
    let mut targets: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == 1)
        .filter_map(|tile| {
            let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let target_y = tile.position as i16 as f32 * 8.0;
            ((target_x - x).hypot(target_y - y) <= range).then_some(tile.position)
        })
        .collect();
    targets.extend(world.base_buildings.iter().filter_map(|building| {
        let target_x = (building.position >> 16) as i16 as f32 * 8.0;
        let target_y = building.position as i16 as f32 * 8.0;
        (building.team == 1 && (target_x - x).hypot(target_y - y) <= range)
            .then_some(building.position)
    }));
    let mut updates = Vec::new();
    for position in targets {
        if let Some(health) = heal_building(world, position, percent, 0.0) {
            updates.push((position, health));
        }
    }
    let (core_x, core_y) = core_world(world);
    let mut core_changed = false;
    if (core_x - x).hypot(core_y - y) <= range {
        let mut health = world.game_state.core_health.write();
        let previous = *health;
        *health = (*health + world.core_max_health * percent / 100.0).min(world.core_max_health);
        core_changed = *health > previous;
    }
    if !updates.is_empty() {
        if let Ok(frame) = encode_build_health_update_frame(&updates) {
            out.broadcast(frame);
        }
        return true;
    }
    core_changed
}

pub(crate) fn heal_nearest_building(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    x: f32,
    y: f32,
    range: f32,
    percent: f32,
) -> bool {
    heal_nearest_building_by(world, out, x, y, range, percent, 0.0)
}

pub(crate) fn heal_nearest_building_flat(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    x: f32,
    y: f32,
    range: f32,
    amount: f32,
) -> bool {
    heal_nearest_building_by(world, out, x, y, range, 0.0, amount)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn heal_nearest_building_by(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    x: f32,
    y: f32,
    range: f32,
    percent: f32,
    amount: f32,
) -> bool {
    let mut candidates: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == 1)
        .filter_map(|tile| {
            let maximum = crate::game::content::block_health(tile.block);
            let current = dynamic_tile_health(&tile);
            let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let target_y = tile.position as i16 as f32 * 8.0;
            let distance = (target_x - x).hypot(target_y - y);
            (current < maximum - 0.0001 && distance <= range).then_some((distance, tile.position))
        })
        .collect();
    candidates.extend(world.base_buildings.iter().filter_map(|building| {
        let maximum = crate::game::content::block_health(building.block);
        let target_x = (building.position >> 16) as i16 as f32 * 8.0;
        let target_y = building.position as i16 as f32 * 8.0;
        let distance = (target_x - x).hypot(target_y - y);
        (building.team == 1 && building.health < maximum - 0.0001 && distance <= range)
            .then_some((distance, building.position))
    }));
    let Some((_, position)) = candidates
        .into_iter()
        .min_by(|left, right| left.0.total_cmp(&right.0))
    else {
        return false;
    };
    let Some(health) = heal_building(world, position, percent, amount) else {
        return false;
    };
    if let Ok(frame) = encode_build_health_update_frame(&[(position, health)]) {
        out.broadcast(frame);
    }
    true
}

pub(crate) fn heal_building(
    world: &DynamicWorld,
    position: i32,
    percent: f32,
    amount: f32,
) -> Option<f32> {
    heal_building_for_team(world, position, 1, percent, amount)
}

pub(crate) fn heal_building_for_team(
    world: &DynamicWorld,
    position: i32,
    team: u8,
    percent: f32,
    amount: f32,
) -> Option<f32> {
    if let Some(mut tile) = world.tiles.get_mut(&position) {
        if tile.block == 0
            || tile.team != team
            || building_heal_suppressed(world, position, tile.block)
        {
            return None;
        }
        let maximum = crate::game::content::block_health(tile.block);
        let current = dynamic_tile_health(&tile);
        let health = (current + maximum * percent / 100.0 + amount).min(maximum);
        if health <= current {
            return None;
        }
        tile.health = health;
        return Some(health);
    }
    let mut building = world.base_buildings.get_mut(&position)?;
    if building.team != team || building_heal_suppressed(world, position, building.block) {
        return None;
    }
    let maximum = crate::game::content::block_health(building.block);
    let health = (building.health + maximum * percent / 100.0 + amount).min(maximum);
    if health <= building.health {
        return None;
    }
    building.health = health;
    Some(health)
}

#[derive(Clone, Copy)]
pub(crate) struct UnitAvoidanceRequest {
    id: i32,
    tile_x: i32,
    tile_y: i32,
    radius: f32,
}

pub(crate) fn unit_avoidance_requests(world: &DynamicWorld) -> Vec<UnitAvoidanceRequest> {
    world
        .enemies
        .iter()
        .filter(|unit| {
            unit.team == world.wave_rules.read().wave_team && unit_collision_layer(unit) == 0
        })
        .filter_map(|unit| {
            let movement = crate::game::content::unit_movement(unit.unit_type);
            movement.physics.then_some(UnitAvoidanceRequest {
                id: unit.id,
                tile_x: (unit.x / 8.0).floor() as i32,
                tile_y: (unit.y / 8.0).floor() as i32,
                radius: movement.hit_size * 0.6 / 8.0 * 2.0,
            })
        })
        .collect()
}

pub(crate) fn navigation_tile_avoided(
    requests: &[UnitAvoidanceRequest],
    current_id: i32,
    tile_x: i32,
    tile_y: i32,
) -> bool {
    requests.iter().any(|other| {
        if other.id >= current_id {
            return false;
        }
        let dx = tile_x - other.tile_x;
        let dy = tile_y - other.tile_y;
        (dx * dx + dy * dy) as f32 <= other.radius * other.radius
    })
}

#[derive(Clone, Copy)]
enum WaveBlockFlag {
    Core,
    Storage,
    Generator,
    LaunchPad,
    Factory,
    Repair,
    Battery,
    Reactor,
    Drill,
    Any,
}

fn block_has_wave_flag(block: i16, flag: WaveBlockFlag) -> bool {
    match flag {
        WaveBlockFlag::Core => crate::network::buildings::snapshot::is_core_block(block),
        WaveBlockFlag::Storage => matches!(block, 345..=348),
        WaveBlockFlag::Generator => matches!(block, 308 | 311 | 313 | 314 | 315 | 316 | 320..=322),
        WaveBlockFlag::LaunchPad => matches!(block, 426 | 427),
        WaveBlockFlag::Factory => matches!(block, 182..=191 | 377..=392),
        WaveBlockFlag::Repair => matches!(block, 245..=249),
        WaveBlockFlag::Battery => matches!(block, 306 | 307 | 317 | 318),
        WaveBlockFlag::Reactor => matches!(block, 315 | 316),
        WaveBlockFlag::Drill => matches!(block, 325..=328 | 337 | 338),
        WaveBlockFlag::Any => block != 0,
    }
}

fn flying_target_flags(unit_type: i16) -> &'static [WaveBlockFlag] {
    match unit_type {
        15 => &[WaveBlockFlag::Generator, WaveBlockFlag::Any],
        16 => &[WaveBlockFlag::Factory, WaveBlockFlag::Any],
        17 => &[
            WaveBlockFlag::LaunchPad,
            WaveBlockFlag::Storage,
            WaveBlockFlag::Battery,
            WaveBlockFlag::Any,
        ],
        18 => &[WaveBlockFlag::Generator, WaveBlockFlag::Core],
        19 => &[
            WaveBlockFlag::Reactor,
            WaveBlockFlag::Battery,
            WaveBlockFlag::Core,
        ],
        23 => &[
            WaveBlockFlag::Battery,
            WaveBlockFlag::Factory,
            WaveBlockFlag::Any,
        ],
        _ => &[WaveBlockFlag::Any],
    }
}

const RANDOM_WAVE_FLAGS: &[WaveBlockFlag] = &[
    WaveBlockFlag::Core,
    WaveBlockFlag::Storage,
    WaveBlockFlag::Generator,
    WaveBlockFlag::LaunchPad,
    WaveBlockFlag::Factory,
    WaveBlockFlag::Repair,
    WaveBlockFlag::Battery,
    WaveBlockFlag::Reactor,
    WaveBlockFlag::Drill,
];

fn nearest_flagged_building(
    world: &DynamicWorld,
    enemy: &EnemyUnit,
    flag: WaveBlockFlag,
) -> Option<(i32, f32, f32)> {
    let defender = world.wave_rules.read().default_team;
    let dynamic = world.tiles.iter().filter_map(|tile| {
        if tile.block == 0 || tile.team != defender || !block_has_wave_flag(tile.block, flag) {
            return None;
        }
        let x = (tile.position >> 16) as i16 as f32 * 8.0;
        let y = tile.position as i16 as f32 * 8.0;
        Some(((x - enemy.x).hypot(y - enemy.y), tile.position, x, y))
    });
    let base = world.base_buildings.iter().filter_map(|building| {
        if building.team != defender || !block_has_wave_flag(building.block, flag) {
            return None;
        }
        let x = (building.position >> 16) as i16 as f32 * 8.0;
        let y = building.position as i16 as f32 * 8.0;
        Some(((x - enemy.x).hypot(y - enemy.y), building.position, x, y))
    });
    dynamic
        .chain(base)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, position, x, y)| (position, x, y))
}

fn flying_ai_navigation_target(
    world: &DynamicWorld,
    enemy: &EnemyUnit,
    core_x: f32,
    core_y: f32,
) -> EnemyNavigationTarget {
    let core_distance = (core_x - enemy.x).hypot(core_y - enemy.y);
    if core_distance <= enemy.attack_range {
        return EnemyNavigationTarget {
            building: None,
            movement: (core_x, core_y),
        };
    }
    let random = world.wave_rules.read().random_wave_ai;
    let waves = world.wave_rules.read().waves_enabled;
    let wave = world
        .game_state
        .wave
        .load(std::sync::atomic::Ordering::Relaxed);
    if random {
        let seed = u64::from(enemy.unit_type as u16)
            + if waves {
                u64::from(wave)
            } else {
                u64::from(enemy.id as u32)
            };
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
        for _ in 0..5 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let flag = RANDOM_WAVE_FLAGS[(state as usize) % RANDOM_WAVE_FLAGS.len()];
            if let Some((position, x, y)) = nearest_flagged_building(world, enemy, flag) {
                return EnemyNavigationTarget {
                    building: Some((position, x, y)),
                    movement: (x, y),
                };
            }
        }
        if let Some((position, x, y)) = nearest_player_building(world, enemy.x, enemy.y) {
            return EnemyNavigationTarget {
                building: Some((position, x, y)),
                movement: (x, y),
            };
        }
    } else {
        for flag in flying_target_flags(enemy.unit_type) {
            if let Some((position, x, y)) = nearest_flagged_building(world, enemy, *flag) {
                return EnemyNavigationTarget {
                    building: Some((position, x, y)),
                    movement: (x, y),
                };
            }
        }
    }
    EnemyNavigationTarget {
        building: None,
        movement: (core_x, core_y),
    }
}

pub(crate) fn enemy_navigation_target(
    world: &DynamicWorld,
    enemy: &EnemyUnit,
    core_x: f32,
    core_y: f32,
    avoidance: &[UnitAvoidanceRequest],
) -> EnemyNavigationTarget {
    let flying =
        crate::game::content::unit_movement(enemy.unit_type).flying || enemy.elevation >= 0.09;
    if flying {
        return flying_ai_navigation_target(world, enemy, core_x, core_y);
    }
    let legs = matches!(enemy.unit_type, 11..=14);
    let costs = if crate::game::content::unit_movement(enemy.unit_type).naval {
        crate::network::combat::navigation_field_class(
            world,
            crate::network::combat::enemy::NavigationClass::Naval,
        )
    } else {
        navigation_field(world, legs)
    };
    let x = crate::network::combat::enemy::world_to_tile_in_map(enemy.x, world.width);
    let y = crate::network::combat::enemy::world_to_tile_in_map(enemy.y, world.height);
    let Some(mut index) = navigation_index(world, x, y) else {
        return EnemyNavigationTarget {
            building: None,
            movement: (enemy.x, enemy.y),
        };
    };
    let mut first_step = None;
    let maximum_steps = ((world.width + world.height).max(1) * 2) as usize;
    for _ in 0..maximum_steps {
        let current_cost = costs[index];
        if current_cost == 0 {
            break;
        }
        let next = if first_step.is_none() {
            choose_navigation_step_with(&costs, world.width, world.height, index, |candidate| {
                let candidate_x = candidate as i32 % world.width;
                let candidate_y = candidate as i32 / world.width;
                !navigation_tile_avoided(avoidance, enemy.id, candidate_x, candidate_y)
            })
        } else {
            choose_navigation_step(&costs, world.width, world.height, index)
        };
        let Some(next) = next else {
            break;
        };
        let next_x = next as i32 % world.width;
        let next_y = next as i32 / world.width;
        if crate::game::content::unit_movement(enemy.unit_type).naval
            && !crate::game::content::floor_is_liquid(crate::network::combat::floor_at_tile(
                world, next_x, next_y,
            ))
        {
            break;
        }
        first_step.get_or_insert((next_x as f32 * 8.0, next_y as f32 * 8.0));
        let position = (next_x << 16) | (next_y as u16 as i32);
        if !legs {
            if let Some(tile) = dynamic_at(world, position).filter(|tile| {
                tile.block != 0
                    && tile.team == world.wave_rules.read().default_team
                    && crate::game::content::block_navigation(tile.block).solid
            }) {
                return EnemyNavigationTarget {
                    building: Some((tile.position, next_x as f32 * 8.0, next_y as f32 * 8.0)),
                    movement: first_step.unwrap(),
                };
            }
            if let Some(building) = base_building_at(world, position).filter(|building| {
                building.team == world.wave_rules.read().default_team
                    && crate::game::content::block_navigation(building.block).solid
            }) {
                let x = (building.position >> 16) as i16 as f32 * 8.0;
                let y = building.position as i16 as f32 * 8.0;
                return EnemyNavigationTarget {
                    building: Some((building.position, x, y)),
                    movement: first_step.unwrap(),
                };
            }
        }
        index = next;
    }
    EnemyNavigationTarget {
        building: None,
        movement: first_step.unwrap_or((enemy.x, enemy.y)),
    }
}

/// First flow-field step toward an arbitrary world point (Pathfinder
/// `PositionTarget`). Flying units go straight; grounded/naval units walk
/// the destination field so concave walls do not stall command/squad orders.
pub(crate) fn first_flow_step_toward(
    world: &DynamicWorld,
    enemy: &EnemyUnit,
    goal_x: f32,
    goal_y: f32,
    avoidance: &[UnitAvoidanceRequest],
) -> (f32, f32) {
    if crate::game::content::unit_movement(enemy.unit_type).flying || enemy.elevation >= 0.09 {
        return (goal_x, goal_y);
    }
    let class = if crate::game::content::unit_movement(enemy.unit_type).naval {
        crate::network::combat::enemy::NavigationClass::Naval
    } else if matches!(enemy.unit_type, 11..=14) {
        crate::network::combat::enemy::NavigationClass::Legs
    } else {
        crate::network::combat::enemy::NavigationClass::Ground
    };
    let costs = crate::network::combat::navigation_field_toward(
        world,
        class,
        crate::network::combat::enemy::world_to_tile_in_map(goal_x, world.width),
        crate::network::combat::enemy::world_to_tile_in_map(goal_y, world.height),
        enemy.team,
    );
    let x = crate::network::combat::enemy::world_to_tile_in_map(enemy.x, world.width);
    let y = crate::network::combat::enemy::world_to_tile_in_map(enemy.y, world.height);
    let Some(index) = navigation_index(world, x, y) else {
        return (goal_x, goal_y);
    };
    let next = choose_navigation_step_with(&costs, world.width, world.height, index, |candidate| {
        let candidate_x = candidate as i32 % world.width;
        let candidate_y = candidate as i32 / world.width;
        !navigation_tile_avoided(avoidance, enemy.id, candidate_x, candidate_y)
    });
    match next {
        Some(next) => {
            let next_x = next as i32 % world.width;
            let next_y = next as i32 / world.width;
            (next_x as f32 * 8.0, next_y as f32 * 8.0)
        }
        None => (goal_x, goal_y),
    }
}

pub(crate) fn choose_navigation_step_with<F>(
    costs: &[u32],
    width: i32,
    height: i32,
    index: usize,
    mut allowed: F,
) -> Option<usize>
where
    F: FnMut(usize) -> bool,
{
    const IMPASSABLE: u32 = u32::MAX / 4;
    let current_cost = *costs.get(index)?;
    let current_x = index as i32 % width;
    let current_y = index as i32 / width;
    let at = |x: i32, y: i32| {
        (x >= 0 && y >= 0 && x < width && y < height).then_some((y * width + x) as usize)
    };

    [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ]
    .into_iter()
    .filter_map(|(dx, dy)| {
        let next = at(current_x + dx, current_y + dy)?;
        if costs[next] >= current_cost || !allowed(next) {
            return None;
        }
        if dx != 0 && dy != 0 {
            let horizontal = at(current_x + dx, current_y)?;
            let vertical = at(current_x, current_y + dy)?;
            if costs[horizontal] >= IMPASSABLE || costs[vertical] >= IMPASSABLE {
                return None;
            }
        }
        Some(next)
    })
    .min_by_key(|next| costs[*next])
}

pub(crate) fn choose_navigation_step(
    costs: &[u32],
    width: i32,
    height: i32,
    index: usize,
) -> Option<usize> {
    choose_navigation_step_with(costs, width, height, index, |_| true)
}
