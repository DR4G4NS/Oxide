use oxide::console::command::ServerCommand;
use oxide::engine::entities::EntityManager;
use oxide::engine::spatial::SpatialHashGrid;
use oxide::state::administration::Administration;
use oxide::state::game_state::{GameMode, GameState};

fn official_msav(name: &str) -> Option<Vec<u8>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        root.join("third_party/mindustry-maps").join(name),
        root.join(name),
    ];
    for path in &candidates {
        if path.is_file() {
            return Some(std::fs::read(path).unwrap_or_else(|err| {
                panic!("read {}: {err}", path.display());
            }));
        }
    }
    eprintln!("skip: {name} not present (expected under third_party/mindustry-maps/)");
    None
}

#[test]
fn test_command_parsing() {
    assert_eq!(ServerCommand::parse("help"), Some(ServerCommand::Help));
    assert_eq!(ServerCommand::parse("status"), Some(ServerCommand::Status));
    assert_eq!(
        ServerCommand::parse("host maze survival"),
        Some(ServerCommand::Host {
            map: "maze".to_string(),
            mode: "survival".to_string(),
        })
    );
    assert_eq!(ServerCommand::parse("stop"), Some(ServerCommand::Stop));
    assert_eq!(ServerCommand::parse("exit"), Some(ServerCommand::Exit));
}

#[test]
fn test_command_parsing_waves() {
    assert_eq!(
        ServerCommand::parse("waves"),
        Some(ServerCommand::Waves { wave: None })
    );
    assert_eq!(
        ServerCommand::parse("waves 5"),
        Some(ServerCommand::Waves { wave: Some(5) })
    );
    assert_eq!(
        ServerCommand::parse("waves 0"),
        Some(ServerCommand::Waves { wave: Some(0) })
    );
    // Robustness: malformed numeric arguments fall back to Unknown.
    assert_eq!(
        ServerCommand::parse("waves abc"),
        Some(ServerCommand::Unknown("waves abc".to_string()))
    );
    assert_eq!(
        ServerCommand::parse("waves 1 2"),
        Some(ServerCommand::Unknown("waves 1 2".to_string()))
    );
}

#[test]
fn test_command_parsing_spawn() {
    assert_eq!(
        ServerCommand::parse("spawn dagger 3"),
        Some(ServerCommand::Spawn {
            unit: "dagger".to_string(),
            count: 3,
            x: None,
            y: None,
        })
    );
    assert_eq!(
        ServerCommand::parse("spawn Reign 2 40 100"),
        Some(ServerCommand::Spawn {
            unit: "Reign".to_string(),
            count: 2,
            x: Some(40),
            y: Some(100),
        })
    );
    // Robustness: missing or malformed arguments fall back to Unknown.
    assert_eq!(
        ServerCommand::parse("spawn"),
        Some(ServerCommand::Unknown("spawn".to_string()))
    );
    assert_eq!(
        ServerCommand::parse("spawn dagger"),
        Some(ServerCommand::Unknown("spawn dagger".to_string()))
    );
    assert_eq!(
        ServerCommand::parse("spawn dagger x"),
        Some(ServerCommand::Unknown("spawn dagger x".to_string()))
    );
    assert_eq!(
        ServerCommand::parse("spawn dagger 2 40"),
        Some(ServerCommand::Unknown("spawn dagger 2 40".to_string()))
    );
}

#[test]
fn test_command_parsing_mode_time() {
    assert_eq!(
        ServerCommand::parse("mode sandbox"),
        Some(ServerCommand::Mode {
            mode: "sandbox".to_string()
        })
    );
    assert_eq!(
        ServerCommand::parse("mode pvp"),
        Some(ServerCommand::Mode {
            mode: "pvp".to_string()
        })
    );
    assert_eq!(
        ServerCommand::parse("time 300"),
        Some(ServerCommand::Time {
            value: 300.0,
            seconds: false
        })
    );
    assert_eq!(
        ServerCommand::parse("time 5 s"),
        Some(ServerCommand::Time {
            value: 5.0,
            seconds: true
        })
    );
    assert_eq!(
        ServerCommand::parse("time 90 seconds"),
        Some(ServerCommand::Time {
            value: 90.0,
            seconds: true
        })
    );
    assert_eq!(
        ServerCommand::parse("time abc"),
        Some(ServerCommand::Unknown("time abc".to_string()))
    );
}

#[test]
fn test_command_parsing_unknown_and_empty() {
    assert_eq!(
        ServerCommand::parse("bogus command here"),
        Some(ServerCommand::Unknown("bogus command here".to_string()))
    );
    assert_eq!(ServerCommand::parse(""), None);
    assert_eq!(ServerCommand::parse("   "), None);
}

#[test]
fn test_multithreaded_entity_updates() {
    let entities = EntityManager::new();
    let id1 = entities.alloc_id();
    let id2 = entities.alloc_id();

    assert_ne!(id1, id2);

    // Run parallel Rayon passes
    entities.update_units_parallel(1.0);
    entities.update_bullets_parallel(1.0);
}

#[test]
fn test_spatial_grid() {
    let grid = SpatialHashGrid::new(200, 200);
    assert_eq!(grid.width_chunks, 13);
    assert_eq!(grid.height_chunks, 13);
    assert!(grid.get_chunk(0, 0).is_some());
    assert!(grid.get_chunk(100, 100).is_none());
}

#[test]
fn test_admin_bans() {
    // Use an isolated file so a stray admin-data.json from other tests never
    // pre-populates bans (Administration::new() loads from the cwd).
    let path = std::env::temp_dir().join(format!(
        "mindustry-server-bans-test-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let admin = Administration::with_file(path.clone());
    let ip = "192.168.1.50";
    let uuid = "abc123test";

    assert!(!admin.is_banned(ip, uuid));
    admin.ban_ip(ip);
    assert!(admin.is_banned(ip, uuid));
    assert!(admin.pardon_ip(ip));
    assert!(!admin.is_banned(ip, uuid));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn test_game_state_hosting() {
    let state = GameState::new();
    assert!(!state.is_active());

    state.start_hosting("frozen".to_string(), GameMode::Survival);
    assert!(state.is_active());
    assert_eq!(*state.map_name.read(), "frozen");

    state.stop_hosting();
    assert!(!state.is_active());
}

#[test]
fn test_max_packet_size_exceeded() {
    use oxide::network::codec::{read_packet, MAX_PACKET_SIZE};
    use std::io::Cursor;

    let mut data = vec![1u8]; // packet id
    data.extend_from_slice(&((MAX_PACKET_SIZE + 1) as u16).to_be_bytes()); // oversized payload length
    data.push(0); // uncompressed

    let cursor = Cursor::new(data);
    assert!(read_packet(cursor).is_err());
}

#[test]
fn test_msav_header_validation() {
    use oxide::engine::save_io::SaveIO;
    use std::io::Cursor;

    let valid_header = b"MSAVdummydata";
    assert!(SaveIO::is_msav_header(Cursor::new(valid_header)));

    let invalid_header = b"BADHdummydata";
    assert!(!SaveIO::is_msav_header(Cursor::new(invalid_header)));
}

#[test]
fn test_content_registry_indexing() {
    use oxide::game::content::{BlockType, ContentRegistry, Item, Liquid};

    let registry = ContentRegistry::new();
    assert_eq!(registry.get_item(0), Some(Item::Copper));
    assert_eq!(registry.get_liquid(0), Some(Liquid::Water));
    assert_eq!(registry.get_block(0), Some(BlockType::Conveyor));
}

#[test]
fn test_official_block_requirements_manifest() {
    use oxide::game::content::{
        block_armor, block_build_time, block_can_replace, block_health, block_navigation,
        block_pathing, block_requirements, block_size, unit_movement,
    };

    assert_eq!(block_requirements(257), &[(0, 1)]); // conveyor
    assert_eq!(block_requirements(349), &[(0, 35)]); // duo
    assert_eq!(block_requirements(0), &[]); // air
    assert_eq!(block_health(216), 320.0); // copper wall
    assert_eq!(block_health(240), 1000.0); // surge wall
    assert_eq!(block_armor(240), 20.0);
    assert!(block_navigation(216).solid); // copper wall
    assert!(!block_navigation(228).solid); // official non-solid entry
    assert!(!block_pathing(80).synthetic); // natural dark wall
    assert!(block_pathing(80).fills_tile);
    assert!(block_pathing(216).synthetic); // copper wall
    assert_eq!(block_size(257), 1); // conveyor
    assert_eq!(block_size(341), 5); // core nucleus
    assert_eq!(block_build_time(350), 74.0); // scatter; alpha needs 148 ticks
    assert!(block_can_replace(261, 257)); // junction over conveyor
    assert!(block_can_replace(266, 257)); // router over conveyor
    assert!(block_can_replace(262, 257)); // bridge conveyor over conveyor
    assert!(!block_can_replace(216, 257)); // wall is not transportation
    assert!(!block_can_replace(257, 216));
    assert_eq!(unit_movement(0).hit_size, 8.0); // Dagger
    assert_eq!(unit_movement(19).hit_size, 58.0); // Eclipse
    assert!(unit_movement(15).flying); // Flare
    assert!(!unit_movement(0).flying);
}

#[test]
fn test_rpc_packet_serialization() {
    use oxide::network::rpc::RpcPacket;
    use std::io::Cursor;

    let packet = RpcPacket::TileTap {
        player_id: 42,
        x: 100,
        y: 200,
    };
    let packet_id = packet.id();
    let mut buf = Vec::new();
    packet.write(&mut buf).unwrap();

    let decoded = RpcPacket::read(Cursor::new(&buf[..]), packet_id).unwrap();
    match decoded {
        RpcPacket::TileTap { player_id, x, y } => {
            assert_eq!(player_id, 42);
            assert_eq!(x, 100);
            assert_eq!(y, 200);
        }
        _ => panic!("Mismatched RPC packet variant"),
    }
}

#[test]
fn test_tcp_framing_kryonet() {
    use oxide::network::codec::{read_tcp_packet, write_tcp_packet};
    use std::io::Cursor;

    let payload = b"Hello Mindustry TCP";
    let mut buf = Vec::new();
    write_tcp_packet(&mut buf, 3, payload, false).unwrap();

    // Verify 2-byte Kryonet length header
    let tcp_len = u16::from_be_bytes([buf[0], buf[1]]);
    assert_eq!(tcp_len as usize, buf.len() - 2);

    let decoded = read_tcp_packet(Cursor::new(buf)).unwrap();
    assert_eq!(decoded[0], 3); // Packet ID
    assert_eq!(&decoded[1..], payload);
}

#[test]
fn test_arcnet_registration_frame() {
    use oxide::network::listener::{framework_keepalive, framework_registration};
    use oxide::network::protocol::CONNECT_CONFIRM_PACKET_ID;

    let frame = framework_registration(4, 0x0102_0304);
    assert_eq!(&frame[..2], &6u16.to_be_bytes());
    assert_eq!(&frame[2..], &[254, 4, 1, 2, 3, 4]);
    assert_eq!(framework_keepalive(), [0, 2, 254, 2]);
    assert_eq!(CONNECT_CONFIRM_PACKET_ID, 33);
}

#[test]
fn test_discovery_response_matches_network_io_layout() {
    use oxide::network::listener::encode_server_info;
    use oxide::network::world::ServerInfo;

    let state = GameState::new();
    state.start_hosting("maze".into(), GameMode::Survival);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let admin_path = std::env::temp_dir().join(format!(
        "mindustry-server-discovery-test-{}-{nonce}.json",
        std::process::id()
    ));
    let admin = Administration::with_file(admin_path.clone());
    admin.set_player_limit(20);
    let bytes = encode_server_info(
        &ServerInfo {
            name: "Rust".into(),
            description: "Test".into(),
            build: 151,
            version_type: "official".into(),
        },
        &state,
        &admin,
        6567,
    );

    assert_eq!(bytes[0], 4);
    assert_eq!(&bytes[1..5], b"Rust");
    assert_eq!(&bytes[bytes.len() - 2..], &6567i16.to_be_bytes());
    drop(admin);
    let _ = std::fs::remove_file(admin_path);
}

#[test]
fn test_oversized_packet_is_not_truncated_on_write() {
    use oxide::network::codec::{write_tcp_packet, MAX_PAYLOAD_SIZE};

    let mut output = Vec::new();
    assert!(write_tcp_packet(&mut output, 3, &vec![0; MAX_PAYLOAD_SIZE + 1], false).is_err());
    assert!(output.is_empty());
}

#[test]
fn test_zero_tps_is_safely_clamped() {
    use oxide::engine::tick::TickEngine;

    let engine = TickEngine::new(0, GameState::new());
    assert_eq!(engine.target_tps, 1);
}

#[test]
fn test_generated_packet_ids_are_not_mistaken_for_framework_messages() {
    use oxide::network::packets::Packet;
    use std::io::Cursor;

    match Packet::read(Cursor::new([1, 2, 3]), 6).unwrap() {
        Packet::Unknown { id, payload } => {
            assert_eq!(id, 6);
            assert_eq!(payload, [1, 2, 3]);
        }
        packet => panic!("expected raw generated packet, got {packet:?}"),
    }
}

#[test]
fn test_v8_state_snapshot_layout() {
    use byteorder::{BigEndian, ReadBytesExt};
    use oxide::network::listener::encode_state_snapshot;
    use std::io::{Cursor, Read};

    let payload = encode_state_snapshot().unwrap();
    let mut input = Cursor::new(payload);
    assert_eq!(input.read_f32::<BigEndian>().unwrap(), 180.0);
    assert_eq!(input.read_i32::<BigEndian>().unwrap(), 1);
    assert_eq!(input.read_i32::<BigEndian>().unwrap(), 0);
    let mut flags = [0; 2];
    input.read_exact(&mut flags).unwrap();
    assert_eq!(flags, [0, 0]);
    input.set_position(input.position() + 4 + 1 + 8 + 8);
    assert_eq!(input.read_i16::<BigEndian>().unwrap(), 10);
    assert_eq!(input.read_u8().unwrap(), 1); // active teams
    assert_eq!(input.read_u8().unwrap(), 1); // sharded
    assert_eq!(input.read_i16::<BigEndian>().unwrap(), 1);
    assert_eq!(input.read_i16::<BigEndian>().unwrap(), 0); // copper
    assert_eq!(input.read_i32::<BigEndian>().unwrap(), 100);
}

#[test]
fn test_world_stream_is_personalized_per_connection() {
    use oxide::engine::world_stream::{
        inspect, inspect_timing, personalize, personalize_desktop_158_with_state,
        personalize_with_state,
    };

    let template = include_bytes!("../src/dummy_world.dat");
    let alice = personalize(template, 1001, "Alice", 0x11223344).unwrap();
    let bob = personalize(template, 1002, "Bob", 0x55667788).unwrap();

    assert_ne!(alice, bob);
    assert_eq!(
        inspect(&alice).unwrap(),
        oxide::engine::world_stream::EmbeddedPlayer {
            id: 1001,
            name: "Alice".into(),
            color: 0x11223344,
        }
    );
    assert_eq!(inspect(&bob).unwrap().id, 1002);
    assert_eq!(inspect(&bob).unwrap().name, "Bob");

    let resumed = personalize_with_state(
        template,
        1003,
        "Resume",
        7,
        (400.0, 800.0),
        61,
        1234.5,
        98765.0,
    )
    .unwrap();
    assert_eq!(inspect_timing(&resumed).unwrap(), (61, 1234.5, 98765.0));

    let desktop = personalize_desktop_158_with_state(
        template,
        1003,
        "Desktop",
        0x01020304,
        (400.0, 800.0),
        61,
        1234.5,
        98765.0,
    )
    .unwrap();
    let mut decoder = flate2::read::ZlibDecoder::new(desktop.as_slice());
    let mut decoded = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut decoded).unwrap();
    let rules_length = u16::from_be_bytes([decoded[0], decoded[1]]) as usize;
    assert!(rules_length > 100);
    assert_eq!(decoded[2], b'{');
    assert_ne!(&decoded[..8], &[0, 0, 0, 2, 0, 0, 0, 0]);
}

#[test]
fn test_network_world_map_is_authoritatively_decoded() {
    let template = include_bytes!("../src/dummy_world.dat");
    let map = oxide::engine::world_stream::inspect_map(template).unwrap();
    assert_eq!((map.width, map.height), (300, 300));
    assert_eq!(map.blocks.len(), 90_000);
    assert_eq!(map.floors.len(), 90_000);
    assert_eq!(map.tile_data.len(), 90_000);
    assert_eq!(map.overlays.len(), 90_000);
    assert!(map.blocks.iter().any(|block| *block != 0));
    assert!(map
        .overlays
        .iter()
        .any(|overlay| (167..=169).contains(overlay)));
    assert_eq!(map.blocks[100 * 300 + 35], 0);
    assert_eq!(map.overlays[100 * 300 + 35], 167);
    assert_eq!(
        map.buildings,
        vec![oxide::engine::world_stream::NetworkBuilding {
            position: (40 << 16) | 100,
            block: 341,
            health: 6000.0,
            rotation: 0,
            team: 1,
            // The authoritative template core stores 400 copper in its item
            // module. Legacy trailing module bytes remain opaque because the
            // network decoder has no block registry at this stage.
            inventory: vec![(0, 400)],
            power_links: Vec::new(),
            power_status: 0.0,
            liquids: Vec::new(),
            enabled: true,
            extra_data: vec![127, 192, 0, 0, 127, 192, 0, 0],
        }]
    );
}

#[test]
fn test_network_world_map_exposes_enemy_spawn_overlays() {
    let map =
        oxide::engine::world_stream::inspect_map(include_bytes!("../src/dummy_world.dat")).unwrap();
    let spawns = map.enemy_spawns();
    assert!(
        !spawns.is_empty(),
        "the bundled survival map needs an enemy spawn"
    );
    assert!(spawns.iter().all(|(x, y)| *x >= 0 && *y >= 0));
}

#[test]
fn test_official_msav_map_region_replaces_network_template() {
    use oxide::engine::world_stream::{
        inspect_map, inspect_metadata, inspect_team_count, replace_map_from_msav,
    };

    let template = include_bytes!("../src/dummy_world.dat");
    let Some(official_map) = official_msav("groundZero.msav") else {
        return;
    };
    let replaced = replace_map_from_msav(template, &official_map).unwrap();
    let original = inspect_map(template).unwrap();
    let selected = inspect_map(&replaced).unwrap();
    let original_metadata = inspect_metadata(template).unwrap();
    let selected_metadata = inspect_metadata(&replaced).unwrap();

    assert_ne!(
        (selected.width, selected.height),
        (original.width, original.height)
    );
    assert_eq!(
        selected.blocks.len(),
        usize::from(selected.width) * usize::from(selected.height)
    );
    assert!(!selected.buildings.is_empty());
    let selected_core = selected
        .buildings
        .iter()
        .find(|building| building.team == 1 && (339..=344).contains(&building.block))
        .expect("official map must expose a Sharded core");
    let original_core = original
        .buildings
        .iter()
        .find(|building| building.team == 1 && (339..=344).contains(&building.block))
        .unwrap();
    assert_ne!(selected_core.position, original_core.position);
    assert_ne!(selected_metadata.rules, original_metadata.rules);
    assert!(selected_metadata
        .tags
        .iter()
        .any(|(key, value)| key == "rules" && value == &selected_metadata.rules));
    assert!(inspect_team_count(&replaced).unwrap() > 0);
}

#[test]
fn test_cli_accepts_an_official_msav_map_path() {
    use clap::Parser;
    use oxide::config::ServerConfig;

    let config = ServerConfig::try_parse_from([
        "oxide",
        "--map-file",
        "../core/assets/maps/default/archipelago.msav",
        "--save-file",
        "archipelago.json",
    ])
    .unwrap();
    assert_eq!(
        config.map_file.unwrap(),
        std::path::PathBuf::from("../core/assets/maps/default/archipelago.msav")
    );
    assert_eq!(
        config.save_file,
        std::path::PathBuf::from("archipelago.json")
    );
    assert_eq!(config.build, 159);
    assert_eq!(config.version_type, "official");
}

#[test]
fn test_connect_packet_official_round_trip_includes_crc_and_all_fields() {
    use oxide::network::packets::{ConnectPacket, Packet};
    use std::io::Cursor;

    let expected = ConnectPacket {
        version: -1,
        version_type: "official".into(),
        name: "Alice".into(),
        locale: "es_MX".into(),
        usid: "session-id".into(),
        uuid: "AQEBAQEBAQEBAQEBAQEBAQ==".into(),
        mobile: true,
        color: 0x11223344,
        mods: vec!["example-mod".into()],
    };
    let mut bytes = Vec::new();
    Packet::ConnectPacket(expected.clone())
        .write(&mut bytes)
        .unwrap();
    let decoded = Packet::read(Cursor::new(bytes), 3).unwrap();
    match decoded {
        Packet::ConnectPacket(actual) => {
            assert_eq!(actual.version, expected.version);
            assert_eq!(actual.version_type, expected.version_type);
            assert_eq!(actual.name, expected.name);
            assert_eq!(actual.locale, expected.locale);
            assert_eq!(actual.usid, expected.usid);
            assert_eq!(actual.uuid, expected.uuid);
            assert_eq!(actual.mobile, expected.mobile);
            assert_eq!(actual.color, expected.color);
            assert_eq!(actual.mods, expected.mods);
        }
        packet => panic!("unexpected packet: {packet:?}"),
    }
}

#[test]
fn test_connect_packet_decodes_exact_desktop_158_fixture() {
    use base64::{engine::general_purpose, Engine as _};
    use oxide::network::codec::read_packet;
    use oxide::network::packets::Packet;
    use std::io::Cursor;

    // Independent v158.1-compatible desktop layout fixture: UUID is 16 bytes
    // and the CRC is over those bytes (the same layout remains in v159.7).
    let object = general_purpose::STANDARD
        .decode("AwBJAAAAAJ4BAAhvZmZpY2lhbAEAB0ZpeHR1cmUBAAVlc19NWAEAB3Nlc3Npb24BAQEBAQEBAQEBAQEBAQEBAAAAAFKgKLcAESIzRAA=")
        .unwrap();
    let decoded = read_packet(Cursor::new(object)).unwrap();
    match Packet::read(Cursor::new(&decoded[1..]), decoded[0]).unwrap() {
        Packet::ConnectPacket(connect) => {
            assert_eq!(connect.version, 158);
            assert_eq!(connect.version_type, "official");
            assert_eq!(connect.name, "Fixture");
            assert_eq!(connect.locale, "es_MX");
            assert_eq!(connect.usid, "session");
            assert_eq!(connect.uuid, "AQEBAQEBAQEBAQEBAQEBAQ==");
            assert!(!connect.mobile);
            assert_eq!(connect.color, 0x11223344);
            assert!(connect.mods.is_empty());
        }
        packet => panic!("unexpected packet: {packet:?}"),
    }
}

#[test]
fn test_connect_packet_decodes_official_desktop_8_byte_uuid_write() {
    use oxide::network::packets::{ConnectPacket, Packet};
    use std::io::Cursor;

    // Official `Platform.getUUID`: "Must be a base64 string 8 bytes in length."
    // ConnectPacket.write emits those 8 bytes plus a CRC32 long — not a
    // 16-byte identity plus a second CRC.
    let expected = ConnectPacket {
        version: 159,
        version_type: "official".into(),
        name: "Dr4g4n".into(),
        locale: "es_MX".into(),
        usid: "session".into(),
        uuid: "AQIDBAUGBwg=".into(),
        mobile: false,
        color: 0xffa6_65ffu32 as i32,
        mods: vec![],
    };
    let mut bytes = Vec::new();
    Packet::ConnectPacket(expected.clone())
        .write(&mut bytes)
        .unwrap();
    assert_eq!(
        bytes.len(),
        4 + 1 + 2 + 8 + 1 + 2 + 6 + 1 + 2 + 5 + 1 + 2 + 7 + 8 + 8 + 1 + 4 + 1,
        "8-byte uuid + CRC must not be padded to 16 identity bytes"
    );
    let decoded = Packet::read(Cursor::new(bytes), 3).unwrap();
    match decoded {
        Packet::ConnectPacket(actual) => {
            assert_eq!(actual.version, expected.version);
            assert_eq!(actual.version_type, expected.version_type);
            assert_eq!(actual.name, expected.name);
            assert_eq!(actual.locale, expected.locale);
            assert_eq!(actual.usid, expected.usid);
            assert_eq!(actual.uuid, expected.uuid);
            assert_eq!(actual.mobile, expected.mobile);
            assert_eq!(actual.color, expected.color);
            assert_eq!(actual.mods, expected.mods);
        }
        packet => panic!("unexpected packet: {packet:?}"),
    }
}

#[test]
fn test_connect_packet_decodes_exact_desktop_1597_jar_write() {
    use base64::{engine::general_purpose, Engine as _};
    use oxide::network::packets::Packet;
    use std::io::Cursor;

    // Byte-exact `Packets.ConnectPacket.write` from desktop 159.7.jar with
    // Version.build=159 and Platform 8-byte UUID `AQIDBAUGBwg=`.
    let object = general_purpose::STANDARD
        .decode("AAAAnwEACG9mZmljaWFsAQAGRHI0ZzRuAQAFZXNfTVgBAAdzZXNzaW9uAQIDBAUGBwgAAAAAP8qIxQD/pmX/AA==")
        .unwrap();
    let decoded = Packet::read(Cursor::new(object), 3).unwrap();
    match decoded {
        Packet::ConnectPacket(connect) => {
            assert_eq!(connect.version, 159);
            assert_eq!(connect.version_type, "official");
            assert_eq!(connect.name, "Dr4g4n");
            assert_eq!(connect.locale, "es_MX");
            assert_eq!(connect.usid, "session");
            assert_eq!(connect.uuid, "AQIDBAUGBwg=");
            assert!(!connect.mobile);
            assert_eq!(connect.color, 0xffa6_65ffu32 as i32);
            assert!(connect.mods.is_empty());
        }
        packet => panic!("unexpected packet: {packet:?}"),
    }
}
