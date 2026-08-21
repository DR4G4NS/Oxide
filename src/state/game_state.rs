#![allow(dead_code)]

use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Sandbox,
    Attack,
    Pvp,
}

/// Official `GameStats` (GameStats.java): counters persisted in the save
/// meta ("stats" JSON) and shown on the game-over screen.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct GameStats {
    /// Enemy (red team) units destroyed.
    pub enemy_units_destroyed: u32,
    /// Total waves lasted.
    pub waves_lasted: u32,
    /// Friendly buildings fully built.
    pub buildings_built: u32,
    /// Friendly buildings fully deconstructed.
    pub buildings_deconstructed: u32,
    /// Friendly buildings destroyed.
    pub buildings_destroyed: u32,
    /// Total units created by any means.
    pub units_created: u32,
    /// Record of blocks placed by count (block id -> count).
    pub placed_block_count: Vec<(i16, u32)>,
    /// Record of enemy blocks destroyed by count (block id -> count).
    pub destroyed_block_count: Vec<(i16, u32)>,
    /// Record of items that entered the core through transport blocks.
    pub core_item_count: Vec<(i16, u32)>,
}

impl GameStats {
    /// Bump a block counter, merging by block id (official ObjectIntMap).
    pub fn bump_block(counter: &mut Vec<(i16, u32)>, block: i16) {
        match counter.iter_mut().find(|(id, _)| *id == block) {
            Some((_, count)) => *count += 1,
            None => counter.push((block, 1)),
        }
    }
}

#[derive(Clone)]
pub struct GameState {
    pub is_hosting: Arc<AtomicBool>,
    pub is_paused: Arc<AtomicBool>,
    /// Rules.pvpAutoPause (158.1 default true): in PvP the game pauses while
    /// fewer than two teams have players connected.
    pub pvp_auto_pause: Arc<AtomicBool>,
    /// Whether the current pause was applied by the PvP auto-pause logic (so
    /// a manual `pause` is never overridden by the auto-resume).
    pub pvp_auto_paused: Arc<AtomicBool>,
    pub wave: Arc<AtomicU32>,
    pub wave_time: Arc<RwLock<f32>>,
    pub simulation_time: Arc<RwLock<f32>>,
    pub enemies_count: Arc<AtomicU32>,
    pub game_over: Arc<AtomicBool>,
    pub core_health: Arc<RwLock<f32>>,
    pub mode: Arc<RwLock<GameMode>>,
    pub map_name: Arc<RwLock<String>>,
    pub players_count: Arc<AtomicU32>,
    pub core_items: Arc<RwLock<Vec<i32>>>,
    /// Per-team core item inventories (official `TeamData.items` — each
    /// team's core storage, synced across that team's cores). Team 1's
    /// canonical store REMAINS `core_items` (the legacy field used by ~56
    /// call sites); `team_items` lazily holds every OTHER team's inventory.
    /// In survival/attack there is a single player team (1) and `team_items`
    /// stays empty, so the existing `core_items` behavior is unchanged.
    pub team_items: Arc<DashMap<u8, Vec<i32>>>,
    /// Official GameStats (game-over statistics), persisted in the save.
    pub game_stats: Arc<RwLock<GameStats>>,
    /// Rules.infiniteResources (official ConstructBlock: building costs
    /// nothing). Mirrors `WaveRules.infinite_resources` for cheap access.
    pub infinite_resources: Arc<AtomicBool>,
    /// P0-7 strict mode: unsupported content (logic statements, spawn group
    /// units) fails with structured diagnostics and location instead of
    /// degrading silently to NoOp / being skipped. Off by default so vanilla
    /// maps keep loading; strict hosting rejects the map/program at load.
    pub strict_mode: Arc<AtomicBool>,
    /// P2: world-loop metrics (SOL-AUDIT P2: tick duration, scheduler lag
    /// and dropped outbound frames). The separate `TickEngine` measures its
    /// own fixed loop; these counters track the authoritative game loop.
    /// Number of world-loop iterations processed.
    pub world_ticks: Arc<AtomicU64>,
    /// Duration of the most recent world-loop iteration, in microseconds.
    pub world_tick_us: Arc<AtomicU64>,
    /// Round 74d develop mode: periodic runtime diagnostics dump.
    pub develop_mode: Arc<AtomicBool>,
    /// Milliseconds between develop dumps.
    pub develop_interval_ms: Arc<AtomicU64>,
    /// World re-host events (host_map calls) since startup — the develop
    /// dump reports this to catch the "server keeps restarting" class of
    /// bugs (map rotation loops, repeated re-streams).
    pub host_map_events: Arc<AtomicU64>,
    /// Round 74d develop: microseconds of the last synchronous world
    /// snapshot build (the part of a save that runs on the tick).
    pub save_build_us: Arc<AtomicU64>,
    /// Round 74g develop: accumulated tick duration (us) for window
    /// averages between dumps.
    pub world_tick_us_sum: Arc<AtomicU64>,
    /// Longest world-loop iteration seen, in microseconds.
    pub world_tick_max_us: Arc<AtomicU64>,
    /// Total outbound frames dropped across connections (slow consumers).
    pub dropped_frames_total: Arc<AtomicU64>,
}

impl Default for GameState {
    fn default() -> Self {
        Self::new()
    }
}

impl GameState {
    pub fn initial_core_items() -> Vec<i32> {
        let mut items = vec![0; 22];
        items[0] = 100; // Rules.loadout: copper x100 in desktop 158.1.
        items
    }

    pub fn new() -> Self {
        Self {
            is_hosting: Arc::new(AtomicBool::new(false)),
            is_paused: Arc::new(AtomicBool::new(false)),
            pvp_auto_pause: Arc::new(AtomicBool::new(true)),
            pvp_auto_paused: Arc::new(AtomicBool::new(false)),
            wave: Arc::new(AtomicU32::new(1)),
            wave_time: Arc::new(RwLock::new(180.0)),
            simulation_time: Arc::new(RwLock::new(0.0)),
            enemies_count: Arc::new(AtomicU32::new(0)),
            game_over: Arc::new(AtomicBool::new(false)),
            core_health: Arc::new(RwLock::new(6000.0)),
            mode: Arc::new(RwLock::new(GameMode::Survival)),
            map_name: Arc::new(RwLock::new("maze".to_string())),
            players_count: Arc::new(AtomicU32::new(0)),
            core_items: Arc::new(RwLock::new(Self::initial_core_items())),
            team_items: Arc::new(DashMap::new()),
            game_stats: Arc::new(RwLock::new(GameStats::default())),
            infinite_resources: Arc::new(AtomicBool::new(false)),
            strict_mode: Arc::new(AtomicBool::new(false)),
            world_ticks: Arc::new(AtomicU64::new(0)),
            world_tick_us: Arc::new(AtomicU64::new(0)),
            develop_mode: Arc::new(AtomicBool::new(false)),
            develop_interval_ms: Arc::new(AtomicU64::new(5000)),
            host_map_events: Arc::new(AtomicU64::new(0)),
            save_build_us: Arc::new(AtomicU64::new(0)),
            world_tick_us_sum: Arc::new(AtomicU64::new(0)),
            world_tick_max_us: Arc::new(AtomicU64::new(0)),
            dropped_frames_total: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn start_hosting(&self, map: String, mode: GameMode) {
        *self.map_name.write() = map;
        *self.mode.write() = mode;
        self.game_over.store(false, Ordering::Relaxed);
        *self.core_health.write() = 6000.0;
        *self.simulation_time.write() = 0.0;
        self.is_hosting.store(true, Ordering::SeqCst);
    }

    pub fn stop_hosting(&self) {
        self.is_hosting.store(false, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool {
        self.is_hosting.load(Ordering::SeqCst) && !self.is_paused.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p2_metrics_counters_start_at_zero_and_are_exposed() {
        // P2: world-loop metrics initialize to zero and the fields the
        // `status` command reads are present.
        let state = GameState::new();
        assert_eq!(state.world_ticks.load(Ordering::Relaxed), 0);
        assert_eq!(state.world_tick_us.load(Ordering::Relaxed), 0);
        assert_eq!(state.world_tick_max_us.load(Ordering::Relaxed), 0);
        assert_eq!(state.dropped_frames_total.load(Ordering::Relaxed), 0);
        state.world_ticks.fetch_add(7, Ordering::Relaxed);
        state.world_tick_max_us.fetch_max(1234, Ordering::Relaxed);
        assert_eq!(state.world_ticks.load(Ordering::Relaxed), 7);
        assert_eq!(state.world_tick_max_us.load(Ordering::Relaxed), 1234);
        // fetch_max never decreases.
        state.world_tick_max_us.fetch_max(100, Ordering::Relaxed);
        assert_eq!(state.world_tick_max_us.load(Ordering::Relaxed), 1234);
    }
}
