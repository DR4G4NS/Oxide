//! Turret/base combat simulation: ammo, aim, base menders/turrets,
//! simulate_turrets.

use crate::network::buildings::snapshot::dynamic_tile_health;
use crate::network::economy::spec::{
    inventory_remove, is_supported_item_turret, liquid_turret_weapon, power_turret_weapon,
    turret_ammo, turret_can_target, turret_shots, TurretAmmo,
};
use crate::network::units::controller::controlling_session_for_building;
use crate::network::wire::auth::player_team;
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

pub(crate) fn default_turret_ammo(block: i16) -> Option<TurretAmmo> {
    (0..22).find_map(|item| turret_ammo(block, item))
}

/// Direct `collidesTeam` healing for player-triggerable unit projectiles.
/// Splash-only healers, Pulsar's nested lightning and RepairBeamWeapon mounts
/// use separate paths.
pub(crate) const fn projectile_direct_heal_percent(bullet_id: i16) -> Option<f32> {
    match bullet_id {
        14 => Some(5.0), // nova LaserBoltBulletType
        17 => Some(10.0),
        18 => Some(1.0),
        20 => Some(25.0),
        37 | 38 => Some(5.5),
        39 => Some(3.0),
        51 => Some(5.5),
        53 => Some(1.5),
        _ => None,
    }
}

pub(crate) const fn projectile_splash_heal_percent(bullet_id: i16) -> Option<f32> {
    match bullet_id {
        40 => Some(15.0), // quad bomb
        52 => Some(4.0),  // retusa mine
        57 => Some(2.8),  // cyerce plasma-missile fragments
        _ => None,
    }
}

/// First damaged allied building intersected by the authoritative mouse ray.
/// `collidesTeam` bullets use this instead of snapping to the nearest repair
/// target, preserving Java's explicit manual aiming semantics.
pub(crate) fn damaged_allied_building_on_ray(
    world: &DynamicWorld,
    team: u8,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
) -> Option<(i32, f32, f32)> {
    let mut seen = HashSet::new();
    let mut candidates = world
        .tiles
        .iter()
        .filter_map(|tile| {
            if tile.block == 0
                || tile.team != team
                || !seen.insert(tile.position)
                || building_heal_suppressed(world, tile.position, tile.block)
            {
                return None;
            }
            let maximum = crate::game::content::block_health(tile.block);
            if dynamic_tile_health(&tile) >= maximum - 0.0001 {
                return None;
            }
            let x = (tile.position >> 16) as i16 as f32 * 8.0;
            let y = tile.position as i16 as f32 * 8.0;
            let radius = f32::from(crate::game::content::block_size(tile.block)) * 4.0 + 2.0;
            point_hits_segment(x, y, source_x, source_y, target_x, target_y, radius)
                .map(|progress| (progress, tile.position, x, y))
        })
        .collect::<Vec<_>>();
    candidates.extend(world.base_buildings.iter().filter_map(|building| {
        if building.team != team
            || !seen.insert(building.position)
            || building_heal_suppressed(world, building.position, building.block)
            || building.health >= crate::game::content::block_health(building.block) - 0.0001
        {
            return None;
        }
        let x = (building.position >> 16) as i16 as f32 * 8.0;
        let y = building.position as i16 as f32 * 8.0;
        let radius = f32::from(crate::game::content::block_size(building.block)) * 4.0 + 2.0;
        point_hits_segment(x, y, source_x, source_y, target_x, target_y, radius)
            .map(|progress| (progress, building.position, x, y))
    }));
    candidates
        .into_iter()
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, position, x, y)| (position, x, y))
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ManualAim {
    pub(crate) target_id: i32,
    pub(crate) distance: f32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ControlledWeaponInput {
    Automatic,
    Idle,
    Firing(ManualAim),
}

/// Resolves an authoritative firing ray from the latest client aim. A manual
/// shot is valid even when it points at empty ground; `target_id` is only an
/// optimization for direct unit impacts, while the projectile/building and
/// PvP collision passes still inspect the complete segment.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_manual_aim(
    world: &DynamicWorld,
    team: u8,
    source_x: f32,
    source_y: f32,
    aim_x: f32,
    aim_y: f32,
    range: f32,
    can_target: impl Fn(&EnemyUnit) -> bool,
) -> Option<ManualAim> {
    if !aim_x.is_finite() || !aim_y.is_finite() || !range.is_finite() || range <= 0.0 {
        return None;
    }
    let dx = aim_x - source_x;
    let dy = aim_y - source_y;
    let raw_distance = dx.hypot(dy);
    if raw_distance <= 0.001 {
        return None;
    }
    let distance = raw_distance.min(range);
    let target_x = source_x + dx / raw_distance * distance;
    let target_y = source_y + dy / raw_distance * distance;
    let target_id = world
        .enemies
        .iter()
        .filter(|unit| unit.team != team && can_target(unit))
        .filter_map(|unit| {
            let along =
                point_hits_segment(unit.x, unit.y, source_x, source_y, target_x, target_y, 8.0)?;
            Some((along, unit.id))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map_or(-1, |(_, id)| id);
    Some(ManualAim {
        target_id,
        distance,
        x: target_x,
        y: target_y,
    })
}

pub(crate) fn controlled_building_weapon_input(
    world: &DynamicWorld,
    position: i32,
    team: u8,
    source_x: f32,
    source_y: f32,
    range: f32,
    can_target: impl Fn(&EnemyUnit) -> bool,
) -> ControlledWeaponInput {
    let Some(controller) = controlling_session_for_building(world, position) else {
        return ControlledWeaponInput::Automatic;
    };
    // Do not let a stale session retain a captured block after a team/map
    // transition. UnitControl itself enforces the same ownership gate.
    if player_team(world, &controller) != team
        || !controller.shooting
        || controller.last_shot.elapsed() > std::time::Duration::from_millis(500)
    {
        return ControlledWeaponInput::Idle;
    }
    resolve_manual_aim(
        world,
        team,
        source_x,
        source_y,
        controller.mouse_x,
        controller.mouse_y,
        range,
        can_target,
    )
    .map_or(ControlledWeaponInput::Idle, ControlledWeaponInput::Firing)
}

/// SOL-001: prebuilt map turrets (base_buildings) fire at the nearest enemy
/// in range using their default ammo (the map's conveyor-fed ammo supply is
/// not simulated; documented approximation). Same reload/targeting rules as
/// dynamic turrets.
/// SOL-001: prebuilt map menders repair the owning team's damaged buildings
/// in range (dynamic tiles and base buildings). Same heal rate as dynamic
/// menders (heal_percent of max health per reload); power is approximated
/// as always available.
pub(crate) fn simulate_base_menders(world: &DynamicWorld, delta_ticks: f32) -> bool {
    // A loaded map building is represented in both registries. The dynamic
    // tile is authoritative; only run this compatibility path for base-only
    // entries, otherwise a prebuilt mender would heal twice per cycle.
    let base_candidates: Vec<i32> = world
        .base_buildings
        .iter()
        .filter(|building| crate::network::economy::mender_spec(building.block).is_some())
        .map(|building| *building.key())
        .collect();
    let keys: Vec<i32> = base_candidates
        .into_iter()
        .filter(|key| !world.tiles.contains_key(key))
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(building) = world.base_buildings.get(&key).map(|b| b.clone()) else {
            continue;
        };
        let Some(spec) = crate::network::economy::mender_spec(building.block) else {
            continue;
        };
        let source_x = (building.position >> 16) as i16 as f32 * 8.0;
        let source_y = building.position as i16 as f32 * 8.0;
        let mut progress = *world.base_mender_progress.entry(key).or_insert(0.0).value();
        progress += delta_ticks;
        if progress < spec.reload {
            world.base_mender_progress.insert(key, progress);
            continue;
        }
        world.base_mender_progress.insert(key, 0.0);
        let heal_scale = spec.heal_percent / 100.0;
        let in_range = |x: f32, y: f32| (x - source_x).hypot(y - source_y) <= spec.range;
        // Heal damaged dynamic tiles of the same team in range.
        let dynamic: Vec<i32> = world
            .tiles
            .iter()
            .filter(|tile| {
                tile.team == building.team
                    && tile.health < crate::game::content::block_health(tile.block)
            })
            .map(|tile| (tile.position, tile.team, tile.health))
            .filter(|(pos, _, _)| {
                in_range(
                    (pos >> 16) as i16 as f32 * 8.0,
                    (*pos & 0xFFFF) as i16 as f32 * 8.0,
                )
            })
            .map(|(pos, _, _)| pos)
            .collect();
        for position in dynamic {
            if let Some(mut target) = world.tiles.get_mut(&position) {
                let maximum = crate::game::content::block_health(target.block);
                target.health = (target.health + maximum * heal_scale).min(maximum);
                changed = true;
            }
        }
        // Heal damaged base buildings of the same team in range.
        let base_candidates: Vec<i32> = world
            .base_buildings
            .iter()
            .filter(|b| {
                b.team == building.team
                    && b.health < crate::game::content::block_health(b.block)
                    && in_range(
                        (b.position >> 16) as i16 as f32 * 8.0,
                        b.position as i16 as f32 * 8.0,
                    )
            })
            .map(|b| *b.key())
            .collect();
        // Do not heal the compatibility copy when the authoritative dynamic
        // tile exists at the same position.
        let base: Vec<i32> = base_candidates
            .into_iter()
            .filter(|position| !world.tiles.contains_key(position))
            .collect();
        for position in base {
            if let Some(mut target) = world.base_buildings.get_mut(&position) {
                let maximum = crate::game::content::block_health(target.block);
                if target.health < maximum {
                    target.health = (target.health + maximum * heal_scale).min(maximum);
                    changed = true;
                }
            }
        }
    }
    changed
}

pub(crate) fn simulate_base_turrets(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    // A loaded building exists in both registries. This compatibility path
    // is exclusively for base-only entries; dynamic turrets are simulated
    // below with their real ammo/power state and must never fire twice.
    let base_candidates: Vec<i32> = world
        .base_buildings
        .iter()
        .filter(|building| {
            is_supported_item_turret(building.block)
                || power_turret_weapon(building.block).is_some()
                || matches!(building.block, 353 | 360)
        })
        .map(|building| *building.key())
        .collect();
    let keys: Vec<i32> = base_candidates
        .into_iter()
        .filter(|key| !world.tiles.contains_key(key))
        .collect();
    let mut changed = false;
    for key in keys {
        let (building, loaded_tile) = if let Some(tile) = world.tiles.get(&key).map(|t| t.clone()) {
            (
                BaseBuildingState {
                    position: tile.position,
                    block: tile.block,
                    team: tile.team,
                    health: tile.health,
                    occupied: tile.occupied,
                    inventory: tile.inventory.clone(),
                },
                true,
            )
        } else if let Some(building) = world.base_buildings.get(&key).map(|b| b.clone()) {
            (building, false)
        } else {
            continue;
        };
        let loaded_ammo = building.inventory.iter().find_map(|(item, amount)| {
            (*amount > 0)
                .then(|| turret_ammo(building.block, *item))
                .flatten()
                .map(|ammo| (ammo, *item))
        });
        // Dynamic map turrets consume their actual ItemModule stock. Legacy
        // base-only entries retain the documented default-ammo fallback.
        let Some((ammo, loaded_item)) = loaded_ammo
            .or_else(|| {
                (!loaded_tile)
                    .then(|| default_turret_ammo(building.block))
                    .flatten()
                    .map(|ammo| (ammo, -1))
            })
            .or_else(|| power_turret_weapon(building.block).map(|ammo| (ammo, -1)))
            .or_else(|| liquid_turret_weapon(building.block, -1).map(|ammo| (ammo, -1)))
        else {
            continue;
        };
        let turret_x = (building.position >> 16) as i16 as f32 * 8.0;
        let turret_y = building.position as i16 as f32 * 8.0;
        let target = match controlled_building_weapon_input(
            world,
            key,
            building.team,
            turret_x,
            turret_y,
            ammo.range,
            |enemy| turret_can_target(building.block, enemy.unit_type),
        ) {
            ControlledWeaponInput::Idle => None,
            ControlledWeaponInput::Firing(aim) => Some((aim.target_id, aim.distance, aim.x, aim.y)),
            ControlledWeaponInput::Automatic => world
                .enemies
                .iter()
                .filter_map(|enemy| {
                    if enemy.team == building.team
                        || !turret_can_target(building.block, enemy.unit_type)
                    {
                        return None;
                    }
                    let distance = (enemy.x - turret_x).hypot(enemy.y - turret_y);
                    (distance <= ammo.range).then_some((enemy.id, distance, enemy.x, enemy.y))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1)),
        };
        let Some((target_id, distance, target_x, target_y)) = target else {
            continue;
        };
        let mut progress = *world.base_turret_progress.entry(key).or_insert(0.0).value();
        progress += delta_ticks;
        let mut fired = false;
        if progress >= ammo.reload {
            progress %= ammo.reload;
            fired = true;
        }
        world.base_turret_progress.insert(key, progress);
        if fired {
            if loaded_item >= 0 {
                if let Some(mut tile) = world.tiles.get_mut(&key) {
                    let _ = inventory_remove(
                        &mut tile.inventory,
                        loaded_item,
                        ammo.ammo_per_shot as i32,
                    );
                }
            }
            let shots = turret_shots(building.block);
            for _ in 0..shots {
                spawn_projectile_for_team(
                    world,
                    out,
                    Some(key),
                    target_id,
                    ammo.bullet_id,
                    turret_x,
                    turret_y,
                    target_x,
                    target_y,
                    ammo.damage / shots as f32,
                    ammo.speed,
                    distance,
                    1.0,
                    building.team,
                );
            }
            changed = true;
        }
    }
    changed
}

pub(crate) fn simulate_turrets(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let turret_keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| {
            is_supported_item_turret(tile.block)
                || power_turret_weapon(tile.block).is_some()
                || matches!(tile.block, 353 | 360)
        })
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in turret_keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let liquid_ammo = liquid_turret_weapon(snapshot.block, snapshot.stored_liquid);
        let ammo = turret_ammo(snapshot.block, snapshot.stored_item)
            .or_else(|| power_turret_weapon(snapshot.block))
            .or(liquid_ammo);
        let Some(ammo) = ammo else {
            continue;
        };
        let available_ammo = if liquid_ammo.is_some() {
            snapshot.liquid_amount
        } else {
            snapshot.ammo_units
        };
        if available_ammo < ammo.ammo_per_shot {
            continue;
        }
        let turret_x = (snapshot.position >> 16) as i16 as f32 * 8.0;
        let turret_y = snapshot.position as i16 as f32 * 8.0;
        let target = match controlled_building_weapon_input(
            world,
            key,
            snapshot.team,
            turret_x,
            turret_y,
            ammo.range,
            |enemy| turret_can_target(snapshot.block, enemy.unit_type),
        ) {
            ControlledWeaponInput::Idle => None,
            ControlledWeaponInput::Firing(aim) => Some((aim.target_id, aim.distance, aim.x, aim.y)),
            ControlledWeaponInput::Automatic => world
                .enemies
                .iter()
                .filter_map(|enemy| {
                    if enemy.team == snapshot.team
                        || !turret_can_target(snapshot.block, enemy.unit_type)
                    {
                        return None;
                    }
                    let distance = (enemy.x - turret_x).hypot(enemy.y - turret_y);
                    (distance <= ammo.range).then_some((enemy.id, distance, enemy.x, enemy.y))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1)),
        };
        let Some((target_id, distance, target_x, target_y)) = target else {
            continue;
        };
        if snapshot.block == 366
            && world.projectiles.iter().any(|projectile| {
                projectile.source_position == Some(key)
                    && projectile.damage_interval.is_some()
                    && projectile.remaining_ticks > 0.0
            })
        {
            continue;
        }
        let ready = if let Some(mut turret) = world.tiles.get_mut(&key) {
            let requires_power = power_role(snapshot.block).is_some_and(|role| role.demand > 0.0);
            let efficiency =
                power
                    .get(&key)
                    .copied()
                    .unwrap_or(if requires_power { 0.0 } else { 1.0 });
            turret.production_progress +=
                delta_ticks * building_time_scale(world, key) * efficiency;
            if turret.production_progress >= ammo.reload {
                turret.production_progress %= ammo.reload;
                if liquid_ammo.is_some() {
                    turret.liquid_amount = (turret.liquid_amount - ammo.ammo_per_shot).max(0.0);
                    if turret.liquid_amount <= 0.0001 {
                        turret.liquid_amount = 0.0;
                        turret.stored_liquid = -1;
                    }
                } else if ammo.ammo_per_shot > 0.0 {
                    turret.ammo_units = (turret.ammo_units - ammo.ammo_per_shot).max(0.0);
                    turret.stored_amount = (turret.ammo_units / ammo.multiplier).ceil() as i32;
                    if turret.ammo_units <= 0.0 {
                        turret.stored_item = -1;
                        turret.stored_amount = 0;
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        changed = true;
        if ready {
            let shots = turret_shots(snapshot.block);
            for _ in 0..shots {
                if snapshot.block == 366 {
                    spawn_continuous_projectile_for_team(
                        world,
                        out,
                        key,
                        target_id,
                        ammo.bullet_id,
                        turret_x,
                        turret_y,
                        target_x,
                        target_y,
                        ammo.damage,
                        230.0,
                        5.0,
                        snapshot.team,
                    );
                } else {
                    spawn_projectile_for_team(
                        world,
                        out,
                        Some(key),
                        target_id,
                        ammo.bullet_id,
                        turret_x,
                        turret_y,
                        target_x,
                        target_y,
                        ammo.damage / shots as f32,
                        ammo.speed,
                        distance,
                        1.0,
                        snapshot.team,
                    );
                }
            }
        }
    }
    changed
}
