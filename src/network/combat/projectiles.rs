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
    pub(crate) status_effect: i16,
    pub(crate) status_duration: f32,
    pub(crate) pierce_units: u8,
    pub(crate) pierce_buildings: u8,
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
    homing_range: 50.0,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    homing_range: 50.0,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    homing_range: 50.0,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    homing_range: 50.0,
    status_effect: 18,
    status_duration: 60.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    homing_range: 50.0,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: u8::MAX,
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
    status_effect: 18,
    status_duration: 60.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
};

// Deviation: the official vela-weapon is a ContinuousLaserBulletType that
// deals 35 damage every damageInterval (5 ticks) along a 180-length beam for
// its 160-tick lifetime; the server collapses the beam into a single
// piercing impact at expiry (same approximation as corvus). Its healPercent 1
// + collidesTeam heal side is applied on impact for team-1 projectiles.
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
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
    status_effect: 8,
    status_duration: 120.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: 9,
    status_duration: 180.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
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
    status_effect: 9,
    status_duration: 180.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
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
    status_effect: 9,
    status_duration: 180.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
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
    status_effect: 9,
    status_duration: 600.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
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
    status_effect: 9,
    status_duration: 600.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: u8::MAX,
    pierce_buildings: 0,
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
    status_effect: 18,
    status_duration: 60.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    homing_range: 0.0,
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
};

// Deviation: poly missile homing (homingPower 0.08) is not modeled; healing
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    status_effect: -1,
    status_duration: 0.0,
    pierce_units: 0,
    pierce_buildings: 0,
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
    match unit_type {
        // Legacy dual-mount units now also use this primary/secondary table:
        // spiroct (sap + sap mount), toxopid (shrapnel + cannon), mega
        // (heal-weapon-mount pair). Values match src/game/unit_weapons.tsv.
        12 => Some(((14.0, SPIROCT_SAP), (18.0, SPIROCT_SAP_MOUNT))),
        14 => Some(((30.0, TOXOPID_SHRAPNEL), (210.0, TOXOPID_CANNON))),
        22 => Some(((24.0, MEGA_HEAL_A), (15.0, MEGA_HEAL_B))),
        25 => Some(((13.0, RISSO_GUN), (25.0, RISSO_MISSILE))),
        26 => Some(((10.0, MINKE_GUN), (30.0, MINKE_ARTILLERY))),
        27 => Some(((65.0, BRYDE_ARTILLERY), (20.0, BRYDE_MISSILES))),
        28 => Some(((45.0, SEI_LAUNCHER), (60.0, SEI_CANNON))),
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
            homing_range: 50.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
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
            homing_range: 50.0,
            status_effect: 1,
            status_duration: 300.0,
            pierce_units: 2,
            pierce_buildings: 2,
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
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
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
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 10,
            pierce_buildings: 0,
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
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
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
            status_effect: 1,
            status_duration: 240.0,
            pierce_units: u8::MAX,
            pierce_buildings: 0,
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
            homing_range: 60.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
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
            status_effect: 10,
            status_duration: 480.0,
            pierce_units: 0,
            pierce_buildings: 0,
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
        22 => Some(MEGA_HEAL_A), // heal-weapon-mount: bullet 38, reload 24 (heal-only bolt, 5.5% max)
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
        shot_index,
        1,
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
    shot_index: u8,
    team: u8,
) -> i32 {
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
    world.projectiles.insert(
        projectile_id,
        Projectile {
            target_id,
            shooter_id,
            team,
            bullet_id: volley.bullet_id,
            damage: volley.direct_damage,
            splash_damage: volley.splash_damage,
            splash_radius: volley.splash_radius,
            status_effect: volley.status_effect,
            status_duration: volley.status_duration,
            pierce_units: volley.pierce_units,
            pierce_buildings: volley.pierce_buildings,
            spawn_reign_frags: volley.bullet_id == 12,
            homing_range: volley.homing_range,
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
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    if let Ok(payload) = encode_create_bullet_payload(
        volley.bullet_id,
        team,
        source_x,
        source_y,
        angle,
        volley.direct_damage,
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
    shot_index: u8,
) -> i32 {
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
    world.projectiles.insert(
        projectile_id,
        Projectile {
            target_id: source_id,
            shooter_id: source_id,
            team: 2,
            bullet_id: volley.bullet_id,
            damage: volley.direct_damage,
            splash_damage: volley.splash_damage,
            splash_radius: volley.splash_radius,
            status_effect: volley.status_effect,
            status_duration: volley.status_duration,
            pierce_units: volley.pierce_units,
            pierce_buildings: volley.pierce_buildings,
            spawn_reign_frags: volley.bullet_id == 12,
            homing_range: volley.homing_range,
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
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    if let Ok(payload) = encode_create_bullet_payload(
        volley.bullet_id,
        2,
        source_x,
        source_y,
        angle,
        volley.direct_damage,
        velocity_scale,
        1.0,
    ) {
        if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
    projectile_id
}

pub(crate) fn projectile_armor_multiplier(bullet_id: i16) -> f32 {
    match bullet_id {
        129 => 4.0, // Lancer LaserBulletType.
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
    let continuous = projectile.damage_interval.is_some();
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
        let impact = if let Some(mut projectile) = world.projectiles.get_mut(&id) {
            if let Some(interval) = projectile.damage_interval {
                let elapsed = delta_ticks.max(0.0).min(projectile.remaining_ticks);
                projectile.remaining_ticks -= elapsed;
                projectile.damage_timer += elapsed;
                let hits = (projectile.damage_timer / interval).floor();
                projectile.damage_timer -= hits * interval;
                let expired = projectile.remaining_ticks <= 0.0;
                (hits > 0.0 || expired).then_some((
                    projectile.target_id,
                    projectile.shooter_id,
                    projectile.team,
                    projectile.bullet_id,
                    projectile.damage * hits,
                    projectile.splash_damage,
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
                (projectile.remaining_ticks <= 0.0).then_some((
                    projectile.target_id,
                    projectile.shooter_id,
                    projectile.team,
                    projectile.bullet_id,
                    projectile.damage,
                    projectile.splash_damage,
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
            source_position,
            mut expired,
        )) = impact
        else {
            continue;
        };
        // A7: official BulletType.damageMultiplier (JAR offsets 0-61) — a
        // bullet fired by a UNIT deals `base * unit.damageMultiplier() *
        // Rules.unitDamage(team)`, with `Rules.unitDamage(team) =
        // unitDamageMultiplier * TeamRule.unitDamageMultiplier`
        // (Rules.unitDamage JAR offsets 0-16). The port stores the base
        // damage and applies the ATTACKER's TeamRule at impact; bullets
        // fired by buildings (turret shots carry `source_position: Some`)
        // would use Rules.blockDamage instead, whose team fields are not
        // modeled (A7) — they keep the base damage. The global
        // Rules.unitDamageMultiplier is also unmodeled (see report).
        let unit_damage_rule = world
            .wave_rules
            .read()
            .team_rule(team)
            .unit_damage_multiplier;
        let (damage, splash_damage) = if source_position.is_none() {
            (damage * unit_damage_rule, splash_damage * unit_damage_rule)
        } else {
            (damage, splash_damage)
        };
        let mut enemy_target_position = enemy_target_position;
        let mut impact_x = impact_x;
        let mut impact_y = impact_y;
        // Official BulletComp.update -> tileRaycast: while in flight, a bullet
        // collides with the first solid enemy building its segment crosses
        // (bullet hitSize default 4). Beams (61-64) and the rail (49) are
        // handled by their own ray logic above.
        // Official BulletComp.update -> tileRaycast collides with the first
        // solid enemy building the source->current segment crosses, even when
        // the nominal target is the core. Beams (61-64) and the rail (49) use
        // their own ray logic and are excluded. The segment is evaluated every
        // tick (position after this tick's advance); when a building is hit
        // the bullet is consumed there instead of flying on.
        if !(61..=64).contains(&bullet_id) && bullet_id != 49 {
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
            if (61..=64).contains(&bullet_id) && damage > 0.0 {
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
                world_changed |= apply_enemy_pierce_player_damage(
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
                && apply_direct_on_impact
                && damage > 0.0
            {
                world_changed |= apply_enemy_direct_damage(
                    world,
                    out,
                    enemy_target_position,
                    enemy_target_core,
                    damage,
                );
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
            if expired && matches!(bullet_id, 23..=25) && damage > 0.0 {
                // SAP lifesteal (spiroct/arkyid): heal the shooter by
                // sapStrength * beam damage. Approximation: heals on beam
                // expiry even when the beam hit nothing.
                if let Some(mut source) = world.enemies.get_mut(&shooter_id) {
                    let heal = (damage * sap_strength(bullet_id))
                        .min((enemy_max_health(&source) - source.health).max(0.0));
                    source.health += heal;
                }
            }
            if expired {
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
            let damage = apply_incoming_unit_damage(&enemy, damage, armor_multiplier);
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
        if expired || dead || !world.enemies.contains_key(&target_id) {
            world.projectiles.remove(&id);
        }
        if dead {
            kill_enemy(world, out, target_id);
        }
    }
    world_changed
}
