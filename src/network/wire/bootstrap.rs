//! Map hosting, wave-rules/mode overrides, host-map and game-over. The
//! listener adapter re-exports these through crate::network::listener::*.

use crate::network::world::*;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// Builds a fresh `DynamicWorld` from a validated network-world template
/// (either the embedded 158.1 template or one produced by
/// `replace_map_from_msav`). The world starts empty: no dynamic tiles,
/// enemies, players or build progress; team build plans and the base-map
/// layout (blocks, floors, overlays, spawns, core, base buildings) come from
/// the template. `map_name` becomes the identity persisted in saves.
/// Official WaveSpawner (Attack): the enemy (waveTeam) cores are ground
/// spawn points. Merges their positions into the overlay spawn list.
use crate::network::buildings::snapshot::*;
use crate::network::codec::Writes;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::state::game_state::{GameMode, GameState};
use dashmap::DashMap;
use tracing::{debug, info, warn};

use crate::network::decoders::read_typeio_object_raw;
use crate::network::outbound::enqueue_outbound;
use crate::network::session::world_stream_frames;
use crate::network::wire::encode::frame_generated_packet;
use crate::network::wire::persistence::{encode_typeio_string, persist_tiles};
use crate::network::world::core_world_for_team;

use crate::network::buildings::construction::{block_footprint_in, network_template_with_plans};
pub(crate) fn extend_attack_spawns(
    spawns: &mut Vec<(i16, i16)>,
    buildings: &[crate::engine::world_stream::NetworkBuilding],
) {
    for building in buildings {
        if building.team == 2 && (339..=344).contains(&building.block) {
            let spawn = ((building.position >> 16) as i16, building.position as i16);
            if !spawns.contains(&spawn) {
                spawns.push(spawn);
            }
        }
    }
}

pub(crate) fn extend_attack_spawns_for_team(
    spawns: &mut Vec<(i16, i16)>,
    buildings: &[crate::engine::world_stream::NetworkBuilding],
    wave_team: u8,
) {
    for building in buildings {
        if building.team == wave_team && (339..=344).contains(&building.block) {
            let spawn = ((building.position >> 16) as i16, building.position as i16);
            if !spawns.contains(&spawn) {
                spawns.push(spawn);
            }
        }
    }
}

pub(crate) fn network_building_tile(
    building: &crate::engine::world_stream::NetworkBuilding,
    occupied: Vec<i32>,
) -> DynamicTile {
    let (stored_liquid, liquid_amount) = building
        .liquids
        .iter()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .copied()
        .unwrap_or((-1, 0.0));
    let mut tile = DynamicTile {
        position: building.position,
        block: building.block,
        rotation: building.rotation,
        team: building.team,
        config: {
            // P1: the map's raw Building tail is config ONLY when it is a
            // structurally valid TypeIO object that consumes the whole
            // buffer (PROTOCOL-RULES rule 5/16: config, Building tail and
            // private metadata are different domains). Legacy tails that do
            // not parse as TypeIO stay out of config instead of leaking raw
            // bytes into snapshots/serializers.
            let mut tail_cursor = std::io::Cursor::new(building.extra_data.as_slice());
            match read_typeio_object_raw(&mut tail_cursor)
                .ok()
                .filter(|_| tail_cursor.position() == building.extra_data.len() as u64)
            {
                Some(object) => object,
                None => {
                    if !building.extra_data.is_empty() {
                        debug!(
                            "map building at {} has a non-TypeIO tail ({} bytes); not treated as config",
                            building.position,
                            building.extra_data.len()
                        );
                    }
                    vec![0]
                }
            }
        },
        enabled: building.enabled,
        message: None,
        occupied,
        stored_item: -1,
        stored_amount: 0,
        production_progress: 0.0,
        transport_progress: 0.0,
        ammo_units: 0.0,
        inventory: building.inventory.clone(),
        power_stored: building.power_status,
        power_links: building.power_links.clone(),
        liquid_inventory: building.liquids.clone(),
        stored_liquid,
        liquid_amount,
        output_liquid_amount: 0.0,
        junction_items: Vec::new(),
        mass_driver_incoming: Vec::new(),
        mass_driver_rotation: 90.0,
        mass_driver_waiting: Vec::new(),
        payload: None,
        payload_progress: 0.0,
        payload_rotation: 0.0,
        payload_accum: Vec::new(),
        health: building.health,
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
    };
    let _ = crate::engine::save_io::apply_msav_building_tail(&mut tile, &building.extra_data);
    tile
}

pub fn fresh_world_from_template(
    state: &GameState,
    network_template: Vec<u8>,
    map_name: String,
    save_path: PathBuf,
) -> std::io::Result<DynamicWorld> {
    let mode = *state.mode.read();
    fresh_world_from_template_for_mode(state, network_template, map_name, save_path, mode)
}

/// P2: applies the Administration global rules overrides (official `rules`
/// command backed by `Core.settings` "globalrules") to the live `WaveRules`
/// after the map rules + Gamemode preset have been resolved. Unknown keys
/// warn instead of failing so a newer server's overrides stay loadable.
pub(crate) fn apply_wave_rules_overrides(
    world: &DynamicWorld,
    admin: &crate::state::administration::Administration,
) {
    let overrides = admin.rules_overrides_snapshot();
    if overrides.is_empty() {
        return;
    }
    let mut rules = world.wave_rules.write();
    for (key, value) in overrides {
        let key = key.as_str();
        let flag = |key: &str, field: &mut bool| match value.as_bool() {
            Some(v) => {
                *field = v;
                true
            }
            None => {
                warn!("Rules override '{}' expects a boolean", key);
                false
            }
        };
        let mult = |key: &str, field: &mut f32| match value.as_f64() {
            Some(v) if v.is_finite() => {
                *field = v as f32;
                true
            }
            _ => {
                warn!("Rules override '{}' expects a finite number", key);
                false
            }
        };
        let applied = match key {
            "buildSpeedMultiplier" => mult(key, &mut rules.build_speed_multiplier),
            "unitMineSpeedMultiplier" => mult(key, &mut rules.unit_mine_speed_multiplier),
            "blockHealthMultiplier" => mult(key, &mut rules.block_health_multiplier),
            "blockDamageMultiplier" => mult(key, &mut rules.block_damage_multiplier),
            "unitDamageMultiplier" => mult(key, &mut rules.unit_damage_multiplier),
            "unitHealthMultiplier" => mult(key, &mut rules.unit_health_multiplier),
            "infiniteResources" => flag(key, &mut rules.infinite_resources),
            "reactorExplosions" => flag(key, &mut rules.reactor_explosions),
            "canGameOver" => flag(key, &mut rules.can_game_over),
            "instantBuild" => flag(key, &mut rules.instant_build),
            "waves" => flag(key, &mut rules.waves_enabled),
            "waveTimer" => flag(key, &mut rules.wave_timer),
            "waveSending" => flag(key, &mut rules.wave_sending),
            "waitEnemies" => flag(key, &mut rules.wait_enemies),
            "possessionAllowed" => flag(key, &mut rules.possession_allowed),
            "winWave" => match value.as_i64().and_then(|v| i32::try_from(v).ok()) {
                Some(v) => {
                    rules.win_wave = v;
                    true
                }
                None => {
                    warn!("Rules override 'winWave' expects an integer");
                    false
                }
            },
            "waveTeam" => match value.as_u64().and_then(|v| u8::try_from(v).ok()) {
                Some(v) => {
                    rules.wave_team = v;
                    true
                }
                None => {
                    warn!("Rules override 'waveTeam' expects a team id");
                    false
                }
            },
            "defaultTeam" => match value.as_u64().and_then(|v| u8::try_from(v).ok()) {
                Some(v) => {
                    rules.default_team = v;
                    true
                }
                None => {
                    warn!("Rules override 'defaultTeam' expects a team id");
                    false
                }
            },
            "fog" => flag(key, &mut rules.fog),
            "loadout" => match value.as_str() {
                Some(loadout) => {
                    rules.loadout = crate::network::units::parse_loadout(loadout);
                    true
                }
                None => {
                    warn!("Rules override 'loadout' expects a string like 'copper-20/lead-10'");
                    false
                }
            },
            _ => {
                warn!("Unknown rules override '{}' ignored", key);
                false
            }
        };
        if applied {
            debug!("Rules override applied: {} = {}", key, value);
        }
    }
}

/// P0-7: strict-mode gate for unsupported map spawn groups. In strict mode
/// any skipped group rejects the map with the full diagnostic list (so waves
/// never disappear silently); otherwise each group is logged as a warning.
pub(crate) fn enforce_strict_spawn_groups(
    map_name: &str,
    diagnostics: &[String],
    strict: bool,
) -> std::io::Result<()> {
    if strict && !diagnostics.is_empty() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "strict mode: map '{map_name}' has {} unsupported spawn group(s): {}",
                diagnostics.len(),
                diagnostics.join("; ")
            ),
        ));
    }
    for diagnostic in diagnostics {
        warn!("map '{}' spawn group skipped: {}", map_name, diagnostic);
    }
    Ok(())
}

/// P0-6: computes the authoritative `WaveRules` for a runtime mode switch.
/// The map's base rules are re-derived from the world template (so switching
/// back to survival restores the map's original `infiniteResources`, wave
/// timer and possession values instead of inheriting the sandbox preset),
/// then the `Gamemode` preset is applied on top. Returns an error when the
/// template cannot be inspected so the caller aborts BEFORE any mutation.
pub(crate) fn mode_transition_rules(
    world: &DynamicWorld,
    mode: GameMode,
) -> std::io::Result<WaveRules> {
    let metadata = crate::engine::world_stream::inspect_metadata(&world.network_template)?;
    let mut rules = crate::network::units::parse_wave_rules(&metadata.rules);
    apply_game_mode_to_wave_rules(&mut rules, mode);
    Ok(rules)
}

/// Applies the subset of the official `Gamemode` preset represented by
/// `WaveRules`. Mode presets are applied after map rules: a Survival map
/// hosted as Sandbox must not be allowed to turn infinite resources back off.
pub(crate) fn apply_game_mode_to_wave_rules(rules: &mut WaveRules, mode: GameMode) {
    match mode {
        GameMode::Sandbox => {
            // Gamemode.sandbox (v158.1): infiniteResources=true, waves=true and
            // waveTimer=false. allowEditRules is a client-facing Rules field and
            // is patched into the serialized world stream separately.
            rules.infinite_resources = true;
            rules.waves_enabled = true;
            rules.wave_timer = false;
        }
        GameMode::Survival => {
            // Gamemode.survival: waveTimer=true, waves=true. Players stay on
            // defaultTeam (sharded); waveTeam (crux) remains the enemy.
            rules.wave_timer = true;
            rules.waves_enabled = true;
        }
        GameMode::Pvp => {
            // Gamemode.pvp: pvp flag is GameMode::Pvp; attackMode is out of
            // scope for this task. Radius/build multipliers match 158.1.
            rules.enemy_core_build_radius = 600.0;
            rules.build_speed_multiplier = 1.0;
        }
        GameMode::Attack => {}
    }
}

pub(crate) fn fresh_world_from_template_for_mode(
    state: &GameState,
    network_template: Vec<u8>,
    map_name: String,
    save_path: PathBuf,
    mode: GameMode,
) -> std::io::Result<DynamicWorld> {
    let base_map = crate::engine::world_stream::inspect_map(&network_template)?;
    let metadata = crate::engine::world_stream::inspect_metadata(&network_template)?;
    let width = i32::from(base_map.width);
    let height = i32::from(base_map.height);
    info!(
        "Prepared fresh world for map '{}' ({}x{})",
        map_name, width, height
    );
    let sharded_unit_cap = sharded_unit_cap(&metadata.rules, &base_map.buildings);
    // Parse Rules before selecting fallback cores/spawns: defaultTeam and
    // waveTeam are map configuration, not fixed sharded/crux IDs.
    // P0-7: unsupported spawn groups warn in normal mode but reject the map
    // in strict mode with the full diagnostic list.
    let (mut wave_rules, spawn_diagnostics) =
        crate::network::units::parse_wave_rules_report(&metadata.rules);
    enforce_strict_spawn_groups(
        &map_name,
        &spawn_diagnostics,
        state.strict_mode.load(Ordering::Relaxed),
    )?;
    apply_game_mode_to_wave_rules(&mut wave_rules, mode);
    // Coreless custom maps are valid in the official server; retain the
    // legacy origin only as a spawn fallback rather than rejecting the map.
    let core_position = base_map
        .buildings
        .iter()
        .find(|building| {
            building.team == wave_rules.default_team && (339..=344).contains(&building.block)
        })
        .map(|building| building.position)
        .unwrap_or(0);
    let core_max_health = base_map
        .buildings
        .iter()
        .find(|building| building.position == core_position)
        .map(|building| building.health)
        .unwrap_or_else(initial_core_health);
    // Keep every core in registration order. `cores` is a compatibility map
    // containing the first core for each team; the list is authoritative.
    let cores = DashMap::new();
    let team_core_lists = DashMap::new();
    for building in &base_map.buildings {
        if (339..=344).contains(&building.block) {
            let core = TeamCore {
                position: building.position,
                block: building.block,
                health: building.health,
                max_health: building.health,
            };
            team_core_lists
                .entry(building.team)
                .or_insert_with(Vec::new)
                .push(core);
            cores.entry(building.team).or_insert(core);
        }
    }
    let core_summary: Vec<_> = team_core_lists
        .iter()
        .flat_map(|entry| {
            let team = *entry.key();
            entry
                .value()
                .iter()
                .map(move |core| {
                    format!(
                        "team {} at ({},{}) block {} hp {}",
                        team,
                        core.position >> 16,
                        core.position as i16,
                        core.block,
                        core.health
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();

    info!("Map '{}' team cores: {}", map_name, core_summary.join("; "));
    let mut base_building_templates = Vec::new();
    for building in &base_map.buildings {
        if (339..=344).contains(&building.block) {
            continue;
        }
        let occupied = block_footprint_in(width, height, building.position, building.block)
            .unwrap_or_else(|| vec![building.position]);
        let maximum = crate::game::content::block_health(building.block);
        base_building_templates.push(BaseBuildingState {
            position: building.position,
            block: building.block,
            team: building.team,
            health: building.health.min(maximum),
            occupied,
            inventory: building.inventory.clone(),
        });
    }
    // Map entities are live buildings too. Seed both compatibility base_buildings
    // and the authoritative tiles registry from the same decoded NetworkBuilding;
    // this is also what makes hot-host worlds retain inventories/modules.
    let initial_tiles = DashMap::new();
    for building in &base_map.buildings {
        let occupied = block_footprint_in(width, height, building.position, building.block)
            .unwrap_or_else(|| vec![building.position]);
        initial_tiles.insert(building.position, network_building_tile(building, occupied));
    }
    let team_build_plans = crate::engine::world_stream::inspect_team_plans(&network_template)?;
    // Official WaveSpawner: in Attack mode the enemy (waveTeam) cores are
    // additional ground spawn points — the crux base in a map like frontier
    // emits waves from its own core instead of the map's spawn overlays.
    // Combine overlay spawns with enemy-core positions when the map has them.
    let mut enemy_spawns = base_map.enemy_spawns();
    if mode == GameMode::Attack {
        extend_attack_spawns_for_team(&mut enemy_spawns, &base_map.buildings, wave_rules.wave_team);
    }
    // Wave generation comes from the loaded map's Rules, with the wave/team
    // contract retained above for every authoritative runtime consumer.
    info!(
        "Map '{}' wave rules: {} spawn groups, waveSpacing {} ticks, initialWaveSpacing {} ticks",
        map_name,
        wave_rules.spawn_groups.len(),
        wave_rules.wave_spacing,
        wave_rules.initial_wave_spacing
    );
    // First wave uses the map's initial spacing (official Logic.play()).
    *state.wave_time.write() = wave_rules.initial_wave_spacing;
    state
        .infinite_resources
        .store(wave_rules.infinite_resources, Ordering::Relaxed);
    Ok(DynamicWorld {
        game_state: state.clone(),
        width,
        height,
        sharded_unit_cap,
        core_position,
        core_max_health,
        cores,
        team_core_lists,
        base_blocks: base_map.blocks,
        base_centers: base_map.block_centers,
        tile_data: base_map.tile_data,
        base_building_templates: base_building_templates.clone(),
        base_buildings: {
            let map = DashMap::new();
            for template in &base_building_templates {
                map.insert(template.position, template.clone());
            }
            map
        },
        floors: base_map.floors.clone(),
        overlays: base_map.overlays.clone(),
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_000),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
        projectiles: DashMap::new(),
        next_projectile_id: AtomicI32::new(4_000_000),
        overdrive_boosts: DashMap::new(),
        heal_suppression: DashMap::new(),
        force_fields: DashMap::new(),
        tiles: initial_tiles,
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::from(build_mineable_ore_index(
            width,
            height,
            &base_map.floors,
            &base_map.overlays,
        )),
        mono_mining_targets: DashMap::new(),
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path,
        logic_flags: DashMap::new(),
        logic_executors: DashMap::new(),
        logic_display_commands: DashMap::new(),
        base_drill_progress: DashMap::new(),
        base_factory_progress: DashMap::new(),
        base_turret_progress: DashMap::new(),
        base_mender_progress: DashMap::new(),
        team_build_plans: parking_lot::RwLock::new(team_build_plans),
        network_template: Arc::new(network_template),
        persistence_dirty: AtomicBool::new(false),
        persistence_lock: parking_lot::Mutex::new(()),
        wave_rules: parking_lot::RwLock::new(wave_rules),
        votekick_target: parking_lot::RwLock::new(None),
        votekick_votes: AtomicI32::new(0),
        votekick_voters: DashMap::new(),
        votekick_cooldowns: DashMap::new(),
        puddles: crate::network::buildings::puddles::PuddleSystem::new(),
    })
}

/// Resolved source for the `host <map>` console command.
#[derive(Debug, Clone)]
pub enum HostMapSource {
    /// The embedded 300x300 maze template (no external MSAV needed).
    EmbeddedTemplate,
    /// An official/custom MSAV file read from disk.
    Msav(Vec<u8>),
}

/// Resolves the `host <map>` argument to map bytes and its display name.
/// The argument may be a path to a `.msav` file or a bare map name resolved
/// against the default maps directory (`../core/assets/maps/default` relative
/// to the server working directory, matching the bundled repo layout).
pub fn resolve_host_map(map: &str) -> std::io::Result<(HostMapSource, String)> {
    // The bundled 300x300 template is the canonical maze; the `maze.msav`
    // shipped in this repo is an unsupported pre-v5 save, so the template
    // always wins for this name.
    if map == "maze" {
        return Ok((HostMapSource::EmbeddedTemplate, "maze".to_owned()));
    }
    let candidates = [
        map.to_string(),
        format!("{map}.msav"),
        format!("maps/{map}.msav"),
        format!("third_party/mindustry-maps/{map}.msav"),
        format!("../core/assets/maps/default/{map}.msav"),
    ];
    for candidate in &candidates {
        match std::fs::read(candidate) {
            Ok(bytes) => {
                // Skip files whose save version the MSAV codec cannot read
                // (the default maps directory contains v1-3 legacy editor maps).
                if crate::engine::save_io::SaveIO::read_meta(bytes.as_slice()).is_err() {
                    continue;
                }
                let name = Path::new(candidate)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or(map)
                    .to_owned();
                return Ok((HostMapSource::Msav(bytes), name));
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Err(Error::new(
        ErrorKind::NotFound,
        format!(
            "no readable map named '{map}' found (tried {})",
            candidates.join(", ")
        ),
    ))
}

/// Outcome of a successful `host` hot-swap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostMapResult {
    pub map_name: String,
    /// Connected players that received WorldDataBegin + the new world stream.
    pub restreamed: usize,
    /// Connections that were still loading the previous world and were kicked.
    pub kicked: usize,
}

/// Hot map swap used by `host <map> [mode]`: builds a fresh world from the
/// resolved MSAV, resets wave/game state, atomically replaces the shared
/// world, persists the fresh state to the active save (updating `map_name`)
/// and re-streams every connected player with `WorldDataBegin` followed by
/// their personalized new world stream. Players keep their identity (UUID,
/// name, color) and respawn at the new map's core after the client finishes
/// loading; connections still receiving the old world are kicked so they
/// reconnect to the new map.
/// Round 74d develop: world re-host counter for the runtime diagnostics
/// dump — any repeated host_map (map rotation loops, repeated re-streams)
/// shows up here immediately.
pub static HOST_MAP_EVENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn host_map(
    store: &WorldStore,
    connections: &DashMap<i32, PendingConnection>,
    map: &str,
    mode: &str,
    admin: Option<&crate::state::administration::Administration>,
) -> std::io::Result<HostMapResult> {
    let event = HOST_MAP_EVENTS.fetch_add(1, Ordering::Relaxed) + 1;
    info!(
        target: "develop",
        "host_map event #{}: map='{}' mode='{}' connections={}",
        event,
        map,
        mode,
        connections.len()
    );
    let (source, map_name) = resolve_host_map(map)?;
    // P1: legacy MSAV v1-v3 are declared metadata-only in this port: the
    // network-map extractor cannot reconstruct their entity/footprint
    // regions (verified: replace_map_from_msav fails on v1-v3 entity
    // chunks), so hosting is rejected with a clear diagnostic instead of a
    // parse error or silently simulated 1x1 ghosts.
    if let HostMapSource::Msav(msav) = &source {
        // Round-73 M2 (documented extension): v12/v13 are accepted by the
        // hosting extractor as a port extension (the user's fixtures are
        // v13; the region layout comes from the newer source tree, not the
        // 158.1 baseline). `read_meta` rejects >11, but hosting does not
        // claim JAR interop for those versions, so the error is tolerated
        // here on purpose and only the metadata-only v1-v3 are rejected.
        if let Ok(meta) = crate::engine::save_io::SaveIO::read_meta(msav.as_slice()) {
            if meta.version <= 3 {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "map '{map_name}' is a legacy MSAV (SaveVersion {}): v1-v3 are metadata-only in this port (footprints, building modules and world entities are not reconstructed); convert the map with the official editor or use a v4+ save",
                        meta.version
                    ),
                ));
            }
        }
    }
    let template = match &source {
        HostMapSource::Msav(msav) => crate::engine::world_stream::replace_map_from_msav(
            crate::engine::world_stream::embedded_template(),
            msav,
        )?,
        HostMapSource::EmbeddedTemplate => {
            crate::engine::world_stream::embedded_template().to_vec()
        }
    };
    let game_mode = match mode {
        "pvp" => GameMode::Pvp,
        "attack" => GameMode::Attack,
        "sandbox" => GameMode::Sandbox,
        _ => GameMode::Survival,
    };

    let old_world = store.load();
    // Snapshot player state before the swap; DashMap entries are cloned so no
    // guard is held while the new world is built.
    let sessions: Vec<SessionPlayer> = old_world
        .player_sessions
        .iter()
        .map(|entry| entry.value().clone())
        .collect();
    let profiles: Vec<PlayerCombatState> = old_world
        .player_profiles
        .iter()
        .map(|entry| entry.value().clone())
        .collect();

    let world = fresh_world_from_template_for_mode(
        &old_world.game_state,
        template,
        map_name.clone(),
        old_world.save_path.clone(),
        game_mode,
    )?;
    if let HostMapSource::Msav(msav) = &source {
        crate::engine::msav_roundtrip::apply_msav_entities(&world, msav)?;
        crate::network::buildings::power::normalize_power_links(&world);
    }

    // Reset the shared game state: new map identity, mode, waves and loadout.
    let state = &world.game_state;
    *state.map_name.write() = map_name.clone();
    *state.mode.write() = game_mode;
    state.wave.store(1, Ordering::Relaxed);
    // First wave uses the new map's initial spacing (official Logic.play()).
    *state.wave_time.write() = world.wave_rules.read().initial_wave_spacing;
    state.infinite_resources.store(
        world.wave_rules.read().infinite_resources,
        Ordering::Relaxed,
    );
    *state.core_items.write() = GameState::initial_core_items();
    *state.core_health.write() = world.core_max_health;
    state.game_over.store(false, Ordering::Relaxed);
    *state.simulation_time.write() = 0.0;
    state.enemies_count.store(0, Ordering::Relaxed);
    state.is_hosting.store(true, Ordering::SeqCst);

    // Players keep their identity; their combat state respawns at the new
    // map's core of their own team (PvP maps carry one core per team).
    let (spawn_x, spawn_y) = core_world_for_team(&world, 1);
    let mut restreamed = 0usize;
    let mut kicked = 0usize;
    let mut next_player_unit_id = world.next_player_unit_id.load(Ordering::Relaxed);
    for connection in connections.iter() {
        let player_id = 1_000_000 + *connection.key();
        let Some(session) = sessions
            .iter()
            .find(|session| session.id == player_id)
            .cloned()
        else {
            if connection.player_name.read().is_some() {
                let payload = encode_typeio_string(
                    "[scarlet]Server switched maps; reconnect to load the new world.",
                )?;
                let frame = frame_generated_packet(KICK_PACKET_ID, &payload, false)?;
                enqueue_outbound(&connection, frame, true);
                kicked += 1;
            }
            continue;
        };
        restreamed += 1;
        next_player_unit_id = next_player_unit_id.max(session.unit_id.saturating_add(1));
        // Preserve the player's assigned team (PvP) across the map swap.
        let kept_team = profiles
            .iter()
            .find(|profile| profile.uuid == session.uuid)
            .map(|profile| profile.team)
            .unwrap_or(1);
        let (team_spawn_x, team_spawn_y) = core_world_for_team(&world, kept_team);
        let combat = PlayerCombatState {
            uuid: session.uuid.clone(),
            player_id,
            unit_id: session.unit_id,
            x: team_spawn_x,
            y: team_spawn_y,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: kept_team,
        };
        world.players.insert(session.unit_id, combat.clone());
        world
            .player_profiles
            .insert(session.uuid.clone(), combat.clone());
        let mut fresh = session.clone();
        fresh.x = team_spawn_x;
        fresh.y = team_spawn_y;
        fresh.mouse_x = team_spawn_x;
        fresh.mouse_y = team_spawn_y;
        fresh.rotation = 90.0;
        fresh.active_plans.clear();
        fresh.mining_position = None;
        fresh.mining_progress = 0.0;
        fresh.carried_item = -1;
        fresh.carried_amount = 0;
        world.player_sessions.insert(session.unit_id, fresh);
    }
    world
        .next_player_unit_id
        .store(next_player_unit_id, Ordering::Relaxed);
    for mut profile in profiles {
        let (profile_x, profile_y) = core_world_for_team(&world, profile.team);
        profile.x = profile_x;
        profile.y = profile_y;
        profile.health = 150.0;
        profile.shield = 0.0;
        profile.status_effect = -1;
        profile.statuses.clear();
        crate::network::units::StatusContainer::clear_statuses(&mut profile);
        profile.status_duration = 0.0;
        profile.dead = false;
        profile.respawn_timer = 0.0;
        world.player_profiles.insert(profile.uuid.clone(), profile);
    }

    // P2: global rules overrides (official `rules` command) apply after the
    // map rules + mode preset, before the world is published.
    if let Some(admin) = admin.as_ref() {
        apply_wave_rules_overrides(&world, admin);
    }

    // Atomic swap: the simulation and every connection pick up the new world
    // on their next loop iteration.
    store.swap(world);
    let world = store.load();

    // Persist the fresh state to the active save; this also records the new
    // map identity so later `load`/restart validations accept the file.
    let _guard = world.persistence_lock.lock();
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

    // Re-stream: WorldDataBegin resets the client world, then the new stream
    // makes it reload and re-send ConnectConfirm (official /sync flow,
    // WorldReloader + NetClient.worldDataBegin / finishConnecting).
    let wave = world.game_state.wave.load(Ordering::Relaxed);
    let wave_time = *world.game_state.wave_time.read();
    let tick = f64::from(*world.game_state.simulation_time.read());
    let mode = *world.game_state.mode.read();
    let begin_frame = frame_generated_packet(WORLD_DATA_BEGIN_PACKET_ID, &[], false)?;
    for connection in connections.iter() {
        let player_id = 1_000_000 + *connection.key();
        let Some(session) = sessions.iter().find(|session| session.id == player_id) else {
            continue;
        };
        let stream = crate::engine::world_stream::personalize_current_with_state_mode(
            &network_template_with_plans(&world)?,
            player_id,
            &session.name,
            session.color,
            (spawn_x, spawn_y),
            wave,
            wave_time,
            tick,
            mode == GameMode::Pvp,
            mode == GameMode::Sandbox,
        )?;
        enqueue_outbound(&connection, begin_frame.clone(), true);
        for frame in world_stream_frames(&stream)? {
            enqueue_outbound(&connection, frame, true);
        }
    }

    info!(
        "Hot map swap complete: '{}' [{:?}], {} player(s) re-streamed, {} connection(s) kicked",
        map_name, game_mode, restreamed, kicked
    );
    Ok(HostMapResult {
        map_name,
        restreamed,
        kicked,
    })
}

pub(crate) fn emit_game_over_packet_with_winner(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    winner: u8,
) {
    world.persistence_dirty.store(true, Ordering::Relaxed);
    if let Ok(frame) = frame_generated_packet(GAME_OVER_PACKET_ID, &[winner], false) {
        out.broadcast(frame);
    }
}

/// Survival/attack game over: the sharded core (team 1) was destroyed, so the
/// wave team (crux, id 2) wins.
pub(crate) fn emit_game_over_packet(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
) {
    emit_game_over_packet_with_winner(world, out, world.wave_rules.read().wave_team);
}

/// Official `NetServer.assignTeam` / default `TeamAssigner` (158.1): in PvP
/// the joiner lands on the active team with the fewest connected players.
/// Java skips waveTeam when `rules.waves` is set, derelict, teams without a
/// living core, and teams with `protectCores == false`. Tie-break is the
/// lowest team id (Java adds a tiny random jitter). Other modes keep
/// `rules.defaultTeam` (sharded = 1).
pub(crate) fn assign_team_for_join(world: &DynamicWorld, _uuid: &str, current_team: u8) -> u8 {
    let rules = world.wave_rules.read();
    if *world.game_state.mode.read() != GameMode::Pvp {
        return rules.default_team;
    }
    let wave_team = rules.wave_team;
    let waves = rules.waves_enabled;
    let mut counts: HashMap<u8, usize> = HashMap::new();
    for entry in world.players.iter() {
        let team = entry.value().team;
        if team != 0 {
            *counts.entry(team).or_insert(0) += 1;
        }
    }
    let mut pool: Vec<u8> = crate::network::world::registered_core_teams(world)
        .into_iter()
        .filter(|team| {
            if *team == 0 {
                return false;
            }
            if waves && *team == wave_team {
                return false;
            }
            if !rules.team_rule(*team).protect_cores {
                return false;
            }
            crate::network::world::team_core_snapshot(world, *team)
                .iter()
                .any(|core| core.health > 0.0)
        })
        .collect();
    drop(rules);
    pool.sort_unstable();
    pool.into_iter()
        .min_by_key(|team| (counts.get(team).copied().unwrap_or(0), *team))
        .unwrap_or(current_team)
}

/// Maps the official team names (Team.java) and numeric ids to a team id.
/// Returns None for unknown names; `0..=255` numeric ids are accepted like
/// `Team.get(id)` in the official server.
pub(crate) fn parse_team_id(team: &str) -> Option<u8> {
    let normalized = team.trim().to_lowercase();
    match normalized.as_str() {
        "derelict" => Some(0),
        "sharded" => Some(1),
        "crux" => Some(2),
        "malis" => Some(3),
        "green" => Some(4),
        "blue" => Some(5),
        "neoplastic" => Some(6),
        _ => normalized.parse::<u8>().ok(),
    }
}
