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
        // Spawn-unit launchers have no terminal splash of their own. Direct
        // carriers spawn at fire; Quell103 delegates to its104 fragment later.
        // Damage belongs to the spawned missile's own death weapon.
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
    if unit.unit_type == 34 {
        collect_navanax_emp_mounts(unit, delta, false);
    }
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
    _target_distance: f32,
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
            unit.secondary_attack_reload += delta_ticks;
            for volley in collect_navanax_emp_mounts(unit, delta_ticks, can_shoot) {
                fire.push(AlliedWeaponFire::Projectile(volley));
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
        let AlliedWeaponFire::Projectile(volley) = shot;
        *volley = scaled_projectile_volley(*volley, damage_multiplier);
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
    let mut fire = Vec::new();
    let multiplier = effective_unit_damage_multiplier(unit);
    let delta = effective_unit_reload_delta(unit, delta_ticks);
    let can_shoot = unit_can_shoot(unit);
    for volley in collect_navanax_emp_mounts(unit, delta, can_shoot) {
        fire.push(AlliedWeaponFire::Projectile(scaled_projectile_volley(
            volley, multiplier,
        )));
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

/// Initialized 160.5 EMP mounts: two independent 130-tick reloads. Preserve
/// sequential half-reload side flips (the old side gates the current shot).
/// Oracle Weapon.update trace: 1/0, 66/1, 132/0, 197/1, 263/0.
pub(crate) fn collect_navanax_emp_mounts(
    unit: &mut EnemyUnit,
    delta: f32,
    shooting: bool,
) -> Vec<EnemyProjectileVolley> {
    let mut shots = Vec::new();
    for index in 0..2 {
        let previous = unit.navanax_emp_reload[index];
        unit.navanax_emp_reload[index] = (previous - delta.max(0.0)).max(0.0);
        let side = unit.navanax_emp_side[index];
        let flipped = index == 1;
        if side == flipped && unit.navanax_emp_reload[index] <= 65.0 && previous > 65.0 {
            unit.navanax_emp_side[0] = !unit.navanax_emp_side[0];
            unit.navanax_emp_side[1] = !unit.navanax_emp_side[1];
        }
        if shooting && side == flipped && unit.navanax_emp_reload[index] <= 0.0001 {
            unit.navanax_emp_reload[index] = 130.0;
            let mut volley = enemy_projectile_volley(34).unwrap();
            volley.mirrored_mounts = 1;
            if flipped {
                volley.mount_offset = -volley.mount_offset;
            }
            shots.push(volley);
        }
    }
    shots
}

/// MissileAI has a forward-flight controller, not GroundAI/CommandAI pursuit.
/// Velocity integration precedes steering; owner loss leaves the last heading.
pub(crate) fn simulate_missile_units(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta: f32,
) -> bool {
    let missiles: Vec<_> = world
        .enemies
        .iter()
        .filter(|u| u.entity_class == 39)
        .map(|u| u.clone())
        .collect();
    let changed = !missiles.is_empty();
    for mut missile in missiles {
        let Some(spec) = crate::game::unit_types::unit_missile_spec(missile.unit_type) else {
            continue;
        };
        missile.missile_time -= delta.max(0.0);
        if missile.missile_time <= 0.0 {
            super::kill_enemy(world, out, missile.id);
            continue;
        }
        let aim = if let Some(owner) = missile.missile_shooter {
            world
                .enemies
                .get(&owner)
                .filter(|u| u.health > 0.0)
                .map(|u| u.clone())
                .and_then(|u| current_unit_weapon_aim(world, &u))
        } else if let Some(position) = missile.missile_source_position {
            let alive = world.tiles.get(&position).is_some_and(|t| {
                t.block != 0
                    && t.health > 0.0
                    && Some(t.generation) == missile.missile_source_generation
            }) || (missile.missile_source_generation == Some(u64::MAX)
                && world
                    .base_buildings
                    .get(&position)
                    .is_some_and(|b| b.health > 0.0));
            alive
                .then(|| world.weapon_aims.get(&(true, position)).map(|a| *a))
                .flatten()
        } else {
            None
        };
        let unit_type = missile.unit_type;
        crate::network::units::integrate_unit_velocity_drag(
            &mut missile,
            unit_type,
            delta,
            world.wave_rules.read().drag_multiplier,
        );
        let elapsed = (spec.lifetime - missile.missile_time).max(0.0);
        if elapsed >= spec.homing_delay {
            if let Some((x, y)) = aim {
                let desired = (y - missile.y).atan2(x - missile.x).to_degrees();
                missile.rotation = crate::network::units::unit_move_toward_angle(
                    missile.rotation,
                    desired,
                    spec.rotate_speed * delta,
                );
            }
        }
        let acceleration = if spec.acceleration_time > 0.0 {
            (elapsed / spec.acceleration_time).min(1.0).powi(2)
        } else {
            1.0
        };
        let speed = effective_unit_speed(&missile) * acceleration;
        let radians = missile.rotation.to_radians();
        crate::network::units::logic_move_at(
            &mut missile,
            radians.cos() * speed,
            radians.sin() * speed,
            delta,
        );
        let position = ((super::world_to_tile(missile.x)) << 16)
            | (super::world_to_tile(missile.y) as u16 as i32);
        let aimed_position = aim
            .map(|(x, y)| (super::world_to_tile(x) << 16) | (super::world_to_tile(y) as u16 as i32))
            .map(|p| dynamic_at(world, p).map(|b| b.position).unwrap_or(p));
        let hittable = |block, team, center| {
            spec.target_ground
                && team != missile.team
                && (!crate::game::content::block_under_bullets(block)
                    || aimed_position == Some(center))
        };
        missile.missile_retarget -= delta;
        if missile.missile_retarget <= 0.0 {
            missile.missile_target =
                missile_proximity_target(world, &missile, spec, aimed_position);
            missile.missile_retarget = 4.0;
        }
        let proximity = missile
            .missile_target
            .and_then(|target| {
                let size = match target {
                    ProjectileHit::Unit(id) => world
                        .enemies
                        .get(&id)
                        .map(|u| crate::game::content::unit_movement(u.unit_type).hit_size),
                    ProjectileHit::Player(id) => world
                        .players
                        .get(&id)
                        .map(|p| (p.team, p.x, p.y))
                        .map(|(team, x, y)| {
                            crate::game::content::unit_movement(
                                crate::network::wire::unit_control::player_core_unit_content_id(
                                    world, team, x, y,
                                ),
                            )
                            .hit_size
                        }),
                    ProjectileHit::Building(p) | ProjectileHit::Core(_, p) => dynamic_at(world, p)
                        .map(|b| f32::from(crate::game::content::block_size(b.block)) * 8.0)
                        .or_else(|| {
                            super::base_building_at(world, p)
                                .map(|b| f32::from(crate::game::content::block_size(b.block)) * 8.0)
                        }),
                }
                .unwrap_or(0.0);
                weapon_target_position(world, target, missile.team).map(|(x, y)| {
                    (x - missile.x).hypot(y - missile.y) <= spec.weapon_range + size / 2.0
                })
            })
            .unwrap_or(false)
            && unit_can_shoot(&missile);
        let contact = dynamic_at(world, position)
            .is_some_and(|b| b.block != 0 && hittable(b.block, b.team, b.position))
            || super::base_building_at(world, position)
                .is_some_and(|b| hittable(b.block, b.team, b.position));
        let id = missile.id;
        world.enemies.insert(id, missile);
        if contact || proximity {
            super::kill_enemy(world, out, id);
        }
    }
    changed
}

pub(crate) fn missile_proximity_target(
    world: &DynamicWorld,
    missile: &EnemyUnit,
    spec: crate::game::unit_types::MissileSpec,
    aimed: Option<i32>,
) -> Option<ProjectileHit> {
    let mut candidates = Vec::new();
    for unit in world.enemies.iter() {
        let movement = crate::game::content::unit_movement(unit.unit_type);
        let air = movement.flying || unit.elevation >= 0.09;
        if unit.team == 0
            || unit.team == missile.team
            || unit.health <= 0.0
            || unit.entity_class == 39
            || (air && !spec.target_air)
            || (!air && !spec.target_ground)
        {
            continue;
        }
        let dx = unit.x - missile.x;
        let dy = unit.y - missile.y;
        // Strict UnitTree rectangle overlap precedes the adjusted circle test.
        let broadphase = spec.target_range + movement.hit_size / 2.0;
        if dx.abs() >= broadphase || dy.abs() >= broadphase {
            continue;
        }
        let distance = dx.powi(2) + dy.powi(2) - movement.hit_size.powi(2);
        if distance < spec.target_range.powi(2) {
            candidates.push((
                crate::game::unit_types::unit_target_priority(unit.unit_type),
                distance,
                ProjectileHit::Unit(unit.id),
            ));
        }
    }
    if spec.target_air {
        for player in world.players.iter() {
            if player.team == 0
                || player.team == missile.team
                || player.dead
                || possessed_unit_id(world, *player.key()).is_some()
            {
                continue;
            }
            let unit_type = crate::network::wire::unit_control::player_core_unit_content_id(
                world,
                player.team,
                player.x,
                player.y,
            );
            let size = crate::game::content::unit_movement(unit_type).hit_size;
            let dx = player.x - missile.x;
            let dy = player.y - missile.y;
            if dx.abs() >= spec.target_range + size / 2.0
                || dy.abs() >= spec.target_range + size / 2.0
            {
                continue;
            }
            let distance = dx.powi(2) + dy.powi(2) - size.powi(2);
            if distance < spec.target_range.powi(2) {
                candidates.push((
                    crate::game::unit_types::unit_target_priority(unit_type),
                    distance,
                    ProjectileHit::Player(*player.key()),
                ));
            }
        }
    }
    if let Some((_, _, target)) = candidates
        .into_iter()
        .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)))
    {
        return Some(target);
    }
    if !spec.target_ground {
        return None;
    }
    let mut buildings = Vec::new();
    let mut candidate = |position, block, team, health| {
        if team == missile.team
            || health <= 0.0
            || block == 0
            || (crate::game::content::block_under_bullets(block) && aimed != Some(position))
        {
            return;
        }
        let x = (position >> 16) as i16 as f32 * 8.0;
        let y = position as i16 as f32 * 8.0;
        let distance = (x - missile.x).hypot(y - missile.y)
            - f32::from(crate::game::content::block_size(block)) * 4.0;
        if distance < spec.target_range {
            buildings.push((
                building_target_priority(block),
                distance,
                ProjectileHit::Building(position),
            ));
        }
    };
    for b in world.tiles.iter() {
        candidate(b.position, b.block, b.team, b.health);
    }
    for b in world.base_buildings.iter() {
        candidate(b.position, b.block, b.team, b.health);
    }
    buildings
        .into_iter()
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)))
        .map(|(_, _, t)| t)
}
