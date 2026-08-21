#![allow(dead_code)]

use dashmap::{DashMap, DashSet};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

/// A connected player tracked by the console for `players`, `ban <name>` and
/// `admin add <name>`. The network listener (orchestrator integration)
/// registers players on ConnectConfirm and unregisters them on teardown; the
/// console layer only ever reads this registry (see audit-reports/console_impl.md
/// for the exact listener.rs call sites).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedPlayer {
    pub uuid: String,
    pub name: String,
    pub ip: String,
    pub player_id: i32,
    pub unit_id: i32,
}

/// On-disk representation of the administration state. `#[serde(default)]`
/// keeps older/partial files loadable.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PersistedAdminData {
    banned_ips: Vec<String>,
    banned_uuids: Vec<String>,
    #[serde(default)]
    banned_names: Vec<String>,
    dos_banned_ips: Vec<String>,
    subnet_bans: Vec<String>,
    whitelist: Vec<String>,
    whitelist_enabled: bool,
    admins: Vec<String>,
    player_limit: u32,
    server_name: String,
    server_description: String,
    server_build: i32,
    server_version_type: String,
    // --- SOL-012 operational model (Java ServerControl parity) ---
    // Field-level #[serde(default)] keeps files written by older builds (which
    // lack these keys) loadable; the struct-level #[serde(default)] would
    // cover them too, this documents the intent. The interval/wait fields use
    // dedicated default fns because their official defaults are not 0.
    #[serde(default = "default_autosave_interval_secs")]
    autosave_interval_secs: u32,
    #[serde(default)]
    map_list: Vec<String>,
    #[serde(default)]
    map_index: usize,
    #[serde(default = "default_round_wait_ticks")]
    round_wait_ticks: u32,
    #[serde(default)]
    rules_overrides: HashMap<String, serde_json::Value>,
    #[serde(default = "default_shuffle_mode")]
    shuffle_mode: String,
    /// M4: kick cooldowns persisted as millis-since-epoch deadlines
    /// (official `Administration.kickedIPs` + `PlayerInfo.lastKicked`,
    /// saved via `NetConnection.kick` -> `admins.save()`).
    #[serde(default)]
    kicked_ips: Vec<(String, u64)>,
    #[serde(default)]
    last_kicked: Vec<(String, u64)>,
}

fn default_shuffle_mode() -> String {
    "none".to_string()
}

/// Official `Config.autosaveSpacing` default: `60 * 5` = 300 seconds
/// (core/src/mindustry/net/Administration.java).
fn default_autosave_interval_secs() -> u32 {
    300
}

/// Official `Config.roundExtraTime` default: 12 s = 720 ticks at 60 tps
/// (ServerControl.java gameOverListener).
fn default_round_wait_ticks() -> u32 {
    720
}

/// Admin policy and blacklist/whitelist manager with JSON persistence.
///
/// Every mutating call snapshots the DashMaps/DashSets *before* writing the
/// file (project rule: never iterate a DashMap while holding a mut guard on
/// the same map). The persistence file defaults to `admin-data.json` next to
/// the process working directory; `with_file`/`set_file` point it at the save
/// directory (orchestrator: `config.save_file.with_extension("admin.json")`).
#[derive(Clone)]
pub struct Administration {
    pub banned_ips: Arc<DashSet<String>>,
    pub banned_uuids: Arc<DashSet<String>>,
    /// Official `Administration.bannedNames` (name bans; the connect check
    /// `admins.isNameBanned(packet.name)` kicks with `KickReason.banned`).
    pub banned_names: Arc<DashSet<String>>,
    pub dos_banned_ips: Arc<DashSet<String>>,
    pub subnet_bans: Arc<DashSet<String>>,
    pub whitelist: Arc<DashSet<String>>,
    pub whitelist_enabled: Arc<AtomicBool>,
    pub admins: Arc<DashSet<String>>,
    pub player_limit: Arc<AtomicU32>,
    /// Connected players keyed by uuid (populated by listener.rs).
    pub connected_players: Arc<DashMap<String, ConnectedPlayer>>,
    file: Arc<RwLock<PathBuf>>,
    server_name: Arc<RwLock<String>>,
    server_description: Arc<RwLock<String>>,
    server_build: Arc<RwLock<i32>>,
    server_version_type: Arc<RwLock<String>>,
    // --- SOL-012 operational model (Java ServerControl parity) ---
    /// Autosave spacing in seconds. Official default: `Config.autosaveSpacing`
    /// = `60 * 5` = 300 (`core/src/mindustry/net/Administration.java:539`,
    /// described as "Spacing between autosaves in seconds"). Note the Java
    /// update loop feeds arc's `Interval` (seconds) with `spacing * 60`
    /// (ServerControl.java:230), so the *effective* official period is
    /// 300 * 60 s; the stored value here keeps the documented config default.
    autosave_interval_secs: Arc<RwLock<u32>>,
    /// Ordered rotation list of map names (listener.rs populates it from the
    /// map folder; `advance_map` cycles it with wrap-around).
    map_list: Arc<RwLock<Vec<String>>>,
    /// Current position in `map_list`; persisted so rotation survives restarts.
    map_index: Arc<RwLock<usize>>,
    /// Game-over wait before loading the next map, in game ticks. Official
    /// default: `Config.roundExtraTime` = 12 s (ServerControl.java:94),
    /// i.e. 720 ticks at 60 tps. Zero is a valid value (no wait).
    round_wait_ticks: Arc<RwLock<u32>>,
    /// Global rule overrides applied on world load regardless of map
    /// (official `rules` command backed by `Core.settings` "globalrules").
    rules_overrides: Arc<RwLock<HashMap<String, serde_json::Value>>>,
    /// Per-IP kick cooldowns (official `Administration.kickedIPs`):
    /// reconnects before the stored instant are rejected with recentKick.
    /// M4: per-UUID kick cooldowns (official `PlayerInfo.lastKicked`),
    /// consulted together with `kicked_ips` by `kick_time(uuid, ip)`.
    kicked_ips: Arc<DashMap<String, std::time::Instant>>,
    last_kicked: Arc<DashMap<String, std::time::Instant>>,
    /// Map shuffle mode (official `shuffle` command: none/all/custom/
    /// builtin). `all` picks a random next map; custom/builtin are stored
    /// but behave like none (no custom/builtin pools in this port).
    shuffle_mode: Arc<RwLock<String>>,
    /// Registered action filters (official `Administration.actionFilters`),
    /// run in order by `allow_action`.
    action_filters: Arc<RwLock<Vec<ActionFilter>>>,
    /// Per-player anti-spam interaction rate state (official
    /// `PlayerInfo.rate`), keyed by uuid.
    interact_rates: Arc<DashMap<String, InteractRateState>>,
    /// M11: anti-spam kick requests (uuid -> kick-until instant) queued by
    /// the rate filter; consumed by the session after a rejected action.
    pending_kicks: Arc<DashMap<String, std::time::Instant>>,
    /// M11: anti-spam warning requests (uuid) consumed by the session.
    pending_warnings: Arc<DashMap<String, ()>>,
}

impl Default for Administration {
    fn default() -> Self {
        Self::new()
    }
}

impl Administration {
    pub fn new() -> Self {
        Self::with_file(PathBuf::from("admin-data.json"))
    }

    /// Creates the manager and loads persisted state from `path` when present.
    pub fn with_file(path: PathBuf) -> Self {
        let admin = Self {
            banned_ips: Arc::new(DashSet::new()),
            banned_uuids: Arc::new(DashSet::new()),
            banned_names: Arc::new(DashSet::new()),
            dos_banned_ips: Arc::new(DashSet::new()),
            subnet_bans: Arc::new(DashSet::new()),
            whitelist: Arc::new(DashSet::new()),
            whitelist_enabled: Arc::new(AtomicBool::new(false)),
            admins: Arc::new(DashSet::new()),
            player_limit: Arc::new(AtomicU32::new(0)),
            connected_players: Arc::new(DashMap::new()),
            file: Arc::new(RwLock::new(path)),
            server_name: Arc::new(RwLock::new("Oxide".to_string())),
            server_description: Arc::new(RwLock::new(
                "Mindustry server written in Rust".to_string(),
            )),
            server_build: Arc::new(RwLock::new(crate::compat_target::CURRENT_PROTOCOL_BUILD)),
            server_version_type: Arc::new(RwLock::new("official".to_string())),
            autosave_interval_secs: Arc::new(RwLock::new(300)),
            map_list: Arc::new(RwLock::new(Vec::new())),
            map_index: Arc::new(RwLock::new(0)),
            round_wait_ticks: Arc::new(RwLock::new(720)),
            rules_overrides: Arc::new(RwLock::new(HashMap::new())),
            action_filters: Arc::new(RwLock::new(Vec::new())),
            interact_rates: Arc::new(DashMap::new()),
            kicked_ips: Arc::new(DashMap::new()),
            last_kicked: Arc::new(DashMap::new()),
            pending_kicks: Arc::new(DashMap::new()),
            pending_warnings: Arc::new(DashMap::new()),
            shuffle_mode: Arc::new(RwLock::new("none".to_string())),
        };
        admin.install_default_action_filters();
        admin.load();
        admin
    }

    /// Installs the official built-in action filters (Administration.java
    /// `lambda$new$1`, desktop 158.1 bytecode offsets 0-147): the anti-spam
    /// interact rate limit for non-break/place/commandUnits actions
    /// (Config.antiSpam=true on headless, window 6 s, limit 25, kick at 60
    /// occurrences). The official filter does NOT exempt admins: it only
    /// checks `player.isLocal()`, which is false for every remote player in
    /// headless (including admins).
    fn install_default_action_filters(&self) {
        let rates = self.interact_rates.clone();
        let kicks = self.pending_kicks.clone();
        let warnings = self.pending_warnings.clone();
        self.add_action_filter(Arc::new(move |action: &PlayerAction| {
            // Official filter: only non-break/place/commandUnits actions are
            // rate-limited; NO admin exemption (isLocal() is false for all
            // remote players, admins included).
            if matches!(
                action.action_type,
                ActionType::BreakBlock | ActionType::PlaceBlock | ActionType::CommandUnits
            ) {
                return true;
            }
            let now = std::time::Instant::now();
            let mut state = rates
                .entry(action.player_uuid.clone())
                .or_insert_with(InteractRateState::new);
            let elapsed = now.duration_since(state.window_start).as_millis();
            if elapsed >= INTERACT_RATE_WINDOW_MILLIS {
                // New window: reset occurrences (official Ratekeeper window
                // rollover).
                state.window_start = now;
                state.occurrences = 0;
                state.kicked = false;
                state.warned_at = None;
            }
            state.occurrences += 1;
            // Official Ratekeeper.allow(window, 25): first 25 allowed.
            if state.occurrences <= INTERACT_RATE_LIMIT {
                return true;
            }
            // Over the limit: kick at >60 occurrences for 30 s
            // (`player.kick("You are interacting with too many blocks.",
            // 30000)`), otherwise warn every 120 s.
            if state.occurrences > INTERACT_RATE_KICK {
                if !state.kicked {
                    state.kicked = true;
                    kicks.insert(action.player_uuid.clone(), now + KICK_DURATION);
                }
            } else if state
                .warned_at
                .map(|warned| now.duration_since(warned) >= WARNING_INTERVAL)
                .unwrap_or(true)
            {
                state.warned_at = Some(now);
                warnings.insert(action.player_uuid.clone(), ());
            }
            false
        }));
    }

    /// Re-points the persistence file (used by main.rs to store admin data
    /// next to the world save) and reloads state from it.
    pub fn set_file(&self, path: PathBuf) {
        *self.file.write() = path;
        self.load();
    }

    pub fn file_path(&self) -> PathBuf {
        self.file.read().clone()
    }

    // --- connected player registry (orchestrator integration) ---

    pub fn register_connection(&self, player: ConnectedPlayer) {
        self.connected_players.insert(player.uuid.clone(), player);
    }

    pub fn unregister_connection(&self, uuid: &str) {
        self.connected_players.remove(uuid);
    }

    pub fn clear_connections(&self) {
        self.connected_players.clear();
    }

    /// Snapshot of all connected players (never holds a DashMap guard across
    /// the caller's use of the returned vector).
    pub fn connected_players_list(&self) -> Vec<ConnectedPlayer> {
        let mut players: Vec<_> = self
            .connected_players
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        players.sort_by(|left, right| left.name.cmp(&right.name));
        players
    }

    pub fn find_connected_by_name(&self, name: &str) -> Option<ConnectedPlayer> {
        let wanted = name.to_lowercase();
        self.connected_players
            .iter()
            .find(|entry| entry.value().name.to_lowercase() == wanted)
            .map(|entry| entry.value().clone())
    }

    pub fn find_connected_by_uuid(&self, uuid: &str) -> Option<ConnectedPlayer> {
        self.connected_players.get(uuid).map(|entry| entry.clone())
    }

    // --- bans ---

    pub fn is_banned(&self, ip: &str, uuid: &str) -> bool {
        self.banned_ips.contains(ip)
            || self.banned_uuids.contains(uuid)
            || self.dos_banned_ips.contains(ip)
    }

    pub fn ban_ip(&self, ip: &str) {
        self.banned_ips.insert(ip.to_string());
        self.persist_quiet();
        info!(target: "admin", "Banned IP: {}", ip);
    }

    pub fn pardon_ip(&self, ip: &str) -> bool {
        let removed = self.banned_ips.remove(ip).is_some();
        if removed {
            self.persist_quiet();
            info!(target: "admin", "Pardoned IP: {}", ip);
        }
        removed
    }

    pub fn ban_uuid(&self, uuid: &str) {
        self.banned_uuids.insert(uuid.to_string());
        self.persist_quiet();
        info!(target: "admin", "Banned UUID: {}", uuid);
    }

    pub fn pardon_uuid(&self, uuid: &str) -> bool {
        let removed = self.banned_uuids.remove(uuid).is_some();
        if removed {
            self.persist_quiet();
            info!(target: "admin", "Pardoned UUID: {}", uuid);
        }
        removed
    }

    pub fn banned_ips_list(&self) -> Vec<String> {
        let mut values: Vec<_> = self.banned_ips.iter().map(|v| v.clone()).collect();
        values.sort();
        values
    }

    pub fn banned_uuids_list(&self) -> Vec<String> {
        let mut values: Vec<_> = self.banned_uuids.iter().map(|v| v.clone()).collect();
        values.sort();
        values
    }

    /// Official `Administration.isNameBanned` (Administration.java): a name
    /// ban is a normalized (trimmed, case-insensitive) substring match.
    pub fn is_name_banned(&self, name: &str) -> bool {
        let normalized = name.trim().to_ascii_lowercase();
        self.banned_names
            .iter()
            .any(|banned| normalized.contains(&banned.to_ascii_lowercase()))
    }

    pub fn ban_name(&self, name: &str) -> bool {
        let normalized = name.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return false;
        }
        self.banned_names.insert(normalized.clone());
        self.persist_quiet();
        true
    }

    pub fn pardon_name(&self, name: &str) -> bool {
        let normalized = name.trim().to_ascii_lowercase();
        let removed = self.banned_names.remove(&normalized);
        if removed.is_some() {
            self.persist_quiet();
        }
        removed.is_some()
    }

    pub fn banned_names_list(&self) -> Vec<String> {
        let mut values: Vec<_> = self.banned_names.iter().map(|v| v.clone()).collect();
        values.sort();
        values
    }

    /// Official `Administration.getKickTime(uuid, ip)` (JAR bytecode) —
    /// `max(PlayerInfo.lastKicked, kickedIPs[ip])`: the earliest wall time
    /// a connection with this ip/uuid may join again.
    pub fn kick_time(&self, uuid: &str, ip: &str) -> Option<std::time::Instant> {
        let by_ip = self.kicked_ips.get(ip).map(|entry| *entry.value());
        let by_uuid = self.last_kicked.get(uuid).map(|entry| *entry.value());
        match (by_ip, by_uuid) {
            (Some(ip), Some(uuid)) => Some(ip.max(uuid)),
            (Some(ip), None) => Some(ip),
            (None, Some(uuid)) => Some(uuid),
            (None, None) => None,
        }
    }

    /// Official `Administration.handleKicked(uuid, ip, duration)` (JAR
    /// bytecode offsets 0-60): `kickedIPs[ip] = max(existing,
    /// Time.millis()+duration)` and `PlayerInfo.lastKicked = ...`; the
    /// official NetConnection.kick only calls this when duration > 0.
    /// Kicked state is persisted (the JAR calls `admins.save()` on kick).
    pub fn handle_kicked(&self, uuid: &str, ip: &str, duration: std::time::Duration) {
        if duration.is_zero() {
            return; // official: `if (duration > 0) handleKicked(...)`
        }
        let until = std::time::Instant::now() + duration;
        if let Some(mut entry) = self.kicked_ips.get_mut(ip) {
            if *entry > until {
                return;
            }
            *entry = until;
        } else {
            self.kicked_ips.insert(ip.to_string(), until);
        }
        self.last_kicked.insert(uuid.to_string(), until);
        self.persist_quiet();
    }

    /// Removes an expired kick cooldown (called on connect when the time
    /// has passed; also keeps the maps bounded).
    pub fn clear_kick_time(&self, uuid: &str, ip: &str) {
        if let Some(until) = self.kicked_ips.get(ip).map(|t| *t) {
            if until <= std::time::Instant::now() {
                self.kicked_ips.remove(ip);
            }
        }
        if let Some(until) = self.last_kicked.get(uuid).map(|t| *t) {
            if until <= std::time::Instant::now() {
                self.last_kicked.remove(uuid);
            }
        }
    }

    /// M11: consumes a pending anti-spam kick request (uuid -> kick-until).
    pub fn take_pending_kick(&self, uuid: &str) -> Option<std::time::Instant> {
        self.pending_kicks.remove(uuid).map(|(_, until)| until)
    }

    /// M11: consumes a pending anti-spam warning request.
    pub fn take_pending_warning(&self, uuid: &str) -> bool {
        self.pending_warnings.remove(uuid).is_some()
    }

    // --- dos bans ---

    pub fn is_dos_blacklisted(&self, ip: &str) -> bool {
        self.dos_banned_ips.contains(ip)
    }

    pub fn add_dos_ban(&self, ip: &str) {
        self.dos_banned_ips.insert(ip.to_string());
        self.persist_quiet();
        info!(target: "admin", "Added DOS ban for IP: {}", ip);
    }

    pub fn remove_dos_ban(&self, ip: &str) -> bool {
        let removed = self.dos_banned_ips.remove(ip).is_some();
        if removed {
            self.persist_quiet();
            info!(target: "admin", "Removed DOS ban for IP: {}", ip);
        }
        removed
    }

    pub fn dos_banned_ips_list(&self) -> Vec<String> {
        let mut values: Vec<_> = self.dos_banned_ips.iter().map(|v| v.clone()).collect();
        values.sort();
        values
    }

    // --- subnet bans (official: rejects IPs starting with the prefix) ---

    pub fn is_subnet_banned(&self, ip: &str) -> bool {
        self.subnet_bans
            .iter()
            .any(|subnet| ip.starts_with(subnet.as_str()))
    }

    pub fn add_subnet_ban(&self, subnet: &str) {
        self.subnet_bans.insert(subnet.to_string());
        self.persist_quiet();
        info!(target: "admin", "Banned subnet: {}", subnet);
    }

    pub fn remove_subnet_ban(&self, subnet: &str) -> bool {
        let removed = self.subnet_bans.remove(subnet).is_some();
        if removed {
            self.persist_quiet();
            info!(target: "admin", "Unbanned subnet: {}", subnet);
        }
        removed
    }

    pub fn subnet_bans_list(&self) -> Vec<String> {
        let mut values: Vec<_> = self.subnet_bans.iter().map(|v| v.clone()).collect();
        values.sort();
        values
    }

    // --- whitelist ---

    /// Official NetServer semantics: the whitelist only filters when enabled.
    pub fn is_whitelisted(&self, uuid: &str) -> bool {
        !self.is_whitelist_enabled() || self.whitelist.contains(uuid)
    }

    pub fn is_whitelist_enabled(&self) -> bool {
        self.whitelist_enabled.load(Ordering::Relaxed)
    }

    pub fn set_whitelist_enabled(&self, enabled: bool) {
        self.whitelist_enabled.store(enabled, Ordering::Relaxed);
        self.persist_quiet();
        info!(target: "admin", "Whitelist {}", if enabled { "enabled" } else { "disabled" });
    }

    pub fn whitelist_add(&self, uuid: &str) -> bool {
        let inserted = self.whitelist.insert(uuid.to_string());
        if inserted {
            self.persist_quiet();
            info!(target: "admin", "Whitelisted UUID: {}", uuid);
        }
        inserted
    }

    pub fn whitelist_remove(&self, uuid: &str) -> bool {
        let removed = self.whitelist.remove(uuid).is_some();
        if removed {
            self.persist_quiet();
            info!(target: "admin", "Un-whitelisted UUID: {}", uuid);
        }
        removed
    }

    pub fn whitelist_list(&self) -> Vec<String> {
        let mut values: Vec<_> = self.whitelist.iter().map(|v| v.clone()).collect();
        values.sort();
        values
    }

    // --- admins ---

    pub fn is_admin(&self, uuid: &str) -> bool {
        self.admins.contains(uuid)
    }

    pub fn add_admin(&self, uuid: &str) -> bool {
        let inserted = self.admins.insert(uuid.to_string());
        if inserted {
            self.persist_quiet();
            info!(target: "admin", "Admin added: {}", uuid);
        }
        inserted
    }

    pub fn remove_admin(&self, uuid: &str) -> bool {
        let removed = self.admins.remove(uuid).is_some();
        if removed {
            self.persist_quiet();
            info!(target: "admin", "Admin removed: {}", uuid);
        }
        removed
    }

    pub fn admins_list(&self) -> Vec<String> {
        let mut values: Vec<_> = self.admins.iter().map(|v| v.clone()).collect();
        values.sort();
        values
    }

    // --- player limit ---

    pub fn set_player_limit(&self, limit: u32) {
        self.player_limit.store(limit, Ordering::Relaxed);
        self.persist_quiet();
    }

    pub fn get_player_limit(&self) -> u32 {
        self.player_limit.load(Ordering::Relaxed)
    }

    /// Official ConnectPacket check: limit reached unless the uuid is admin
    /// (`NetServer.java:205`: `limit > 0 && players >= limit && !isAdmin`).
    pub fn is_at_player_limit(&self, current_players: u32, uuid: &str) -> bool {
        let limit = self.get_player_limit();
        limit > 0 && current_players >= limit && !self.is_admin(uuid)
    }

    // --- runtime config overrides (config command / discovery parity) ---

    pub fn server_name(&self) -> String {
        self.server_name.read().clone()
    }

    pub fn set_server_name(&self, name: &str) {
        *self.server_name.write() = name.to_string();
        self.persist_quiet();
        info!(target: "admin", "Server name set to: {}", name);
    }

    pub fn server_description(&self) -> String {
        self.server_description.read().clone()
    }

    pub fn set_server_description(&self, description: &str) {
        *self.server_description.write() = description.to_string();
        self.persist_quiet();
        info!(target: "admin", "Server description set to: {}", description);
    }

    pub fn server_build(&self) -> i32 {
        *self.server_build.read()
    }

    pub fn set_server_build(&self, build: i32) {
        *self.server_build.write() = build;
        self.persist_quiet();
        info!(target: "admin", "Server build set to: {}", build);
    }

    pub fn server_version_type(&self) -> String {
        self.server_version_type.read().clone()
    }

    pub fn set_server_version_type(&self, version_type: &str) {
        *self.server_version_type.write() = version_type.to_string();
        self.persist_quiet();
        info!(target: "admin", "Server version type set to: {}", version_type);
    }

    // --- autosave, map rotation and rules overrides (SOL-012) ---

    pub fn autosave_interval_secs(&self) -> u32 {
        *self.autosave_interval_secs.read()
    }

    pub fn set_autosave_interval_secs(&self, secs: u32) {
        *self.autosave_interval_secs.write() = secs;
        self.persist_quiet();
        info!(target: "admin", "Autosave interval set to {}s", secs);
    }

    /// Whether an autosave is due: `elapsed_secs` (measured by the caller,
    /// e.g. the game loop) has reached the configured interval. Pure check —
    /// the caller resets its own timer after triggering the save (Java
    /// parity: `autosaveCount.get(Config.autosaveSpacing.num() * 60)` in
    /// ServerControl.java:230). An interval of 0 means "always due" (zero is
    /// a valid value).
    pub fn autosave_due(&self, elapsed_secs: f64) -> bool {
        elapsed_secs >= self.autosave_interval_secs() as f64
    }

    pub fn map_list(&self) -> Vec<String> {
        self.map_list.read().clone()
    }

    pub fn set_map_list(&self, maps: Vec<String>) {
        let count = maps.len();
        *self.map_list.write() = maps;
        self.persist_quiet();
        info!(target: "admin", "Map rotation set to {} maps", count);
    }

    pub fn map_index(&self) -> usize {
        *self.map_index.read()
    }

    pub fn set_map_index(&self, index: usize) {
        *self.map_index.write() = index;
        self.persist_quiet();
    }

    /// Returns the map name at the current rotation position and advances the
    /// position by one, wrapping around (sequential deterministic form of the
    /// official `Maps.getNextMap`/`shuffle` behavior). `None` when the list
    /// is empty. A stale persisted index (>= list length, e.g. after the map
    /// list shrank) is clamped to 0 first.
    pub fn advance_map(&self) -> Option<String> {
        let maps = self.map_list.read().clone();
        if maps.is_empty() {
            return None;
        }
        // Official shuffle=all picks a random map from the rotation.
        if self.shuffle_mode.read().eq_ignore_ascii_case("all") {
            let index = rand::random::<usize>() % maps.len();
            return Some(maps[index].clone());
        }
        let mut index = *self.map_index.read();
        if index >= maps.len() {
            index = 0;
        }
        let next = maps[index].clone();
        *self.map_index.write() = (index + 1) % maps.len();
        // The rotation position is persisted state: it must survive restarts.
        self.persist_quiet();
        Some(next)
    }

    pub fn round_wait_ticks(&self) -> u32 {
        *self.round_wait_ticks.read()
    }

    /// Official `shuffle [none/all/custom/builtin]`: stores and reports the
    /// shuffle mode (custom/builtin accepted but behave like none).
    pub fn set_shuffle_mode(&self, mode: &str) -> bool {
        let normalized = mode.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "none" | "all" | "custom" | "builtin") {
            *self.shuffle_mode.write() = normalized;
            self.persist_quiet();
            true
        } else {
            false
        }
    }

    pub fn shuffle_mode(&self) -> String {
        self.shuffle_mode.read().clone()
    }

    pub fn set_round_wait_ticks(&self, ticks: u32) {
        *self.round_wait_ticks.write() = ticks;
        self.persist_quiet();
        info!(target: "admin", "Round wait set to {} ticks", ticks);
    }

    /// Adds or replaces a global rule override (official `rules add <name>
    /// <value>`). `value` must already be parsed JSON — the console layer
    /// reports parse failures before reaching this method (Java prints
    /// "Error parsing rule JSON" in that case). The live `Rules` struct must
    /// be rebuilt from `rules_overrides_snapshot()` on world load so the
    /// override applies "regardless of map" (listener.rs wiring).
    pub fn apply_rules_override(&self, key: &str, value: serde_json::Value) -> Result<(), String> {
        let key = key.trim();
        if key.is_empty() {
            return Err("rule name must not be empty".to_string());
        }
        self.rules_overrides.write().insert(key.to_string(), value);
        self.persist_quiet();
        info!(target: "admin", "Rules override set: {}", key);
        Ok(())
    }

    /// Removes every global rule override (the official `rules remove <name>`
    /// per-key form is a console-layer loop over `rules_overrides_snapshot()`).
    pub fn clear_rules_overrides(&self) {
        self.rules_overrides.write().clear();
        self.persist_quiet();
        info!(target: "admin", "All rules overrides cleared");
    }

    /// Sorted snapshot of the current rules overrides (never holds the map
    /// guard across the caller's use).
    pub fn rules_overrides_snapshot(&self) -> Vec<(String, serde_json::Value)> {
        let mut values: Vec<_> = self
            .rules_overrides
            .read()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        values.sort_by(|left, right| left.0.cmp(&right.0));
        values
    }

    // SOL-012 REMAINING (needs listener.rs / console wiring; the state model
    // above is complete, these hooks are out of scope for this file):
    // - autosave timer hook: from the game loop call `autosave_due(elapsed)`
    //   each tick; on true, write the world save and reset the local timer.
    //   Honor `autosave_interval_secs` (Java: Trigger.update +
    //   autosaveCount.get(Config.autosaveSpacing * 60), gated by
    //   Config.autosave on/off — ServerControl.java:227-241).
    // - map rotation on game-over: in the GameOver path call `advance_map()`
    //   and schedule the returned map after `round_wait_ticks` (Java:
    //   gameOverListener -> maps.getNextMap + Config.roundExtraTime,
    //   ServerControl.java:80-97).
    // - console commands (ServerControl.java): `nextmap <name>` (set
    //   map_list/set_map_index or a one-shot override), `shuffle
    //   <none|all|custom|builtin>` (ShuffleMode), `roundtime <seconds>`,
    //   `rules [add/remove] <name> <value>` (parse the value as JSON, then
    //   apply_rules_override; broadcast `Call.setRules` on change),
    //   `autosave <on|off>` + `autosaveSpacing` config.
    // - apply `rules_overrides_snapshot()` to the live Rules struct on every
    //   world load (ServerControl `rules` command reads Core.settings
    //   "globalrules" and calls Call.setRules — ServerControl.java:567-616).

    // --- persistence ---

    /// Loads persisted state from the configured file (if it exists).
    pub fn load(&self) {
        let path = self.file_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) => {
                warn!("Could not read admin data {}: {}", path.display(), err);
                return;
            }
        };
        let data: PersistedAdminData = match serde_json::from_slice(&bytes) {
            Ok(data) => data,
            Err(err) => {
                warn!("Could not parse admin data {}: {}", path.display(), err);
                return;
            }
        };
        self.banned_ips.clear();
        for value in data.banned_ips {
            self.banned_ips.insert(value);
        }
        self.banned_uuids.clear();
        for value in data.banned_uuids {
            self.banned_uuids.insert(value);
        }
        self.banned_names.clear();
        for value in data.banned_names {
            self.banned_names.insert(value);
        }
        self.dos_banned_ips.clear();
        for value in data.dos_banned_ips {
            self.dos_banned_ips.insert(value);
        }
        self.subnet_bans.clear();
        for value in data.subnet_bans {
            self.subnet_bans.insert(value);
        }
        self.whitelist.clear();
        for value in data.whitelist {
            self.whitelist.insert(value);
        }
        self.whitelist_enabled
            .store(data.whitelist_enabled, Ordering::Relaxed);
        self.admins.clear();
        for value in data.admins {
            self.admins.insert(value);
        }
        self.player_limit
            .store(data.player_limit, Ordering::Relaxed);
        *self.server_name.write() = data.server_name;
        *self.server_description.write() = data.server_description;
        *self.server_build.write() = data.server_build;
        *self.server_version_type.write() = data.server_version_type;
        *self.autosave_interval_secs.write() = data.autosave_interval_secs;
        *self.map_list.write() = data.map_list;
        *self.map_index.write() = data.map_index;
        *self.round_wait_ticks.write() = data.round_wait_ticks;
        *self.rules_overrides.write() = data.rules_overrides;
        *self.shuffle_mode.write() = data.shuffle_mode;
        // M4: restore kick cooldowns (persisted as millis-since-epoch
        // deadlines; remaining wall time is reconstructed).
        self.kicked_ips.clear();
        self.last_kicked.clear();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        for (ip, until) in data.kicked_ips {
            let remaining = until.saturating_sub(now);
            if remaining > 0 {
                self.kicked_ips.insert(
                    ip,
                    std::time::Instant::now() + std::time::Duration::from_millis(remaining),
                );
            }
        }
        for (uuid, until) in data.last_kicked {
            let remaining = until.saturating_sub(now);
            if remaining > 0 {
                self.last_kicked.insert(
                    uuid,
                    std::time::Instant::now() + std::time::Duration::from_millis(remaining),
                );
            }
        }
        info!(target: "admin", "Loaded admin data from {}", path.display());
    }

    /// Atomically writes the current administration state (snapshot first).
    pub fn persist(&self) -> std::io::Result<()> {
        // Snapshot every DashSet before serializing (anti-deadlock rule).
        let data = PersistedAdminData {
            banned_ips: self.banned_ips_list(),
            banned_uuids: self.banned_uuids_list(),
            banned_names: self.banned_names_list(),
            dos_banned_ips: self.dos_banned_ips_list(),
            subnet_bans: self.subnet_bans_list(),
            whitelist: self.whitelist_list(),
            whitelist_enabled: self.is_whitelist_enabled(),
            admins: self.admins_list(),
            player_limit: self.get_player_limit(),
            server_name: self.server_name(),
            server_description: self.server_description(),
            server_build: self.server_build(),
            server_version_type: self.server_version_type(),
            autosave_interval_secs: self.autosave_interval_secs(),
            map_list: self.map_list(),
            map_index: self.map_index(),
            round_wait_ticks: self.round_wait_ticks(),
            rules_overrides: self.rules_overrides.read().clone(),
            shuffle_mode: self.shuffle_mode(),
            kicked_ips: {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let mut entries: Vec<(String, u64)> = self
                    .kicked_ips
                    .iter()
                    .map(|entry| {
                        let remaining = entry
                            .value()
                            .saturating_duration_since(std::time::Instant::now())
                            .as_millis() as u64;
                        (entry.key().clone(), now + remaining)
                    })
                    .collect();
                entries.sort();
                entries
            },
            last_kicked: {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let mut entries: Vec<(String, u64)> = self
                    .last_kicked
                    .iter()
                    .map(|entry| {
                        let remaining = entry
                            .value()
                            .saturating_duration_since(std::time::Instant::now())
                            .as_millis() as u64;
                        (entry.key().clone(), now + remaining)
                    })
                    .collect();
                entries.sort();
                entries
            },
        };
        let bytes = serde_json::to_vec_pretty(&data)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let path = self.file_path();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(temporary, path)
    }

    /// `persist()` that logs failures instead of propagating them; used by the
    /// mutating admin commands so a read-only filesystem never breaks banning.
    fn persist_quiet(&self) {
        if let Err(err) = self.persist() {
            warn!("Could not persist admin data: {}", err);
        }
    }

    /// Registers an action filter (official `Administration.addActionFilter`).
    /// Filters run in registration order inside `allow_action`; a filter
    /// returning false vetoes the action.
    pub fn add_action_filter(&self, filter: ActionFilter) {
        self.action_filters.write().push(filter);
    }

    /// Official `Administration.allowAction(Player, ActionType, Cons<PlayerAction>)`
    /// (Administration.java:173): server actions (null player) are always
    /// allowed; otherwise every registered filter must accept the action.
    /// The actor identity is supplied by the caller from the authenticated
    /// session (SOL-002): uuid + admin flag come from the ConnectPacket
    /// profile, never from the packet payload.
    pub fn allow_action(&self, action: &PlayerAction) -> bool {
        let filters = self.action_filters.read();
        filters.iter().all(|filter| filter(action))
    }

    /// Number of registered action filters (diagnostics/tests).
    pub fn action_filter_count(&self) -> usize {
        self.action_filters.read().len()
    }
}

/// Official `Administration.ActionType` (Administration.java:764): the 16
/// action categories a player can perform on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionType {
    BreakBlock,
    PlaceBlock,
    Rotate,
    Configure,
    WithdrawItem,
    DepositItem,
    Control,
    BuildSelect,
    Command,
    RemovePlanned,
    CommandUnits,
    CommandBuilding,
    Respawn,
    PickupBlock,
    DropPayload,
    PingLocation,
}

/// Official `Administration.PlayerAction` (Administration.java:700): the
/// per-action context handed to every filter. Fields are populated only for
/// the action types that define them (tile/block for place/break, unit for
/// control, item for withdraw/deposit, plans for removePlanned, ...).
#[derive(Debug, Clone)]
pub struct PlayerAction {
    pub player_uuid: String,
    pub player_admin: bool,
    pub action_type: ActionType,
    /// Packed tile position, when the action targets a tile/building.
    pub tile: Option<i32>,
    /// Block id for placement/break actions.
    pub block: Option<i16>,
    /// Unit id for control/payload actions.
    pub unit_id: Option<i32>,
    /// Item id for withdraw/deposit actions.
    pub item: Option<i16>,
    /// Packed positions for removePlanned actions.
    pub plans: Vec<i32>,
    /// Unit ids for commandUnits actions.
    pub unit_ids: Vec<i32>,
    /// Building positions for commandBuilding actions.
    pub building_positions: Vec<i32>,
}

impl PlayerAction {
    /// Builds an action context for the authenticated actor (SOL-002).
    pub fn new(player_uuid: String, player_admin: bool, action_type: ActionType) -> Self {
        PlayerAction {
            player_uuid,
            player_admin,
            action_type,
            tile: None,
            block: None,
            unit_id: None,
            item: None,
            plans: Vec::new(),
            unit_ids: Vec::new(),
            building_positions: Vec::new(),
        }
    }

    pub fn with_tile(mut self, tile: i32) -> Self {
        self.tile = Some(tile);
        self
    }

    pub fn with_block(mut self, block: i16) -> Self {
        self.block = Some(block);
        self
    }

    pub fn with_unit(mut self, unit_id: i32) -> Self {
        self.unit_id = Some(unit_id);
        self
    }

    pub fn with_item(mut self, item: i16) -> Self {
        self.item = Some(item);
        self
    }
}

/// A registered action filter (official `ActionFilter` interface). Filters
/// receive the full `PlayerAction` and return whether the action is allowed.
pub type ActionFilter = Arc<dyn Fn(&PlayerAction) -> bool + Send + Sync>;

/// Per-player anti-spam interaction rate state (official
/// `PlayerInfo.rate` `Ratekeeper` + `Config.interactRateWindow/Limit/Kick`,
/// Administration.java:74-91). The window is a fixed 6 s slice: occurrences
/// reset when the window rolls over.
#[derive(Debug, Clone)]
struct InteractRateState {
    /// Monotonic window start (std Instant, converted to millis via elapsed).
    window_start: std::time::Instant,
    /// Occurrences inside the current window (kick counter, official
    /// `rate.occurences`).
    occurrences: u32,
    /// Whether the player was already over the kick threshold in this window.
    kicked: bool,
    /// M11: last warning time (official `PlayerInfo.messageTimer`, 120 s).
    warned_at: Option<std::time::Instant>,
}

impl InteractRateState {
    fn new() -> Self {
        InteractRateState {
            window_start: std::time::Instant::now(),
            occurrences: 0,
            kicked: false,
            warned_at: None,
        }
    }
}

/// Official defaults from Administration.java:543-547:
/// antiSpam=headless(true), interactRateWindow=6s, interactRateLimit=25,
/// interactRateKick=60.
const INTERACT_RATE_WINDOW_MILLIS: u128 = 6_000;
const INTERACT_RATE_LIMIT: u32 = 25;
const INTERACT_RATE_KICK: u32 = 60;
/// Official `player.kick(..., 30000)` duration for the anti-spam kick.
const KICK_DURATION: std::time::Duration = std::time::Duration::from_secs(30);
/// Official `PlayerInfo.messageTimer.get(120f)` warning interval (seconds).
const WARNING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mindustry-admin-test-{}-{}.json",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn ban_persists_across_reload() {
        let path = temp_file("ban");
        {
            let admin = Administration::with_file(path.clone());
            admin.ban_uuid("uuid-1234");
            admin.ban_ip("203.0.113.7");
            assert!(admin.is_banned("203.0.113.7", "uuid-1234"));
            assert!(!admin.is_banned("203.0.113.8", "other-uuid"));
        }
        let reloaded = Administration::with_file(path.clone());
        assert!(reloaded.is_banned("203.0.113.7", "uuid-1234"));
        // UUID ban applies from any IP; IP ban applies to any UUID.
        assert!(reloaded.is_banned("203.0.113.99", "uuid-1234"));
        assert!(reloaded.is_banned("203.0.113.7", "other-uuid"));
        assert!(!reloaded.is_banned("203.0.113.8", "other-uuid"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pardon_removes_ban_and_persists() {
        let path = temp_file("pardon");
        {
            let admin = Administration::with_file(path.clone());
            admin.ban_uuid("uuid-a");
            admin.ban_ip("198.51.100.4");
            assert!(admin.pardon_uuid("uuid-a"));
            assert!(!admin.pardon_uuid("uuid-a"));
            assert!(admin.pardon_ip("198.51.100.4"));
            assert!(!admin.is_banned("198.51.100.4", "uuid-a"));
        }
        let reloaded = Administration::with_file(path.clone());
        assert!(!reloaded.is_banned("198.51.100.4", "uuid-a"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn whitelist_filters_when_enabled() {
        let path = temp_file("whitelist");
        let admin = Administration::with_file(path.clone());
        // Disabled by default: everyone passes.
        assert!(admin.is_whitelisted("any-uuid"));
        admin.set_whitelist_enabled(true);
        assert!(!admin.is_whitelisted("any-uuid"));
        admin.whitelist_add("whitelisted-uuid");
        assert!(admin.is_whitelisted("whitelisted-uuid"));
        assert!(!admin.is_whitelisted("other-uuid"));
        admin.whitelist_remove("whitelisted-uuid");
        assert!(!admin.is_whitelisted("whitelisted-uuid"));
        drop(admin);
        let reloaded = Administration::with_file(path.clone());
        assert!(reloaded.is_whitelist_enabled());
        assert!(!reloaded.is_whitelisted("whitelisted-uuid"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn player_limit_hot_change_and_admin_bypass() {
        let path = temp_file("limit");
        let admin = Administration::with_file(path.clone());
        assert_eq!(admin.get_player_limit(), 0);
        admin.set_player_limit(3);
        assert_eq!(admin.get_player_limit(), 3);
        // Limit reached for a normal player...
        assert!(admin.is_at_player_limit(3, "normal-uuid"));
        assert!(admin.is_at_player_limit(4, "normal-uuid"));
        // ...but admins always bypass the limit (official NetServer.java:205).
        admin.add_admin("admin-uuid");
        assert!(!admin.is_at_player_limit(3, "admin-uuid"));
        // Below the limit is always fine.
        assert!(!admin.is_at_player_limit(2, "normal-uuid"));
        // Disabling (0) removes the cap.
        admin.set_player_limit(0);
        assert!(!admin.is_at_player_limit(100, "normal-uuid"));
        drop(admin);
        let reloaded = Administration::with_file(path.clone());
        assert_eq!(reloaded.get_player_limit(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn subnet_ban_matches_prefix() {
        let path = temp_file("subnet");
        let admin = Administration::with_file(path.clone());
        admin.add_subnet_ban("192.168.");
        assert!(admin.is_subnet_banned("192.168.1.44"));
        assert!(admin.is_subnet_banned("192.168.0.1"));
        assert!(!admin.is_subnet_banned("10.0.0.1"));
        assert!(admin.remove_subnet_ban("192.168."));
        assert!(!admin.is_subnet_banned("192.168.1.44"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dos_ban_persists() {
        let path = temp_file("dos");
        {
            let admin = Administration::with_file(path.clone());
            admin.add_dos_ban("198.51.100.99");
            assert!(admin.is_dos_blacklisted("198.51.100.99"));
            assert!(admin.is_banned("198.51.100.99", "irrelevant"));
        }
        let reloaded = Administration::with_file(path.clone());
        assert!(reloaded.is_dos_blacklisted("198.51.100.99"));
        assert!(reloaded.remove_dos_ban("198.51.100.99"));
        assert!(!reloaded.is_dos_blacklisted("198.51.100.99"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn admin_set_persists_and_lists() {
        let path = temp_file("admins");
        {
            let admin = Administration::with_file(path.clone());
            assert!(admin.add_admin("admin-uuid-1"));
            assert!(!admin.add_admin("admin-uuid-1")); // already admin
            assert!(admin.is_admin("admin-uuid-1"));
            assert!(!admin.is_admin("nobody"));
            assert_eq!(admin.admins_list(), vec!["admin-uuid-1".to_string()]);
        }
        let reloaded = Administration::with_file(path.clone());
        assert!(reloaded.is_admin("admin-uuid-1"));
        assert!(reloaded.remove_admin("admin-uuid-1"));
        assert!(!reloaded.is_admin("admin-uuid-1"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn config_overrides_persist() {
        let path = temp_file("config");
        {
            let admin = Administration::with_file(path.clone());
            admin.set_server_name("Test Arena");
            admin.set_server_description("desc with spaces");
            admin.set_server_build(159);
            admin.set_server_version_type("custom");
            assert_eq!(admin.server_name(), "Test Arena");
            assert_eq!(admin.server_description(), "desc with spaces");
            assert_eq!(admin.server_build(), 159);
            assert_eq!(admin.server_version_type(), "custom");
        }
        let reloaded = Administration::with_file(path.clone());
        assert_eq!(reloaded.server_name(), "Test Arena");
        assert_eq!(reloaded.server_description(), "desc with spaces");
        assert_eq!(reloaded.server_build(), 159);
        assert_eq!(reloaded.server_version_type(), "custom");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn autosave_due_thresholds() {
        let path = temp_file("autosave");
        let admin = Administration::with_file(path.clone());
        // Official default: Config.autosaveSpacing = 60 * 5 = 300 seconds
        // (core/src/mindustry/net/Administration.java).
        assert_eq!(admin.autosave_interval_secs(), 300);
        assert!(!admin.autosave_due(299.999));
        assert!(admin.autosave_due(300.0));
        assert!(admin.autosave_due(1200.5));
        // Zero is a valid interval: always due once elapsed >= 0.
        admin.set_autosave_interval_secs(0);
        assert!(admin.autosave_due(0.0));
        assert!(admin.autosave_due(1e-9));
        assert!(!admin.autosave_due(-1.0));
        admin.set_autosave_interval_secs(10);
        assert!(!admin.autosave_due(9.999));
        assert!(admin.autosave_due(10.0));
        drop(admin);
        let reloaded = Administration::with_file(path.clone());
        assert_eq!(reloaded.autosave_interval_secs(), 10);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn map_rotation_wraps_around() {
        let path = temp_file("rotation");
        let admin = Administration::with_file(path.clone());
        // Empty list -> no next map (official getNextMap may return null).
        assert_eq!(admin.advance_map(), None);
        admin.set_map_list(vec![
            "archipelago".to_string(),
            "fork".to_string(),
            "veins".to_string(),
        ]);
        assert_eq!(admin.advance_map(), Some("archipelago".to_string()));
        assert_eq!(admin.advance_map(), Some("fork".to_string()));
        assert_eq!(admin.advance_map(), Some("veins".to_string()));
        // Wraps around.
        assert_eq!(admin.advance_map(), Some("archipelago".to_string()));
        // A stale persisted index (>= len) is clamped to 0 on the next advance.
        admin.set_map_index(7);
        assert_eq!(admin.advance_map(), Some("archipelago".to_string()));
        assert_eq!(admin.map_index(), 1);
        drop(admin);
        // Rotation list and position persist across reload.
        let reloaded = Administration::with_file(path.clone());
        assert_eq!(
            reloaded.map_list(),
            vec![
                "archipelago".to_string(),
                "fork".to_string(),
                "veins".to_string()
            ]
        );
        assert_eq!(reloaded.map_index(), 1);
        assert_eq!(reloaded.advance_map(), Some("fork".to_string()));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rules_overrides_round_trip_and_legacy_file() {
        let path = temp_file("rules");
        {
            let admin = Administration::with_file(path.clone());
            assert!(admin.rules_overrides_snapshot().is_empty());
            // Empty keys are rejected.
            assert!(admin
                .apply_rules_override("  ", serde_json::json!(1))
                .is_err());
            admin
                .apply_rules_override("waveSpacing", serde_json::json!(60))
                .unwrap();
            admin
                .apply_rules_override("damageMultiplier", serde_json::json!(2.5))
                .unwrap();
            admin
                .apply_rules_override("pvp", serde_json::json!(true))
                .unwrap();
            assert_eq!(admin.rules_overrides_snapshot().len(), 3);
        }
        let reloaded = Administration::with_file(path.clone());
        assert_eq!(
            reloaded.rules_overrides_snapshot(),
            vec![
                ("damageMultiplier".to_string(), serde_json::json!(2.5)),
                ("pvp".to_string(), serde_json::json!(true)),
                ("waveSpacing".to_string(), serde_json::json!(60)),
            ]
        );
        drop(reloaded);
        // clear_rules_overrides empties the map and persists.
        let cleared = Administration::with_file(path.clone());
        cleared.clear_rules_overrides();
        assert!(cleared.rules_overrides_snapshot().is_empty());
        drop(cleared);
        // A legacy file without the SOL-012 keys still loads with defaults.
        std::fs::write(
            &path,
            r#"{"banned_ips":[],"banned_uuids":[],"dos_banned_ips":[],"subnet_bans":[],"whitelist":[],"whitelist_enabled":false,"admins":[],"player_limit":0,"server_name":"s","server_description":"d","server_build":158,"server_version_type":"official"}"#,
        )
        .unwrap();
        let legacy = Administration::with_file(path.clone());
        assert_eq!(legacy.autosave_interval_secs(), 300);
        assert!(legacy.map_list().is_empty());
        assert_eq!(legacy.map_index(), 0);
        assert_eq!(legacy.round_wait_ticks(), 720);
        assert!(legacy.rules_overrides_snapshot().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn connected_player_registry_snapshots() {
        let admin = Administration::new();
        admin.register_connection(ConnectedPlayer {
            uuid: "u1".into(),
            name: "Alpha".into(),
            ip: "10.0.0.1".into(),
            player_id: 1001,
            unit_id: 2001,
        });
        admin.register_connection(ConnectedPlayer {
            uuid: "u2".into(),
            name: "beta".into(),
            ip: "10.0.0.2".into(),
            player_id: 1002,
            unit_id: 2002,
        });
        assert_eq!(admin.connected_players_list().len(), 2);
        let alpha = admin.find_connected_by_name("alpha").unwrap();
        assert_eq!(alpha.uuid, "u1");
        assert_eq!(alpha.ip, "10.0.0.1");
        let by_uuid = admin.find_connected_by_uuid("u2").unwrap();
        assert_eq!(by_uuid.name, "beta");
        admin.unregister_connection("u1");
        assert_eq!(admin.connected_players_list().len(), 1);
        admin.clear_connections();
        assert!(admin.connected_players_list().is_empty());
    }

    #[test]
    fn allow_action_runs_registered_filters_in_order() {
        let admin = Administration::new();
        // Baseline: no custom filter vetoes a place action (the built-in
        // anti-spam filter exempts place/break).
        let place = PlayerAction::new("uuid-1".into(), false, ActionType::PlaceBlock)
            .with_tile((10 << 16) | 20)
            .with_block(216);
        assert!(admin.allow_action(&place));

        let vetoed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let vetoed_clone = vetoed.clone();
        admin.add_action_filter(std::sync::Arc::new(move |action: &PlayerAction| {
            if action.action_type == ActionType::PlaceBlock && action.block == Some(216) {
                vetoed_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                return false;
            }
            true
        }));
        assert!(!admin.allow_action(&place));
        assert!(vetoed.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(admin.action_filter_count(), 2);
    }

    #[test]
    fn interact_rate_filter_limits_non_place_actions() {
        let admin = Administration::new();
        let mut allowed = 0;
        // Non-place action (configure) is rate-limited: 25 allowed, then
        // rejected until the 6 s window rolls over. Bypass the window by
        // manipulating time is not possible through the public API, so the
        // test asserts the limit boundary only.
        for _ in 0..30 {
            let action =
                PlayerAction::new("uuid-rate".into(), false, ActionType::Configure).with_tile(0);
            if admin.allow_action(&action) {
                allowed += 1;
            }
        }
        assert_eq!(allowed, INTERACT_RATE_LIMIT as usize);
        // M11: admins are NOT exempt — the official filter only checks
        // `player.isLocal()`, false for every remote player in headless.
        let mut admin_allowed = 0;
        for _ in 0..40 {
            let action = PlayerAction::new("uuid-admin".into(), true, ActionType::Configure);
            if admin.allow_action(&action) {
                admin_allowed += 1;
            }
        }
        assert_eq!(admin_allowed, INTERACT_RATE_LIMIT as usize);
        // Place/break/commandUnits are never rate-limited.
        for _ in 0..40 {
            let action =
                PlayerAction::new("uuid-rate".into(), false, ActionType::PlaceBlock).with_block(5);
            assert!(admin.allow_action(&action));
        }
    }

    #[test]
    fn anti_spam_kicks_at_sixty_occurrences_and_warns_between() {
        let admin = Administration::new();
        // 25 allowed, 26..60 denied with a pending warning on the first
        // denial (official messageTimer 120 s).
        for _ in 0..26 {
            let action = PlayerAction::new("uuid-spam".into(), false, ActionType::Configure);
            let _ = admin.allow_action(&action);
        }
        assert!(
            admin.take_pending_kick("uuid-spam").is_none(),
            "no kick below the 60-occurrence threshold"
        );
        assert!(
            admin.take_pending_warning("uuid-spam"),
            "first over-limit action queues the warning"
        );
        assert!(
            !admin.take_pending_warning("uuid-spam"),
            "warning is rate-limited to once per 120 s"
        );
        // 61st occurrence -> the official kicks for 30 s.
        for _ in 0..35 {
            let action = PlayerAction::new("uuid-spam".into(), false, ActionType::Configure);
            let _ = admin.allow_action(&action);
        }
        assert!(
            admin.take_pending_kick("uuid-spam").is_some(),
            "61st occurrence queues a 30 s kick"
        );
    }

    #[test]
    fn kicked_cooldown_is_queried_by_uuid_and_ip_and_persists() {
        let path = temp_file("kick");
        let admin = Administration::with_file(path.clone());
        assert!(admin.kick_time("uuid-1", "1.2.3.4").is_none());
        admin.handle_kicked("uuid-1", "1.2.3.4", std::time::Duration::from_secs(30));
        assert!(admin.kick_time("uuid-1", "1.2.3.4").is_some());
        // The cooldown is keyed by BOTH uuid and ip (getKickTime = max of
        // PlayerInfo.lastKicked and kickedIPs[ip]).
        assert!(admin.kick_time("uuid-1", "9.9.9.9").is_some());
        assert!(admin.kick_time("uuid-2", "1.2.3.4").is_some());
        assert!(admin.kick_time("uuid-2", "9.9.9.9").is_none());
        // Reload: the persisted millis deadlines restore the cooldown.
        let reloaded = Administration::with_file(path.clone());
        assert!(
            reloaded.kick_time("uuid-1", "1.2.3.4").is_some(),
            "kick cooldown survives reload"
        );
        // A zero duration registers nothing (official kick(reason) path).
        let admin = Administration::new();
        admin.handle_kicked("uuid-0", "5.5.5.5", std::time::Duration::ZERO);
        assert!(admin.kick_time("uuid-0", "5.5.5.5").is_none());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("tmp"));
    }
}
