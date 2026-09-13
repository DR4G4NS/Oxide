//! Authoritative lightning chains (official `Lightning.create` /
//! `LightningBulletType`).
//!
//! Vanilla semantics (core/src/mindustry/entities/Lightning.java, v159.7
//! oracle): a bolt walks `length / 2` segments from the impact point. Each
//! segment creates a `lightningType` bullet at the current point (that bullet
//! deals the bolt damage) and then jumps to the FURTHEST enemy within a 30 u
//! box that was not hit before in the same chain (max 8 distinct units); when
//! no unit is found, the rotation strays by a random ±20° and the chain
//! advances 15 u.
//!
//! The server emits one CreateBullet frame per segment using the FREE wire ids
//! 1 (`Bullets.damageLightning`, hits air + ground), 2
//! (`damageLightningGround`) and 3 (`damageLightningAir`), and applies the
//! per-segment damage to the chained enemy directly.

use super::*;

/// Free wire id for `Bullets.damageLightning` (hits air and ground).
pub(crate) const LIGHTNING_BULLET_ALL: i16 = 1;
/// Free wire id for `Bullets.damageLightningGround` (collidesAir = false).
pub(crate) const LIGHTNING_BULLET_GROUND: i16 = 2;
/// Free wire id for `Bullets.damageLightningAir` (collidesGround = false).
pub(crate) const LIGHTNING_BULLET_AIR: i16 = 3;

/// Official `Lightning.hitRange`.
const CHAIN_HIT_RANGE: f32 = 30.0;
/// Official `Lightning.maxChain`: distinct units one chain may visit.
const MAX_CHAIN_UNITS: usize = 8;
/// Official stray applied when no unit is found: `random.range(20f)`.
const SEGMENT_STRAY_DEGREES: f32 = 20.0;

/// Which units a lightning chain may visit (official collidesAir/collidesGround).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LightningTarget {
    All,
    Ground,
    Air,
}

/// Per-bolt lightning parameters extracted from the official unit table.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LightningSpec {
    /// Official `lightning`: number of separate roots spawned at impact.
    pub(crate) roots: u8,
    /// Official `lightningLength`; each chain walks `length / 2` segments.
    pub(crate) length: u8,
    /// Official `lightningLengthRand` (inclusive extra length).
    pub(crate) length_rand: u8,
    /// Damage dealt by EVERY created segment bullet (official `lightningDamage`
    /// when >= 0, otherwise the host bullet damage).
    pub(crate) damage: f32,
    pub(crate) target: LightningTarget,
}

/// Lightning parameters for bullets that spawn chains on impact.
///
/// Oracle: UnitTypes.java (v159.7 tree).
/// - scepter-weapon/scepter-mount (9/10): `lightning = 2, lightningLength = 6,
///   lightningDamage = 20` (BasicBulletType, default full-circle cone).
/// - pulsar heal-shotgun (15): the weapon IS a `LightningBulletType`
///   (`damage = 15, lightningLength = 8, lightningLengthRand = 7`), so its
///   single root spawns at fire time; the headless model keeps the chain at
///   impact instead (documented deviation).
/// - arkyid large-purple-mount (26): artillery bullet with `lightning = 3,
///   lightningLength = 10`; `lightningDamage` defaults to the bullet damage.
pub(crate) fn lightning_spec(bullet_id: i16) -> Option<LightningSpec> {
    match bullet_id {
        9 | 10 => Some(LightningSpec {
            roots: 2,
            length: 6,
            length_rand: 0,
            damage: 20.0,
            target: LightningTarget::All,
        }),
        15 => Some(LightningSpec {
            roots: 1,
            length: 8,
            length_rand: 7,
            damage: 15.0,
            target: LightningTarget::All,
        }),
        26 => Some(LightningSpec {
            roots: 3,
            length: 10,
            length_rand: 0,
            damage: 12.0,
            target: LightningTarget::All,
        }),
        _ => None,
    }
}

/// Small deterministic splitmix64 generator. Lightning is random in vanilla;
/// the server needs reproducible simulations, so every chain derives its
/// generator from the projectile id plus the root index.
pub(crate) struct DetRand(u64);

impl DetRand {
    pub(crate) fn new(seed: u64) -> Self {
        DetRand(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform `[0, 1)`.
    pub(crate) fn unit_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Uniform `[-range, range]` (official `random.range(20f)` shape).
    fn symmetric_range(&mut self, range: f32) -> f32 {
        (self.unit_f32() * 2.0 - 1.0) * range
    }

    /// Inclusive integer `[0, bound]` (official `Mathf.random(int)` shape).
    fn inclusive(&mut self, bound: u8) -> u8 {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % (u64::from(bound) + 1)) as u8
        }
    }
}

fn chain_bullet_id(target: LightningTarget) -> i16 {
    match target {
        LightningTarget::All => LIGHTNING_BULLET_ALL,
        LightningTarget::Ground => LIGHTNING_BULLET_GROUND,
        LightningTarget::Air => LIGHTNING_BULLET_AIR,
    }
}

fn unit_matches_target(unit_type: i16, target: LightningTarget) -> bool {
    let flying = crate::game::content::unit_movement(unit_type).flying;
    match target {
        LightningTarget::All => true,
        LightningTarget::Ground => !flying,
        LightningTarget::Air => flying,
    }
}

/// Furthest un-hit enemy inside the official 30 u box around `(x, y)`.
/// Ties break on the lowest unit id so chains stay deterministic.
fn furthest_chain_enemy(
    world: &DynamicWorld,
    team: u8,
    x: f32,
    y: f32,
    hit: &[i32],
    target: LightningTarget,
) -> Option<(i32, f32, f32)> {
    if hit.len() >= MAX_CHAIN_UNITS {
        return None;
    }
    let mut best: Option<(i32, f32, f32, f32)> = None;
    for entry in world.enemies.iter() {
        let unit = entry.value();
        if unit.team == team || hit.contains(&unit.id) {
            continue;
        }
        if !unit_matches_target(unit.unit_type, target) {
            continue;
        }
        let dx = unit.x - x;
        let dy = unit.y - y;
        if dx.abs() > CHAIN_HIT_RANGE || dy.abs() > CHAIN_HIT_RANGE {
            continue;
        }
        let distance = dx.hypot(dy);
        let better = match best {
            None => true,
            Some((_, _, _, best_distance)) => {
                distance > best_distance
                    || (distance - best_distance).abs() < 0.001 && unit.id < best.unwrap().0
            }
        };
        if better {
            best = Some((unit.id, unit.x, unit.y, distance));
        }
    }
    best.map(|(id, ux, uy, _)| (id, ux, uy))
}

/// Apply one segment's damage to the chained enemy (shield first, then
/// armoured health, mirroring the allied direct-damage path).
fn apply_chain_damage(world: &DynamicWorld, unit_id: i32, damage: f32) -> bool {
    let mut changed = false;
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        let applied = super::apply_incoming_unit_damage_in_world(world, &unit, damage, 1.0);
        let absorbed = unit.shield.min(applied);
        unit.shield -= absorbed;
        unit.health -= applied - absorbed;
        changed = true;
    }
    changed
}

fn emit_chain_segment(
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    x: f32,
    y: f32,
    angle: f32,
    damage: f32,
    target: LightningTarget,
) {
    if let Ok(payload) =
        encode_create_bullet_payload(chain_bullet_id(target), team, x, y, angle, damage, 1.0, 1.0)
    {
        if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
}

/// Spawn `spec.roots` authoritative lightning chains from an impact point.
/// Returns whether any enemy took damage (world changed).
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_impact_lightning(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    spec: LightningSpec,
    projectile_id: i32,
    source_x: f32,
    source_y: f32,
    impact_x: f32,
    impact_y: f32,
) -> bool {
    let travel_angle = (impact_y - source_y)
        .atan2(impact_x - source_x)
        .to_degrees();
    let mut world_changed = false;
    for root in 0..u64::from(spec.roots) {
        // Deterministic per (projectile, root): identical inputs reproduce
        // identical chains across ticks and restarts.
        let mut rng = DetRand::new((projectile_id as u64) << 8 | root.wrapping_add(1));
        let mut x = impact_x;
        let mut y = impact_y;
        // Official BulletType.createLightning call: rotation + range(cone/2),
        // default lightningCone = 360.
        let mut root_angle = travel_angle + rng.symmetric_range(180.0);
        let segments = (spec.length + rng.inclusive(spec.length_rand)) / 2;
        let mut hit: Vec<i32> = Vec::new();
        for _ in 0..segments {
            emit_chain_segment(out, team, x, y, root_angle, spec.damage, spec.target);
            if let Some((unit_id, unit_x, unit_y)) =
                furthest_chain_enemy(world, team, x, y, &hit, spec.target)
            {
                hit.push(unit_id);
                world_changed |= apply_chain_damage(world, unit_id, spec.damage);
                x = unit_x;
                y = unit_y;
            } else {
                root_angle += rng.symmetric_range(SEGMENT_STRAY_DEGREES);
                let radians = root_angle.to_radians();
                x += radians.cos() * (CHAIN_HIT_RANGE / 2.0);
                y += radians.sin() * (CHAIN_HIT_RANGE / 2.0);
            }
        }
    }
    world_changed
}
