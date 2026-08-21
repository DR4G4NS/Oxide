#![allow(dead_code)]

use crate::engine::entities::EntityManager;
use crate::engine::world::World;
use crate::state::game_state::GameState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

pub struct TickEngine {
    pub target_tps: u32,
    pub total_ticks: Arc<AtomicU64>,
    pub entities: Arc<EntityManager>,
    pub world: Arc<RwLock<Option<World>>>,
    pub state: GameState,
}

use parking_lot::RwLock;

impl TickEngine {
    pub fn new(target_tps: u32, state: GameState) -> Self {
        Self {
            target_tps: target_tps.max(1),
            total_ticks: Arc::new(AtomicU64::new(0)),
            entities: Arc::new(EntityManager::new()),
            world: Arc::new(RwLock::new(None)),
            state,
        }
    }

    /// Applies one entity tick synchronously.
    ///
    /// This is intentionally separate from the Tokio loop so the small legacy
    /// entity slice can be exercised deterministically in tests and by callers
    /// that already own a simulation thread.  The caller supplies Java-style
    /// `Time.delta` units (60 ticks per second).
    pub fn tick_once(&self, delta: f32) {
        self.entities.update_units_parallel(delta);
        self.entities.update_bullets_parallel(delta);
        self.total_ticks.fetch_add(1, Ordering::Relaxed);
    }

    /// Spawns the background target-TPS game logic loop.
    pub fn start_loop(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                "Mindustry {} TPS engine tick loop started.",
                self.target_tps
            );
            let tick_duration = Duration::from_secs_f64(1.0 / self.target_tps as f64);
            let delta = tick_delta_for_tps(self.target_tps);
            let mut next_tick = Instant::now();

            loop {
                if self.state.is_active() {
                    // Rayon work must stay off the Tokio worker pool.  The
                    // blocking closure owns only the engine Arc and acquires
                    // each entity lock for one short, ordered pass; it never
                    // holds a lock across an await.
                    let engine = self.clone();
                    if tokio::task::spawn_blocking(move || engine.tick_once(delta))
                        .await
                        .is_err()
                    {
                        // Runtime shutdown/abort: do not spin a replacement
                        // task after the blocking worker has gone away.
                        break;
                    }
                }

                next_tick += tick_duration;
                let now = Instant::now();
                if next_tick > now {
                    tokio::time::sleep(next_tick.duration_since(now)).await;
                } else {
                    // Tick catch-up threshold.  As in the current world
                    // simulation, a missed wakeup does not replay stale ticks.
                    next_tick = now;
                }
            }
        })
    }
}

/// Mindustry advances simulation time in 60 game ticks per real-time second.
/// A loop configured below/above 60 TPS therefore uses a correspondingly
/// larger/smaller `Time.delta`; a fixed `1.0` would make a 10 TPS loop run six
/// times slower than the authoritative server.
fn tick_delta_for_tps(target_tps: u32) -> f32 {
    60.0 / target_tps.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::{tick_delta_for_tps, TickEngine};
    use crate::engine::entities::BulletEntity;
    use crate::state::game_state::GameState;

    #[test]
    fn tick_delta_matches_java_game_tick_rate() {
        assert_eq!(tick_delta_for_tps(60), 1.0);
        assert_eq!(tick_delta_for_tps(10), 6.0);
        assert_eq!(tick_delta_for_tps(120), 0.5);
        // TickEngine::new clamps zero, and the helper is safe on its own too.
        assert_eq!(tick_delta_for_tps(0), 60.0);
    }

    #[test]
    fn tick_once_updates_entities_and_counts_one_tick() {
        let engine = TickEngine::new(10, GameState::new());
        engine.entities.bullets.write().push(BulletEntity {
            id: 1,
            bullet_type: 0,
            x: 0.0,
            y: 0.0,
            vel_x: 2.0,
            vel_y: -1.0,
            lifetime: 10.0,
            team: 1,
        });

        engine.tick_once(6.0);

        assert_eq!(
            engine
                .total_ticks
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let bullets = engine.entities.bullets.read();
        assert_eq!(bullets.len(), 1);
        assert_eq!(bullets[0].x, 12.0);
        assert_eq!(bullets[0].y, -6.0);
        assert_eq!(bullets[0].lifetime, 4.0);
    }
}
