//! Integration tests for the hot map swap (`host <map> [mode]`):
//! atomic world replacement, game-state reset, save persistence with the new
//! map identity, per-team build plans and client re-streaming through
//! WorldDataBegin + the new personalized world stream.

use dashmap::DashMap;
use oxide::console::command::ServerCommand;
use oxide::engine::typeio::{TeamBlockPlan, TeamBlocks, TeamPlans};
use oxide::engine::world_stream::{
    embedded_template, inspect_map, personalize_current_with_state_mode, replace_map_from_msav,
};
use oxide::network::codec::read_packet;
use oxide::network::listener::{
    fresh_world_from_template, host_map, load_tiles, persist_tiles, resolve_host_map,
    world_stream_frames, HostMapResult, HostMapSource, OUTBOUND_QUEUE_CAPACITY,
};
use oxide::network::world::{PendingConnection, SessionPlayer, WorldStore};
use oxide::state::game_state::{GameMode, GameState};
use std::io::Cursor;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Official default maps live next to this repo in a local Mindustry checkout.
/// GitHub CI does not have that tree; skip those cases instead of failing.
fn java_default_map(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(format!("../core/assets/maps/default/{name}.msav"));
    path.is_file().then_some(path)
}

/// Connection id whose player id is `1_000_000 + CONNECTION_ID`.
const CONNECTION_ID: i32 = 7;
const PLAYER_ID: i32 = 1_000_000 + CONNECTION_ID;
const UNIT_ID: i32 = 2_000_000 + CONNECTION_ID;

fn test_connection(
    name: Option<&str>,
) -> (PendingConnection, tokio::sync::mpsc::Receiver<Vec<u8>>) {
    let (outbound, outbound_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let (udp_inbound, _) = tokio::sync::mpsc::unbounded_channel();
    let connection = PendingConnection {
        ip: IpAddr::from([127, 0, 0, 1]),
        outbound,
        udp_inbound,
        udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
        udp_socket: None,
        player_name: Arc::new(parking_lot::RwLock::new(name.map(str::to_owned))),
        outbound_drops: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        critical_drops: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        last_keepalive_rtt_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        last_packet_epoch_ms: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        outbound_queued: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    };
    (connection, outbound_rx)
}

fn joined_session() -> SessionPlayer {
    SessionPlayer {
        id: PLAYER_ID,
        controlled_unit: oxide::network::world::ControlledUnit::Core,
        unit_id: UNIT_ID,
        uuid: "uuid-hot-host".to_string(),
        name: "Tester".to_string(),
        color: 0x11223344,
        last_snapshot: 3,
        x: 320.0,
        y: 800.0,
        mouse_x: 320.0,
        mouse_y: 800.0,
        rotation: 90.0,
        boosting: false,
        shooting: false,
        last_command: None,
        active_plans: Default::default(),
        mining_position: None,
        mining_progress: 0.0,
        mining_updated: std::time::Instant::now(),
        carried_item: -1,
        carried_amount: 0,
        preview_plan_group: -1,
        preview_plans: Vec::new(),
        last_shot: std::time::Instant::now(),
        admin: false,
        chat_rate: oxide::network::listener::ChatRateLimiter::new(),
    }
}

fn temp_save_path(label: &str) -> std::path::PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "mindustry-hot-host-{}-{}.json",
        label,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn host_command_parses_map_and_mode() {
    assert_eq!(
        ServerCommand::parse("host archipelago attack"),
        Some(ServerCommand::Host {
            map: "archipelago".to_string(),
            mode: "attack".to_string(),
        })
    );
    assert_eq!(
        ServerCommand::parse("host maze"),
        Some(ServerCommand::Host {
            map: "maze".to_string(),
            mode: "survival".to_string(),
        })
    );
    assert_eq!(
        ServerCommand::parse("host  ../core/assets/maps/default/fortress.msav  pvp"),
        Some(ServerCommand::Host {
            map: "../core/assets/maps/default/fortress.msav".to_string(),
            mode: "pvp".to_string(),
        })
    );
}

#[test]
fn resolve_host_map_finds_paths_names_and_maze_template() {
    let (source, name) = resolve_host_map("maze").unwrap();
    assert!(matches!(source, HostMapSource::EmbeddedTemplate));
    assert_eq!(name, "maze");

    let (source, name) = resolve_host_map("groundZero").unwrap();
    assert!(matches!(source, HostMapSource::Msav(_)));
    assert_eq!(name, "groundZero");

    if java_default_map("archipelago").is_some() {
        // Bare official map name resolved against the default maps directory.
        let (source, name) = resolve_host_map("archipelago").unwrap();
        assert!(matches!(source, HostMapSource::Msav(_)));
        assert_eq!(name, "archipelago");

        // Explicit relative path (with extension).
        let (source, name) =
            resolve_host_map("../core/assets/maps/default/archipelago.msav").unwrap();
        assert!(matches!(source, HostMapSource::Msav(_)));
        assert_eq!(name, "archipelago");
    }

    if java_default_map("fortress").is_some() {
        // v4 editor saves (fortress, shattered) load through the short-chunk
        // reader (Save4 layout: no markers/custom regions, no entity mapping).
        let (source, name) = resolve_host_map("../core/assets/maps/default/fortress.msav").unwrap();
        assert!(matches!(source, HostMapSource::Msav(_)));
        assert_eq!(name, "fortress");
    }
    if java_default_map("shattered").is_some() {
        let (source, name) = resolve_host_map("shattered").unwrap();
        assert!(matches!(source, HostMapSource::Msav(_)));
        assert_eq!(name, "shattered");
    }

    assert!(resolve_host_map("no-such-map-xyz").is_err());
}

#[test]
fn fresh_world_from_msav_uses_map_layout_and_core() {
    let Some(path) = java_default_map("archipelago") else {
        return;
    };
    let state = GameState::new();
    let msav = std::fs::read(path).unwrap();
    let template = replace_map_from_msav(embedded_template(), &msav).unwrap();
    let save_path = temp_save_path("fresh");
    let world = fresh_world_from_template(
        &state,
        template.clone(),
        "archipelago".to_string(),
        save_path,
    )
    .unwrap();

    let map = inspect_map(&template).unwrap();
    assert_eq!(world.width(), i32::from(map.width));
    assert_eq!(world.height(), i32::from(map.height));
    assert_eq!(world.width(), 500);
    assert_eq!(world.height(), 500);
    let core = map
        .buildings
        .iter()
        .find(|building| building.team == 1 && (339..=344).contains(&building.block))
        .unwrap();
    assert_eq!(world.core_position(), core.position);
    assert_eq!(world.core_max_health(), core.health);
    // A fresh world has no dynamic state but keeps the map's team plans:
    // archipelago.msav carries one sharded team entry with zero plans.
    assert!(world.player_sessions().is_empty());
    assert!(world.player_profiles().is_empty());
    assert_eq!(world.team_build_plans().teams.len(), 1);
    assert_eq!(world.team_build_plans().teams[0].team, 1);
    assert!(world.team_build_plans().teams[0].plans.is_empty());
    assert!(world
        .save_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("mindustry-hot-host-fresh"));
    // The network template is still a valid 158.1 world stream.
    assert_eq!(inspect_map(world.network_template()).unwrap().width, 500);
    assert_eq!(world.game_state().map_name.read().as_str(), "maze");
}

#[test]
fn world_store_swaps_worlds_atomically() {
    let Some(path) = java_default_map("archipelago") else {
        return;
    };
    let state = GameState::new();
    let maze = fresh_world_from_template(
        &state,
        embedded_template().to_vec(),
        "maze".to_string(),
        temp_save_path("swap-a"),
    )
    .unwrap();
    let store = WorldStore::new(maze);
    assert_eq!(store.load().width(), 300);

    let msav = std::fs::read(path).unwrap();
    let template = replace_map_from_msav(embedded_template(), &msav).unwrap();
    let archipelago = fresh_world_from_template(
        &state,
        template,
        "archipelago".to_string(),
        temp_save_path("swap-b"),
    )
    .unwrap();
    let previous = store.swap(archipelago);
    assert_eq!(previous.width(), 300);
    let current = store.load();
    assert_eq!(current.width(), 500);
    assert_eq!(current.core_max_health(), 6000.0);
}

#[test]
fn host_map_swaps_resets_persists_and_restreams_players() {
    if java_default_map("archipelago").is_none() {
        return;
    }
    let save_path = temp_save_path("host");
    let state = GameState::new();
    state.start_hosting("maze".to_string(), GameMode::Survival);
    state.wave.store(37, Ordering::Relaxed);
    *state.wave_time.write() = 12.0;
    state.game_over.store(true, Ordering::Relaxed);

    let world = fresh_world_from_template(
        &state,
        embedded_template().to_vec(),
        "maze".to_string(),
        save_path.clone(),
    )
    .unwrap();
    let store = WorldStore::new(world);
    // A player that already finished loading the previous world.
    store
        .load()
        .player_sessions()
        .insert(UNIT_ID, joined_session());

    let connections = DashMap::new();
    let (connection, mut outbound) = test_connection(Some("Tester"));
    connections.insert(CONNECTION_ID, connection);

    let result = host_map(&store, &connections, "archipelago", "attack", None).unwrap();
    assert_eq!(
        result,
        HostMapResult {
            map_name: "archipelago".to_string(),
            restreamed: 1,
            kicked: 0,
        }
    );

    // Game state reset: new identity, mode, waves, loadout and core.
    assert_eq!(state.map_name.read().as_str(), "archipelago");
    assert_eq!(*state.mode.read(), GameMode::Attack);
    assert_eq!(state.wave.load(Ordering::Relaxed), 1);
    // First wave uses the new map's initial wave spacing (official
    // Logic.play()); when initialWaveSpacing is absent, Java uses
    // waveSpacing * 2.  Archipelago sets waveSpacing=8400.
    assert_eq!(*state.wave_time.read(), 16800.0);
    assert!(!state.game_over.load(Ordering::Relaxed));
    let items = state.core_items.read();
    assert_eq!(items[0], 100);
    assert!(items[1..].iter().all(|amount| *amount == 0));

    // The shared world is the new map; the player respawns at its core.
    let world = store.load();
    assert_eq!(world.width(), 500);
    let session = world
        .player_sessions()
        .get(&UNIT_ID)
        .expect("player session re-inserted into the new world");
    assert_eq!(session.id, PLAYER_ID);
    assert_eq!(session.name, "Tester");
    let (core_x, core_y) = (
        (world.core_position() >> 16) as f32 * 8.0,
        (world.core_position() as i16 as f32) * 8.0,
    );
    assert_eq!(session.x, core_x);
    assert_eq!(session.y, core_y);
    assert!(world.player_profiles().contains_key("uuid-hot-host"));

    // Client frames: WorldDataBegin first, then the new personalized stream.
    let frame = outbound.try_recv().expect("WorldDataBegin frame");
    let decoded = read_packet(Cursor::new(&frame[2..])).unwrap();
    assert_eq!(decoded[0], 164, "WorldDataBeginCallPacket id");
    assert_eq!(decoded.len(), 1, "WorldDataBegin carries no payload");
    let mut stream = Vec::new();
    let mut chunks = 0;
    // First stream frame: StreamBegin (packet id 0) with the total length.
    let begin = outbound.try_recv().expect("StreamBegin frame");
    let decoded = read_packet(Cursor::new(&begin[2..])).unwrap();
    assert_eq!(decoded[0], 0, "StreamBegin packet id");
    assert_eq!(
        i32::from_be_bytes(decoded[1..5].try_into().unwrap()),
        1,
        "stream id"
    );
    let total = i32::from_be_bytes(decoded[5..9].try_into().unwrap()) as usize;
    assert_eq!(decoded[9], 2, "Net.packetIdWorldStream");
    // Remaining frames: StreamChunk (packet id 1), 1024-byte slices.
    while let Ok(frame) = outbound.try_recv() {
        let decoded = read_packet(Cursor::new(&frame[2..])).unwrap();
        assert_eq!(decoded[0], 1, "StreamChunk packet id");
        assert_eq!(
            i32::from_be_bytes(decoded[1..5].try_into().unwrap()),
            1,
            "stream id"
        );
        let length = i16::from_be_bytes(decoded[5..7].try_into().unwrap()) as usize;
        stream.extend_from_slice(&decoded[7..7 + length]);
        chunks += 1;
    }
    assert!(chunks > 0, "world stream must be chunked");
    assert_eq!(stream.len(), total, "reassembled stream length");
    // The stream carries the new map's first-wave delay: archipelago sets
    // waveSpacing=8400, so Java's Logic.play() uses waveSpacing * 2 = 16800.
    let expected = personalize_current_with_state_mode(
        world.network_template(),
        PLAYER_ID,
        "Tester",
        0x11223344,
        (core_x, core_y),
        1,
        16800.0,
        0.0,
        false,
        false,
    )
    .unwrap();
    assert_eq!(stream, expected);
    // The 159.7 wire format keeps the Save13 data-patch header (8 bytes)
    // before the rules string — personalize_current retains the template's
    // [0,0,0,2,0,0,0,0] prefix, unlike the legacy 158.1 helper which stripped it.
    let mut decoder = flate2::read::ZlibDecoder::new(stream.as_slice());
    let mut decoded = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
    assert_eq!(&decoded[..8], &[0, 0, 0, 2, 0, 0, 0, 0]);
    let rules_length = u16::from_be_bytes(decoded[8..10].try_into().unwrap()) as usize;
    assert!(rules_length > 100, "rules JSON must be embedded");
    assert_eq!(decoded[10], b'{');

    // The active save now belongs to the new map, revision 9, and loads back.
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&save_path).unwrap()).unwrap();
    assert_eq!(json["version"], 14);
    assert_eq!(json["map_name"], "archipelago");
    assert_eq!(json["wave"], 1);
    // Game-over is ephemeral runtime state and is NOT persisted (official
    // server behavior): a restart always boots a live game. Old saves that
    // still carry `game_over` are accepted and the flag is ignored.
    assert!(
        json.get("game_over").is_none(),
        "game_over must not be persisted"
    );
    let loaded = load_tiles(&save_path, Some((500, 500))).unwrap();
    assert_eq!(loaded.map_name.as_deref(), Some("archipelago"));
    // Map buildings are live authoritative tiles (SOL-001): the fresh
    // archipelago world persists its prebuilt buildings, including the
    // team-1 core-foundation (block 341), through the hot swap.
    assert!(!loaded.tiles.is_empty());
    assert!(loaded
        .tiles
        .iter()
        .any(|tile| tile.block == 341 && tile.team == 1));
    assert_eq!(loaded.wave, Some(1));
    // The map's team plans survive the hot swap and the save round trip.
    assert_eq!(loaded.team_build_plans.teams.len(), 1);
    assert_eq!(loaded.team_build_plans.teams[0].team, 1);
    assert!(loaded.team_build_plans.teams[0].plans.is_empty());
}

#[test]
fn host_map_kicks_connections_still_loading_the_previous_world() {
    if java_default_map("archipelago").is_none() {
        return;
    }
    let save_path = temp_save_path("kick");
    let state = GameState::new();
    state.start_hosting("maze".to_string(), GameMode::Survival);
    let world = fresh_world_from_template(
        &state,
        embedded_template().to_vec(),
        "maze".to_string(),
        save_path,
    )
    .unwrap();
    let store = WorldStore::new(world);

    let connections = DashMap::new();
    // 7: joined (gets re-streamed); 8: still loading (gets kicked);
    // 9: connected but pre-ConnectPacket (untouched).
    store
        .load()
        .player_sessions()
        .insert(UNIT_ID, joined_session());
    let (connection, _rx7) = test_connection(Some("Tester"));
    connections.insert(CONNECTION_ID, connection);
    let (loading, mut rx8) = test_connection(Some("Loading"));
    connections.insert(8, loading);
    let (silent, mut rx9) = test_connection(None);
    connections.insert(9, silent);

    let result = host_map(&store, &connections, "archipelago", "survival", None).unwrap();
    assert_eq!(
        result,
        HostMapResult {
            map_name: "archipelago".to_string(),
            restreamed: 1,
            kicked: 1,
        }
    );
    let frame = rx8.try_recv().expect("kicked connection receives a frame");
    let decoded = read_packet(Cursor::new(&frame[2..])).unwrap();
    assert_eq!(decoded[0], 60, "KickCallPacket id");
    assert!(rx8.try_recv().is_err(), "no world stream after the kick");
    assert!(
        rx9.try_recv().is_err(),
        "pre-connect connection is untouched"
    );
}

#[test]
fn team_build_plans_survive_persist_and_load() {
    let save_path = temp_save_path("plans");
    let state = GameState::new();
    state.start_hosting("maze".to_string(), GameMode::Survival);
    let world = fresh_world_from_template(
        &state,
        embedded_template().to_vec(),
        "maze".to_string(),
        save_path.clone(),
    )
    .unwrap();
    let plans = TeamBlocks {
        teams: vec![
            TeamPlans {
                team: 1,
                plans: vec![TeamBlockPlan {
                    x: 10,
                    y: 20,
                    rotation: 1,
                    block: 257,
                    config: vec![1, 0, 0, 0, 42],
                }],
            },
            TeamPlans {
                team: 2,
                plans: vec![TeamBlockPlan {
                    x: 3,
                    y: 4,
                    rotation: 0,
                    block: 98,
                    config: vec![0],
                }],
            },
        ],
    };
    world.set_team_build_plans(plans.clone());
    persist_tiles(
        &save_path,
        world.tiles(),
        world.game_state(),
        world.enemies(),
        world.base_buildings(),
        world.player_profiles(),
        world.building_commands(),
        world.unit_orders(),
        &world.team_build_plans(),
        &world.cores,
        &world.logic_flags,
        &world.puddles,
    )
    .unwrap();

    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&save_path).unwrap()).unwrap();
    assert_eq!(json["version"], 14);
    assert_eq!(json["team_build_plans"]["teams"][0]["team"], 1);
    assert_eq!(
        json["team_build_plans"]["teams"][0]["plans"][0]["block"],
        257
    );

    let loaded = load_tiles(&save_path, Some((300, 300))).unwrap();
    assert_eq!(loaded.team_build_plans, plans);
    assert_eq!(loaded.map_name.as_deref(), Some("maze"));
}

#[test]
fn world_stream_frames_match_handshake_layout() {
    let stream: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let frames = world_stream_frames(&stream).unwrap();
    assert!(frames.len() > 1, "stream must be split into chunks");

    let begin = read_packet(Cursor::new(&frames[0][2..])).unwrap();
    assert_eq!(begin[0], 0, "StreamBegin packet id");
    assert_eq!(
        i32::from_be_bytes(begin[1..5].try_into().unwrap()),
        1,
        "stream id"
    );
    assert_eq!(
        i32::from_be_bytes(begin[5..9].try_into().unwrap()),
        stream.len() as i32,
        "total stream length"
    );
    assert_eq!(begin[9], 2, "Net.packetIdWorldStream");

    let mut restored = Vec::new();
    for frame in frames.iter().skip(1) {
        let chunk = read_packet(Cursor::new(&frame[2..])).unwrap();
        assert_eq!(chunk[0], 1, "StreamChunk packet id");
        assert_eq!(
            i32::from_be_bytes(chunk[1..5].try_into().unwrap()),
            1,
            "stream id"
        );
        let length = i16::from_be_bytes(chunk[5..7].try_into().unwrap()) as usize;
        assert!(length <= 1024);
        restored.extend_from_slice(&chunk[7..7 + length]);
    }
    assert_eq!(restored, stream);
}

#[test]
fn host_map_on_maze_restarts_without_external_file() {
    let save_path = temp_save_path("maze");
    let state = GameState::new();
    state.start_hosting("maze".to_string(), GameMode::Survival);
    state.wave.store(99, Ordering::Relaxed);
    let world = fresh_world_from_template(
        &state,
        embedded_template().to_vec(),
        "maze".to_string(),
        save_path.clone(),
    )
    .unwrap();
    let store = WorldStore::new(world);
    store
        .load()
        .player_sessions()
        .insert(UNIT_ID, joined_session());
    let connections = DashMap::new();
    let (connection, _rx) = test_connection(Some("Tester"));
    connections.insert(CONNECTION_ID, connection);

    let result = host_map(&store, &connections, "maze", "sandbox", None).unwrap();
    assert_eq!(result.map_name, "maze");
    assert_eq!(result.restreamed, 1);
    assert_eq!(*state.mode.read(), GameMode::Sandbox);
    assert_eq!(state.wave.load(Ordering::Relaxed), 1);
    let world = store.load();
    assert_eq!(world.width(), 300);
    assert_eq!(world.height(), 300);
    // Save identity updated and reloadable for the same map.
    let loaded = load_tiles(&save_path, Some((300, 300))).unwrap();
    assert_eq!(loaded.map_name.as_deref(), Some("maze"));
}

#[test]
fn host_map_loads_tracked_world_entity_framing_fixtures() {
    // Production path (`host_map` → `apply_msav_entities`) over frozen 4×4
    // fixtures. Must not consult `../core` or skip: this is the CI gate for
    // the archipelago short-chunk framing bug.
    fn host_fixture(label: &str, bytes: &[u8], class_id: u8, serialized_id: Option<i32>) {
        let path = temp_save_path(&format!("frame-{label}")).with_extension("msav");
        std::fs::write(&path, bytes).unwrap();
        let state = GameState::new();
        state.start_hosting("maze".to_string(), GameMode::Survival);
        let store = WorldStore::new(
            fresh_world_from_template(
                &state,
                embedded_template().to_vec(),
                "maze".to_string(),
                temp_save_path(&format!("frame-host-{label}")),
            )
            .unwrap(),
        );
        let connections = DashMap::new();
        host_map(
            &store,
            &connections,
            &path.display().to_string(),
            "survival",
            None,
        )
        .unwrap_or_else(|err| panic!("{label} host_map: {err}"));
        let world = store.load();
        assert_eq!(world.enemies().len(), 1, "{label} must restore one unit");
        let unit = world.enemies().iter().next().unwrap();
        assert_eq!(unit.entity_class, class_id, "{label}");
        if let Some(id) = serialized_id {
            assert_eq!(unit.id, id, "{label}");
        }
        let _ = std::fs::remove_file(&path);
    }

    host_fixture(
        "v4",
        include_bytes!("fixtures/msav-world-entities/v4.msav"),
        3,
        None,
    );
    host_fixture(
        "v5",
        include_bytes!("fixtures/msav-world-entities/v5.msav"),
        3,
        None,
    );
    host_fixture(
        "v6",
        include_bytes!("fixtures/msav-world-entities/v6.msav"),
        3,
        Some(42),
    );
    host_fixture(
        "v7",
        include_bytes!("fixtures/msav-world-entities/v7.msav"),
        3,
        Some(42),
    );
    host_fixture(
        "v8",
        include_bytes!("fixtures/msav-world-entities/v8.msav"),
        3,
        Some(42),
    );
    host_fixture(
        "v9",
        include_bytes!("fixtures/msav-world-entities/v9.msav"),
        3,
        Some(42),
    );
    host_fixture(
        "v10",
        include_bytes!("fixtures/msav-world-entities/v10.msav"),
        3,
        Some(42),
    );
    host_fixture(
        "v11",
        include_bytes!("fixtures/msav-world-entities/v11.msav"),
        3,
        Some(42),
    );
    host_fixture(
        "v5-archipelago-poly",
        include_bytes!("fixtures/msav-world-entities/v5-archipelago-poly.msav"),
        18,
        None,
    );
}

#[test]
fn host_map_rejects_legacy_v1_to_v3_msav_in_strict_mode() {
    // P1: legacy MSAV v1-v3 are metadata-only (no footprints/modules/
    // entities); strict mode rejects hosting them, non-strict warns and
    // hosts the terrain.
    use oxide::engine::save_io::{write_msav_complete, MsavWorld};
    use std::collections::HashMap;
    let map = HashMap::from([
        ("mapname".to_string(), "legacy".to_string()),
        ("width".to_string(), "2".to_string()),
        ("height".to_string(), "2".to_string()),
        ("build".to_string(), "158".to_string()),
    ]);
    let floors = vec![0i16; 4];
    let overlays = vec![0i16; 4];
    let blocks = vec![0i16; 4];
    let tiles = dashmap::DashMap::new();
    let puddles: [(i32, f32, i16, i32); 0] = [];
    let world = MsavWorld {
        width: 2,
        height: 2,
        floors: &floors,
        overlays: &overlays,
        puddles: &puddles,
        blocks: &blocks,
        team_blocks: None,
        dynamic_tiles: &tiles,
        enemy_units: &[],
        runtime: None,
    };
    let msav = write_msav_complete(&map, 3, &world).unwrap();
    let path = temp_save_path("legacy-v3").with_extension("msav");
    std::fs::write(&path, &msav).unwrap();

    // The host must refuse the legacy save with a clear diagnostic (the
    // network-map extractor cannot reconstruct v1-v3 entity regions).
    let state = GameState::new();
    state.start_hosting("maze".to_string(), GameMode::Survival);
    let world = fresh_world_from_template(
        &state,
        embedded_template().to_vec(),
        "maze".to_string(),
        temp_save_path("legacy-v3"),
    )
    .unwrap();
    let store = WorldStore::new(world);
    let connections = DashMap::new();
    let err = host_map(
        &store,
        &connections,
        &path.display().to_string(),
        "survival",
        None,
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("legacy MSAV"), "{message}");
    assert!(message.contains("SaveVersion 3"), "{message}");
    // A v11 save (the runtime writer output) hosts fine.
    let map11 = HashMap::from([
        ("mapname".to_string(), "modern".to_string()),
        ("width".to_string(), "2".to_string()),
        ("height".to_string(), "2".to_string()),
        ("build".to_string(), "158".to_string()),
    ]);
    let dynamic_tiles = dashmap::DashMap::new();
    let puddles: [(i32, f32, i16, i32); 0] = [];
    let world11 = MsavWorld {
        width: 2,
        height: 2,
        floors: &floors,
        overlays: &overlays,
        puddles: &puddles,
        blocks: &blocks,
        team_blocks: None,
        dynamic_tiles: &dynamic_tiles,
        enemy_units: &[],
        runtime: None,
    };
    let msav11 = write_msav_complete(&map11, 11, &world11).unwrap();
    let path11 = temp_save_path("modern-v11").with_extension("msav");
    std::fs::write(&path11, &msav11).unwrap();
    let state11 = GameState::new();
    state11.start_hosting("maze".to_string(), GameMode::Survival);
    let store11 = WorldStore::new(
        fresh_world_from_template(
            &state11,
            embedded_template().to_vec(),
            "maze".to_string(),
            temp_save_path("modern-host"),
        )
        .unwrap(),
    );
    let result = host_map(
        &store11,
        &connections,
        &path11.display().to_string(),
        "survival",
        None,
    )
    .unwrap();
    assert!(
        result.map_name.contains("modern-v11"),
        "v11 hosts under its file stem: {}",
        result.map_name
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&path11);
}

#[test]
fn game_over_is_ephemeral_never_persisted_nor_restored() {
    // User-reported regression: `gameover` on the console followed by a
    // server restart booted the world ALREADY over — the flag was
    // persisted and restored on load, so the client joined a frozen game
    // with no game-over screen and no way out. Game-over is ephemeral
    // runtime state (the official server does not serialize
    // state.gameOver): old saves that carry the flag must load as live
    // games, and fresh persists must never write it.
    let save_path = temp_save_path("gameover-ephemeral");

    // Old-format save WITH the buggy persisted flag.
    let legacy = r#"{
        "version": 13,
        "map_name": "pruebas01",
        "tiles": [],
        "core_items": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
        "wave": 3,
        "wave_time": 120.0,
        "core_health": 6000.0,
        "game_over": true,
        "enemies": [],
        "base_building_health": [],
        "players": [],
        "building_commands": [],
        "unit_orders": [],
        "team_build_plans": {"teams": []},
        "team_cores": [],
        "team_items": [],
        "simulation_time": 900.0,
        "logic_flags": [],
        "game_stats": {
            "enemy_units_destroyed": 0,
            "waves_lasted": 0,
            "buildings_built": 0,
            "buildings_deconstructed": 0,
            "buildings_destroyed": 0,
            "units_created": 0,
            "placed_block_count": [],
            "destroyed_block_count": [],
            "core_item_count": []
        }
    }"#;
    std::fs::write(&save_path, legacy).unwrap();
    let loaded = load_tiles(&save_path, Some((200, 200))).unwrap();
    assert_eq!(loaded.map_name.as_deref(), Some("pruebas01"));
    assert_eq!(
        loaded.wave,
        Some(3),
        "the legacy game_over flag must not break or poison the load"
    );

    // A fresh persist never writes the field, even while the runtime game
    // is over (snapshot_persisted_world no longer carries it).
    let state = GameState::new();
    state.start_hosting("pruebas01".to_string(), GameMode::Survival);
    state.game_over.store(true, Ordering::Relaxed);
    let world = fresh_world_from_template(
        &state,
        embedded_template().to_vec(),
        "pruebas01".to_string(),
        temp_save_path("gameover-ephemeral-fresh"),
    )
    .unwrap();
    persist_tiles(
        &save_path,
        world.tiles(),
        world.game_state(),
        world.enemies(),
        world.base_buildings(),
        world.player_profiles(),
        world.building_commands(),
        world.unit_orders(),
        &world.team_build_plans(),
        &world.cores,
        &world.logic_flags,
        &world.puddles,
    )
    .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&save_path).unwrap()).unwrap();
    assert!(
        json.get("game_over").is_none(),
        "game_over must not be persisted"
    );
}
