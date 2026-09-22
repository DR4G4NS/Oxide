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
    let tile_x = crate::network::combat::enemy::world_to_tile(x);
    let tile_y = crate::network::combat::enemy::world_to_tile(y);
    let Some(index) = navigation_index(world, tile_x, tile_y) else {
        return false;
    };
    let position = (tile_x << 16) | (tile_y as u16 as i32);
    let floor_id = crate::network::combat::floor_at_tile(world, tile_x, tile_y);
    let floor = crate::game::content::block_navigation(floor_id);
    let movement = crate::game::content::unit_movement(unit.unit_type);
    if movement.naval {
        if !crate::game::content::floor_is_liquid(floor_id) {
            return false;
        }
    } else if floor.solid {
        // Natural rock / solid floors (ASTRA F05 / C04).
        return false;
    }
    if let Some(tile) = dynamic_at(world, position) {
        return !crate::game::content::building_check_solid(tile.block, tile.door_open);
    }
    let block = base_building_at(world, position)
        .map(|building| building.block)
        .unwrap_or(world.base_blocks[index]);
    !crate::game::content::building_check_solid(block, false)
}

/// Navigation fields only depend on solid building occupancy/health. Belts,
/// routers, power blocks and other non-solid machines must not invalidate a
/// full-map Dijkstra field every time a large plan finishes.
pub(crate) fn invalidate_navigation_for_block(world: &DynamicWorld, block: i16) {
    if crate::game::content::block_navigation(block).solid || matches!(block, 228 | 229 | 239) {
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
        // SpawnUnitBulletType launchers (BulletType.create JAR 158.1): these
        // bullets never become bullet entities and never reach hit(), so they
        // carry NO terminal splash of their own. The spawnUnit MissileUnitType
        // is inserted at impact (simulate_projectiles::spawn_projectile_unit)
        // and the payload damage lives in the spawned missile's own weapon.
        92 | 103 | 106 => (weapon.damage, 0.0, 0.0),
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
        mirrored_mounts: if weapon.mirror { 2 } else { 1 },
        mount_offset: weapon.mount_x,
        direct_damage,
        splash_damage,
        splash_radius,
        speed,
        lifetime: weapon.lifetime.max(1.0),
        inaccuracy: 0.0,
        velocity_random: 0.0,
        homing_range: 0.0,
        homing_power: 0.0,
        homing_delay: -1.0,

        collides_air: true,
        collides_ground: true,
        heals: false,
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

/// Vanilla WeaponsComp counts down and stays at 0 (ready). Counting up, that
/// is a cap at `reload`. Extra idle time must not dump as simultaneous shots
/// (ASTRA E06).
pub(crate) fn accumulate_weapon_timer(timer: f32, delta: f32, reload: f32) -> f32 {
    let reload = reload.max(0.0001);
    (timer + delta.max(0.0)).min(reload)
}

pub(crate) fn take_weapon_shot(timer: &mut f32, reload: f32) -> bool {
    let reload = reload.max(0.0001);
    if *timer >= reload {
        *timer = 0.0;
        true
    } else {
        false
    }
}

/// Advance every mount's reload without firing, capped at each weapon's period.
pub(crate) fn accumulate_unit_weapon_timers(unit: &mut EnemyUnit, delta_ticks: f32) {
    accumulate_idle_weapon_timers(unit, delta_ticks);
}

pub(crate) fn accumulate_wave_weapon_timers(unit: &mut EnemyUnit, delta_ticks: f32) {
    accumulate_idle_weapon_timers(unit, delta_ticks);
}

fn accumulate_idle_weapon_timers(unit: &mut EnemyUnit, delta_ticks: f32) {
    let delta = effective_unit_reload_delta(unit, delta_ticks);
    let weapons = crate::game::content::unit_weapons(unit.unit_type);
    if weapons.is_empty() {
        unit.attack_reload =
            accumulate_weapon_timer(unit.attack_reload, delta, unit.attack_reload_time);
        return;
    }
    for (index, weapon) in weapons.iter().copied().take(4).enumerate() {
        let reload = (weapon.reload * if weapon.mirror { 2.0 } else { 1.0 }).max(0.0001);
        let timer = accumulate_weapon_timer(unit_weapon_timer(unit, index), delta, reload);
        set_unit_weapon_timer(unit, index, timer);
    }
}

pub(crate) fn drain_weapon_timer(
    timer: &mut f32,
    reload: f32,
    fire: AlliedWeaponFire,
    out: &mut Vec<AlliedWeaponFire>,
) {
    drain_weapon_timer_n(timer, reload, fire, out, usize::MAX);
}

fn drain_weapon_timer_n(
    timer: &mut f32,
    reload: f32,
    fire: AlliedWeaponFire,
    out: &mut Vec<AlliedWeaponFire>,
    max_bursts: usize,
) {
    let reload = reload.max(0.0001);
    if max_bursts == 1 {
        // Runtime updates fire at most once and discard overshoot, as a
        // countdown reset to reload would. Idle credit is never a backlog.
        *timer = timer.min(reload);
    }
    let mut bursts = 0;
    while *timer >= reload && bursts < max_bursts {
        *timer -= reload;
        out.push(fire);
        bursts += 1;
    }
}

pub(crate) fn collect_allied_weapon_fire(
    unit: &mut EnemyUnit,
    delta_ticks: f32,
    target_distance: f32,
) -> Option<Vec<AlliedWeaponFire>> {
    collect_allied_weapon_fire_n(unit, delta_ticks, target_distance, usize::MAX)
}

/// One burst per mount: the 60 Hz server tick. Large `delta_ticks` must not
/// dump idle overcharge as simultaneous volleys (ASTRA E06 / factory fire).
pub(crate) fn collect_allied_weapon_fire_tick(
    unit: &mut EnemyUnit,
    delta_ticks: f32,
    target_distance: f32,
) -> Option<Vec<AlliedWeaponFire>> {
    collect_allied_weapon_fire_n(unit, delta_ticks, target_distance, 1)
}

fn collect_allied_weapon_fire_n(
    unit: &mut EnemyUnit,
    delta_ticks: f32,
    target_distance: f32,
    max_bursts: usize,
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
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                40.0,
                AlliedWeaponFire::Projectile(ANTUMBRA_MISSILE),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.secondary_attack_reload,
                70.0,
                AlliedWeaponFire::Projectile(ANTUMBRA_MISSILE),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.tertiary_attack_reload,
                24.0,
                AlliedWeaponFire::Projectile(ANTUMBRA_CANNON),
                &mut fire,
                max_bursts,
            );
        }
        3 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            unit.tertiary_attack_reload += delta_ticks;
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                90.0,
                AlliedWeaponFire::Projectile(SCEPTER_BOLT),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.secondary_attack_reload,
                24.0,
                AlliedWeaponFire::Projectile(SCEPTER_MOUNT),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.tertiary_attack_reload,
                30.0,
                AlliedWeaponFire::Projectile(SCEPTER_MOUNT),
                &mut fire,
                max_bursts,
            );
        }
        13 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            unit.tertiary_attack_reload += delta_ticks;
            unit.quaternary_attack_reload += delta_ticks;
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                90.0,
                AlliedWeaponFire::Projectile(ARKYID_ARTILLERY),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.secondary_attack_reload,
                18.0,
                AlliedWeaponFire::Projectile(volley_with_mount_offset(ARKYID_SAP, 4.0)),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.tertiary_attack_reload,
                28.0,
                AlliedWeaponFire::Projectile(volley_with_mount_offset(ARKYID_SAP, 9.0)),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.quaternary_attack_reload,
                44.0,
                AlliedWeaponFire::Projectile(volley_with_mount_offset(ARKYID_SAP, 14.0)),
                &mut fire,
                max_bursts,
            );
        }
        19 => {
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            unit.tertiary_attack_reload += delta_ticks;
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                90.0,
                AlliedWeaponFire::Projectile(ECLIPSE_LASER),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.secondary_attack_reload,
                18.0,
                AlliedWeaponFire::Projectile(volley_with_mount_offset(ECLIPSE_FLAK, 11.0)),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.tertiary_attack_reload,
                24.0,
                AlliedWeaponFire::Projectile(volley_with_mount_offset(ECLIPSE_FLAK, 20.0)),
                &mut fire,
                max_bursts,
            );
        }
        12 | 14 | 22 | 25..=28 => {
            let ((primary_reload, primary), (secondary_reload, secondary)) =
                naval_weapon_volleys(unit.unit_type).unwrap();
            unit.attack_reload += delta_ticks;
            unit.secondary_attack_reload += delta_ticks;
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                primary_reload,
                AlliedWeaponFire::Projectile(primary),
                &mut fire,
                max_bursts,
            );
            drain_weapon_timer_n(
                &mut unit.secondary_attack_reload,
                secondary_reload,
                AlliedWeaponFire::Projectile(secondary),
                &mut fire,
                max_bursts,
            );
        }
        8 => {
            unit.attack_reload += delta_ticks;
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                155.0,
                AlliedWeaponFire::Projectile(VELA_BEAM),
                &mut fire,
                max_bursts,
            );
        }
        30 => {
            unit.attack_reload += delta_ticks;
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                44.0,
                AlliedWeaponFire::Projectile(RETUSA_BOLT),
                &mut fire,
                max_bursts,
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
            drain_weapon_timer_n(
                &mut unit.attack_reload,
                130.0,
                AlliedWeaponFire::Projectile(enemy_projectile_volley(34).unwrap()),
                &mut fire,
                max_bursts,
            );
            if target_distance <= 90.0 {
                drain_weapon_timer_n(
                    &mut unit.secondary_attack_reload,
                    170.0,
                    AlliedWeaponFire::NavanaxLasers(damage_multiplier),
                    &mut fire,
                    max_bursts,
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
                let volley = generic_unit_weapon_volley(unit, weapon);
                // The TSV is pre-init. UnitType.init doubles each mirrored
                // mount's reload; our volley represents the pair together.
                let reload = (weapon.reload * if weapon.mirror { 2.0 } else { 1.0 }).max(0.0001);
                let mut timer = unit_weapon_timer(unit, index) + delta_ticks.max(0.0);
                if max_bursts == 1 {
                    timer = timer.min(reload);
                }
                let mut bursts = 0;
                while timer >= reload && bursts < max_bursts {
                    timer -= reload;
                    fire.push(AlliedWeaponFire::Projectile(volley));
                    bursts += 1;
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
        return collect_allied_weapon_fire_tick(unit, delta_ticks, target_distance);
    }
    let volley = scaled_projectile_volley(
        enemy_projectile_volley(34).unwrap(),
        effective_unit_damage_multiplier(unit),
    );
    let can_shoot = unit_can_shoot(unit);
    unit.attack_reload += effective_unit_reload_delta(unit, delta_ticks);
    let mut fire = Vec::new();
    drain_weapon_timer_n(
        &mut unit.attack_reload,
        65.0,
        AlliedWeaponFire::Projectile(volley),
        &mut fire,
        1,
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
                // Vanilla Weapon.mirror: every mount (the flipped copy has its
                // local muzzle x negated) fires the full shot pattern.
                for mount in 0..crate::network::combat::volley_mount_count(*volley) {
                    let lateral = crate::network::combat::volley_mount_lateral(*volley, mount);
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
                            lateral,
                            volley_shot,
                            team,
                        );
                    }
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct SupportWeaponTarget {
    pub unit_id: i32,
    pub building_position: Option<i32>,
    pub x: f32,
    pub y: f32,
}

/// Shared by support-unit simulation and weapon snapshots. RepairAI prefers
/// a damaged allied building; without one, its weapons can target enemies.
pub(crate) fn support_weapon_target(
    world: &DynamicWorld,
    unit: &EnemyUnit,
) -> Option<SupportWeaponTarget> {
    let command = world
        .unit_orders
        .get(&unit.id)
        .map(|order| order.command)
        .unwrap_or_else(|| default_unit_command(unit.unit_type));
    if command == 1 {
        if let Some((position, x, y)) =
            damaged_allied_building_target(world, unit.team, unit.x, unit.y, f32::INFINITY)
        {
            // A distant repair target must not make the unit shoot at a
            // different object while it is approaching that building.
            return ((x - unit.x).hypot(y - unit.y) <= unit.attack_range).then_some(
                SupportWeaponTarget {
                    unit_id: -1,
                    building_position: Some(position),
                    x,
                    y,
                },
            );
        }
    }
    if let Some((position, x, y)) =
        crate::network::units::unit_orders::ordered_opposing_building(world, unit)
    {
        return ((x - unit.x).hypot(y - unit.y) <= unit.attack_range).then_some(
            SupportWeaponTarget {
                unit_id: -1,
                building_position: Some(position),
                x,
                y,
            },
        );
    }
    crate::network::wire::nearest_opposing_unit(world, unit.team, unit.x, unit.y)
        .filter(|(_, x, y)| (*x - unit.x).hypot(*y - unit.y) <= unit.attack_range)
        .map(|(unit_id, x, y)| SupportWeaponTarget {
            unit_id,
            building_position: None,
            x,
            y,
        })
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
                || crate::network::buildings::construction::is_construct_block(tile.block)
                || tile.team != team
                || building_heal_suppressed(world, tile.position, tile.block)
                || !seen.insert(tile.position)
            {
                return None;
            }
            let maximum = crate::game::content::block_health(tile.block);
            let health = dynamic_tile_health(&tile);
            let offset = if crate::game::content::block_size(tile.block).is_multiple_of(2) {
                4.0
            } else {
                0.0
            };
            let target_x = (tile.position >> 16) as i16 as f32 * 8.0 + offset;
            let target_y = tile.position as i16 as f32 * 8.0 + offset;
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
            let offset = if crate::game::content::block_size(building.block).is_multiple_of(2) {
                4.0
            } else {
                0.0
            };
            let target_x = (building.position >> 16) as i16 as f32 * 8.0 + offset;
            let target_y = building.position as i16 as f32 * 8.0 + offset;
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
        .min_by(|left, right| left.0.total_cmp(&right.0).then(left.1.cmp(&right.1)))
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
    effective_unit_speed_on_floor(unit, None)
}

/// World-aware variant applying the official `Floor.speedMultiplier` under
/// the unit's tile (audit H13/H4). Flying units ignore terrain; grounded
/// units are slowed by liquid/mud/ice floors per the JAR-probed table.
pub(crate) fn effective_unit_speed_on_floor(unit: &EnemyUnit, floor: Option<i16>) -> f32 {
    let boost = boost_properties(unit.unit_type)
        .map(|(boost, _, _)| 1.0 + (boost - 1.0) * unit.elevation.clamp(0.0, 1.0))
        .unwrap_or(1.0);
    let status = crate::network::units::StatusContainer::status_aggregate(unit).speed;
    let floor_multiplier =
        if unit.elevation >= 0.09 || crate::game::content::unit_movement(unit.unit_type).flying {
            1.0
        } else {
            floor
                .map(crate::game::content::floor_speed_multiplier)
                .unwrap_or(1.0)
        };
    unit.move_speed * boost * status * floor_multiplier
}

pub(crate) fn effective_unit_reload_delta(unit: &EnemyUnit, delta_ticks: f32) -> f32 {
    let agg = crate::network::units::StatusContainer::status_aggregate(unit);
    delta_ticks.max(0.0) * agg.reload
}

/// 159.7 UnitEntity.canShoot: disarmed and airborne boost units cannot fire.
pub(crate) fn unit_can_shoot(unit: &EnemyUnit) -> bool {
    !crate::network::units::StatusContainer::status_aggregate(unit).disarmed
        && !(boost_properties(unit.unit_type).is_some() && unit.elevation >= 0.09)
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
