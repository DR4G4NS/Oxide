//! Unit combat: collision, weapon volleys/fire, effective stats.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::placement as building_placement;
use crate::network::combat::projectiles::*;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::network::world::*;
use dashmap::DashMap;

use crate::network::buildings::construction::dynamic_at;
use crate::network::buildings::snapshot::dynamic_tile_health;
use crate::network::combat::enemy::{base_building_at, navigation_index};
use crate::network::units::unit_orders::unit_build_speed;

pub(crate) fn unit_collision_layer(unit: &EnemyUnit) -> u8 {
    let movement = crate::game::content::unit_movement(unit.unit_type);
    if movement.allow_leg_step && movement.leg_physics_layer {
        1
    } else if movement.flying || unit.elevation > 0.01 {
        2
    } else {
        0
    }
}

pub(crate) fn collision_position_passable(
    world: &DynamicWorld,
    unit: &EnemyUnit,
    x: f32,
    y: f32,
) -> bool {
    if unit_collision_layer(unit) == 2 {
        return true;
    }
    let tile_x = (x / 8.0).floor() as i32;
    let tile_y = (y / 8.0).floor() as i32;
    let Some(index) = navigation_index(world, tile_x, tile_y) else {
        return false;
    };
    let position = (tile_x << 16) | (tile_y as u16 as i32);
    let block = dynamic_at(world, position)
        .map(|tile| tile.block)
        .or_else(|| base_building_at(world, position).map(|building| building.block))
        .unwrap_or(world.base_blocks[index]);
    !crate::game::content::block_navigation(block).solid
}

/// Navigation fields only depend on solid building occupancy/health. Belts,
/// routers, power blocks and other non-solid machines must not invalidate a
/// full-map Dijkstra field every time a large plan finishes.
pub(crate) fn invalidate_navigation_for_block(world: &DynamicWorld, block: i16) {
    if crate::game::content::block_navigation(block).solid {
        world.navigation_revision.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AlliedWeaponFire {
    Projectile(EnemyProjectileVolley),
    NavanaxLasers(f32),
}

pub(crate) fn generic_unit_weapon_volley(
    unit: &EnemyUnit,
    weapon: crate::game::content::UnitWeapon,
) -> EnemyProjectileVolley {
    // Keep the richer hand-audited definitions (homing, inaccuracy and
    // special pierce caps) whenever this is the unit's primary mount. The
    // generated TSV supplies every remaining side mount and every Erekir
    // weapon, which used to fall through to invisible instant damage.
    if let Some(primary) = enemy_projectile_volley(unit.unit_type) {
        if primary.bullet_id == weapon.bullet_id {
            return primary;
        }
    }

    let (direct_damage, splash_damage, splash_radius) = match weapon.bullet_id {
        // SpawnUnitBulletType launchers are represented by their spawned
        // missile's terminal payload in this headless authoritative model.
        92 => (0.75, 140.0, 25.0),  // anthicus -> anthicus-missile
        103 => (0.75, 110.0, 25.0), // quell -> quell-missile
        106 => (0.75, 140.0, 25.0), // disrupt -> disrupt-missile
        _ => (
            weapon.damage,
            weapon.splash_damage,
            weapon.splash_radius.max(0.0),
        ),
    };
    // A speed-zero entry is a beam/rail/bomb whose range is encoded in its
    // BulletType rather than in this export. Giving the server-side flight a
    // derived speed preserves the official lifetime and reaches exactly the
    // unit's authoritative attack range; CreateBullet still uses the
    // client's registered BulletType and velocityScale=1.
    let speed = if weapon.speed <= 0.0 {
        unit.attack_range / weapon.lifetime.max(1.0)
    } else {
        weapon.speed
    };
    EnemyProjectileVolley {
        bullet_id: weapon.bullet_id,
        shots: weapon.shots,
        direct_damage,
        splash_damage,
        splash_radius,
        speed,
        lifetime: weapon.lifetime.max(1.0),
        inaccuracy: 0.0,
        velocity_random: 0.0,
        homing_range: 0.0,
        status_effect: weapon.status_effect,
        status_duration: weapon.status_duration,
        pierce_units: if weapon.pierce_units { u8::MAX } else { 0 },
        pierce_buildings: if weapon.pierce_buildings { u8::MAX } else { 0 },
    }
}

pub(crate) fn unit_weapon_timer(unit: &EnemyUnit, index: usize) -> f32 {
    match index {
        0 => unit.attack_reload,
        1 => unit.secondary_attack_reload,
        2 => unit.tertiary_attack_reload,
        _ => unit.quaternary_attack_reload,
    }
}

pub(crate) fn set_unit_weapon_timer(unit: &mut EnemyUnit, index: usize, value: f32) {
    match index {
        0 => unit.attack_reload = value,
        1 => unit.secondary_attack_reload = value,
        2 => unit.tertiary_attack_reload = value,
        _ => unit.quaternary_attack_reload = value,
    }
}

pub(crate) fn drain_weapon_timer(
    timer: &mut f32,
    reload: f32,
    fire: AlliedWeaponFire,
    out: &mut Vec<AlliedWeaponFire>,
) {
    while *timer >= reload {
        *timer -= reload;
        out.push(fire);
    }
}

pub(crate) fn collect_allied_weapon_fire(
    unit: &mut EnemyUnit,
    delta_ticks: f32,
    target_distance: f32,
) -> Option<Vec<AlliedWeaponFire>> {
    let damage_multiplier = effective_unit_damage_multiplier(unit);
    let can_shoot = unit_can_shoot(unit);
    let delta_ticks = effective_unit_reload_delta(unit, delta_ticks);
    let mut fire = Vec::new();
    match unit.unit_type {
        18 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            unit.tertiary_attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                20.0,
                AlliedWeaponFire::Projectile(ANTUMBRA_MISSILE),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.secondary_attack_reload,
                35.0,
                AlliedWeaponFire::Projectile(ANTUMBRA_MISSILE),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.tertiary_attack_reload,
                12.0,
                AlliedWeaponFire::Projectile(ANTUMBRA_CANNON),
                &mut fire,
            );
        }
        3 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            unit.tertiary_attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                45.0,
                AlliedWeaponFire::Projectile(SCEPTER_BOLT),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.secondary_attack_reload,
                12.0,
                AlliedWeaponFire::Projectile(SCEPTER_MOUNT),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.tertiary_attack_reload,
                15.0,
                AlliedWeaponFire::Projectile(SCEPTER_MOUNT),
                &mut fire,
            );
        }
        13 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            unit.tertiary_attack_reload += delta_ticks;
            unit.quaternary_attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                45.0,
                AlliedWeaponFire::Projectile(ARKYID_ARTILLERY),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.secondary_attack_reload,
                9.0,
                AlliedWeaponFire::Projectile(ARKYID_SAP),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.tertiary_attack_reload,
                14.0,
                AlliedWeaponFire::Projectile(ARKYID_SAP),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.quaternary_attack_reload,
                22.0,
                AlliedWeaponFire::Projectile(ARKYID_SAP),
                &mut fire,
            );
        }
        19 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            unit.tertiary_attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                45.0,
                AlliedWeaponFire::Projectile(ECLIPSE_LASER),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.secondary_attack_reload,
                9.0,
                AlliedWeaponFire::Projectile(ECLIPSE_FLAK),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.tertiary_attack_reload,
                12.0,
                AlliedWeaponFire::Projectile(ECLIPSE_FLAK),
                &mut fire,
            );
        }
        12 | 14 | 22 | 25..=28 => {
            let ((primary_reload, primary), (secondary_reload, secondary)) =
                naval_weapon_volleys(unit.unit_type).unwrap();
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                primary_reload,
                AlliedWeaponFire::Projectile(primary),
                &mut fire,
            );
            drain_weapon_timer(
                &mut unit.secondary_attack_reload,
                secondary_reload,
                AlliedWeaponFire::Projectile(secondary),
                &mut fire,
            );
        }
        8 => {
            unit.attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                155.0,
                AlliedWeaponFire::Projectile(VELA_BEAM),
                &mut fire,
            );
        }
        30 => {
            unit.attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                22.0,
                AlliedWeaponFire::Projectile(RETUSA_BOLT),
                &mut fire,
            );
            let previous = unit.tertiary_attack_reload.max(0.0);
            let current = previous + delta_ticks;
            unit.tertiary_attack_reload = current;
            unit.secondary_attack_reload = current % 90.0;
            for _ in 0..retusa_mine_shots_between(previous, current) {
                fire.push(AlliedWeaponFire::Projectile(RETUSA_MINE));
            }
        }
        34 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            drain_weapon_timer(
                &mut unit.attack_reload,
                65.0,
                AlliedWeaponFire::Projectile(enemy_projectile_volley(34).unwrap()),
                &mut fire,
            );
            if target_distance <= 90.0 {
                drain_weapon_timer(
                    &mut unit.secondary_attack_reload,
                    170.0,
                    AlliedWeaponFire::NavanaxLasers(damage_multiplier),
                    &mut fire,
                );
            }
        }
        _ => {
            let weapons = crate::game::content::unit_weapons(unit.unit_type);
            if weapons.is_empty() {
                return None;
            }
            // The official controllable set has at most four independent
            // reload groups. Navanax's four synchronized plasma lasers are
            // handled above as one group.
            debug_assert!(weapons.len() <= 4);
            for (index, weapon) in weapons.iter().copied().take(4).enumerate() {
                let reload = weapon.reload.max(0.0001);
                let mut timer = unit_weapon_timer(unit, index) + delta_ticks.max(0.0);
                let volley = generic_unit_weapon_volley(unit, weapon);
                while timer >= reload {
                    timer -= reload;
                    fire.push(AlliedWeaponFire::Projectile(volley));
                }
                set_unit_weapon_timer(unit, index, timer);
            }
        }
    }
    for shot in &mut fire {
        if let AlliedWeaponFire::Projectile(volley) = shot {
            *volley = scaled_projectile_volley(*volley, damage_multiplier);
        }
    }
    if !can_shoot {
        fire.clear();
    }
    Some(fire)
}

/// Player-triggered subset of a unit's mounts. Autonomous repair beams,
/// point-defense and Navanax's four auto-target plasma lasers keep running in
/// their dedicated simulations and must not be redirected by the mouse.
pub(crate) fn collect_manual_weapon_fire(
    unit: &mut EnemyUnit,
    delta_ticks: f32,
    target_distance: f32,
) -> Option<Vec<AlliedWeaponFire>> {
    if unit.unit_type != 34 {
        return collect_allied_weapon_fire(unit, delta_ticks, target_distance);
    }
    let volley = scaled_projectile_volley(
        enemy_projectile_volley(34).unwrap(),
        effective_unit_damage_multiplier(unit),
    );
    let can_shoot = unit_can_shoot(unit);
    unit.attack_reload += effective_unit_reload_delta(unit, delta_ticks);
    let mut fire = Vec::new();
    drain_weapon_timer(
        &mut unit.attack_reload,
        65.0,
        AlliedWeaponFire::Projectile(volley),
        &mut fire,
    );
    if !can_shoot {
        fire.clear();
    }
    Some(fire)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_allied_weapon_fire(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    fires: &[AlliedWeaponFire],
    shooter_id: i32,
    target_id: i32,
    target_position: Option<i32>,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
) {
    spawn_weapon_fire_for_team(
        world,
        out,
        fires,
        shooter_id,
        target_id,
        target_position,
        source_x,
        source_y,
        target_x,
        target_y,
        1,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_weapon_fire_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    fires: &[AlliedWeaponFire],
    shooter_id: i32,
    target_id: i32,
    target_position: Option<i32>,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    team: u8,
) {
    for fire in fires {
        match fire {
            AlliedWeaponFire::Projectile(volley) => {
                for volley_shot in 0..volley.shots {
                    spawn_unit_projectile_for_team(
                        world,
                        out,
                        shooter_id,
                        target_id,
                        target_position,
                        *volley,
                        source_x,
                        source_y,
                        target_x,
                        target_y,
                        volley_shot,
                        team,
                    );
                }
            }
            AlliedWeaponFire::NavanaxLasers(multiplier) => spawn_navanax_lasers(
                world,
                out,
                team,
                *multiplier,
                shooter_id,
                target_id,
                target_position,
                false,
                source_x,
                source_y,
                target_x,
                target_y,
            ),
        }
    }
}

pub(crate) fn damaged_allied_building_target(
    world: &DynamicWorld,
    team: u8,
    x: f32,
    y: f32,
    range: f32,
) -> Option<(i32, f32, f32)> {
    let mut seen = HashSet::new();
    let dynamic = world
        .tiles
        .iter()
        .filter_map(|tile| {
            if tile.block == 0
                || tile.team != team
                || building_heal_suppressed(world, tile.position, tile.block)
                || !seen.insert(tile.position)
            {
                return None;
            }
            let maximum = crate::game::content::block_health(tile.block);
            let health = dynamic_tile_health(&tile);
            let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let target_y = tile.position as i16 as f32 * 8.0;
            let distance = (target_x - x).hypot(target_y - y);
            (health < maximum && distance <= range).then_some((
                distance,
                tile.position,
                target_x,
                target_y,
            ))
        })
        .collect::<Vec<_>>();
    let base = world
        .base_buildings
        .iter()
        .filter_map(|building| {
            if building.team != team
                || building_heal_suppressed(world, building.position, building.block)
                || !seen.insert(building.position)
            {
                return None;
            }
            let maximum = crate::game::content::block_health(building.block);
            let target_x = (building.position >> 16) as i16 as f32 * 8.0;
            let target_y = building.position as i16 as f32 * 8.0;
            let distance = (target_x - x).hypot(target_y - y);
            (building.health < maximum && distance <= range).then_some((
                distance,
                building.position,
                target_x,
                target_y,
            ))
        })
        .collect::<Vec<_>>();
    dynamic
        .into_iter()
        .chain(base)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, position, target_x, target_y)| (position, target_x, target_y))
}

pub(crate) fn boost_properties(unit_type: i16) -> Option<(f32, f32, f32)> {
    match unit_type {
        5 => Some((1.5, 0.08, 0.08)),
        6 => Some((1.6, 0.07, 0.07)),
        7 => Some((2.0, 0.05, 0.05)),
        8 => Some((2.4, 0.02, 0.02)),
        _ => None,
    }
}

pub(crate) fn effective_unit_damage_multiplier(unit: &EnemyUnit) -> f32 {
    crate::network::units::StatusContainer::status_aggregate(unit).damage
}

pub(crate) fn scaled_projectile_volley(
    mut volley: EnemyProjectileVolley,
    multiplier: f32,
) -> EnemyProjectileVolley {
    volley.direct_damage *= multiplier;
    volley.splash_damage *= multiplier;
    volley
}

pub(crate) fn effective_unit_speed(unit: &EnemyUnit) -> f32 {
    let boost = boost_properties(unit.unit_type)
        .map(|(boost, _, _)| 1.0 + (boost - 1.0) * unit.elevation.clamp(0.0, 1.0))
        .unwrap_or(1.0);
    let status = crate::network::units::StatusContainer::status_aggregate(unit).speed;
    unit.move_speed * boost * status
}

pub(crate) fn effective_unit_reload_delta(unit: &EnemyUnit, delta_ticks: f32) -> f32 {
    let agg = crate::network::units::StatusContainer::status_aggregate(unit);
    delta_ticks.max(0.0) * agg.reload
}

/// Official `UnitComp.canShoot` status half: `!disarmed`.
pub(crate) fn unit_can_shoot(unit: &EnemyUnit) -> bool {
    !crate::network::units::StatusContainer::status_aggregate(unit).disarmed
}

/// Official `BuilderComp` `type.buildSpeed * buildSpeedMultiplier`.
pub(crate) fn effective_unit_build_speed(unit: &EnemyUnit) -> Option<f32> {
    let base = unit_build_speed(unit.unit_type)?;
    Some(base * crate::network::units::StatusContainer::status_aggregate(unit).build_speed)
}

pub(crate) fn unit_hit_size(unit_type: i16) -> f32 {
    const SIZES: [f32; 35] = [
        8.0, 10.0, 13.0, 22.0, 30.0, 8.0, 11.0, 13.0, 24.0, 29.0, 8.0, 13.0, 15.0, 23.0, 26.0, 9.0,
        11.0, 20.0, 46.0, 58.0, 6.0, 9.0, 16.05, 36.0, 66.0, 10.0, 13.0, 20.0, 39.0, 58.0, 11.0,
        14.0, 20.0, 44.0, 58.0,
    ];
    usize::try_from(unit_type)
        .ok()
        .and_then(|index| SIZES.get(index))
        .copied()
        .unwrap_or(8.0)
}
