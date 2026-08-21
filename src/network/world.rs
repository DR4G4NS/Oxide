//! World data model. Types are shared by the network layer, the simulation
//! modules and the logic executor.

use crate::state::game_state::GameState;
use dashmap::DashMap;
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic instance counter for [`DynamicTile::generation`]. Distinct
/// buildings that occupy the same tile (A destroyed, B placed) must not
/// share an identity: Java's `Building.isValid()` is `tile.build == this`.
static NEXT_BUILDING_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Cheap instance identity of a live building. Position alone is not
/// enough: a new processor on the same tile is a different `Building`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BuildingIdentity {
    pub position: i32,
    pub generation: u64,
}

/// Raises the process-wide generation counter past `generation` so a later
/// stamp cannot reuse an id that already exists (or existed) on a tile.
pub fn note_building_generation(generation: u64) {
    let target = generation.saturating_add(1);
    let mut current = NEXT_BUILDING_GENERATION.load(Ordering::Relaxed);
    while current < target {
        match NEXT_BUILDING_GENERATION.compare_exchange_weak(
            current,
            target,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}

/// Next unused building instance id.
pub fn next_building_generation() -> u64 {
    NEXT_BUILDING_GENERATION.fetch_add(1, Ordering::Relaxed)
}
use tracing::warn;

#[derive(Clone, Debug)]
pub struct SessionPlayer {
    pub id: i32,
    /// Unit currently possessed by this player. `unit_id` remains the
    /// persistent/core-unit identity used by player profiles; keeping the two
    /// concepts separate mirrors Mindustry's `Player.unit()` without
    /// re-keying the server's combat/profile maps when a player temporarily
    /// controls an AI unit or a `ControlBlock` proxy.
    pub controlled_unit: ControlledUnit,
    pub unit_id: i32,
    pub uuid: String,
    pub name: String,
    pub color: i32,
    pub last_snapshot: i32,
    pub x: f32,
    pub y: f32,
    pub mouse_x: f32,
    pub mouse_y: f32,
    pub rotation: f32,
    pub boosting: bool,
    pub shooting: bool,
    /// Command followed by the last incoming CommandAI unit this player
    /// possessed. Mirrors PlayerComp.lastCommand (`@NoSync`): runtime-only,
    /// player-owned state that is reset on reconnect.
    pub last_command: Option<u8>,
    pub active_plans: HashSet<(bool, i32, i16)>,
    pub mining_position: Option<i32>,
    pub mining_progress: f32,
    pub mining_updated: std::time::Instant,
    pub carried_item: i16,
    pub carried_amount: i32,
    /// Latest ClientPlanSnapshot group id (-1 = none) and its raw plan bytes
    /// (forwarded verbatim to other clients as ClientPlanSnapshotReceived).
    pub preview_plan_group: i32,
    pub preview_plans: Vec<u8>,
    pub last_shot: std::time::Instant,
    /// Whether this player is a registered admin (Player.writeSync admin flag).
    pub admin: bool,
    /// Official chat spam limit (NetClient.chatRate: 2 msgs / 2 s).
    pub chat_rate: crate::network::wire::ChatRateLimiter,
}

/// Authoritative counterpart of TypeIO's unit reference discriminator.
///
/// The official wire uses type 2 for a regular unit and type 1 for the proxy
/// unit owned by a building (`BlockUnitc`). The core Alpha is represented
/// separately because its stable server ID is also the key of the persisted
/// player combat state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ControlledUnit {
    #[default]
    Core,
    Standard(i32),
    Building(i32),
}

impl ControlledUnit {
    pub(crate) fn standard_id(self) -> Option<i32> {
        match self {
            Self::Standard(id) => Some(id),
            _ => None,
        }
    }

    pub(crate) fn building_position(self) -> Option<i32> {
        match self {
            Self::Building(position) => Some(position),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct DynamicTile {
    pub position: i32,
    pub block: i16,
    pub rotation: u8,
    pub team: u8,
    pub config: Vec<u8>,
    /// SwitchBlock enabled state (defaults to enabled, like the official
    /// `enabled = true` field on BuildingComp).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// MessageBlock / WorldMessageBlock text (TypeIO String config).
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub occupied: Vec<i32>,
    #[serde(default = "no_item")]
    pub stored_item: i16,
    #[serde(default)]
    pub stored_amount: i32,
    #[serde(default)]
    pub production_progress: f32,
    #[serde(default)]
    pub transport_progress: f32,
    #[serde(default)]
    pub ammo_units: f32,
    #[serde(default)]
    pub inventory: Vec<(i16, i32)>,
    #[serde(default)]
    pub power_stored: f32,
    /// Linked power-node positions loaded from BuildingComp.PowerModule.
    #[serde(default)]
    pub power_links: Vec<i32>,
    /// Full LiquidModule contents; stored_liquid/liquid_amount are the
    /// compatibility current-liquid view used by older simulation paths.
    #[serde(default)]
    pub liquid_inventory: Vec<(i16, f32)>,
    #[serde(default = "no_item")]
    pub stored_liquid: i16,
    #[serde(default)]
    pub liquid_amount: f32,
    #[serde(default)]
    pub output_liquid_amount: f32,
    #[serde(default)]
    pub junction_items: Vec<(u8, i16, f32)>,
    #[serde(default)]
    pub mass_driver_incoming: Vec<(i32, i16, i32, f32)>,
    #[serde(default = "default_mass_driver_rotation")]
    pub mass_driver_rotation: f32,
    #[serde(default)]
    pub mass_driver_waiting: Vec<i32>,
    #[serde(default)]
    pub payload: Option<Box<CarriedPayload>>,
    #[serde(default)]
    pub payload_progress: f32,
    #[serde(default)]
    pub payload_rotation: f32,
    #[serde(default)]
    pub payload_accum: Vec<f32>,
    #[serde(default)]
    pub health: f32,
    #[serde(default)]
    pub door_open: bool,
    #[serde(default)]
    pub shield: f32,
    #[serde(default = "default_light_color")]
    pub light_color: i32,
    #[serde(default)]
    pub memory: Vec<f64>,
    #[serde(default)]
    pub duct_rec_dir: u8,
    #[serde(default)]
    pub unloader_offset: i16,
    /// Per-item transport positions for conveyors (257/258/260) and stack
    /// conveyors (259): `(item, progress)`, FIFO head first. Plain conveyors
    /// keep progress in 0..=1 with official 0.4 spacing; old saves are healed
    /// deterministically by the logistics simulation and snapshot encoder.
    /// The official client animates each item individually (ConveyorBuild
    /// keeps `ids/xs/ys` arrays and moves them locally between snapshots), so
    /// one shared `transport_progress` makes items jump in place (micro
    /// stutters). Empty for non-transport blocks and old saves.
    #[serde(default)]
    pub conveyor_items: Vec<(i16, f32)>,
    /// P0-10: typed private metadata for unit factories (blocks 377-379).
    /// The official `UnitFactoryBuild` keeps its selected command in a
    /// dedicated building field; the legacy checkpoint format smuggled it
    /// into `config` as a `[254, command]` suffix. Loads migrate that suffix
    /// here so `config` stays a pure TypeIO object and serializers never
    /// emit ambiguous `Vec<u8>`.
    #[serde(default)]
    pub factory_command: Option<u8>,
    /// P1: official StackConveyor state machine (StackConveyorBuild):
    /// stateMove=0, stateLoad=1, stateUnload=2.
    #[serde(default)]
    pub stack_state: u8,
    /// P1: the upstream tile this conveyor is reeling from (official `link`,
    /// -1 = empty). Runtime state, not config.
    #[serde(default = "no_link")]
    pub stack_link: i32,
    /// P1: reel/transfer cooldown (official `cooldown`, clamped 0..recharge).
    #[serde(default)]
    pub stack_cooldown: f32,
    /// Instance identity of this building. Distinct from `position`: a new
    /// block on the same tile (even the same block id) is a new Building in
    /// 158.1 (`tile.build == this`). Default 0 for map tiles and old saves.
    #[serde(default)]
    pub generation: u64,
}

impl DynamicTile {
    pub fn identity(&self) -> BuildingIdentity {
        BuildingIdentity {
            position: self.position,
            generation: self.generation,
        }
    }
}

impl Default for DynamicTile {
    fn default() -> Self {
        DynamicTile {
            position: 0,
            block: 0,
            rotation: 0,
            team: 0,
            config: Vec::new(),
            enabled: true,
            message: None,
            occupied: Vec::new(),
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: Vec::new(),
            power_stored: 0.0,
            power_links: Vec::new(),
            liquid_inventory: Vec::new(),
            stored_liquid: -1,
            liquid_amount: 0.0,
            output_liquid_amount: 0.0,
            junction_items: Vec::new(),
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 90.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: 1.0,
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
        }
    }
}

pub(crate) const fn no_link() -> i32 {
    -1
}

/// Pal.accent.rgba() = 0xffd37fff in the official 158.1 client (LightBlock default).
pub(crate) const fn default_light_color() -> i32 {
    -1_900_545
}

pub(crate) const fn no_item() -> i16 {
    -1
}

pub(crate) const fn default_enabled() -> bool {
    true
}

pub(crate) const fn permanent_status_duration() -> f32 {
    f32::MAX
}

pub(crate) const fn default_mass_driver_rotation() -> f32 {
    90.0
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct PersistedWorld {
    pub(crate) version: u8,
    #[serde(default)]
    pub(crate) map_name: String,
    pub(crate) tiles: Vec<DynamicTile>,
    pub(crate) core_items: Vec<i32>,
    #[serde(default = "initial_wave")]
    pub(crate) wave: u32,
    #[serde(default = "initial_wave_time")]
    pub(crate) wave_time: f32,
    #[serde(default = "initial_core_health")]
    pub(crate) core_health: f32,
    #[serde(default)]
    pub(crate) enemies: Vec<EnemyUnit>,
    #[serde(default)]
    pub(crate) base_building_health: Vec<PersistedBaseBuildingHealth>,
    #[serde(default)]
    pub(crate) players: Vec<PlayerCombatState>,
    #[serde(default)]
    pub(crate) building_commands: Vec<BuildingCommand>,
    #[serde(default)]
    pub(crate) unit_orders: Vec<UnitOrder>,
    /// Per-team build plans carried by the map (SaveVersion.writeTeamBlocks);
    /// revision 9 persists them so a save/load cycle does not drop them.
    #[serde(default)]
    pub(crate) team_build_plans: crate::engine::typeio::TeamBlocks,
    /// Per-team cores (position + current health). Revision 10; saves from
    /// earlier revisions have no entry and re-derive the cores from the map.
    #[serde(default)]
    pub(crate) team_cores: Vec<PersistedTeamCore>,
    /// Per-team core item inventories. Revision 11; team 1 stays in the
    /// legacy `core_items` field, so saves from earlier revisions load with
    /// `team_items` empty (other teams fall back to an empty inventory).
    #[serde(default)]
    pub(crate) team_items: Vec<PersistedTeamItems>,
    /// Accumulated simulation ticks (official SaveVersion.writeMeta "tick" =
    /// `state.tick`). Revision 12.
    #[serde(default)]
    pub(crate) simulation_time: f32,
    /// Global logic flags (setflag/getflag), persisted for save/load parity.
    /// Revision 12; absent in older saves.
    #[serde(default)]
    pub(crate) logic_flags: Vec<(String, f64)>,
    /// Official GameStats (game-over statistics). Revision 13.
    #[serde(default)]
    pub(crate) game_stats: crate::state::game_state::GameStats,
    /// Authoritative puddles (round 73, revision 14). The official server
    /// syncs them via entity snapshots (classId 13) and persists them in
    /// MSAV; the JSON checkpoint does the same so save/load keeps the
    /// puddles the client is drawing. Absent in earlier revisions.
    #[serde(default)]
    pub(crate) puddles: Vec<PersistedPuddle>,
}

/// One puddle in the JSON checkpoint (round 73, revision 14).
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct PersistedPuddle {
    pub(crate) position: i32,
    pub(crate) liquid: i16,
    pub(crate) amount: f32,
    pub(crate) entity_id: i32,
}

/// One team's core inventory snapshot (`{team, items[item_id]}`).
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct PersistedTeamItems {
    pub(crate) team: u8,
    pub(crate) items: Vec<i32>,
}

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub struct PersistedTeamCore {
    pub(crate) team: u8,
    pub(crate) position: i32,
    pub(crate) block: i16,
    pub(crate) health: f32,
    pub(crate) max_health: f32,
}

/// A registered core for one team: packed tile position, block id, current
/// health and maximum health. Populated at map load from the MSAV
/// `NetworkBuilding` list (core blocks 339-344; one core per team — when a
/// map carries several cores for the same team, the first is kept). Maps
/// without cores (custom maps) and teams beyond the map's core set fall back
/// to the legacy single core (`DynamicWorld.core_position` /
/// `GameState.core_health`).
#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
pub struct TeamCore {
    pub(crate) position: i32,
    pub(crate) block: i16,
    pub(crate) health: f32,
    pub(crate) max_health: f32,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct PersistedBaseBuildingHealth {
    pub position: i32,
    pub health: f32,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct BuildingCommand {
    pub position: i32,
    pub target_x: f32,
    pub target_y: f32,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct UnitOrder {
    pub unit_id: i32,
    pub command: u8,
    #[serde(default)]
    pub stances: u32,
    #[serde(default)]
    pub payload_cooldown: f32,
    #[serde(default)]
    pub target_kind: u8,
    #[serde(default = "negative_one")]
    pub target_id: i32,
    #[serde(default)]
    pub target_x: Option<f32>,
    #[serde(default)]
    pub target_y: Option<f32>,
    /// LogicAI `control` mode (desktop 158.1 LogicAI.java): 0 idle, 1 stop,
    /// 2 move. Only meaningful while [`UnitAuthority::Logic`] holds the unit.
    #[serde(default)]
    pub logic_control: u8,
    #[serde(default)]
    pub queue: Vec<UnitOrderTarget>,
}

impl Default for UnitOrder {
    fn default() -> Self {
        Self {
            unit_id: 0,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        }
    }
}

/// LogicAI `LUnitControl` values mirrored for [`UnitOrder::logic_control`].
pub(crate) mod logic_control {
    pub(crate) const IDLE: u8 = 0;
    pub(crate) const STOP: u8 = 1;
    pub(crate) const MOVE: u8 = 2;
    pub(crate) const PATHFIND: u8 = 3;
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct UnitOrderTarget {
    pub kind: u8,
    #[serde(default = "negative_one")]
    pub id: i32,
    pub x: f32,
    pub y: f32,
}

/// P0-01: the single authoritative answer to "who controls this unit".
///
/// `UnitOrder` keeps describing WHAT the unit was told to do (command,
/// target, queue, stances) and must never be used on its own to decide
/// control ownership — the official split is `Unit.controller` (who)
/// versus `CommandAI.hasCommand()` (what; 158.1: `targetPos != null`, see
/// [`crate::network::units::unit_order_has_active_rts_target`]). The mere
/// existence of a `UnitOrder` therefore never implies an active RTS
/// command: factory and wave spawns carry default orders without targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum UnitAuthority {
    /// `type.createController` AI — wave AI, or a CommandAI unit whose team
    /// is not player-commandable. The common default.
    #[default]
    DefaultAi,
    /// Player RTS orders hold the unit (`CommandAI`); whether a command is
    /// currently ACTIVE is answered by the order's target, not by this tag.
    Command,
    /// A logic processor drives the unit (`LogicAI`): `processor_pos`
    /// locates the owning processor, `processor_generation` is the
    /// Building instance that acquired the lease (Java's `la.controller`
    /// object identity), and `remaining_ticks` mirrors the LogicAI control
    /// timeout.
    Logic {
        processor_pos: i32,
        remaining_ticks: f32,
        #[serde(default)]
        processor_generation: u64,
    },
    /// A player possesses the unit (`Player` controller).
    Player { player_id: i32 },
}

pub(crate) const fn negative_one() -> i32 {
    -1
}

pub(crate) const fn initial_wave() -> u32 {
    1
}

pub(crate) const fn initial_wave_time() -> f32 {
    180.0
}

pub(crate) const fn initial_core_health() -> f32 {
    6000.0
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum PersistedWorldCompat {
    Current(Box<PersistedWorld>),
    Legacy(Vec<DynamicTile>),
}

pub struct LoadedWorld {
    pub tiles: DashMap<i32, DynamicTile>,
    pub core_items: Option<Vec<i32>>,
    pub wave: Option<u32>,
    pub wave_time: Option<f32>,
    pub core_health: Option<f32>,
    pub enemies: Vec<EnemyUnit>,
    pub base_building_health: Vec<PersistedBaseBuildingHealth>,
    pub players: Vec<PlayerCombatState>,
    pub building_commands: Vec<BuildingCommand>,
    pub unit_orders: Vec<UnitOrder>,
    pub map_name: Option<String>,
    pub team_build_plans: crate::engine::typeio::TeamBlocks,
    pub(crate) team_cores: Vec<PersistedTeamCore>,
    /// Persisted per-team item inventories (revision 11; empty for v<=10).
    pub(crate) team_items: Vec<PersistedTeamItems>,
    /// Persisted simulation ticks (revision 12; None for v<=11).
    pub(crate) simulation_time: Option<f32>,
    /// Persisted global logic flags (revision 12; empty for v<=11).
    pub(crate) logic_flags: Vec<(String, f64)>,
    /// Persisted GameStats (revision 13; default for v<=12).
    pub(crate) game_stats: crate::state::game_state::GameStats,
    /// Persisted puddles (revision 14; empty for v<=13).
    pub(crate) puddles: Vec<PersistedPuddle>,
}

/// Static spatial index of mineable base-map ore tiles (round 74 fix).
pub struct OreIndex {
    pub cells_x: i32,
    pub cells_y: i32,
    pub grid: Vec<Vec<(i32, i16, u8)>>,
    /// Ore tile count per item id (index = item id, len 18).
    pub per_item: Vec<u32>,
    /// Total indexed ore tiles (any item).
    pub total: usize,
}

pub struct DynamicWorld {
    pub(crate) game_state: GameState,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) sharded_unit_cap: i32,
    pub(crate) core_position: i32,
    pub(crate) core_max_health: f32,
    /// Legacy representative core per team, retained for API/save compatibility.
    /// The authoritative topology is `team_core_lists`, which preserves every
    /// core in registration order.
    pub cores: DashMap<u8, TeamCore>,
    /// All live cores grouped by team, in the same order Java's TeamData.cores
    /// receives them. A team's inventory is shared independently in GameState.
    pub team_core_lists: DashMap<u8, Vec<TeamCore>>,
    pub(crate) base_blocks: Vec<i16>,
    pub(crate) base_centers: Vec<bool>,
    pub(crate) tile_data: Vec<u8>,
    pub(crate) base_building_templates: Vec<BaseBuildingState>,
    pub(crate) base_buildings: DashMap<i32, BaseBuildingState>,
    pub(crate) floors: Vec<i16>,
    pub(crate) overlays: Vec<i16>,
    pub(crate) enemy_spawns: Vec<(i16, i16)>,
    pub(crate) enemies: DashMap<i32, EnemyUnit>,
    /// Insertion order of live units, mirroring `Groups.unit` (desktop 158.1
    /// `EntityGroup` backing `Seq(ordered=false)`): add appends, remove is
    /// swap-remove. `ubind @UnitType` rebuilds `TeamData.unitCache` by
    /// iterating this sequence — not by unit id.
    pub(crate) unit_group_order: parking_lot::Mutex<Vec<i32>>,
    pub(crate) players: DashMap<i32, PlayerCombatState>,
    pub(crate) player_sessions: DashMap<i32, SessionPlayer>,
    pub(crate) player_profiles: DashMap<String, PlayerCombatState>,
    pub(crate) building_commands: DashMap<i32, BuildingCommand>,
    pub(crate) unit_orders: DashMap<i32, UnitOrder>,
    pub(crate) next_player_unit_id: AtomicI32,
    pub(crate) next_enemy_id: AtomicI32,
    pub(crate) projectiles: DashMap<i32, Projectile>,
    pub(crate) next_projectile_id: AtomicI32,
    pub(crate) overdrive_boosts: DashMap<i32, TimedBoost>,
    pub(crate) heal_suppression: DashMap<i32, f32>,
    /// Oct ForceFieldAbility area shields, keyed by oct unit id.
    pub(crate) force_fields: DashMap<i32, ForceFieldState>,
    pub(crate) tiles: DashMap<i32, DynamicTile>,
    pub(crate) pending_builds: DashMap<i32, PendingBuild>,
    pub(crate) pending_breaks: DashMap<i32, PendingBreak>,
    /// Static spatial index of mineable base-map ore tiles (round 74 fix):
    /// a coarse grid (ORE_CELL_TILES tiles per cell) so the per-unit
    /// nearest-ore search scans only nearby cells instead of expanding a
    /// square over the whole map, which made the world tick take seconds
    /// when ore was far away or absent (18+ monos on a 200x200 map).
    pub(crate) mineable_ore: std::sync::OnceLock<crate::network::world::OreIndex>,
    /// Round 74d: per-tick footprint index (every occupied position ->
    /// origin tile) rebuilt once per world tick. Makes `dynamic_at` and
    /// `effective_block` O(1) in the hot logistics path — the legacy linear
    /// scan over every tile made the tick 20-32 ms with ~360 tiles.
    pub(crate) tile_footprint: DashMap<i32, i32>,
    /// Transient AI state (round 74): cached mining target per mono unit —
    /// `(ore tile position, search backoff cooldown)`. Position 0 with a
    /// cooldown means the last search found nothing and the next scan is
    /// deferred; 0 without cooldown forces a fresh scan.
    pub(crate) mono_mining_targets: DashMap<i32, (i32, f32)>,
    pub(crate) navigation_revision: AtomicU64,
    pub(crate) ground_navigation: parking_lot::Mutex<Option<NavigationField>>,
    pub(crate) leg_navigation: parking_lot::Mutex<Option<NavigationField>>,
    pub(crate) save_path: PathBuf,
    pub(crate) team_build_plans: parking_lot::RwLock<crate::engine::typeio::TeamBlocks>,
    /// Global logic flags (setflag/getflag), keyed by name.
    pub logic_flags: DashMap<String, f64>,
    /// Drill progress (ticks) for prebuilt map drills (SOL-001: base
    /// buildings participate in production; their output feeds the team
    /// core directly because map conveyor networks are not simulated).
    pub(crate) base_drill_progress: DashMap<i32, f32>,
    /// Craft progress (ticks) for prebuilt map item factories.
    pub(crate) base_factory_progress: DashMap<i32, f32>,
    /// Reload progress (ticks) for prebuilt map turrets (SOL-001).
    pub(crate) base_turret_progress: DashMap<i32, f32>,
    /// Reload progress (ticks) for prebuilt map menders.
    pub(crate) base_mender_progress: DashMap<i32, f32>,
    /// Live logic executors for processor tiles (431-433), keyed by position.
    pub(crate) logic_executors: DashMap<i32, crate::logic::ExecutorState>,
    /// Pending packed DisplayCmd values for logic displays. This mirrors
    /// LogicDisplayBuild.commands; it is transient rendering state and is not
    /// persisted in the world delta.
    pub(crate) logic_display_commands: DashMap<i32, Vec<u64>>,
    pub(crate) network_template: Arc<Vec<u8>>,
    pub(crate) persistence_dirty: AtomicBool,
    pub(crate) persistence_lock: parking_lot::Mutex<()>,
    /// Wave generation rules parsed from the loaded map's rules JSON
    /// (`Rules.spawns`, `waveSpacing`, `initialWaveSpacing`).
    pub(crate) wave_rules: parking_lot::RwLock<crate::network::units::WaveRules>,
    /// Votekick state (official VoteSession): target name + accumulated votes.
    pub(crate) votekick_target: parking_lot::RwLock<Option<String>>,
    pub(crate) votekick_votes: std::sync::atomic::AtomicI32,
    /// Per-voter votes (uuid -> +1/-1) so nobody votes twice (official
    /// VoteSession.voted keyed by uuid + lastIP).
    pub(crate) votekick_voters: dashmap::DashMap<String, i32>,
    /// Per-player cooldown (uuid -> last votekick time) for voteCooldown.
    pub(crate) votekick_cooldowns: dashmap::DashMap<String, std::time::Instant>,
    /// P1: authoritative puddle model (liquid leaks; see buildings/puddles.rs).
    pub puddles: crate::network::buildings::puddles::PuddleSystem,
}

impl crate::network::units::StatusContainer for EnemyUnit {
    fn status_effect(&self) -> i16 {
        self.status_effect
    }
    fn status_fields(
        &mut self,
    ) -> (
        &mut i16,
        &mut f32,
        &mut Vec<crate::game::status::ActiveStatus>,
    ) {
        (
            &mut self.status_effect,
            &mut self.status_duration,
            &mut self.statuses,
        )
    }
    fn statuses_ref(&self) -> &[crate::game::status::ActiveStatus] {
        &self.statuses
    }
    fn unit_type_id(&self) -> i16 {
        self.unit_type
    }
    fn take_status_damage(&mut self, pierce: f32, normal: f32) {
        let mut taken = 0.0;
        if pierce.is_finite() && pierce > 0.0 {
            taken += pierce;
        }
        if normal.is_finite() && normal > 0.0 {
            taken += normal;
        }
        if taken > 0.0 {
            self.health = (self.health - taken).max(0.0);
        }
        if normal.is_finite() && normal < 0.0 && self.health.is_finite() {
            let max = crate::network::units::enemy_spec(self.unit_type)
                .map(|spec| spec.health)
                .unwrap_or(self.health);
            self.health = (self.health - normal).clamp(0.0, max);
        }
    }

    fn status_aggregate_cached(&self) -> Option<crate::game::status::StatusAggregate> {
        self.status_agg
    }

    fn set_status_aggregate_cache(&mut self, agg: crate::game::status::StatusAggregate) {
        self.status_agg = Some(agg);
    }
}

impl crate::network::units::StatusContainer for PlayerCombatState {
    fn status_effect(&self) -> i16 {
        self.status_effect
    }
    fn status_fields(
        &mut self,
    ) -> (
        &mut i16,
        &mut f32,
        &mut Vec<crate::game::status::ActiveStatus>,
    ) {
        (
            &mut self.status_effect,
            &mut self.status_duration,
            &mut self.statuses,
        )
    }
    fn statuses_ref(&self) -> &[crate::game::status::ActiveStatus] {
        &self.statuses
    }
    fn unit_type_id(&self) -> i16 {
        35 // core Alpha; player-specific immunities follow the possessed unit
    }
    fn take_status_damage(&mut self, pierce: f32, normal: f32) {
        // Player DoT still goes through `damage_player` (death/RPC). Tick
        // only expires the collection for players.
        let _ = (pierce, normal);
    }
}

impl DynamicWorld {
    /// Append `id` to `Groups.unit` order. No-op if already present (Java
    /// `Unit.add` returns early when `added` is set).
    pub fn register_unit_group(&self, id: i32) {
        let mut order = self.unit_group_order.lock();
        if !order.contains(&id) {
            order.push(id);
        }
    }

    /// Swap-remove `id` from `Groups.unit` order (unordered Seq).
    pub fn unregister_unit_group(&self, id: i32) {
        let mut order = self.unit_group_order.lock();
        if let Some(index) = order.iter().position(|&live| live == id) {
            order.swap_remove(index);
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn core_position(&self) -> i32 {
        self.core_position
    }

    pub fn core_max_health(&self) -> f32 {
        self.core_max_health
    }

    pub fn team_build_plans(
        &self,
    ) -> parking_lot::RwLockReadGuard<'_, crate::engine::typeio::TeamBlocks> {
        self.team_build_plans.read()
    }

    pub fn set_team_build_plans(&self, plans: crate::engine::typeio::TeamBlocks) {
        *self.team_build_plans.write() = plans;
    }

    pub fn player_sessions(&self) -> &DashMap<i32, SessionPlayer> {
        &self.player_sessions
    }

    pub fn player_profiles(&self) -> &DashMap<String, PlayerCombatState> {
        &self.player_profiles
    }

    pub fn tiles(&self) -> &DashMap<i32, DynamicTile> {
        &self.tiles
    }

    pub fn enemies(&self) -> &DashMap<i32, EnemyUnit> {
        &self.enemies
    }

    pub fn players(&self) -> &DashMap<i32, PlayerCombatState> {
        &self.players
    }

    pub fn base_buildings(&self) -> &DashMap<i32, BaseBuildingState> {
        &self.base_buildings
    }

    pub fn building_commands(&self) -> &DashMap<i32, BuildingCommand> {
        &self.building_commands
    }

    pub fn unit_orders(&self) -> &DashMap<i32, UnitOrder> {
        &self.unit_orders
    }

    pub fn game_state(&self) -> &GameState {
        &self.game_state
    }

    pub fn save_path(&self) -> &Path {
        &self.save_path
    }

    pub fn network_template(&self) -> &[u8] {
        &self.network_template
    }
}

/// Identity for a building about to occupy `position`. Observes any live
/// occupant first so a same-tile replacement cannot collide with the
/// previous instance — including after a save/load that restored a high
/// generation onto a counter that had reset.
pub fn assign_new_building_generation(world: &DynamicWorld, position: i32) -> u64 {
    if let Some(tile) = world.tiles.get(&position) {
        note_building_generation(tile.generation);
    }
    next_building_generation()
}

/// Assigns a fresh instance id to `tile` (a newly created building).
pub fn stamp_new_building(world: &DynamicWorld, tile: &mut DynamicTile) {
    tile.generation = assign_new_building_generation(world, tile.position);
}

/// The atomically shared `DynamicWorld` holder. The simulation, every client
/// connection and the console runtime reload the current world on each loop
/// iteration; `host` swaps the world in place so connected clients can be
/// re-streamed without restarting the server.
#[derive(Clone)]
pub struct WorldStore {
    pub(crate) current: Arc<parking_lot::RwLock<Arc<DynamicWorld>>>,
}

impl WorldStore {
    pub fn new(world: DynamicWorld) -> Self {
        Self {
            current: Arc::new(parking_lot::RwLock::new(Arc::new(world))),
        }
    }

    /// Snapshot of the current world; the caller may hold the `Arc` across
    /// await points (no lock is retained).
    pub fn load(&self) -> Arc<DynamicWorld> {
        self.current.read().clone()
    }

    /// Atomically replaces the active world and returns the previous one.
    pub fn swap(&self, world: DynamicWorld) -> Arc<DynamicWorld> {
        std::mem::replace(&mut *self.current.write(), Arc::new(world))
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TimedBoost {
    pub(crate) multiplier: f32,
    pub(crate) remaining_ticks: f32,
}

/// Oct ForceFieldAbility(140, 4, 7000, 480, 8) state: `hp` is the area shield
/// pool and `remaining_ticks` is the cooldown after the shield breaks.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ForceFieldState {
    pub(crate) hp: f32,
    pub(crate) remaining_ticks: f32,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct PlayerCombatState {
    pub uuid: String,
    pub player_id: i32,
    pub unit_id: i32,
    pub x: f32,
    pub y: f32,
    pub health: f32,
    pub shield: f32,
    #[serde(default = "no_item")]
    pub status_effect: i16,
    #[serde(default)]
    pub status_duration: f32,
    /// P0-06: authoritative `StatusEntry` collection. `status_effect` /
    /// `status_duration` remain a derived view of the first entry.
    #[serde(default)]
    pub statuses: Vec<crate::game::status::ActiveStatus>,
    #[serde(default)]
    pub dead: bool,
    #[serde(default)]
    pub respawn_timer: f32,
    /// Team of the player (1 = sharded, 2 = crux, 3 = malis, 4 = green,
    /// 5 = blue, 6 = neoplastic). Assigned by `NetServer.assignTeam` in PvP;
    /// survival/attack keep the default sharded team 1. Persisted so a
    /// reconnect keeps the player on their assigned team.
    #[serde(default = "default_player_team")]
    pub team: u8,
}

pub(crate) const fn default_player_team() -> u8 {
    1
}

#[derive(Clone, Debug)]
pub struct BaseBuildingState {
    pub position: i32,
    pub block: i16,
    pub team: u8,
    pub health: f32,
    pub occupied: Vec<i32>,
    /// Item inventory (SOL-001: prebuilt map factories/drills participate in
    /// production). Not persisted: rebuilt from the map on load.
    pub inventory: Vec<(i16, i32)>,
}

#[derive(Clone)]
pub(crate) struct NavigationField {
    pub(crate) revision: u64,
    pub(crate) costs: Arc<Vec<u32>>,
}

pub(crate) struct EnemyNavigationTarget {
    pub(crate) building: Option<(i32, f32, f32)>,
    pub(crate) movement: (f32, f32),
}

pub(crate) fn core_tile(world: &DynamicWorld) -> (i16, i16) {
    (
        (world.core_position >> 16) as i16,
        world.core_position as i16,
    )
}

pub(crate) fn core_world(world: &DynamicWorld) -> (f32, f32) {
    let (x, y) = core_tile(world);
    (f32::from(x) * 8.0, f32::from(y) * 8.0)
}

/// Return a snapshot of every core owned by `team`, preserving registration order.
/// Legacy callers that construct only `cores` remain supported.
pub(crate) fn team_core_snapshot(world: &DynamicWorld, team: u8) -> Vec<TeamCore> {
    world
        .team_core_lists
        .get(&team)
        .map(|cores| cores.clone())
        .or_else(|| world.cores.get(&team).map(|core| vec![*core]))
        .unwrap_or_default()
}

/// Register a core without dropping another core owned by the same team.
pub(crate) fn register_team_core(world: &DynamicWorld, team: u8, core: TeamCore) {
    let mut entry = world.team_core_lists.entry(team).or_default();
    if let Some(existing) = entry
        .iter_mut()
        .find(|existing| existing.position == core.position)
    {
        *existing = core;
    } else {
        entry.push(core);
    }
    let representative = entry.first().copied();
    drop(entry);
    if let Some(representative) = representative {
        world.cores.insert(team, representative);
    }
}

/// Remove one core from a team. The remaining first core becomes the legacy
/// representative; unlike the old port this does not eliminate the team.
pub(crate) fn unregister_team_core(
    world: &DynamicWorld,
    team: u8,
    position: i32,
) -> Option<TeamCore> {
    let (removed, representative, empty) =
        if let Some(mut entry) = world.team_core_lists.get_mut(&team) {
            let removed = entry
                .iter()
                .position(|core| core.position == position)
                .map(|index| entry.remove(index));
            let representative = entry.first().copied();
            let empty = entry.is_empty();
            (removed, representative, empty)
        } else {
            (
                world
                    .cores
                    .get(&team)
                    .filter(|core| core.position == position)
                    .map(|core| *core),
                None,
                true,
            )
        };
    if empty {
        world.team_core_lists.remove(&team);
        world.cores.remove(&team);
    } else if let Some(representative) = representative {
        world.cores.insert(team, representative);
    }
    removed
}

pub(crate) fn core_position_for_team(world: &DynamicWorld, team: u8) -> i32 {
    team_core_snapshot(world, team)
        .first()
        .map(|core| core.position)
        .unwrap_or(world.core_position)
}

pub(crate) fn core_tile_for_team(world: &DynamicWorld, team: u8) -> (i16, i16) {
    let position = core_position_for_team(world, team);
    ((position >> 16) as i16, position as i16)
}

pub(crate) fn core_world_for_team(world: &DynamicWorld, team: u8) -> (f32, f32) {
    let (x, y) = core_tile_for_team(world, team);
    (f32::from(x) * 8.0, f32::from(y) * 8.0)
}

/// The legacy team-1 health mirror remains authoritative for old economy paths.
pub(crate) fn core_health_for_team(world: &DynamicWorld, team: u8) -> f32 {
    if team == 1 {
        return *world.game_state.core_health.read();
    }
    team_core_snapshot(world, team)
        .first()
        .map(|core| core.health)
        .unwrap_or_else(|| *world.game_state.core_health.read())
}

pub(crate) fn core_max_health_for_team(world: &DynamicWorld, team: u8) -> f32 {
    team_core_snapshot(world, team)
        .first()
        .map(|core| core.max_health)
        .unwrap_or(world.core_max_health)
}

/// Snapshot of teams that own at least one registered core, ascending.
pub(crate) fn registered_core_teams(world: &DynamicWorld) -> Vec<u8> {
    let mut teams: Vec<u8> = world
        .team_core_lists
        .iter()
        .map(|entry| *entry.key())
        .collect();
    for entry in world.cores.iter() {
        if !teams.contains(entry.key()) {
            teams.push(*entry.key());
        }
    }
    teams.sort_unstable();
    teams
}

pub(crate) fn core_team_at_position(world: &DynamicWorld, position: i32) -> Option<u8> {
    for entry in world.team_core_lists.iter() {
        if entry.value().iter().any(|core| core.position == position) {
            return Some(*entry.key());
        }
    }
    world
        .cores
        .iter()
        .find(|entry| entry.value().position == position)
        .map(|entry| *entry.key())
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct EnemyUnit {
    pub id: i32,
    pub unit_type: i16,
    pub entity_class: u8,
    #[serde(default = "enemy_team")]
    pub team: u8,
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub health: f32,
    pub shield: f32,
    #[serde(default = "no_item")]
    pub status_effect: i16,
    #[serde(default = "permanent_status_duration")]
    pub status_duration: f32,
    /// P0-06: authoritative `StatusEntry` collection (official `Unit.statuses`).
    /// `status_effect`/`status_duration` remain the compatibility view of
    /// the FIRST entry. The wire still emits `i count + [s id + f dur]`.
    #[serde(default)]
    pub statuses: Vec<crate::game::status::ActiveStatus>,
    pub velocity_x: f32,
    pub velocity_y: f32,
    #[serde(default)]
    pub elevation: f32,
    #[serde(default)]
    pub payloads: Vec<CarriedPayload>,
    /// Logic flag (setflag via ucontrol flag; read by radar/sensor).
    #[serde(default)]
    pub flag: f64,
    /// Carried items (item id, amount) for logic miners (ucontrol mine).
    #[serde(default)]
    pub items: Vec<(i16, i32)>,
    /// Mining progress accumulator (ucontrol mine).
    #[serde(default)]
    pub mine_progress: f32,
    pub attack_reload: f32,
    #[serde(default)]
    pub secondary_attack_reload: f32,
    #[serde(default)]
    pub tertiary_attack_reload: f32,
    #[serde(default)]
    pub quaternary_attack_reload: f32,
    pub move_speed: f32,
    pub attack_damage: f32,
    pub attack_reload_time: f32,
    pub attack_range: f32,
    /// P0-01: who holds authority over this unit's controller
    /// ([`UnitAuthority`]); `unit_orders` alone never decides ownership.
    /// Defaults for JSON checkpoints written before the field existed.
    #[serde(default)]
    pub authority: UnitAuthority,
    /// P2-04: per-unit `BuilderComp.plans` (ConstructBuild queue).
    #[serde(default)]
    pub build_plans: Vec<UnitBuildPlan>,
    /// Official `StatusComp` transients — refreshed in `tick_statuses` only.
    #[serde(skip, default)]
    pub status_agg: Option<crate::game::status::StatusAggregate>,
    /// Official `@SyncLocal boolean updateBuilding` (default true).
    /// ClientSnapshot overwrites it for possessed units; disconnect clears it.
    #[serde(default = "default_update_building")]
    pub update_building: bool,
}

pub(crate) const fn default_update_building() -> bool {
    true
}

/// One entry of a unit's `BuilderComp.plans` queue (place or break).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct UnitBuildPlan {
    pub breaking: bool,
    pub position: i32,
    pub block: i16,
    pub rotation: u8,
    #[serde(default)]
    pub config: Vec<u8>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum CarriedPayload {
    /// Untagged keeps old saves containing raw EnemyUnit objects readable.
    Unit(EnemyUnit),
    Build(CarriedBuildPayload),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct CarriedBuildPayload {
    pub tile: DynamicTile,
    pub version: u8,
    pub sync: Vec<u8>,
}

pub(crate) fn enemy_team() -> u8 {
    2
}

#[derive(Clone, Debug)]
pub(crate) struct Projectile {
    pub(crate) target_id: i32,
    /// Id of the unit that fired the projectile (used by allied SAP lifesteal).
    /// -1 for building-fired projectiles (turrets, alpha), which never lifesteal.
    pub(crate) shooter_id: i32,
    pub(crate) team: u8,
    pub(crate) bullet_id: i16,
    pub(crate) damage: f32,
    pub(crate) splash_damage: f32,
    pub(crate) splash_radius: f32,
    pub(crate) status_effect: i16,
    pub(crate) status_duration: f32,
    pub(crate) pierce_units: u8,
    pub(crate) pierce_buildings: u8,
    pub(crate) spawn_reign_frags: bool,
    pub(crate) homing_range: f32,
    pub(crate) enemy_target_position: Option<i32>,
    pub(crate) enemy_target_core: bool,
    pub(crate) apply_direct_on_impact: bool,
    pub(crate) armor_multiplier: f32,
    pub(crate) remaining_ticks: f32,
    pub(crate) total_ticks: f32,
    pub(crate) source_x: f32,
    pub(crate) source_y: f32,
    pub(crate) target_x: f32,
    pub(crate) target_y: f32,
    pub(crate) lifetime_scale: f32,
    pub(crate) source_position: Option<i32>,
    pub(crate) damage_interval: Option<f32>,
    pub(crate) damage_timer: f32,
}

#[derive(Clone, Debug)]
pub struct PendingBuild {
    pub(crate) position: i32,
    pub(crate) block: i16,
    pub(crate) rotation: u8,
    pub(crate) config: Vec<u8>,
    pub(crate) occupied: Vec<i32>,
    /// Team of the player who placed the plan (official plans belong to the
    /// player's team: `TeamData.plans`). Consumed requirements and the
    /// finished tile use this team; survival/attack is always 1.
    pub(crate) team: u8,
    pub(crate) builder: SessionPlayer,
    pub(crate) last_seen: std::time::Instant,
    pub(crate) assist_progress: f32,
    /// Remaining construction work in game ticks (SOL-003: advanced by the
    /// world loop at `delta` per tick, respecting pause and --tps, instead
    /// of wall-clock tokio timers).
    pub(crate) remaining_ticks: f32,
    /// assist_progress already applied toward remaining_ticks.
    pub(crate) applied_assist: f32,
}

#[derive(Clone, Debug)]
pub struct PendingBreak {
    pub(crate) position: i32,
    pub(crate) block: i16,
    pub(crate) occupied: Vec<i32>,
    pub(crate) dynamic: bool,
    /// Team that started the break. This is stable across mode/PvP team
    /// reassignment and owns both the plan removal and any eventual refund.
    pub(crate) team: u8,
    pub(crate) builder: SessionPlayer,
    pub(crate) last_seen: std::time::Instant,
    /// Remaining deconstruction work in game ticks (advanced by the loop).
    pub(crate) remaining_ticks: f32,
}

#[derive(Clone)]
pub struct PendingConnection {
    pub ip: IpAddr,
    /// Bounded server->client frame queue (P0-9): a slow consumer fills the
    /// queue, drops are counted, and the connection is torn down past the
    /// drop limit instead of letting the queue grow without bound.
    pub outbound: tokio::sync::mpsc::Sender<Vec<u8>>,
    pub udp_inbound: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    pub udp_endpoint: Arc<parking_lot::RwLock<Option<SocketAddr>>>,
    /// Shared server UDP socket for unreliable ArcNet sends (P0-12). Tests
    /// leave this `None` so routing falls back to the TCP outbound queue.
    /// `std::net` is used so simulation/broadcast can `send_to` without a
    /// Tokio poll (tokio `try_send_to` WouldBlocks until the socket is polled).
    pub udp_socket: Option<Arc<std::net::UdpSocket>>,
    pub player_name: Arc<parking_lot::RwLock<Option<String>>>,
    /// Total frames dropped because the outbound queue was full.
    pub outbound_drops: Arc<std::sync::atomic::AtomicU64>,
    /// Dropped frames that were marked critical (kicks, world restreams).
    pub critical_drops: Arc<std::sync::atomic::AtomicU64>,
    /// Round 74d develop: last measured framework keepalive RTT in ms
    /// (0 = never measured).
    pub last_keepalive_rtt_ms: Arc<std::sync::atomic::AtomicU64>,
    /// Round 74d develop: wall-clock millis of the last packet received
    /// from this client (0 = nothing yet).
    pub last_packet_epoch_ms: Arc<std::sync::atomic::AtomicU64>,
    /// Round 74g develop: frames currently queued in the bounded outbound
    /// channel (enqueue increments, the session's send loop decrements).
    pub outbound_queued: Arc<std::sync::atomic::AtomicU64>,
}

#[derive(Debug)]
pub enum RuntimeCommand {
    Save(String),
    /// Exports the current world as an official .msav save (v11).
    SaveMsav(String),
    Load(String),
    Kick(String),
    Say(String),
    GameOver,
    SpawnEnemy {
        unit: String,
        count: u32,
        x: Option<i16>,
        y: Option<i16>,
    },
    /// Hot-swaps the active map: builds a fresh world from the resolved MSAV,
    /// atomically replaces the shared world, persists the new state and
    /// re-streams every connected player (WorldDataBegin + new world stream).
    HostMap {
        map: String,
        mode: String,
    },
    /// Console `team <player> <team>`: assigns a connected player to a team
    /// (name or numeric id) and broadcasts the updated player snapshot.
    SetTeam {
        player: String,
        team: String,
    },
    /// Console `nextmap`: immediately advances the map rotation.
    NextMap,
    /// Console `pause` / `resume`: toggles `GameState.is_paused` directly
    /// (a manual pause is not overridden by the PvP auto-pause logic).
    Pause(bool),
    /// Console `mode <mode>`: switches GameMode and reconfigures world-level
    /// PvP state (re-assigns teams to connected players when entering PvP).
    SetMode {
        mode: String,
    },
}

#[derive(Clone)]
pub struct NetworkControl {
    pub(crate) sender: tokio::sync::mpsc::UnboundedSender<RuntimeCommand>,
}

impl NetworkControl {
    pub fn send(&self, command: RuntimeCommand) {
        if self.sender.send(command).is_err() {
            warn!("Network runtime is not available");
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub description: String,
    pub build: i32,
    pub version_type: String,
}

impl Default for ServerInfo {
    fn default() -> Self {
        Self {
            name: "Oxide".into(),
            description: "Mindustry server written in Rust".into(),
            build: crate::compat_target::CURRENT_PROTOCOL_BUILD,
            version_type: "official".into(),
        }
    }
}
