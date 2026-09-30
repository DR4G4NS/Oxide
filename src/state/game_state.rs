#![allow(dead_code)]

use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Mid-game simulation extras that do not belong on `WaveRules` (weather,
/// fog vision, map objectives, queued S→C Call packets, map-area clamp).
#[derive(Debug)]
pub struct WorldExtras {
    pub weather: RwLock<Vec<WeatherEntry>>,
    pub objectives: RwLock<Vec<MapObjective>>,
    pub markers_json: RwLock<String>,
    pub pending_calls: Mutex<Vec<Vec<u8>>>,
    pub fog_visible: DashMap<(u8, i32), ()>,
    /// Logic `setblock floor` live layer (Vec floors is otherwise read-only).
    pub floor_overrides: DashMap<i32, i16>,
    /// Logic `setblock ore` live layer.
    pub overlay_overrides: DashMap<i32, i16>,
    /// Player drownTime, keyed by unit id (audit H4).
    pub player_drown: DashMap<i32, f32>,
    pub limit_x: AtomicI32,
    pub limit_y: AtomicI32,
    pub limit_w: AtomicI32,
    pub limit_h: AtomicI32,
    pub rand_seed0: AtomicI64,
    pub rand_seed1: AtomicI64,
    /// Units queued for `kill_enemy` from domains that lack a `FrameEmit`.
    pub pending_unit_kills: Mutex<Vec<i32>>,
    /// Surge-wall lightning strikes queued so `damage_building` never re-enters `enemies`.
    pub pending_wall_lightning: Mutex<Vec<(f32, f32, u8)>>,
    /// Ground item stacks from unit death (audit M9). 159.7 `entities.json`
    /// has no serialized ItemEntity class, so these are server-side pickups.
    pub ground_items: Mutex<Vec<GroundItem>>,
    /// UnitAssembler tether: assembler tile position -> live assembly-drone
    /// ids plus the official `droneProgress` accumulator (`droneConstructTime`
    /// = 240 ticks). Runtime-only; the drones themselves live in `enemies`.
    pub assembler_drones: Mutex<HashMap<i32, AssemblerDroneBind>>,
    /// Simulation tick until which `WaveSpawner.isSpawning()` is true
    /// (121 ticks after `runWave`; ASTRA W06).
    pub spawner_until: AtomicU32,
}

/// Per-assembler AssemblerAI bind (UnitAssemblerBuild.units + droneProgress).
#[derive(Clone, Debug, Default)]
pub struct AssemblerDroneBind {
    pub unit_ids: Vec<i32>,
    pub progress: f32,
}

#[derive(Clone, Debug)]
pub struct GroundItem {
    pub x: f32,
    pub y: f32,
    pub item: i16,
    pub amount: i32,
    /// Remaining ticks (vanilla ItemComp.lifetime = 60*60*3).
    pub life: f32,
}

#[derive(Clone, Debug)]
pub struct WeatherEntry {
    pub weather_id: i16,
    pub intensity: f32,
    pub remaining: f32,
}

#[derive(Clone, Debug)]
pub struct MapObjective {
    pub kind: MapObjectiveKind,
    pub complete: bool,
    /// Timer elapsed ticks (MapObjectives.TimerObjective.countup).
    pub progress: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum MapObjectiveKind {
    WinWave(i32),
    DestroyCores,
    Flag(u64),
    Item {
        item: i16,
        amount: i32,
    },
    CoreItem {
        item: i16,
        amount: i32,
    },
    BuildCount {
        block: i16,
        count: i32,
    },
    UnitCount {
        unit: i16,
        count: i32,
    },
    DestroyUnits {
        count: i32,
    },
    Timer {
        duration: f32,
    },
    DestroyBlock {
        x: i16,
        y: i16,
        team: u8,
        block: i16,
    },
    CommandMode,
}

impl Default for WorldExtras {
    fn default() -> Self {
        Self {
            weather: RwLock::new(Vec::new()),
            objectives: RwLock::new(Vec::new()),
            markers_json: RwLock::new("{}".to_string()),
            pending_calls: Mutex::new(Vec::new()),
            fog_visible: DashMap::new(),
            floor_overrides: DashMap::new(),
            overlay_overrides: DashMap::new(),
            player_drown: DashMap::new(),
            limit_x: AtomicI32::new(0),
            limit_y: AtomicI32::new(0),
            limit_w: AtomicI32::new(0),
            limit_h: AtomicI32::new(0),
            rand_seed0: AtomicI64::new(0x9E37_79B9_7F4A_7C15u64 as i64),
            rand_seed1: AtomicI64::new(0xA076_1D64_78BD_642Fu64 as i64),
            pending_unit_kills: Mutex::new(Vec::new()),
            pending_wall_lightning: Mutex::new(Vec::new()),
            ground_items: Mutex::new(Vec::new()),
            assembler_drones: Mutex::new(HashMap::new()),
            spawner_until: AtomicU32::new(0),
        }
    }
}

impl WorldExtras {
    pub fn queue_call(&self, frame: Vec<u8>) {
        self.pending_calls.lock().push(frame);
    }

    pub fn take_calls(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.pending_calls.lock())
    }

    pub fn rand_seeds(&self) -> (i64, i64) {
        (
            self.rand_seed0.load(Ordering::Relaxed),
            self.rand_seed1.load(Ordering::Relaxed),
        )
    }

    pub fn queue_unit_kill(&self, unit_id: i32) {
        self.pending_unit_kills.lock().push(unit_id);
    }

    pub fn take_unit_kills(&self) -> Vec<i32> {
        std::mem::take(&mut *self.pending_unit_kills.lock())
    }

    pub fn queue_wall_lightning(&self, x: f32, y: f32, team: u8) {
        self.pending_wall_lightning.lock().push((x, y, team));
    }

    pub fn take_wall_lightning(&self) -> Vec<(f32, f32, u8)> {
        std::mem::take(&mut *self.pending_wall_lightning.lock())
    }
}

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

    /// Add `amount` to a content counter (core-item ingress).
    pub fn bump_amount(counter: &mut Vec<(i16, u32)>, id: i16, amount: u32) {
        if amount == 0 {
            return;
        }
        match counter.iter_mut().find(|(existing, _)| *existing == id) {
            Some((_, count)) => *count = count.saturating_add(amount),
            None => counter.push((id, amount)),
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
    /// Set when `setrule` mutates live Rules; the world loop broadcasts SetRules.
    pub rules_dirty: Arc<AtomicBool>,
    /// Fog/weather/objectives/queued Call packets (audit M11/M13–M19).
    pub extras: Arc<WorldExtras>,
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
            rules_dirty: Arc::new(AtomicBool::new(false)),
            extras: Arc::new(WorldExtras::default()),
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
