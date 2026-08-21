#![allow(dead_code)]

use parking_lot::RwLock;
use rayon::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct UnitEntity {
    pub id: u32,
    pub unit_type: u16,
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub health: f32,
    pub max_health: f32,
    pub team: u8,
}

#[derive(Clone, Debug)]
pub struct BulletEntity {
    pub id: u32,
    pub bullet_type: u16,
    pub x: f32,
    pub y: f32,
    pub vel_x: f32,
    pub vel_y: f32,
    pub lifetime: f32,
    pub team: u8,
}

pub struct EntityManager {
    next_id: AtomicU32,
    pub units: Arc<RwLock<Vec<UnitEntity>>>,
    pub bullets: Arc<RwLock<Vec<BulletEntity>>>,
}

impl Default for EntityManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EntityManager {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU32::new(1),
            units: Arc::new(RwLock::new(Vec::new())),
            bullets: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn alloc_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Runs the small, state-only unit update supported by this legacy entity
    /// store.
    ///
    /// The authoritative server simulation keeps unit velocity and AI state in
    /// [`crate::network::world::EnemyUnit`].  `UnitEntity` deliberately has no
    /// velocity field, so inventing movement here (the old `x += 0.05 * delta`
    /// placeholder) would move every unit in the wrong direction and would
    /// diverge from both the network simulation and Java's
    /// `UnitEntity.update()` (`move(vel * Time.delta)`).  This slice therefore
    /// only enforces the health/liveness contract that this type can represent.
    /// Invalid entities are discarded after the parallel pass, rather than
    /// being allowed to poison later ticks with NaN coordinates or health.
    pub fn update_units_parallel(&self, _delta: f32) {
        let mut units_guard = self.units.write();
        units_guard.par_iter_mut().for_each(|unit| {
            if !unit.max_health.is_finite() || unit.max_health <= 0.0 || !unit.health.is_finite() {
                unit.health = 0.0;
            } else {
                // Java Healthc implementations keep health in [0, maxHealth].
                unit.health = unit.health.clamp(0.0, unit.max_health);
            }
        });

        // A dead unit is removed from Groups.unit by the official entity
        // lifecycle.  Do the equivalent while also rejecting malformed state.
        units_guard.retain(|unit| {
            unit.health > 0.0
                && unit.max_health.is_finite()
                && unit.max_health > 0.0
                && unit.x.is_finite()
                && unit.y.is_finite()
                && unit.rotation.is_finite()
        });
    }

    /// Parallel multi-threaded physics tick for all bullets using Rayon.
    ///
    /// This mirrors the representable part of Java `Bullet.update()`: advance
    /// by velocity and age the bullet, then remove bullets whose lifetime has
    /// elapsed.  A malformed frame delta is treated as zero and malformed
    /// bullets are retired instead of producing NaN state that can leak into
    /// snapshots or future physics passes.
    pub fn update_bullets_parallel(&self, delta: f32) {
        let delta = sanitized_delta(delta);
        let mut bullets_guard = self.bullets.write();

        bullets_guard.par_iter_mut().for_each(|bullet| {
            if bullet.x.is_finite()
                && bullet.y.is_finite()
                && bullet.vel_x.is_finite()
                && bullet.vel_y.is_finite()
            {
                bullet.x += bullet.vel_x * delta;
                bullet.y += bullet.vel_y * delta;
                if bullet.lifetime.is_finite() {
                    bullet.lifetime -= delta;
                }
            }
        });

        // `lifetime == +infinity` is a valid way to represent a persistent
        // effect; NaN, non-positive lifetimes, and non-finite position/velocity
        // state are not valid live bullets.
        bullets_guard.retain(|bullet| {
            bullet.lifetime > 0.0
                && !bullet.lifetime.is_nan()
                && bullet.x.is_finite()
                && bullet.y.is_finite()
                && bullet.vel_x.is_finite()
                && bullet.vel_y.is_finite()
        });
    }
}

/// Java's Time.delta is never negative.  Treat an invalid caller-provided
/// delta as no elapsed time instead of allowing NaN/negative time to corrupt
/// projectile position and lifetime.
fn sanitized_delta(delta: f32) -> f32 {
    if delta.is_finite() && delta > 0.0 {
        delta
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{BulletEntity, EntityManager, UnitEntity};

    fn unit(id: u32, health: f32, max_health: f32) -> UnitEntity {
        UnitEntity {
            id,
            unit_type: 0,
            x: 10.0,
            y: 20.0,
            rotation: 90.0,
            health,
            max_health,
            team: 1,
        }
    }

    #[test]
    fn unit_tick_does_not_invent_motion_and_retires_dead_state() {
        let entities = EntityManager::new();
        entities.units.write().extend([
            unit(1, 125.0, 100.0),
            unit(2, 0.0, 100.0),
            unit(3, f32::NAN, 100.0),
            unit(4, 100.0, f32::NAN),
        ]);

        entities.update_units_parallel(6.0);

        let units = entities.units.read();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, 1);
        assert_eq!(units[0].health, 100.0);
        // UnitEntity has no velocity; a fixed positive-x drift is not a valid
        // substitute for Java's velocity-driven UnitEntity.update().
        assert_eq!(units[0].x, 10.0);
        assert_eq!(units[0].y, 20.0);
    }

    #[test]
    fn bullet_tick_integrates_velocity_and_expires() {
        let entities = EntityManager::new();
        entities.bullets.write().push(BulletEntity {
            id: 1,
            bullet_type: 0,
            x: 1.0,
            y: -2.0,
            vel_x: 4.0,
            vel_y: -8.0,
            lifetime: 2.0,
            team: 1,
        });

        entities.update_bullets_parallel(0.5);
        {
            let bullets = entities.bullets.read();
            assert_eq!(bullets.len(), 1);
            assert!((bullets[0].x - 3.0).abs() < f32::EPSILON);
            assert!((bullets[0].y + 6.0).abs() < f32::EPSILON);
            assert!((bullets[0].lifetime - 1.5).abs() < f32::EPSILON);
        }

        entities.update_bullets_parallel(1.5);
        assert!(entities.bullets.read().is_empty());
    }

    #[test]
    fn bullet_tick_rejects_invalid_delta_and_entity_state() {
        let entities = EntityManager::new();
        entities.bullets.write().extend([
            BulletEntity {
                id: 1,
                bullet_type: 0,
                x: 1.0,
                y: 2.0,
                vel_x: 3.0,
                vel_y: 4.0,
                lifetime: 2.0,
                team: 1,
            },
            BulletEntity {
                id: 2,
                bullet_type: 0,
                x: f32::NAN,
                y: 0.0,
                vel_x: 1.0,
                vel_y: 0.0,
                lifetime: f32::INFINITY,
                team: 1,
            },
        ]);

        entities.update_bullets_parallel(f32::NAN);
        entities.update_bullets_parallel(-1.0);

        let bullets = entities.bullets.read();
        assert_eq!(bullets.len(), 1);
        assert_eq!(bullets[0].id, 1);
        assert_eq!(bullets[0].x, 1.0);
        assert_eq!(bullets[0].y, 2.0);
        assert_eq!(bullets[0].lifetime, 2.0);
    }
}
