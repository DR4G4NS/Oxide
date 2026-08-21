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
    world
        .enemies
        .iter()
        .filter(|unit| unit.team == world.wave_rules.read().wave_team)
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

pub(crate) fn navigation_index(world: &DynamicWorld, x: i32, y: i32) -> Option<usize> {
    (x >= 0 && y >= 0 && x < world.width && y < world.height)
        .then_some((y * world.width + x) as usize)
}

pub(crate) fn navigation_field(world: &DynamicWorld, legs: bool) -> Arc<Vec<u32>> {
    let revision = world.navigation_revision.load(Ordering::Relaxed);
    let cache = if legs {
        &world.leg_navigation
    } else {
        &world.ground_navigation
    };
    let mut cached = cache.lock();
    if let Some(field) = cached.as_ref().filter(|field| field.revision == revision) {
        return field.costs.clone();
    }
    let costs = Arc::new(build_navigation_field(world, legs));
    *cached = Some(NavigationField {
        revision,
        costs: costs.clone(),
    });
    costs
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

pub(crate) fn build_navigation_field(world: &DynamicWorld, legs: bool) -> Vec<u32> {
    const IMPASSABLE: u32 = u32::MAX / 4;

    let total = (world.width * world.height).max(0) as usize;
    let mut dynamic_cells = HashMap::new();
    for tile in world.tiles.iter().filter(|tile| tile.block != 0) {
        let health = dynamic_tile_health(&tile);
        for position in &tile.occupied {
            dynamic_cells.insert(*position, (tile.block, tile.team, health));
        }
        dynamic_cells
            .entry(tile.position)
            .or_insert((tile.block, tile.team, health));
    }
    let mut base_cells = HashMap::new();
    for building in world.base_buildings.iter() {
        for position in &building.occupied {
            base_cells.insert(*position, (building.block, building.team, building.health));
        }
    }
    let (target_x, target_y) = core_tile(world);
    let target_x = i32::from(target_x);
    let target_y = i32::from(target_y);
    let Some(target) = navigation_index(world, target_x, target_y) else {
        return vec![IMPASSABLE; total];
    };
    let tile_cost = |x: i32, y: i32| {
        if x == target_x && y == target_y {
            return 1;
        }
        let index = (y * world.width + x) as usize;
        let floor_id = world.floors[index];
        let floor = crate::game::content::block_navigation(floor_id);
        let terrain = 1 + u32::from(floor.deep) * 6000 + u32::from(floor.damages) * 30;
        let position = (x << 16) | (y as u16 as i32);
        let effective_block = dynamic_cells
            .get(&position)
            .map(|(block, _, _)| *block)
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
        if let Some((block, team, health)) = dynamic_cells.get(&position).copied() {
            let navigation = crate::game::content::block_navigation(block);
            if navigation.solid {
                if legs {
                    return terrain + 5;
                }
                if team == world.wave_rules.read().wave_team && !navigation.team_passable {
                    return IMPASSABLE;
                }
                let scaled_health = ((health / 40.0) as u32).min(80);
                return terrain + scaled_health * 5;
            }
        }
        if let Some((block, team, health)) = base_cells.get(&position).copied() {
            let navigation = crate::game::content::block_navigation(block);
            if navigation.solid {
                if legs {
                    return terrain + 5;
                }
                if team == world.wave_rules.read().wave_team && !navigation.team_passable {
                    return IMPASSABLE;
                }
                let scaled_health = ((health / 40.0) as u32).min(80);
                return terrain + scaled_health * 5;
            }
        }
        let base = crate::game::content::block_navigation(world.base_blocks[index]);
        if base.solid {
            if legs {
                terrain + 5
            } else {
                IMPASSABLE
            }
        } else {
            terrain
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

pub(crate) fn damage_building(
    world: &DynamicWorld,
    position: i32,
    damage: f32,
) -> Option<(bool, f32)> {
    if let Some(mut tile) = world.tiles.get_mut(&position) {
        let max_health = crate::game::content::block_health(tile.block);
        if tile.health <= 0.0 || tile.health > max_health {
            tile.health = max_health;
        }
        // Official Rules: blockDamage(team) scales incoming damage
        // (BulletType.damageMultiplier, JAR), then Building.damage divides
        // by blockHealth(team) (Rules.blockHealth = global * TeamRule of the
        // BUILDING's team, Rules.java): a blockHealth of 3 means buildings
        // take one third of the damage. A zero/absent multiplier destroys
        // the building outright (official `Mathf.zero(dm)`).
        let building_team = tile.team;
        let rules = world.wave_rules.read();
        let team_rule = rules.team_rule(building_team);
        let scaled =
            damage * (rules.block_damage_multiplier * team_rule.block_damage_multiplier).max(0.0);
        let health_mult = rules.block_health_multiplier * team_rule.block_health_multiplier;
        let effective = if health_mult.abs() <= 0.0001 {
            tile.health + 1.0
        } else {
            scaled / health_mult
        };
        tile.health -= apply_unit_armor(effective, crate::game::content::block_armor(tile.block));
        let destroyed = tile.health <= 0.0;
        let health = tile.health.max(0.0);
        let destroyed_state = destroyed.then(|| tile.clone());
        drop(tile);
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
    let mut building = world.base_buildings.get_mut(&position)?;
    let building_team = building.team;
    let rules = world.wave_rules.read();
    let team_rule = rules.team_rule(building_team);
    let scaled =
        damage * (rules.block_damage_multiplier * team_rule.block_damage_multiplier).max(0.0);
    let health_mult = rules.block_health_multiplier * team_rule.block_health_multiplier;
    let effective = if health_mult.abs() <= 0.0001 {
        building.health + 1.0
    } else {
        scaled / health_mult
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
        if unit_type == 24 {
            // Oct RepairFieldAbility(130, 120, 140): every 120 ticks heals
            // 130 HP to EVERY allied unit within 140 (Units.nearby includes
            // the oct itself; buildings are not healed).
            if crossed(120.0) {
                let allies: Vec<_> = world
                    .enemies
                    .iter()
                    .filter(|ally| {
                        ally.team == team
                            && ally.health < enemy_max_health(ally)
                            && (ally.x - x).hypot(ally.y - y) <= 140.0
                    })
                    .map(|ally| ally.id)
                    .collect();
                for ally_id in allies {
                    if let Some(mut ally) = world.enemies.get_mut(&ally_id) {
                        ally.health = (ally.health + 130.0).min(enemy_max_health(&ally));
                    }
                }
                if team == 1 {
                    for mut player in world.players.iter_mut() {
                        if !player.dead
                            && player.health < 150.0
                            && (player.x - x).hypot(player.y - y) <= 140.0
                        {
                            player.health = (player.health + 130.0).min(150.0);
                        }
                    }
                }
            }
            continue;
        }
        if unit_type == 21 {
            // Poly RepairFieldAbility(5, 480, 50): every 480 ticks heal 5 HP
            // to damaged allies within 50 tiles.
            if crossed(480.0) {
                let allies: Vec<_> = world
                    .enemies
                    .iter()
                    .filter(|ally| {
                        ally.team == team
                            && ally.health < enemy_max_health(ally)
                            && (ally.x - x).hypot(ally.y - y) <= 50.0
                    })
                    .map(|ally| ally.id)
                    .collect();
                for ally_id in allies {
                    if let Some(mut ally) = world.enemies.get_mut(&ally_id) {
                        ally.health = (ally.health + 5.0).min(enemy_max_health(&ally));
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
        // Nova RepairFieldAbility(10, 240, 60); Pulsar/Bryde
        // ShieldRegenFieldAbility(20, 40, reload, 60).
        let (period, heal, shield_amount, shield_cap) = match unit_type {
            5 => (240.0, Some(10.0), None, 0.0),
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
        .map(|spec| {
            spec.health * status_multipliers_composite(enemy.status_effect, &enemy.statuses).0
        })
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

pub(crate) fn spawn_wave(world: &DynamicWorld) {
    if world.enemy_spawns.is_empty() {
        warn!("Cannot spawn wave: bundled map has no spawn overlays");
        return;
    }
    let wave = world.game_state.wave.fetch_add(1, Ordering::Relaxed);
    world.game_state.game_stats.write().waves_lasted += 1;
    // Prefer the loaded map's Rules.spawns (official WaveSpawner); fall back
    // to the bundled maze table only when the map defines no spawns.
    let groups = if world.wave_rules.read().is_default() {
        initial_official_wave_groups(wave - 1)
    } else {
        map_wave_spawns(wave - 1, &world.wave_rules.read())
    };
    let amount: u32 = groups.iter().map(|group| group.amount).sum();
    let mut index = 0u32;
    for group in groups {
        let (health_multiplier, speed_multiplier, damage_multiplier) =
            crate::game::status::status_multipliers(group.status_effect);
        // Official WaveSpawner.eachGroundSpawn(group.spawn, ...): groups with a
        // packed spawn position only use that spawn point; others use all.
        let spawns: Vec<(i16, i16)> = if group.spawn >= 0 {
            let (sx, sy) = ((group.spawn >> 16) as i16, (group.spawn & 0xffff) as i16);
            let matched: Vec<(i16, i16)> = world
                .enemy_spawns
                .iter()
                .copied()
                .filter(|(x, y)| *x == sx && *y == sy)
                .collect();
            if matched.is_empty() {
                world.enemy_spawns.clone()
            } else {
                matched
            }
        } else {
            world.enemy_spawns.clone()
        };
        for _ in 0..group.amount {
            let (tile_x, tile_y) = spawns[index as usize % spawns.len()];
            let id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
            let spread = (index / spawns.len() as u32) as f32 * 5.0;
            world.enemies.insert(
                id,
                EnemyUnit {
                    id,
                    unit_type: group.spec.unit_type,
                    entity_class: group.spec.entity_class,
                    team: world.wave_rules.read().wave_team,
                    x: tile_x as f32 * 8.0 + spread,
                    y: tile_y as f32 * 8.0,
                    rotation: -90.0,
                    health: group.spec.health * health_multiplier,
                    shield: group.shield,
                    status_effect: group.status_effect,
                    status_duration: f32::MAX,
                    statuses: if group.status_effect >= 0 {
                        vec![crate::game::status::ActiveStatus::simple(
                            group.status_effect,
                            f32::MAX,
                        )]
                    } else {
                        Vec::new()
                    },
                    velocity_x: 0.0,
                    velocity_y: 0.0,
                    elevation: 0.0,
                    payloads: Vec::new(),
                    flag: 0.0,
                    items: Vec::new(),
                    mine_progress: 0.0,
                    attack_reload: 0.0,
                    secondary_attack_reload: 0.0,
                    tertiary_attack_reload: 0.0,
                    quaternary_attack_reload: 0.0,
                    move_speed: group.spec.speed * speed_multiplier,
                    attack_damage: group.spec.attack_damage * damage_multiplier,
                    attack_reload_time: group.spec.attack_reload,
                    attack_range: group.spec.attack_range,
                    authority: UnitAuthority::DefaultAi,
                    build_plans: Vec::new(),
                    update_building: true,
                    status_agg: Default::default(),
                },
            );
            world.register_unit_group(id);
            index += 1;
        }
    }
    world
        .game_state
        .enemies_count
        .store(hostile_unit_count(world), Ordering::Relaxed);
    info!(
        "Spawned official wave {} with {} supported units",
        wave, amount
    );
}
