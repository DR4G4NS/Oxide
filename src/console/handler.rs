use crate::console::command::{BanKind, BanListAction, ServerCommand, WhitelistAction};
use crate::engine::tick::TickEngine;
use crate::network::world::{NetworkControl, RuntimeCommand};
use crate::state::administration::Administration;
use crate::state::game_state::{GameMode, GameState};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{info, warn};

// Keep the handler's established println-style call sites readable while
// allowing the TUI to capture their output without redirecting process-wide
// stdout. In classic/headless mode `write_line` still writes to stdout.
macro_rules! println {
    ($($arg:tt)*) => {
        crate::console::output::write_line(format!($($arg)*))
    };
}

pub struct ConsoleHandler {
    pub state: GameState,
    pub admin: Administration,
    pub tick_engine: Arc<TickEngine>,
    pub network: NetworkControl,
}

impl ConsoleHandler {
    pub fn new(
        state: GameState,
        admin: Administration,
        tick_engine: Arc<TickEngine>,
        network: NetworkControl,
    ) -> Self {
        Self {
            state,
            admin,
            tick_engine,
            network,
        }
    }

    pub async fn run_interactive_loop(&self) {
        info!("Mindustry Headless Console Ready. Type 'help' for commands.");
        let stdin = tokio::io::stdin();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        // Rolling UPS sampling state for the extended `status` command.
        let mut last_status_sample = Instant::now();
        let mut last_status_ticks = self.tick_engine.total_ticks.load(Ordering::Relaxed);

        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(cmd) = ServerCommand::parse(&line) {
                match cmd {
                    ServerCommand::Status => {
                        // The status sampling state lives in the loop so UPS
                        // is measured between consecutive `status` commands.
                        self.status(&mut last_status_sample, &mut last_status_ticks);
                    }
                    ServerCommand::Help => {
                        self.print_help();
                    }
                    other => {
                        if self.handle_command(other).await {
                            return;
                        }
                    }
                }
            }
        }

        // Services commonly run without an attached stdin. EOF must not tear
        // down the Tokio runtime (and therefore every network listener).
        info!("Console input closed; server will continue until SIGINT/SIGTERM.");
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!("Could not install shutdown signal handler: {}", err);
        }
    }

    fn print_help(&self) {
        println!("--- Mindustry Server Commands ---");
        println!("help               - Display this help message");
        println!("status             - Print server CPU/TPS and player stats");
        println!("players            - List connected players (name/uuid/ip/admin)");
        println!("host [map] [mode]  - Start hosting or hot-switch map");
        println!("stop               - Disconnect all players and stop hosting");
        println!("exit / quit        - Flush saves and shutdown server");
        println!("maps               - List available maps");
        println!("save <slot>        - Save game state");
        println!("load <slot>        - Load game state");
        println!("kick <player>      - Kick player");
        println!("ban <name|uuid|ip> - Ban and immediately kick (ban ip <ip> / ban id <uuid>)");
        println!("unban <uuid|ip>    - Remove a ban");
        println!("bans               - List banned IPs and UUIDs");
        println!("pause [on|off]     - Pause or resume the simulation");
        println!("playerlimit [n]    - Show or change the live player limit (off to disable)");
        println!("whitelist          - List whitelist; whitelist add|remove <uuid>; on|off");
        println!("admin <add|remove> <uuid|name> - Manage admins");
        println!("admins             - List all admins");
        println!("config [k [v]]     - Show/change name, desc, build, versionType, maxPlayers");
        println!("subnet-ban [add|remove <prefix>] - Ban an IP prefix");
        println!("dos-ban [add|remove <ip>] - Manage DOS bans");
        println!("say <message>      - Broadcast chat message");
        println!("gameover           - Trigger game over");
        println!("waves [n]          - Dispatch next wave, or set wave counter to n");
        println!("spawn <u> <n> [x y]- Spawn n enemy units");
        println!("mode <mode>        - Switch GameMode live (survival/sandbox/pvp/attack)");
        println!("time <n> [s]       - Set wave countdown in ticks (or seconds with s)");
        println!("version            - Print the advertised protocol build and type");
        println!("rules [k [v]]      - Inspect/set global rules overrides (rules remove <k>)");
        println!("nextmap            - Advance to the next map in the rotation");
        println!("saves              - List save slots");
        println!("loadautosave       - Load the autosave1 slot");
        println!("reloadmaps         - Rescan the maps directory and refresh the rotation");
        println!("shuffle [m]        - Show/set shuffle mode (none/all/custom/builtin)");
    }

    /// Synchronous-ish dispatch core shared by the interactive loop and the
    /// regression tests. Async so `exit` can give the 1 s autosave a final
    /// flush window before returning a graceful-exit request.
    /// Returns `true` when the operator requested a graceful process exit.
    /// The caller owns the event loop, which lets a TUI restore raw mode and
    /// the alternate screen before `main` returns.
    pub async fn handle_command(&self, cmd: ServerCommand) -> bool {
        let exit_requested = matches!(&cmd, ServerCommand::Exit);
        match cmd {
            ServerCommand::Help => self.print_help(),
            ServerCommand::Status => {
                let mut sample = Instant::now();
                let mut ticks = self.tick_engine.total_ticks.load(Ordering::Relaxed);
                self.status(&mut sample, &mut ticks);
            }
            ServerCommand::Host { map, mode } => {
                info!(
                    "Hosting map '{}' [{}] requested; hot-swapping the world...",
                    map, mode
                );
                self.network.send(RuntimeCommand::HostMap { map, mode });
            }
            ServerCommand::Stop => {
                self.stop_all();
            }
            ServerCommand::Exit => {
                self.exit_server().await;
            }
            ServerCommand::Maps => {
                println!(
                    "Loaded map: {}. Select an official/custom .msav at startup with \
                     --map-file <path>.",
                    self.state.map_name.read()
                );
            }
            ServerCommand::Save { slot } => {
                self.network.send(RuntimeCommand::Save(slot));
            }
            ServerCommand::SaveMsav { slot } => {
                self.network.send(RuntimeCommand::SaveMsav(slot));
            }
            ServerCommand::Load { slot } => {
                self.network.send(RuntimeCommand::Load(slot));
            }
            ServerCommand::Kick { player } => {
                self.network.send(RuntimeCommand::Kick(player));
            }
            ServerCommand::Ban { target, kind } => {
                self.ban_target(&target, kind);
            }
            ServerCommand::Pardon { target } => {
                self.unban_target(&target);
            }
            ServerCommand::Say { message } => {
                self.network.send(RuntimeCommand::Say(message));
            }
            ServerCommand::GameOver => {
                self.state.game_over.store(true, Ordering::SeqCst);
                self.network.send(RuntimeCommand::GameOver);
                info!("Force Game Over triggered.");
            }
            ServerCommand::Players => {
                self.list_players();
            }
            ServerCommand::Bans => {
                self.list_bans();
            }
            ServerCommand::Pause { on } => {
                self.set_paused(on);
            }
            ServerCommand::Team { player, team } => {
                self.network.send(RuntimeCommand::SetTeam { player, team });
            }
            ServerCommand::PlayerLimit { limit } => {
                self.set_player_limit(limit);
            }
            ServerCommand::Whitelist { action } => {
                self.manage_whitelist(action);
            }
            ServerCommand::Admin { add, target } => {
                self.manage_admin(add, &target);
            }
            ServerCommand::Admins => {
                let admins = self.admin.admins_list();
                if admins.is_empty() {
                    println!("No admins have been found.");
                } else {
                    println!("Admins:");
                    for uuid in admins {
                        let name = self
                            .admin
                            .find_connected_by_uuid(&uuid)
                            .map(|player| player.name)
                            .unwrap_or_else(|| "<offline>".to_string());
                        println!(" - {} / ID: {}", name, uuid);
                    }
                }
            }
            ServerCommand::Config { key, value } => {
                self.manage_config(key.as_deref(), value.as_deref());
            }
            ServerCommand::SubnetBan { action } => match action {
                None => {
                    let subnets = self.admin.subnet_bans_list();
                    if subnets.is_empty() {
                        println!("Subnets banned: <none>");
                    } else {
                        println!("Subnets banned:");
                        for subnet in subnets {
                            println!("\t{}", subnet);
                        }
                    }
                }
                Some(BanListAction::Add(address)) => {
                    if self.admin.subnet_bans.contains(&address) {
                        warn!("That subnet is already banned.");
                    } else {
                        self.admin.add_subnet_ban(&address);
                        info!("Banned subnet {}", address);
                    }
                }
                Some(BanListAction::Remove(address)) => {
                    if self.admin.remove_subnet_ban(&address) {
                        info!("Unbanned subnet {}", address);
                    } else {
                        warn!("That subnet isn't banned.");
                    }
                }
            },
            ServerCommand::DosBan { action } => match action {
                None => {
                    let dos = self.admin.dos_banned_ips_list();
                    if dos.is_empty() {
                        println!("DOS bans: <none>");
                    } else {
                        println!("DOS bans:");
                        for ip in dos {
                            println!("\t{}", ip);
                        }
                    }
                }
                Some(BanListAction::Add(ip)) => {
                    self.admin.add_dos_ban(&ip);
                    info!("Dos banned: {}", ip);
                }
                Some(BanListAction::Remove(ip)) => {
                    if self.admin.remove_dos_ban(&ip) {
                        info!("Removed dos ban: {}", ip);
                    } else {
                        warn!("That IP is not DOS-banned.");
                    }
                }
            },
            ServerCommand::Waves { wave } => match wave {
                Some(wave) => {
                    self.state.wave.store(wave, Ordering::Relaxed);
                    *self.state.wave_time.write() = 60.0 * 60.0;
                    info!("Wave counter set to {}", wave);
                    println!("Wave counter set to {}.", wave);
                }
                None => {
                    *self.state.wave_time.write() = 0.0;
                    info!("Dispatch of next wave requested (wave_time = 0)");
                    println!("Next wave will dispatch on the next simulation tick.");
                }
            },
            ServerCommand::Spawn { unit, count, x, y } => {
                self.network
                    .send(RuntimeCommand::SpawnEnemy { unit, count, x, y });
            }
            ServerCommand::Mode { mode } => {
                let game_mode = match mode.as_str() {
                    "survival" => Some(GameMode::Survival),
                    "sandbox" => Some(GameMode::Sandbox),
                    "pvp" => Some(GameMode::Pvp),
                    "attack" => Some(GameMode::Attack),
                    _ => None,
                };
                match game_mode {
                    Some(game_mode) => {
                        self.network
                            .send(RuntimeCommand::SetMode { mode: mode.clone() });
                        info!("Game mode switched to {:?}", game_mode);
                        println!("Game mode switched to {:?}.", game_mode);
                        println!(
                            "Note: mode only gates wave simulation; sandbox/pvp skip enemy waves."
                        );
                    }
                    None => {
                        warn!("Unknown game mode '{}'; leaving mode unchanged", mode);
                        println!(
                            "Unknown mode '{}'. Valid: survival, sandbox, pvp, attack.",
                            mode
                        );
                    }
                }
            }
            ServerCommand::Time { value, seconds } => {
                let ticks = if seconds { value * 60.0 } else { value };
                *self.state.wave_time.write() = ticks;
                info!("Wave countdown set to {} ticks ({} s)", ticks, value);
                println!(
                    "Wave countdown set to {:.0} ticks ({} {}).",
                    ticks,
                    value,
                    if seconds { "seconds" } else { "ticks" }
                );
            }
            ServerCommand::Version => {
                println!(
                    "Oxide {} (build {}, type {})",
                    self.admin.server_name(),
                    self.admin.server_build(),
                    self.admin.server_version_type()
                );
            }
            ServerCommand::Rules { key, value } => {
                self.rules_command(key.as_deref(), value.as_deref());
            }
            ServerCommand::NextMap => {
                self.network.send(RuntimeCommand::NextMap);
                println!("Advancing to the next map in the rotation.");
            }
            ServerCommand::Saves => {
                let pattern = std::path::Path::new(".")
                    .read_dir()
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.file_name().to_string_lossy().into_owned())
                            .filter(|name| name.ends_with(".json") || name.ends_with(".msav"))
                            .filter(|name| name.contains("autosave") || name.contains("slot"))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if pattern.is_empty() {
                    println!("Save slots: <none>");
                } else {
                    println!("Save slots:");
                    for name in pattern {
                        println!("\t{}", name);
                    }
                }
            }
            ServerCommand::LoadAutosave => {
                self.network
                    .send(RuntimeCommand::Load("autosave1".to_string()));
                println!("Loading autosave1 slot.");
            }
            ServerCommand::ReloadMaps => {
                let maps = default_map_list();
                self.admin.set_map_list(maps.clone());
                println!("Reloaded {} maps into the rotation.", maps.len());
            }
            ServerCommand::Shuffle { mode } => match mode {
                None => {
                    println!("Shuffle mode is currently '{}'.", self.admin.shuffle_mode());
                }
                Some(mode) => {
                    if self.admin.set_shuffle_mode(&mode) {
                        println!("Shuffle mode set to '{}'.", mode);
                    } else {
                        println!(
                            "Invalid shuffle mode '{}'. Valid: none, all, custom, builtin.",
                            mode
                        );
                    }
                }
            },
            ServerCommand::Unknown(u) => {
                println!("Unknown command: '{}'. Type 'help' for command list.", u);
            }
        }
        exit_requested
    }

    /// `rules [key [value]]` / `rules remove <key>`: inspects or mutates the
    /// global rules overrides (official `rules` command).
    fn rules_command(&self, key: Option<&str>, value: Option<&str>) {
        match (key, value) {
            (None, _) => {
                let overrides = self.admin.rules_overrides_snapshot();
                if overrides.is_empty() {
                    println!("Global rules overrides: <none>");
                } else {
                    println!("Global rules overrides:");
                    for (key, value) in overrides {
                        println!("\t{} = {}", key, value);
                    }
                }
            }
            (Some("remove"), Some(target)) => {
                // Remove one key: rebuild the map without it.
                let mut kept = self.admin.rules_overrides_snapshot();
                let before = kept.len();
                kept.retain(|(key, _)| key != target);
                if kept.len() == before {
                    println!("No override '{}' to remove.", target);
                    return;
                }
                self.admin.clear_rules_overrides();
                for (key, value) in kept {
                    let _ = self.admin.apply_rules_override(&key, value);
                }
                println!("Removed rules override '{}'.", target);
            }
            (Some(key), Some(raw_value)) => {
                let parsed: serde_json::Value = if raw_value.eq_ignore_ascii_case("true") {
                    serde_json::json!(true)
                } else if raw_value.eq_ignore_ascii_case("false") {
                    serde_json::json!(false)
                } else if let Ok(number) = raw_value.parse::<f64>() {
                    serde_json::json!(number)
                } else {
                    serde_json::json!(raw_value)
                };
                match self.admin.apply_rules_override(key, parsed) {
                    Ok(()) => println!("Rules override '{}' set.", key),
                    Err(err) => println!("Rules error: {}", err),
                }
            }
            (Some(_), None) => {
                println!("Usage: rules [key [value]] | rules remove <key>");
            }
        }
    }

    fn status(&self, last_status_sample: &mut Instant, last_status_ticks: &mut u64) {
        let total_ticks = self.tick_engine.total_ticks.load(Ordering::Relaxed);
        let is_active = self.state.is_active();
        let map = self.state.map_name.read().clone();
        let players = self.state.players_count.load(Ordering::Relaxed);
        let limit = self.admin.get_player_limit();
        let wave = self.state.wave.load(Ordering::Relaxed);
        let wave_time = *self.state.wave_time.read();
        let enemies = self.state.enemies_count.load(Ordering::Relaxed);
        let core_health = *self.state.core_health.read();
        let mode = *self.state.mode.read();

        // Rolling UPS sample: ticks processed by the tick engine since the
        // previous `status` invocation.
        let now = Instant::now();
        let elapsed = now.duration_since(*last_status_sample).as_secs_f64();
        let delta_ticks = total_ticks.saturating_sub(*last_status_ticks);
        *last_status_ticks = total_ticks;
        *last_status_sample = now;
        let ups = if elapsed > 0.0 {
            delta_ticks as f64 / elapsed
        } else {
            0.0
        };

        println!("--- Server Status ---");
        println!("Status: {}", if is_active { "HOSTING" } else { "STOPPED" });
        println!("Paused: {}", self.state.is_paused.load(Ordering::Relaxed));
        println!("Map: {}", map);
        println!("Game Mode: {:?}", mode);
        println!(
            "Players: {}/{}",
            players,
            if limit == 0 {
                "Unlimited".to_string()
            } else {
                limit.to_string()
            }
        );
        println!("Wave: {}", wave);
        println!("Wave Timer: {:.1} ticks remaining", wave_time);
        println!("Enemies Alive: {}", enemies);
        println!("Core Health: {:.0}", core_health);
        println!("Total Ticks Processed: {}", total_ticks);
        println!("Target TPS: {}", self.tick_engine.target_tps);
        println!("UPS (since last status): {:.1}", ups);
        println!("CPU Multithreading: Rayon Worker Pool Active");
        // P2: world-loop metrics (the authoritative game loop, distinct from
        // the TickEngine fixed loop).
        println!(
            "World Loop Iterations: {}",
            self.state.world_ticks.load(Ordering::Relaxed)
        );
        println!(
            "World Tick Duration: {:.1} ms (max {:.1} ms)",
            self.state.world_tick_us.load(Ordering::Relaxed) as f64 / 1000.0,
            self.state.world_tick_max_us.load(Ordering::Relaxed) as f64 / 1000.0
        );
        println!(
            "Dropped Outbound Frames: {}",
            self.state.dropped_frames_total.load(Ordering::Relaxed)
        );
    }

    /// `players`: official format `[A] name / ID: uuid / IP: ip`.
    fn list_players(&self) {
        let players = self.admin.connected_players_list();
        if players.is_empty() {
            println!("No players are currently in the server.");
            return;
        }
        println!("Players: {}", players.len());
        for player in players {
            let marker = if self.admin.is_admin(&player.uuid) {
                "[A]"
            } else {
                "[P]"
            };
            println!(
                " {} {} / ID: {} / IP: {}",
                marker, player.name, player.uuid, player.ip
            );
        }
    }

    fn list_bans(&self) {
        let uuids = self.admin.banned_uuids_list();
        if uuids.is_empty() {
            println!("No ID-banned players have been found.");
        } else {
            println!("Banned players [ID]:");
            for uuid in uuids {
                println!("  {}", uuid);
            }
        }
        let ips = self.admin.banned_ips_list();
        if ips.is_empty() {
            println!("No IP-banned players have been found.");
        } else {
            println!("Banned players [IP]:");
            for ip in ips {
                println!("  {}", ip);
            }
        }
    }

    /// Official `ban` semantics: ban ID + IP, then immediately kick every
    /// connected player matching the target (ServerControl.java:1007).
    fn ban_target(&self, target: &str, kind: BanKind) {
        let resolved = self.resolve_ban_target(target, kind);
        let Some((uuid, ip, kick_name)) = resolved else {
            match kind {
                BanKind::Name => {
                    warn!(
                        "No online player named '{}'. Use 'ban id <uuid>' to ban an offline player.",
                        target
                    );
                }
                _ => warn!("Could not ban '{}': no matching player/IP/UUID", target),
            }
            return;
        };
        if let Some(uuid) = &uuid {
            self.admin.ban_uuid(uuid);
        }
        if let Some(ip) = &ip {
            self.admin.ban_ip(ip);
        }
        if let Some(name) = &kick_name {
            // Immediate expulsion: the existing runtime Kick path emits a
            // KickCallPacket (ID 58) to the matching connection. Reason-typed
            // parity (KickReason.banned = 3) is documented in console_impl.md.
            self.network.send(RuntimeCommand::Kick(name.clone()));
            info!("Banned and kicked connected player '{}'", name);
        } else {
            info!("Banned '{}' (no connected player to kick)", target);
        }
    }

    fn resolve_ban_target(
        &self,
        target: &str,
        kind: BanKind,
    ) -> Option<(Option<String>, Option<String>, Option<String>)> {
        match kind {
            BanKind::Auto => {
                // Connected name first, then IP-shaped, then raw UUID.
                if let Some(player) = self.admin.find_connected_by_name(target) {
                    return Some((Some(player.uuid), Some(player.ip), Some(player.name)));
                }
                if target.contains('.') {
                    return Some((None, Some(target.to_string()), Some(target.to_string())));
                }
                Some((Some(target.to_string()), None, None))
            }
            BanKind::Ip => Some((None, Some(target.to_string()), Some(target.to_string()))),
            BanKind::Id => {
                if let Some(player) = self.admin.find_connected_by_uuid(target) {
                    Some((Some(player.uuid), Some(player.ip), Some(player.name)))
                } else {
                    Some((Some(target.to_string()), None, None))
                }
            }
            BanKind::Name => self
                .admin
                .find_connected_by_name(target)
                .map(|player| (Some(player.uuid), Some(player.ip), Some(player.name))),
        }
    }

    /// `unban <uuid|name|ip>`: pardons by connected name, uuid and/or IP.
    fn unban_target(&self, target: &str) {
        if target.is_empty() {
            warn!("Usage: unban <uuid|name|ip>");
            return;
        }
        let mut removed = false;
        if let Some(player) = self.admin.find_connected_by_name(target) {
            removed |= self.admin.pardon_uuid(&player.uuid);
            removed |= self.admin.pardon_ip(&player.ip);
        }
        removed |= self.admin.pardon_uuid(target);
        removed |= self.admin.pardon_ip(target);
        if removed {
            info!("Unbanned player: {}", target);
            println!("Unbanned player: {}", target);
        } else {
            warn!("That IP/ID is not banned: {}", target);
        }
    }

    /// `pause [on|off]` / `resume`: flips the simulation flag through the
    /// runtime channel. The listener's `RuntimeCommand::Pause` handler clears
    /// the PvP auto-pause marker (so a manual pause is never overridden) and
    /// the world simulation loop gates on `state.is_active()` (which includes
    /// `!is_paused`); StateSnapshot already transmits `is_paused`.
    fn set_paused(&self, on: bool) {
        if !self.state.is_hosting.load(Ordering::SeqCst) {
            warn!("Cannot pause without a game running.");
            return;
        }
        self.network.send(RuntimeCommand::Pause(on));
        if on {
            info!("Game paused.");
            println!("Game paused.");
        } else {
            info!("Game unpaused.");
            println!("Game unpaused.");
        }
    }

    /// `playerlimit [off|<n>]`: hot-change of the admission cap.
    fn set_player_limit(&self, limit: Option<u32>) {
        match limit {
            None => {
                let current = self.admin.get_player_limit();
                println!(
                    "Player limit is currently {}.",
                    if current == 0 {
                        "off".to_string()
                    } else {
                        current.to_string()
                    }
                );
            }
            Some(0) => {
                self.admin.set_player_limit(0);
                info!("Player limit disabled.");
                println!("Player limit disabled.");
            }
            Some(limit) => {
                self.admin.set_player_limit(limit);
                info!("Player limit is now {}.", limit);
                println!("Player limit is now {}.", limit);
            }
        }
    }

    /// `whitelist [add|remove <uuid>|on|off]`: management + persistence.
    /// ConnectPacket enforcement (KickReason.whitelist = 13) is the
    /// orchestrator's listener.rs integration; see console_impl.md.
    fn manage_whitelist(&self, action: Option<WhitelistAction>) {
        match action {
            None => {
                println!(
                    "Whitelist {}.",
                    if self.admin.is_whitelist_enabled() {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                let whitelist = self.admin.whitelist_list();
                if whitelist.is_empty() {
                    println!("No whitelisted players found.");
                } else {
                    println!("Whitelist:");
                    for uuid in whitelist {
                        let name = self
                            .admin
                            .find_connected_by_uuid(&uuid)
                            .map(|player| player.name)
                            .unwrap_or_else(|| "<offline>".to_string());
                        println!("- Name: {} / UUID: {}", name, uuid);
                    }
                }
            }
            Some(WhitelistAction::Add(uuid)) => {
                if self.admin.whitelist_add(&uuid) {
                    info!("Player '{}' has been whitelisted.", uuid);
                    println!("Player '{}' has been whitelisted.", uuid);
                } else {
                    warn!("Player '{}' is already whitelisted.", uuid);
                }
            }
            Some(WhitelistAction::Remove(uuid)) => {
                if self.admin.whitelist_remove(&uuid) {
                    info!("Player '{}' has been un-whitelisted.", uuid);
                    println!("Player '{}' has been un-whitelisted.", uuid);
                } else {
                    warn!("Player '{}' is not whitelisted.", uuid);
                }
            }
            Some(WhitelistAction::Enable(on)) => {
                self.admin.set_whitelist_enabled(on);
                info!("Whitelist {}.", if on { "enabled" } else { "disabled" });
                println!("Whitelist {}.", if on { "enabled" } else { "disabled" });
            }
        }
    }

    /// `admin <add|remove> <uuid|name>`: resolve online names to UUIDs.
    fn manage_admin(&self, add: bool, target: &str) {
        let uuid = self
            .admin
            .find_connected_by_name(target)
            .map(|player| player.uuid)
            .unwrap_or_else(|| target.to_string());
        if add {
            if self.admin.add_admin(&uuid) {
                info!("Changed admin status of player: {}", uuid);
                println!("Admin added: {}", uuid);
            } else {
                warn!("Player '{}' is already an admin.", uuid);
            }
        } else if self.admin.remove_admin(&uuid) {
            info!("Changed admin status of player: {}", uuid);
            println!("Admin removed: {}", uuid);
        } else {
            warn!("Player '{}' is not an admin.", uuid);
        }
    }

    /// `config [key [value...]]`: runtime overrides for name, desc, build,
    /// versionType and maxPlayers. Values live in Administration so the
    /// discovery encoder and ConnectPacket validation can read them
    /// (orchestrator integration, see console_impl.md).
    fn manage_config(&self, key: Option<&str>, value: Option<&str>) {
        let show_all = || {
            println!("All config values:");
            println!("| name: {}", self.admin.server_name());
            println!("| desc: {}", self.admin.server_description());
            println!("| build: {}", self.admin.server_build());
            println!("| versionType: {}", self.admin.server_version_type());
            println!("| maxPlayers: {}", self.admin.get_player_limit());
        };
        let Some(key) = key else {
            show_all();
            return;
        };
        let normalized = key.to_lowercase();
        let matches = |name: &str| normalized == name;
        let known = matches("name")
            || matches("servername")
            || matches("desc")
            || matches("description")
            || matches("build")
            || matches("versiontype")
            || matches("version_type")
            || matches("maxplayers")
            || matches("strict");
        if !known {
            warn!(
                "Unknown config: '{}'. Run the command with no arguments to get a list.",
                key
            );
            println!(
                "Unknown config: '{}'. Run 'config' with no arguments to get a list.",
                key
            );
            return;
        }
        let Some(value) = value else {
            let current = if matches("name") || matches("servername") {
                self.admin.server_name()
            } else if matches("desc") || matches("description") {
                self.admin.server_description()
            } else if matches("build") {
                self.admin.server_build().to_string()
            } else if matches("versiontype") || matches("version_type") {
                self.admin.server_version_type()
            } else if matches("strict") {
                if self.state.strict_mode.load(Ordering::Relaxed) {
                    "on".to_string()
                } else {
                    "off".to_string()
                }
            } else {
                let limit = self.admin.get_player_limit();
                if limit == 0 {
                    "off".to_string()
                } else {
                    limit.to_string()
                }
            };
            println!("'{}' is currently {}.", key, current);
            return;
        };
        if matches("name") || matches("servername") {
            self.admin.set_server_name(value);
        } else if matches("desc") || matches("description") {
            self.admin.set_server_description(value);
        } else if matches("build") {
            match value.parse::<i32>() {
                Ok(build) => self.admin.set_server_build(build),
                Err(_) => {
                    warn!("Not a valid number: {}", value);
                    return;
                }
            }
        } else if matches("versiontype") || matches("version_type") {
            self.admin.set_server_version_type(value);
        } else if matches("strict") {
            let enabled = match value.to_ascii_lowercase().as_str() {
                "on" | "1" | "true" | "yes" => true,
                "off" | "0" | "false" | "no" => false,
                _ => {
                    warn!("Not a valid boolean: {}", value);
                    return;
                }
            };
            self.state.strict_mode.store(enabled, Ordering::Relaxed);
            if enabled {
                warn!(
                    "Strict mode ON: unsupported logic statements and unknown spawn groups now reject content with diagnostics instead of degrading silently."
                );
            } else {
                info!("Strict mode OFF: unsupported content warns and continues.");
            }
        } else {
            match value.parse::<u32>() {
                Ok(limit) => self.admin.set_player_limit(limit),
                Err(_) => {
                    warn!("Not a valid number: {}", value);
                    return;
                }
            }
        }
        println!("{} set to {}.", key, value);
    }

    /// `stop`: disconnect every connected player (KickCallPacket) and stop
    /// hosting. The official `stop` does `net.closeServer()` + menu state.
    fn stop_all(&self) {
        let names: Vec<String> = self
            .admin
            .connected_players_list()
            .iter()
            .map(|player| player.name.clone())
            .collect();
        for name in &names {
            self.network.send(RuntimeCommand::Kick(name.clone()));
        }
        self.admin.clear_connections();
        self.state.stop_hosting();
        info!("Stopped server; disconnected {} player(s).", names.len());
        println!("Stopped server; disconnected {} player(s).", names.len());
    }

    /// `exit`: persist admin data, give the 1 s autosave one final window to
    /// flush the dirty world, then return control to the owning console loop.
    async fn exit_server(&self) {
        info!("Shutting down Mindustry Rust Server...");
        if let Err(err) = self.admin.persist() {
            warn!("Could not persist admin data on exit: {}", err);
        } else {
            info!("Admin data flushed to {}", self.admin.file_path().display());
        }
        println!("Flushing world save (1 s autosave window)...");
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    }
}

/// Rescans the default maps directory (`../core/assets/maps/default` plus the
/// bundled repo layout) and returns the sorted map names for the rotation.
fn default_map_list() -> Vec<String> {
    let mut maps = Vec::new();
    let candidates = ["../core/assets/maps/default", "maps"];
    for directory in candidates {
        if let Ok(entries) = std::fs::read_dir(directory) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.ends_with(".msav") {
                    maps.push(name.trim_end_matches(".msav").to_string());
                }
            }
        }
    }
    maps.sort();
    maps.dedup();
    maps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::administration::ConnectedPlayer;
    use tokio::sync::mpsc;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mindustry-console-test-{}-{}.json",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn harness(name: &str) -> (ConsoleHandler, mpsc::UnboundedReceiver<RuntimeCommand>) {
        let state = GameState::new();
        state.start_hosting("maze".to_string(), GameMode::Survival);
        let admin = Administration::with_file(temp_file(name));
        let tick_engine = Arc::new(TickEngine::new(60, state.clone()));
        let (tx, rx) = mpsc::unbounded_channel();
        let network = NetworkControl { sender: tx };
        (ConsoleHandler::new(state, admin, tick_engine, network), rx)
    }

    fn register(admin: &Administration, uuid: &str, name: &str, ip: &str) {
        admin.register_connection(ConnectedPlayer {
            uuid: uuid.to_string(),
            name: name.to_string(),
            ip: ip.to_string(),
            player_id: 1_000_001,
            unit_id: 2_000_001,
        });
    }

    #[tokio::test]
    async fn ban_persists_and_kicks_connected_player() {
        let path = temp_file("ban-kick");
        let state = GameState::new();
        let admin = Administration::with_file(path.clone());
        register(&admin, "uuid-player", "Attacker", "203.0.113.9");
        let tick_engine = Arc::new(TickEngine::new(60, state.clone()));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let handler = ConsoleHandler::new(
            state,
            admin.clone(),
            tick_engine,
            NetworkControl { sender: tx },
        );

        handler
            .handle_command(ServerCommand::Ban {
                target: "attacker".to_string(),
                kind: BanKind::Auto,
            })
            .await;

        // Persisted ban on both ID and IP.
        assert!(admin.is_banned("203.0.113.9", "uuid-player"));
        // Immediate expulsion: a KickCallPacket command is queued for the name.
        match rx.try_recv() {
            Ok(RuntimeCommand::Kick(target)) => assert_eq!(target, "Attacker"),
            other => panic!("expected Kick command, got {:?}", other),
        }
        drop(handler);
        let reloaded = Administration::with_file(path.clone());
        assert!(reloaded.is_banned("203.0.113.9", "uuid-player"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn pause_flag_toggles_simulation_active() {
        let (handler, mut rx) = harness("pause");
        handler
            .handle_command(ServerCommand::Pause { on: true })
            .await;
        // The command routes through the runtime channel; the listener task
        // flips `is_paused`, which gates the simulation loop via is_active().
        match rx.try_recv() {
            Ok(RuntimeCommand::Pause(true)) => {}
            other => panic!("expected RuntimeCommand::Pause(true), got {:?}", other),
        }
        handler
            .handle_command(ServerCommand::Pause { on: false })
            .await;
        match rx.try_recv() {
            Ok(RuntimeCommand::Pause(false)) => {}
            other => panic!("expected RuntimeCommand::Pause(false), got {:?}", other),
        }
        // `resume` parses to the same off-pause command.
        assert_eq!(
            ServerCommand::parse("resume"),
            Some(ServerCommand::Pause { on: false })
        );
        assert_eq!(
            ServerCommand::parse("pause"),
            Some(ServerCommand::Pause { on: true })
        );
        assert_eq!(
            ServerCommand::parse("pause off"),
            Some(ServerCommand::Pause { on: false })
        );
    }

    #[tokio::test]
    async fn team_command_forwards_to_runtime() {
        let (handler, mut rx) = harness("team");
        handler
            .handle_command(ServerCommand::Team {
                player: "Alpha".to_string(),
                team: "blue".to_string(),
            })
            .await;
        match rx.try_recv() {
            Ok(RuntimeCommand::SetTeam { player, team }) => {
                assert_eq!(player, "Alpha");
                assert_eq!(team, "blue");
            }
            other => panic!("expected RuntimeCommand::SetTeam, got {:?}", other),
        }
        assert_eq!(
            ServerCommand::parse("team Alpha blue"),
            Some(ServerCommand::Team {
                player: "Alpha".to_string(),
                team: "blue".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn playerlimit_changes_in_hot() {
        let (handler, _rx) = harness("playerlimit");
        handler
            .handle_command(ServerCommand::PlayerLimit { limit: Some(7) })
            .await;
        assert_eq!(handler.admin.get_player_limit(), 7);
        handler
            .handle_command(ServerCommand::PlayerLimit { limit: Some(0) })
            .await;
        assert_eq!(handler.admin.get_player_limit(), 0);
        assert_eq!(
            ServerCommand::parse("playerlimit 12"),
            Some(ServerCommand::PlayerLimit { limit: Some(12) })
        );
        assert_eq!(
            ServerCommand::parse("playerlimit off"),
            Some(ServerCommand::PlayerLimit { limit: Some(0) })
        );
        assert_eq!(
            ServerCommand::parse("playerlimit"),
            Some(ServerCommand::PlayerLimit { limit: None })
        );
    }

    #[tokio::test]
    async fn whitelist_command_management_and_filtering() {
        let path = temp_file("whitelist-cmd");
        let state = GameState::new();
        let admin = Administration::with_file(path.clone());
        let tick_engine = Arc::new(TickEngine::new(60, state.clone()));
        let (tx, _rx) = mpsc::unbounded_channel();
        let handler = ConsoleHandler::new(
            state,
            admin.clone(),
            tick_engine,
            NetworkControl { sender: tx },
        );

        handler
            .handle_command(ServerCommand::Whitelist {
                action: Some(WhitelistAction::Enable(true)),
            })
            .await;
        assert!(admin.is_whitelist_enabled());
        handler
            .handle_command(ServerCommand::Whitelist {
                action: Some(WhitelistAction::Add("wl-uuid".to_string())),
            })
            .await;
        assert!(admin.is_whitelisted("wl-uuid"));
        assert!(!admin.is_whitelisted("other-uuid"));
        // Persisted across reload.
        drop(handler);
        let reloaded = Administration::with_file(path.clone());
        assert!(reloaded.is_whitelist_enabled());
        assert!(reloaded.is_whitelisted("wl-uuid"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn stop_disconnects_all_players_and_stops_hosting() {
        let (handler, mut rx) = harness("stop");
        register(&handler.admin, "u1", "Alpha", "10.0.0.1");
        register(&handler.admin, "u2", "Beta", "10.0.0.2");

        handler.handle_command(ServerCommand::Stop).await;

        assert!(!handler.state.is_hosting.load(Ordering::SeqCst));
        assert!(handler.admin.connected_players_list().is_empty());
        let mut kicks = Vec::new();
        while let Ok(RuntimeCommand::Kick(target)) = rx.try_recv() {
            kicks.push(target);
        }
        kicks.sort();
        assert_eq!(kicks, vec!["Alpha".to_string(), "Beta".to_string()]);
    }

    #[tokio::test]
    async fn admin_command_resolves_online_name_and_persists() {
        let path = temp_file("admin-cmd");
        let state = GameState::new();
        let admin = Administration::with_file(path.clone());
        register(&admin, "uuid-boss", "Boss", "10.1.1.1");
        let tick_engine = Arc::new(TickEngine::new(60, state.clone()));
        let (tx, _rx) = mpsc::unbounded_channel();
        let handler = ConsoleHandler::new(
            state,
            admin.clone(),
            tick_engine,
            NetworkControl { sender: tx },
        );

        handler
            .handle_command(ServerCommand::Admin {
                add: true,
                target: "boss".to_string(),
            })
            .await;
        assert!(admin.is_admin("uuid-boss"));
        handler
            .handle_command(ServerCommand::Admin {
                add: false,
                target: "uuid-boss".to_string(),
            })
            .await;
        assert!(!admin.is_admin("uuid-boss"));
        drop(handler);
        let reloaded = Administration::with_file(path.clone());
        assert!(!reloaded.is_admin("uuid-boss"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn config_command_reads_and_writes_runtime_values() {
        let (handler, _rx) = harness("config");
        handler
            .handle_command(ServerCommand::Config {
                key: Some("name".to_string()),
                value: Some("Test Arena".to_string()),
            })
            .await;
        assert_eq!(handler.admin.server_name(), "Test Arena");
        handler
            .handle_command(ServerCommand::Config {
                key: Some("maxPlayers".to_string()),
                value: Some("5".to_string()),
            })
            .await;
        assert_eq!(handler.admin.get_player_limit(), 5);
        handler
            .handle_command(ServerCommand::Config {
                key: Some("build".to_string()),
                value: Some("159".to_string()),
            })
            .await;
        assert_eq!(handler.admin.server_build(), 159);
        // Parsing keeps multi-word values (desc).
        assert_eq!(
            ServerCommand::parse("config desc a server with space"),
            Some(ServerCommand::Config {
                key: Some("desc".to_string()),
                value: Some("a server with space".to_string()),
            })
        );
    }

    #[tokio::test]
    async fn players_command_lists_registry() {
        let (handler, _rx) = harness("players");
        register(&handler.admin, "u1", "Alpha", "10.0.0.1");
        handler.admin.add_admin("u1");
        handler.handle_command(ServerCommand::Players).await;
        handler.handle_command(ServerCommand::Bans).await;
        // Registry is readable; the command itself only prints.
        let players = handler.admin.connected_players_list();
        assert_eq!(players.len(), 1);
        assert!(handler.admin.is_admin(&players[0].uuid));
    }

    #[tokio::test]
    async fn subnet_and_dos_commands() {
        let (handler, _rx) = harness("subnet-dos");
        handler
            .handle_command(ServerCommand::SubnetBan {
                action: Some(BanListAction::Add("192.168.".to_string())),
            })
            .await;
        assert!(handler.admin.is_subnet_banned("192.168.55.1"));
        handler
            .handle_command(ServerCommand::DosBan {
                action: Some(BanListAction::Add("198.51.100.2".to_string())),
            })
            .await;
        assert!(handler.admin.is_dos_blacklisted("198.51.100.2"));
        assert!(handler.admin.is_banned("198.51.100.2", "whatever"));
    }
}
