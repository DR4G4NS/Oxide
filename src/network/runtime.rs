//! Runtime/lifecycle commands.
//!
//! Extracted from `listener.rs` (P2 listener split): the long-running
//! `spawn_runtime_commands` task that executes console/runtime commands
//! (save/load/map cycle/team ops/kick) against the world store.

use crate::network::buildings::construction::network_template_with_plans;
use crate::network::buildings::construction::ModeRestream;
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
                    // Freeze the simulation/build-plan writer for the entire
                    // transition. Pending ConstructBlocks were scheduled with
                    // the OLD rules; carrying their cached remaining_ticks
                    // into a new infinite/survival mode can charge or refund
                    // under a different rule set. Cancel both authoritative
                    // work and its visual TeamPlans half before restreaming.
                    let transition_guard = world.persistence_lock.lock();
                    let saved_pending_builds: Vec<_> = world
                        .pending_builds
                        .iter()
                        .map(|entry| entry.value().clone())
                        .collect();
                    let saved_pending_breaks: Vec<_> = world
                        .pending_breaks
                        .iter()
                        .map(|entry| entry.value().clone())
                        .collect();
                    let saved_team_plans = world.team_build_plans.read().clone();
                    let saved_active_plans: Vec<_> = world
                        .player_sessions
                        .iter()
                        .map(|session| (*session.key(), session.active_plans.clone()))
                        .collect();
                    let reconciled = reconcile_mode_transition_actions(&world);

                    // Build every personalized stream from the reconciled
                    // state before committing mode/rules. WorldDataBegin makes
                    // the client replace its local world/plan queue, and the
                    // template now contains no TeamPlans; clearing the matching
                    // server-side active_plans prevents either half from
                    // resurrecting a cancelled plan after the reload. On error,
                    // restore the exact pending state and leave the mode untouched.
                    let restream = (|| -> std::io::Result<ModeRestream> {
                        let template = network_template_with_plans(&world)?;
                        let wave = world.game_state.wave.load(Ordering::Relaxed);
                        // Logic.play() resets the first countdown to the newly
                        // selected rules' initial spacing. Serialize that same
                        // value in the stream built before the commit.
                        let wave_time = next_rules.initial_wave_spacing;
                        let tick = f64::from(*world.game_state.simulation_time.read());
                        let begin_frame =
                            frame_generated_packet(WORLD_DATA_BEGIN_PACKET_ID, &[], false)?;
                        let mut streams = Vec::new();
                        for (session, _team, (spawn_x, spawn_y)) in &reassignments {
                            let stream =
                                crate::engine::world_stream::personalize_current_with_state_mode(
                                    &template,
                                    session.id,
                                    &session.name,
                                    session.color,
                                    (*spawn_x, *spawn_y),
                                    wave,
                                    wave_time,
                                    tick,
                                    game_mode == GameMode::Pvp,
                                    game_mode == GameMode::Sandbox,
                                )?;
                            streams.push((session.id, stream));
                        }
                        Ok((begin_frame, streams))
                    })();
                    let (begin_frame, streams) = match restream {
                        Ok(streams) => streams,
                        Err(err) => {
                            for pending in saved_pending_builds {
                                world.pending_builds.insert(pending.position, pending);
                            }
                            for pending in saved_pending_breaks {
                                world.pending_breaks.insert(pending.position, pending);
                            }
                            *world.team_build_plans.write() = saved_team_plans;
                            for (unit_id, active_plans) in saved_active_plans {
                                if let Some(mut session) = world.player_sessions.get_mut(&unit_id) {
                                    session.active_plans = active_plans;
                                }
                            }
                            drop(transition_guard);
                            warn!(
                                "SetMode: cannot personalize world stream ({}); leaving mode unchanged",
                                err
                            );
                            continue;
                        }
                    };
                    // Commit: mode, live rules, authority flags, teams.
                    *world.game_state.mode.write() = game_mode;
                    *world.wave_rules.write() = next_rules;
                    world.game_state.infinite_resources.store(
                        world.wave_rules.read().infinite_resources,
                        Ordering::Relaxed,
                    );
                    // Official Logic.play(): the first wave countdown uses the
                    // (possibly mode-adjusted) initial spacing.
                    *world.game_state.wave_time.write() =
                        world.wave_rules.read().initial_wave_spacing;
                    for (session, team, (spawn_x, spawn_y)) in reassignments {
                        let Some(mut combat) = world.players.get_mut(&session.unit_id) else {
                            continue;
                        };
                        combat.team = team;
                        combat.x = spawn_x;
                        combat.y = spawn_y;
                        let profile = combat.clone();
                        drop(combat);
                        world
                            .player_profiles
                            .insert(session.uuid.clone(), profile.clone());
                        if let Ok(snapshot) =
                            encode_initial_entity_snapshot(&session, Some(&profile))
                        {
                            if let Ok(frame) =
                                frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true)
                            {
                                broadcast(&connections, frame);
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
                    // Round 74d: entering sandbox clears a stale game-over
                    // flag so the rotation loop can never re-host a sandbox
                    // world (the legacy session re-hosted every 12 s after
                    // a core was destroyed).
                    if game_mode == crate::state::game_state::GameMode::Sandbox {
                        world.game_state.game_over.store(false, Ordering::Relaxed);
                    }
                    drop(transition_guard);

                    // Publish the same personalized world (Rules included) to
                    // every connection: WorldDataBegin + stream triggers the
                    // official reload flow (same as the hot-swap path).
                    for connection in connections.iter() {
                        let player_id = 1_000_000 + *connection.key();
                        let Some(stream) = streams.iter().find(|(id, _)| *id == player_id) else {
                            continue;
                        };
                        enqueue_outbound(&connection, begin_frame.clone(), true);
                        match world_stream_frames(&stream.1) {
                            Ok(frames) => {
                                for frame in frames {
                                    enqueue_outbound(&connection, frame, true);
                                }
                            }
                            Err(err) => {
                                warn!("SetMode: could not frame stream for {}: {}", player_id, err);
                            }
                        }
                    }
                    if reconciled != ModeActionReconciliation::default() {
                        info!(
                            "Mode switch cancelled {} build(s), {} break(s), removed {} team plan(s) and {} session plan marker(s)",
                            reconciled.builds,
                            reconciled.breaks,
                            reconciled.plans,
                            reconciled.active_plans
                        );
                    }
                    info!("Game mode switched to {:?}", game_mode);
                }
            }
        }
    });
}
