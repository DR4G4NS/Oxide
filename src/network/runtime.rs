//! Runtime/lifecycle commands.
//!
//! Extracted from `listener.rs` (P2 listener split): the long-running
//! `spawn_runtime_commands` task that executes console/runtime commands
//! (save/load/map cycle/team ops/kick) against the world store.

use crate::network::combat::enemy::cancel_transient_world_actions;
use crate::network::combat::enemy::hostile_unit_count;
use crate::network::combat::enemy::restore_base_buildings;
use crate::network::listener::apply_loaded_team_cores;
use crate::network::listener::apply_loaded_team_items;
use crate::network::listener::mode_transition_rules;
use crate::network::listener::*;
use crate::network::protocol::*;
use crate::network::units::parse_unit_type;
use crate::network::units::spawn_enemy_units;
use crate::network::world::{core_world_for_team, RuntimeCommand, SessionPlayer};
use crate::network::world::{PendingConnection, WorldStore};
use crate::state::game_state::GameMode;
use dashmap::DashMap;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModeActionReconciliation {
    pub(crate) builds: usize,
    pub(crate) breaks: usize,
    pub(crate) plans: usize,
    pub(crate) active_plans: usize,
}

/// Cancels authoritative construction work together with its TeamPlans and
/// per-session deduplication mirrors. Callers hold `persistence_lock`; no
/// cost/refund path is run during a rules switch.
pub(crate) fn reconcile_mode_transition_actions(
    world: &crate::network::world::DynamicWorld,
) -> ModeActionReconciliation {
    let result = ModeActionReconciliation {
        builds: world.pending_builds.len(),
        breaks: world.pending_breaks.len(),
        plans: world
            .team_build_plans
            .read()
            .teams
            .iter()
            .map(|team| team.plans.len())
            .sum(),
        active_plans: world
            .player_sessions
            .iter()
            .map(|session| session.active_plans.len())
            .sum(),
    };
    world.pending_builds.clear();
    world.pending_breaks.clear();
    world.team_build_plans.write().teams.clear();
    for mut session in world.player_sessions.iter_mut() {
        session.active_plans.clear();
    }
    result
}

/// Mode-specific wave spawn set. Official WaveSpawner emits Attack-mode
/// waves from the wave team's cores in addition to the map's `Blocks.spawn`
/// overlays (the fresh-host path in `wire/bootstrap.rs` does the same).
/// Re-deriving this on every runtime mode switch keeps `world.enemy_spawns`
/// consistent with the new mode instead of leaking Attack spawn points into
/// Survival waves or losing them when entering Attack.
pub(crate) fn mode_enemy_spawns(
    base_spawns: Vec<(i16, i16)>,
    buildings: &[crate::engine::world_stream::NetworkBuilding],
    mode: GameMode,
    wave_team: u8,
) -> Vec<(i16, i16)> {
    let mut spawns = base_spawns;
    if mode == GameMode::Attack {
        extend_attack_spawns_for_team(&mut spawns, buildings, wave_team);
    }
    spawns
}

/// Commits the world-state half of a runtime `SetMode` switch. The caller
/// holds `persistence_lock`. Pending ConstructBlocks are left in place so
/// `ConstructBlock.construct` can re-read the new `infiniteResources` flag.
/// Returns how many stale wave-team units were removed.
pub(crate) fn apply_mode_switch_world_state(
    world: &crate::network::world::DynamicWorld,
    game_mode: GameMode,
    next_rules: crate::network::units::WaveRules,
    next_enemy_spawns: Vec<(i16, i16)>,
) -> usize {
    let previous_wave_team = world.wave_rules.read().wave_team;
    *world.game_state.mode.write() = game_mode;
    *world.wave_rules.write() = next_rules;
    world.game_state.infinite_resources.store(
        world.wave_rules.read().infinite_resources,
        Ordering::Relaxed,
    );
    // Official Logic.play(): the first wave countdown uses the newly
    // selected rules' initial spacing.
    *world.game_state.wave_time.write() = world.wave_rules.read().initial_wave_spacing;
    // Spawn set follows the new mode: entering Attack gains the wave team's
    // cores as spawn points, leaving Attack drops them again.
    *world.enemy_spawns.write() = next_enemy_spawns;
    let mut removed_wave_units = 0usize;
    if matches!(game_mode, GameMode::Pvp | GameMode::Sandbox) {
        // PvP/Sandbox never run the enemy simulation
        // (`simulate_waves_and_enemies` early-outs), so wave units carried
        // over from the previous mode would freeze in place as invulnerable
        // ghosts. Remove the finished game's wave-team units; other teams'
        // factory/console-spawned units stay.
        let stale_ids: Vec<i32> = world
            .enemies
            .iter()
            .filter(|enemy| enemy.team == previous_wave_team)
            .map(|enemy| enemy.id)
            .collect();
        for unit_id in stale_ids {
            world.enemies.remove(&unit_id);
            world.unregister_unit_group(unit_id);
            removed_wave_units += 1;
        }
        if removed_wave_units > 0 {
            // In-flight damage from the finished game must not land after
            // the switch.
            world.projectiles.clear();
            world
                .game_state
                .enemies_count
                .store(hostile_unit_count(world), Ordering::Relaxed);
        }
    }
    // Cancelled TeamPlans invalidate cached rebuild sites, abandonment marks
    // and builder pursuit targets; the BuildAI rescans from a clean slate in
    // the new mode.
    *world.ai_rebuild_state.lock() = Default::default();
    // Cleared units and the new spawn set change navigation consumers.
    world.navigation_revision.fetch_add(1, Ordering::Relaxed);
    // A runtime mode switch starts a fresh game on the same world. Carrying
    // a stale game-over flag into the new mode would make the rotation loop
    // re-host (destroying the switched world) after roundExtraTime ticks —
    // previously only Sandbox cleared it, so Survival/PvP/Attack switches
    // corrupted the running world whenever a previous game had ended.
    world.game_state.game_over.store(false, Ordering::Relaxed);
    *world.game_state.game_stats.write() = Default::default();
    removed_wave_units
}

pub fn save_slot_path(base: &Path, slot: &str) -> std::io::Result<PathBuf> {
    if slot.is_empty()
        || slot.len() > 64
        || !slot
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(Error::new(ErrorKind::InvalidInput, "invalid save slot"));
    }
    let stem = base
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("world-delta");
    let extension = base
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("json");
    Ok(base.with_file_name(format!("{stem}-{slot}.{extension}")))
}

pub fn spawn_runtime_commands(
    store: WorldStore,
    connections: Arc<DashMap<i32, PendingConnection>>,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<RuntimeCommand>,
    admin: crate::state::administration::Administration,
) {
    tokio::spawn(async move {
        while let Some(command) = receiver.recv().await {
            let world = store.load();
            match command {
                RuntimeCommand::Save(slot) => match save_slot_path(&world.save_path, &slot)
                    .and_then(|path| {
                        let _guard = world.persistence_lock.lock();
                        persist_tiles(
                            &path,
                            &world.tiles,
                            &world.game_state,
                            &world.enemies,
                            &world.base_buildings,
                            &world.player_profiles,
                            &world.building_commands,
                            &world.unit_orders,
                            &world.team_build_plans.read(),
                            (&world.cores, &world.team_core_lists),
                            &world.logic_flags,
                            &world.puddles,
                            checkpoint_rules_json(&world),
                        )
                        .map(|_| path)
                    }) {
                    Ok(path) => info!("Saved runtime world to {}", path.display()),
                    Err(err) => warn!("Could not save slot '{}': {}", slot, err),
                },
                RuntimeCommand::SaveMsav(slot) => {
                    // SOL-008: export the current world as an official .msav
                    // (v11) that the desktop client can open. The map region
                    // uses the base terrain with built tiles overlaid; live
                    // building/unit entities are still pending.
                    let path =
                        save_slot_path(&world.save_path, &slot).map(|p| p.with_extension("msav"));
                    let result = path.and_then(|path| {
                        let _guard = world.persistence_lock.lock();
                        let mut meta = std::collections::HashMap::new();
                        meta.insert("mapname".into(), world.game_state.map_name.read().clone());
                        meta.insert(
                            "wave".into(),
                            world.game_state.wave.load(Ordering::Relaxed).to_string(),
                        );
                        meta.insert(
                            "tick".into(),
                            world.game_state.simulation_time.read().to_string(),
                        );
                        meta.insert("width".into(), world.width.to_string());
                        meta.insert("height".into(), world.height.to_string());
                        meta.insert(
                            "build".into(),
                            crate::compat_target::CURRENT_PROTOCOL_BUILD.to_string(),
                        );
                        meta.insert(
                            "rules".into(),
                            crate::engine::msav_roundtrip::rules_json_from_world(&world),
                        );
                        meta.insert(
                            "saved".into(),
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis().to_string())
                                .unwrap_or_default(),
                        );
                        // Overlay built tiles onto the base block map.
                        let mut blocks: Vec<i16> = world.base_blocks.clone();
                        for tile in world.tiles.iter() {
                            let x = (tile.position >> 16) as i16 as i32;
                            let y = tile.position as i16 as i32;
                            if (0..world.width).contains(&x) && (0..world.height).contains(&y) {
                                let index = (y * world.width + x) as usize;
                                if index < blocks.len() {
                                    blocks[index] = tile.block;
                                }
                            }
                        }
                        let enemy_units: Vec<_> = world
                            .enemies
                            .iter()
                            .map(|entry| entry.value().clone())
                            .collect();
                        let puddles: Vec<_> = world
                            .puddles
                            .puddles
                            .iter()
                            .map(|entry| {
                                (
                                    *entry.key(),
                                    entry.value().amount,
                                    entry.value().liquid,
                                    entry.value().entity_id,
                                )
                            })
                            .collect();
                        let bytes = crate::engine::save_io::write_msav_complete(
                            &meta,
                            crate::compat_target::CURRENT_SAVE_VERSION,
                            &crate::engine::save_io::MsavWorld {
                                width: world.width as usize,
                                height: world.height as usize,
                                floors: &world.floors,
                                overlays: &world.overlays,
                                blocks: &blocks,
                                team_blocks: Some(&world.team_build_plans.read().clone()),
                                dynamic_tiles: &world.tiles,
                                enemy_units: &enemy_units,
                                puddles: &puddles,
                                runtime: Some(&world),
                            },
                        )?;
                        std::fs::write(&path, bytes)?;
                        Ok(path)
                    });
                    match result {
                        Ok(path) => info!("Exported .msav world to {}", path.display()),
                        Err(err) => warn!("Could not export .msav slot '{}': {}", slot, err),
                    }
                }
                RuntimeCommand::Load(slot) => {
                    let result = save_slot_path(&world.save_path, &slot).and_then(|path| {
                        let loaded = load_tiles(&path, Some((world.width, world.height)))?;
                        if loaded
                            .map_name
                            .as_ref()
                            .is_some_and(|name| name != &*world.game_state.map_name.read())
                        {
                            return Err(Error::new(
                                ErrorKind::InvalidData,
                                "save slot belongs to a different map",
                            ));
                        }
                        let _guard = world.persistence_lock.lock();
                        world.tiles.clear();
                        for tile in loaded.tiles.iter() {
                            crate::network::world::note_building_generation(tile.generation);
                            world.tiles.insert(*tile.key(), tile.value().clone());
                        }
                        crate::network::buildings::power::normalize_power_links(&world);
                        restore_base_buildings(&world, &loaded.base_building_health);
                        apply_loaded_team_cores(&world, &loaded);
                        apply_loaded_team_items(&world, &loaded);
                        apply_loaded_wave_rules(&world, &loaded);
                        apply_wave_rules_overrides(&world, &admin);
                        *world.team_build_plans.write() = loaded.team_build_plans.clone();
                        world.enemies.clear();
                        world.unit_group_order.lock().clear();
                        let mut next_enemy_id = 3_000_000;
                        for enemy in loaded.enemies {
                            next_enemy_id = next_enemy_id.max(enemy.id.saturating_add(1));
                            let enemy_id = enemy.id;
                            world.enemies.insert(enemy_id, enemy);
                            world.register_unit_group(enemy_id);
                        }
                        world.players.clear();
                        world.player_sessions.clear();
                        world.player_profiles.clear();
                        for player in loaded.players {
                            world.player_profiles.insert(player.uuid.clone(), player);
                        }
                        world.building_commands.clear();
                        for command in loaded.building_commands {
                            world.building_commands.insert(command.position, command);
                        }
                        world.unit_orders.clear();
                        for order in loaded.unit_orders {
                            world.unit_orders.insert(order.unit_id, order);
                        }
                        world.next_enemy_id.store(next_enemy_id, Ordering::Relaxed);
                        cancel_transient_world_actions(&world);
                        if let Some(items) = loaded.core_items {
                            *world.game_state.core_items.write() = items;
                        }
                        if crate::network::core_inventory::clamp_core_inventories(&world) {
                            world.persistence_dirty.store(true, Ordering::Relaxed);
                        }
                        if let Some(simulation_time) = loaded.simulation_time {
                            *world.game_state.simulation_time.write() = simulation_time;
                        }
                        for (name, value) in &loaded.logic_flags {
                            world.logic_flags.insert(name.clone(), *value);
                        }
                        *world.game_state.game_stats.write() = loaded.game_stats.clone();
                        if let Some(wave) = loaded.wave {
                            world.game_state.wave.store(wave, Ordering::Relaxed);
                        }
                        if let Some(wave_time) = loaded.wave_time {
                            *world.game_state.wave_time.write() = wave_time;
                        }
                        if let Some(core_health) = loaded.core_health {
                            *world.game_state.core_health.write() = core_health;
                        }
                        // Game-over is ephemeral runtime state: a console
                        // `load` never restores a finished game.
                        world
                            .game_state
                            .enemies_count
                            .store(hostile_unit_count(&world), Ordering::Relaxed);
                        world.navigation_revision.fetch_add(1, Ordering::Relaxed);
                        persist_tiles(
                            &world.save_path,
                            &world.tiles,
                            &world.game_state,
                            &world.enemies,
                            &world.base_buildings,
                            &world.player_profiles,
                            &world.building_commands,
                            &world.unit_orders,
                            &world.team_build_plans.read(),
                            (&world.cores, &world.team_core_lists),
                            &world.logic_flags,
                            &world.puddles,
                            checkpoint_rules_json(&world),
                        )?;
                        Ok(path)
                    });
                    match result {
                        Ok(path) => {
                            info!("Loaded runtime world from {}", path.display());
                            if let Ok(payload) = encode_typeio_string(
                                "[accent]World loaded by console; reconnecting is required.",
                            ) {
                                if let Ok(frame) =
                                    frame_generated_packet(KICK_PACKET_ID, &payload, false)
                                {
                                    broadcast(&connections, frame);
                                }
                            }
                        }
                        Err(err) => warn!("Could not load slot '{}': {}", slot, err),
                    }
                }
                RuntimeCommand::Kick(target) => {
                    let target = target.to_lowercase();
                    let payload = encode_typeio_string("Kicked by server console");
                    if let Ok(payload) = payload {
                        if let Ok(frame) = frame_generated_packet(KICK_PACKET_ID, &payload, false) {
                            let mut matched = 0;
                            for connection in connections.iter() {
                                let name_matches = connection
                                    .player_name
                                    .read()
                                    .as_ref()
                                    .is_some_and(|name| name.to_lowercase() == target);
                                if name_matches || connection.ip.to_string() == target {
                                    enqueue_outbound(&connection, frame.clone(), true);
                                    matched += 1;
                                    // M4: every production kick path goes
                                    // through handleKicked(uuid, ip,
                                    // duration); the official console kick
                                    // uses `kick(reason)` with duration 0
                                    // (no cooldown registered).
                                    let uuid = world
                                        .player_sessions
                                        .iter()
                                        .find(|session| {
                                            session.value().name.to_lowercase() == target
                                                || session
                                                    .value()
                                                    .uuid
                                                    .eq_ignore_ascii_case(&target)
                                        })
                                        .map(|session| session.value().uuid.clone());
                                    if let Some(uuid) = uuid {
                                        admin.handle_kicked(
                                            &uuid,
                                            &connection.ip.to_string(),
                                            std::time::Duration::ZERO,
                                        );
                                    }
                                }
                            }
                            if matched == 0 {
                                warn!("No connected player matched '{}'", target);
                            }
                        }
                    }
                }
                RuntimeCommand::Say(message) => {
                    if let Ok(payload) =
                        encode_typeio_string(&format!("[accent][Server][] {message}"))
                    {
                        if let Ok(frame) =
                            frame_generated_packet(SEND_MESSAGE_PACKET_ID, &payload, false)
                        {
                            broadcast(&connections, frame);
                        }
                    }
                }
                RuntimeCommand::GameOver => {
                    world.game_state.game_over.store(true, Ordering::Relaxed);
                    // Winner is the waveTeam (enemy team 2) like the official
                    // `gameover` command: ServerControl fires
                    // GameOverEvent(state.rules.waveTeam) -> Call.gameOver.
                    emit_game_over_packet(&world, &connections);
                }
                RuntimeCommand::SpawnEnemy { unit, count, x, y } => {
                    let Some(unit_type) = parse_unit_type(&unit) else {
                        warn!(
                            "Cannot spawn '{}': unknown unit name or unsupported id",
                            unit
                        );
                        continue;
                    };
                    let spawned = spawn_enemy_units(&world, unit_type, count, x, y);
                    if spawned == 0 {
                        // B13: strict mode rejects unit spawns without an
                        // enemy_spec instead of silently dropping them (same
                        // policy as map spawn groups and logic `spawn`).
                        if world.game_state.strict_mode.load(Ordering::Relaxed)
                            && crate::network::units::enemy_spec(unit_type).is_none()
                        {
                            error!(
                                "strict mode: console spawn of unsupported unit '{}' rejected",
                                unit
                            );
                        } else {
                            warn!(
                                "Could not spawn '{}' x{}: unsupported unit, zero count, or no enemy spawns",
                                unit, count
                            );
                        }
                    } else {
                        info!("Spawned {} enemy {} unit(s) at team 2", spawned, unit);
                    }
                }
                RuntimeCommand::HostMap { map, mode } => {
                    match host_map(&store, &connections, &map, &mode, Some(&admin)) {
                        Ok(result) => info!(
                            "Console `host {} {}` -> map '{}', {} re-streamed, {} kicked",
                            map, mode, result.map_name, result.restreamed, result.kicked
                        ),
                        Err(err) => warn!("Could not host map '{}': {}", map, err),
                    }
                }
                RuntimeCommand::SetTeam { player, team } => {
                    let Some(team_id) = parse_team_id(&team) else {
                        warn!("Cannot assign team '{}': unknown team name or id", team);
                        continue;
                    };
                    let target = player.to_lowercase();
                    let sessions: Vec<SessionPlayer> = world
                        .player_sessions
                        .iter()
                        .map(|entry| entry.value().clone())
                        .collect();
                    let Some(session) = sessions.iter().find(|session| {
                        session.name.to_lowercase() == target
                            || session.uuid.eq_ignore_ascii_case(&target)
                    }) else {
                        warn!("No connected player matched '{}'", player);
                        continue;
                    };
                    let Some(mut combat) = world.players.get_mut(&session.unit_id) else {
                        warn!("Player '{}' has no live combat state", session.name);
                        continue;
                    };
                    combat.team = team_id;
                    let profile = combat.clone();
                    drop(combat);
                    world
                        .player_profiles
                        .insert(session.uuid.clone(), profile.clone());
                    if let Ok(snapshot) = encode_initial_entity_snapshot(session, Some(&profile)) {
                        if let Ok(frame) =
                            frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true)
                        {
                            broadcast(&connections, frame);
                        }
                    }
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                    info!("Assigned player '{}' to team {}", session.name, team_id);
                }
                RuntimeCommand::NextMap => {
                    let mode = format!("{:?}", *world.game_state.mode.read()).to_lowercase();
                    match admin.advance_map() {
                        Some(next) => {
                            info!("Console `nextmap` -> hosting '{}'", next);
                            if let Err(err) =
                                host_map(&store, &connections, &next, &mode, Some(&admin))
                            {
                                warn!("Could not host next map '{}': {}", next, err);
                            }
                        }
                        None => warn!("No map rotation configured; nextmap does nothing"),
                    }
                }
                RuntimeCommand::Pause(paused) => {
                    // Manual pause/resume: clear the auto-pause marker so the
                    // PvP auto-pause logic never overrides an explicit
                    // console command (official `pause`/`resume`).
                    world
                        .game_state
                        .pvp_auto_paused
                        .store(false, Ordering::Relaxed);
                    world.game_state.is_paused.store(paused, Ordering::Relaxed);
                    info!(
                        "Console {} the game",
                        if paused { "paused" } else { "resumed" }
                    );
                }
                RuntimeCommand::SetMode { mode } => {
                    let game_mode = match mode.as_str() {
                        "survival" => Some(GameMode::Survival),
                        "sandbox" => Some(GameMode::Sandbox),
                        "pvp" => Some(GameMode::Pvp),
                        "attack" => Some(GameMode::Attack),
                        _ => None,
                    };
                    let Some(game_mode) = game_mode else {
                        warn!("Unknown game mode '{}'; leaving mode unchanged", mode);
                        continue;
                    };
                    // P0-6: transactional mode switch. The preset is applied
                    // to the LIVE WaveRules (re-derived from the map template
                    // so survival restores the map's original values after a
                    // sandbox session), teams are recomputed, every client is
                    // re-streamed with the same personalized Rules, and any
                    // error aborts BEFORE the first mutation.
                    let next_rules = match mode_transition_rules(&world, game_mode) {
                        Ok(rules) => rules,
                        Err(err) => {
                            warn!(
                                "SetMode: cannot re-derive map rules from template ({}); leaving mode unchanged",
                                err
                            );
                            continue;
                        }
                    };
                    // Mode-specific spawn set, decoded from the same map
                    // template as the rules so an unreadable template aborts
                    // before any mutation.
                    let next_enemy_spawns = match crate::engine::world_stream::inspect_map(
                        &world.network_template,
                    ) {
                        Ok(base_map) => mode_enemy_spawns(
                            base_map.enemy_spawns(),
                            &base_map.buildings,
                            game_mode,
                            next_rules.wave_team,
                        ),
                        Err(err) => {
                            warn!(
                                "SetMode: cannot decode map spawns from template ({}); leaving mode unchanged",
                                err
                            );
                            continue;
                        }
                    };
                    let sessions: Vec<SessionPlayer> = world
                        .player_sessions
                        .iter()
                        .map(|entry| entry.value().clone())
                        .collect();
                    // Pre-compute team reassignments (reads current state).
                    let mut reassignments: Vec<(SessionPlayer, u8, (f32, f32))> = Vec::new();
                    for session in &sessions {
                        let current_team = world
                            .players
                            .get(&session.unit_id)
                            .map(|combat| combat.team)
                            .unwrap_or(1);
                        let team = if game_mode == GameMode::Pvp {
                            assign_team_for_join(&world, &session.uuid, current_team)
                        } else {
                            1
                        };
                        let (spawn_x, spawn_y) = core_world_for_team(&world, team);
                        reassignments.push((session.clone(), team, (spawn_x, spawn_y)));
                    }
                    // Vanilla `Call.setRules` (id 119). WorldDataBegin plus a
                    // full world restream is the host-map path. ConstructBlock
                    // re-reads `infiniteResources` every update, so pending
                    // work is kept: a survival plan finishes on the next tick
                    // once sandbox is live, and a sandbox→survival switch
                    // keeps the ConstructBuild the client is already drawing.
                    let rules_json =
                        match crate::network::wire::bootstrap::client_visible_rules_json(
                            &world,
                            &next_rules,
                        ) {
                            Ok(json) => json,
                            Err(err) => {
                                warn!(
                                "SetMode: cannot project rules JSON ({}); leaving mode unchanged",
                                err
                            );
                                continue;
                            }
                        };
                    let rules_frame =
                        match crate::network::wire::encode_set_rules_frame(&rules_json) {
                            Ok(frame) => frame,
                            Err(err) => {
                                warn!(
                                    "SetMode: cannot encode SetRules ({}); leaving mode unchanged",
                                    err
                                );
                                continue;
                            }
                        };

                    // Freeze the construction writer only for the rules
                    // commit. Do not cancel ConstructBlocks: vanilla
                    // ConstructBlock.construct/deconstruct re-reads the live
                    // rules, and cancelling left the client's ghost with no
                    // BeginPlace (tile.build is already ConstructBuild).
                    let transition_guard = world.persistence_lock.lock();

                    // Commit: mode, live rules, spawn set, authority flags,
                    // teams (see apply_mode_switch_world_state for the full
                    // world-state reset).
                    let removed_wave_units = apply_mode_switch_world_state(
                        &world,
                        game_mode,
                        next_rules,
                        next_enemy_spawns,
                    );
                    let mut player_snapshots = Vec::new();
                    for (session, team, (spawn_x, spawn_y)) in reassignments {
                        let Some(mut combat) = world.players.get_mut(&session.unit_id) else {
                            continue;
                        };
                        let team_changed = combat.team != team;
                        combat.team = team;
                        // Vanilla setRules does not teleport. Only a real team
                        // reassignment (entering/leaving PvP) moves the unit.
                        if team_changed {
                            combat.x = spawn_x;
                            combat.y = spawn_y;
                        }
                        let profile = combat.clone();
                        drop(combat);
                        world
                            .player_profiles
                            .insert(session.uuid.clone(), profile.clone());
                        if team_changed {
                            if let Ok(snapshot) = encode_initial_entity_snapshot_in(
                                &session,
                                Some(&profile),
                                Some(&world),
                            ) {
                                if let Ok(frame) = frame_generated_packet(
                                    ENTITY_SNAPSHOT_PACKET_ID,
                                    &snapshot,
                                    true,
                                ) {
                                    player_snapshots.push(frame);
                                }
                            }
                        }
                    }
                    if game_mode == GameMode::Pvp {
                        // Entering PvP: release any manual pause; the
                        // auto-pause (waiting for both teams) takes over.
                        world.game_state.is_paused.store(false, Ordering::Relaxed);
                        world
                            .game_state
                            .pvp_auto_paused
                            .store(true, Ordering::Relaxed);
                    } else {
                        world.game_state.is_paused.store(false, Ordering::Relaxed);
                        world
                            .game_state
                            .pvp_auto_paused
                            .store(false, Ordering::Relaxed);
                    }
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                    drop(transition_guard);

                    for connection in connections.iter() {
                        enqueue_outbound(&connection, rules_frame.clone(), true);
                    }
                    for frame in player_snapshots {
                        broadcast(&connections, frame);
                    }
                    if removed_wave_units > 0 {
                        info!(
                            "Mode switch removed {} stale wave-team unit(s) for {:?}",
                            removed_wave_units, game_mode
                        );
                    }
                    info!("Game mode switched to {:?}", game_mode);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::world::{
        DynamicWorld, EnemyUnit, PlayerCombatState, SessionPlayer, UnitAuthority,
    };
    use dashmap::DashMap;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
    use std::sync::Arc;

    fn test_world() -> DynamicWorld {
        let state = crate::state::game_state::GameState::new();
        state.start_hosting(
            "mode-switch-test".into(),
            crate::state::game_state::GameMode::Survival,
        );
        DynamicWorld {
            game_state: state,
            width: 40,
            height: 40,
            sharded_unit_cap: 8,
            core_position: (20 << 16) | 20,
            core_max_health: 6_000.0,
            cores: DashMap::new(),
            team_core_lists: DashMap::new(),
            base_blocks: vec![0; 40 * 40],
            base_centers: vec![false; 40 * 40],
            tile_data: Vec::new(),
            base_building_templates: Vec::new(),
            base_buildings: DashMap::new(),
            floors: vec![0; 40 * 40],
            overlays: vec![0; 40 * 40],
            enemy_spawns: parking_lot::RwLock::new(Vec::new()),
            enemies: DashMap::new(),
            players: DashMap::new(),
            player_sessions: DashMap::new(),
            player_profiles: DashMap::new(),
            building_commands: DashMap::new(),
            unit_orders: DashMap::new(),
            next_player_unit_id: AtomicI32::new(2_500_000),
            next_enemy_id: AtomicI32::new(3_000_100),
            unit_group_order: parking_lot::Mutex::new(Vec::new()),
            damaged_window: parking_lot::Mutex::new(Vec::new()),
            projectiles: DashMap::new(),
            next_projectile_id: AtomicI32::new(4_000_000),
            overdrive_boosts: DashMap::new(),
            heal_suppression: DashMap::new(),
            force_fields: DashMap::new(),
            tiles: DashMap::new(),
            pending_builds: DashMap::new(),
            pending_breaks: DashMap::new(),
            mineable_ore: std::sync::OnceLock::new(),
            mono_mining_targets: DashMap::new(),
            ai_rebuild_state: Default::default(),
            tile_footprint: DashMap::new(),
            navigation_revision: AtomicU64::new(0),
            ground_navigation: parking_lot::Mutex::new(None),
            leg_navigation: parking_lot::Mutex::new(None),
            naval_navigation: parking_lot::Mutex::new(None),
            save_path: std::env::temp_dir().join("mode-switch-test.json"),
            network_template: Arc::new(Vec::new()),
            persistence_dirty: AtomicBool::new(false),
            persistence_lock: parking_lot::Mutex::new(()),
            logic_flags: DashMap::new(),
            logic_executors: DashMap::new(),
            logic_display_commands: DashMap::new(),
            base_drill_progress: DashMap::new(),
            base_factory_progress: DashMap::new(),
            base_turret_progress: DashMap::new(),
            base_mender_progress: DashMap::new(),
            team_build_plans: parking_lot::RwLock::new(crate::engine::typeio::TeamBlocks::default()),
            wave_rules: parking_lot::RwLock::new(crate::network::units::WaveRules::default()),
            votekick_target: parking_lot::RwLock::new(None),
            votekick_votes: AtomicI32::new(0),
            votekick_voters: DashMap::new(),
            votekick_cooldowns: DashMap::new(),
            puddles: crate::network::buildings::puddles::PuddleSystem::new(),
            building_last_damage: DashMap::new(),
            repair_beam_strengths: DashMap::new(),
        }
    }

    fn wave_unit(id: i32, team: u8) -> EnemyUnit {
        EnemyUnit {
            id,
            unit_type: 0,
            entity_class: 0,
            team,
            x: 50.0,
            y: 0.0,
            rotation: 0.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 0.0,
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: 1.0,
            attack_damage: 1.0,
            attack_reload_time: 1.0,
            attack_range: 1.0,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            missile_time: 0.0,
            status_agg: None,
            drown_progress: 0.0,
        }
    }

    fn survival_rules() -> crate::network::units::WaveRules {
        crate::network::units::WaveRules {
            wave_team: 2,
            waves_enabled: true,
            wave_timer: true,
            ..crate::network::units::WaveRules::default()
        }
    }

    fn sandbox_rules() -> crate::network::units::WaveRules {
        crate::network::units::WaveRules {
            infinite_resources: true,
            wave_timer: false,
            ..survival_rules()
        }
    }

    /// Deterministic Survival -> Pvp -> Sandbox -> Attack hop sequence: every
    /// hop must leave the world internally consistent (no stale game-over, no
    /// frozen wave-team ghosts in modes without enemy simulation, spawn set
    /// matching the mode, AI plan caches reset).
    #[test]
    fn mode_switch_hop_sequence_keeps_world_invariants() {
        let world = test_world();
        *world.wave_rules.write() = survival_rules();
        // Wave-team ghosts plus a player-team factory unit.
        for id in [10, 11, 12] {
            let unit = wave_unit(id, 2);
            world.enemies.insert(id, unit);
            world.register_unit_group(id);
        }
        let friendly = wave_unit(20, 1);
        world.enemies.insert(20, friendly);
        world.register_unit_group(20);
        world.projectiles.insert(
            30,
            crate::network::world::Projectile {
                target_id: -1,
                shooter_id: -1,
                team: 2,
                bullet_id: 6,
                damage: 10.0,
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
                apply_direct_on_impact: true,
                armor_multiplier: 1.0,
                remaining_ticks: 1.0,
                total_ticks: 1.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 0.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
                collided: Vec::new(),
            },
        );
        // A finished previous game must not leak into the next mode.
        world.game_state.game_over.store(true, Ordering::Relaxed);
        world.game_state.game_stats.write().waves_lasted = 7;
        // Stale Attack-mode spawns (wave-team core) from a previous Attack
        // session and a populated AI rebuild cache.
        *world.enemy_spawns.write() = vec![(5, 5), (6, 6)];
        world
            .ai_rebuild_state
            .lock()
            .sites
            .push(crate::network::world::AiRebuildSite {
                position: 1,
                block: 257,
                rotation: 0,
                team: 1,
                config: Vec::new(),
            });
        let base_spawns = vec![(5, 5)];

        // --- Hop 1: Survival -> Pvp ---
        let removed = apply_mode_switch_world_state(
            &world,
            GameMode::Pvp,
            survival_rules(),
            base_spawns.clone(),
        );
        assert_eq!(removed, 3, "all wave-team units leave with the old game");
        assert!(world.enemies.contains_key(&20), "player-team units stay");
        assert_eq!(
            world.game_state.enemies_count.load(Ordering::Relaxed),
            hostile_unit_count(&world)
        );
        assert_eq!(hostile_unit_count(&world), 0);
        assert!(world.projectiles.is_empty(), "in-flight damage is dropped");
        assert!(!world.game_state.game_over.load(Ordering::Relaxed));
        assert_eq!(world.game_state.game_stats.read().waves_lasted, 0);
        assert!(world.ai_rebuild_state.lock().sites.is_empty());
        assert_eq!(*world.enemy_spawns.read(), base_spawns);
        assert!(matches!(*world.game_state.mode.read(), GameMode::Pvp));

        // --- Hop 2: Pvp -> Sandbox ---
        let removed = apply_mode_switch_world_state(
            &world,
            GameMode::Sandbox,
            sandbox_rules(),
            base_spawns.clone(),
        );
        assert_eq!(removed, 0, "no wave-team units remain to remove");
        assert!(!world.game_state.game_over.load(Ordering::Relaxed));
        assert!(
            world.wave_rules.read().infinite_resources,
            "sandbox preset applies infinite resources"
        );

        // --- Hop 3: Sandbox -> Attack ---
        // Attack gains the wave team's core as an extra spawn point.
        let enemy_core = vec![crate::engine::world_stream::NetworkBuilding {
            position: (9 << 16) | 9,
            block: 344,
            health: 100.0,
            team: 2,
            rotation: 0,
            inventory: Vec::new(),
            power_links: Vec::new(),
            power_status: 0.0,
            liquids: Vec::new(),
            enabled: true,
            extra_data: Vec::new(),
        }];
        let attack_spawns =
            mode_enemy_spawns(base_spawns.clone(), &enemy_core, GameMode::Attack, 2);
        assert_eq!(
            attack_spawns,
            vec![(5, 5), (9, 9)],
            "entering Attack adds the wave team's core spawn"
        );
        apply_mode_switch_world_state(&world, GameMode::Attack, survival_rules(), attack_spawns);
        assert_eq!(world.enemy_spawns.read().len(), base_spawns.len() + 1);

        // --- Hop 4: Attack -> Survival ---
        apply_mode_switch_world_state(
            &world,
            GameMode::Survival,
            survival_rules(),
            base_spawns.clone(),
        );
        assert_eq!(
            *world.enemy_spawns.read(),
            base_spawns,
            "leaving Attack drops the wave-team-core spawn point"
        );
        assert!(!world.game_state.game_over.load(Ordering::Relaxed));
        assert!(matches!(*world.game_state.mode.read(), GameMode::Survival));
    }

    #[test]
    fn mode_enemy_spawns_match_fresh_host_behaviour() {
        // Non-Attack modes keep exactly the map overlay spawns.
        let base = vec![(1, 2)];
        assert_eq!(
            mode_enemy_spawns(base.clone(), &[], GameMode::Survival, 2),
            base
        );
        assert_eq!(mode_enemy_spawns(base.clone(), &[], GameMode::Pvp, 2), base);
        assert_eq!(
            mode_enemy_spawns(base.clone(), &[], GameMode::Sandbox, 2),
            base
        );
        assert_eq!(
            mode_enemy_spawns(base.clone(), &[], GameMode::Attack, 2),
            base
        );
        // Attack adds the wave team's core positions, deduplicated.
        let cores = vec![crate::engine::world_stream::NetworkBuilding {
            position: (9 << 16) | 9,
            block: 344,
            health: 100.0,
            team: 2,
            rotation: 0,
            inventory: Vec::new(),
            power_links: Vec::new(),
            power_status: 0.0,
            liquids: Vec::new(),
            enabled: true,
            extra_data: Vec::new(),
        }];
        let spawns = mode_enemy_spawns(vec![], &cores, GameMode::Attack, 2);
        assert_eq!(spawns, vec![(9, 9)]);
        // A different wave team's core is not used as an Attack spawn.
        let spawns = mode_enemy_spawns(vec![], &cores, GameMode::Attack, 3);
        assert!(spawns.is_empty());
    }
}
