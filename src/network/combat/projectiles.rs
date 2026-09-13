//! Projectile spawn/volley/fragments and simulate_projectiles.

use crate::network::buildings::construction::effective_building_team;
use crate::network::combat::enemy::{
    apply_enemy_support_abilities, damage_building, enemy_max_health,
};
use crate::network::combat::unit_combat::collect_allied_weapon_fire;
use crate::network::simulation::{simulate_enemy_point_defense, simulate_enemy_statuses};
use crate::network::units::mining::heal_building_for_team;
use crate::network::wire::encode::{
    encode_build_destroyed_frame, encode_build_health_update_frame, frame_generated_packet,
};
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_projectile(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_position: Option<i32>,
    target_id: i32,
    bullet_id: i16,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    speed: f32,
    distance: f32,
    lifetime_scale: f32,
) -> i32 {
    spawn_projectile_for_team(
        world,
        out,
        source_position,
        target_id,
        bullet_id,
        source_x,
        source_y,
        target_x,
        target_y,
        damage,
        speed,
        distance,
        lifetime_scale,
        1,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_projectile_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_position: Option<i32>,
    target_id: i32,
    bullet_id: i16,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    speed: f32,
    distance: f32,
    lifetime_scale: f32,
    team: u8,
) -> i32 {
    let angle = (target_y - source_y)
        .atan2(target_x - source_x)
        .to_degrees();
    let total_ticks = if speed <= 0.0 { 0.0 } else { distance / speed };
    let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
    // C2: uninventoried ids must not silently degrade to plain direct damage.
    // ASTRA R03: freeze Rules.unitDamage / blockDamage at fire, not impact.
    let damage = crate::game::bullet_catalog::inventoried_damage(bullet_id, damage)
        * fire_damage_rule(world, team, source_position.is_some());
    world.projectiles.insert(
        projectile_id,
        Projectile {
            target_id,
            shooter_id: -1,
            team,
            bullet_id,
            damage,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            homing_power: 0.0,
            homing_delay: -1.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: false,
            armor_multiplier: projectile_armor_multiplier(bullet_id),
            remaining_ticks: total_ticks,
            total_ticks,
            source_x,
            source_y,
            target_x,
            target_y,
            lifetime_scale,
            source_position,
            damage_interval: None,
            damage_timer: 0.0,
            collided: Vec::new(),
        },
    );
    if let Ok(payload) = encode_create_bullet_payload(
        bullet_id,
        team,
        source_x,
        source_y,
        angle,
        damage,
        1.0,
        lifetime_scale,
    ) {
        if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
    projectile_id
}

fn fire_damage_rule(world: &DynamicWorld, team: u8, from_building: bool) -> f32 {
    let rules = world.wave_rules.read();
    if from_building {
        rules.block_damage_multiplier * rules.team_rule(team).block_damage_multiplier
    } else {
        rules.unit_damage_multiplier * rules.team_rule(team).unit_damage_multiplier
    }
}

fn inventoried_hit(bullet_id: i16, damage: f32, splash: f32) -> (f32, f32) {
    if crate::game::bullet_catalog::bullet_is_inventoried(bullet_id) {
        (damage, splash)
    } else {
        (0.0, 0.0)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_enemy_horizon_bomb(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_id: i32,
    damage_multiplier: f32,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
) -> i32 {
    const BULLET_ID: i16 = 31;
    const SPEED: f32 = 0.7;
    const LIFETIME: f32 = 30.0;
    const DIRECT_DAMAGE: f32 = 13.5;
    const SPLASH_DAMAGE: f32 = 27.0;
    const SPLASH_RADIUS: f32 = 25.0;

    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let distance = dx.hypot(dy);
    let travel = (SPEED * LIFETIME).min(distance);
    let (impact_x, impact_y) = if distance > 0.001 {
        (
            source_x + dx / distance * travel,
            source_y + dy / distance * travel,
        )
    } else {
        (source_x, source_y)
    };
    let angle = dy.atan2(dx).to_degrees();
    let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
    world.projectiles.insert(
        projectile_id,
        Projectile {
            target_id: source_id,
            shooter_id: source_id,
            team: 2,
            bullet_id: BULLET_ID,
            damage: DIRECT_DAMAGE * damage_multiplier,
            splash_damage: SPLASH_DAMAGE * damage_multiplier,
            splash_radius: SPLASH_RADIUS,
            status_effect: 18,
            status_duration: 60.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            homing_power: 0.0,
            homing_delay: -1.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: false,
            armor_multiplier: 1.0,
            remaining_ticks: LIFETIME,
            total_ticks: LIFETIME,
            source_x,
            source_y,
            target_x: impact_x,
            target_y: impact_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
            collided: Vec::new(),
        },
    );
    if let Ok(payload) = encode_create_bullet_payload(
        BULLET_ID,
        2,
        source_x,
        source_y,
        angle,
        DIRECT_DAMAGE * damage_multiplier,
        1.0,
        1.0,
    ) {
        if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
    projectile_id
}

#[derive(Clone, Copy)]
pub(crate) struct EnemyProjectileVolley {
    pub(crate) bullet_id: i16,
    pub(crate) shots: u8,
    pub(crate) direct_damage: f32,
    pub(crate) splash_damage: f32,
    pub(crate) splash_radius: f32,
    pub(crate) speed: f32,
    pub(crate) lifetime: f32,
    pub(crate) inaccuracy: f32,
    pub(crate) velocity_random: f32,
    pub(crate) homing_range: f32,
    /// Vanilla turn-rate cap = homingPower * 50 degrees per tick
    /// (BulletType.updateHoming: Angles.moveToward(rotation, angleTo, ...)).
    pub(crate) homing_power: f32,
    /// Vanilla BulletType.homingDelay (JAR constructor default -1f; no
    /// content class overrides it): updateHoming does nothing before
    /// bullet.time >= homingDelay.
    pub(crate) homing_delay: f32,
    /// Vanilla Weapon.mirror (default TRUE): UnitType.init appends a flipped
    /// copy of every mirrored weapon (local muzzle x negated) and both mounts
    /// fire the full shot pattern on their own reload.
    /// 1 = single mount, 2 = mirrored pair.
    pub(crate) mirrored_mounts: u8,
    /// Vanilla Weapon.x of the primary mount (desktop.jar probe dump); the
    /// flipped copy fires from -mount_x.
    pub(crate) mount_offset: f32,
    pub(crate) collides_air: bool,
    pub(crate) collides_ground: bool,
    pub(crate) heals: bool,
    pub(crate) status_effect: i16,
    pub(crate) status_duration: f32,
    pub(crate) pierce_units: u8,
    pub(crate) pierce_buildings: u8,
}

/// Approximate TargetPriority (TargetPriority.java: wall -3, transport -1,
/// base 0, turret 1, core 2) for the blocks the port can classify.
fn building_target_priority(block: i16) -> i8 {
    if matches!(block, 339..=344) {
        2 // cores
    } else if crate::network::buildings::snapshot::is_snapshot_item_turret(block)
        || matches!(block, 356 | 359 | 384 | 385)
    {
        1 // turrets
    } else {
        0 // base blocks (walls not separately classified yet)
    }
}

pub(crate) const RISSO_GUN: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 41,
    shots: 1,
    direct_damage: 9.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 2.5,
    lifetime: 60.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 4.0,
    homing_delay: -1.0,
};

pub(crate) const RISSO_MISSILE: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 42,
    shots: 1,
    direct_damage: 12.0,
    splash_damage: 10.0,
    splash_radius: 25.0,
    speed: 2.7,
    lifetime: 65.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 60.0,
    homing_power: 0.08,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

pub(crate) const MINKE_GUN: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 43,
    shots: 1,
    direct_damage: 3.0,
    splash_damage: 40.5,
    splash_radius: 15.0,
    speed: 4.2,
    lifetime: 52.5,
    inaccuracy: 8.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 5.0,
    homing_delay: -1.0,
};

pub(crate) const MINKE_ARTILLERY: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 44,
    shots: 1,
    direct_damage: 20.0,
    splash_damage: 40.0,
    splash_radius: 22.5,
    speed: 3.0,
    lifetime: 73.5,
    inaccuracy: 2.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 5.0,
    homing_delay: -1.0,
};

pub(crate) const BRYDE_ARTILLERY: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 45,
    shots: 1,
    direct_damage: 15.0,
    splash_damage: 70.0,
    splash_radius: 40.0,
    speed: 3.2,
    lifetime: 84.0,
    inaccuracy: 3.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 18,
    status_duration: 60.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

pub(crate) const BRYDE_MISSILES: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 46,
    shots: 2,
    direct_damage: 12.0,
    splash_damage: 10.0,
    splash_radius: 25.0,
    speed: 2.7,
    lifetime: 70.0,
    inaccuracy: 5.0,
    velocity_random: 0.1,
    homing_range: 60.0,
    homing_power: 0.08,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 8.5,
    homing_delay: -1.0,
};

pub(crate) const SEI_LAUNCHER: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 47,
    shots: 6,
    direct_damage: 42.0,
    splash_damage: 45.0,
    splash_radius: 35.0,
    speed: 4.2,
    lifetime: 62.0,
    inaccuracy: 7.0,
    velocity_random: 0.4,
    homing_range: 80.0,
    homing_power: 0.12,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

pub(crate) const SEI_CANNON: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 48,
    shots: 3,
    direct_damage: 57.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 7.0,
    lifetime: 35.0,
    inaccuracy: 1.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 17.5,
    homing_delay: -1.0,
};

pub(crate) const RETUSA_BOLT: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 51,
    shots: 1,
    direct_damage: 12.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 5.2,
    lifetime: 30.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 4.5,
    homing_delay: -1.0,
};

pub(crate) const RETUSA_MINE: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 52,
    shots: 1,
    direct_damage: 1.0,
    splash_damage: 40.0,
    splash_radius: 32.0,
    speed: 0.7,
    lifetime: 87.0,
    inaccuracy: 2.0,
    velocity_random: 0.0,
    homing_range: 50.0,
    homing_power: 0.05,
    collides_air: false, // JAR probe: retusa mine cannot track flyers
    collides_ground: true,
    heals: true, // healPercent 4
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

pub(crate) const OMURA_RAIL: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 49,
    shots: 1,
    direct_damage: 1250.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 1.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: u8::MAX,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

pub(crate) const ANTUMBRA_MISSILE: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 33,
    shots: 1,
    direct_damage: 18.0,
    splash_damage: 37.0,
    splash_radius: 20.0,
    speed: 2.7,
    lifetime: 50.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 50.0,
    homing_power: 0.08,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 18,
    status_duration: 60.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 17.0,
    homing_delay: -1.0,
};

pub(crate) const ANTUMBRA_CANNON: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 34,
    shots: 1,
    direct_damage: 55.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 7.0,
    lifetime: 25.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 10.0,
    homing_delay: -1.0,
};

// Legacy ground/air units ported to authoritative projectiles (see
// tools/TASK_LEGACY_WEAPONS.md). The tsv in src/game/unit_weapons.tsv is the
// source of truth for bullet ids and fields; beam/sap lengths come from
// UnitTypes.java. Deviations are documented next to each constant.
pub(crate) const SCEPTER_BOLT: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 10,
    shots: 3,
    direct_damage: 70.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 8.0,
    lifetime: 27.0,
    inaccuracy: 3.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 16.0,
    homing_delay: -1.0,
};

pub(crate) const SCEPTER_MOUNT: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 9,
    shots: 1,
    direct_damage: 20.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 12.0,
    lifetime: 17.333334,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 8.5,
    homing_delay: -1.0,
};

// Scepter shotDelay (4 ticks between the 3 burst shots) is modeled by
// staggering each shot's remaining_ticks in the spawn helpers
// (volley_shot_delay). Lightning (2 hits of 20) and interval lightning (5)
// are still approximated as plain direct damage.
pub(crate) const VELA_BEAM: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 18,
    shots: 1,
    direct_damage: 35.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 160.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

// JAR class probe: vela (18) is a ContinuousLaserBulletType - 35 damage every
// damageInterval (5 ticks) along its beam; corvus (20) is a plain
// LaserBulletType - ONE 560-damage line impact, no per-interval ticking. The
// corvus healPercent 25 + collidesTeam heal side is applied on impact for
// team-1 projectiles.
pub(crate) const CORVUS_LASER: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 20,
    shots: 1,
    direct_damage: 560.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 65.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

// Deviation: corvus LaserBulletType heals same-team damaged buildings
// (healPercent 25, collidesTeam) along the beam; the server applies the heal
// on impact for team-1 projectiles (allied units are never healed: official
// collideLine only damages enemies and heals tiles).
pub(crate) const ATRAX_SLAG: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 22,
    shots: 1,
    direct_damage: 13.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 2.5,
    lifetime: 57.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 8,
    status_duration: 120.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 7.0,
    homing_delay: -1.0,
};

// Atrax is immune to burning(1) and melting(8): unit_immune_to_status
// blocks those statuses on every EnemyUnit application site.
pub(crate) const SPIROCT_SAP: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 23,
    shots: 1,
    direct_damage: 23.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 35.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 9,
    status_duration: 180.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 8.5,
    homing_delay: -1.0,
};

pub(crate) const SPIROCT_SAP_MOUNT: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 24,
    shots: 1,
    direct_damage: 18.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 25.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 9,
    status_duration: 180.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 4.0,
    homing_delay: -1.0,
};

pub(crate) const ARKYID_SAP: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 25,
    shots: 1,
    direct_damage: 40.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 30.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 9,
    status_duration: 180.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

pub(crate) const ARKYID_ARTILLERY: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 26,
    shots: 1,
    direct_damage: 12.0,
    splash_damage: 65.0,
    splash_radius: 70.0,
    speed: 2.0,
    lifetime: 70.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 9,
    status_duration: 600.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 9.0,
    homing_delay: -1.0,
};

// Deviation: arkyid large-purple-mount lightning (3) is approximated as plain
// direct + splash damage.
pub(crate) const TOXOPID_SHRAPNEL: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 27,
    shots: 2,
    direct_damage: 110.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 10.0,
    inaccuracy: 17.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 11.0,
    homing_delay: -1.0,
};

pub(crate) const TOXOPID_CANNON: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 28,
    shots: 1,
    direct_damage: 50.0,
    splash_damage: 75.0,
    splash_radius: 80.0,
    speed: 3.0,
    lifetime: 80.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 9,
    status_duration: 600.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

// Deviation: toxopid-cannon fragBullets (9 x 30 dmg, spread ±180 with
// random velocity 0.2..1.0 and life 0.3..1.0) are spawned deterministically
// in a full-circle spread by spawn_toxopid_fragments; lightning (5) is not
// modeled.
pub(crate) const FLARE_BOLT: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 30,
    shots: 3,
    direct_damage: 9.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 2.5,
    lifetime: 32.0,
    inaccuracy: 4.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

// Deviation: flare shotDelay 3 is modeled via volley_shot_delay (same
// staggering as the scepter burst).
pub(crate) const ECLIPSE_LASER: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 36,
    shots: 1,
    direct_damage: 115.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 0.0,
    lifetime: 16.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 18.0,
    homing_delay: -1.0,
};

pub(crate) const ECLIPSE_FLAK: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 35,
    shots: 1,
    direct_damage: 15.0,
    splash_damage: 65.0,
    splash_radius: 25.0,
    speed: 4.0,
    lifetime: 47.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: 18,
    status_duration: 60.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

// Deviation: eclipse laser side beams (sideAngle 20, sideLength 80) are not
// modeled; the main beam is approximated as a piercing beam projectile.
pub(crate) const POLY_MISSILE: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 37,
    shots: 1,
    direct_damage: 12.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 4.0,
    lifetime: 50.0,
    inaccuracy: 15.0,
    velocity_random: 0.5,
    homing_range: 50.0,
    homing_power: 0.08,
    collides_air: true,
    collides_ground: true,
    heals: true, // healPercent 5.5
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 3.75,
    homing_delay: -1.0,
};

// Poly missile homes per the JAR (homingPower 0.08, range 50); healing
// allies (healPercent 5.5) is applied on impact for team-1 projectiles.
pub(crate) const MEGA_HEAL_A: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 38,
    shots: 1,
    direct_damage: 10.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 5.2,
    lifetime: 35.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: true,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 8.0,
    homing_delay: -1.0,
};

pub(crate) const MEGA_HEAL_B: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 39,
    shots: 1,
    direct_damage: 8.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 5.2,
    lifetime: 35.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: true,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 2,
    mount_offset: 4.0,
    homing_delay: -1.0,
};

pub(crate) const QUAD_BOMB: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 40,
    shots: 1,
    direct_damage: 154.0,
    splash_damage: 220.0,
    splash_radius: 80.0,
    speed: 0.0,
    lifetime: 70.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

// Deviation: quad bomb drops (autoDropBombs, maxRange 30) are approximated by
// a 30-tile maximum travel; the 15% max-health heal for allies is applied on
// splash for team-1 projectiles.

// Oct (24) has no weapons: its ForceFieldAbility(140, 4, 7000, 480, 8) is
// modeled as a per-unit area shield (world.force_fields + oct_force_field_absorb)
// and its RepairFieldAbility(130, 120, 140) as a support pulse in
// apply_enemy_support_abilities. Vela (8) fires its main beam (18) through the
// volley table and repairs allies through the support beam in
// apply_enemy_support_abilities.

// Aegires (33) has no standard projectile weapon: its two point-defense mounts
// (bullets 58/59, 30 damage) only intercept hostile bullets
// (simulate_enemy_point_defense) and its EnergyFieldAbility is simulated
// separately. This entry approximates the primary point-defense bolt for the
// generic attack fallback so aegires units use their official bullet id
// instead of invisible instant damage (documented deviation).
pub(crate) const AEGIRES_PD: EnemyProjectileVolley = EnemyProjectileVolley {
    bullet_id: 58,
    shots: 1,
    direct_damage: 30.0,
    splash_damage: 0.0,
    splash_radius: 0.0,
    speed: 1.0,
    lifetime: 40.0,
    inaccuracy: 0.0,
    velocity_random: 0.0,
    homing_range: 0.0,
    homing_power: 0.0,
    collides_air: true,
    collides_ground: true,
    heals: false,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
    mirrored_mounts: 1,
    mount_offset: 0.0,
    homing_delay: -1.0,
};

pub(crate) fn retusa_mine_shots_between(previous: f32, current: f32) -> usize {
    if !previous.is_finite() || !current.is_finite() || current <= previous {
        return 0;
    }
    let first_cycle = (((previous - 14.0).max(0.0) / 90.0).floor() as i32).max(1);
    let last_cycle = (current / 90.0).floor() as i32 + 1;
    let mut shots = 0;
    for cycle in first_cycle..=last_cycle {
        for delay in [0.0, 7.0, 14.0] {
            let event = cycle as f32 * 90.0 + delay;
            if event > previous && event <= current {
                shots += 1;
            }
        }
    }
    shots
}

pub(crate) fn naval_weapon_volleys(
    unit_type: i16,
) -> Option<((f32, EnemyProjectileVolley), (f32, EnemyProjectileVolley))> {
    // Cycle lengths are the vanilla per-mount fire period: UnitType.init
    // doubles Weapon.reload when it appends the flipped mirror copy, so a
    // mirrored volley fires BOTH mounts every declared*2 ticks. Mount counts
    // come from volley_mount_count so the table stays tsv-driven.
    fn cycle(reload: f32, volley: EnemyProjectileVolley) -> f32 {
        reload * f32::from(volley_mount_count(volley))
    }
    match unit_type {
        // Legacy dual-mount units now also use this primary/secondary table:
        // spiroct (sap + sap mount), toxopid (shrapnel + cannon), mega
        // (heal-weapon-mount pair). Declared reloads match
        // src/game/unit_weapons.tsv.
        12 => Some((
            (cycle(14.0, SPIROCT_SAP), SPIROCT_SAP),
            (cycle(18.0, SPIROCT_SAP_MOUNT), SPIROCT_SAP_MOUNT),
        )),
        14 => Some((
            (cycle(30.0, TOXOPID_SHRAPNEL), TOXOPID_SHRAPNEL),
            (cycle(210.0, TOXOPID_CANNON), TOXOPID_CANNON),
        )),
        22 => Some((
            (cycle(24.0, MEGA_HEAL_A), MEGA_HEAL_A),
            (cycle(15.0, MEGA_HEAL_B), MEGA_HEAL_B),
        )),
        25 => Some((
            (cycle(13.0, RISSO_GUN), RISSO_GUN),
            (cycle(25.0, RISSO_MISSILE), RISSO_MISSILE),
        )),
        26 => Some((
            (cycle(10.0, MINKE_GUN), MINKE_GUN),
            (cycle(30.0, MINKE_ARTILLERY), MINKE_ARTILLERY),
        )),
        27 => Some((
            (cycle(65.0, BRYDE_ARTILLERY), BRYDE_ARTILLERY),
            (cycle(20.0, BRYDE_MISSILES), BRYDE_MISSILES),
        )),
        28 => Some((
            (cycle(45.0, SEI_LAUNCHER), SEI_LAUNCHER),
            (cycle(60.0, SEI_CANNON), SEI_CANNON),
        )),
        _ => None,
    }
}

/// Beam/sap/shrapnel lengths for speed-0 unit bullets, in world units
/// (pixels). Sources: UnitTypes.java weapon definitions (LaserBulletType.length,
/// SapBulletType.length, ShrapnelBulletType.length, quad bomb maxRange).
pub(crate) fn unit_weapon_beam_length(bullet_id: i16) -> Option<f32> {
    match bullet_id {
        18 => Some(180.0), // vela continuous laser
        20 => Some(460.0), // corvus laser
        23 => Some(75.0),  // spiroct sap
        24 => Some(40.0),  // spiroct mount-purple-weapon sap
        25 => Some(55.0),  // arkyid sap
        27 => Some(90.0),  // toxopid shrapnel
        36 => Some(230.0), // eclipse laser
        40 => Some(30.0),  // quad bomb maxRange
        _ => None,
    }
}

/// SAP lifesteal strength per bullet id (SapBulletType.sapStrength).
pub(crate) fn sap_strength(bullet_id: i16) -> f32 {
    match bullet_id {
        23 => 0.5,
        24 => 0.8,
        25 => 0.85,
        _ => 0.0,
    }
}

/// Weapon.shoot.shotDelay per bullet id: burst shots of a volley are fired
/// `shotDelay` ticks apart (UnitTypes.java scepter-weapon shotDelay 4,
/// flare shotDelay 3). The spawn helpers add `delay * shot_index` to each
/// shot's total flight time so the authoritative impacts stay spaced.
pub(crate) fn volley_shot_delay(bullet_id: i16) -> f32 {
    match bullet_id {
        10 => 4.0, // scepter-weapon burst (3 shots)
        30 => 3.0, // flare burst (3 shots)
        _ => 0.0,
    }
}

pub(crate) fn projectile_maximum_travel(volley: EnemyProjectileVolley, velocity_scale: f32) -> f32 {
    unit_weapon_beam_length(volley.bullet_id).unwrap_or(if volley.bullet_id == 49 {
        500.0 // RailBulletType.length
    } else {
        volley.speed * velocity_scale * volley.lifetime
    })
}

pub(crate) fn enemy_projectile_volley(unit_type: i16) -> Option<EnemyProjectileVolley> {
    match unit_type {
        0 => Some(EnemyProjectileVolley {
            bullet_id: 6,
            shots: 1,
            direct_damage: 9.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            speed: 2.5,
            lifetime: 60.0,
            inaccuracy: 0.0,
            velocity_random: 0.0,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            mirrored_mounts: 2,
            mount_offset: 4.0,
            homing_delay: -1.0,
        }),
        1 => Some(EnemyProjectileVolley {
            bullet_id: 7,
            shots: 1,
            direct_damage: 74.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            speed: 4.2,
            lifetime: 13.0,
            inaccuracy: 0.0,
            velocity_random: 0.0,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: 1,
            status_duration: 300.0,
            pierce_units: 2,
            pierce_buildings: 2,
            mirrored_mounts: 2,
            mount_offset: 5.0,
            homing_delay: -1.0,
        }),
        2 => Some(EnemyProjectileVolley {
            bullet_id: 8,
            shots: 1,
            direct_damage: 20.0,
            splash_damage: 80.0,
            splash_radius: 35.0,
            speed: 2.0,
            lifetime: 106.5,
            inaccuracy: 0.0,
            velocity_random: 0.0,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            mirrored_mounts: 2,
            mount_offset: 9.0,
            homing_delay: -1.0,
        }),
        4 => Some(EnemyProjectileVolley {
            bullet_id: 12,
            shots: 1,
            direct_damage: 80.0,
            splash_damage: 18.0,
            splash_radius: 13.0,
            speed: 13.0,
            lifetime: 15.0,
            inaccuracy: 0.0,
            velocity_random: 0.0,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 10,
            pierce_buildings: 0,
            mirrored_mounts: 2,
            mount_offset: 21.5,
            homing_delay: -1.0,
        }),
        17 => Some(EnemyProjectileVolley {
            bullet_id: 32,
            shots: 2,
            direct_damage: 14.0,
            splash_damage: 15.0,
            splash_radius: 25.0,
            speed: 3.0,
            lifetime: 50.0,
            inaccuracy: 5.0,
            velocity_random: 0.2,
            homing_range: 60.0,
            homing_power: 0.08,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            mirrored_mounts: 2,
            mount_offset: 7.0,
            homing_delay: -1.0,
        }),
        29 => Some(OMURA_RAIL),
        31 => Some(EnemyProjectileVolley {
            bullet_id: 53,
            shots: 1,
            direct_damage: 23.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            speed: 3.4,
            lifetime: 18.0,
            inaccuracy: 10.0,
            velocity_random: 0.0,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: 1,
            status_duration: 240.0,
            pierce_units: u8::MAX,
            pierce_buildings: 0,
            mirrored_mounts: 2,
            mount_offset: 4.5,
            homing_delay: -1.0,
        }),
        32 => Some(EnemyProjectileVolley {
            bullet_id: 56,
            shots: 1,
            direct_damage: 25.0,
            splash_damage: 25.0,
            splash_radius: 30.0,
            speed: 2.5,
            lifetime: 80.0,
            inaccuracy: 1.0,
            velocity_random: 0.1,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            mirrored_mounts: 2,
            mount_offset: 9.0,
            homing_delay: -1.0,
        }),
        34 => Some(EnemyProjectileVolley {
            bullet_id: 60,
            shots: 1,
            direct_damage: 60.0,
            splash_damage: 70.0,
            splash_radius: 100.0,
            speed: 5.0,
            lifetime: 60.0,
            inaccuracy: 0.0,
            velocity_random: 0.0,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            status_effect: 10,
            status_duration: 480.0,
            pierce_units: 0,
            pierce_buildings: 0,
            mirrored_mounts: 2,
            mount_offset: 17.5,
            homing_delay: -1.0,
        }),
        8 => Some(VELA_BEAM),
        9 => Some(CORVUS_LASER),
        11 => Some(ATRAX_SLAG),
        15 => Some(FLARE_BOLT),
        21 => Some(POLY_MISSILE),
        23 => Some(QUAD_BOMB),
        // Legacy units whose primary weapon resolves through this table.
        // Values follow src/game/unit_weapons.tsv and UnitTypes.java; the
        // secondary mounts stay in the dedicated enemy branches and in
        // collect_allied_weapon_fire / naval_weapon_volleys (documented).
        3 => Some(SCEPTER_BOLT), // scepter-weapon: bullet 10, 70 dmg, 3-shot burst, reload 45
        12 => Some(SPIROCT_SAP), // spiroct-weapon: bullet 23, 23 dmg sap, reload 14
        13 => Some(ARKYID_ARTILLERY), // large-purple-mount: bullet 26, 12 dmg + 65/70 splash, reload 45; the three spiroct-weapons (25) fire on secondary timers
        14 => Some(TOXOPID_SHRAPNEL), // large-purple-mount: bullet 27, 110 dmg, 2 shots, reload 30
        19 => Some(ECLIPSE_LASER),    // large-laser-mount: bullet 36, 115 dmg, reload 45
        22 => Some(MEGA_HEAL_A), // heal-weapon-mount: bullet 38, reload 24 (damage/heal bolt, 5.5% max)
        30 => Some(RETUSA_BOLT), // retusa-weapon: bullet 51, 12 dmg, reload 22
        33 => Some(AEGIRES_PD), // point-defense-mount: bullet 58, 30 dmg (approximation, see AEGIRES_PD)
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_navanax_lasers(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    damage_multiplier: f32,
    shooter_id: i32,
    projectile_target_id: i32,
    target_position: Option<i32>,
    target_core: bool,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
) {
    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let distance = dx.hypot(dy);
    let length = distance.min(95.0);
    let (impact_x, impact_y) = if distance > 0.001 {
        (
            source_x + dx / distance * length,
            source_y + dy / distance * length,
        )
    } else {
        (source_x, source_y)
    };
    let angle = dy.atan2(dx).to_degrees();
    for bullet_id in 61..=64 {
        let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
        world.projectiles.insert(
            projectile_id,
            Projectile {
                target_id: projectile_target_id,
                shooter_id,
                team,
                bullet_id,
                damage: 27.0 * damage_multiplier,
                splash_damage: 0.0,
                splash_radius: 0.0,
                status_effect: -1,
                status_duration: 0.0,
                pierce_units: u8::MAX,
                pierce_buildings: 0,
                spawn_reign_frags: false,
                homing_range: 0.0,
                homing_power: 0.0,
                homing_delay: -1.0,
                collides_air: true,
                collides_ground: true,
                heals: false,
                enemy_target_position: target_position,
                enemy_target_core: target_core,
                apply_direct_on_impact: true,
                armor_multiplier: 1.0,
                remaining_ticks: 155.0,
                total_ticks: 155.0,
                source_x,
                source_y,
                target_x: impact_x,
                target_y: impact_y,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: Some(5.0),
                damage_timer: 0.0,
                collided: Vec::new(),
            },
        );
        if let Ok(payload) = encode_create_bullet_payload(
            bullet_id,
            team,
            source_x,
            source_y,
            angle,
            27.0 * damage_multiplier,
            1.0,
            1.0,
        ) {
            if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
                out.broadcast(frame);
            }
        }
    }
}

/// Number of mounts that fire one weapon definition per reload. Vanilla
/// `Weapon.mirror` defaults to TRUE: `UnitType.init` appends a flipped copy of
/// every mirrored weapon (local muzzle x negated) and WeaponsComp fires every
/// mount with its own reload timer and the full shot pattern.
pub(crate) fn volley_mount_count(volley: EnemyProjectileVolley) -> u8 {
    volley.mirrored_mounts.clamp(1, 2)
}

/// Copy of `volley` with an explicit lateral muzzle offset. Shared volley
/// consts (one weapon definition fired by several reload timers) need the
/// per-mount Weapon.x of each timer group, e.g. arkyid's three spiroct-weapons
/// mounts at x=4/9/14 and eclipse's two large-artillery mounts at x=11/20.
pub(crate) fn volley_with_mount_offset(
    volley: EnemyProjectileVolley,
    mount_offset: f32,
) -> EnemyProjectileVolley {
    let mut adjusted = volley;
    adjusted.mount_offset = mount_offset;
    adjusted
}

/// Lateral muzzle offset of `mount` in world units, perpendicular to the aim
/// line (vanilla Weapon.x of the primary mount; the flipped copy negates it).
pub(crate) fn volley_mount_lateral(volley: EnemyProjectileVolley, mount: u8) -> f32 {
    if mount == 0 {
        volley.mount_offset
    } else {
        -volley.mount_offset
    }
}

/// Fire one weapon definition the way vanilla does: once per mount (mirrored
/// weapons fire their full shot pattern from both mounts), each shot through
/// [`spawn_enemy_projectile`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_enemy_volley(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_id: i32,
    target_position: Option<i32>,
    target_core: bool,
    volley: EnemyProjectileVolley,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
) {
    for mount in 0..volley_mount_count(volley) {
        let lateral = volley_mount_lateral(volley, mount);
        for shot_index in 0..volley.shots {
            spawn_enemy_projectile(
                world,
                out,
                source_id,
                target_position,
                target_core,
                volley,
                source_x,
                source_y,
                target_x,
                target_y,
                lateral,
                shot_index,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_allied_unit_projectile(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    shooter_id: i32,
    target_id: i32,
    target_position: Option<i32>,
    volley: EnemyProjectileVolley,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    shot_index: u8,
) -> i32 {
    spawn_allied_unit_projectile_lateral(
        world,
        out,
        shooter_id,
        target_id,
        target_position,
        volley,
        source_x,
        source_y,
        target_x,
        target_y,
        0.0,
        shot_index,
        1,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_allied_unit_projectile_lateral(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    shooter_id: i32,
    target_id: i32,
    target_position: Option<i32>,
    volley: EnemyProjectileVolley,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    mount_lateral: f32,
    shot_index: u8,
    team: u8,
) -> i32 {
    spawn_unit_projectile_for_team(
        world,
        out,
        shooter_id,
        target_id,
        target_position,
        volley,
        source_x,
        source_y,
        target_x,
        target_y,
        mount_lateral,
        shot_index,
        team,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_unit_projectile_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    shooter_id: i32,
    target_id: i32,
    target_position: Option<i32>,
    volley: EnemyProjectileVolley,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    mount_lateral: f32,
    shot_index: u8,
    team: u8,
) -> i32 {
    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let distance = dx.hypot(dy);
    // Mirror geometry (WeaponsComp): the muzzle sits at local (weapon.x * side,
    // weapon.y) rotated by unit.rotation - 90. With facing ~ aim direction the
    // offset is perpendicular to the aim line: mount_lateral * (sin, -cos).
    // The aim vector is computed FROM THE ACTUAL MUZZLE so every mount's shot
    // converges on the target point, like vanilla bullets fired at a target.
    let (source_x, source_y) = if mount_lateral != 0.0 && distance > 0.001 {
        (
            source_x + mount_lateral * dy / distance,
            source_y - mount_lateral * dx / distance,
        )
    } else {
        (source_x, source_y)
    };
    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let distance = dx.hypot(dy);
    let shot_fraction = if volley.shots <= 1 {
        0.5
    } else {
        f32::from(shot_index) / f32::from(volley.shots - 1)
    };
    let angle_offset = (shot_fraction - 0.5) * volley.inaccuracy;
    let velocity_scale = 1.0 + (shot_fraction * 2.0 - 1.0) * volley.velocity_random;
    let maximum_travel = projectile_maximum_travel(volley, velocity_scale);
    let travel = distance.min(maximum_travel);
    let (impact_x, impact_y) = if distance > 0.001 {
        (
            source_x + dx / distance * travel,
            source_y + dy / distance * travel,
        )
    } else {
        (source_x, source_y)
    };
    let adjusted_speed = volley.speed * velocity_scale;
    let total_ticks = if adjusted_speed > 0.0 {
        travel / adjusted_speed
    } else {
        volley.lifetime
    } + volley_shot_delay(volley.bullet_id) * f32::from(shot_index);
    let angle = dy.atan2(dx).to_degrees() + angle_offset;
    let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
    let rule = fire_damage_rule(world, team, false);
    world.projectiles.insert(
        projectile_id,
        Projectile {
            target_id,
            shooter_id,
            team,
            bullet_id: volley.bullet_id,
            damage: volley.direct_damage * rule,
            splash_damage: volley.splash_damage * rule,
            splash_radius: volley.splash_radius,
            status_effect: volley.status_effect,
            status_duration: volley.status_duration,
            pierce_units: volley.pierce_units,
            pierce_buildings: volley.pierce_buildings,
            spawn_reign_frags: volley.bullet_id == 12,
            homing_range: volley.homing_range,
            homing_power: volley.homing_power,
            homing_delay: volley.homing_delay,
            collides_air: volley.collides_air,
            collides_ground: volley.collides_ground,
            heals: volley.heals,
            enemy_target_position: target_position,
            enemy_target_core: false,
            apply_direct_on_impact: true,
            armor_multiplier: if volley.bullet_id == 60 { 0.8 } else { 1.0 },
            remaining_ticks: total_ticks,
            total_ticks,
            source_x,
            source_y,
            target_x: impact_x,
            target_y: impact_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: continuous_damage_interval(volley.bullet_id),
            damage_timer: 0.0,
            collided: Vec::new(),
        },
    );
    if let Ok(payload) = encode_create_bullet_payload(
        volley.bullet_id,
        team,
        source_x,
        source_y,
        angle,
        volley.direct_damage * rule,
        velocity_scale,
        1.0,
    ) {
        if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
    projectile_id
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_enemy_projectile(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_id: i32,
    target_position: Option<i32>,
    target_core: bool,
    volley: EnemyProjectileVolley,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    mount_lateral: f32,
    shot_index: u8,
) -> i32 {
    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let distance = dx.hypot(dy);
    // Mirror geometry (WeaponsComp): the muzzle sits at local (weapon.x * side,
    // weapon.y) rotated by unit.rotation - 90. With facing ~ aim direction the
    // offset is perpendicular to the aim line: mount_lateral * (sin, -cos).
    // The aim vector is computed FROM THE ACTUAL MUZZLE so every mount's shot
    // converges on the target point, like vanilla bullets fired at a target.
    let (source_x, source_y) = if mount_lateral != 0.0 && distance > 0.001 {
        (
            source_x + mount_lateral * dy / distance,
            source_y - mount_lateral * dx / distance,
        )
    } else {
        (source_x, source_y)
    };
    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let distance = dx.hypot(dy);
    let shot_fraction = if volley.shots <= 1 {
        0.5
    } else {
        f32::from(shot_index) / f32::from(volley.shots - 1)
    };
    let angle_offset = (shot_fraction - 0.5) * volley.inaccuracy;
    let velocity_scale = 1.0 + (shot_fraction * 2.0 - 1.0) * volley.velocity_random;
    let maximum_travel = projectile_maximum_travel(volley, velocity_scale);
    let travel = distance.min(maximum_travel);
    let (impact_x, impact_y) = if distance > 0.001 {
        (
            source_x + dx / distance * travel,
            source_y + dy / distance * travel,
        )
    } else {
        (source_x, source_y)
    };
    let adjusted_speed = volley.speed * velocity_scale;
    let total_ticks = if adjusted_speed > 0.0 {
        travel / adjusted_speed
    } else {
        volley.lifetime
    } + volley_shot_delay(volley.bullet_id) * f32::from(shot_index);
    let angle = dy.atan2(dx).to_degrees() + angle_offset;
    let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
    let rule = fire_damage_rule(world, 2, false);
    world.projectiles.insert(
        projectile_id,
        Projectile {
            target_id: source_id,
            shooter_id: source_id,
            team: 2,
            bullet_id: volley.bullet_id,
            damage: volley.direct_damage * rule,
            splash_damage: volley.splash_damage * rule,
            splash_radius: volley.splash_radius,
            status_effect: volley.status_effect,
            status_duration: volley.status_duration,
            pierce_units: volley.pierce_units,
            pierce_buildings: volley.pierce_buildings,
            spawn_reign_frags: volley.bullet_id == 12,
            homing_range: volley.homing_range,
            homing_power: volley.homing_power,
            homing_delay: volley.homing_delay,
            collides_air: volley.collides_air,
            collides_ground: volley.collides_ground,
            heals: volley.heals,
            enemy_target_position: target_position,
            enemy_target_core: target_core,
            apply_direct_on_impact: distance <= maximum_travel + 0.001
                && (volley.pierce_buildings == 0 || target_core),
            armor_multiplier: if volley.bullet_id == 60 { 0.8 } else { 1.0 },
            remaining_ticks: total_ticks,
            total_ticks,
            source_x,
            source_y,
            target_x: impact_x,
            target_y: impact_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: continuous_damage_interval(volley.bullet_id),
            damage_timer: 0.0,
            collided: Vec::new(),
        },
    );
    if let Ok(payload) = encode_create_bullet_payload(
        volley.bullet_id,
        2,
        source_x,
        source_y,
        angle,
        volley.direct_damage * rule,
        velocity_scale,
        1.0,
    ) {
        if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
    projectile_id
}

/// Beam-class bullets routed through the line-damage path
/// (`Damage.collideLine`) instead of the single-target impact path:
/// vela 18 and corvus 20. Only vela is a real `ContinuousLaserBulletType`;
/// corvus is a plain `LaserBulletType` that hits ONCE.
pub(crate) fn is_continuous_laser(bullet_id: i16) -> bool {
    // JAR class probe: only 18 (vela continuous laser) is a
    // ContinuousLaserBulletType. 20 (corvus) is a LaserBulletType - a single
    // 560-damage beam impact, not per-interval damage.
    bullet_id == 18
}

/// Per-interval damage (official `ContinuousBulletType.damageInterval` = 5):
/// only the `ContinuousLaserBulletType` beams re-apply damage every interval
/// over their lifetime (vela-weapon, UnitTypes.java: damage 35, lifetime 160).
/// The corvus-weapon is a plain `LaserBulletType` (extends BulletType, NOT
/// ContinuousBulletType): it applies its full 560 damage exactly once along
/// the beam, then keeps drawing until its 65-tick lifetime ends - no
/// per-interval repetition.
fn continuous_damage_interval(bullet_id: i16) -> Option<f32> {
    match bullet_id {
        18 => Some(5.0),
        _ => None,
    }
}

/// Bridge from the projectile impact path to the authoritative lightning
/// chains (`Lightning.create`). No-op for bullets without a lightning spec.
#[allow(clippy::too_many_arguments)]
fn spawn_impact_lightning_for_bullet(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    bullet_id: i16,
    projectile_id: i32,
    source_x: f32,
    source_y: f32,
    impact_x: f32,
    impact_y: f32,
) -> bool {
    match super::lightning_spec(bullet_id) {
        Some(spec) => super::spawn_impact_lightning(
            world,
            out,
            team,
            spec,
            projectile_id,
            source_x,
            source_y,
            impact_x,
            impact_y,
        ),
        None => false,
    }
}

/// Surge-wall `Wall.collision` lightning: 5% chance, length 17, 20 damage
/// (30 on reinforced 240/241). Fires back along the incoming bullet.
fn surge_wall_lightning_spec(block: i16) -> Option<(f32, u8)> {
    match block {
        226 | 227 => Some((20.0, 17)),
        240 | 241 => Some((30.0, 17)),
        _ => None,
    }
}

fn maybe_surge_wall_lightning(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    position: i32,
    projectile_id: i32,
    source_x: f32,
    source_y: f32,
) -> bool {
    let (block, team) = if let Some(tile) = world.tiles.get(&position) {
        (tile.block, tile.team)
    } else if let Some(building) = world.base_buildings.get(&position) {
        (building.block, building.team)
    } else {
        return false;
    };
    let Some((damage, length)) = surge_wall_lightning_spec(block) else {
        return false;
    };
    let mut rng = super::DetRand::new((position as u64).rotate_left(7) ^ projectile_id as u64);
    if rng.unit_f32() >= 0.05 {
        return false;
    }
    let wx = ((position >> 16) as i16 as f32) * 8.0 + 4.0;
    let wy = (position as i16 as f32) * 8.0 + 4.0;
    super::spawn_impact_lightning(
        world,
        out,
        team,
        super::LightningSpec {
            roots: 1,
            length,
            length_rand: 0,
            damage,
            target: super::LightningTarget::All,
        },
        projectile_id,
        wx,
        wy,
        source_x,
        source_y,
    )
}

pub(crate) fn projectile_armor_multiplier(bullet_id: i16) -> f32 {
    match bullet_id {
        129 => 4.0, // Lancer LaserBulletType.
        _ => 1.0,
    }
}

/// Official BulletType.buildingDamageMultiplier for splash applied to buildings.
pub(crate) fn projectile_building_damage_multiplier(bullet_id: i16) -> f32 {
    match bullet_id {
        188 => 0.1,
        191 => 0.2,
        _ => 1.0,
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_create_bullet_payload(
    bullet_id: i16,
    team: u8,
    x: f32,
    y: f32,
    angle: f32,
    damage: f32,
    velocity_scale: f32,
    lifetime_scale: f32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(29);
    payload.write_s(bullet_id)?;
    payload.write_b(team)?;
    payload.write_f(x)?;
    payload.write_f(y)?;
    payload.write_f(angle)?;
    payload.write_f(damage)?;
    payload.write_f(velocity_scale)?;
    payload.write_f(lifetime_scale)?;
    Ok(payload)
}

pub(crate) fn encode_projectile_replay_payload(
    projectile: &Projectile,
    current_target: Option<(f32, f32)>,
) -> std::io::Result<Vec<u8>> {
    let remaining_fraction = if projectile.total_ticks > 0.0001 {
        (projectile.remaining_ticks / projectile.total_ticks).clamp(0.001, 1.0)
    } else {
        1.0
    };
    // Beam-class bullets stay anchored at the shooter for the whole
    // lifetime, even when only their damage repeats per interval.
    let continuous =
        is_continuous_laser(projectile.bullet_id) || projectile.damage_interval.is_some();
    let (x, y) = if continuous {
        (projectile.source_x, projectile.source_y)
    } else {
        let progress = 1.0 - remaining_fraction;
        (
            projectile.source_x + (projectile.target_x - projectile.source_x) * progress,
            projectile.source_y + (projectile.target_y - projectile.source_y) * progress,
        )
    };
    let (target_x, target_y) = current_target.unwrap_or((projectile.target_x, projectile.target_y));
    let angle = (target_y - y).atan2(target_x - x).to_degrees();
    encode_create_bullet_payload(
        projectile.bullet_id,
        projectile.team,
        x,
        y,
        angle,
        projectile.damage,
        1.0,
        (projectile.lifetime_scale * remaining_fraction).max(0.001),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_continuous_projectile(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_position: i32,
    target_id: i32,
    bullet_id: i16,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    duration: f32,
    damage_interval: f32,
) {
    spawn_continuous_projectile_for_team(
        world,
        out,
        source_position,
        target_id,
        bullet_id,
        source_x,
        source_y,
        target_x,
        target_y,
        damage,
        duration,
        damage_interval,
        1,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_continuous_projectile_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_position: i32,
    target_id: i32,
    bullet_id: i16,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    duration: f32,
    damage_interval: f32,
    team: u8,
) {
    let projectile_id = spawn_projectile_for_team(
        world,
        out,
        Some(source_position),
        target_id,
        bullet_id,
        source_x,
        source_y,
        target_x,
        target_y,
        damage,
        0.0,
        0.0,
        duration / 16.0,
        team,
    );
    if let Some(mut projectile) = world.projectiles.get_mut(&projectile_id) {
        projectile.remaining_ticks = duration;
        projectile.total_ticks = duration;
        projectile.damage_interval = Some(damage_interval);
    }
}

/// Official `BulletType.spawnUnit` payloads (JAR 158.1 `BulletType.create`):
/// these launcher bullets never become bullet entities; they carry a
/// MissileUnitType that joins the shooter's team. The headless flight model
/// keeps the launcher projectile until its target point and inserts the unit
/// there (vanilla inserts it immediately at the launch site with
/// `spawned.rotation = angle`).
pub(crate) fn spawn_unit_bullet_payload(bullet_id: i16) -> Option<i16> {
    match bullet_id {
        92 => Some(46), // anthicus -> anthicus-missile
        // quell: the launcher (103) carries an anonymous fragBullet carrier
        // (port id 104, UnitTypes.java quell-weapon "workaround ... spawning
        // on death"); the carrier's spawnUnit inserts quell-missile.
        104 => Some(53),
        106 => Some(55), // disrupt -> disrupt-missile
        // scathe turret launchers (Blocks.java v159.7, economy/erekir.rs
        // module comment): 186/189/192 are BulletType(0,0) payloads carrying
        // spawnUnit scathe-missile/-phase/-surge; they never become bullet
        // entities in vanilla. The surge-split(67) payload rides frag
        // carrier 194 of the surge missile's own death explosion (193),
        // which the port does not fly as a projectile - see README.
        186 => Some(64), // scathe: scathe-missile
        189 => Some(65), // scathe: scathe-missile-phase
        192 => Some(66), // scathe: scathe-missile-surge
        _ => None,
    }
}

/// Official `BulletType.fragBullet` chains that end in a `spawnUnit` payload
/// (quell-weapon, UnitTypes.java 159.7): when launcher 103 dies it creates
/// exactly one frag of the anonymous carrier bullet 104 (fragBullets 1,
/// fragRandomSpread 0, speed 0), whose `create` immediately spawns the unit.
/// Vanilla never registers the carrier as a bullet entity (`create` returns
/// before `init`), so this only reroutes the payload lookup - no projectile
/// is spawned and no extra CreateBullet packet is emitted.
pub(crate) fn spawn_unit_frag_carrier(bullet_id: i16) -> i16 {
    match bullet_id {
        103 => 104,
        _ => bullet_id,
    }
}

/// Insert the spawnUnit payload of a launcher projectile at its impact point.
/// Returns true when a unit joined `world.enemies`. Unit types without an
/// enemy spec are refused (same rule as console spawn), keeping the old
/// no-insert behaviour instead of guessing stats.
fn spawn_projectile_unit(
    world: &DynamicWorld,
    bullet_id: i16,
    team: u8,
    source_x: f32,
    source_y: f32,
    x: f32,
    y: f32,
) -> bool {
    let Some(unit_type) = spawn_unit_bullet_payload(bullet_id) else {
        return false;
    };
    let rotation = (y - source_y).atan2(x - source_x).to_degrees();
    crate::network::units::spawn_unit_world(world, unit_type, team, x, y, rotation).is_some()
}

/// Physical bullets collide with swept hitboxes as they travel. A cached aim
/// is only a firing direction; it never authorizes remote damage to that target.
fn simulate_ballistic_projectile(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    id: i32,
    mut projectile: Projectile,
    delta_ticks: f32,
) -> bool {
    let mut collision = crate::game::bullet_catalog::bullet_collision(projectile.bullet_id);
    collision.air &= projectile.collides_air;
    collision.ground &= projectile.collides_ground;
    let progress = |remaining: f32| {
        if projectile.total_ticks <= 0.00001 {
            1.0
        } else {
            (1.0 - remaining / projectile.total_ticks).clamp(0.0, 1.0)
        }
    };
    let point = |fraction: f32| {
        (
            projectile.source_x + (projectile.target_x - projectile.source_x) * fraction,
            projectile.source_y + (projectile.target_y - projectile.source_y) * fraction,
        )
    };
    let from = point(progress(projectile.remaining_ticks));
    let remaining = projectile.remaining_ticks - delta_ticks.max(0.0);
    let to = point(progress(remaining));
    if oct_force_field_absorb(world, projectile.team, to.0, to.1, projectile.damage)
        || quasar_force_field_absorb(world, projectile.team, to.0, to.1, projectile.damage)
        || tecta_shield_arc_absorb(world, projectile.team, to.0, to.1, projectile.damage)
    {
        world.projectiles.remove(&id);
        return true;
    }
    let hits = super::damage::projectile_line_targets(world, projectile.team, from, to, collision);
    projectile.remaining_ticks = remaining;
    (projectile.damage, projectile.splash_damage) = inventoried_hit(
        projectile.bullet_id,
        projectile.damage,
        projectile.splash_damage,
    );
    let mut changed = false;
    let mut removed = false;
    for (fraction, target) in hits {
        if projectile.collided.contains(&target) {
            continue;
        }
        let (x, y) = (
            from.0 + (to.0 - from.0) * fraction,
            from.1 + (to.1 - from.1) * fraction,
        );
        if oct_force_field_absorb(world, projectile.team, x, y, projectile.damage)
            || quasar_force_field_absorb(world, projectile.team, x, y, projectile.damage)
            || tecta_shield_arc_absorb(world, projectile.team, x, y, projectile.damage)
        {
            removed = true;
            changed = true;
            break;
        }
        changed |= super::damage::damage_projectile_target(world, out, target, &projectile);
        changed |= projectile_impact_effects(world, out, id, &projectile, x, y);
        projectile.collided.push(target);
        let pierce = match target {
            ProjectileHit::Unit(_) | ProjectileHit::Player(_) => projectile.pierce_units,
            ProjectileHit::Building(_) | ProjectileHit::Core(_, _) => projectile.pierce_buildings,
        };
        if pierce == 0 || projectile.collided.len() >= usize::from(pierce) {
            removed = true;
            break;
        }
    }
    if !removed && remaining <= 0.0 {
        if collision.despawn_hit {
            changed |= projectile_impact_effects(world, out, id, &projectile, to.0, to.1);
        }
        removed = true;
    }
    if removed {
        world.projectiles.remove(&id);
    } else {
        world.projectiles.insert(id, projectile);
    }
    changed
}

fn projectile_impact_effects(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    id: i32,
    projectile: &Projectile,
    x: f32,
    y: f32,
) -> bool {
    let p = projectile;
    let mut changed = false;
    if p.splash_damage > 0.0 && p.splash_radius > 0.0 {
        let collision = crate::game::bullet_catalog::bullet_collision(p.bullet_id);
        changed |= super::damage::apply_splash_damage_filtered(
            world,
            out,
            p.team,
            x,
            y,
            p.splash_damage,
            p.splash_radius,
            p.armor_multiplier,
            p.status_effect,
            p.status_duration,
            projectile_building_damage_multiplier(p.bullet_id),
            collision.air && p.collides_air,
            collision.ground && p.collides_ground,
        );
    }
    if let Some(percent) = projectile_splash_heal_percent(p.bullet_id) {
        changed |=
            apply_splash_building_heal_for_team(world, out, p.team, x, y, p.splash_radius, percent);
    }
    if p.bullet_id == 60 {
        changed |= apply_emp_bullet_effects(world, out, p.team, x, y, p.splash_radius, p.damage);
    }
    if p.spawn_reign_frags {
        spawn_reign_fragments(
            world,
            out,
            p.team,
            p.target_id,
            p.source_x,
            p.source_y,
            x,
            y,
        );
    }
    if p.bullet_id == 56 {
        spawn_cyerce_fragments(
            world,
            out,
            p.team,
            p.target_id,
            p.source_x,
            p.source_y,
            x,
            y,
        );
    }
    if p.bullet_id == 28 {
        changed |= spawn_toxopid_fragments(
            world,
            out,
            p.team,
            p.target_id,
            p.shooter_id,
            p.source_x,
            p.source_y,
            x,
            y,
        );
    }
    changed |= spawn_projectile_unit(
        world,
        spawn_unit_frag_carrier(p.bullet_id),
        p.team,
        p.source_x,
        p.source_y,
        x,
        y,
    );
    changed |= spawn_impact_lightning_for_bullet(
        world,
        out,
        p.team,
        p.bullet_id,
        id,
        p.source_x,
        p.source_y,
        x,
        y,
    );
    changed
}

pub(crate) fn simulate_projectiles(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let mut world_changed = false;
    let ids: Vec<_> = world.projectiles.iter().map(|entry| *entry.key()).collect();
    for id in ids {
        let absorbed = world
            .projectiles
            .get(&id)
            .is_some_and(|projectile| absorb_enemy_projectile(world, &projectile, delta_ticks));
        if absorbed {
            world.projectiles.remove(&id);
            world_changed = true;
            continue;
        }
        // Official BulletType.updateHoming calls Units.closestTarget every
        // tick from the bullet position: the missile homes toward the NEAREST
        // opposing live target inside homingRange, retargeting as units move
        // or die. Snapshot first and drop the guard before touching another
        // map (DashMap DM rule).
        // Official BulletType.updateHoming: the whole homing block is gated
        // on homingPower > 1.0E-4 and bullet.time >= homingDelay. Below that
        // power the bullet flies BALLISTICALLY -- it never re-aims.
        let homing_aim: Option<(f32, f32)> = if world.projectiles.get(&id).is_some_and(|p| {
            p.homing_range > 0.0
                && p.homing_power > 0.0001
                && p.total_ticks - p.remaining_ticks >= p.homing_delay
                && p.damage_interval.is_none()
        }) {
            let (
                progress,
                source_x,
                source_y,
                target_x,
                target_y,
                homing_range,
                team,
                aim_tile,
                collides_air,
                collides_ground,
                heals,
            ) = {
                let p = world.projectiles.get(&id).unwrap();
                (
                    1.0 - p.remaining_ticks / p.total_ticks,
                    p.source_x,
                    p.source_y,
                    p.target_x,
                    p.target_y,
                    p.homing_range,
                    p.team,
                    p.enemy_target_position,
                    p.collides_air,
                    p.collides_ground,
                    p.heals,
                )
            };
            let bx = source_x + (target_x - source_x) * progress;
            let by = source_y + (target_y - source_y) * progress;
            // Official updateHoming / Units.closestTarget order:
            // 1. heals() branch or aimTile.build (non-heals, collidesGround).
            // 2. nearest opposing UNIT passing checkTarget(air, ground) --
            //    units ALWAYS outrank buildings.
            // 3. only with no unit in range: opposing buildings by
            //    TargetPriority (core 2 > turret 1 > base 0 > wall -3),
            //    nearest first within equal priority.
            let mut best: Option<(f32, (f32, f32))> = None;
            let unit_in_range = |out: &mut Option<(f32, (f32, f32))>,
                                 current: Option<(f32, (f32, f32))>,
                                 ux: f32,
                                 uy: f32,
                                 hit_size: f32|
             -> bool {
                // Units.java:301-303: dst2 - hitSize^2 < range^2, strict,
                // so big units are hittable from slightly farther.
                let distance_sq = (ux - bx).hypot(uy - by);
                if (distance_sq * distance_sq - hit_size * hit_size) >= homing_range * homing_range
                {
                    return false;
                }
                if current.is_none_or(|(d, _)| distance_sq < d) {
                    *out = Some((distance_sq, (ux, uy)));
                }
                true
            };
            // Step 1 (non-heals): b.aimTile.build outranks every
            // closestTarget candidate when the shooter aimed at an opposing
            // building and collidesGround allows hitting it.
            if !heals {
                if let Some(aim_position) = aim_tile.filter(|_| collides_ground) {
                    if let Some(tile) = world.tiles.get(&aim_position) {
                        if tile.block != 0 && tile.team != team {
                            let tx = (tile.position >> 16) as i16 as f32 * 8.0;
                            let ty = tile.position as i16 as f32 * 8.0;
                            best = Some(((tx - bx).hypot(ty - by), (tx, ty)));
                        }
                    } else if let Some(building) = world.base_buildings.get(&aim_position) {
                        if building.team != team {
                            let bx2 = (building.position >> 16) as i16 as f32 * 8.0;
                            let by2 = building.position as i16 as f32 * 8.0;
                            best = Some(((bx2 - bx).hypot(by2 - by), (bx2, by2)));
                        }
                    }
                }
            }
            let mut found_unit = false;
            // Vanilla closestTarget aims at the OPPOSING team for normal
            // bullets; heals() bolts pass targetTeam = bullet.team, so they
            // scan ALLIED units (checkTarget only -- no damaged gate on
            // units, unlike allied buildings).
            let scan_allied_units = heals;
            if (team == 2) != scan_allied_units {
                for entry in world.players.iter() {
                    if entry.dead || possessed_unit_id(world, *entry.key()).is_some() {
                        continue;
                    }
                    // PlayerCombatState carries no unit type; treat riders as
                    // grounded (collidesGround is true for every homing id).
                    found_unit |= {
                        let snapshot = best;
                        unit_in_range(&mut best, snapshot, entry.x, entry.y, 0.0)
                    };
                }
            } else {
                for entry in world.enemies.iter() {
                    let movement = crate::game::content::unit_movement(entry.unit_type);
                    let flying = movement.flying || entry.elevation > 0.01;
                    // Unit.checkTarget(collidesAir, collidesGround).
                    if (!collides_air && flying) || (!collides_ground && !flying) {
                        continue;
                    }
                    let hit_size =
                        crate::network::combat::unit_combat::unit_hit_size(entry.unit_type);
                    found_unit |= {
                        let snapshot = best;
                        unit_in_range(&mut best, snapshot, entry.x, entry.y, hit_size)
                    };
                }
            }
            if !found_unit && collides_ground {
                // findEnemyTile: highest TargetPriority wins regardless of
                // distance; ties break by distance (BlockIndexer.java:470+).
                let mut building_best: Option<(i8, f32, (f32, f32))> = None;
                for tile in world.tiles.iter() {
                    if tile.block == 0 || tile.team == team {
                        continue;
                    }
                    let priority = building_target_priority(tile.block);
                    let tx = (tile.position >> 16) as i16 as f32 * 8.0;
                    let ty = tile.position as i16 as f32 * 8.0;
                    let hit_size = f32::from(crate::game::content::block_size(tile.block)) * 8.0;
                    let effective = (tx - bx).hypot(ty - by) - hit_size / 2.0;
                    if effective >= homing_range {
                        continue;
                    }
                    // heals() bolts home toward damaged ALLIED buildings
                    // only (closestTarget with targetTeam = bullet.team);
                    // they NEVER target enemies. Normal bullets keep the
                    // opposing-team filter.
                    let candidate_team_ok = if heals {
                        tile.team == team
                            && tile.health < crate::game::content::block_health(tile.block)
                    } else {
                        tile.team != team
                    };
                    if candidate_team_ok {
                        let candidate = (priority, effective, (tx, ty));
                        let replace = match building_best {
                            None => true,
                            Some((bp, bd, _)) => {
                                priority > bp || (priority == bp && effective < bd)
                            }
                        };
                        if replace {
                            building_best = Some(candidate);
                        }
                    }
                }
                for building in world.base_buildings.iter() {
                    if heals {
                        if building.team != team
                            || building.health >= crate::game::content::block_health(building.block)
                        {
                            continue;
                        }
                    } else if building.team == team {
                        continue;
                    }
                    let bx2 = (building.position >> 16) as i16 as f32 * 8.0;
                    let by2 = building.position as i16 as f32 * 8.0;
                    let priority = building_target_priority(building.block);
                    let hit_size =
                        f32::from(crate::game::content::block_size(building.block)) * 8.0;
                    let effective = (bx2 - bx).hypot(by2 - by) - hit_size / 2.0;
                    if effective >= homing_range {
                        continue;
                    }
                    let candidate = (priority, effective, (bx2, by2));
                    let replace = match building_best {
                        None => true,
                        Some((bp, bd, _)) => priority > bp || (priority == bp && effective < bd),
                    };
                    if replace {
                        building_best = Some(candidate);
                    }
                }
                if let Some((_, _, aim)) = building_best {
                    best = Some((0.0, aim));
                }
            }
            best.map(|(_, aim)| aim)
        } else {
            None
        };
        if let Some((tx, ty)) = homing_aim {
            if let Some(mut projectile) = world.projectiles.get_mut(&id) {
                // Official BulletType.updateHoming steering:
                // vel.setAngle(Angles.moveToward(vel.angle(), angleTo(target),
                // homingPower * Time.delta * 50f)). The heading turns toward
                // the target at most homingPower * 50 degrees per tick -- it
                // does NOT snap. In this lerp model the heading is the
                // direction from the current position to the stored impact
                // point, so rotate that point around the bullet by the
                // capped angle step. Vanilla rates are degrees/tick; the
                // rotation below works in radians, so convert first.
                let turn_rate = (projectile.homing_power * 50.0 * delta_ticks).to_radians();
                let progress = 1.0 - projectile.remaining_ticks / projectile.total_ticks;
                let bx =
                    projectile.source_x + (projectile.target_x - projectile.source_x) * progress;
                let by =
                    projectile.source_y + (projectile.target_y - projectile.source_y) * progress;
                let to_target_x = projectile.target_x - bx;
                let to_target_y = projectile.target_y - by;
                let to_aim_x = tx - bx;
                let to_aim_y = ty - by;
                if turn_rate > 0.0
                    && to_target_x * to_target_x + to_target_y * to_target_y > 1.0e-6
                    && to_aim_x * to_aim_x + to_aim_y * to_aim_y > 1.0e-6
                {
                    use std::f32::consts::PI;
                    use std::f32::consts::TAU;
                    let heading = to_target_y.atan2(to_target_x);
                    let desired = to_aim_y.atan2(to_aim_x);
                    // Shortest signed angle delta, bounded math (no loops).
                    let diff = (desired - heading + PI).rem_euclid(TAU) - PI;
                    let step = diff.clamp(-turn_rate, turn_rate);
                    if step != 0.0 {
                        let (sin, cos) = step.sin_cos();
                        projectile.target_x = bx + to_target_x * cos - to_target_y * sin;
                        projectile.target_y = by + to_target_x * sin + to_target_y * cos;
                    }
                }
                // turn_rate <= 0 cannot happen here (gate above), and even if
                // it could, vanilla would keep the bullet ballistic instead
                // of snapping onto the aim point.
            }
        }
        // LightningBulletType and spawnUnit carriers execute an immediate
        // effect/creation path; their default collides flag does not make
        // them moving bullet entities.
        let ballistic = world
            .projectiles
            .get(&id)
            .filter(|p| {
                p.damage_interval.is_none()
                    && p.bullet_id != 15
                    && spawn_unit_bullet_payload(p.bullet_id).is_none()
                    && crate::game::bullet_catalog::bullet_collision(p.bullet_id).collides
            })
            .map(|p| p.clone());
        if let Some(projectile) = ballistic {
            world_changed |= simulate_ballistic_projectile(world, out, id, projectile, delta_ticks);
            continue;
        }
        let impact = if let Some(mut projectile) = world.projectiles.get_mut(&id) {
            if let Some(interval) = projectile.damage_interval {
                let elapsed = delta_ticks.max(0.0).min(projectile.remaining_ticks);
                projectile.remaining_ticks -= elapsed;
                projectile.damage_timer += elapsed;
                let hits = (projectile.damage_timer / interval).floor();
                projectile.damage_timer -= hits * interval;
                let expired = projectile.remaining_ticks <= 0.0;
                let (damage, splash) = inventoried_hit(
                    projectile.bullet_id,
                    projectile.damage * hits,
                    projectile.splash_damage,
                );
                (hits > 0.0 || expired).then_some((
                    projectile.target_id,
                    projectile.shooter_id,
                    projectile.team,
                    projectile.bullet_id,
                    damage,
                    splash,
                    projectile.splash_radius,
                    projectile.status_effect,
                    projectile.status_duration,
                    projectile.target_x,
                    projectile.target_y,
                    projectile.enemy_target_position,
                    projectile.enemy_target_core,
                    projectile.apply_direct_on_impact,
                    projectile.armor_multiplier,
                    projectile.pierce_units,
                    projectile.pierce_buildings,
                    projectile.spawn_reign_frags,
                    projectile.homing_range,
                    projectile.remaining_ticks,
                    projectile.total_ticks,
                    projectile.source_x,
                    projectile.source_y,
                    projectile.source_position,
                    expired,
                ))
            } else {
                projectile.remaining_ticks -= delta_ticks;
                let (damage, splash) = inventoried_hit(
                    projectile.bullet_id,
                    projectile.damage,
                    projectile.splash_damage,
                );
                (projectile.remaining_ticks <= 0.0).then_some((
                    projectile.target_id,
                    projectile.shooter_id,
                    projectile.team,
                    projectile.bullet_id,
                    damage,
                    splash,
                    projectile.splash_radius,
                    projectile.status_effect,
                    projectile.status_duration,
                    projectile.target_x,
                    projectile.target_y,
                    projectile.enemy_target_position,
                    projectile.enemy_target_core,
                    projectile.apply_direct_on_impact,
                    projectile.armor_multiplier,
                    projectile.pierce_units,
                    projectile.pierce_buildings,
                    projectile.spawn_reign_frags,
                    projectile.homing_range,
                    projectile.remaining_ticks,
                    projectile.total_ticks,
                    projectile.source_x,
                    projectile.source_y,
                    projectile.source_position,
                    true,
                ))
            }
        } else {
            None
        };
        let Some((
            target_id,
            shooter_id,
            team,
            bullet_id,
            damage,
            splash_damage,
            splash_radius,
            status_effect,
            status_duration,
            impact_x,
            impact_y,
            enemy_target_position,
            enemy_target_core,
            apply_direct_on_impact,
            armor_multiplier,
            pierce_units,
            pierce_buildings,
            spawn_reign_frags,
            homing_range,
            remaining_ticks,
            impact_total_ticks,
            source_x,
            source_y,
            _source_position,
            mut expired,
        )) = impact
        else {
            continue;
        };
        // ASTRA R03: BulletType.damageMultiplier is frozen into the projectile
        // at fire (spawn_unit_projectile / spawn_projectile). Changing rules
        // while a bullet is in flight must not change the already-emitted damage.
        let mut enemy_target_position = enemy_target_position;
        let mut impact_x = impact_x;
        let mut impact_y = impact_y;
        // Official BulletComp.update -> tileRaycast: while in flight, a bullet
        // collides with the first solid enemy building its segment crosses
        // (bullet hitSize default 4). Beams (61-64), the continuous lasers
        // (18/20) and the rail (49) are handled by their own ray logic above.
        // Official BulletComp.update -> tileRaycast collides with the first
        // solid enemy building the source->current segment crosses, even when
        // the nominal target is the core. Beams (61-64) and the rail (49) use
        // their own ray logic and are excluded. The segment is evaluated every
        // tick (position after this tick's advance); when a building is hit
        // the bullet is consumed there instead of flying on.
        if !(61..=64).contains(&bullet_id) && !is_continuous_laser(bullet_id) && bullet_id != 49 {
            let progress = {
                let total = impact_total_ticks;
                if total <= 0.0001 {
                    1.0
                } else {
                    (1.0 - remaining_ticks / total).clamp(0.0, 1.0)
                }
            };
            let bx = source_x + (impact_x - source_x) * progress;
            let by = source_y + (impact_y - source_y) * progress;
            if let Some((hit_pos, hit_x, hit_y)) = crate::network::economy::projectile_building_hit(
                world, source_x, source_y, bx, by, team, bullet_id, 4.0,
            ) {
                enemy_target_position = Some(hit_pos);
                impact_x = hit_x;
                impact_y = hit_y;
                expired = true;
            }
        }
        if team == 2
            && expired
            && homing_range > 0.0
            && !enemy_target_core
            && enemy_target_position.is_none_or(|position| !building_exists(world, position))
        {
            if let Some((position, x, y)) =
                nearest_player_building_in_range(world, impact_x, impact_y, homing_range)
            {
                enemy_target_position = Some(position);
                impact_x = x;
                impact_y = y;
            }
        }
        // Oct ForceFieldAbility(140, 4, 7000, 480, 8): enemy projectiles
        // expiring inside the field's radius are absorbed by the oct's area
        // shield (BulletComp.absorb + ForceFieldAbility.shieldConsumer). The
        // whole projectile is consumed, so its direct/splash/frag effects are
        // skipped. Continuous interval beams are absorbed on their first hit.
        if oct_force_field_absorb(world, team, impact_x, impact_y, damage)
            || quasar_force_field_absorb(world, team, impact_x, impact_y, damage)
            || tecta_shield_arc_absorb(world, team, impact_x, impact_y, damage)
        {
            world.projectiles.remove(&id);
            world_changed = true;
            continue;
        }
        if team == 2 {
            // SAP lifesteal gate: official SapBulletType heals the shooter
            // inside applyDamage, i.e. only when the beam actually hit a
            // target. Track whether this impact reached anything.
            let mut sap_hit_something = false;
            let mut sap_dealt = 0.0_f32;
            if is_continuous_laser(bullet_id) && damage > 0.0 {
                // Official ContinuousBulletType.update runs Damage.collideLine
                // over the full beam every damageInterval: EVERY unit, player
                // and building on the line takes `damage` per interval, and
                // the beam keeps piercing up to its locked target (core
                // included when the beam was fired at it).
                world_changed |= apply_enemy_shared_pierce_damage(
                    world,
                    out,
                    source_x,
                    source_y,
                    impact_x,
                    impact_y,
                    damage,
                    pierce_units,
                    status_effect,
                    status_duration,
                );
                if enemy_target_core {
                    damage_team_core(world, out, 1, damage);
                    world_changed = true;
                }
            } else if (61..=64).contains(&bullet_id) && damage > 0.0 {
                world_changed |= apply_enemy_direct_damage(
                    world,
                    out,
                    enemy_target_position,
                    enemy_target_core,
                    damage,
                );
            } else if expired && bullet_id == 49 {
                world_changed |= apply_enemy_rail_damage(
                    world,
                    out,
                    source_x,
                    source_y,
                    impact_x,
                    impact_y,
                    enemy_target_core,
                    damage,
                    0.5,
                );
            } else if expired && pierce_units > 0 && pierce_buildings > 0 {
                world_changed |= apply_enemy_shared_pierce_damage(
                    world,
                    out,
                    source_x,
                    source_y,
                    impact_x,
                    impact_y,
                    damage,
                    pierce_units.min(pierce_buildings),
                    status_effect,
                    status_duration,
                );
            } else if expired && pierce_units > 0 {
                let player_dealt = apply_enemy_pierce_player_damage(
                    world,
                    out,
                    source_x,
                    source_y,
                    impact_x,
                    impact_y,
                    damage,
                    pierce_units,
                    status_effect,
                    status_duration,
                );
                if matches!(bullet_id, 23..=25) && player_dealt > 0.0 {
                    sap_hit_something = true;
                    sap_dealt = player_dealt;
                }
                world_changed |= player_dealt > 0.0;
            } else if expired && pierce_buildings > 0 {
                world_changed |= apply_enemy_pierce_building_damage(
                    world,
                    out,
                    source_x,
                    source_y,
                    impact_x,
                    impact_y,
                    damage,
                    pierce_buildings,
                );
            }
            if expired
                && bullet_id != 49
                && !(61..=64).contains(&bullet_id)
                && !is_continuous_laser(bullet_id)
                && apply_direct_on_impact
                && damage > 0.0
            {
                let direct = apply_enemy_direct_damage(
                    world,
                    out,
                    enemy_target_position,
                    enemy_target_core,
                    damage,
                );
                if matches!(bullet_id, 23..=25) && direct {
                    sap_hit_something = true;
                }
                world_changed |= direct;
            }
            if expired && splash_damage > 0.0 && splash_radius > 0.0 {
                world_changed |= apply_enemy_splash_damage(
                    world,
                    out,
                    impact_x,
                    impact_y,
                    splash_damage,
                    splash_radius,
                    armor_multiplier,
                    status_effect,
                    status_duration,
                    projectile_building_damage_multiplier(bullet_id),
                );
            }
            if expired && bullet_id == 60 {
                world_changed |= apply_emp_bullet_effects(
                    world,
                    out,
                    team,
                    impact_x,
                    impact_y,
                    splash_radius,
                    damage,
                );
            }
            if expired && spawn_reign_frags {
                spawn_reign_fragments(
                    world, out, 2, target_id, source_x, source_y, impact_x, impact_y,
                );
            }
            if expired && bullet_id == 56 {
                spawn_cyerce_fragments(
                    world, out, 2, target_id, source_x, source_y, impact_x, impact_y,
                );
            }
            if expired && bullet_id == 28 {
                world_changed |= spawn_toxopid_fragments(
                    world, out, 2, target_id, shooter_id, source_x, source_y, impact_x, impact_y,
                );
            }
            if expired && matches!(bullet_id, 23..=25) && damage > 0.0 && sap_hit_something {
                // SAP lifesteal (spiroct/arkyid): heal the shooter by
                // sapStrength * beam damage, only when the beam actually hit
                // a target (official SapBulletType.applyDamage heals the
                // owner inside the damage application; an expiring beam that
                // touched nothing heals nothing).
                if let Some(mut source) = world.enemies.get_mut(&shooter_id) {
                    // Official SapBulletType.applyDamage: heal by
                    // min(target.health, damage) * sapStrength per target.
                    let heal = (sap_dealt * sap_strength(bullet_id))
                        .min((enemy_max_health(&source) - source.health).max(0.0));
                    source.health += heal;
                }
            }
            if expired {
                // Official BulletType.spawnUnit: launcher payloads insert
                // their MissileUnitType on the shooter's team at the impact
                // point (see spawn_projectile_unit). Quell routes through its
                // fragBullet carrier (103 -> 104 -> 53).
                world_changed |= spawn_projectile_unit(
                    world,
                    spawn_unit_frag_carrier(bullet_id),
                    team,
                    source_x,
                    source_y,
                    impact_x,
                    impact_y,
                );
                // Port model: a ballistic projectile "expires" exactly when it
                // reaches its target point, so this is the official
                // BulletType.hit() collision moment (Java only spawns chains
                // here; despawnHit is false for every vanilla weapon). A
                // target that dies mid-flight still yields a chain, which
                // vanilla would not - an accepted edge-case divergence.
                world_changed |= spawn_impact_lightning_for_bullet(
                    world, out, team, bullet_id, id, source_x, source_y, impact_x, impact_y,
                );
                world.projectiles.remove(&id);
            }
            continue;
        }
        let piercing =
            pierce_units > 0 && (enemy_target_position.is_none() || pierce_buildings > 0);
        let (hit, dead) = if piercing {
            let (pierce_x, pierce_y) = if bullet_id == 49 {
                let dx = impact_x - source_x;
                let dy = impact_y - source_y;
                let distance = dx.hypot(dy);
                if distance > 0.001 {
                    (
                        source_x + dx / distance * 500.0,
                        source_y + dy / distance * 500.0,
                    )
                } else {
                    (impact_x, impact_y)
                }
            } else {
                (impact_x, impact_y)
            };
            let changed = apply_allied_pierce_damage_for_team(
                world,
                out,
                team,
                source_x,
                source_y,
                pierce_x,
                pierce_y,
                damage,
                pierce_units,
                pierce_buildings > 0,
                if bullet_id == 49 { 0.5 } else { 1.0 },
                status_effect,
                status_duration,
            );
            (changed, false)
        } else if let Some(mut enemy) = world
            .enemies
            .get_mut(&target_id)
            .filter(|enemy| enemy.team != team)
        {
            let damage =
                apply_incoming_unit_damage_in_world(world, &enemy, damage, armor_multiplier);
            let absorbed = enemy.shield.min(damage);
            enemy.shield -= absorbed;
            enemy.health -= damage - absorbed;
            if status_effect >= 0
                && status_duration > 0.0
                && !unit_immune_to_status(enemy.unit_type, status_effect)
            {
                // A6: statuses go into the StatusEntry collection so they
                // stack with active statuses and survive tick_statuses
                // (simulate_enemy_statuses reads the collection).
                crate::network::units::StatusContainer::apply_status(
                    &mut *enemy,
                    status_effect,
                    status_duration,
                );
            }
            (true, enemy.health <= 0.0)
        } else if let Some(position) = enemy_target_position {
            // Beam/bomb bullets (speed 0, unit-fired) expire at their max
            // travel; only damage/heal the targeted building when the impact
            // point actually reaches it (official maxRange/length semantics).
            let reached = if matches!(bullet_id, 17 | 18 | 20 | 23 | 24 | 25 | 27 | 36 | 40) {
                let building_x = (position >> 16) as i16 as f32 * 8.0;
                let building_y = position as i16 as f32 * 8.0;
                (building_x - impact_x).hypot(building_y - impact_y) <= 1.0
            } else {
                true
            };
            let changed = if reached
                && effective_building_team(world, position) == team
                && projectile_direct_heal_percent(bullet_id).is_some()
            {
                let heal_percent = projectile_direct_heal_percent(bullet_id).unwrap();
                if let Some(health) =
                    heal_building_for_team(world, position, team, heal_percent, 0.0)
                {
                    if let Ok(frame) = encode_build_health_update_frame(&[(position, health)]) {
                        out.broadcast(frame);
                    }
                    true
                } else {
                    false
                }
            } else if reached {
                if let Some((destroyed, health)) = damage_building(world, position, damage) {
                    let _ =
                        maybe_surge_wall_lightning(world, out, position, id, source_x, source_y);
                    if destroyed {
                        if let Ok(frame) = encode_build_destroyed_frame(position) {
                            out.broadcast(frame);
                        }
                    } else if let Ok(frame) =
                        encode_build_health_update_frame(&[(position, health)])
                    {
                        out.broadcast(frame);
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            (changed, false)
        } else {
            (false, false)
        };
        world_changed |= hit;
        if expired && splash_damage > 0.0 && splash_radius > 0.0 {
            world_changed |= apply_allied_splash_damage_for_team(
                world,
                out,
                team,
                impact_x,
                impact_y,
                splash_damage,
                splash_radius,
                armor_multiplier,
                status_effect,
                status_duration,
                projectile_building_damage_multiplier(bullet_id),
            );
        }
        if expired && splash_radius > 0.0 {
            if let Some(heal_percent) = projectile_splash_heal_percent(bullet_id) {
                world_changed |= apply_splash_building_heal_for_team(
                    world,
                    out,
                    team,
                    impact_x,
                    impact_y,
                    splash_radius,
                    heal_percent,
                );
            }
        }
        if expired && bullet_id == 60 {
            world_changed |= apply_emp_bullet_effects(
                world,
                out,
                team,
                impact_x,
                impact_y,
                splash_radius,
                damage,
            );
        }
        if expired && spawn_reign_frags {
            spawn_reign_fragments(
                world, out, team, target_id, source_x, source_y, impact_x, impact_y,
            );
        }
        if expired && bullet_id == 56 {
            spawn_cyerce_fragments(
                world, out, team, target_id, source_x, source_y, impact_x, impact_y,
            );
        }
        if expired && bullet_id == 28 {
            world_changed |= spawn_toxopid_fragments(
                world, out, team, target_id, shooter_id, source_x, source_y, impact_x, impact_y,
            );
        }
        if expired && matches!(bullet_id, 23..=25) && damage > 0.0 {
            // Allied SAP lifesteal: heal the same-team shooter.
            // shooter by sapStrength * beam damage, mirroring the enemy side.
            if let Some(mut shooter) = world.enemies.get_mut(&shooter_id) {
                if shooter.team == team {
                    let maximum = enemy_max_health(&shooter);
                    let heal =
                        (damage * sap_strength(bullet_id)).min((maximum - shooter.health).max(0.0));
                    shooter.health += heal;
                }
            }
        }
        if expired {
            // Official BulletType.spawnUnit: launcher payloads insert their
            // MissileUnitType on the shooter's team at the impact point.
            // Quell routes through its fragBullet carrier (103 -> 104 -> 53).
            world_changed |= spawn_projectile_unit(
                world,
                spawn_unit_frag_carrier(bullet_id),
                team,
                source_x,
                source_y,
                impact_x,
                impact_y,
            );
            world_changed |= spawn_impact_lightning_for_bullet(
                world, out, team, bullet_id, id, source_x, source_y, impact_x, impact_y,
            );
        }
        if expired || dead || !world.enemies.contains_key(&target_id) {
            world.projectiles.remove(&id);
        }
        if dead {
            kill_enemy(world, out, target_id);
        }
    }
    world_changed
}
