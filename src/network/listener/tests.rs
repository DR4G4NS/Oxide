//! Adapter integration tests.

use crate::network::buildings::plans::{assist_visual_plan, rebuild_plan};

use super::*;

/// Official vanilla campaign maps live under `third_party/mindustry-maps/`
/// (GPLv3, Anuken). Tests skip only if that extract is missing.
fn official_msav(name: &str) -> Option<Vec<u8>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
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
fn desktop_159_client_noop_call_ids_and_payload_guards_are_verified() {
    assert_eq!(DEBUG_STATUS_CLIENT_PACKET_ID, 39);
    assert_eq!(DEBUG_STATUS_CLIENT_UNRELIABLE_PACKET_ID, 40);
    assert_eq!(TILE_TAP_PACKET_ID, 140);
    assert_eq!(REQUEST_DEBUG_STATUS_PACKET_ID, 89);
    assert_eq!(MENU_CHOOSE_PACKET_ID, 71);
    assert_eq!(TEXT_INPUT_RESULT_PACKET_ID, 138);

    // TileTap.write emits one packed tile position.
    assert!(valid_client_noop_payload(TILE_TAP_PACKET_ID, &[0; 4]));
    assert!(!valid_client_noop_payload(TILE_TAP_PACKET_ID, &[0; 3]));
    // RequestDebugStatus.write emits no fields.
    assert!(valid_client_noop_payload(
        REQUEST_DEBUG_STATUS_PACKET_ID,
        &[]
    ));
    assert!(!valid_client_noop_payload(
        REQUEST_DEBUG_STATUS_PACKET_ID,
        &[0]
    ));
    // MenuChoose.write emits two ints.
    assert!(valid_client_noop_payload(MENU_CHOOSE_PACKET_ID, &[0; 8]));
    assert!(!valid_client_noop_payload(MENU_CHOOSE_PACKET_ID, &[0; 7]));

    // TextInputResultCallPacket.write emits int textInputId +
    // TypeIO.writeString: one tag byte (0 = null, 1 = present) and, for
    // non-null, a big-endian u16 modified-UTF byte length + the bytes.
    // Null is exactly [id][tag 0] (5 bytes total).
    assert!(valid_text_input_result_payload(&[0, 0, 0, 7, 0]));
    assert!(!valid_text_input_result_payload(&[0, 0, 0, 7, 0, 0]));
    // Too short to hold id + tag.
    assert!(!valid_text_input_result_payload(&[0; 4]));
    // Truncated before the u16 length.
    assert!(!valid_text_input_result_payload(&[0, 0, 0, 7, 1, 0]));
    // ASCII: [id][tag 1][u16 len 3][b"abc"] (7 + 3 bytes).
    let mut text = vec![0, 0, 0, 7, 1, 0, 3];
    text.extend_from_slice(b"abc");
    assert!(valid_text_input_result_payload(&text));
    // One byte short of the declared length is malformed.
    text.pop();
    assert!(!valid_text_input_result_payload(&text));
    // Length field overstates the payload.
    let mut overstated = vec![0, 0, 0, 7, 1, 0, 4];
    overstated.extend_from_slice(b"abc");
    assert!(!valid_text_input_result_payload(&overstated));
    // MUTF-8: "é" is 2 bytes, so the u16 length counts bytes, not chars.
    let mut utf = vec![0, 0, 0, 7, 1, 0, 2];
    utf.extend_from_slice(&[0xc3, 0xa9]);
    assert!(valid_text_input_result_payload(&utf));
    utf.push(0);
    assert!(!valid_text_input_result_payload(&utf));
    // Unknown tags are rejected.
    assert!(!valid_text_input_result_payload(&[0, 0, 0, 7, 2, 0, 0]));
    assert!(!valid_text_input_result_payload(&[
        0, 0, 0, 7, 0xff, 0, 1, b'a'
    ]));
}

#[test]
fn debug_status_response_matches_desktop_158_call_layout() {
    use crate::network::codec::read_tcp_packet;
    use std::io::Cursor;

    for (packet_id, expected_flags) in [
        (DEBUG_STATUS_CLIENT_PACKET_ID, 0b1110),
        (DEBUG_STATUS_CLIENT_UNRELIABLE_PACKET_ID, 0b1110),
    ] {
        let frame = encode_debug_status_client(packet_id, expected_flags, 42).unwrap();
        let packet = read_tcp_packet(Cursor::new(frame)).unwrap();
        assert_eq!(packet[0], packet_id);
        assert_eq!(&packet[1..5], &expected_flags.to_be_bytes());
        assert_eq!(&packet[5..9], &42i32.to_be_bytes());
        // DebugStatusClientCallPacket.snapshotsSent is left at its Java
        // default (zero) by NetServer.requestDebugStatus.
        assert_eq!(&packet[9..13], &0i32.to_be_bytes());
    }
    assert!(encode_debug_status_client(REQUEST_DEBUG_STATUS_PACKET_ID, 0, 0).is_err());
}

#[test]
fn leg_solid_matches_static_darkness_and_solid_floor_rules() {
    assert!(!tile_is_leg_solid(80, 0, 1));
    assert!(tile_is_leg_solid(80, 0, 2));
    assert!(!tile_is_leg_solid(216, 0, 2)); // synthetic Copper Wall
    assert!(tile_is_leg_solid(0, 31, 0));
    assert!(!tile_is_leg_solid(80, 31, 0));
}

#[test]
fn allied_multi_mount_timers_match_official_volleys() {
    let make = |id: i32, spec: EnemySpec, health: f32| EnemyUnit {
        id,
        unit_type: spec.unit_type,
        entity_class: spec.entity_class,
        team: 1,
        x: 0.0,
        y: 0.0,
        rotation: 0.0,
        health,
        shield: 0.0,
        status_effect: -1,
        status_duration: f32::MAX,
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
        move_speed: spec.speed,
        attack_damage: spec.attack_damage,
        attack_reload_time: spec.attack_reload,
        attack_range: spec.attack_range,
        authority: UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        status_agg: None,
    };
    let mut antumbra = make(1, ANTUMBRA, ANTUMBRA.health);
    antumbra.team = 1;
    let fire = collect_allied_weapon_fire(&mut antumbra, 140.0, 50.0).unwrap();
    let missiles = fire
        .iter()
        .filter(
            |fire| matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == 33),
        )
        .count();
    let cannons = fire
        .iter()
        .filter(
            |fire| matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == 34),
        )
        .count();
    assert_eq!((missiles, cannons), (11, 11));

    let mut risso = make(2, enemy_spec(25).unwrap(), 1_000.0);
    risso.team = 1;
    let fire = collect_allied_weapon_fire(&mut risso, 50.0, 50.0).unwrap();
    assert_eq!(
        fire.iter()
            .filter(|fire| matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == 41))
            .count(),
        3
    );
    assert_eq!(
        fire.iter()
            .filter(|fire| matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == 42))
            .count(),
        2
    );

    let mut retusa = make(3, RETUSA, RETUSA.health);
    retusa.team = 1;
    let fire = collect_allied_weapon_fire(&mut retusa, 105.0, 50.0).unwrap();
    assert_eq!(
        fire.iter()
            .filter(|fire| matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == 51))
            .count(),
        4
    );
    assert_eq!(
        fire.iter()
            .filter(|fire| matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == 52))
            .count(),
        3
    );

    let mut navanax = make(4, enemy_spec(34).unwrap(), 10_000.0);
    navanax.team = 1;
    let fire = collect_allied_weapon_fire(&mut navanax, 170.0, 90.0).unwrap();
    assert_eq!(
        fire.iter()
            .filter(|fire| matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == 60))
            .count(),
        2
    );
    assert_eq!(
        fire.iter()
            .filter(|fire| matches!(fire, AlliedWeaponFire::NavanaxLasers(_)))
            .count(),
        1
    );
    let mut boosted_antumbra = make(5, ANTUMBRA, 10_000.0);
    boosted_antumbra.team = 1;
    boosted_antumbra.status_effect = 14;
    boosted_antumbra.status_duration = 360.0;
    let fire = collect_allied_weapon_fire(&mut boosted_antumbra, 20.0, 100.0).unwrap();
    assert!(fire.iter().any(|fire| matches!(
        fire,
        AlliedWeaponFire::Projectile(volley)
            if volley.bullet_id == 33
                && (volley.direct_damage - ANTUMBRA_MISSILE.direct_damage * 1.15).abs() < 0.001
    )));

    let mut disarmed = make(6, ANTUMBRA, ANTUMBRA.health);
    disarmed.team = 1;
    crate::network::units::StatusContainer::apply_status(&mut disarmed, 20, 60.0);
    disarmed.attack_reload = 20.0;
    let fire = collect_allied_weapon_fire(&mut disarmed, 20.0, 50.0).unwrap();
    assert!(
        fire.is_empty(),
        "disarmed must suppress firing even when the reload timer is ready"
    );
    assert!(!unit_can_shoot(&disarmed));

    let mut nova = make(7, enemy_spec(5).unwrap(), 500.0);
    crate::network::units::StatusContainer::apply_status(&mut nova, 22, f32::INFINITY);
    nova.statuses[0].dynamic = Some(crate::game::status::DynamicStatus {
        build_speed: 4.0,
        armor_override: Some(10.0),
        ..crate::game::status::DynamicStatus::default()
    });
    assert!(
        (effective_unit_build_speed(&nova).unwrap() - 0.3 * 4.0).abs() < 1e-6,
        "buildSpeed aggregate must scale BuilderComp work"
    );
    assert!(
        (crate::network::combat::unit_effective_armor(&nova) - 10.0).abs() < 1e-6,
        "armorOverride must replace type armor"
    );
    let incoming = crate::network::combat::apply_incoming_unit_damage(&nova, 20.0, 1.0);
    assert!(
        (incoming - crate::network::combat::apply_unit_armor(20.0, 10.0)).abs() < 1e-4,
        "incoming damage must use armorOverride"
    );
}

#[test]
fn retusa_mine_burst_preserves_official_seven_tick_delays() {
    assert_eq!(retusa_mine_shots_between(0.0, 89.0), 0);
    assert_eq!(retusa_mine_shots_between(0.0, 90.0), 1);
    assert_eq!(retusa_mine_shots_between(90.0, 96.0), 0);
    assert_eq!(retusa_mine_shots_between(90.0, 97.0), 1);
    assert_eq!(retusa_mine_shots_between(97.0, 104.0), 1);
    assert_eq!(retusa_mine_shots_between(0.0, 194.0), 6);
}

#[test]
fn navigation_step_uses_official_diagonal_sampling_in_open_space() {
    // Cardinal propagation costs toward tile 0 on a 3x3 field.
    let costs = [0, 1, 2, 1, 2, 3, 2, 3, 4];
    assert_eq!(choose_navigation_step(&costs, 3, 3, 8), Some(4));
}

#[test]
fn navigation_step_uses_exact_geometry_d8_tie_order() {
    // From the center, east and north-east have the same best cost.
    // Geometry.d8 in Arc 158.1 lists east first, and Java keeps the first
    // candidate because later comparisons use a strict cost inequality.
    let costs = [9, 9, 9, 9, 5, 4, 9, 9, 4];
    assert_eq!(choose_navigation_step(&costs, 3, 3, 4), Some(5));
}

#[test]
fn navigation_step_honors_official_unit_avoidance_mask() {
    let costs = [9, 2, 9, 9, 5, 1, 9, 9, 9];
    assert_eq!(choose_navigation_step(&costs, 3, 3, 4), Some(5));
    assert_eq!(
        choose_navigation_step_with(&costs, 3, 3, 4, |candidate| candidate != 5),
        Some(1)
    );
}

#[test]
fn navigation_step_does_not_cut_an_impassable_corner() {
    let impassable = u32::MAX / 4;
    let costs = [0, 1, 2, 1, 2, impassable, 2, impassable, 4];
    assert_eq!(choose_navigation_step(&costs, 3, 3, 8), None);
}

#[test]
fn pending_construction_snapshot_uses_construct_block_class() {
    use crate::network::codec::Reads;

    for (target, expected_construct) in [(270, 5), (267, 6)] {
        let frame = encode_construct_block_snapshot((102 << 16) | 182, target, 3, 1).unwrap();
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        assert_eq!(packet[0], BLOCK_SNAPSHOT_PACKET_ID);
        let mut input = std::io::Cursor::new(&packet[1..]);
        assert_eq!(input.read_s().unwrap(), 1);
        assert!(input.read_s().unwrap() > 0);
        assert_eq!(input.read_i().unwrap(), (102 << 16) | 182);
        assert_eq!(input.read_s().unwrap(), expected_construct);
    }
}

#[test]
fn parallel_block_snapshots_match_sequential_bytes_and_are_deterministic() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let mut power = std::collections::HashMap::new();
    // Memory banks make each independent codec non-trivial (512 f64s),
    // while their writeSync path reads only the owned DynamicTile clone.
    for index in 0..128i32 {
        let x = 10 + index % 16;
        let y = 10 + index / 16;
        let position = (x << 16) | y;
        let mut tile = DynamicTile {
            position,
            block: if index % 3 == 0 { 435 } else { 181 },
            rotation: (index % 4) as u8,
            team: 1,
            occupied: vec![position],
            health: crate::game::content::block_health(if index % 3 == 0 { 435 } else { 181 }),
            production_progress: index as f32,
            inventory: vec![(0, index + 1)],
            ..DynamicTile::default()
        };
        if tile.block == 435 {
            tile.memory = (0..512).map(|cell| f64::from(index * 512 + cell)).collect();
        }
        power.insert(position, (index % 10) as f32 / 10.0);
        world.tiles.insert(position, tile);
    }
    // Interleave codecs that intentionally stay on the caller because
    // they consult topology/simulation state. The hybrid merge must still
    // retain the single globally sorted wire order.
    for (offset, block) in [302, 261, 262, 345].into_iter().enumerate() {
        let position = ((30 + offset as i32) << 16) | 12;
        world.tiles.insert(
            position,
            DynamicTile {
                position,
                block,
                team: 1,
                occupied: vec![position],
                health: crate::game::content::block_health(block),
                config: vec![0],
                ..DynamicTile::default()
            },
        );
    }

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    let (sequential, sequential_execution) =
        encode_block_snapshots_with_threshold(&world, &power, usize::MAX).unwrap();
    let (parallel_a, parallel_execution) = pool
        .install(|| encode_block_snapshots_with_threshold(&world, &power, 2))
        .unwrap();
    let (parallel_b, _) = pool
        .install(|| encode_block_snapshots_with_threshold(&world, &power, 2))
        .unwrap();
    assert_eq!(parallel_a, sequential, "parallel wire bytes must be exact");
    assert_eq!(
        parallel_b, sequential,
        "repeated runs must be deterministic"
    );
    assert!(!sequential_execution.parallel);
    assert!(parallel_execution.parallel);
    assert!(
        parallel_execution.workers_used > 1,
        "large block snapshot must execute on multiple Rayon workers: {parallel_execution:?}"
    );
}

#[test]
fn unit_item_stack_never_serializes_a_null_item_with_contents() {
    assert_eq!(valid_item_stack(-1, 12), (0, 0));
    assert_eq!(valid_item_stack(22, 12), (0, 0));
    assert_eq!(valid_item_stack(0, 0), (0, 0));
    assert_eq!(valid_item_stack(0, 12), (0, 12));
    assert_eq!(valid_item_stack(1, 1), (1, 1));
}
use crate::network::codec::{Reads, Writes};

#[test]
fn generated_packet_ids_match_exact_desktop_159_registry() {
    assert_eq!(
        [
            CONNECT_CONFIRM_PACKET_ID,
            CLIENT_SNAPSHOT_PACKET_ID,
            COMMAND_BUILDING_PACKET_ID,
            COMMAND_UNITS_PACKET_ID,
            PING_PACKET_ID,
            PING_RESPONSE_PACKET_ID,
            PLAYER_DISCONNECT_PACKET_ID,
            PLAYER_SPAWN_PACKET_ID,
            SEND_CHAT_PACKET_ID,
            SEND_MESSAGE_PACKET_ID,
            SET_UNIT_COMMAND_PACKET_ID,
            SET_UNIT_STANCE_PACKET_ID,
            STATE_SNAPSHOT_PACKET_ID,
            ENTITY_SNAPSHOT_PACKET_ID,
            CREATE_BULLET_PACKET_ID,
            UNIT_DEATH_PACKET_ID,
            UNIT_DESPAWN_PACKET_ID,
            UNIT_CLEAR_PACKET_ID,
            UNIT_SPAWN_PACKET_ID,
            PAYLOAD_DROPPED_PACKET_ID,
            PICKED_BUILD_PAYLOAD_PACKET_ID,
            PICKED_UNIT_PAYLOAD_PACKET_ID,
            UNIT_ENTERED_PAYLOAD_PACKET_ID,
            CONSTRUCT_FINISH_PACKET_ID,
            BEGIN_PLACE_PACKET_ID,
            BEGIN_BREAK_PACKET_ID,
            BLOCK_SNAPSHOT_PACKET_ID,
            BUILD_DESTROYED_PACKET_ID,
            BUILD_HEALTH_UPDATE_PACKET_ID,
            DECONSTRUCT_FINISH_PACKET_ID,
            REMOVE_TILE_PACKET_ID,
            REQUEST_BLOCK_SNAPSHOT_PACKET_ID,
            REQUEST_ITEM_PACKET_ID,
            ROTATE_BLOCK_PACKET_ID,
            TAKE_ITEMS_PACKET_ID,
            TILE_CONFIG_PACKET_ID,
            TRANSFER_INVENTORY_PACKET_ID,
            TRANSFER_ITEM_TO_PACKET_ID,
            KICK_PACKET_ID,
            GAME_OVER_PACKET_ID,
            BUILDING_CONTROL_SELECT_PACKET_ID,
            CLIENT_PLAN_SNAPSHOT_PACKET_ID,
            CLIENT_PLAN_SNAPSHOT_RECEIVED_PACKET_ID,
            DELETE_PLANS_PACKET_ID,
            DROP_ITEM_PACKET_ID,
            PING_LOCATION_PACKET_ID,
            UNIT_CONTROL_PACKET_ID,
        ],
        [
            33, 28, 29, 30, 76, 78, 80, 81, 97, 98, 128, 129, 133, 48, 36, 151, 152, 149, 157, 73,
            74, 75, 154, 34, 12, 11, 13, 14, 15, 41, 84, 87, 91, 95, 135, 139, 142, 144, 60, 50,
            16, 26, 27, 42, 44, 77, 150,
        ]
    );
}

#[test]
fn rust_packet_ids_match_committed_159_7_packets_json() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../compat/159.7/packets.json")).unwrap();
    assert_eq!(doc["schema_version"], 2);
    let packets = doc["packets"].as_array().unwrap();
    assert_eq!(packets.len(), 165);
    let by_name: std::collections::HashMap<&str, i64> = packets
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), p["id"].as_i64().unwrap()))
        .collect();
    assert_eq!(
        by_name["ConnectConfirmCallPacket"],
        CONNECT_CONFIRM_PACKET_ID as i64
    );
    assert_eq!(by_name["RequestAssetsCallPacket"], 86);
    assert_eq!(by_name["RequestWorldCallPacket"], 93);
    assert_eq!(
        by_name["WorldDataBeginCallPacket"],
        WORLD_DATA_BEGIN_PACKET_ID as i64
    );
    assert_eq!(by_name["AssetRequirementStream"], 4);
    assert_eq!(by_name["AssetStream"], 5);
    assert_eq!(by_name["WorldStream"], 2);
}

#[test]
fn construct_finish_never_emits_an_unknown_typeio_object_tag() {
    // Regression: map Building tails were stored in DynamicTile::config.
    // Replaying a tail beginning with 127 made desktop 158.1 throw
    // "Unknown object type: 127" in ConstructFinishCallPacket.handled().
    let invalid =
        encode_construct_finish_for_unit(3_100_021, (41 << 16) | 100, 216, 3, 1, &[127, 1, 2, 3])
            .unwrap();
    assert_eq!(&invalid[13..], &[0]); // one null TypeIO object

    // Valid objects survive byte-for-byte. Internal simulation suffixes
    // are not part of the wire object and must be stripped.
    let with_suffix = encode_construct_finish_for_unit(
        3_100_021,
        (41 << 16) | 100,
        216,
        3,
        1,
        &[1, 0, 0, 0, 42, 0xff, 9],
    )
    .unwrap();
    assert_eq!(&with_suffix[13..], &[1, 0, 0, 0, 42]);

    let empty =
        encode_construct_finish_for_unit(3_100_021, (41 << 16) | 100, 216, 3, 1, &[]).unwrap();
    assert_eq!(&empty[13..], &[0]);
}

#[test]
fn construct_finish_wrapper_preserves_the_building_team() {
    let plan = BuildPlan {
        breaking: false,
        position: (41 << 16) | 100,
        block: 216,
        rotation: 3,
        config: vec![0],
    };
    let payload = encode_construct_finish(&player(), &plan, 3, 5).unwrap();
    assert_eq!(payload[12], 5, "ConstructFinish team byte");
}

#[test]
fn new_packet_decoders_match_exact_desktop_158_layouts() {
    // DropItem (42): f32 angle only.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&3.25f32.to_bits().to_be_bytes());
    assert!((decode_drop_item(&bytes).unwrap() - 3.25).abs() < 1e-6);
    let mut bad = bytes.clone();
    bad.push(0);
    assert!(decode_drop_item(&bad).is_err());

    // DeletePlans (40): s count + i pos[] (TypeIO.writeInts).
    let position_a: i32 = (45 << 16) | 100;
    let position_b: i32 = (46 << 16) | 101;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&2i16.to_be_bytes());
    bytes.extend_from_slice(&position_a.to_be_bytes());
    bytes.extend_from_slice(&position_b.to_be_bytes());
    assert_eq!(
        decode_delete_plans(&bytes).unwrap(),
        vec![position_a, position_b]
    );
    assert!(decode_delete_plans(&[0x00, 0xc9]).is_err()); // count 201

    // PingLocation (73): f f typeio_str.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1.5f32.to_bits().to_be_bytes());
    bytes.extend_from_slice(&(-2.5f32).to_bits().to_be_bytes());
    bytes.push(1);
    bytes.extend_from_slice(&3u16.to_be_bytes());
    bytes.extend_from_slice(b"hey");
    let (x, y, text) = decode_ping_location(&bytes).unwrap();
    assert!((x - 1.5).abs() < 1e-6 && (y + 2.5).abs() < 1e-6);
    assert_eq!(text.as_deref(), Some("hey"));

    // UnitControl (143): b type + i id (TypeIO.writeUnit).
    let mut bytes = Vec::new();
    bytes.push(2);
    bytes.extend_from_slice(&3_000_042i32.to_be_bytes());
    assert_eq!(decode_unit_control(&bytes).unwrap(), (2, 3_000_042));
    assert!(decode_unit_control(&[3, 0, 0, 0, 0]).is_err()); // type 3 invalid

    // BuildingControlSelect (14): i buildPos.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&position_a.to_be_bytes());
    assert_eq!(decode_building_control_select(&bytes).unwrap(), position_a);

    // ClientPlanSnapshot (24): i groupId + s count + per plan
    // (us x, us y, us blockId, [b rotation if rotate], TypeIO object).
    // Conveyor 257 rotates; its config is a plain integer object.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&7i32.to_be_bytes()); // groupId
    bytes.extend_from_slice(&1i16.to_be_bytes()); // amount
    bytes.extend_from_slice(&45u16.to_be_bytes()); // x
    bytes.extend_from_slice(&100u16.to_be_bytes()); // y
    bytes.extend_from_slice(&257u16.to_be_bytes()); // conveyor
    bytes.push(1); // rotation (rotatable block)
    bytes.extend_from_slice(&[1, 0, 0, 0, 42]); // config Integer 42
    let (group_id, plans_raw) = decode_client_plan_snapshot(&bytes).unwrap();
    assert_eq!(group_id, 7);
    assert_eq!(plans_raw.len(), bytes.len() - 4);
    // A non-rotatable block (router 264) must NOT carry the rotation byte.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&8i32.to_be_bytes());
    bytes.extend_from_slice(&1i16.to_be_bytes());
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&2u16.to_be_bytes());
    bytes.extend_from_slice(&264u16.to_be_bytes());
    bytes.extend_from_slice(&[0]); // null config
    let (group_id, _) = decode_client_plan_snapshot(&bytes).unwrap();
    assert_eq!(group_id, 8);
}

#[test]
fn new_packet_forwards_match_exact_desktop_158_layouts() {
    // Generated frames: u16 TCP length + packet ID byte + payload.
    // DeletePlans S2C: i player_id + s count + i pos[].
    let position: i32 = (45 << 16) | 100;
    let frame = encode_delete_plans_forward(1_000_001, &[position]).unwrap();
    assert_eq!(frame[2], DELETE_PLANS_PACKET_ID);
    // Wire format: u16 TCP len + id + u16 payload len + compress byte.
    let data = &frame[6..];
    assert_eq!(&data[0..4], &1_000_001i32.to_be_bytes());
    assert_eq!(&data[4..6], &1i16.to_be_bytes());
    assert_eq!(&data[6..10], &position.to_be_bytes());

    // PingLocation S2C: i player_id + f x + f y + typeio_str.
    let frame = encode_ping_location_forward(1_000_002, 4.0, 5.0, Some("hi")).unwrap();
    assert_eq!(frame[2], PING_LOCATION_PACKET_ID);
    // Wire format: u16 TCP len + id + u16 payload len + compress byte.
    let data = &frame[6..];
    assert_eq!(&data[0..4], &1_000_002i32.to_be_bytes());
    assert_eq!(&data[4..8], &4.0f32.to_bits().to_be_bytes());
    assert_eq!(&data[8..12], &5.0f32.to_bits().to_be_bytes());
    assert_eq!(data[12], 1); // string present
    assert_eq!(&data[13..15], &2u16.to_be_bytes());

    // UnitControl S2C: i player_id + b type + i id.
    let frame = encode_unit_control_forward(1_000_003, 2, 3_000_042).unwrap();
    assert_eq!(frame[2], UNIT_CONTROL_PACKET_ID);
    // Wire format: u16 TCP len + id + u16 payload len + compress byte.
    let data = &frame[6..];
    assert_eq!(&data[0..4], &1_000_003i32.to_be_bytes());
    assert_eq!(data[4], 2);
    assert_eq!(&data[5..9], &3_000_042i32.to_be_bytes());

    // ClientPlanSnapshotReceived S2C: i player_id + i groupId + plan bytes.
    let plans = [0u8, 1, 0, 45, 0, 100, 1, 0, 1, 0, 0, 0, 42];
    let frame = encode_client_plan_snapshot_received(1_000_005, 9, &plans).unwrap();
    assert_eq!(frame[2], CLIENT_PLAN_SNAPSHOT_RECEIVED_PACKET_ID);
    // Wire format: u16 TCP len + id + u16 payload len + compress byte.
    let data = &frame[6..];
    assert_eq!(&data[0..4], &1_000_005i32.to_be_bytes());
    assert_eq!(&data[4..8], &9i32.to_be_bytes());
    assert_eq!(&data[8..], plans);
}

#[test]
fn tile_config_uses_exact_typeio_object_and_server_forward_layout() {
    let item = [5, 0, 0, 3];
    assert!(valid_tile_config(264, &item));
    assert!(valid_tile_config(270, &[0]));
    assert!(!valid_tile_config(264, &[5, 1, 0, 3]));
    assert!(!valid_tile_config(264, &[5, 0, 0, 22]));
    assert!(valid_tile_config(262, &[7, 0, 0, 0, 4, 0, 0, 0, 0]));
    assert!(valid_tile_config(271, &[1, 0, 45, 0, 100]));
    assert_eq!(unit_factory_plan(377, &[5, 6, 0, 0]), Some((0, 0)));
    assert_eq!(unit_factory_plan(377, &[1, 0, 0, 0, 1]), Some((1, 10)));
    assert_eq!(
        unit_factory_plan(377, &[5, 6, 0, 0, FACTORY_COMMAND_MARKER, 0]),
        Some((0, 0))
    );
    assert_eq!(decode_unit_command_config(&[23, 0, 4]), Some(Some(4)));
    assert_eq!(decode_unit_command_config(&[0]), Some(None));
    assert_eq!(decode_unit_command_config(&[23, 0, 10]), None);
    assert!(valid_tile_config(377, &[23, 0, 0]));
    assert!(!valid_tile_config(377, &[5, 6, 0, 15]));
    assert!(!valid_tile_config(181, &item));

    let position: i32 = (45 << 16) | 100;
    let mut client_payload = position.to_be_bytes().to_vec();
    client_payload.extend_from_slice(&item);
    assert_eq!(
        decode_tile_config(&client_payload).unwrap(),
        Some((position, item.to_vec()))
    );
    client_payload.push(9);
    assert!(decode_tile_config(&client_payload).is_err());

    let player = SessionPlayer {
        id: 1_000_001,
        controlled_unit: ControlledUnit::Core,
        unit_id: 2_000_001,
        uuid: "AQIDBAUGBwg=".into(),
        name: "config".into(),
        color: 0,
        last_snapshot: 0,
        x: 360.0,
        y: 800.0,
        mouse_x: 360.0,
        mouse_y: 800.0,
        rotation: 0.0,
        boosting: false,
        shooting: false,
        last_command: None,
        active_plans: HashSet::new(),
        mining_position: None,
        mining_progress: 0.0,
        mining_updated: std::time::Instant::now(),
        carried_item: -1,
        carried_amount: 0,
        preview_plan_group: -1,
        preview_plans: Vec::new(),
        last_shot: std::time::Instant::now(),
        admin: false,
        chat_rate: crate::network::listener::ChatRateLimiter::new(),
    };
    let packet = encode_tile_config_frame(&player, position, &item).unwrap();
    let payload = read_packet(std::io::Cursor::new(&packet[2..])).unwrap();
    assert_eq!(payload[0], TILE_CONFIG_PACKET_ID);
    assert_eq!(&payload[1..5], &player.id.to_be_bytes());
    assert_eq!(&payload[5..9], &position.to_be_bytes());
    assert_eq!(&payload[9..], &item);
}

#[test]
fn placement_power_configs_are_broadcast_immediately_for_every_changed_node() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let connections = DashMap::new();
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("power".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    let actor_id = 1_000_001;
    let nodes = [(10 << 16) | 10, (20 << 16) | 20];
    let configs = [vec![8, 0], vec![8, 1, 0, 1, 0, 0]];
    let changes = building_placement::PlacementChanges {
        configured_power_links: false,
        auto_linked_power: true,
        power_node_configs: nodes.into_iter().zip(configs.iter().cloned()).collect(),
    };

    broadcast_placement_power_configs(&connections, actor_id, &changes).unwrap();

    for (position, config) in nodes.into_iter().zip(configs) {
        let frame = rx.try_recv().unwrap();
        let payload = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        assert_eq!(payload[0], TILE_CONFIG_PACKET_ID);
        assert_eq!(&payload[1..5], &actor_id.to_be_bytes());
        assert_eq!(&payload[5..9], &position.to_be_bytes());
        assert_eq!(&payload[9..], config);
    }
    assert!(rx.try_recv().is_err());
}

#[test]
fn rotate_block_uses_exact_client_and_server_layout() {
    let position: i32 = (45 << 16) | 100;
    let mut client_payload = position.to_be_bytes().to_vec();
    client_payload.push(1);
    assert_eq!(
        decode_rotate_block(&client_payload).unwrap(),
        (position, true)
    );
    client_payload[4] = 2;
    assert!(decode_rotate_block(&client_payload).is_err());
    client_payload[4] = 0;
    client_payload.push(0);
    assert!(decode_rotate_block(&client_payload).is_err());

    let player = SessionPlayer {
        id: 1_000_001,
        controlled_unit: ControlledUnit::Core,
        unit_id: 2_000_001,
        uuid: "AQIDBAUGBwg=".into(),
        name: "rotate".into(),
        color: 0,
        last_snapshot: 0,
        x: 360.0,
        y: 800.0,
        mouse_x: 360.0,
        mouse_y: 800.0,
        rotation: 0.0,
        boosting: false,
        shooting: false,
        last_command: None,
        active_plans: HashSet::new(),
        mining_position: None,
        mining_progress: 0.0,
        mining_updated: std::time::Instant::now(),
        carried_item: -1,
        carried_amount: 0,
        preview_plan_group: -1,
        preview_plans: Vec::new(),
        last_shot: std::time::Instant::now(),
        admin: false,
        chat_rate: crate::network::listener::ChatRateLimiter::new(),
    };
    let packet = encode_rotate_block_frame(&player, position, false).unwrap();
    let payload = read_packet(std::io::Cursor::new(&packet[2..])).unwrap();
    assert_eq!(payload[0], ROTATE_BLOCK_PACKET_ID);
    assert_eq!(&payload[1..5], &player.id.to_be_bytes());
    assert_eq!(&payload[5..9], &position.to_be_bytes());
    assert_eq!(payload[9], 0);
}

#[test]
fn command_building_uses_exact_client_and_server_layout() {
    let positions = [(45 << 16) | 100, (46 << 16) | 100];
    let mut client_payload = Vec::new();
    client_payload.write_s(2).unwrap();
    for position in positions {
        client_payload.write_i(position).unwrap();
    }
    client_payload.write_f(512.5).unwrap();
    client_payload.write_f(704.25).unwrap();
    assert_eq!(
        decode_command_building(&client_payload).unwrap(),
        (positions.to_vec(), 512.5, 704.25)
    );

    let player = player();
    let frame = encode_command_building_frame(&player, &positions, 512.5, 704.25).unwrap();
    let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    assert_eq!(packet[0], COMMAND_BUILDING_PACKET_ID);
    assert_eq!(&packet[1..5], &player.id.to_be_bytes());
    assert_eq!(&packet[5..], &client_payload);

    let mut invalid = client_payload.clone();
    let last = invalid.len() - 1;
    invalid[last] = 0x7f;
    invalid.extend_from_slice(&[0xc0, 0, 0]);
    assert!(decode_command_building(&invalid).is_err());
}

#[test]
fn command_units_uses_exact_client_and_server_layout() {
    let unit_ids = [3_000_001, 3_000_002];
    let mut client_payload = Vec::new();
    client_payload.write_s(2).unwrap();
    for id in unit_ids {
        client_payload.write_i(id).unwrap();
    }
    client_payload.write_i(-1).unwrap();
    client_payload.write_b(0).unwrap();
    client_payload.write_i(0).unwrap();
    client_payload.write_f(512.5).unwrap();
    client_payload.write_f(704.25).unwrap();
    client_payload.write_bool(true).unwrap();
    client_payload.write_bool(false).unwrap();
    assert_eq!(
        decode_command_units(&client_payload).unwrap(),
        CommandUnitsRequest {
            unit_ids: unit_ids.to_vec(),
            build_target: -1,
            unit_target_type: 0,
            unit_target_id: 0,
            pos_x: 512.5,
            pos_y: 704.25,
            queue_command: true,
            final_batch: false,
        }
    );

    let player = player();
    let frame = encode_command_units_frame(&player, &client_payload).unwrap();
    let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    assert_eq!(packet[0], COMMAND_UNITS_PACKET_ID);
    assert_eq!(&packet[1..5], &player.id.to_be_bytes());
    assert_eq!(&packet[5..], &client_payload);

    let mut invalid = client_payload;
    let queue_flag = invalid.len() - 2;
    invalid[queue_flag] = 2;
    assert!(decode_command_units(&invalid).is_err());
}

#[test]
fn set_unit_command_uses_exact_layout_and_official_allow_lists() {
    let unit_ids = [3_000_001, 3_000_002];
    let mut client_payload = Vec::new();
    client_payload.write_s(2).unwrap();
    for id in unit_ids {
        client_payload.write_i(id).unwrap();
    }
    client_payload.write_b(4).unwrap();
    assert_eq!(
        decode_set_unit_command(&client_payload).unwrap(),
        (unit_ids.to_vec(), 4)
    );

    let player = player();
    let frame = encode_set_unit_command_frame(&player, &client_payload).unwrap();
    let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    assert_eq!(packet[0], SET_UNIT_COMMAND_PACKET_ID);
    assert_eq!(&packet[1..5], &player.id.to_be_bytes());
    assert_eq!(&packet[5..], &client_payload);

    assert!(unit_allows_command(0, 0));
    assert!(unit_allows_command(0, 5));
    assert!(!unit_allows_command(0, 4));
    assert!(unit_allows_command(20, 4));
    assert!(unit_allows_command(21, 1));
    assert!(unit_allows_command(22, 9));
    assert!(!unit_allows_command(24, 1));

    let last = client_payload.len() - 1;
    client_payload[last] = 255;
    assert!(decode_set_unit_command(&client_payload).is_err());
}

#[test]
fn set_unit_stance_uses_exact_layout_and_official_compatibility() {
    let unit_ids = [3_000_001, 3_000_002];
    let mut client_payload = Vec::new();
    client_payload.write_s(2).unwrap();
    for id in unit_ids {
        client_payload.write_i(id).unwrap();
    }
    client_payload.write_b(1).unwrap();
    client_payload.write_bool(true).unwrap();
    assert_eq!(
        decode_set_unit_stance(&client_payload).unwrap(),
        (unit_ids.to_vec(), 1, true)
    );

    let player = player();
    let frame = encode_set_unit_stance_frame(&player, &client_payload).unwrap();
    let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    assert_eq!(packet[0], SET_UNIT_STANCE_PACKET_ID);
    assert_eq!(&packet[1..5], &player.id.to_be_bytes());
    assert_eq!(&packet[5..], &client_payload);

    assert!(unit_allows_stance(0, 0, 4));
    assert!(!unit_allows_stance(15, 0, 4));
    assert!(unit_allows_stance(5, 0, 5));
    assert!(unit_allows_stance(21, 1, 6));
    assert!(!unit_allows_stance(21, 1, 3));
    assert!(unit_allows_stance(20, 4, 7));
    assert!(unit_allows_stance(20, 4, 8));
    assert!(!unit_allows_stance(20, 4, 1));

    let last = client_payload.len() - 1;
    client_payload[last] = 2;
    assert!(decode_set_unit_stance(&client_payload).is_err());
}

#[test]
fn item_transfer_packets_match_exact_desktop_158_layouts() {
    let core: i32 = (40 << 16) | 100;
    let unit = 2_000_001;

    assert_eq!(
        decode_building_reference(&core.to_be_bytes(), "TransferInventory").unwrap(),
        core
    );
    assert!(decode_building_reference(
        &[core.to_be_bytes().as_slice(), &[0]].concat(),
        "TransferInventory"
    )
    .is_err());

    let mut request = core.to_be_bytes().to_vec();
    request.extend_from_slice(&0i16.to_be_bytes());
    request.extend_from_slice(&7i32.to_be_bytes());
    assert_eq!(decode_request_item(&request).unwrap(), (core, 0, 7));
    request[5] = 22;
    assert!(decode_request_item(&request).is_err());

    let take = encode_take_items_frame(core, 0, 7, unit).unwrap();
    let take = read_packet(std::io::Cursor::new(&take[2..])).unwrap();
    assert_eq!(take[0], TAKE_ITEMS_PACKET_ID);
    assert_eq!(&take[1..5], &core.to_be_bytes());
    assert_eq!(&take[5..7], &0i16.to_be_bytes());
    assert_eq!(&take[7..11], &7i32.to_be_bytes());
    assert_eq!(take[11], 2);
    assert_eq!(&take[12..16], &unit.to_be_bytes());

    let transfer = encode_transfer_item_to_frame(unit, 0, 7, 320.0, 800.0, core).unwrap();
    let transfer = read_packet(std::io::Cursor::new(&transfer[2..])).unwrap();
    assert_eq!(transfer[0], TRANSFER_ITEM_TO_PACKET_ID);
    assert_eq!(transfer[1], 2);
    assert_eq!(&transfer[2..6], &unit.to_be_bytes());
    assert_eq!(&transfer[6..8], &0i16.to_be_bytes());
    assert_eq!(&transfer[8..12], &7i32.to_be_bytes());
    assert_eq!(&transfer[12..16], &320.0f32.to_be_bytes());
    assert_eq!(&transfer[16..20], &800.0f32.to_be_bytes());
    assert_eq!(&transfer[20..24], &core.to_be_bytes());
}

#[test]
#[ignore = "developer fixture exporter for tools/VerifyProtocol158.java"]
fn export_desktop_158_post_join_fixtures() {
    use crate::engine::world_stream::personalize_desktop_158_with_state;

    let output = std::path::Path::new("target/protocol-158-fixtures");
    std::fs::create_dir_all(output).unwrap();
    let session = player();
    std::fs::write(
        output.join("entities.bin"),
        encode_initial_entity_snapshot(&session, None).unwrap(),
    )
    .unwrap();
    std::fs::write(output.join("state.bin"), encode_state_snapshot().unwrap()).unwrap();
    let construct_frame = encode_construct_block_snapshot((102 << 16) | 182, 270, 3, 1).unwrap();
    let construct_packet = read_packet(std::io::Cursor::new(&construct_frame[2..])).unwrap();
    std::fs::write(
        output.join("construct-snapshot.bin"),
        &construct_packet[1..],
    )
    .unwrap();
    let tile_position = (40_i32 << 16) | 100;
    let mut spawn = Vec::new();
    spawn.write_i(tile_position).unwrap();
    spawn.write_i(session.id).unwrap();
    std::fs::write(output.join("spawn.bin"), spawn).unwrap();
    std::fs::write(
        output.join("world.bin"),
        personalize_desktop_158_with_state(
            include_bytes!("../../dummy_world.dat"),
            session.id,
            &session.name,
            session.color,
            (session.x, session.y),
            1,
            60.0,
            0.0,
        )
        .unwrap(),
    )
    .unwrap();
    let mut unit_spawns = Vec::new();
    unit_spawns.write_s(35).unwrap();
    for unit_type in 0..35i16 {
        let spec = enemy_spec(unit_type).unwrap();
        let mut unit = EnemyUnit {
            id: 3_100_000 + i32::from(unit_type),
            unit_type,
            entity_class: spec.entity_class,
            team: 1,
            x: 320.0 + f32::from(unit_type),
            y: 800.0,
            rotation: 90.0,
            health: spec.health,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: if unit_type == 5 { 1.0 } else { 0.0 },
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: spec.speed,
            attack_damage: spec.attack_damage,
            attack_reload_time: spec.attack_reload,
            attack_range: spec.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        };
        if unit_type == 22 {
            unit.payloads.push(CarriedPayload::Unit(EnemyUnit {
                id: 3_200_000,
                unit_type: DAGGER.unit_type,
                entity_class: DAGGER.entity_class,
                team: 1,
                x: unit.x,
                y: unit.y,
                rotation: unit.rotation,
                health: DAGGER.health,
                shield: 0.0,
                status_effect: -1,
                status_duration: f32::MAX,
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
                move_speed: DAGGER.speed,
                attack_damage: DAGGER.attack_damage,
                attack_reload_time: DAGGER.attack_reload,
                attack_range: DAGGER.attack_range,
                authority: UnitAuthority::DefaultAi,
                build_plans: Vec::new(),
                update_building: true,
                status_agg: None,
            }));
            let wall_position = (42 << 16) | 100;
            let mut wall = base_building_tombstone(&BaseBuildingState {
                position: wall_position,
                block: 216,
                team: 1,
                health: crate::game::content::block_health(216),
                occupied: vec![wall_position],
                inventory: Vec::new(),
            });
            wall.block = 216;
            wall.team = 1;
            wall.health = crate::game::content::block_health(216);
            let mut sync = Vec::new();
            encode_simple_wall_sync(&mut sync, &wall).unwrap();
            unit.payloads
                .push(CarriedPayload::Build(CarriedBuildPayload {
                    tile: wall,
                    version: 0,
                    sync,
                }));
        }
        let mut payload = Vec::new();
        payload.write_i(unit.id).unwrap();
        payload.write_b(unit.entity_class).unwrap();
        let assist_plan = (unit_type == 21).then(|| BuildPlan {
            breaking: false,
            position: (41 << 16) | 100,
            block: 216,
            rotation: 3,
            config: vec![0],
        });
        write_unit_sync(
            &mut payload,
            None,
            &unit,
            unit.x,
            unit.y,
            None,
            assist_plan.as_ref(),
        )
        .unwrap();
        unit_spawns
            .write_s(i16::try_from(payload.len()).unwrap())
            .unwrap();
        unit_spawns.extend_from_slice(&payload);
    }
    std::fs::write(output.join("unit-spawns.bin"), unit_spawns).unwrap();
    let conveyor_position = (45 << 16) | 100;
    let mut conveyor = base_building_tombstone(&BaseBuildingState {
        position: conveyor_position,
        block: 398,
        team: 1,
        health: crate::game::content::block_health(398),
        occupied: vec![conveyor_position],
        inventory: Vec::new(),
    });
    conveyor.block = 398;
    conveyor.team = 1;
    conveyor.rotation = 0;
    conveyor.health = crate::game::content::block_health(398);
    conveyor.payload_progress = 22.5;
    conveyor.payload_rotation = 180.0;
    let wall_position = (46 << 16) | 100;
    let mut carried_wall = base_building_tombstone(&BaseBuildingState {
        position: wall_position,
        block: 216,
        team: 1,
        health: crate::game::content::block_health(216),
        occupied: vec![wall_position],
        inventory: Vec::new(),
    });
    carried_wall.block = 216;
    carried_wall.team = 1;
    carried_wall.health = crate::game::content::block_health(216);
    let mut wall_sync = Vec::new();
    encode_simple_wall_sync(&mut wall_sync, &carried_wall).unwrap();
    conveyor.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
        tile: carried_wall,
        version: 0,
        sync: wall_sync,
    })));
    let mut conveyor_sync = Vec::new();
    encode_payload_conveyor_sync(&mut conveyor_sync, &conveyor).unwrap();
    std::fs::write(output.join("payload-conveyor.bin"), conveyor_sync).unwrap();
    let driver_position = (50 << 16) | 100;
    let mut driver = base_building_tombstone(&BaseBuildingState {
        position: driver_position,
        block: 402,
        team: 1,
        health: crate::game::content::block_health(402),
        occupied: vec![driver_position],
        inventory: Vec::new(),
    });
    driver.block = 402;
    driver.team = 1;
    driver.health = crate::game::content::block_health(402);
    driver.payload_rotation = 135.0;
    driver.transport_progress = 45.0;
    driver.payload = conveyor.payload.clone();
    let mut driver_sync = Vec::new();
    encode_payload_mass_driver_sync(
        &mut driver_sync,
        &driver,
        &std::collections::HashMap::new(),
        None,
    )
    .unwrap();
    std::fs::write(output.join("payload-mass-driver.bin"), driver_sync).unwrap();
    let loader_position = (55 << 16) | 100;
    let mut loader = base_building_tombstone(&BaseBuildingState {
        position: loader_position,
        block: 408,
        team: 1,
        health: crate::game::content::block_health(408),
        occupied: vec![loader_position],
        inventory: Vec::new(),
    });
    loader.block = 408;
    loader.team = 1;
    loader.health = crate::game::content::block_health(408);
    loader.inventory = vec![(0, 12)];
    loader.stored_liquid = 0;
    loader.liquid_amount = 20.0;
    loader.production_progress = 1.0;
    let container_position = (56 << 16) | 100;
    let mut container = base_building_tombstone(&BaseBuildingState {
        position: container_position,
        block: 345,
        team: 1,
        health: crate::game::content::block_health(345),
        occupied: vec![container_position],
        inventory: Vec::new(),
    });
    container.block = 345;
    container.team = 1;
    container.health = crate::game::content::block_health(345);
    container.inventory = vec![(9, 7)];
    let mut container_sync = Vec::new();
    encode_storage_sync(&mut container_sync, &container, None).unwrap();
    loader.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
        tile: container,
        version: 0,
        sync: container_sync,
    })));
    let mut loader_sync = Vec::new();
    encode_payload_loader_sync(
        &mut loader_sync,
        &loader,
        &std::collections::HashMap::from([(loader_position, 1.0)]),
    )
    .unwrap();
    std::fs::write(output.join("payload-loader.bin"), loader_sync).unwrap();
    let deconstructor_position = (60 << 16) | 100;
    let mut deconstructor = base_building_tombstone(&BaseBuildingState {
        position: deconstructor_position,
        block: 404,
        team: 1,
        health: crate::game::content::block_health(404),
        occupied: vec![deconstructor_position],
        inventory: Vec::new(),
    });
    deconstructor.block = 404;
    deconstructor.team = 1;
    deconstructor.health = crate::game::content::block_health(404);
    deconstructor.inventory = vec![(0, 2)];
    deconstructor.payload_progress = 0.5;
    deconstructor.payload_rotation = 90.0;
    deconstructor.payload_accum = vec![0.5];
    deconstructor.payload = conveyor.payload.clone();
    let mut deconstructor_sync = Vec::new();
    encode_payload_deconstructor_sync(
        &mut deconstructor_sync,
        &deconstructor,
        &std::collections::HashMap::from([(deconstructor_position, 1.0)]),
    )
    .unwrap();
    std::fs::write(output.join("payload-deconstructor.bin"), deconstructor_sync).unwrap();
    let constructor_position = (65 << 16) | 100;
    let mut constructor = base_building_tombstone(&BaseBuildingState {
        position: constructor_position,
        block: 406,
        team: 1,
        health: crate::game::content::block_health(406),
        occupied: vec![constructor_position],
        inventory: Vec::new(),
    });
    constructor.block = 406;
    constructor.team = 1;
    constructor.rotation = 1;
    constructor.health = crate::game::content::block_health(406);
    constructor.config = vec![5, 1, 0, 236];
    constructor.inventory = vec![(16, 24)];
    constructor.production_progress = 72.0;
    constructor.payload_rotation = 90.0;
    let mut constructor_sync = Vec::new();
    encode_payload_constructor_sync(
        &mut constructor_sync,
        &constructor,
        &std::collections::HashMap::from([(constructor_position, 1.0)]),
    )
    .unwrap();
    std::fs::write(output.join("payload-constructor.bin"), constructor_sync).unwrap();
    let mut large_codecs = Vec::new();
    large_codecs
        .write_s(i16::try_from(NEW_LARGE_CODEC_RECIPES.len()).unwrap())
        .unwrap();
    for block in NEW_LARGE_CODEC_RECIPES {
        let position = ((70 + i32::from(*block)) << 16) | 100;
        let mut tile = base_building_tombstone(&BaseBuildingState {
            position,
            block: *block,
            team: 1,
            health: crate::game::content::block_health(*block),
            occupied: vec![position],
            inventory: Vec::new(),
        });
        tile.block = *block;
        tile.team = 1;
        tile.health = crate::game::content::block_health(*block);
        let mut sync = Vec::new();
        encode_dynamic_tile_sync(&mut sync, &tile, &std::collections::HashMap::new(), None)
            .unwrap();
        large_codecs.write_s(*block).unwrap();
        large_codecs.write_b(build_payload_version(*block)).unwrap();
        large_codecs
            .write_i(i32::try_from(sync.len()).unwrap())
            .unwrap();
        large_codecs.extend_from_slice(&sync);
    }
    std::fs::write(output.join("large-constructor-codecs.bin"), large_codecs).unwrap();
    let command_frame =
        encode_command_building_frame(&session, &[tile_position], 512.5, 704.25).unwrap();
    let command_packet = read_packet(std::io::Cursor::new(&command_frame[2..])).unwrap();
    std::fs::write(output.join("command-building.bin"), &command_packet[1..]).unwrap();
    let mut command_units_client = Vec::new();
    command_units_client.write_s(1).unwrap();
    command_units_client.write_i(3_100_000).unwrap();
    command_units_client.write_i(-1).unwrap();
    command_units_client.write_b(2).unwrap();
    command_units_client.write_i(3_100_001).unwrap();
    command_units_client.write_f(0.0).unwrap();
    command_units_client.write_f(0.0).unwrap();
    command_units_client.write_bool(false).unwrap();
    command_units_client.write_bool(true).unwrap();
    let command_units_frame = encode_command_units_frame(&session, &command_units_client).unwrap();
    let command_units_packet =
        read_packet(std::io::Cursor::new(&command_units_frame[2..])).unwrap();
    std::fs::write(output.join("command-units.bin"), &command_units_packet[1..]).unwrap();
    let mut set_command_client = Vec::new();
    set_command_client.write_s(1).unwrap();
    set_command_client.write_i(3_100_020).unwrap();
    set_command_client.write_b(4).unwrap();
    let set_command_frame = encode_set_unit_command_frame(&session, &set_command_client).unwrap();
    let set_command_packet = read_packet(std::io::Cursor::new(&set_command_frame[2..])).unwrap();
    std::fs::write(
        output.join("set-unit-command.bin"),
        &set_command_packet[1..],
    )
    .unwrap();
    let mut set_stance_client = Vec::new();
    set_stance_client.write_s(1).unwrap();
    set_stance_client.write_i(3_100_000).unwrap();
    set_stance_client.write_b(1).unwrap();
    set_stance_client.write_bool(true).unwrap();
    let set_stance_frame = encode_set_unit_stance_frame(&session, &set_stance_client).unwrap();
    let set_stance_packet = read_packet(std::io::Cursor::new(&set_stance_frame[2..])).unwrap();
    std::fs::write(output.join("set-unit-stance.bin"), &set_stance_packet[1..]).unwrap();
    std::fs::write(
        output.join("construct-finish-builder.bin"),
        encode_construct_finish_for_unit(3_100_021, (41 << 16) | 100, 216, 3, 1, &[0]).unwrap(),
    )
    .unwrap();
    std::fs::write(
        output.join("construct-finish-invalid-config.bin"),
        encode_construct_finish_for_unit(3_100_021, (41 << 16) | 101, 216, 3, 1, &[127, 1, 2, 3])
            .unwrap(),
    )
    .unwrap();
    let entered = encode_unit_entered_payload_frame(3_100_000, tile_position).unwrap();
    let entered_packet = read_packet(std::io::Cursor::new(&entered[2..])).unwrap();
    std::fs::write(
        output.join("unit-entered-payload.bin"),
        &entered_packet[1..],
    )
    .unwrap();
    let picked_build = encode_picked_build_payload_frame(3_100_022, tile_position, true).unwrap();
    let picked_build_packet = read_packet(std::io::Cursor::new(&picked_build[2..])).unwrap();
    std::fs::write(
        output.join("picked-build-payload.bin"),
        &picked_build_packet[1..],
    )
    .unwrap();

    // BufferedItemBridge (262) block snapshot fixture: exercises the exact
    // client path that crashed (ItemBuffer.accept with a corrupted index).
    // The payload mirrors BlockSnapshotCallPacket: s amount, s dataLen,
    // then per tile: i pos, s block, writeSync bytes.
    let bridge_position = (46 << 16) | 100;
    let mut bridge = base_building_tombstone(&BaseBuildingState {
        position: bridge_position,
        block: 262,
        team: 1,
        health: crate::game::content::block_health(262),
        occupied: vec![bridge_position],
        inventory: Vec::new(),
    });
    bridge.block = 262;
    bridge.team = 1;
    bridge.rotation = 1;
    bridge.stored_item = 0;
    bridge.stored_amount = 3;
    bridge.conveyor_items = vec![(0, 0.35), (1, 0.75)];
    let mut bridge_payload = Vec::new();
    bridge_payload.write_s(1).unwrap();
    let mut bridge_data = Vec::new();
    bridge_data.write_i(bridge_position).unwrap();
    bridge_data.write_s(262).unwrap();
    encode_item_bridge_sync(
        &mut bridge_data,
        &bridge,
        &std::collections::HashMap::new(),
        None,
    )
    .unwrap();
    bridge_payload
        .write_s(i16::try_from(bridge_data.len()).unwrap())
        .unwrap();
    bridge_payload.extend_from_slice(&bridge_data);
    std::fs::write(output.join("buffered-bridge-262.bin"), bridge_payload).unwrap();

    // Shield-projector (255) block snapshot fixture: verifies the
    // BaseShieldBuild tail (f smoothRadius + bool broken) is present so
    // the official client does not corrupt the snapshot batch.
    let shield_position = (48 << 16) | 100;
    let mut shield = base_building_tombstone(&BaseBuildingState {
        position: shield_position,
        block: 255,
        team: 1,
        health: crate::game::content::block_health(255),
        occupied: vec![shield_position],
        inventory: Vec::new(),
    });
    shield.block = 255;
    shield.team = 1;
    let mut shield_payload = Vec::new();
    shield_payload.write_s(1).unwrap();
    let mut shield_data = Vec::new();
    shield_data.write_i(shield_position).unwrap();
    shield_data.write_s(255).unwrap();
    encode_shield_sync(&mut shield_data, &shield, &std::collections::HashMap::new()).unwrap();
    shield_payload
        .write_s(i16::try_from(shield_data.len()).unwrap())
        .unwrap();
    shield_payload.extend_from_slice(&shield_data);
    std::fs::write(output.join("shield-projector-255.bin"), shield_payload).unwrap();

    // Reconstructor (380) fixture with an assist command configured, to
    // verify the official client decodes command/commandPos semantics.
    let recon_position = (50 << 16) | 100;
    let mut recon = base_building_tombstone(&BaseBuildingState {
        position: recon_position,
        block: 380,
        team: 1,
        health: crate::game::content::block_health(380),
        occupied: vec![recon_position],
        inventory: Vec::new(),
    });
    recon.block = 380;
    recon.team = 1;
    recon.rotation = 1;
    recon.health = crate::game::content::block_health(380);
    recon.config = vec![23, 0, 3]; // TypeIO UnitCommand assist (id 3)
    recon.inventory = vec![(1, 40), (9, 40)];
    recon.production_progress = 120.0;
    let mut recon_payload = Vec::new();
    recon_payload.write_s(1).unwrap();
    let mut recon_data = Vec::new();
    recon_data.write_i(recon_position).unwrap();
    recon_data.write_s(380).unwrap();
    encode_reconstructor_sync(
        &mut recon_data,
        &recon,
        &std::collections::HashMap::from([(recon_position, 1.0)]),
    )
    .unwrap();
    recon_payload
        .write_s(i16::try_from(recon_data.len()).unwrap())
        .unwrap();
    recon_payload.extend_from_slice(&recon_data);
    std::fs::write(output.join("reconstructor-380.bin"), recon_payload).unwrap();

    // Memory cell (434) fixture: full 64-cell array, values at 0 and 3.
    let mem_position = (52 << 16) | 100;
    let mut mem = base_building_tombstone(&BaseBuildingState {
        position: mem_position,
        block: 434,
        team: 1,
        health: crate::game::content::block_health(434),
        occupied: vec![mem_position],
        inventory: Vec::new(),
    });
    mem.block = 434;
    mem.team = 1;
    let mut memory = vec![0.0f64; 64];
    memory[0] = 1.5;
    memory[3] = -2.0;
    mem.memory = memory;
    let mut mem_payload = Vec::new();
    mem_payload.write_s(1).unwrap();
    let mut mem_data = Vec::new();
    mem_data.write_i(mem_position).unwrap();
    mem_data.write_s(434).unwrap();
    encode_memory_sync(&mut mem_data, &mem).unwrap();
    mem_payload
        .write_s(i16::try_from(mem_data.len()).unwrap())
        .unwrap();
    mem_payload.extend_from_slice(&mem_data);
    std::fs::write(output.join("memory-cell-434.bin"), mem_payload).unwrap();

    // Chat SendMessageCallPacket2 (92) fixture: frame payload after the
    // packet id (u16 len + id + u16 payload len + compress byte stripped
    // by read_packet). Verified against SendMessageCallPacket2.java.
    let chat_sender = player();
    let chat_frame = encode_chat_message2_frame(&chat_sender, "hello world").unwrap();
    let chat_packet = read_packet(std::io::Cursor::new(&chat_frame[2..])).unwrap();
    std::fs::write(
        output.join("chat-message2.bin"),
        &chat_packet[1..], // strip the packet id byte
    )
    .unwrap();
}

fn player() -> SessionPlayer {
    SessionPlayer {
        id: 1,
        controlled_unit: ControlledUnit::Core,
        unit_id: 2,
        uuid: "test-uuid".into(),
        name: "test".into(),
        color: 0,
        last_snapshot: -1,
        x: 0.0,
        y: 0.0,
        mouse_x: 0.0,
        mouse_y: 0.0,
        rotation: 0.0,
        boosting: false,
        shooting: false,
        last_command: None,
        active_plans: HashSet::new(),
        mining_position: None,
        mining_progress: 0.0,
        mining_updated: std::time::Instant::now(),
        carried_item: -1,
        carried_amount: 0,
        preview_plan_group: -1,
        preview_plans: Vec::new(),
        last_shot: std::time::Instant::now() - std::time::Duration::from_secs(1),
        admin: false,
        chat_rate: crate::network::listener::ChatRateLimiter::new(),
    }
}

#[test]
fn client_snapshot_is_decoded_and_movement_is_bounded() {
    let mut bytes = Vec::new();
    bytes.write_i(7).unwrap();
    bytes.write_i(2).unwrap();
    bytes.write_bool(false).unwrap();
    for value in [1000.0, 0.0, 12.0, 13.0, -30.0, 0.0, 0.0, 0.0] {
        bytes.write_f(value).unwrap();
    }
    bytes.write_i(-1).unwrap();
    bytes.write_bool(true).unwrap();
    bytes.write_bool(true).unwrap();
    bytes.write_bool(false).unwrap();
    bytes.write_bool(false).unwrap();
    bytes.write_s(-1).unwrap();
    bytes.write_i(0).unwrap();
    bytes.write_i(0).unwrap();
    for value in [0.0, 0.0, 100.0, 100.0] {
        bytes.write_f(value).unwrap();
    }

    let snapshot = decode_client_snapshot(&bytes).unwrap();
    let mut actor = player();
    // P1: non-strict accepts the (sanitized) client position; strict
    // limits the movement by elapsed time (official anticheat).
    apply_client_snapshot(&mut actor, &snapshot, false, 50);
    assert_eq!(actor.last_snapshot, 7);
    assert_eq!(actor.x, 1000.0, "non-strict accepts the client position");
    assert_eq!(actor.y, 0.0);
    assert_eq!(actor.mouse_x, 12.0);
    assert_eq!(actor.rotation, 330.0);
    // M3: alpha cannot boost — clientSnapshot forces boosting off in
    // both modes (bytecode offsets 214-249).
    assert!(!actor.boosting);
    assert!(actor.shooting);
    let mut strict_player = actor.clone();
    strict_player.x = 0.0;
    strict_player.y = 0.0;
    apply_client_snapshot(&mut strict_player, &snapshot, true, 50);
    // M3: official speed = unit.speed() = 3.0 world units/tick.
    let expected = 0.05 * 60.0 * 3.0 * 1.1;
    assert!(
        (strict_player.x - expected).abs() < 0.01,
        "strict caps the movement to {expected}: {}",
        strict_player.x
    );
}

#[test]
fn generic_crafter_block_snapshot_matches_official_layout() {
    use crate::network::codec::Reads;

    let position = (20 << 16) | 30;
    let tile = DynamicTile {
        position,
        block: 181,
        rotation: 2,
        team: 1,
        config: vec![0],
        enabled: true,
        message: None,
        occupied: vec![position],
        stored_item: -1,
        stored_amount: 0,
        production_progress: 45.0,
        transport_progress: 0.0,
        ammo_units: 0.0,
        inventory: vec![(5, 2)],
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
        health: f32::MAX,
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
    let frame = encode_factory_snapshot_tiles(
        std::slice::from_ref(&tile),
        &std::collections::HashMap::new(),
        None,
    )
    .unwrap();
    let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    assert_eq!(packet[0], BLOCK_SNAPSHOT_PACKET_ID);
    let mut input = std::io::Cursor::new(&packet[1..]);
    assert_eq!(input.read_s().unwrap(), 1);
    let data_length = input.read_s().unwrap() as usize;
    assert_eq!(data_length, packet.len() - 5);
    assert_eq!(input.read_i().unwrap(), position);
    assert_eq!(input.read_s().unwrap(), 181);
    assert_eq!(
        input.read_f().unwrap(),
        crate::game::content::block_health(tile.block)
    );
    assert_eq!(input.read_b().unwrap(), 130); // rotation 2 | new-format marker
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 3);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 1 | 8); // item module + 1<<3
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 5);
    assert_eq!(input.read_i().unwrap(), 2);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_f().unwrap(), 0.5);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.position() as usize, packet.len() - 1);

    let mut generator = DynamicTile {
        block: 308,
        stored_item: 15,
        production_progress: 342.0,
        inventory: vec![(5, 1)],
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    generator.position = (21 << 16) | 30;
    let mut sync = Vec::new();
    encode_power_generator_sync(&mut sync, &generator).unwrap();
    let mut input = std::io::Cursor::new(sync);
    assert_eq!(
        input.read_f().unwrap(),
        crate::game::content::block_health(generator.block)
    );
    assert_eq!(input.read_b().unwrap(), 130);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 3);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 11); // item + power + 1<<3
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 5);
    assert_eq!(input.read_i().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert!((input.read_f().unwrap() - 1.4).abs() < 0.0001);
    assert!((input.read_f().unwrap() - 0.95).abs() < 0.0001);

    let battery = DynamicTile {
        block: 306,
        power_stored: 2_000.0,
        power_links: Vec::new(),
        liquid_inventory: Vec::new(),
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut sync = Vec::new();
    encode_battery_sync(&mut sync, &battery).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 0.5);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let mender_position = (23 << 16) | 30;
    let mender = DynamicTile {
        position: mender_position,
        block: 245,
        transport_progress: 0.4,
        output_liquid_amount: 0.25,
        inventory: vec![(9, 2)],
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut power = std::collections::HashMap::new();
    power.insert(mender_position, 0.4);
    let mut sync = Vec::new();
    encode_mender_sync(&mut sync, &mender, &power).unwrap();
    let mut input = std::io::Cursor::new(sync);
    assert_eq!(
        input.read_f().unwrap(),
        crate::game::content::block_health(mender.block)
    );
    assert_eq!(input.read_b().unwrap(), 130);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 3);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 11); // item + power + 1<<3
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 9);
    assert_eq!(input.read_i().unwrap(), 2);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 0.4);
    assert_eq!(input.read_b().unwrap(), 102);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_f().unwrap(), 0.4);
    assert_eq!(input.read_f().unwrap(), 0.25);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let projector_position = (24 << 16) | 30;
    let projector = DynamicTile {
        position: projector_position,
        block: 247,
        transport_progress: 0.75,
        output_liquid_amount: 0.5,
        inventory: vec![(11, 1)],
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut power = std::collections::HashMap::new();
    power.insert(projector_position, 1.0);
    let mut sync = Vec::new();
    encode_overdrive_sync(&mut sync, &projector, &power).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(8);
    assert_eq!(input.read_b().unwrap(), 11); // items|power|1<<3
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 11);
    assert_eq!(input.read_i().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_f().unwrap(), 0.75);
    assert_eq!(input.read_f().unwrap(), 0.5);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let pump_position = (22 << 16) | 30;
    let pump = DynamicTile {
        position: pump_position,
        block: 284,
        stored_liquid: 0,
        liquid_amount: 12.5,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut power = std::collections::HashMap::new();
    power.insert(pump_position, 0.75);
    let mut sync = Vec::new();
    encode_liquid_block_sync(&mut sync, &pump, &power).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 0.75);
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 12.5);
    assert_eq!(input.read_b().unwrap(), 191);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let bridge_position = (24 << 16) | 30;
    let bridge = DynamicTile {
        position: bridge_position,
        block: 293,
        config: vec![7, 0, 0, 0, 3, 0, 0, 0, 0],
        enabled: true,
        message: None,
        stored_liquid: 0,
        liquid_amount: 8.0,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut sync = Vec::new();
    encode_liquid_bridge_sync(&mut sync, &bridge, &std::collections::HashMap::new(), None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 8.0);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_i().unwrap(), (27 << 16) | 30);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_b().unwrap(), 0);
    assert!(!input.read_bool().unwrap());
    assert_eq!(input.position() as usize, input.get_ref().len());

    let phase_position = (25 << 16) | 30;
    let phase_bridge = DynamicTile {
        position: phase_position,
        block: 263,
        config: vec![7, 0, 0, 0, 6, 0, 0, 0, 0],
        enabled: true,
        message: None,
        stored_item: 3,
        stored_amount: 2,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut power = std::collections::HashMap::new();
    power.insert(phase_position, 0.5);
    let mut sync = Vec::new();
    encode_item_bridge_sync(&mut sync, &phase_bridge, &power, None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 3);
    assert_eq!(input.read_i().unwrap(), 2);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 0.5);
    assert_eq!(input.read_b().unwrap(), 127);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_i().unwrap(), (31 << 16) | 30);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_b().unwrap(), 0);
    assert!(!input.read_bool().unwrap());
    assert_eq!(input.position() as usize, input.get_ref().len());

    let buffered_bridge = DynamicTile {
        block: 262,
        config: vec![0],
        enabled: true,
        message: None,
        stored_item: 0,
        stored_amount: 4,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut sync = Vec::new();
    encode_item_bridge_sync(
        &mut sync,
        &buffered_bridge,
        &std::collections::HashMap::new(),
        None,
    )
    .unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    // 158.1: block 262 is bridge-conveyor = BufferedItemBridge. Its write()
    // appends ItemBuffer (b index, b capacity=14, 14 longs) after the
    // ItemBridgeBuild fields (link, warmup, incoming, moved). Omitting it
    // makes the official client read the next block snapshot as its
    // buffer (ArrayIndexOutOfBounds in ItemBuffer.accept).
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_i().unwrap(), 4);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_i().unwrap(), -1);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_b().unwrap(), 0);
    assert!(!input.read_bool().unwrap());
    // ItemBuffer: one in-transit item (stored_item 0), capacity 14.
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 14);
    let first = input.read_l().unwrap() as u64;
    // time=0 (no world) << 32 | item 0 << 16 | data 0xffff (-1).
    assert_eq!(first & 0xFFFF, 0xFFFF);
    assert_eq!((first >> 16) & 0xFFFF, 0);
    for _ in 1..14 {
        assert_eq!(input.read_l().unwrap(), 0);
    }
    assert_eq!(input.position() as usize, input.get_ref().len());

    let junction = DynamicTile {
        block: 261,
        junction_items: vec![(0, 3, 13.0), (2, 0, 26.0)],
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut sync = Vec::new();
    encode_junction_sync(&mut sync, &junction, None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    for direction in 0..4 {
        let count = input.read_b().unwrap();
        assert_eq!(count, u8::from(matches!(direction, 0 | 2)));
        assert_eq!(input.read_b().unwrap(), 6);
        for slot in 0..6 {
            let packed = input.read_l().unwrap() as u64;
            if direction == 0 && slot == 0 {
                assert_eq!(packed as u16, 3);
                assert_eq!(f32::from_bits((packed >> 16) as u32), -13.0);
            } else if direction == 2 && slot == 0 {
                assert_eq!(packed as u16, 0);
                assert_eq!(f32::from_bits((packed >> 16) as u32), 0.0);
            } else {
                assert_eq!(packed, 0);
            }
        }
    }
    assert_eq!(input.position() as usize, input.get_ref().len());

    let duo = DynamicTile {
        block: 349,
        stored_item: 0,
        stored_amount: 3,
        ammo_units: 6.0,
        production_progress: 10.0,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut sync = Vec::new();
    encode_turret_sync(&mut sync, &duo, &std::collections::HashMap::new(), None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    // ItemTurretBuild in 158.1 carries ItemModule + LiquidModule (coolant).
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 0); // empty ItemModule
    assert_eq!(input.read_s().unwrap(), 0); // empty LiquidModule
    assert_eq!(input.read_b().unwrap(), 255); // efficiency 1 (has ammo)
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_f().unwrap(), 10.0);
    assert_eq!(input.read_f().unwrap(), 90.0);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_s().unwrap(), 6);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let lancer_position = (98 << 16) | 30;
    let lancer = DynamicTile {
        position: lancer_position,
        block: 354,
        production_progress: 20.0,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut power = std::collections::HashMap::new();
    power.insert(lancer_position, 0.75);
    let mut sync = Vec::new();
    encode_turret_sync(&mut sync, &lancer, &power, None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    // PowerTurretBuild in 158.1 carries PowerModule + LiquidModule (coolant).
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 0); // power links
    assert_eq!(input.read_f().unwrap(), 0.75);
    assert_eq!(input.read_s().unwrap(), 0); // empty LiquidModule
    assert_eq!(input.read_b().unwrap(), 191);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_f().unwrap(), 20.0);
    assert_eq!(input.read_f().unwrap(), 90.0);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let meltdown_position = (99 << 16) | 30;
    let meltdown = DynamicTile {
        position: meltdown_position,
        block: 366,
        production_progress: 60.0,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut power = std::collections::HashMap::new();
    power.insert(meltdown_position, 0.8);
    let mut sync = Vec::new();
    encode_turret_sync(&mut sync, &meltdown, &power, None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    // LaserTurretBuild in 158.1 carries PowerModule + LiquidModule (coolant).
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 0); // power links
    assert_eq!(input.read_f().unwrap(), 0.8);
    assert_eq!(input.read_s().unwrap(), 0); // empty LiquidModule
    assert_eq!(input.read_b().unwrap(), 204);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_f().unwrap(), 30.0);
    assert_eq!(input.read_f().unwrap(), 90.0);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let wave = DynamicTile {
        block: 353,
        stored_liquid: 0,
        liquid_amount: 7.5,
        production_progress: 2.0,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut sync = Vec::new();
    encode_turret_sync(&mut sync, &wave, &std::collections::HashMap::new(), None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 7.5);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_f().unwrap(), 2.0);
    assert_eq!(input.read_f().unwrap(), 90.0);
    assert_eq!(input.position() as usize, input.get_ref().len());

    let router = DynamicTile {
        block: 266,
        stored_item: 3,
        stored_amount: 1,
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile.clone()
    };
    let mut sync = Vec::new();
    encode_item_logistics_sync(&mut sync, &router).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9);
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 3);
    assert_eq!(input.read_i().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.position() as usize, input.get_ref().len());

    for block in [264, 265, 270] {
        let configured = DynamicTile {
            block,
            config: vec![5, 0, 0, 3],
            enabled: true,
            message: None,
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
            ..tile.clone()
        };
        let mut sync = Vec::new();
        encode_item_logistics_sync(&mut sync, &configured).unwrap();
        let mut input = std::io::Cursor::new(sync);
        input.set_position(9);
        if block == 270 {
            assert_eq!(input.read_s().unwrap(), 0);
        }
        assert_eq!(input.read_b().unwrap(), 255);
        assert_eq!(input.read_b().unwrap(), 255);
        assert_eq!(input.read_s().unwrap(), 3);
        assert_eq!(input.position() as usize, input.get_ref().len());
    }

    let driver_position = (30 << 16) | 30;
    let mut driver = DynamicTile {
        position: driver_position,
        block: 271,
        rotation: 0,
        config: vec![7, 0, 0, 0, 10, 0, 0, 0, 0],
        enabled: true,
        message: None,
        inventory: vec![(0, 12)],
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
        ..tile
    };
    driver.production_progress = 0.0;
    let mut power = std::collections::HashMap::new();
    power.insert(driver_position, 1.0);
    let mut sync = Vec::new();
    encode_mass_driver_sync(&mut sync, &driver, &power, None).unwrap();
    let mut input = std::io::Cursor::new(sync);
    input.set_position(9); // health, rotation/team, base revision/enabled/modules
    assert_eq!(input.read_s().unwrap(), 1);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_i().unwrap(), 12);
    assert_eq!(input.read_s().unwrap(), 0);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_b().unwrap(), 255);
    assert_eq!(input.read_i().unwrap(), (40 << 16) | 30);
    assert_eq!(input.read_f().unwrap(), 90.0);
    assert_eq!(input.read_b().unwrap(), 2);
}

#[test]
fn non_finite_client_snapshot_is_rejected() {
    let mut bytes = Vec::new();
    bytes.write_i(1).unwrap();
    bytes.write_i(2).unwrap();
    bytes.write_bool(false).unwrap();
    for value in [f32::NAN, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0] {
        bytes.write_f(value).unwrap();
    }
    bytes.write_i(-1).unwrap();
    bytes.write_bool(false).unwrap();
    bytes.write_bool(false).unwrap();
    bytes.write_bool(false).unwrap();
    bytes.write_bool(false).unwrap();
    bytes.write_s(-1).unwrap();
    bytes.write_i(0).unwrap();
    bytes.write_i(0).unwrap();
    for value in [0.0, 0.0, 100.0, 100.0] {
        bytes.write_f(value).unwrap();
    }
    assert!(decode_client_snapshot(&bytes).is_err());
}

#[test]
fn active_projectile_replay_uses_remaining_position_and_lifetime() {
    use crate::network::codec::Reads;

    let projectile = Projectile {
        target_id: 42,
        shooter_id: 0,
        team: 1,
        bullet_id: 113,
        damage: 9.0,
        splash_damage: 0.0,
        splash_radius: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        pierce_units: 0,
        pierce_buildings: 0,
        spawn_reign_frags: false,
        homing_range: 0.0,
        enemy_target_position: None,
        enemy_target_core: false,
        apply_direct_on_impact: false,
        armor_multiplier: 1.0,
        remaining_ticks: 25.0,
        total_ticks: 100.0,
        source_x: 0.0,
        source_y: 10.0,
        target_x: 100.0,
        target_y: 10.0,
        lifetime_scale: 1.0,
        source_position: None,
        damage_interval: None,
        damage_timer: 0.0,
    };
    let payload = encode_projectile_replay_payload(&projectile, Some((100.0, 10.0))).unwrap();
    let mut input = std::io::Cursor::new(payload);
    assert_eq!(input.read_s().unwrap(), 113);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_f().unwrap(), 75.0);
    assert_eq!(input.read_f().unwrap(), 10.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 9.0);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_f().unwrap(), 0.25);

    let continuous = Projectile {
        bullet_id: 162,
        remaining_ticks: 115.0,
        total_ticks: 230.0,
        lifetime_scale: 230.0 / 16.0,
        source_position: Some((10 << 16) | 10),
        damage_interval: Some(5.0),
        ..projectile
    };
    let payload = encode_projectile_replay_payload(&continuous, Some((100.0, 10.0))).unwrap();
    let mut input = std::io::Cursor::new(payload);
    assert_eq!(input.read_s().unwrap(), 162);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 10.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 9.0);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_f().unwrap(), 115.0 / 16.0);
}

#[test]
fn build_plan_with_null_config_is_decoded() {
    let mut bytes = Vec::new();
    bytes.write_i(2).unwrap();
    bytes.write_i(2).unwrap();
    bytes.write_bool(false).unwrap();
    for value in [320.0, 800.0, 320.0, 800.0, 0.0, 0.0, 0.0, 0.0] {
        bytes.write_f(value).unwrap();
    }
    bytes.write_i(-1).unwrap();
    for value in [false, false, false, true] {
        bytes.write_bool(value).unwrap();
    }
    bytes.write_s(-1).unwrap();
    bytes.write_i(0).unwrap();
    bytes.write_i(1).unwrap();
    bytes.write_b(0).unwrap(); // place
    bytes.write_i((41 << 16) | 100).unwrap();
    bytes.write_s(10).unwrap();
    bytes.write_b(2).unwrap();
    bytes.write_b(1).unwrap();
    bytes.write_b(0).unwrap(); // null config object
    for value in [0.0, 0.0, 100.0, 100.0] {
        bytes.write_f(value).unwrap();
    }

    let snapshot = decode_client_snapshot(&bytes).unwrap();
    assert_eq!(
        snapshot.plans,
        vec![BuildPlan {
            breaking: false,
            position: (41 << 16) | 100,
            block: 10,
            rotation: 2,
            config: vec![0],
        }]
    );
}

#[test]
fn dynamic_tiles_survive_a_save_and_load_cycle() {
    let path = std::env::temp_dir().join(format!(
        "mindustry-rs-test-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let tiles = DashMap::new();
    let position = (41 << 16) | 100;
    tiles.insert(
        position,
        DynamicTile {
            position,
            block: 261,
            rotation: 2,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![position],
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
            junction_items: vec![(0, 3, 12.5)],
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 90.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: crate::game::content::block_health(261) - 7.0,
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
        },
    );
    let rebuild_position = (42 << 16) | 100;
    let rebuild_source = DynamicTile {
        position: rebuild_position,
        block: 216,
        rotation: 3,
        team: 1,
        config: vec![0],
        enabled: true,
        message: None,
        occupied: vec![rebuild_position],
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
        health: crate::game::content::block_health(216),
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
    tiles.insert(
        rebuild_position,
        dynamic_building_tombstone(&rebuild_source),
    );
    let driver_position = (60 << 16) | 100;
    tiles.insert(
        driver_position,
        DynamicTile {
            position: driver_position,
            block: 271,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![driver_position],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 25.0,
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
            mass_driver_incoming: vec![((50 << 16) | 100, 0, 12, 9.5)],
            mass_driver_rotation: 123.0,
            mass_driver_waiting: vec![(50 << 16) | 100],
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: crate::game::content::block_health(271) - 11.0,
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
        },
    );

    let state = GameState::new();
    state.core_items.write()[0] = 4321;
    let enemies = DashMap::new();
    enemies.insert(
        3_000_123,
        EnemyUnit {
            id: 3_000_123,
            unit_type: 15,
            entity_class: FLARE.entity_class,
            team: 2,
            x: 320.0,
            y: 640.0,
            rotation: 90.0,
            health: 35.0,
            shield: 125.0,
            status_effect: 13,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 1.0,
            velocity_y: 0.0,
            elevation: 0.75,
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 12.0,
            secondary_attack_reload: 7.0,
            tertiary_attack_reload: 11.0,
            quaternary_attack_reload: 0.0,
            move_speed: FLARE.speed,
            attack_damage: FLARE.attack_damage,
            attack_reload_time: FLARE.attack_reload,
            attack_range: FLARE.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    let base_buildings = DashMap::new();
    let base_position = (80 << 16) | 100;
    base_buildings.insert(
        base_position,
        BaseBuildingState {
            position: base_position,
            block: 216,
            team: 1,
            health: 123.0,
            occupied: vec![base_position],
            inventory: Vec::new(),
        },
    );
    let player_profiles = DashMap::new();
    player_profiles.insert(
        "saved-player".into(),
        PlayerCombatState {
            uuid: "saved-player".into(),
            player_id: 100,
            unit_id: 200,
            x: 328.0,
            y: 800.0,
            health: 106.0,
            shield: 0.0,
            status_effect: 18,
            status_duration: 54.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    let building_commands = DashMap::new();
    building_commands.insert(
        driver_position,
        BuildingCommand {
            position: driver_position,
            target_x: 512.5,
            target_y: 704.25,
        },
    );
    let unit_orders = DashMap::new();
    unit_orders.insert(
        3_000_123,
        UnitOrder {
            unit_id: 3_000_123,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(512.5),
            target_y: Some(704.25),
            logic_control: 0,
            queue: Vec::new(),
        },
    );
    let team_plans = crate::engine::typeio::TeamBlocks::default();
    // Per-team cores are persisted too (revision 10).
    let cores = DashMap::new();
    cores.insert(
        5,
        TeamCore {
            position: (35 << 16) | 90,
            block: 339,
            health: 950.0,
            max_health: 1100.0,
        },
    );
    let logic_flags = DashMap::new();
    persist_tiles(
        &path,
        &tiles,
        &state,
        &enemies,
        &base_buildings,
        &player_profiles,
        &building_commands,
        &unit_orders,
        &team_plans,
        &cores,
        &logic_flags,
        &crate::network::buildings::puddles::PuddleSystem::new(),
    )
    .unwrap();
    let restored = load_tiles(&path, None).unwrap();
    assert_eq!(restored.map_name.as_deref(), Some("maze"));
    assert_eq!(restored.team_cores.len(), 1);
    assert_eq!(restored.team_cores[0].team, 5);
    assert_eq!(restored.team_cores[0].position, (35 << 16) | 90);
    assert_eq!(restored.team_cores[0].health, 950.0);
    assert_eq!(restored.team_cores[0].max_health, 1100.0);
    assert_eq!(restored.players.len(), 1);
    assert_eq!(restored.players[0].uuid, "saved-player");
    assert_eq!(restored.players[0].health, 106.0);
    assert_eq!(restored.players[0].status_effect, 18);
    let tile = restored.tiles.get(&position).unwrap();
    assert_eq!(tile.block, 261);
    assert_eq!(tile.rotation, 2);
    assert_eq!(tile.config, vec![0]);
    assert_eq!(tile.junction_items, vec![(0, 3, 12.5)]);
    assert_eq!(tile.health, crate::game::content::block_health(261) - 7.0);
    let rebuild = restored.tiles.get(&rebuild_position).unwrap();
    assert_eq!(rebuild.block, 0);
    assert_eq!(rebuild.stored_amount, 217);
    assert_eq!(rebuild.rotation, 3);
    assert_eq!(rebuild.team, 1);
    assert_eq!(rebuild_plan(&rebuild), Some((216, 3, 1, vec![0])));
    let driver = restored.tiles.get(&driver_position).unwrap();
    assert_eq!(
        driver.mass_driver_incoming,
        vec![((50 << 16) | 100, 0, 12, 9.5)]
    );
    assert_eq!(driver.mass_driver_rotation, 123.0);
    assert_eq!(driver.mass_driver_waiting, vec![(50 << 16) | 100]);
    assert_eq!(
        driver.health,
        crate::game::content::block_health(271) - 11.0
    );
    assert_eq!(restored.core_items.unwrap()[0], 4321);
    let enemy = restored
        .enemies
        .iter()
        .find(|enemy| enemy.id == 3_000_123)
        .unwrap();
    assert_eq!(enemy.entity_class, FLARE.entity_class);
    assert_eq!(enemy.health, 35.0);
    assert_eq!(enemy.shield, 125.0);
    assert_eq!(enemy.status_effect, 13);
    assert_eq!(enemy.status_duration, f32::MAX);
    assert_eq!(enemy.secondary_attack_reload, 7.0);
    assert_eq!(enemy.tertiary_attack_reload, 11.0);
    assert_eq!(enemy.elevation, 0.75);
    assert_eq!(enemy.move_speed, FLARE.speed * 1.15);
    assert_eq!(restored.base_building_health.len(), 1);
    assert_eq!(restored.base_building_health[0].position, base_position);
    assert_eq!(restored.base_building_health[0].health, 123.0);
    assert_eq!(restored.building_commands.len(), 1);
    assert_eq!(restored.building_commands[0].target_x, 512.5);
    assert_eq!(restored.unit_orders.len(), 1);
    assert_eq!(restored.unit_orders[0].command, 0);
    assert_eq!(restored.unit_orders[0].target_y, Some(704.25));
    drop(tile);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn named_save_slots_cannot_escape_the_save_directory() {
    let base = Path::new("saves/world-delta.json");
    assert_eq!(
        save_slot_path(base, "campaign_01").unwrap(),
        PathBuf::from("saves/world-delta-campaign_01.json")
    );
    assert!(save_slot_path(base, "../outside").is_err());
    assert!(save_slot_path(base, "slot/child").is_err());
    assert!(save_slot_path(base, "").is_err());
}

#[test]
fn official_costs_are_consumed_and_half_refunded() {
    let state = GameState::new();
    *state.core_items.write() = vec![0; 20];
    state.core_items.write()[0] = 2;
    assert!(consume_requirements(&state, 1, 257)); // conveyor: 1 copper
    assert_eq!(state.core_items.read()[0], 1);
    refund_requirements(&state, 1, 257);
    assert_eq!(state.core_items.read()[0], 2);
    assert!(!consume_requirements(&state, 1, 349)); // duo: 35 copper
}

#[test]
fn infinite_resources_skips_build_cost() {
    let state = GameState::new();
    *state.core_items.write() = vec![0; 20];
    state.core_items.write()[0] = 0; // no copper at all
    assert!(!consume_requirements(&state, 1, 257)); // conveyor: 1 copper
                                                    // With infiniteResources on, building costs nothing (official
                                                    // ConstructBlock: `progress >= 1f || state.rules.infiniteResources`).
    state.infinite_resources.store(true, Ordering::Relaxed);
    assert!(consume_requirements(&state, 1, 257));
    assert_eq!(state.core_items.read()[0], 0, "no items consumed");
    refund_requirements(&state, 1, 257);
    assert_eq!(state.core_items.read()[0], 0, "no refund minted");
}

#[test]
fn cave_survival_hosted_as_sandbox_overrides_map_rules() {
    // Survival-like map rules, then the explicit server mode is applied
    // AFTER map rules just like Gamemode.apply in the official server.
    // Prefer the user-reported cavesurvival.msav when present (gitignored);
    // otherwise a local official-map extract if the operator has one.
    let cave = Path::new(env!("CARGO_MANIFEST_DIR")).join("cavesurvival.msav");
    let msav = if cave.is_file() {
        std::fs::read(&cave).unwrap()
    } else {
        let Some(bytes) = official_msav("groundZero.msav") else {
            return;
        };
        bytes
    };
    let template = crate::engine::world_stream::replace_map_from_msav(
        include_bytes!("../../dummy_world.dat"),
        &msav,
    )
    .unwrap();
    let map_metadata = crate::engine::world_stream::inspect_metadata(&template).unwrap();
    let map_rules = crate::network::units::parse_wave_rules(&map_metadata.rules);
    assert!(
        !map_rules.infinite_resources,
        "fixture must retain the conflicting Survival rule"
    );
    let state = GameState::new();
    state.start_hosting("cavesurvival".into(), GameMode::Sandbox);
    let world = fresh_world_from_template(
        &state,
        template,
        "cavesurvival".into(),
        std::env::temp_dir().join("cavesurvival-sandbox-rules-test.json"),
    )
    .unwrap();
    assert!(world.wave_rules.read().infinite_resources);
    assert!(world.wave_rules.read().waves_enabled);
    assert!(!world.wave_rules.read().wave_timer);
    assert!(state.infinite_resources.load(Ordering::Relaxed));
}

#[test]
fn sandbox_build_and_break_are_immediate_and_resource_free() {
    let (world, connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Sandbox;
    apply_game_mode_to_wave_rules(&mut world.wave_rules.write(), GameMode::Sandbox);
    world
        .game_state
        .infinite_resources
        .store(true, Ordering::Relaxed);
    *world.game_state.core_items.write() = vec![0; 22];

    let position = (44 << 16) | 104;
    let pending = PendingBuild {
        position,
        block: 257,
        rotation: 0,
        config: vec![0],
        occupied: vec![position],
        team: 1,
        builder: player(),
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: f32::MAX,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(position, pending.clone());
    schedule_build(&world, &pending);
    assert_eq!(
        world.pending_builds.get(&position).unwrap().remaining_ticks,
        0.0
    );
    simulate_constructions(&world, &connections, 0.0);
    assert_eq!(world.tiles.get(&position).unwrap().block, 257);
    assert_eq!(
        world.game_state.core_items.read()[0],
        0,
        "build costs nothing"
    );

    let pending = PendingBreak {
        position,
        block: 257,
        occupied: vec![position],
        dynamic: true,
        team: 1,
        builder: player(),
        last_seen: std::time::Instant::now(),
        remaining_ticks: f32::MAX,
    };
    world.pending_breaks.insert(position, pending.clone());
    schedule_break(&world, &pending);
    assert_eq!(
        world.pending_breaks.get(&position).unwrap().remaining_ticks,
        0.0
    );
    simulate_breaks(&world, &connections, 0.0);
    assert!(
        world.tiles.get(&position).is_none(),
        "deconstruction finishes immediately"
    );
    assert_eq!(
        world.game_state.core_items.read()[0],
        0,
        "sandbox deconstruction does not create a refund"
    );
}

#[test]
fn block_health_multiplier_reduces_building_damage() {
    // Official Building.damage divides incoming damage by
    // Rules.blockHealthMultiplier; a multiplier of 2 makes buildings
    // take half damage.
    let (world, _, _, _) = legacy_weapons_test_world();
    let world = world;
    let pos = (40 << 16) | 100;
    let block = 341; // a buildable block with a known health
    let occupied = block_footprint_in(300, 300, pos, block).unwrap();
    world.tiles.insert(
        pos,
        DynamicTile {
            position: pos,
            block,
            team: 1,
            health: 1000.0,
            occupied,
            enabled: true,
            config: Vec::new(),
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
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            rotation: 0,
            message: None,
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
        },
    );
    // Default multiplier 1: 100 damage -> 900.
    let (destroyed, health) = damage_building(&world, pos, 100.0).unwrap();
    assert!(!destroyed);
    assert!((health - 900.0).abs() < 0.01);
    // Multiplier 2: 100 damage -> 950 (half).
    world.tiles.get_mut(&pos).unwrap().health = 1000.0;
    world.wave_rules.write().block_health_multiplier = 2.0;
    let (destroyed, health) = damage_building(&world, pos, 100.0).unwrap();
    assert!(!destroyed);
    assert!((health - 950.0).abs() < 0.01);
}

#[test]
fn multiblock_footprint_uses_official_size_and_offset() {
    let origin = (40 << 16) | 100;
    let footprint = block_footprint_in(300, 300, origin, 341).unwrap();
    assert_eq!(footprint.len(), 25); // core nucleus is 5x5
    assert!(footprint.contains(&((38 << 16) | 98)));
    assert!(footprint.contains(&((42 << 16) | 102)));
}

#[test]
fn initial_wave_composition_matches_bundled_rules() {
    let wave_one = initial_official_wave_groups(0);
    assert_eq!(wave_one.len(), 1);
    assert_eq!(wave_one[0].spec.unit_type, 0);
    assert_eq!(wave_one[0].amount, 1);

    let wave_eight = initial_official_wave_groups(7);
    assert!(wave_eight
        .iter()
        .any(|group| group.spec.unit_type == 0 && group.amount == 4));
    assert!(wave_eight
        .iter()
        .any(|group| group.spec.unit_type == 10 && group.amount == 4));
    assert!(wave_eight
        .iter()
        .any(|group| group.spec.unit_type == 1 && group.amount == 1));

    let wave_fifty_one = initial_official_wave_groups(50);
    assert!(wave_fifty_one
        .iter()
        .any(|group| { group.spec.unit_type == FLARE.unit_type && group.shield >= 100.0 }));
    let wave_one_thirty_two = initial_official_wave_groups(131);
    assert!(wave_one_thirty_two
        .iter()
        .any(|group| group.spec.unit_type == ANTUMBRA.unit_type));
    assert!(initial_official_wave_groups(45)
        .iter()
        .any(|group| group.spec.unit_type == SPIROCT.unit_type && group.status_effect == 13));
    assert!(initial_official_wave_groups(41)
        .iter()
        .any(|group| group.spec.unit_type == PULSAR.unit_type && group.status_effect == 15));
}

#[test]
fn reconstructor_upgrade_chains_and_desktop_mount_counts_are_complete() {
    for chain in [
        [0, 1, 2, 3, 4],
        [5, 6, 7, 8, 9],
        [10, 11, 12, 13, 14],
        [15, 16, 17, 18, 19],
        [20, 21, 22, 23, 24],
        [25, 26, 27, 28, 29],
        [30, 31, 32, 33, 34],
    ] {
        for (tier, pair) in chain.windows(2).enumerate() {
            assert_eq!(
                reconstructor_upgrade(380 + tier as i16, pair[0]),
                Some(pair[1])
            );
            assert!(enemy_spec(pair[1]).is_some());
        }
    }
    // Campaign mechs have specs so late Serpulo spawn groups (gamma
    // waves) are not skipped by parse_spawn_group (SOL-009).
    assert_eq!(enemy_spec(35).unwrap().health, 150.0);
    assert_eq!(enemy_spec(36).unwrap().health, 170.0);
    assert_eq!(enemy_spec(37).unwrap().health, 220.0);
    assert_eq!(enemy_weapon_mount_count(20), 0);
    assert_eq!(enemy_weapon_mount_count(24), 0);
    assert_eq!(enemy_weapon_mount_count(3), 3);
    assert_eq!(enemy_weapon_mount_count(13), 4);
    assert_eq!(enemy_weapon_mount_count(34), 5);
    assert_eq!(reconstructor_recipe(380).unwrap().build_time, 600.0);
    assert_eq!(reconstructor_recipe(383).unwrap().liquid_rate, 3.0);
}

#[test]
fn official_sharded_unit_cap_uses_rules_and_core_modifier() {
    use crate::engine::world_stream::NetworkBuilding;

    let core = NetworkBuilding {
        position: 0,
        block: 339,
        health: 1_100.0,
        rotation: 0,
        team: 1,
        inventory: Vec::new(),
        power_links: Vec::new(),
        power_status: 0.0,
        liquids: Vec::new(),
        enabled: true,
        extra_data: Vec::new(),
    };
    assert_eq!(sharded_unit_cap("{}", std::slice::from_ref(&core)), 8);
    assert_eq!(
        sharded_unit_cap(
            r#"{"unitCap":12,"unitCapVariable":true}"#,
            std::slice::from_ref(&core)
        ),
        20
    );
    assert_eq!(
        sharded_unit_cap(
            r#"{"unitCap":12,"unitCapVariable":false}"#,
            std::slice::from_ref(&core)
        ),
        12
    );
    assert_eq!(
        sharded_unit_cap(r#"{"disableUnitCap":true}"#, std::slice::from_ref(&core)),
        i32::MAX
    );
}

#[test]
fn unit_factory_and_reconstructor_produce_persisted_sharded_units() {
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("unit-factory-test".into(), GameMode::Sandbox);
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
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
        tiles: DashMap::new(),
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::new(),
        mono_mining_targets: DashMap::new(),
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-unit-factory-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let position = (i32::from(SPAWN_X) << 16) | (i32::from(SPAWN_Y) + 10);
    world.tiles.insert(
        position,
        DynamicTile {
            position,
            block: 377,
            rotation: 0,
            team: 1,
            config: vec![5, 6, 0, 0],
            enabled: true,
            message: None,
            occupied: vec![position],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: vec![(9, 10), (1, 10)],
            power_stored: 0.0,
            power_links: Vec::new(),
            liquid_inventory: Vec::new(),
            stored_liquid: -1,
            liquid_amount: 0.0,
            output_liquid_amount: 0.0,
            junction_items: Vec::new(),
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 0.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: f32::MAX,
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
        },
    );
    let mut power = std::collections::HashMap::new();
    power.insert(position, 1.0);
    assert!(apply_command_building(&world, &[position], 500.0, 880.0));
    assert!(simulate_unit_factories(
        &world,
        &DashMap::new(),
        900.0,
        &power
    ));
    let factory = world.tiles.get(&position).unwrap();
    assert_eq!(inventory_count(&factory.inventory, 9), 0);
    assert_eq!(inventory_count(&factory.inventory, 1), 0);
    drop(factory);
    let mut ally = world.enemies.get(&3_000_000).unwrap().clone();
    assert_eq!(ally.team, 1);
    assert_eq!(ally.unit_type, DAGGER.unit_type);
    assert_eq!(hostile_unit_count(&world), 0);
    let order = world.unit_orders.get(&ally.id).unwrap();
    assert_eq!(order.command, 0);
    assert_eq!((order.target_x, order.target_y), (Some(500.0), Some(880.0)));
    drop(order);
    assert!(simulate_allied_units(&world, &DashMap::new(), 1.0));
    let moved = world.enemies.get(&ally.id).unwrap().clone();
    assert_ne!((moved.x, moved.y), (ally.x, ally.y));
    let first_order_x = moved.x + 16.0;
    let second_order_x = moved.x + 32.0;
    let order_y = moved.y;
    assert!(apply_command_units(
        &world,
        &CommandUnitsRequest {
            unit_ids: vec![ally.id],
            build_target: -1,
            unit_target_type: 0,
            unit_target_id: 0,
            pos_x: first_order_x,
            pos_y: order_y,
            queue_command: false,
            final_batch: true,
        }
    ));
    assert!(apply_command_units(
        &world,
        &CommandUnitsRequest {
            unit_ids: vec![ally.id],
            build_target: -1,
            unit_target_type: 0,
            unit_target_id: 0,
            pos_x: second_order_x,
            pos_y: order_y,
            queue_command: true,
            final_batch: true,
        }
    ));
    assert_eq!(world.unit_orders.get(&ally.id).unwrap().queue.len(), 1);
    assert!(apply_set_unit_stance(&world, &[ally.id], 3, true));
    assert!(simulate_allied_units(&world, &DashMap::new(), 1_000.0));
    for _ in 0..100 {
        assert!(simulate_allied_units(&world, &DashMap::new(), 1.0));
        if world.unit_orders.get(&ally.id).unwrap().target_x == Some(second_order_x) {
            break;
        }
    }
    let order = world.unit_orders.get(&ally.id).unwrap();
    assert_eq!(order.target_x, Some(second_order_x));
    assert_eq!(order.queue.len(), 1);
    assert_eq!(order.queue[0].x, first_order_x);
    drop(order);
    assert!(apply_set_unit_stance(&world, &[ally.id], 3, false));
    ally = world.enemies.get(&ally.id).unwrap().clone();

    world.enemies.insert(
        3_000_001,
        EnemyUnit {
            id: 3_000_001,
            unit_type: DAGGER.unit_type,
            entity_class: DAGGER.entity_class,
            team: 2,
            x: ally.x + 100.0,
            y: ally.y,
            rotation: 180.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    assert!(apply_command_units(
        &world,
        &CommandUnitsRequest {
            unit_ids: vec![ally.id],
            build_target: -1,
            unit_target_type: 2,
            unit_target_id: 3_000_001,
            pos_x: 0.0,
            pos_y: 0.0,
            queue_command: false,
            final_batch: true,
        }
    ));
    assert!(apply_set_unit_stance(&world, &[ally.id], 1, true));
    assert!(simulate_allied_units(&world, &DashMap::new(), 13.0));
    assert_eq!(world.enemies.get(&3_000_001).unwrap().health, 150.0);
    assert!(apply_set_unit_stance(&world, &[ally.id], 1, false));
    let combat_connections = DashMap::new();
    assert!(simulate_allied_units(&world, &combat_connections, 13.0));
    assert_eq!(world.enemies.get(&3_000_001).unwrap().health, 150.0);
    assert_eq!(world.projectiles.iter().next().unwrap().bullet_id, 6);
    assert!(simulate_projectiles(&world, &combat_connections, 100.0));
    assert_eq!(world.enemies.get(&3_000_001).unwrap().health, 141.0);
    {
        let mut target = world.enemies.get_mut(&3_000_001).unwrap();
        target.x = ally.x + ally.attack_range / 2.0;
        target.y = ally.y;
    }
    assert!(apply_set_unit_stance(&world, &[ally.id], 4, true));
    let ram_x = world.enemies.get(&ally.id).unwrap().x;
    let ram_target_health = world.enemies.get(&3_000_001).unwrap().health;
    assert!(simulate_allied_units(&world, &combat_connections, 13.0));
    assert!(world.enemies.get(&ally.id).unwrap().x > ram_x);
    assert!(simulate_projectiles(&world, &combat_connections, 100.0));
    assert!(world.enemies.get(&3_000_001).unwrap().health < ram_target_health);
    assert!(apply_set_unit_stance(&world, &[ally.id], 4, false));
    assert_eq!(hostile_unit_count(&world), 1);

    world.enemies.remove(&3_000_001);
    let attack_x = (ally.x / 8.0).round() as i32 + 5;
    let attack_y = (ally.y / 8.0).round() as i32;
    let enemy_wall_position = (attack_x << 16) | attack_y;
    let mut enemy_wall = base_building_tombstone(&BaseBuildingState {
        position: enemy_wall_position,
        block: 216,
        team: 2,
        health: 320.0,
        occupied: vec![enemy_wall_position],
        inventory: Vec::new(),
    });
    enemy_wall.block = 216;
    enemy_wall.team = 2;
    enemy_wall.health = 320.0;
    world.tiles.insert(enemy_wall_position, enemy_wall);
    assert!(apply_command_units(
        &world,
        &CommandUnitsRequest {
            unit_ids: vec![ally.id],
            build_target: enemy_wall_position,
            unit_target_type: 0,
            unit_target_id: 0,
            pos_x: 0.0,
            pos_y: 0.0,
            queue_command: false,
            final_batch: true,
        }
    ));
    assert!(simulate_allied_units(&world, &combat_connections, 13.0));
    assert_eq!(world.tiles.get(&enemy_wall_position).unwrap().health, 320.0);
    assert_eq!(world.projectiles.iter().next().unwrap().bullet_id, 6);
    assert!(simulate_projectiles(&world, &combat_connections, 100.0));
    assert_eq!(world.tiles.get(&enemy_wall_position).unwrap().health, 311.0);
    world.tiles.remove(&enemy_wall_position);
    let reconstructor_position = ((i32::from(SPAWN_X) + 3) << 16) | (i32::from(SPAWN_Y) + 10);
    {
        let mut unit = world.enemies.get_mut(&ally.id).unwrap();
        unit.x = f32::from((reconstructor_position >> 16) as i16) * 8.0;
        unit.y = f32::from(reconstructor_position as i16) * 8.0;
    }
    world.tiles.insert(
        reconstructor_position,
        DynamicTile {
            position: reconstructor_position,
            block: 380,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![reconstructor_position],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: vec![(9, 40), (4, 40)],
            power_stored: 0.0,
            power_links: Vec::new(),
            liquid_inventory: Vec::new(),
            stored_liquid: -1,
            liquid_amount: 0.0,
            output_liquid_amount: 0.0,
            junction_items: Vec::new(),
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 0.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: f32::MAX,
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
        },
    );
    power.insert(reconstructor_position, 1.0);
    assert!(apply_set_unit_command(&world, &[ally.id], 5));
    assert!(apply_command_units(
        &world,
        &CommandUnitsRequest {
            unit_ids: vec![ally.id],
            build_target: reconstructor_position,
            unit_target_type: 0,
            unit_target_id: 0,
            pos_x: 0.0,
            pos_y: 0.0,
            queue_command: false,
            final_batch: true,
        }
    ));
    assert!(simulate_unit_payload_entries(&world, &DashMap::new(), 1.0));
    assert!(!world.enemies.contains_key(&3_000_000));
    assert_eq!(
        world
            .tiles
            .get(&reconstructor_position)
            .unwrap()
            .stored_amount,
        1
    );
    assert!(simulate_reconstructors(
        &world,
        &DashMap::new(),
        600.0,
        &power
    ));
    let upgraded = world.enemies.get(&3_000_001).unwrap();
    assert_eq!(upgraded.team, 1);
    assert_eq!(upgraded.unit_type, MACE.unit_type);
    drop(upgraded);
    let reconstructor = world.tiles.get(&reconstructor_position).unwrap();
    assert_eq!(reconstructor.stored_amount, 0);
    assert_eq!(inventory_count(&reconstructor.inventory, 9), 0);
    assert_eq!(inventory_count(&reconstructor.inventory, 4), 0);
    drop(reconstructor);

    let wall_position = ((i32::from(SPAWN_X) + 4) << 16) | (i32::from(SPAWN_Y) + 10);
    let mut wall = base_building_tombstone(&BaseBuildingState {
        position: wall_position,
        block: 216,
        team: 1,
        health: 100.0,
        occupied: vec![wall_position],
        inventory: Vec::new(),
    });
    wall.block = 216;
    wall.team = 1;
    wall.health = 100.0;
    world.tiles.insert(wall_position, wall);
    let mut nova = world.enemies.get(&3_000_001).unwrap().clone();
    nova.id = 3_000_010;
    nova.unit_type = NOVA.unit_type;
    nova.entity_class = NOVA.entity_class;
    nova.x = (i32::from(SPAWN_X) as f32 + 4.0) * 8.0;
    nova.y = (i32::from(SPAWN_Y) as f32 + 10.0) * 8.0;
    nova.move_speed = NOVA.speed;
    world.enemies.insert(nova.id, nova.clone());
    assert!(apply_command_units(
        &world,
        &CommandUnitsRequest {
            unit_ids: vec![nova.id],
            build_target: -1,
            unit_target_type: 0,
            unit_target_id: 0,
            pos_x: nova.x + 1_000.0,
            pos_y: nova.y,
            queue_command: false,
            final_batch: true,
        }
    ));
    assert!(apply_set_unit_stance(&world, &[nova.id], 5, true));
    assert!(simulate_unit_elevation(&world, 20.0));
    assert_eq!(world.enemies.get(&nova.id).unwrap().elevation, 1.0);
    let nova_x = world.enemies.get(&nova.id).unwrap().x;
    let nova_y = world.enemies.get(&nova.id).unwrap().y;
    assert!(simulate_allied_units(&world, &DashMap::new(), 10.0));
    let routed_nova = world.enemies.get(&nova.id).unwrap();
    let nova_distance = (routed_nova.x - nova_x).hypot(routed_nova.y - nova_y);
    assert!(nova_distance <= 8.25 + 0.001);
    drop(routed_nova);
    let nova_snapshot = world.enemies.get(&nova.id).unwrap().clone();
    let mut sync = Vec::new();
    write_unit_sync(
        &mut sync,
        Some(&world),
        &nova_snapshot,
        nova_snapshot.x,
        nova_snapshot.y,
        None,
        None,
    )
    .unwrap();
    let mut sync = std::io::Cursor::new(sync);
    assert_eq!(sync.read_b().unwrap(), 0);
    let _aim_x = sync.read_f().unwrap();
    let _aim_y = sync.read_f().unwrap();
    let _base_rotation = sync.read_f().unwrap();
    assert_eq!(sync.read_b().unwrap(), 9); // TypeIO CommandAI
    assert!(!sync.read_bool().unwrap()); // no attack target
    assert!(sync.read_bool().unwrap()); // targetPos
    assert_eq!(sync.read_f().unwrap(), nova.x + 1_000.0);
    assert_eq!(sync.read_f().unwrap(), nova.y);
    assert_eq!(sync.read_b().unwrap(), 0); // move command
    assert_eq!(sync.read_b().unwrap(), 0); // empty command queue
    assert_eq!(sync.read_b().unwrap(), 1); // one stance
    assert_eq!(sync.read_b().unwrap(), 5); // boost
    assert_eq!(sync.read_f().unwrap(), 1.0);
    assert!(apply_set_unit_stance(&world, &[nova.id], 5, false));
    assert!(simulate_unit_elevation(&world, 13.0));
    assert_eq!(world.enemies.get(&nova.id).unwrap().elevation, 0.0);
    *world.game_state.simulation_time.write() = 240.0;
    assert!(simulate_support_units(&world, &DashMap::new(), 6.0));
    assert_eq!(world.tiles.get(&wall_position).unwrap().health, 132.0);
    let mut poly = world.enemies.get(&3_000_001).unwrap().clone();
    poly.id = 3_000_012;
    poly.unit_type = 21;
    poly.entity_class = enemy_spec(21).unwrap().entity_class;
    poly.x -= 300.0;
    poly.y -= 300.0;
    poly.move_speed = enemy_spec(21).unwrap().speed;
    world.enemies.insert(poly.id, poly.clone());
    assert!(apply_set_unit_command(&world, &[poly.id], 1));
    assert!(simulate_support_units(&world, &DashMap::new(), 1.0));
    let moved_poly = world.enemies.get(&poly.id).unwrap();
    assert_ne!((moved_poly.x, moved_poly.y), (poly.x, poly.y));
    drop(moved_poly);

    let assist_snapshot = world.enemies.get(&poly.id).unwrap().clone();
    let assist_position = (((assist_snapshot.x / 8.0).round() as i32) << 16)
        | ((assist_snapshot.y / 8.0).round() as i16 as u16 as i32);
    world.pending_builds.insert(
        assist_position,
        PendingBuild {
            position: assist_position,
            block: 216,
            rotation: 0,
            config: vec![0],
            occupied: vec![assist_position],
            team: 1,
            builder: player(),
            last_seen: std::time::Instant::now(),
            assist_progress: 0.0,
            remaining_ticks: 0.0,
            applied_assist: 0.0,
        },
    );
    assert!(apply_set_unit_command(&world, &[poly.id], 3));
    let visual = assist_visual_plan(&world, &world.enemies.get(&poly.id).unwrap()).unwrap();
    assert_eq!(visual.position, assist_position);
    assert_eq!(visual.block, 216);
    assert_eq!(visual.rotation, 0);
    assert_eq!(visual.config, vec![0]);
    assert!(simulate_assist_units(&world, 10.0));
    assert_eq!(
        world
            .pending_builds
            .get(&assist_position)
            .unwrap()
            .assist_progress,
        5.0
    );
    world.pending_builds.remove(&assist_position);

    *world.game_state.mode.write() = GameMode::Survival;
    let copper_before_rebuild = world.game_state.core_items.read()[0];
    assert_eq!(
        damage_building(&world, wall_position, 10_000.0),
        Some((true, 0.0))
    );
    let tombstone = world.tiles.get(&wall_position).unwrap();
    assert_eq!(tombstone.block, 0);
    assert_eq!(tombstone.stored_amount, 217);
    assert_eq!(tombstone.team, 1);
    drop(tombstone);
    assert!(apply_set_unit_command(&world, &[poly.id], 2));
    let mut assistant = poly.clone();
    assistant.id = 3_000_013;
    assistant.x = f32::from((wall_position >> 16) as i16) * 8.0;
    assistant.y = f32::from(wall_position as i16) * 8.0;
    world.enemies.insert(assistant.id, assistant.clone());
    assert!(apply_set_unit_command(&world, &[assistant.id], 3));
    let before_rebuild_move = world.enemies.get(&poly.id).unwrap().clone();
    assert!(simulate_builder_units(&world, &DashMap::new(), 1.0));
    let moved_builder = world.enemies.get(&poly.id).unwrap();
    assert_ne!(
        (moved_builder.x, moved_builder.y),
        (before_rebuild_move.x, before_rebuild_move.y)
    );
    drop(moved_builder);
    {
        let mut builder = world.enemies.get_mut(&poly.id).unwrap();
        builder.x = f32::from((wall_position >> 16) as i16) * 8.0;
        builder.y = f32::from(wall_position as i16) * 8.0;
    }
    assert!(simulate_builder_units(&world, &DashMap::new(), 1.0));
    assert!(simulate_assist_units(&world, 10.0));
    assert_eq!(
        world.tiles.get(&wall_position).unwrap().production_progress,
        5.5
    );
    let rebuild_ticks = crate::game::content::block_build_time(216) / 0.5;
    assert!(simulate_builder_units(
        &world,
        &DashMap::new(),
        rebuild_ticks
    ));
    let rebuilt = world.tiles.get(&wall_position).unwrap();
    assert_eq!(rebuilt.block, 216);
    assert_eq!(rebuilt.team, 1);
    assert_eq!(rebuilt.health, crate::game::content::block_health(216));
    drop(rebuilt);
    assert_eq!(
        world.game_state.core_items.read()[0],
        copper_before_rebuild - 6
    );
    *world.game_state.mode.write() = GameMode::Sandbox;
    let finish = encode_construct_finish_for_unit(poly.id, wall_position, 216, 0, 1, &[0]).unwrap();
    let mut finish = std::io::Cursor::new(finish);
    assert_eq!(finish.read_i().unwrap(), wall_position);
    assert_eq!(finish.read_s().unwrap(), 216);
    assert_eq!(finish.read_b().unwrap(), 2);
    assert_eq!(finish.read_i().unwrap(), poly.id);
    assert_eq!(finish.read_b().unwrap(), 0);
    assert_eq!(finish.read_b().unwrap(), 1);
    assert_eq!(finish.read_b().unwrap(), 0);
    let begin = encode_begin_place_for_unit(poly.id, wall_position, 216, 0, 1, &[0]).unwrap();
    let mut begin = std::io::Cursor::new(begin);
    assert_eq!(begin.read_b().unwrap(), 2);
    assert_eq!(begin.read_i().unwrap(), poly.id);
    assert_eq!(begin.read_s().unwrap(), 216);
    assert_eq!(begin.read_b().unwrap(), 1);
    assert_eq!(begin.read_i().unwrap(), (wall_position >> 16) as i16 as i32);
    assert_eq!(begin.read_i().unwrap(), wall_position as i16 as i32);
    assert_eq!(begin.read_i().unwrap(), 0);
    assert_eq!(begin.read_b().unwrap(), 0);

    let mut mega = world.enemies.get(&3_000_001).unwrap().clone();
    let mega_spec = enemy_spec(22).unwrap();
    mega.id = 3_000_022;
    mega.unit_type = 22;
    mega.entity_class = mega_spec.entity_class;
    mega.x = f32::from((wall_position >> 16) as i16) * 8.0;
    mega.y = f32::from(wall_position as i16) * 8.0;
    mega.payloads.clear();
    mega.move_speed = mega_spec.speed;
    world.enemies.insert(mega.id, mega.clone());
    assert!(apply_set_unit_command(&world, &[mega.id], 7));
    assert!(apply_command_units(
        &world,
        &CommandUnitsRequest {
            unit_ids: vec![mega.id],
            build_target: wall_position,
            unit_target_type: 0,
            unit_target_id: 0,
            pos_x: mega.x,
            pos_y: mega.y,
            queue_command: false,
            final_batch: true,
        }
    ));
    assert!(simulate_payload_carriers(&world, &DashMap::new(), 1.0));
    assert!(dynamic_at(&world, wall_position).is_none());
    assert!(matches!(
        world.enemies.get(&mega.id).unwrap().payloads.last(),
        Some(CarriedPayload::Build(build)) if build.tile.block == 216
    ));
    assert!(apply_set_unit_command(&world, &[mega.id], 8));
    {
        let mut carrier = world.enemies.get_mut(&mega.id).unwrap();
        carrier.x += 80.0;
    }
    assert!(simulate_payload_carriers(&world, &DashMap::new(), 1.0));
    assert!(world.enemies.get(&mega.id).unwrap().payloads.is_empty());
    assert!(world
        .tiles
        .iter()
        .any(|tile| tile.block == 216 && tile.position != wall_position));

    let conveyor_a = ((i32::from(SPAWN_X) + 20) << 16) | (i32::from(SPAWN_Y) + 20);
    let conveyor_b = ((i32::from(SPAWN_X) + 23) << 16) | (i32::from(SPAWN_Y) + 20);
    let conveyor_c = ((i32::from(SPAWN_X) + 26) << 16) | (i32::from(SPAWN_Y) + 20);
    for (position, block) in [(conveyor_a, 398), (conveyor_b, 399), (conveyor_c, 400)] {
        let mut tile = base_building_tombstone(&BaseBuildingState {
            position,
            block,
            team: 1,
            health: crate::game::content::block_health(block),
            occupied: block_footprint(&world, position, block).unwrap(),
            inventory: Vec::new(),
        });
        tile.block = block;
        tile.team = 1;
        tile.rotation = 0;
        tile.health = crate::game::content::block_health(block);
        world.tiles.insert(position, tile);
    }
    let mut carried = world.enemies.get(&3_000_001).unwrap().clone();
    carried.id = 3_000_030;
    carried.unit_type = DAGGER.unit_type;
    carried.entity_class = DAGGER.entity_class;
    carried.team = 1;
    carried.payloads.clear();
    carried.health = DAGGER.health;
    carried.move_speed = DAGGER.speed;
    carried.attack_damage = DAGGER.attack_damage;
    carried.attack_reload_time = DAGGER.attack_reload;
    carried.attack_range = DAGGER.attack_range;
    world.tiles.get_mut(&conveyor_a).unwrap().payload =
        Some(Box::new(CarriedPayload::Unit(carried)));
    let next_id = world.next_enemy_id.load(Ordering::Relaxed);
    let mut payload_changed = false;
    for _ in 0..10 {
        payload_changed |= simulate_payload_conveyors(&world, &DashMap::new(), 45.0);
        if [conveyor_a, conveyor_b, conveyor_c]
            .iter()
            .all(|position| world.tiles.get(position).unwrap().payload.is_none())
        {
            break;
        }
    }
    assert!(payload_changed);
    assert!(world.tiles.get(&conveyor_a).unwrap().payload.is_none());
    assert!(world.tiles.get(&conveyor_b).unwrap().payload.is_none());
    assert!(world.tiles.get(&conveyor_c).unwrap().payload.is_none());
    let dumped = world.enemies.get(&next_id).unwrap();
    assert_eq!(dumped.unit_type, DAGGER.unit_type);
    assert_eq!(dumped.team, 1);
    let mut driver_payload = dumped.clone();
    drop(dumped);

    let driver_a = ((i32::from(SPAWN_X) + 30) << 16) | (i32::from(SPAWN_Y) + 30);
    let driver_b = ((i32::from(SPAWN_X) + 40) << 16) | (i32::from(SPAWN_Y) + 30);
    for position in [driver_a, driver_b] {
        let mut tile = base_building_tombstone(&BaseBuildingState {
            position,
            block: 402,
            team: 1,
            health: crate::game::content::block_health(402),
            occupied: block_footprint(&world, position, 402).unwrap(),
            inventory: Vec::new(),
        });
        tile.block = 402;
        tile.team = 1;
        tile.health = crate::game::content::block_health(402);
        world.tiles.insert(position, tile);
    }
    driver_payload.id = 3_000_031;
    world.tiles.get_mut(&driver_a).unwrap().payload =
        Some(Box::new(CarriedPayload::Unit(driver_payload)));
    world.tiles.get_mut(&driver_a).unwrap().config =
        [vec![1], driver_b.to_be_bytes().to_vec()].concat();
    let driver_power = std::collections::HashMap::from([(driver_a, 1.0), (driver_b, 1.0)]);
    assert!(simulate_payload_mass_drivers(&world, 220.0, &driver_power));
    assert!(world.tiles.get(&driver_a).unwrap().payload.is_none());
    assert!(matches!(
        world.tiles.get(&driver_b).unwrap().payload.as_deref(),
        Some(CarriedPayload::Unit(unit)) if unit.unit_type == DAGGER.unit_type
    ));

    let loader_position = ((i32::from(SPAWN_X) + 50) << 16) | (i32::from(SPAWN_Y) + 30);
    let loader_output = ((i32::from(SPAWN_X) + 53) << 16) | (i32::from(SPAWN_Y) + 30);
    for (position, block) in [(loader_position, 408), (loader_output, 398)] {
        let mut tile = base_building_tombstone(&BaseBuildingState {
            position,
            block,
            team: 1,
            health: crate::game::content::block_health(block),
            occupied: block_footprint(&world, position, block).unwrap(),
            inventory: Vec::new(),
        });
        tile.block = block;
        tile.team = 1;
        tile.rotation = 0;
        tile.health = crate::game::content::block_health(block);
        world.tiles.insert(position, tile);
    }
    let container_position = ((i32::from(SPAWN_X) + 51) << 16) | (i32::from(SPAWN_Y) + 30);
    let mut container = base_building_tombstone(&BaseBuildingState {
        position: container_position,
        block: 345,
        team: 1,
        health: crate::game::content::block_health(345),
        occupied: vec![container_position],
        inventory: Vec::new(),
    });
    container.block = 345;
    container.team = 1;
    container.health = crate::game::content::block_health(345);
    container.inventory = vec![(9, 298)];
    let mut container_sync = Vec::new();
    encode_storage_sync(&mut container_sync, &container, None).unwrap();
    world.tiles.get_mut(&loader_position).unwrap().payload =
        Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
            tile: container,
            version: 0,
            sync: container_sync,
        })));
    world.tiles.get_mut(&loader_position).unwrap().inventory = vec![(9, 10)];
    let loader_power = std::collections::HashMap::from([(loader_position, 1.0)]);
    for _ in 0..3 {
        assert!(simulate_payload_loaders(&world, 2.0, &loader_power));
    }
    assert!(world.tiles.get(&loader_position).unwrap().payload.is_none());
    let mut loaded_payload = world
        .tiles
        .get_mut(&loader_output)
        .unwrap()
        .payload
        .take()
        .unwrap();
    assert!(
        matches!(loaded_payload.as_ref(), CarriedPayload::Build(build)
        if inventory_count(&build.tile.inventory, 9) == 300)
    );
    if let CarriedPayload::Build(build) = loaded_payload.as_mut() {
        build.tile.inventory = vec![(9, 10)];
        assert!(refresh_build_payload_sync(&world, &loader_power, build));
    }

    let unloader_position = ((i32::from(SPAWN_X) + 60) << 16) | (i32::from(SPAWN_Y) + 30);
    let mut unloader = base_building_tombstone(&BaseBuildingState {
        position: unloader_position,
        block: 409,
        team: 1,
        health: crate::game::content::block_health(409),
        occupied: block_footprint(&world, unloader_position, 409).unwrap(),
        inventory: Vec::new(),
    });
    unloader.block = 409;
    unloader.team = 1;
    unloader.health = crate::game::content::block_health(409);
    unloader.payload = Some(loaded_payload);
    world.tiles.insert(unloader_position, unloader);
    let unloader_power = std::collections::HashMap::from([(unloader_position, 1.0)]);
    assert!(simulate_payload_loaders(&world, 2.0, &unloader_power));
    assert!(simulate_payload_loaders(&world, 2.0, &unloader_power));
    let unloader = world.tiles.get(&unloader_position).unwrap();
    assert_eq!(inventory_count(&unloader.inventory, 9), 10);
    assert!(
        matches!(unloader.payload.as_deref(), Some(CarriedPayload::Build(build))
        if build.tile.inventory.is_empty())
    );
    drop(unloader);

    let tank_position = ((i32::from(SPAWN_X) + 54) << 16) | (i32::from(SPAWN_Y) + 30);
    let mut tank = base_building_tombstone(&BaseBuildingState {
        position: tank_position,
        block: 290,
        team: 1,
        health: crate::game::content::block_health(290),
        occupied: vec![tank_position],
        inventory: Vec::new(),
    });
    tank.block = 290;
    tank.team = 1;
    tank.health = crate::game::content::block_health(290);
    tank.stored_liquid = 0;
    tank.liquid_amount = 690.0;
    let mut tank_sync = Vec::new();
    encode_liquid_block_sync(&mut tank_sync, &tank, &std::collections::HashMap::new()).unwrap();
    {
        let mut loader = world.tiles.get_mut(&loader_position).unwrap();
        loader.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
            tile: tank,
            version: 0,
            sync: tank_sync,
        })));
        loader.production_progress = 0.0;
        loader.stored_liquid = 0;
        loader.liquid_amount = 20.0;
    }
    assert!(simulate_payload_loaders(&world, 2.0, &loader_power));
    assert!(simulate_payload_loaders(&world, 2.0, &loader_power));
    let loader = world.tiles.get(&loader_position).unwrap();
    assert_eq!(loader.liquid_amount, 10.0);
    assert_eq!(loader.production_progress, 1.0);
    assert!(
        matches!(loader.payload.as_deref(), Some(CarriedPayload::Build(build))
        if build.tile.block == 290 && build.tile.liquid_amount == 700.0)
    );
    drop(loader);

    let battery_position = ((i32::from(SPAWN_X) + 57) << 16) | (i32::from(SPAWN_Y) + 30);
    let mut battery = base_building_tombstone(&BaseBuildingState {
        position: battery_position,
        block: 306,
        team: 1,
        health: crate::game::content::block_health(306),
        occupied: vec![battery_position],
        inventory: Vec::new(),
    });
    battery.block = 306;
    battery.team = 1;
    battery.health = crate::game::content::block_health(306);
    battery.power_stored = 3_900.0;
    let mut battery_sync = Vec::new();
    encode_battery_sync(&mut battery_sync, &battery).unwrap();
    {
        let mut loader = world.tiles.get_mut(&loader_position).unwrap();
        loader.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
            tile: battery,
            version: 0,
            sync: battery_sync,
        })));
        loader.production_progress = 0.0;
        loader.liquid_amount = 0.0;
        loader.stored_liquid = -1;
    }
    assert!(simulate_payload_loaders(&world, 3.0, &loader_power));
    let loader = world.tiles.get(&loader_position).unwrap();
    assert_eq!(loader.production_progress, 1.0);
    let charged_battery = loader.payload.clone().unwrap();
    assert!(
        matches!(charged_battery.as_ref(), CarriedPayload::Build(build)
        if build.tile.block == 306 && build.tile.power_stored == 4_000.0)
    );
    drop(loader);
    if let CarriedPayload::Build(mut build) = *charged_battery {
        build.tile.power_stored = 100.0;
        assert!(refresh_build_payload_sync(
            &world,
            &loader_power,
            &mut build
        ));
        let mut unloader = world.tiles.get_mut(&unloader_position).unwrap();
        unloader.payload = Some(Box::new(CarriedPayload::Build(build)));
        unloader.production_progress = 0.0;
        unloader.inventory.clear();
    }
    assert!(simulate_payload_loaders(&world, 2.0, &unloader_power));
    let unloader = world.tiles.get(&unloader_position).unwrap();
    assert_eq!(unloader.output_liquid_amount, 50.0);
    assert_eq!(unloader.production_progress, 1.0);
    assert_eq!(compute_power_efficiency(&world)[&unloader_position], 1.0);
    assert!(
        matches!(unloader.payload.as_deref(), Some(CarriedPayload::Build(build))
        if build.tile.power_stored == 0.0)
    );
    drop(unloader);

    let small_deconstructor = ((i32::from(SPAWN_X) + 70) << 16) | (i32::from(SPAWN_Y) + 30);
    let large_deconstructor = ((i32::from(SPAWN_X) + 76) << 16) | (i32::from(SPAWN_Y) + 30);
    for (position, block) in [(small_deconstructor, 404), (large_deconstructor, 405)] {
        let mut tile = base_building_tombstone(&BaseBuildingState {
            position,
            block,
            team: 1,
            health: crate::game::content::block_health(block),
            occupied: block_footprint(&world, position, block).unwrap(),
            inventory: Vec::new(),
        });
        tile.block = block;
        tile.team = 1;
        tile.health = crate::game::content::block_health(block);
        world.tiles.insert(position, tile);
    }
    let wall_position = ((i32::from(SPAWN_X) + 71) << 16) | (i32::from(SPAWN_Y) + 30);
    let mut wall = base_building_tombstone(&BaseBuildingState {
        position: wall_position,
        block: 216,
        team: 1,
        health: crate::game::content::block_health(216),
        occupied: vec![wall_position],
        inventory: Vec::new(),
    });
    wall.block = 216;
    wall.team = 1;
    wall.health = crate::game::content::block_health(216);
    let mut wall_sync = Vec::new();
    encode_simple_wall_sync(&mut wall_sync, &wall).unwrap();
    world.tiles.get_mut(&small_deconstructor).unwrap().payload =
        Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
            tile: wall,
            version: 0,
            sync: wall_sync,
        })));
    let mut dagger_payload = world.enemies.get(&3_000_001).unwrap().clone();
    dagger_payload.id = 3_000_032;
    dagger_payload.unit_type = DAGGER.unit_type;
    dagger_payload.entity_class = DAGGER.entity_class;
    dagger_payload.payloads.clear();
    world.tiles.get_mut(&large_deconstructor).unwrap().payload =
        Some(Box::new(CarriedPayload::Unit(dagger_payload)));
    let deconstructor_power =
        std::collections::HashMap::from([(small_deconstructor, 1.0), (large_deconstructor, 1.0)]);
    assert!(simulate_payload_deconstructors(
        &world,
        crate::game::content::block_build_time(216) / 3.0,
        &deconstructor_power,
    ));
    let _ = simulate_payload_deconstructors(
        &world,
        crate::game::content::unit_requirements(DAGGER.unit_type)
            .unwrap()
            .0
            / 6.0,
        &deconstructor_power,
    );
    let deconstructor = world.tiles.get(&small_deconstructor).unwrap();
    assert!(deconstructor.payload.is_none());
    assert_eq!(inventory_count(&deconstructor.inventory, 0), 6);
    drop(deconstructor);
    let deconstructor = world.tiles.get(&large_deconstructor).unwrap();
    assert!(deconstructor.payload.is_none());
    assert_eq!(inventory_count(&deconstructor.inventory, 1), 10);
    assert_eq!(inventory_count(&deconstructor.inventory, 9), 10);
    drop(deconstructor);

    let constructor_position = ((i32::from(SPAWN_X) + 82) << 16) | (i32::from(SPAWN_Y) + 30);
    let mut constructor = base_building_tombstone(&BaseBuildingState {
        position: constructor_position,
        block: 406,
        team: 1,
        health: crate::game::content::block_health(406),
        occupied: block_footprint(&world, constructor_position, 406).unwrap(),
        inventory: Vec::new(),
    });
    constructor.block = 406;
    constructor.team = 1;
    constructor.health = crate::game::content::block_health(406);
    constructor.config = vec![5, 1, 0, 236]; // Beryllium Wall Large
    constructor.inventory = vec![(16, 24)];
    world.tiles.insert(constructor_position, constructor);
    assert_eq!(constructor_item_capacity(406, &[5, 1, 0, 236], 16), 48);
    assert!(SMALL_CONSTRUCTOR_RECIPES
        .iter()
        .all(|recipe| is_pickup_payload_supported(*recipe)));
    let unsupported_large: Vec<_> = LARGE_CONSTRUCTOR_RECIPES
        .iter()
        .copied()
        .filter(|recipe| !is_pickup_payload_supported(*recipe))
        .collect();
    assert!(
        unsupported_large.is_empty(),
        "Large Constructor recipes without BuildPayload codec: {unsupported_large:?}"
    );
    assert!(simulate_payload_constructors(
        &world,
        crate::game::content::block_build_time(236) / 0.6,
        &std::collections::HashMap::from([(constructor_position, 1.0)]),
    ));
    let constructor = world.tiles.get(&constructor_position).unwrap();
    assert_eq!(inventory_count(&constructor.inventory, 16), 0);
    assert!(matches!(
        constructor.payload.as_deref(),
        Some(CarriedPayload::Build(build)) if build.tile.block == 236
    ));
    drop(constructor);

    let large_constructor_position = ((i32::from(SPAWN_X) + 88) << 16) | (i32::from(SPAWN_Y) + 30);
    let mut large_constructor = base_building_tombstone(&BaseBuildingState {
        position: large_constructor_position,
        block: 407,
        team: 1,
        health: crate::game::content::block_health(407),
        occupied: block_footprint(&world, large_constructor_position, 407).unwrap(),
        inventory: Vec::new(),
    });
    large_constructor.block = 407;
    large_constructor.team = 1;
    large_constructor.health = crate::game::content::block_health(407);
    large_constructor.config = vec![5, 1, 1, 118]; // Scathe, block 374.
    large_constructor.inventory = crate::game::content::block_requirements(374)
        .iter()
        .map(|(item, amount)| (i16::try_from(*item).unwrap(), *amount))
        .collect();
    world
        .tiles
        .insert(large_constructor_position, large_constructor);
    assert!(simulate_payload_constructors(
        &world,
        crate::game::content::block_build_time(374) / 0.75,
        &std::collections::HashMap::from([(large_constructor_position, 1.0)]),
    ));
    let large_constructor = world.tiles.get(&large_constructor_position).unwrap();
    assert!(large_constructor.inventory.is_empty());
    assert!(matches!(
        large_constructor.payload.as_deref(),
        Some(CarriedPayload::Build(build))
            if build.tile.block == 374 && build.version == 2 && !build.sync.is_empty()
    ));
    drop(large_constructor);

    // Mono's vanilla UnitType.mineItems contains copper/lead only. Pick
    // the same least-stocked target that MinerAI will choose instead of
    // the first arbitrary mineable overlay (which may be sand/scrap).
    let ore_item = crate::network::simulation::mono_target_item(&world, 1)
        .expect("dummy map exposes a vanilla Mono ore");
    let (core_x, core_y) = core_world(&world);
    let (ore_position, found_item, ore_hardness, _, _) =
        nearest_mineable_ore(&world, core_x, core_y, ore_item).unwrap();
    assert_eq!(found_item, ore_item);
    let mut mono = world.enemies.get(&3_000_001).unwrap().clone();
    mono.id = 3_000_011;
    mono.unit_type = MONO.unit_type;
    mono.entity_class = MONO.entity_class;
    mono.x = (ore_position >> 16) as i16 as f32 * 8.0;
    mono.y = ore_position as i16 as f32 * 8.0;
    mono.attack_reload = 0.0;
    mono.secondary_attack_reload = 0.0;
    mono.tertiary_attack_reload = 0.0;
    mono.move_speed = MONO.speed;
    mono.attack_damage = 0.0;
    world.enemies.insert(mono.id, mono);
    assert!(apply_set_unit_command(&world, &[3_000_011], 0));
    assert!(!apply_set_unit_command(&world, &[3_000_011], 1));
    assert!(apply_set_unit_command(&world, &[3_000_011], 4));
    assert_eq!(world.unit_orders.get(&3_000_011).unwrap().command, 4);
    assert!(apply_set_unit_stance(&world, &[3_000_011], 8, true));
    assert!(unit_has_stance(&world, 3_000_011, 8));
    assert!(apply_set_unit_stance(&world, &[3_000_011], 7, false));
    assert!(unit_has_stance(&world, 3_000_011, 7));
    assert!(!unit_has_stance(&world, 3_000_011, 8));
    let mining_ticks = (50.0 + f32::from(ore_hardness) * 15.0) / 2.5;
    assert!(simulate_support_units(
        &world,
        &DashMap::new(),
        mining_ticks
    ));
    let mut mono = world.enemies.get_mut(&3_000_011).unwrap();
    assert_eq!(mono.secondary_attack_reload, 1.0);
    assert_eq!(mono.tertiary_attack_reload, f32::from(ore_item + 1));
    mono.secondary_attack_reload = 30.0;
    mono.x = core_x;
    mono.y = core_y;
    drop(mono);
    let previous = world.game_state.core_items.read()[ore_item as usize];
    assert!(simulate_support_units(&world, &DashMap::new(), 1.0));
    assert_eq!(
        world.game_state.core_items.read()[ore_item as usize],
        previous + 30
    );

    for offset in 0..8 {
        let mut dagger = ally.clone();
        dagger.id = 3_000_100 + offset;
        world.enemies.insert(dagger.id, dagger);
    }
    {
        let mut factory = world.tiles.get_mut(&position).unwrap();
        inventory_add(&mut factory.inventory, 9, 10);
        inventory_add(&mut factory.inventory, 1, 10);
        factory.production_progress = 0.0;
    }
    assert!(!can_create_unit(&world, 1, DAGGER.unit_type));
    assert!(!simulate_unit_factories(
        &world,
        &DashMap::new(),
        900.0,
        &power
    ));
    let factory = world.tiles.get(&position).unwrap();
    assert_eq!(inventory_count(&factory.inventory, 9), 10);
    assert_eq!(inventory_count(&factory.inventory, 1), 10);
    assert_eq!(factory.production_progress, 0.0);
}

#[test]
fn live_team_plans_are_spliced_into_the_personalized_world_stream() {
    // A late joiner must see ghost builds that appeared after hosting:
    // the official server writes the current TeamData.plans on every
    // connection (SaveVersion.writeTeamBlocks), not just host-time plans.
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("team-plans-test".into(), GameMode::Survival);
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
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
        tiles: DashMap::new(),
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::new(),
        mono_mining_targets: DashMap::new(),
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-team-plans-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };

    // Simulate a construction the server mirrored into the live plans.
    add_team_plan(
        &world,
        1,
        crate::engine::typeio::TeamBlockPlan {
            x: 45,
            y: 100,
            rotation: 1,
            block: 257,
            config: vec![1, 0, 0, 0, 42],
        },
    );
    let plan = crate::engine::typeio::TeamBlockPlan {
        x: 46,
        y: 101,
        rotation: 0,
        block: 261,
        config: vec![7, 0, 0, 1, 0, 0, 0, 2, 0],
    };
    add_team_plan(&world, 1, plan.clone());
    // Adding the same plan twice must not duplicate the ghost.
    add_team_plan(&world, 1, plan.clone());
    assert_eq!(
        world.team_build_plans.read().teams.len(),
        1,
        "only the sharded team should own plans"
    );
    assert_eq!(world.team_build_plans.read().teams[0].plans.len(), 2);

    // The re-emitted stream must carry the live plans byte-exactly.
    let template = network_template_with_plans(&world).unwrap();
    let plans = crate::engine::world_stream::inspect_team_plans(&template).unwrap();
    assert_eq!(&plans, &*world.team_build_plans.read());
    assert!(
        plans.teams[0]
            .plans
            .iter()
            .any(|p| p.x == 45 && p.y == 100 && p.block == 257 && p.rotation == 1),
        "the live conveyor plan must reach the late joiner"
    );

    // Completing the construction removes the ghost from the next stream.
    remove_team_plan(&world, 1, 45, 100);
    assert_eq!(world.team_build_plans.read().teams[0].plans.len(), 1);
    let template = network_template_with_plans(&world).unwrap();
    let plans = crate::engine::world_stream::inspect_team_plans(&template).unwrap();
    assert!(
        !plans.teams[0].plans.iter().any(|p| p.x == 45 && p.y == 100),
        "completed plans must not be re-emitted"
    );
}

#[test]
fn remove_team_plan_is_scoped_to_the_acting_team() {
    // Official InputHandler.deletePlans (158.1) only iterates
    // `player.team().data().plans`; a delete request from one team must
    // never erase another team's ghost plans, even at the same tile.
    let mut blocks = crate::engine::typeio::TeamBlocks {
        teams: vec![
            crate::engine::typeio::TeamPlans {
                team: 1,
                plans: vec![
                    crate::engine::typeio::TeamBlockPlan {
                        x: 45,
                        y: 100,
                        rotation: 0,
                        block: 257,
                        config: Vec::new(),
                    },
                    crate::engine::typeio::TeamBlockPlan {
                        x: 10,
                        y: 10,
                        rotation: 0,
                        block: 261,
                        config: Vec::new(),
                    },
                ],
            },
            crate::engine::typeio::TeamPlans {
                team: 2,
                plans: vec![
                    crate::engine::typeio::TeamBlockPlan {
                        x: 45,
                        y: 100,
                        rotation: 0,
                        block: 258,
                        config: Vec::new(),
                    },
                    crate::engine::typeio::TeamBlockPlan {
                        x: 20,
                        y: 20,
                        rotation: 0,
                        block: 261,
                        config: Vec::new(),
                    },
                ],
            },
        ],
    };
    assert!(remove_team_plan_from(&mut blocks, 1, 45, 100));
    assert_eq!(blocks.teams[0].plans.len(), 1);
    assert_eq!(blocks.teams[0].plans[0].x, 10);
    // Team 2's plan at the SAME tile survives the team-1 deletion.
    assert_eq!(blocks.teams[1].plans.len(), 2);
    assert!(
        blocks.teams[1]
            .plans
            .iter()
            .any(|p| p.x == 45 && p.y == 100),
        "another team's ghost plan must not be removed"
    );
    // A team with no matching entry is untouched and reports no removal.
    assert!(!remove_team_plan_from(&mut blocks, 3, 45, 100));
    assert_eq!(blocks.teams[1].plans.len(), 2);
}

#[test]
fn enemy_roles_apply_suicide_orbits_support_and_armor() {
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("enemy-role-test".into(), GameMode::Survival);
    *state.wave_time.write() = 10_000.0;
    *state.simulation_time.write() = 240.0;
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_100),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-enemy-role-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let connections = DashMap::new();
    let make_enemy = |id: i32, spec: EnemySpec, x: f32, y: f32, health: f32| EnemyUnit {
        id,
        unit_type: spec.unit_type,
        entity_class: spec.entity_class,
        team: 2,
        x,
        y,
        rotation: 0.0,
        health,
        shield: 0.0,
        status_effect: -1,
        status_duration: f32::MAX,
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
        move_speed: spec.speed,
        attack_damage: spec.attack_damage,
        attack_reload_time: spec.attack_reload,
        attack_range: spec.attack_range,
        authority: UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        status_agg: None,
    };
    let core_x = SPAWN_X as f32 * 8.0;
    let core_y = SPAWN_Y as f32 * 8.0;
    world.enemies.insert(
        1,
        make_enemy(1, CRAWLER, core_x + 5.0, core_y, CRAWLER.health),
    );
    world.enemies.insert(
        2,
        make_enemy(2, HORIZON, core_x + 100.0, core_y, HORIZON.health),
    );
    world
        .enemies
        .insert(3, make_enemy(3, NOVA, core_x + 500.0, core_y, NOVA.health));
    world
        .enemies
        .insert(4, make_enemy(4, DAGGER, core_x + 510.0, core_y, 50.0));
    world.enemies.insert(
        5,
        make_enemy(5, QUASAR, core_x + 600.0, core_y, QUASAR.health),
    );
    world.enemies.insert(
        6,
        make_enemy(6, PULSAR, core_x + 505.0, core_y, PULSAR.health),
    );

    assert!(simulate_waves_and_enemies(&world, &connections, 6.0).0);
    assert!(!world.enemies.contains_key(&1));
    assert_eq!(*world.game_state.core_health.read(), 6000.0 - 81.0);
    assert_eq!(world.enemies.get(&4).unwrap().health, 60.0);
    assert_eq!(world.enemies.get(&5).unwrap().shield, 2.4);
    let horizon = world.enemies.get(&2).unwrap();
    assert!(horizon.velocity_x.abs() > 0.001 || horizon.velocity_y.abs() > 0.001);
    assert_ne!(horizon.x, core_x + 100.0);
    drop(horizon);

    world.enemies.get_mut(&4).unwrap().shield = 0.0;
    *world.game_state.simulation_time.write() = 300.0;
    simulate_waves_and_enemies(&world, &connections, 6.0);
    assert_eq!(world.enemies.get(&4).unwrap().shield, 20.0);
    assert_eq!(apply_unit_armor(10.0, enemy_armor(MACE.unit_type)), 6.0);
    assert_eq!(apply_unit_armor(2.0, enemy_armor(REIGN.unit_type)), 0.2);
    assert_eq!(projectile_armor_multiplier(129), 4.0);

    {
        let mut horizon = world.enemies.get_mut(&2).unwrap();
        horizon.x = core_x + 40.0;
        horizon.y = core_y;
        horizon.attack_reload = 0.0;
    }
    *world.game_state.core_health.write() = 6000.0;
    simulate_waves_and_enemies(&world, &connections, HORIZON.attack_reload);
    assert_eq!(world.projectiles.len(), 2);
    let bomb = world.projectiles.iter().next().unwrap();
    assert_eq!(bomb.team, 2);
    assert_eq!(bomb.bullet_id, 31);
    assert_eq!(bomb.damage, 13.5);
    assert_eq!(bomb.splash_damage, 27.0);
    assert_eq!(bomb.splash_radius, 25.0);
    let payload = encode_projectile_replay_payload(&bomb, None).unwrap();
    let mut input = std::io::Cursor::new(payload);
    assert_eq!(
        crate::network::codec::Reads::read_s(&mut input).unwrap(),
        31
    );
    assert_eq!(crate::network::codec::Reads::read_b(&mut input).unwrap(), 2);
    drop(bomb);

    for position in [
        ((i32::from(SPAWN_X) + 2) << 16) | i32::from(SPAWN_Y),
        ((i32::from(SPAWN_X) + 2) << 16) | (i32::from(SPAWN_Y) + 2),
    ] {
        world.base_buildings.insert(
            position,
            BaseBuildingState {
                position,
                block: 216,
                team: 1,
                health: 320.0,
                occupied: vec![position],
                inventory: Vec::new(),
            },
        );
    }
    world.players.insert(
        2_000_001,
        PlayerCombatState {
            uuid: "horizon-target".into(),
            player_id: 1_000_001,
            unit_id: 2_000_001,
            x: core_x + 10.0,
            y: core_y,
            health: 150.0,
            shield: 10.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    let mut session = player();
    session.id = 1_000_001;
    session.unit_id = 2_000_001;
    session.uuid = "horizon-target".into();
    world.player_sessions.insert(session.unit_id, session);
    assert_eq!(encode_all_player_snapshots(&world).unwrap().len(), 1);
    assert!(simulate_projectiles(&world, &connections, 30.0));
    assert!(world.projectiles.is_empty());
    assert_eq!(*world.game_state.core_health.read(), 6000.0 - 54.0);
    assert!(world
        .base_buildings
        .iter()
        .all(|building| building.health == 320.0 - 54.0));
    {
        let player = world.players.get(&2_000_001).unwrap();
        assert_eq!(player.health, 106.0);
        assert_eq!(player.shield, 0.0);
        // Horizon bomb applies `blasted` (18), which is reactive in
        // 158.1 and never persists as a StatusEntry.
        assert_eq!(player.status_effect, -1);
        assert_eq!(player.status_duration, 0.0);
    }
    let (player_tx, mut player_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: player_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("target".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    assert!(apply_enemy_splash_damage(
        &world,
        &connections,
        core_x + 10.0,
        core_y,
        200.0,
        1.0,
        1.0,
        18,
        60.0,
    ));
    assert!(world.players.get(&2_000_001).unwrap().dead);
    {
        use crate::network::codec::Reads;
        let snapshots = encode_all_player_snapshots(&world).unwrap();
        let mut snapshot = std::io::Cursor::new(&snapshots[0]);
        assert_eq!(snapshot.read_s().unwrap(), 2);
        let _data_length = snapshot.read_s().unwrap();
        assert_eq!(snapshot.read_i().unwrap(), 2_000_001);
        assert_eq!(snapshot.read_b().unwrap(), ALPHA_CLASS_ID);
        assert_eq!(snapshot.read_b().unwrap(), 0);
        let _aim_x = snapshot.read_f().unwrap();
        let _aim_y = snapshot.read_f().unwrap();
        assert_eq!(snapshot.read_b().unwrap(), 0);
        assert_eq!(snapshot.read_i().unwrap(), 1_000_001);
        let _elevation = snapshot.read_f().unwrap();
        let _flag = snapshot.read_l().unwrap();
        assert_eq!(snapshot.read_f().unwrap(), 0.0);
    }
    assert!(simulate_player_combat(&world, &connections, 59.0));
    assert!(world.players.get(&2_000_001).unwrap().dead);
    assert!(simulate_player_combat(&world, &connections, 1.0));
    let mut lifecycle_packets = Vec::new();
    while let Ok(frame) = player_rx.try_recv() {
        lifecycle_packets.push(read_packet(std::io::Cursor::new(&frame[2..])).unwrap()[0]);
    }
    assert!(lifecycle_packets.contains(&UNIT_DEATH_PACKET_ID));
    assert!(lifecycle_packets.contains(&PLAYER_SPAWN_PACKET_ID));
    let final_sync = lifecycle_packets
        .iter()
        .position(|packet| *packet == ENTITY_SNAPSHOT_PACKET_ID)
        .expect("a reliable final empty-stack sync must precede player death");
    let death = lifecycle_packets
        .iter()
        .position(|packet| *packet == UNIT_DEATH_PACKET_ID)
        .unwrap();
    assert!(final_sync < death);
    {
        assert!(!world.players.contains_key(&2_000_001));
        let player = world.players.get(&2_500_000).unwrap();
        assert!(!player.dead);
        assert_eq!(player.health, 150.0);
        assert_eq!(player.unit_id, 2_500_000);
        assert_eq!(player.x, core_x);
        assert_eq!(player.y, core_y);
    }
    assert!(!world.player_sessions.contains_key(&2_000_001));
    let (_, removed_session) = world.player_sessions.remove(&2_500_000).unwrap();
    let disconnect_frames = encode_player_disconnect_frames(&removed_session).unwrap();
    let disconnect_packet = read_packet(std::io::Cursor::new(&disconnect_frames[0][2..])).unwrap();
    let despawn_packet = read_packet(std::io::Cursor::new(&disconnect_frames[1][2..])).unwrap();
    assert_eq!(disconnect_packet[0], PLAYER_DISCONNECT_PACKET_ID);
    assert_eq!(despawn_packet[0], UNIT_DESPAWN_PACKET_ID);
    assert_eq!(&disconnect_packet[1..], &1_000_001i32.to_be_bytes());
    assert_eq!(despawn_packet[1], 2);
    assert_eq!(&despawn_packet[2..], &2_500_000i32.to_be_bytes());
    assert!(encode_all_player_snapshots(&world).unwrap().is_empty());

    world.enemies.clear();
    world.base_buildings.clear();
    world.projectiles.clear();
    for (id, spec, shots, bullet_id, volley_damage) in [
        (19, MACE, 1, 7, 74.0),
        (20, FORTRESS, 1, 8, 100.0),
        (21, REIGN, 1, 12, 98.0),
        (22, ZENITH, 2, 32, 58.0),
    ] {
        *world.game_state.core_health.write() = 6000.0;
        if spec.unit_type == MACE.unit_type {
            world.players.insert(
                2_599_999,
                PlayerCombatState {
                    uuid: "mace-pierce".into(),
                    player_id: 1_599_999,
                    unit_id: 2_599_999,
                    x: core_x + 20.0,
                    y: core_y,
                    health: 150.0,
                    shield: 0.0,
                    status_effect: -1,
                    status_duration: 0.0,
                    statuses: Vec::new(),
                    dead: false,
                    respawn_timer: 0.0,
                    team: 1,
                },
            );
        }
        if spec.unit_type == REIGN.unit_type {
            world.players.insert(
                2_600_000,
                PlayerCombatState {
                    uuid: "reign-pierce".into(),
                    player_id: 1_600_000,
                    unit_id: 2_600_000,
                    x: core_x + 50.0,
                    y: core_y,
                    health: 150.0,
                    shield: 0.0,
                    status_effect: -1,
                    status_duration: 0.0,
                    statuses: Vec::new(),
                    dead: false,
                    respawn_timer: 0.0,
                    team: 1,
                },
            );
        }
        world.enemies.insert(
            id,
            make_enemy(
                id,
                spec,
                core_x
                    + if spec.unit_type == MACE.unit_type {
                        40.0
                    } else {
                        100.0
                    },
                core_y,
                spec.health,
            ),
        );
        simulate_waves_and_enemies(&world, &connections, spec.attack_reload);
        assert_eq!(world.projectiles.len(), shots);
        assert!(world
            .projectiles
            .iter()
            .all(|projectile| projectile.team == 2 && projectile.bullet_id == bullet_id));
        if spec.unit_type == MACE.unit_type {
            let flame = world.projectiles.iter().next().unwrap();
            assert_eq!(flame.damage, 74.0);
            assert_eq!(flame.total_ticks, 40.0 / 4.2);
            assert_eq!(flame.status_effect, 1);
            assert_eq!(flame.status_duration, 300.0);
            assert_eq!(flame.pierce_units, 2);
            assert_eq!(flame.pierce_buildings, 2);
        }
        if spec.unit_type == ZENITH.unit_type {
            let mut lifetimes: Vec<_> = world
                .projectiles
                .iter()
                .map(|projectile| projectile.total_ticks)
                .collect();
            lifetimes.sort_unstable_by(f32::total_cmp);
            assert_ne!(lifetimes[0], lifetimes[1]);
            assert!(world
                .projectiles
                .iter()
                .all(|projectile| projectile.homing_range == 60.0));
        }
        assert!(simulate_projectiles(&world, &connections, 200.0));
        if spec.unit_type == MACE.unit_type {
            let player = world.players.get(&2_599_999).unwrap();
            assert_eq!(player.health, 76.0);
            assert_eq!(player.status_effect, 1);
            assert_eq!(player.status_duration, 300.0);
            drop(player);
            assert!(simulate_player_combat(&world, &connections, 10.0));
            let player = world.players.get(&2_599_999).unwrap();
            assert!((player.health - 74.33).abs() < 0.001);
            assert_eq!(player.status_duration, 290.0);
            drop(player);
            world.players.remove(&2_599_999);
        }
        if spec.unit_type == REIGN.unit_type {
            assert_eq!(world.players.get(&2_600_000).unwrap().health, 70.0);
            assert_eq!(world.projectiles.len(), 3);
            assert!(world.projectiles.iter().all(|projectile| {
                projectile.bullet_id == 13
                    && projectile.pierce_units == 3
                    && projectile.pierce_buildings == 3
            }));
            simulate_projectiles(&world, &connections, 20.0);
            world.players.remove(&2_600_000);
        }
        assert!(world.projectiles.is_empty());
        assert_eq!(*world.game_state.core_health.read(), 6000.0 - volley_damage);
        world.enemies.clear();
    }

    let pierce_building = ((i32::from(SPAWN_X) + 4) << 16) | i32::from(SPAWN_Y);
    world.base_buildings.insert(
        pierce_building,
        BaseBuildingState {
            position: pierce_building,
            block: 216,
            team: 1,
            health: 320.0,
            occupied: vec![pierce_building],
            inventory: Vec::new(),
        },
    );
    for (id, offset) in [(2_600_010, 20.0), (2_600_011, 10.0)] {
        world.players.insert(
            id,
            PlayerCombatState {
                uuid: format!("shared-pierce-{id}"),
                player_id: id - 1_000_000,
                unit_id: id,
                x: core_x + offset,
                y: core_y,
                health: 150.0,
                shield: 0.0,
                status_effect: -1,
                status_duration: 0.0,
                statuses: Vec::new(),
                dead: false,
                respawn_timer: 0.0,
                team: 1,
            },
        );
    }
    assert!(apply_enemy_shared_pierce_damage(
        &world,
        &connections,
        core_x + 40.0,
        core_y,
        core_x,
        core_y,
        74.0,
        2,
        1,
        300.0,
    ));
    assert_eq!(
        world.base_buildings.get(&pierce_building).unwrap().health,
        246.0
    );
    assert_eq!(world.players.get(&2_600_010).unwrap().health, 76.0);
    assert_eq!(world.players.get(&2_600_010).unwrap().status_effect, 1);
    assert_eq!(world.players.get(&2_600_011).unwrap().health, 150.0);
    world.base_buildings.clear();
    world.players.clear();

    world.players.insert(
        2_600_001,
        PlayerCombatState {
            uuid: "antumbra-splash".into(),
            player_id: 1_600_001,
            unit_id: 2_600_001,
            x: core_x,
            y: core_y + 18.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    world.enemies.insert(
        23,
        make_enemy(23, ANTUMBRA, core_x + 100.0, core_y, ANTUMBRA.health),
    );
    *world.game_state.core_health.write() = 6000.0;
    assert_eq!(enemy_weapon_mount_count(ANTUMBRA.unit_type), 3);
    simulate_waves_and_enemies(&world, &connections, 12.0);
    assert_eq!(world.projectiles.len(), 1);
    assert_eq!(world.projectiles.iter().next().unwrap().bullet_id, 34);
    simulate_waves_and_enemies(&world, &connections, 8.0);
    assert_eq!(world.projectiles.len(), 2);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 33)
            .count(),
        1
    );
    simulate_waves_and_enemies(&world, &connections, 15.0);
    assert_eq!(world.projectiles.len(), 4);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| {
                projectile.bullet_id == 33
                    && projectile.damage == 18.0
                    && projectile.splash_damage == 37.0
                    && projectile.splash_radius == 20.0
                    && projectile.status_effect == 18
                    && projectile.status_duration == 60.0
            })
            .count(),
        2
    );
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 34 && projectile.damage == 55.0)
            .count(),
        2
    );
    assert!(simulate_projectiles(&world, &connections, 200.0));
    assert!(world.projectiles.is_empty());
    assert_eq!(*world.game_state.core_health.read(), 6000.0 - 220.0);
    {
        let player = world.players.get(&2_600_001).unwrap();
        assert_eq!(player.health, 76.0);
        assert_eq!(player.status_effect, -1);
        assert_eq!(player.status_duration, 0.0);
    }
    world.players.remove(&2_600_001);
    world.enemies.clear();
    world.projectiles.clear();

    world.enemies.insert(
        24,
        make_enemy(24, RISSO, core_x + 100.0, core_y, RISSO.health),
    );
    assert_eq!(enemy_weapon_mount_count(RISSO.unit_type), 2);
    simulate_waves_and_enemies(&world, &connections, 25.0);
    assert_eq!(world.projectiles.len(), 2);
    assert!(world.projectiles.iter().any(|projectile| {
        projectile.bullet_id == 41 && projectile.damage == 9.0 && projectile.splash_damage == 0.0
    }));
    assert!(world.projectiles.iter().any(|projectile| {
        projectile.bullet_id == 42
            && projectile.damage == 12.0
            && projectile.splash_damage == 10.0
            && projectile.splash_radius == 25.0
    }));
    let risso = world.enemies.get(&24).unwrap();
    assert_eq!(risso.attack_reload, 12.0);
    assert_eq!(risso.secondary_attack_reload, 0.0);
    drop(risso);
    world.enemies.clear();
    world.projectiles.clear();

    for (unit_type, delta, expected) in [
        (26, 30.0, vec![(43, 3), (44, 1)]),
        (27, 65.0, vec![(45, 1), (46, 6)]),
        (28, 60.0, vec![(47, 6), (48, 3)]),
    ] {
        let spec = enemy_spec(unit_type).unwrap();
        world.enemies.insert(
            25 + i32::from(unit_type),
            make_enemy(
                25 + i32::from(unit_type),
                spec,
                core_x + 100.0,
                core_y,
                spec.health,
            ),
        );
        simulate_waves_and_enemies(&world, &connections, delta);
        for (bullet_id, count) in expected {
            assert_eq!(
                world
                    .projectiles
                    .iter()
                    .filter(|projectile| projectile.bullet_id == bullet_id)
                    .count(),
                count
            );
        }
        world.enemies.clear();
        world.projectiles.clear();
    }
    assert!(naval_weapon_volleys(26).is_some_and(|(_, (_, artillery))| {
        artillery.direct_damage == 20.0
            && artillery.splash_damage == 40.0
            && artillery.splash_radius == 22.5
    }));
    assert!(
        naval_weapon_volleys(27).is_some_and(|((_, artillery), (_, missiles))| {
            artillery.status_effect == 18
                && artillery.status_duration == 60.0
                && missiles.shots == 2
                && missiles.velocity_random == 0.1
        })
    );
    assert!(
        naval_weapon_volleys(28).is_some_and(|((_, launcher), (_, cannon))| {
            launcher.shots == 6
                && launcher.homing_range == 80.0
                && cannon.shots == 3
                && cannon.direct_damage == 57.0
        })
    );

    let omura = enemy_spec(29).unwrap();
    world.enemies.insert(
        60,
        make_enemy(60, omura, core_x + 400.0, core_y, omura.health),
    );
    for (id, offset) in [(2_700_001, 300.0), (2_700_002, 200.0)] {
        world.players.insert(
            id,
            PlayerCombatState {
                uuid: format!("omura-rail-{id}"),
                player_id: id - 1_000_000,
                unit_id: id,
                x: core_x + offset,
                y: core_y,
                health: 2000.0,
                shield: 0.0,
                status_effect: -1,
                status_duration: 0.0,
                statuses: Vec::new(),
                dead: false,
                respawn_timer: 0.0,
                team: 1,
            },
        );
    }
    *world.game_state.core_health.write() = 6000.0;
    simulate_waves_and_enemies(&world, &connections, 110.0);
    assert_eq!(world.projectiles.len(), 1);
    let rail = world.projectiles.iter().next().unwrap();
    assert_eq!(rail.bullet_id, 49);
    assert_eq!(rail.damage, 1250.0);
    assert_eq!(rail.target_x, core_x);
    drop(rail);
    assert!(simulate_projectiles(&world, &connections, 1.0));
    assert_eq!(world.players.get(&2_700_001).unwrap().health, 750.0);
    assert_eq!(world.players.get(&2_700_002).unwrap().health, 1375.0);
    assert_eq!(*world.game_state.core_health.read(), 5687.5);
    world.players.clear();
    world.enemies.clear();
    world.projectiles.clear();

    let oxynoe = enemy_spec(31).unwrap();
    world.enemies.insert(
        61,
        make_enemy(61, oxynoe, core_x + 50.0, core_y, oxynoe.health),
    );
    world.projectiles.insert(
        4_990_001,
        Projectile {
            target_id: 0,
            shooter_id: 0,
            team: 1,
            bullet_id: 6,
            damage: 30.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: true,
            armor_multiplier: 1.0,
            remaining_ticks: 20.0,
            total_ticks: 20.0,
            source_x: core_x + 60.0,
            source_y: core_y,
            target_x: core_x + 80.0,
            target_y: core_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    assert!(simulate_enemy_point_defense(&world, 9.0));
    assert_eq!(world.projectiles.get(&4_990_001).unwrap().damage, 13.0);
    assert!(simulate_enemy_point_defense(&world, 9.0));
    assert!(!world.projectiles.contains_key(&4_990_001));
    simulate_waves_and_enemies(&world, &connections, 5.0);
    let plasma = world
        .projectiles
        .iter()
        .find(|projectile| projectile.bullet_id == 53)
        .unwrap();
    assert_eq!(plasma.damage, 23.0);
    assert_eq!(plasma.status_effect, 1);
    assert_eq!(plasma.status_duration, 240.0);
    drop(plasma);
    world.enemies.clear();
    world.projectiles.clear();

    let aegires = enemy_spec(33).unwrap();
    world.enemies.insert(
        66,
        make_enemy(66, aegires, core_x + 50.0, core_y, aegires.health),
    );
    for projectile_id in [4_990_010, 4_990_011] {
        world.projectiles.insert(
            projectile_id,
            Projectile {
                target_id: 0,
                shooter_id: 0,
                team: 1,
                bullet_id: 6,
                damage: 20.0,
                splash_damage: 0.0,
                splash_radius: 0.0,
                status_effect: -1,
                status_duration: 0.0,
                pierce_units: 0,
                pierce_buildings: 0,
                spawn_reign_frags: false,
                homing_range: 0.0,
                enemy_target_position: None,
                enemy_target_core: false,
                apply_direct_on_impact: true,
                armor_multiplier: 1.0,
                remaining_ticks: 20.0,
                total_ticks: 20.0,
                source_x: core_x + 60.0,
                source_y: core_y,
                target_x: core_x + 80.0,
                target_y: core_y,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
    }
    assert!(simulate_enemy_point_defense(&world, 4.0));
    assert!(!world.projectiles.contains_key(&4_990_010));
    assert!(!world.projectiles.contains_key(&4_990_011));
    world
        .enemies
        .insert(67, make_enemy(67, DAGGER, core_x + 55.0, core_y, 100.0));
    let mut field_enemy = make_enemy(68, enemy_spec(21).unwrap(), core_x + 70.0, core_y, 400.0);
    field_enemy.team = 1;
    world.enemies.insert(68, field_enemy);
    world.players.insert(
        2_700_100,
        PlayerCombatState {
            uuid: "aegires-field-target".into(),
            player_id: 1_700_100,
            unit_id: 2_700_100,
            x: core_x + 60.0,
            y: core_y,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    *world.game_state.core_health.write() = 6000.0;
    assert!(simulate_aegires_energy_fields(&world, &connections, 65.0));
    assert_eq!(world.enemies.get(&67).unwrap().health, 102.25);
    let field_target = world.players.get(&2_700_100).unwrap();
    assert_eq!(field_target.health, 110.0);
    assert_eq!(field_target.status_effect, 10);
    assert_eq!(field_target.status_duration, 360.0);
    drop(field_target);
    assert_eq!(*world.game_state.core_health.read(), 5960.0);
    let electrified = world.enemies.get(&68).unwrap();
    assert_eq!(electrified.health, 360.0);
    assert_eq!(electrified.status_effect, 10);
    assert_eq!(electrified.status_duration, 360.0);
    assert!((effective_unit_speed(&electrified) - 1.82).abs() < 0.0001);
    assert_eq!(effective_unit_reload_delta(&electrified, 10.0), 6.0);
    drop(electrified);
    assert!(simulate_enemy_statuses(&world, &connections, 60.0));
    assert_eq!(world.enemies.get(&68).unwrap().status_duration, 300.0);
    assert!(simulate_enemy_statuses(&world, &connections, 300.0));
    assert_eq!(world.enemies.get(&68).unwrap().status_effect, -1);
    assert_eq!(world.enemies.get(&68).unwrap().status_duration, 0.0);
    world.players.clear();
    world.enemies.clear();

    world.enemies.insert(
        62,
        make_enemy(62, RETUSA, core_x + 50.0, core_y, RETUSA.health),
    );
    world
        .enemies
        .insert(63, make_enemy(63, DAGGER, core_x + 55.0, core_y, 100.0));
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&63).unwrap().health, 107.5);
    world.enemies.get_mut(&63).unwrap().health = DAGGER.health;
    let repair_position = ((i32::from(SPAWN_X) + 7) << 16) | i32::from(SPAWN_Y);
    let mut repair_wall = base_building_tombstone(&BaseBuildingState {
        position: repair_position,
        block: 216,
        team: 2,
        health: 300.0,
        occupied: vec![repair_position],
        inventory: Vec::new(),
    });
    repair_wall.block = 216;
    repair_wall.team = 2;
    repair_wall.health = 300.0;
    world.tiles.insert(repair_position, repair_wall);
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.tiles.get(&repair_position).unwrap().health, 307.5);
    world.tiles.remove(&repair_position);
    simulate_waves_and_enemies(&world, &connections, 89.0);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 51)
            .count(),
        4
    );
    assert!(!world
        .projectiles
        .iter()
        .any(|projectile| projectile.bullet_id == 52));
    simulate_waves_and_enemies(&world, &connections, 1.0);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 52)
            .count(),
        1
    );
    simulate_waves_and_enemies(&world, &connections, 7.0);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 52)
            .count(),
        2
    );
    simulate_waves_and_enemies(&world, &connections, 7.0);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 52)
            .count(),
        3
    );
    let mine = world
        .projectiles
        .iter()
        .find(|projectile| projectile.bullet_id == 52)
        .unwrap();
    assert_eq!(mine.splash_damage, 40.0);
    assert_eq!(mine.splash_radius, 32.0);
    drop(mine);
    world.enemies.clear();
    world.projectiles.clear();

    let cyerce = enemy_spec(32).unwrap();
    world.enemies.insert(
        64,
        make_enemy(64, cyerce, core_x + 100.0, core_y, cyerce.health),
    );
    world
        .enemies
        .insert(65, make_enemy(65, DAGGER, core_x + 105.0, core_y, 100.0));
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&65).unwrap().health, 107.0);
    simulate_waves_and_enemies(&world, &connections, 60.0);
    let missile = world
        .projectiles
        .iter()
        .find(|projectile| projectile.bullet_id == 56)
        .unwrap();
    assert_eq!(missile.damage, 25.0);
    assert_eq!(missile.splash_damage, 25.0);
    assert_eq!(missile.splash_radius, 30.0);
    let travel_ticks = missile.remaining_ticks;
    drop(missile);
    assert!(simulate_projectiles(&world, &connections, travel_ticks));
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 57)
            .count(),
        7
    );
    assert!(world
        .projectiles
        .iter()
        .filter(|projectile| projectile.bullet_id == 57)
        .all(|fragment| fragment.damage == 11.0
            && fragment.splash_damage == 13.0
            && fragment.splash_radius == 20.0));
    world.enemies.clear();
    world.projectiles.clear();

    let navanax = enemy_spec(34).unwrap();
    world.enemies.insert(
        69,
        make_enemy(69, navanax, core_x + 80.0, core_y, navanax.health),
    );
    world.players.insert(
        2_700_200,
        PlayerCombatState {
            uuid: "navanax-emp-target".into(),
            player_id: 1_700_200,
            unit_id: 2_700_200,
            x: core_x,
            y: core_y + 10.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    *world.game_state.core_health.write() = 6000.0;
    simulate_waves_and_enemies(&world, &connections, 170.0);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 60)
            .count(),
        2
    );
    for bullet_id in 61..=64 {
        assert_eq!(
            world
                .projectiles
                .iter()
                .filter(|projectile| projectile.bullet_id == bullet_id)
                .count(),
            1
        );
    }
    assert!(simulate_projectiles(&world, &connections, 5.0));
    assert_eq!(*world.game_state.core_health.read(), 5892.0);
    let lasers: Vec<_> = world
        .projectiles
        .iter()
        .filter(|projectile| (61..=64).contains(&projectile.bullet_id))
        .map(|projectile| *projectile.key())
        .collect();
    for id in lasers {
        world.projectiles.remove(&id);
    }
    assert!(simulate_projectiles(&world, &connections, 20.0));
    assert_eq!(*world.game_state.core_health.read(), 5632.0);
    let emp_target = world.players.get(&2_700_200).unwrap();
    assert_eq!(emp_target.health, 38.0);
    assert_eq!(emp_target.status_effect, 10);
    assert_eq!(emp_target.status_duration, 480.0);
    drop(emp_target);
    world.players.clear();
    world.enemies.clear();
    world.projectiles.clear();

    world
        .enemies
        .insert(70, make_enemy(70, DAGGER, core_x + 50.0, core_y, 100.0));
    world
        .enemies
        .insert(71, make_enemy(71, DAGGER, core_x + 55.0, core_y, 100.0));
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        70,
        None,
        enemy_projectile_volley(32).unwrap(),
        core_x,
        core_y,
        core_x + 50.0,
        core_y,
        0,
    );
    assert!(simulate_projectiles(&world, &connections, 100.0));
    assert_eq!(world.enemies.get(&70).unwrap().health, 50.0);
    assert_eq!(world.enemies.get(&71).unwrap().health, 75.0);
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 57)
            .count(),
        7
    );
    world.projectiles.clear();
    world.enemies.get_mut(&70).unwrap().health = 100.0;
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        70,
        None,
        enemy_projectile_volley(31).unwrap(),
        core_x,
        core_y,
        core_x + 50.0,
        core_y,
        0,
    );
    assert!(simulate_projectiles(&world, &connections, 100.0));
    let burning = world.enemies.get(&70).unwrap();
    assert_eq!(burning.health, 77.0);
    assert_eq!(burning.status_effect, 1);
    assert_eq!(burning.status_duration, 240.0);
    drop(burning);
    assert!(simulate_enemy_statuses(&world, &connections, 10.0));
    assert!((world.enemies.get(&70).unwrap().health - 75.33).abs() < 0.001);
    assert_eq!(world.enemies.get(&70).unwrap().status_duration, 230.0);
    world.enemies.clear();
    world.projectiles.clear();
    for (id, x, y) in [
        (80, core_x + 100.0, core_y),
        (81, core_x + 200.0, core_y),
        (82, core_x + 300.0, core_y),
        (83, core_x + 200.0, core_y + 20.0),
    ] {
        world
            .enemies
            .insert(id, make_enemy(id, DAGGER, x, y, 2_000.0));
    }
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        80,
        None,
        OMURA_RAIL,
        core_x,
        core_y,
        core_x + 100.0,
        core_y,
        0,
    );
    assert!(simulate_projectiles(&world, &connections, 2.0));
    assert_eq!(world.enemies.get(&80).unwrap().health, 750.0);
    assert_eq!(world.enemies.get(&81).unwrap().health, 1_375.0);
    assert_eq!(world.enemies.get(&82).unwrap().health, 1_687.5);
    assert_eq!(world.enemies.get(&83).unwrap().health, 2_000.0);
    world.enemies.clear();
    world.projectiles.clear();
    let projectile_building_y = core_y + 80.0;
    let projectile_building_position = (i32::from((core_x / 8.0) as i16 + 10) << 16)
        | (i32::from((projectile_building_y / 8.0) as i16) & 0xffff);
    world.base_buildings.insert(
        projectile_building_position,
        BaseBuildingState {
            position: projectile_building_position,
            block: 216,
            team: 2,
            health: 100.0,
            occupied: Vec::new(),
            inventory: Vec::new(),
        },
    );
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        -1,
        Some(projectile_building_position),
        enemy_projectile_volley(DAGGER.unit_type).unwrap(),
        core_x,
        projectile_building_y,
        core_x + 80.0,
        projectile_building_y,
        0,
    );
    assert_eq!(
        world
            .base_buildings
            .get(&projectile_building_position)
            .unwrap()
            .health,
        100.0
    );
    assert!(simulate_projectiles(&world, &connections, 100.0));
    assert_eq!(
        world
            .base_buildings
            .get(&projectile_building_position)
            .unwrap()
            .health,
        91.0
    );
    world
        .base_buildings
        .get_mut(&projectile_building_position)
        .unwrap()
        .health = 100.0;
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        -1,
        Some(projectile_building_position),
        enemy_projectile_volley(32).unwrap(),
        core_x,
        projectile_building_y,
        core_x + 80.0,
        projectile_building_y,
        0,
    );
    assert!(simulate_projectiles(&world, &connections, 100.0));
    assert_eq!(
        world
            .base_buildings
            .get(&projectile_building_position)
            .unwrap()
            .health,
        50.0
    );
    world.projectiles.clear();
    world
        .base_buildings
        .get_mut(&projectile_building_position)
        .unwrap()
        .health = 200.0;
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        -1,
        Some(projectile_building_position),
        enemy_projectile_volley(4).unwrap(),
        core_x,
        projectile_building_y,
        core_x + 80.0,
        projectile_building_y,
        0,
    );
    assert!(simulate_projectiles(&world, &connections, 100.0));
    assert_eq!(
        world
            .base_buildings
            .get(&projectile_building_position)
            .unwrap()
            .health,
        102.0
    );
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.team == 1 && projectile.bullet_id == 13)
            .count(),
        3
    );
    world.projectiles.clear();
    let repair_maximum = crate::game::content::block_health(216);
    {
        let mut repair_building = world
            .base_buildings
            .get_mut(&projectile_building_position)
            .unwrap();
        repair_building.team = 1;
        repair_building.health = 100.0;
    }
    let mut allied_oxynoe = make_enemy(
        84,
        enemy_spec(31).unwrap(),
        core_x + 40.0,
        projectile_building_y,
        enemy_spec(31).unwrap().health,
    );
    allied_oxynoe.team = 1;
    world.enemies.insert(84, allied_oxynoe);
    let mut overclock_target = make_enemy(
        85,
        DAGGER,
        core_x + 45.0,
        projectile_building_y,
        DAGGER.health,
    );
    overclock_target.team = 1;
    world.enemies.insert(85, overclock_target);
    *world.game_state.simulation_time.write() = 360.0;
    apply_enemy_support_abilities(&world, &connections, 1.0);
    let boosted = world.enemies.get(&85).unwrap();
    assert_eq!(boosted.status_effect, 14);
    assert_eq!(boosted.status_duration, 360.0);
    assert!((effective_unit_speed(&boosted) - DAGGER.speed * 1.15).abs() < 0.001);
    assert!((effective_unit_reload_delta(&boosted, 4.0) - 5.0).abs() < 0.001);
    drop(boosted);
    assert!(simulate_allied_units(&world, &connections, 5.0));
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.team == 1 && projectile.bullet_id == 53)
            .count(),
        1
    );
    assert!(simulate_projectiles(&world, &connections, 100.0));
    assert_eq!(
        world
            .base_buildings
            .get(&projectile_building_position)
            .unwrap()
            .health,
        (100.0 + repair_maximum * 0.015).min(repair_maximum)
    );
    world.enemies.remove(&84);
    world.enemies.remove(&85);
    world.projectiles.clear();
    world.base_buildings.remove(&projectile_building_position);

    let rail_y = core_y + 40.0;
    let rail_tile_y = (rail_y / 8.0) as i16;
    let first_rail_position =
        (i32::from((core_x / 8.0) as i16 + 10) << 16) | (i32::from(rail_tile_y) & 0xffff);
    let second_rail_position =
        (i32::from((core_x / 8.0) as i16 + 20) << 16) | (i32::from(rail_tile_y) & 0xffff);
    for position in [first_rail_position, second_rail_position] {
        world.base_buildings.insert(
            position,
            BaseBuildingState {
                position,
                block: 67,
                team: 2,
                health: 2_000.0,
                occupied: Vec::new(),
                inventory: Vec::new(),
            },
        );
    }
    assert!(apply_allied_pierce_damage(
        &world,
        &connections,
        core_x,
        rail_y,
        core_x + 500.0,
        rail_y,
        1_250.0,
        u8::MAX,
        true,
        0.5,
        -1,
        0.0,
    ));
    assert_eq!(
        world
            .base_buildings
            .get(&first_rail_position)
            .unwrap()
            .health,
        750.0
    );
    assert_eq!(
        world
            .base_buildings
            .get(&second_rail_position)
            .unwrap()
            .health,
        1_375.0
    );
    world.base_buildings.remove(&first_rail_position);
    world.base_buildings.remove(&second_rail_position);

    world.enemies.clear();
    world.projectiles.clear();
    let mut allied_antumbra = make_enemy(90, ANTUMBRA, core_x, core_y, ANTUMBRA.health);
    allied_antumbra.team = 1;
    world.enemies.insert(90, allied_antumbra);
    world
        .enemies
        .insert(91, make_enemy(91, DAGGER, core_x + 40.0, core_y, 20_000.0));
    assert!(simulate_allied_units(&world, &connections, 140.0));
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.team == 1 && projectile.bullet_id == 33)
            .count(),
        11
    );
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.team == 1 && projectile.bullet_id == 34)
            .count(),
        11
    );
    world.enemies.clear();
    world.projectiles.clear();

    let original_target = ((i32::from(SPAWN_X) + 2) << 16) | i32::from(SPAWN_Y);
    let homing_target = ((i32::from(SPAWN_X) + 7) << 16) | i32::from(SPAWN_Y);
    for position in [original_target, homing_target] {
        world.base_buildings.insert(
            position,
            BaseBuildingState {
                position,
                block: 216,
                team: 1,
                health: 320.0,
                occupied: vec![position],
                inventory: Vec::new(),
            },
        );
    }
    let zenith_volley = enemy_projectile_volley(ZENITH.unit_type).unwrap();
    spawn_enemy_projectile(
        &world,
        &connections,
        99,
        Some(original_target),
        false,
        zenith_volley,
        core_x + 100.0,
        core_y,
        core_x + 16.0,
        core_y,
        0,
    );
    world.base_buildings.remove(&original_target);
    assert!(simulate_projectiles(&world, &connections, 200.0));
    assert_eq!(
        world.base_buildings.get(&homing_target).unwrap().health,
        320.0 - 14.0 - 15.0
    );

    let force_position = ((i32::from(SPAWN_X) + 10) << 16) | i32::from(SPAWN_Y);
    let mut force = base_building_tombstone(&BaseBuildingState {
        position: force_position,
        block: FORCE_PROJECTOR_BLOCK,
        team: 1,
        health: crate::game::content::block_health(FORCE_PROJECTOR_BLOCK),
        occupied: block_footprint_in(
            world.width,
            world.height,
            force_position,
            FORCE_PROJECTOR_BLOCK,
        )
        .unwrap(),
        inventory: Vec::new(),
    });
    force.block = FORCE_PROJECTOR_BLOCK;
    force.team = 1;
    force.health = crate::game::content::block_health(FORCE_PROJECTOR_BLOCK);
    force.transport_progress = 1.0;
    force.ammo_units = 1.0;
    force.config = vec![0];
    world.tiles.insert(force_position, force);
    *world.game_state.core_health.write() = 6_000.0;
    world.projectiles.insert(
        4_900_000,
        Projectile {
            target_id: 99,
            shooter_id: 0,
            team: 2,
            bullet_id: 31,
            damage: 100.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: true,
            apply_direct_on_impact: true,
            armor_multiplier: 1.0,
            remaining_ticks: 20.0,
            total_ticks: 20.0,
            source_x: core_x + 200.0,
            source_y: core_y,
            target_x: core_x,
            target_y: core_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    assert!(simulate_projectiles(&world, &connections, 20.0));
    assert!(world.projectiles.is_empty());
    assert_eq!(*world.game_state.core_health.read(), 6_000.0);
    assert_eq!(
        world
            .tiles
            .get(&force_position)
            .unwrap()
            .production_progress,
        100.0
    );

    let shockwave_position = ((i32::from(SPAWN_X) + 30) << 16) | i32::from(SPAWN_Y);
    let mut shockwave = base_building_tombstone(&BaseBuildingState {
        position: shockwave_position,
        block: SHOCKWAVE_TOWER_BLOCK,
        team: 1,
        health: crate::game::content::block_health(SHOCKWAVE_TOWER_BLOCK),
        occupied: block_footprint_in(
            world.width,
            world.height,
            shockwave_position,
            SHOCKWAVE_TOWER_BLOCK,
        )
        .unwrap(),
        inventory: Vec::new(),
    });
    shockwave.block = SHOCKWAVE_TOWER_BLOCK;
    shockwave.team = 1;
    shockwave.health = crate::game::content::block_health(SHOCKWAVE_TOWER_BLOCK);
    shockwave.production_progress = 74.0;
    shockwave.stored_liquid = CYANOGEN_LIQUID;
    shockwave.liquid_amount = 15.0;
    world.tiles.insert(shockwave_position, shockwave);
    world.projectiles.insert(
        4_900_001,
        Projectile {
            target_id: 99,
            shooter_id: 0,
            team: 2,
            bullet_id: 34,
            damage: 55.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: true,
            apply_direct_on_impact: true,
            armor_multiplier: 1.0,
            remaining_ticks: 10.0,
            total_ticks: 20.0,
            source_x: (i32::from(SPAWN_X) as f32 + 30.0) * 8.0 + 100.0,
            source_y: core_y,
            target_x: (i32::from(SPAWN_X) as f32 + 30.0) * 8.0,
            target_y: core_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    let mut shockwave_power = std::collections::HashMap::new();
    shockwave_power.insert(shockwave_position, 1.0);
    assert!(simulate_shockwave_towers(&world, 6.0, &shockwave_power));
    assert!(!world.projectiles.contains_key(&4_900_001));
    let shockwave = world.tiles.get(&shockwave_position).unwrap();
    assert_eq!(shockwave.production_progress, 0.0);
    assert!((shockwave.liquid_amount - 14.85).abs() < 0.0001);
    drop(shockwave);

    let mine_position = ((i32::from(SPAWN_X) + 40) << 16) | i32::from(SPAWN_Y);
    let mut mine = base_building_tombstone(&BaseBuildingState {
        position: mine_position,
        block: SHOCK_MINE_BLOCK,
        team: 1,
        health: crate::game::content::block_health(SHOCK_MINE_BLOCK),
        occupied: vec![mine_position],
        inventory: Vec::new(),
    });
    mine.block = SHOCK_MINE_BLOCK;
    mine.team = 1;
    mine.health = crate::game::content::block_health(SHOCK_MINE_BLOCK);
    world.tiles.insert(mine_position, mine);
    let mine_x = (i32::from(SPAWN_X) as f32 + 40.0) * 8.0;
    world
        .enemies
        .insert(101, make_enemy(101, DAGGER, mine_x, core_y, 150.0));
    assert!(simulate_shock_mines(&world, &connections, 6.0));
    assert_eq!(world.enemies.get(&101).unwrap().health, 50.0);
    let mine = world.tiles.get(&mine_position).unwrap();
    assert_eq!(mine.health, 43.0);
    assert_eq!(mine.production_progress, 80.0);
    drop(mine);
    assert!(simulate_shock_mines(&world, &connections, 6.0));
    assert_eq!(world.enemies.get(&101).unwrap().health, 50.0);
    assert_eq!(
        world.tiles.get(&mine_position).unwrap().production_progress,
        74.0
    );
}

#[test]
fn navanax_suppression_blocks_allied_healing() {
    let _connections: DashMap<i32, PendingConnection> = DashMap::new();
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("enemy-role-test".into(), GameMode::Survival);
    *state.wave_time.write() = 10_000.0;
    *state.simulation_time.write() = 240.0;
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_100),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-enemy-role-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let core_x = (i32::from(SPAWN_X) as f32) * 8.0;
    let core_y = (i32::from(SPAWN_Y) as f32) * 8.0;
    let make_enemy = |id: i32, spec: EnemySpec, x: f32, y: f32, health: f32| EnemyUnit {
        id,
        unit_type: spec.unit_type,
        entity_class: spec.entity_class,
        team: 2,
        x,
        y,
        rotation: 0.0,
        health,
        shield: 0.0,
        status_effect: -1,
        status_duration: f32::MAX,
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
        move_speed: spec.speed,
        attack_damage: spec.attack_damage,
        attack_reload_time: spec.attack_reload,
        attack_range: spec.attack_range,
        authority: UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        status_agg: None,
    };
    let suppressible = |position: i32, block: i16, team: u8, health: f32| {
        let mut tile = base_building_tombstone(&BaseBuildingState {
            position,
            block,
            team,
            health,
            occupied: Vec::new(),
            inventory: Vec::new(),
        });
        tile.block = block;
        tile.team = team;
        tile.health = health;
        world.tiles.insert(position, tile);
    };
    let mend_position = ((i32::from(SPAWN_X) + 15) << 16) | i32::from(SPAWN_Y);
    let wall_position = ((i32::from(SPAWN_X) + 18) << 16) | i32::from(SPAWN_Y);
    let far_mend_position = ((i32::from(SPAWN_X) + 60) << 16) | i32::from(SPAWN_Y);
    suppressible(
        mend_position,
        246,
        1,
        crate::game::content::block_health(246) * 0.5,
    );
    suppressible(
        wall_position,
        216,
        1,
        crate::game::content::block_health(216) * 0.5,
    );
    suppressible(
        far_mend_position,
        246,
        1,
        crate::game::content::block_health(246) * 0.5,
    );
    world.enemies.insert(
        51,
        make_enemy(51, enemy_spec(34).unwrap(), core_x + 96.0, core_y, 20_000.0),
    );
    assert!(simulate_navanax_suppression(&world, 90.0));
    assert!(world.heal_suppression.contains_key(&mend_position));
    assert!(world.heal_suppression.contains_key(&wall_position));
    assert!(!world.heal_suppression.contains_key(&far_mend_position));
    assert_eq!(
        heal_building_for_team(&world, mend_position, 1, 50.0, 0.0),
        None
    );
    assert!(heal_building_for_team(&world, wall_position, 1, 50.0, 0.0).is_some());
    assert!(heal_building_for_team(&world, far_mend_position, 1, 50.0, 0.0).is_some());
    assert!(simulate_navanax_suppression(&world, 90.0));
    assert!(world.heal_suppression.contains_key(&mend_position));
    world.enemies.remove(&51);
    assert!(simulate_navanax_suppression(&world, 91.0));
    assert!(!world.heal_suppression.contains_key(&mend_position));
    assert!(heal_building_for_team(&world, mend_position, 1, 50.0, 0.0).is_some());
}

#[test]
fn emp_bullet_heals_boosts_and_strikes_power_buildings() {
    let _connections: DashMap<i32, PendingConnection> = DashMap::new();
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("enemy-role-test".into(), GameMode::Survival);
    *state.wave_time.write() = 10_000.0;
    *state.simulation_time.write() = 240.0;
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_100),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-enemy-role-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let core_x = (i32::from(SPAWN_X) as f32) * 8.0;
    let core_y = (i32::from(SPAWN_Y) as f32) * 8.0;
    let insert_building = |position: i32, block: i16, team: u8, health: f32| {
        let mut tile = base_building_tombstone(&BaseBuildingState {
            position,
            block,
            team,
            health,
            occupied: Vec::new(),
            inventory: Vec::new(),
        });
        tile.block = block;
        tile.team = team;
        tile.health = health;
        world.tiles.insert(position, tile);
    };
    let allied_generator = ((i32::from(SPAWN_X) + 5) << 16) | i32::from(SPAWN_Y);
    let enemy_generator = ((i32::from(SPAWN_X) + 7) << 16) | i32::from(SPAWN_Y);
    let enemy_wall = ((i32::from(SPAWN_X) + 9) << 16) | i32::from(SPAWN_Y);
    let far_generator = ((i32::from(SPAWN_X) + 30) << 16) | i32::from(SPAWN_Y);
    let generator_max = crate::game::content::block_health(308);
    let wall_max = crate::game::content::block_health(216);
    insert_building(allied_generator, 308, 1, generator_max * 0.5);
    insert_building(enemy_generator, 308, 2, generator_max);
    insert_building(enemy_wall, 216, 2, wall_max);
    insert_building(far_generator, 308, 1, generator_max * 0.5);
    let emp_x = core_x + 48.0;
    let emp_y = core_y;
    assert!(apply_emp_bullet_effects(
        &world,
        &_connections,
        1,
        emp_x,
        emp_y,
        100.0,
        60.0,
    ));
    assert!((building_time_scale(&world, allied_generator) - 3.0).abs() < 0.001);
    assert_eq!(
        world.tiles.get(&allied_generator).unwrap().health,
        generator_max * 0.7
    );
    assert_eq!(
        world.tiles.get(&enemy_generator).unwrap().health,
        (generator_max - 180.0).max(0.0)
    );
    assert_eq!(world.tiles.get(&enemy_wall).unwrap().health, wall_max);
    assert_eq!(
        world.tiles.get(&far_generator).unwrap().health,
        generator_max * 0.5
    );
    assert!((building_time_scale(&world, far_generator) - 1.0).abs() < 0.001);
    assert_eq!(
        world
            .overdrive_boosts
            .get(&allied_generator)
            .unwrap()
            .remaining_ticks,
        1200.0
    );
}

#[test]
fn ground_enemies_damage_and_remove_route_buildings() {
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("building-target-test".into(), GameMode::Survival);
    *state.wave_time.write() = 10_000.0;
    let enemy_spawns = map.enemy_spawns();
    let width = i32::from(map.width);
    let height = i32::from(map.height);
    let total = (width * height) as usize;
    let world = DynamicWorld {
        game_state: state,
        width,
        height,
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: vec![0; total],
        base_centers: vec![true; total],
        tile_data: vec![0; total],
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: vec![0; total],
        overlays: vec![0; total],
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_100),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-building-target-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let core_x = SPAWN_X as f32 * 8.0;
    let core_y = SPAWN_Y as f32 * 8.0;
    let wall_x = i32::from(SPAWN_X) + 5;
    for wall_y in 0..world.height {
        let wall_position = (wall_x << 16) | wall_y;
        world.tiles.insert(
            wall_position,
            DynamicTile {
                position: wall_position,
                block: 216, // copper wall: 320 health in the official manifest
                rotation: 0,
                team: 1,
                config: Vec::new(),
                enabled: true,
                message: None,
                occupied: vec![wall_position],
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
                health: crate::game::content::block_health(216),
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
            },
        );
    }
    world.enemies.insert(
        1,
        EnemyUnit {
            id: 1,
            unit_type: DAGGER.unit_type,
            entity_class: DAGGER.entity_class,
            team: 2,
            x: core_x + 100.0,
            y: core_y,
            rotation: 180.0,
            health: DAGGER.health,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );

    let (_, destroyed, health_updates) =
        simulate_waves_and_enemies(&world, &DashMap::new(), DAGGER.attack_reload);
    assert!(destroyed.is_empty());
    assert!(health_updates.is_empty());
    assert_eq!(world.projectiles.len(), 1);
    let projectile = world.projectiles.iter().next().unwrap();
    assert_eq!(projectile.bullet_id, 6);
    let attacked_position = projectile.enemy_target_position.unwrap();
    drop(projectile);
    assert!(simulate_projectiles(&world, &DashMap::new(), 26.0));
    assert_eq!((attacked_position >> 16) as i16 as i32, wall_x);
    // Official in-flight collision: the dagger's bullet (slightly
    // randomized angle) hits the first wall of the column its segment
    // crosses — the originally-targeted wall or a neighbour in the same
    // column. Either way one wall in column wall_x must be damaged and
    // the bullet consumed.
    assert!(world.projectiles.is_empty(), "bullet consumed by collision");
    let damaged = (0..world.height)
        .map(|wall_y| (wall_x << 16) | wall_y)
        .filter(|pos| {
            world
                .tiles
                .get(pos)
                .is_some_and(|tile| tile.block != 0 && tile.health < 320.0)
        })
        .count();
    assert!(damaged >= 1, "no wall in column {wall_x} took the hit");
    let hit_pos = (0..world.height)
        .map(|wall_y| (wall_x << 16) | wall_y)
        .find(|pos| {
            world
                .tiles
                .get(pos)
                .is_some_and(|tile| tile.block != 0 && tile.health < 320.0)
        })
        .unwrap();
    let hit_health = world.tiles.get(&hit_pos).unwrap().health;
    let frame = encode_build_health_update_frame(&[(hit_pos, hit_health)]).unwrap();
    let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    let mut payload = std::io::Cursor::new(&packet[1..]);
    assert_eq!(packet[0], BUILD_HEALTH_UPDATE_PACKET_ID);
    assert_eq!(
        crate::network::codec::Reads::read_i(&mut payload).unwrap(),
        2
    );
    assert_eq!(
        crate::network::codec::Reads::read_i(&mut payload).unwrap(),
        hit_pos
    );
    assert_eq!(
        f32::from_bits(crate::network::codec::Reads::read_i(&mut payload).unwrap() as u32),
        311.0
    );
    // Official in-flight collision (STATUS round 24): the dagger's bullet
    // (slight inaccuracy) hits the first wall of the column its segment
    // crosses — the originally-targeted wall OR a neighbour. When the
    // segment lands on a neighbour, the targeted wall stays at full
    // health (320) and the 9-damage hit lands on the neighbour (311).
    // This is nondeterministic at HEAD (DashMap shard order in the
    // building-collision scan), so accept either outcome.
    assert!(
        world.tiles.get(&attacked_position).unwrap().health == 311.0
            || world.tiles.get(&attacked_position).unwrap().health == 320.0,
        "targeted wall {} must be hit (311) or skipped (320), got {}",
        attacked_position,
        world.tiles.get(&attacked_position).unwrap().health
    );
    assert_eq!(*world.game_state.core_health.read(), 6000.0);

    // Continue the destroy-and-reroute phase with the wall that ACTUALLY
    // took the hit (hit_pos): with the official inaccuracy the bullet may
    // have landed on a neighbour of the originally-targeted wall, so the
    // wall to weaken and destroy is the one that ended at 311 HP.
    world.tiles.get_mut(&hit_pos).unwrap().health = 5.0;
    world.navigation_revision.fetch_add(1, Ordering::Relaxed);
    let (_, destroyed, health_updates) =
        simulate_waves_and_enemies(&world, &DashMap::new(), DAGGER.attack_reload);
    assert!(destroyed.is_empty());
    assert!(health_updates.is_empty());
    assert_eq!(world.projectiles.len(), 1);
    assert!(simulate_projectiles(&world, &DashMap::new(), 26.0));
    assert!(!world
        .projectiles
        .iter()
        .any(|projectile| { projectile.enemy_target_position == Some(hit_pos) }));
    let frame = encode_build_destroyed_frame(hit_pos).unwrap();
    let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    assert_eq!(packet[0], BUILD_DESTROYED_PACKET_ID);
    assert_eq!(
        crate::network::codec::Reads::read_i(&mut std::io::Cursor::new(&packet[1..])).unwrap(),
        hit_pos
    );
    let tombstone = world.tiles.get(&hit_pos).unwrap();
    assert_eq!(tombstone.block, 0);
    assert_eq!(tombstone.stored_amount, 217);
    drop(tombstone);
    assert_eq!(*world.game_state.core_health.read(), 6000.0);
    let avoidance = unit_avoidance_requests(&world);
    let enemy = world.enemies.get(&1).unwrap();
    let navigation = enemy_navigation_target(&world, &enemy, core_x, core_y, &avoidance);
    assert!(
        navigation.building.is_none(),
        "the rebuilt field must route through the destroyed wall's gap"
    );
    drop(enemy);

    world.tiles.clear();
    for wall_y in 0..world.height {
        let position = (wall_x << 16) | wall_y;
        world.base_buildings.insert(
            position,
            BaseBuildingState {
                position,
                block: 216,
                team: 1,
                health: crate::game::content::block_health(216),
                occupied: vec![position],
                inventory: Vec::new(),
            },
        );
    }
    {
        let mut enemy = world.enemies.get_mut(&1).unwrap();
        enemy.x = core_x + 100.0;
        enemy.y = core_y;
        enemy.attack_reload = 0.0;
    }
    world.navigation_revision.fetch_add(1, Ordering::Relaxed);
    let (_, destroyed, health_updates) =
        simulate_waves_and_enemies(&world, &DashMap::new(), DAGGER.attack_reload);
    assert!(destroyed.is_empty());
    assert!(health_updates.is_empty());
    let base_position = world
        .projectiles
        .iter()
        .next()
        .unwrap()
        .enemy_target_position
        .unwrap();
    assert!(simulate_projectiles(&world, &DashMap::new(), 26.0));
    // Same official in-flight collision rule as the dynamic-wall phase:
    // the bullet may land on a neighbour of the targeted base building.
    let base_hit = (0..world.height)
        .map(|wall_y| (wall_x << 16) | wall_y)
        .find(|pos| {
            world
                .base_buildings
                .get(pos)
                .is_some_and(|building| building.health < 320.0)
        })
        .unwrap();
    assert_eq!(world.base_buildings.get(&base_hit).unwrap().health, 311.0);
    assert!(
        world.base_buildings.get(&base_position).unwrap().health == 311.0
            || world.base_buildings.get(&base_position).unwrap().health == 320.0
    );

    world.base_buildings.get_mut(&base_position).unwrap().health = 5.0;
    world.navigation_revision.fetch_add(1, Ordering::Relaxed);
    let (_, destroyed, _) =
        simulate_waves_and_enemies(&world, &DashMap::new(), DAGGER.attack_reload);
    assert!(destroyed.is_empty());
    assert!(simulate_projectiles(&world, &DashMap::new(), 26.0));
    assert!(!world.base_buildings.contains_key(&base_position));
    assert_eq!(world.tiles.get(&base_position).unwrap().block, 0);
    world.base_buildings.clear();
    world.tiles.clear();
    for wall_y in 0..world.height {
        let position = (wall_x << 16) | wall_y;
        world.base_buildings.insert(
            position,
            BaseBuildingState {
                position,
                block: 216,
                team: 2,
                health: 320.0,
                occupied: vec![position],
                inventory: Vec::new(),
            },
        );
    }
    world.navigation_revision.fetch_add(1, Ordering::Relaxed);
    let avoidance = unit_avoidance_requests(&world);
    let enemy = world.enemies.get(&1).unwrap();
    let blocked_x = enemy.x;
    let blocked_y = enemy.y;
    let navigation = enemy_navigation_target(&world, &enemy, core_x, core_y, &avoidance);
    assert_eq!(navigation.movement, (blocked_x, blocked_y));
    drop(enemy);
    simulate_waves_and_enemies(&world, &DashMap::new(), 60.0);
    let enemy = world.enemies.get(&1).unwrap();
    assert_eq!((enemy.x, enemy.y), (blocked_x, blocked_y));
    drop(enemy);
    world.base_buildings.clear();
    let mut stacked = world.enemies.get(&1).unwrap().clone();
    stacked.id = 2;
    world.enemies.insert(2, stacked);
    assert!(simulate_unit_collisions(&world));
    let first = world.enemies.get(&1).unwrap();
    let second = world.enemies.get(&2).unwrap();
    let separation = (first.x - second.x).hypot(first.y - second.y);
    assert!((separation - 7.68).abs() < 0.001);
    let first_tile_x = (first.x / 8.0).floor() as i32;
    let first_tile_y = (first.y / 8.0).floor() as i32;
    let avoidance = unit_avoidance_requests(&world);
    assert!(navigation_tile_avoided(
        &avoidance,
        2,
        first_tile_x,
        first_tile_y
    ));
    assert!(!navigation_tile_avoided(
        &avoidance,
        1,
        first_tile_x,
        first_tile_y
    ));
    drop(first);
    drop(second);
    world.enemies.remove(&2);
    let unit_position = world.enemies.get(&1).unwrap().clone();
    world.players.insert(
        99,
        PlayerCombatState {
            uuid: "collision-player".into(),
            player_id: 99,
            unit_id: 99,
            x: unit_position.x,
            y: unit_position.y,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    assert!(simulate_unit_collisions(&world));
    let player = world.players.get(&99).unwrap();
    let unit = world.enemies.get(&1).unwrap();
    assert!((player.x - unit.x).hypot(player.y - unit.y) > 0.0);
    drop(player);
    drop(unit);
}

#[test]
fn power_turrets_require_energy_and_meltdown_damage_is_continuous() {
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("power-turret-test".into(), GameMode::Survival);
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_001),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-power-turret-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let arc_position = (10 << 16) | 10;
    world.tiles.insert(
        arc_position,
        DynamicTile {
            position: arc_position,
            block: 355,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![arc_position],
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
            health: f32::MAX,
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
        },
    );
    let enemy_id = 3_000_000;
    world.enemies.insert(
        enemy_id,
        EnemyUnit {
            id: enemy_id,
            unit_type: DAGGER.unit_type,
            entity_class: DAGGER.entity_class,
            team: 2,
            x: 10.0 * 8.0 + 40.0,
            y: 10.0 * 8.0,
            rotation: 180.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    let connections = DashMap::new();
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("test".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );

    let power = compute_power_efficiency(&world);
    assert_eq!(power[&arc_position], 0.0);
    simulate_turrets(&world, &connections, 70.0, &power);
    assert_eq!(
        world.tiles.get(&arc_position).unwrap().production_progress,
        0.0
    );
    assert!(world.projectiles.is_empty());
    assert!(packet_rx.try_recv().is_err());

    let solar_position = (11 << 16) | 10;
    world.tiles.insert(
        solar_position,
        DynamicTile {
            position: solar_position,
            block: 314,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![solar_position],
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
            health: f32::MAX,
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
        },
    );
    let power = compute_power_efficiency(&world);
    let expected_efficiency = 1.6 / 3.3;
    assert!((power[&arc_position] - expected_efficiency).abs() < 0.0001);
    simulate_turrets(&world, &connections, 35.0, &power);
    assert!(world.projectiles.is_empty());
    assert!(
        (world.tiles.get(&arc_position).unwrap().production_progress - 35.0 * expected_efficiency)
            .abs()
            < 0.0001
    );
    simulate_turrets(&world, &connections, 38.0, &power);
    assert_eq!(world.projectiles.len(), 1);
    simulate_projectiles(&world, &connections, 1.0);
    assert_eq!(world.enemies.get(&enemy_id).unwrap().health, 80.0);

    let frame = packet_rx.try_recv().unwrap();
    assert_eq!(
        read_packet(std::io::Cursor::new(&frame[2..])).unwrap()[0],
        CREATE_BULLET_PACKET_ID
    );

    world.tiles.remove(&arc_position);
    world.tiles.remove(&solar_position);
    world.enemies.get_mut(&enemy_id).unwrap().health = 5_000.0;
    world.tiles.insert(
        arc_position,
        DynamicTile {
            position: arc_position,
            block: 366,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![arc_position],
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
            health: f32::MAX,
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
        },
    );
    let mut full_power = std::collections::HashMap::new();
    full_power.insert(arc_position, 1.0);
    simulate_turrets(&world, &connections, 90.0, &full_power);
    assert_eq!(world.projectiles.len(), 1);
    simulate_projectiles(&world, &connections, 4.0);
    assert_eq!(world.enemies.get(&enemy_id).unwrap().health, 5_000.0);
    simulate_projectiles(&world, &connections, 1.0);
    assert_eq!(world.enemies.get(&enemy_id).unwrap().health, 4_922.0);

    simulate_turrets(&world, &connections, 200.0, &full_power);
    assert_eq!(world.projectiles.len(), 1);
    assert_eq!(
        world.tiles.get(&arc_position).unwrap().production_progress,
        0.0
    );
    simulate_projectiles(&world, &connections, 225.0);
    assert!(world.projectiles.is_empty());
    assert_eq!(world.enemies.get(&enemy_id).unwrap().health, 1_412.0);
    assert_eq!(
        read_packet(std::io::Cursor::new(&packet_rx.try_recv().unwrap()[2..])).unwrap()[0],
        CREATE_BULLET_PACKET_ID
    );
    assert!(packet_rx.try_recv().is_err());

    simulate_turrets(&world, &connections, 89.0, &full_power);
    assert!(world.projectiles.is_empty());
    simulate_turrets(&world, &connections, 1.0, &full_power);
    assert_eq!(world.projectiles.len(), 1);
    cancel_transient_world_actions(&world);
    assert!(world.projectiles.is_empty());
}

#[test]
fn logistics_and_alpha_weapon_are_authoritative() {
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("test".into(), GameMode::Survival);
    *state.core_items.write() = vec![0; 22];
    let enemy_spawns = map.enemy_spawns();
    let mut world = DynamicWorld {
        game_state: state.clone(),
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
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
        tiles: DashMap::new(),
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::new(),
        mono_mining_targets: DashMap::new(),
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-logistics-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let drill_position = (35 << 16) | 100;
    world.tiles.insert(
        drill_position,
        DynamicTile {
            position: drill_position,
            block: 325,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: block_footprint(&world, drill_position, 325).unwrap(),
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
            health: f32::MAX,
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
        },
    );
    let conveyor_position = (37 << 16) | 100;
    world.tiles.insert(
        conveyor_position,
        DynamicTile {
            position: conveyor_position,
            block: 257,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![conveyor_position],
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
            health: f32::MAX,
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
        },
    );

    for _ in 0..40 {
        let power = compute_power_efficiency(&world);
        simulate_logistics(&world, 6.0, &power);
    }
    assert!(state.core_items.read()[0] >= 1);

    *state.core_health.write() = 9.0;
    world.enemies.insert(
        3_000_001,
        EnemyUnit {
            id: 3_000_001,
            unit_type: 0,
            entity_class: 4,
            team: 2,
            x: SPAWN_X as f32 * 8.0,
            y: SPAWN_Y as f32 * 8.0,
            rotation: 0.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    simulate_waves_and_enemies(&world, &DashMap::new(), 13.0);
    assert_eq!(world.projectiles.len(), 1);
    simulate_projectiles(&world, &DashMap::new(), 0.0);
    assert!(state.game_over.load(Ordering::Relaxed));
    world.enemies.remove(&3_000_001);

    let duo_position = 1 << 16;
    world.tiles.insert(
        duo_position,
        DynamicTile {
            position: duo_position,
            block: 349,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![duo_position],
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
            health: f32::MAX,
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
        },
    );
    assert!(accept_logistics_item(&world, duo_position, 0));
    assert_eq!(world.tiles.get(&duo_position).unwrap().ammo_units, 2.0);
    let ripple_ammo = turret_ammo(362, 15).unwrap();
    assert_eq!(ripple_ammo.bullet_id, 148);
    assert_eq!(ripple_ammo.damage, 552.0);
    assert_eq!(ripple_ammo.ammo_per_shot, 2.0);
    assert_eq!(turret_ammo(350, 8).unwrap().ammo_per_shot, 1.0);
    let fuse_ammo = turret_ammo(361, 7).unwrap();
    assert_eq!(fuse_ammo.bullet_id, 145);
    assert_eq!(fuse_ammo.speed, 0.0);
    assert_eq!(fuse_ammo.ammo_per_shot, 1.0);
    assert_eq!(liquid_turret_weapon(360, 0).unwrap().ammo_per_shot, 2.5);
    // Swarmer and salvo explicitly opt out of consumeAmmoOnce in Java.
    assert_eq!(turret_ammo(357, 14).unwrap().ammo_per_shot, 4.0);
    assert_eq!(turret_ammo(358, 0).unwrap().ammo_per_shot, 4.0);
    let foreshadow_ammo = turret_ammo(364, 12).unwrap();
    assert_eq!(foreshadow_ammo.bullet_id, 158);
    assert_eq!(foreshadow_ammo.damage, 1350.0);
    assert_eq!(foreshadow_ammo.ammo_per_shot, 5.0);
    assert_eq!(turret_max_ammo(364), 40.0);
    assert_eq!(power_role(364).unwrap().demand, 10.0);
    assert!(turret_can_target(350, FLARE.unit_type));
    assert!(!turret_can_target(350, DAGGER.unit_type));
    assert!(turret_can_target(352, DAGGER.unit_type));
    assert!(!turret_can_target(352, FLARE.unit_type));
    // The shared registry covers Serpulo/Erekir/core/missile flyers and
    // does not confuse hovering ground units with flying units.
    for unit in [15, 19, 20, 23, 35, 37, 46, 50, 55, 58, 60, 62, 67] {
        assert!(unit_type_is_flying(unit), "unit {unit} must be flying");
    }
    for unit in [0, 14, 25, 34, 38, 49, 56, 57, 61, 68] {
        assert!(!unit_type_is_flying(unit), "unit {unit} must be grounded");
    }
    assert!(turret_can_target(350, 50)); // scatter -> avert
    assert!(!turret_can_target(362, 50)); // ripple !-> avert
    assert!(turret_can_target(362, 49)); // ripple -> hovering elude

    let transport_tile = |position: i32, block: i16, config: Vec<u8>| DynamicTile {
        enabled: true,
        message: None,
        position,
        block,
        rotation: 0,
        team: 1,
        config,
        occupied: vec![position],
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
        health: f32::MAX,
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
    let sorter = (80 << 16) | 80;
    let sorter_source = (79 << 16) | 80;
    let sorter_forward = (81 << 16) | 80;
    let sorter_left = (80 << 16) | 81;
    world.tiles.insert(
        sorter,
        transport_tile(sorter, 264, vec![5, 0, 0, 0]), // copper
    );
    world
        .tiles
        .insert(sorter_forward, transport_tile(sorter_forward, 257, vec![0]));
    world
        .tiles
        .insert(sorter_left, transport_tile(sorter_left, 257, vec![0]));
    assert!(accept_logistics_item_from(
        &world,
        sorter,
        0,
        Some(sorter_source),
        0
    ));
    assert_eq!(world.tiles.get(&sorter_forward).unwrap().stored_item, 0);
    assert!(accept_logistics_item_from(
        &world,
        sorter,
        5,
        Some(sorter_source),
        0
    ));
    assert_eq!(world.tiles.get(&sorter_left).unwrap().stored_item, 5);

    let overflow = (90 << 16) | 80;
    let overflow_source = (89 << 16) | 80;
    let overflow_forward = (91 << 16) | 80;
    let overflow_left = (90 << 16) | 81;
    let mut blocked = transport_tile(overflow_forward, 257, vec![0]);
    blocked.stored_item = 0;
    blocked.stored_amount = 3;
    world
        .tiles
        .insert(overflow, transport_tile(overflow, 268, vec![0]));
    world.tiles.insert(overflow_forward, blocked);
    world
        .tiles
        .insert(overflow_left, transport_tile(overflow_left, 257, vec![0]));
    assert!(accept_logistics_item_from(
        &world,
        overflow,
        5,
        Some(overflow_source),
        0
    ));
    assert_eq!(world.tiles.get(&overflow_left).unwrap().stored_item, 5);

    let junction = (100 << 16) | 80;
    let junction_source = (99 << 16) | 80;
    let junction_output = (101 << 16) | 80;
    world
        .tiles
        .insert(junction, transport_tile(junction, 261, vec![0]));
    world.tiles.insert(
        junction_output,
        transport_tile(junction_output, 257, vec![0]),
    );
    assert!(accept_logistics_item_from(
        &world,
        junction,
        3,
        Some(junction_source),
        0
    ));
    simulate_junctions(&world, 25.0);
    assert_eq!(world.tiles.get(&junction_output).unwrap().stored_amount, 0);
    simulate_junctions(&world, 1.0);
    assert_eq!(world.tiles.get(&junction_output).unwrap().stored_item, 3);
    assert!(world
        .tiles
        .get(&junction)
        .unwrap()
        .junction_items
        .is_empty());

    let container = (110 << 16) | 80;
    let unloader = (111 << 16) | 80;
    let unload_target = (112 << 16) | 80;
    let mut container_tile = transport_tile(container, 345, vec![0]);
    container_tile.inventory = vec![(3, 2)];
    world.tiles.insert(container, container_tile);
    world.tiles.insert(
        unloader,
        transport_tile(unloader, 270, vec![5, 0, 0, 3]), // graphite
    );
    world
        .tiles
        .insert(unload_target, transport_tile(unload_target, 257, vec![0]));
    simulate_unloaders(&world, 5.0);
    assert_eq!(world.tiles.get(&unload_target).unwrap().stored_amount, 0);
    simulate_unloaders(&world, 0.5);
    assert_eq!(world.tiles.get(&unload_target).unwrap().stored_item, 3);
    assert_eq!(
        inventory_count(&world.tiles.get(&container).unwrap().inventory, 3),
        1
    );
    assert!(accept_logistics_item(&world, container, 3));
    assert_eq!(
        inventory_count(&world.tiles.get(&container).unwrap().inventory, 3),
        2
    );

    let driver_source = (120 << 16) | 80;
    let driver_target = (130 << 16) | 80;
    let mut source_driver = transport_tile(driver_source, 271, vec![7, 0, 0, 0, 10, 0, 0, 0, 0]);
    source_driver.inventory = vec![(0, 12)];
    world.tiles.insert(driver_source, source_driver);
    world
        .tiles
        .insert(driver_target, transport_tile(driver_target, 271, vec![0]));
    let mut driver_power = std::collections::HashMap::new();
    driver_power.insert(driver_source, 1.0);
    driver_power.insert(driver_target, 1.0);
    simulate_mass_drivers(&world, 1.0, &driver_power);
    assert_eq!(
        inventory_total(&world.tiles.get(&driver_source).unwrap().inventory),
        12
    );
    assert_eq!(
        world.tiles.get(&driver_target).unwrap().mass_driver_waiting,
        vec![driver_source]
    );
    assert_eq!(
        world
            .tiles
            .get(&driver_source)
            .unwrap()
            .mass_driver_rotation,
        85.0
    );
    assert_eq!(
        world
            .tiles
            .get(&driver_target)
            .unwrap()
            .mass_driver_rotation,
        95.0
    );
    simulate_mass_drivers(&world, 17.0, &driver_power);
    assert_eq!(
        inventory_total(&world.tiles.get(&driver_source).unwrap().inventory),
        0
    );
    assert_eq!(
        world
            .tiles
            .get(&driver_target)
            .unwrap()
            .mass_driver_incoming
            .len(),
        1
    );
    simulate_mass_drivers(&world, 14.0, &driver_power);
    assert_eq!(
        inventory_total(&world.tiles.get(&driver_target).unwrap().inventory),
        0
    );
    simulate_mass_drivers(&world, 1.0, &driver_power);
    assert_eq!(
        inventory_count(&world.tiles.get(&driver_target).unwrap().inventory, 0),
        12
    );
    assert_eq!(
        world.tiles.get(&driver_target).unwrap().production_progress,
        200.0
    );

    let queued_source_a = (140 << 16) | 80;
    let queued_source_b = (150 << 16) | 80;
    for source_position in [queued_source_a, queued_source_b] {
        let dx = 130 - ((source_position >> 16) as i16 as i32);
        let mut queued = transport_tile(source_position, 271, {
            let mut config = vec![7];
            config.extend_from_slice(&dx.to_be_bytes());
            config.extend_from_slice(&0i32.to_be_bytes());
            config
        });
        queued.inventory = vec![(0, 12)];
        world.tiles.insert(source_position, queued);
        driver_power.insert(source_position, 1.0);
    }
    simulate_mass_drivers(&world, 36.0, &driver_power);
    assert_eq!(
        world.tiles.get(&driver_target).unwrap().mass_driver_waiting,
        vec![queued_source_a, queued_source_b]
    );
    assert_eq!(
        world
            .tiles
            .get(&driver_target)
            .unwrap()
            .mass_driver_incoming
            .first()
            .map(|cargo| cargo.0),
        Some(queued_source_a)
    );
    assert_eq!(
        inventory_total(&world.tiles.get(&queued_source_b).unwrap().inventory),
        12
    );

    world.enemies.insert(
        3_000_002,
        EnemyUnit {
            id: 3_000_002,
            unit_type: 0,
            entity_class: 4,
            team: 2,
            x: 100.0,
            y: 0.0,
            rotation: 180.0,
            health: 18.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    let connections = DashMap::new();
    let (death_tx, mut death_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: death_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("test".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    let power = compute_power_efficiency(&world);
    simulate_turrets(&world, &connections, 20.0, &power);
    let first_projectile_changed_world = simulate_projectiles(&world, &connections, 40.0);
    simulate_turrets(&world, &connections, 20.0, &power);
    let second_projectile_changed_world = simulate_projectiles(&world, &connections, 40.0);
    assert!(
        first_projectile_changed_world || second_projectile_changed_world,
        "allied projectile damage must dirty persistence"
    );
    assert!(!world.enemies.contains_key(&3_000_002));
    assert_eq!(world.tiles.get(&duo_position).unwrap().ammo_units, 0.0);
    let mut packet_ids = Vec::new();
    while let Ok(frame) = death_rx.try_recv() {
        packet_ids.push(read_packet(std::io::Cursor::new(&frame[2..])).unwrap()[0]);
    }
    assert!(packet_ids.contains(&CREATE_BULLET_PACKET_ID));
    assert!(packet_ids.contains(&UNIT_DEATH_PACKET_ID));

    world.enemies.insert(
        3_000_000,
        EnemyUnit {
            id: 3_000_000,
            unit_type: 0,
            entity_class: 4,
            team: 2,
            x: 100.0,
            y: 0.0,
            rotation: 180.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    let mut shooter = player();
    shooter.shooting = true;
    shooter.mouse_x = 100.0;
    for _ in 0..14 {
        shooter.last_shot = std::time::Instant::now() - std::time::Duration::from_secs(1);
        update_player_combat(&mut shooter, &world, &connections).unwrap();
    }
    simulate_projectiles(&world, &connections, 60.0);
    assert!(world.enemies.is_empty());
    assert_eq!(state.enemies_count.load(Ordering::Relaxed), 0);

    for (x, block) in [(10, 313), (15, 302), (20, 327), (100, 328)] {
        let position = (x << 16) | 10;
        // Power nodes connect ONLY through explicit power_links
        // (SOL-010); give the node the links a placement autolink would
        // have created (solar/laser/battery within laserRange 6*8), with
        // the reverse link on each non-node neighbour (JAR config writes
        // both sides).
        let power_links = match block {
            302 => vec![(10 << 16) | 10, (20 << 16) | 10, (12 << 16) | 10],
            313 | 327 => vec![(15 << 16) | 10],
            _ => Vec::new(),
        };
        world.tiles.insert(
            position,
            DynamicTile {
                position,
                block,
                rotation: 0,
                team: 1,
                config: vec![0],
                enabled: true,
                message: None,
                occupied: vec![position],
                stored_item: -1,
                stored_amount: 0,
                production_progress: 0.0,
                transport_progress: 0.0,
                ammo_units: 0.0,
                inventory: Vec::new(),
                power_stored: 0.0,
                power_links,
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
                health: f32::MAX,
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
            },
        );
    }
    let power = compute_power_efficiency(&world);
    let laser = (20 << 16) | 10;
    let blast = (100 << 16) | 10;
    assert!((power[&laser] - 0.12 / 1.1).abs() < 0.0001);
    assert_eq!(power[&blast], 0.0);

    let combustion = (11 << 16) | 10;
    world.tiles.insert(
        combustion,
        DynamicTile {
            position: combustion,
            block: 308,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![combustion],
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
            health: f32::MAX,
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
        },
    );
    assert!(accept_logistics_item(&world, combustion, 5));
    simulate_generators(&world, 6.0);
    assert_eq!(
        world.tiles.get(&combustion).unwrap().production_progress,
        114.0
    );
    assert_eq!(compute_power_efficiency(&world)[&laser], 1.0);

    let battery = (12 << 16) | 10;
    world.tiles.insert(
        battery,
        DynamicTile {
            position: battery,
            block: 306,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![battery],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: Vec::new(),
            power_stored: 0.0,
            power_links: vec![(15 << 16) | 10],
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
            health: f32::MAX,
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
        },
    );
    update_power_network(&world, 60.0);
    assert!((world.tiles.get(&battery).unwrap().power_stored - 1.2).abs() < 0.001);
    world.tiles.remove(&combustion);
    world.tiles.remove(&((10 << 16) | 10));
    let battery_power = update_power_network(&world, 1.0);
    assert_eq!(battery_power[&laser], 1.0);
    assert!((world.tiles.get(&battery).unwrap().power_stored - 0.1).abs() < 0.001);

    // SOL-010: mechanical-pump (283) selects its liquid from the floor
    // (Pump.canPump/onProximityUpdate, Pump.java:138-146); the dummy map
    // tile is darksand, so set shallow-water (22) under the pump.
    world.floors[30 * world.width as usize + 30] = 22;
    for (x, block) in [(30, 283), (31, 286), (32, 289)] {
        let position = (x << 16) | 30;
        world.tiles.insert(
            position,
            DynamicTile {
                position,
                block,
                rotation: 0,
                team: 1,
                config: vec![0],
                enabled: true,
                message: None,
                occupied: vec![position],
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
                health: f32::MAX,
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
            },
        );
    }
    for _ in 0..3 {
        let power = compute_power_efficiency(&world);
        simulate_liquids(&world, 60.0, &power);
    }
    let liquid_total: f32 = (30..=32)
        .map(|x| world.tiles.get(&((x << 16) | 30)).unwrap().liquid_amount)
        .sum();
    assert!(liquid_total > 0.0);
    assert!((30..=32).any(|x| {
        let tile = world.tiles.get(&((x << 16) | 30)).unwrap();
        tile.stored_liquid == 0 && tile.liquid_amount > 0.0
    }));

    let tsunami = (70 << 16) | 70;
    world.tiles.insert(
        tsunami,
        DynamicTile {
            position: tsunami,
            block: 360,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![tsunami],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: Vec::new(),
            power_stored: 0.0,
            power_links: Vec::new(),
            liquid_inventory: Vec::new(),
            stored_liquid: 1,
            liquid_amount: 10.0,
            output_liquid_amount: 0.0,
            junction_items: Vec::new(),
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 90.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: f32::MAX,
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
        },
    );
    assert_eq!(accept_liquid(&world, tsunami, 0, 1.0), 0.0);
    world.tiles.get_mut(&tsunami).unwrap().liquid_amount = 2.5;
    assert_eq!(accept_liquid(&world, tsunami, 0, 1.0), 1.0);
    {
        let mut tile = world.tiles.get_mut(&tsunami).unwrap();
        tile.stored_liquid = 1;
        tile.liquid_amount = 10.0;
    }
    let tsunami_target = 3_000_004;
    world.enemies.insert(
        tsunami_target,
        EnemyUnit {
            id: tsunami_target,
            unit_type: DAGGER.unit_type,
            entity_class: DAGGER.entity_class,
            team: 2,
            x: 70.0 * 8.0 + 100.0,
            y: 70.0 * 8.0,
            rotation: 180.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    let power = compute_power_efficiency(&world);
    simulate_turrets(&world, &connections, 3.0, &power);
    // LiquidTurret.useAmmo (158.1): removes 1f / ammoMultiplier(0.4) = 2.5
    // liquid per shot; one reload (3 ticks) leaves 10 - 2.5.
    assert_eq!(world.tiles.get(&tsunami).unwrap().liquid_amount, 7.5);
    assert_eq!(world.projectiles.len(), 2);
    simulate_projectiles(&world, &connections, 30.0);
    assert_eq!(world.enemies.get(&tsunami_target).unwrap().health, 90.5);
    world.enemies.remove(&tsunami_target);

    for (x, block, with_item, with_liquid) in [
        (50, 262, true, false),
        (53, 262, false, false),
        (60, 293, false, true),
        (63, 293, false, false),
    ] {
        let position = (x << 16) | 40;
        let mut config = vec![0];
        if matches!(x, 50 | 60) {
            config = vec![7];
            config.extend_from_slice(&3i32.to_be_bytes());
            config.extend_from_slice(&0i32.to_be_bytes());
        }
        world.tiles.insert(
            position,
            DynamicTile {
                position,
                block,
                rotation: 0,
                team: 1,
                config,
                enabled: true,
                message: None,
                occupied: vec![position],
                stored_item: if with_item { 0 } else { -1 },
                stored_amount: i32::from(with_item),
                production_progress: 0.0,
                transport_progress: 0.0,
                ammo_units: 0.0,
                inventory: Vec::new(),
                power_stored: 0.0,
                power_links: Vec::new(),
                liquid_inventory: Vec::new(),
                stored_liquid: if with_liquid { 0 } else { -1 },
                liquid_amount: if with_liquid { 10.0 } else { 0.0 },
                output_liquid_amount: 0.0,
                junction_items: Vec::new(),
                mass_driver_incoming: Vec::new(),
                mass_driver_rotation: 90.0,
                mass_driver_waiting: Vec::new(),
                payload: None,
                payload_progress: 0.0,
                payload_rotation: 0.0,
                payload_accum: Vec::new(),
                health: f32::MAX,
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
            },
        );
    }
    let power = compute_power_efficiency(&world);
    simulate_logistics(&world, 74.0, &power);
    assert_eq!(
        world.tiles.get(&((53 << 16) | 40)).unwrap().stored_amount,
        1
    );
    let bridge_output = (54 << 16) | 40;
    let mut bridge_output_tile = world.tiles.get(&((53 << 16) | 40)).unwrap().clone();
    bridge_output_tile.position = bridge_output;
    bridge_output_tile.block = 257;
    bridge_output_tile.rotation = 0;
    bridge_output_tile.config = vec![0];
    bridge_output_tile.occupied = vec![bridge_output];
    bridge_output_tile.stored_item = -1;
    bridge_output_tile.stored_amount = 0;
    bridge_output_tile.transport_progress = 0.0;
    bridge_output_tile.conveyor_items = Vec::new();
    world.tiles.insert(bridge_output, bridge_output_tile);
    assert!(simulate_logistics(&world, 74.0, &power));
    assert_eq!(
        world.tiles.get(&((53 << 16) | 40)).unwrap().stored_amount,
        0
    );
    assert_eq!(world.tiles.get(&bridge_output).unwrap().stored_amount, 1);

    // A receiver must not dump back into a bridge that links to it.
    world.tiles.remove(&bridge_output);
    let reverse_source = (56 << 16) | 40;
    let mut reverse_bridge = world.tiles.get(&((50 << 16) | 40)).unwrap().clone();
    reverse_bridge.position = reverse_source;
    reverse_bridge.config = vec![7];
    reverse_bridge
        .config
        .extend_from_slice(&(-3i32).to_be_bytes());
    reverse_bridge.config.extend_from_slice(&0i32.to_be_bytes());
    reverse_bridge.occupied = vec![reverse_source];
    reverse_bridge.stored_item = -1;
    reverse_bridge.stored_amount = 0;
    world.tiles.insert(reverse_source, reverse_bridge);
    let north_output = (53 << 16) | 41;
    let mut north_conveyor = world.tiles.get(&((53 << 16) | 40)).unwrap().clone();
    north_conveyor.position = north_output;
    north_conveyor.block = 257;
    north_conveyor.rotation = 1;
    north_conveyor.config = vec![0];
    north_conveyor.occupied = vec![north_output];
    north_conveyor.stored_item = -1;
    north_conveyor.stored_amount = 0;
    world.tiles.insert(north_output, north_conveyor);
    {
        let mut receiver = world.tiles.get_mut(&((53 << 16) | 40)).unwrap();
        receiver.stored_item = 0;
        receiver.stored_amount = 1;
        receiver.transport_progress = 0.0;
    }
    assert!(simulate_logistics(&world, 5.0, &power));
    assert_eq!(world.tiles.get(&reverse_source).unwrap().stored_amount, 0);
    assert_eq!(world.tiles.get(&north_output).unwrap().stored_amount, 1);

    let replacement_position = (100 << 16) | 40;
    let mut replaceable_conveyor = world.tiles.get(&north_output).unwrap().clone();
    replaceable_conveyor.position = replacement_position;
    replaceable_conveyor.block = 257;
    replaceable_conveyor.occupied = vec![replacement_position];
    replaceable_conveyor.stored_item = -1;
    replaceable_conveyor.stored_amount = 0;
    world
        .tiles
        .insert(replacement_position, replaceable_conveyor);
    world.game_state.core_items.write()[0] = 100;
    let replacement = PendingBuild {
        position: replacement_position,
        block: 261,
        rotation: 0,
        config: vec![0],
        occupied: vec![replacement_position],
        team: 1,
        builder: player(),
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: 0.0,
        applied_assist: 0.0,
    };
    world
        .pending_builds
        .insert(replacement_position, replacement.clone());
    finish_pending_build(&world, &connections, replacement).unwrap();
    assert_eq!(world.tiles.get(&replacement_position).unwrap().block, 261);
    assert_eq!(world.game_state.core_items.read()[0], 97);

    simulate_liquids(&world, 6.0, &power);
    assert!(world.tiles.get(&((63 << 16) | 40)).unwrap().liquid_amount > 0.0);

    let press = (200 << 16) | 10;
    world.tiles.insert(
        press,
        DynamicTile {
            position: press,
            block: 181,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![press],
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
            health: f32::MAX,
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
        },
    );
    assert!(accept_logistics_item(&world, press, 5));
    assert!(accept_logistics_item(&world, press, 5));
    let power = compute_power_efficiency(&world);
    simulate_factories(&world, 90.0, &power);
    assert_eq!(
        inventory_count(&world.tiles.get(&press).unwrap().inventory, 3),
        1
    );

    let smelter = (210 << 16) | 10;
    world.tiles.insert(
        smelter,
        DynamicTile {
            position: smelter,
            block: 183,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![smelter],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: vec![(5, 1), (4, 2)],
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
            health: f32::MAX,
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
        },
    );
    let power = compute_power_efficiency(&world);
    simulate_factories(&world, 400.0, &power);
    assert_eq!(
        inventory_count(&world.tiles.get(&smelter).unwrap().inventory, 9),
        0
    );

    let cryofluid_mixer = (220 << 16) | 10;
    world.tiles.insert(
        cryofluid_mixer,
        DynamicTile {
            position: cryofluid_mixer,
            block: 189,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![cryofluid_mixer],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: vec![(6, 1)],
            power_stored: 0.0,
            power_links: Vec::new(),
            liquid_inventory: Vec::new(),
            stored_liquid: 0,
            liquid_amount: 24.0,
            output_liquid_amount: 0.0,
            junction_items: Vec::new(),
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 90.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: f32::MAX,
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
        },
    );
    let powered = std::collections::HashMap::from([(cryofluid_mixer, 1.0)]);
    simulate_liquid_factories(&world, 120.0, &powered);
    let mixer = world.tiles.get(&cryofluid_mixer).unwrap();
    assert_eq!(mixer.stored_liquid, -1);
    assert_eq!(mixer.liquid_amount, 0.0);
    assert_eq!(mixer.output_liquid_amount, 24.0);
}

#[test]
fn puddle_entities_ride_the_entity_snapshot_stream_with_class_13() {
    // Round-73 A2: the official server syncs puddles in the entity
    // snapshot stream (Groups.sync, classId 13, `Puddle.writeSync` = f
    // amount, s liquid.id, TypeIO.writeTile (i packed pos), f x, f y —
    // all verified in desktop.jar 158.1 bytecode).
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let position = (10 << 16) | 12;
    world.puddles.deposit(position, 0, 35.0); // water
    world.puddles.tick(1.0); // official: deposit queues `accepting`, update applies it
    let payloads = encode_enemy_entity_snapshots(&world).unwrap();
    assert_eq!(payloads.len(), 1);
    let payload = &payloads[0];
    // s entityCount, s byteLength, then entities.
    assert_eq!(i16::from_be_bytes(payload[0..2].try_into().unwrap()), 1);
    let data_len = i16::from_be_bytes(payload[2..4].try_into().unwrap()) as usize;
    assert_eq!(data_len + 4, payload.len());
    let body = &payload[4..];
    // i entity id (allocator starts at 100), b classId 13.
    let entity_id = i32::from_be_bytes(body[0..4].try_into().unwrap());
    assert!(entity_id >= 100, "entity id from allocator");
    assert_eq!(body[4], 13, "Puddle.classId()");
    // f amount, s liquid id, i tile pos, f x, f y.
    let amount = f32::from_be_bytes(body[5..9].try_into().unwrap());
    assert!(
        amount > 30.0 && amount <= 35.0,
        "amount after one tick: {amount}"
    );
    assert_eq!(i16::from_be_bytes(body[9..11].try_into().unwrap()), 0);
    assert_eq!(
        i32::from_be_bytes(body[11..15].try_into().unwrap()),
        position
    );
    let x = f32::from_be_bytes(body[15..19].try_into().unwrap());
    let y = f32::from_be_bytes(body[19..23].try_into().unwrap());
    assert_eq!(x, 10.0 * 8.0 + 4.0);
    assert_eq!(y, 12.0 * 8.0 + 4.0);
    assert_eq!(body.len(), 23, "exact Puddle.writeSync size");
}

#[test]
fn puddles_survive_the_json_checkpoint_round_trip() {
    // Round-73 A2: puddles persist in the JSON checkpoint (revision 14)
    // like the official MSAV does via serialize()==true.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let position = (22 << 16) | 7;
    world.puddles.deposit(position, 1, 60.0); // slag
    world.puddles.tick(1.0); // apply the queued deposit
    let path = std::env::temp_dir().join(format!(
        "mindustry-puddle-persist-test-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
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
    .unwrap();
    let loaded = load_tiles(&path, Some((world.width, world.height))).unwrap();
    // The spread pass deposits into d4 neighbors (official behavior), so
    // the checkpoint holds the origin plus small neighbor puddles.
    let origin = loaded
        .puddles
        .iter()
        .find(|puddle| puddle.position == position)
        .expect("origin puddle persisted");
    assert_eq!(origin.liquid, 1);
    assert!(
        origin.amount > 50.0 && origin.amount <= 60.0,
        "{}",
        origin.amount
    );
    assert!(origin.entity_id >= 100);
    assert!(loaded
        .puddles
        .iter()
        .all(|puddle| puddle.amount >= 0.0 && puddle.amount <= 70.0));
    let _ = std::fs::remove_file(&path);
}

/// Isolated Administration for tests: a unique temp file per call so
/// kicked cooldowns/bans never leak into the repo's `admin-data.json`
/// (round 73: M4 persists kicked_ips; the shared file used to pick up
/// test cooldowns and break later runs / the user's server).
fn test_admin() -> crate::state::administration::Administration {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "mindustry-admin-test-{}-{}.json",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    crate::state::administration::Administration::with_file(path)
}

fn legacy_weapons_test_world() -> (DynamicWorld, DashMap<i32, PendingConnection>, f32, f32) {
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("legacy-weapons".into(), GameMode::Survival);
    *state.wave_time.write() = 10_000.0;
    *state.simulation_time.write() = 240.0;
    let enemy_spawns = map.enemy_spawns();
    let core_x = SPAWN_X as f32 * 8.0;
    let core_y = SPAWN_Y as f32 * 8.0;
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_100),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-legacy-weapons-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    (world, DashMap::new(), core_x, core_y)
}

fn legacy_weapons_make_enemy(id: i32, spec: EnemySpec, x: f32, y: f32, health: f32) -> EnemyUnit {
    EnemyUnit {
        id,
        unit_type: spec.unit_type,
        entity_class: spec.entity_class,
        team: 2,
        x,
        y,
        rotation: 0.0,
        health,
        shield: 0.0,
        status_effect: -1,
        status_duration: f32::MAX,
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
        move_speed: spec.speed,
        attack_damage: spec.attack_damage,
        attack_reload_time: spec.attack_reload,
        attack_range: spec.attack_range,
        authority: UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        status_agg: None,
    }
}

#[test]
fn core_destruction_emits_game_over_call_packet_48_team_2() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("gameover".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    // One dagger bolt (9.0 damage) must bring the 9 HP core to zero.
    *world.game_state.core_health.write() = 9.0;
    world.enemies.insert(
        3_000_200,
        legacy_weapons_make_enemy(3_000_200, DAGGER, core_x, core_y, DAGGER.health),
    );
    assert!(!world.game_state.game_over.load(Ordering::Relaxed));

    // Natural flow: the enemy fires at the core and the projectile hits.
    simulate_waves_and_enemies(&world, &connections, DAGGER.attack_reload);
    assert_eq!(world.projectiles.len(), 1);
    assert!(simulate_projectiles(&world, &connections, 0.0));

    assert!(world.game_state.game_over.load(Ordering::Relaxed));
    assert!(world.persistence_dirty.load(Ordering::Relaxed));

    // The broadcast must contain exactly one GameOverCallPacket (48) with
    // payload `b team` = 2 (waveTeam / crux, the enemy in survival).
    let mut game_over_frames = Vec::new();
    while let Ok(frame) = packet_rx.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            game_over_frames.push(packet);
        }
    }
    assert_eq!(
        game_over_frames.len(),
        1,
        "GameOverCallPacket must be broadcast exactly once on core death"
    );
    assert_eq!(&game_over_frames[0][1..], &[2]);

    // A further hit on the already-dead core must not re-emit the packet.
    assert!(apply_enemy_direct_damage(
        &world,
        &connections,
        None,
        true,
        5.0
    ));
    let mut extra = Vec::new();
    while let Ok(frame) = packet_rx.try_recv() {
        extra.push(frame);
    }
    assert!(
        extra.iter().all(|frame| {
            read_packet(std::io::Cursor::new(&frame[2..]))
                .map(|packet| packet[0] != GAME_OVER_PACKET_ID)
                .unwrap_or(true)
        }),
        "no duplicate GameOverCallPacket after the core is already dead"
    );
}

#[test]
fn runtime_built_core_registers_team_and_reregisters_on_destroy() {
    let (world, _, _, _) = legacy_weapons_test_world();
    let pos_a = (44 << 16) | 104;
    let pos_b = (52 << 16) | 104;
    // Team 5 has no registered core yet (PvP player just placed it).
    assert!(world.cores.get(&5).is_none());
    // Fund team 5 so the core's build cost can be consumed.
    if let Some(mut items) = world.game_state.team_items.get_mut(&5) {
        items.fill(50_000);
    } else {
        world.game_state.team_items.insert(5, vec![50_000; 22]);
    }
    // finish_pending_build registers it.
    let occupied_a = block_footprint_in(300, 300, pos_a, 339).unwrap();
    let pending = PendingBuild {
        position: pos_a,
        block: 339,
        rotation: 0,
        config: Vec::new(),
        occupied: occupied_a,
        team: 5,
        builder: player(),
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: 0.0,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(pos_a, pending.clone());
    finish_pending_build(&world, &DashMap::new(), pending).unwrap();
    let registered = *world.cores.get(&5).unwrap();
    assert_eq!(registered.position, pos_a);
    // Team 5's economy now routes to its own inventory.
    let before = items_for_team(&world, 5)[0];
    if let Some(v) = items_for_team_mut(&world, 5).get_mut(0) {
        *v += 1;
    }
    assert_eq!(items_for_team(&world, 5)[0], before + 1);
    // Team 1 keeps its own inventory (initial loadout: 100 copper).
    assert_eq!(items_for_team(&world, 1)[0], 100, "team 1 untouched");

    // Destroy pos_a: with a second team-5 core at pos_b, the team
    // re-registers to it.
    let occupied_b = block_footprint_in(300, 300, pos_b, 339).unwrap();
    world.tiles.insert(
        pos_b,
        DynamicTile {
            position: pos_b,
            block: 339,
            team: 5,
            health: 6000.0,
            occupied: occupied_b,
            enabled: true,
            config: Vec::new(),
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
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            rotation: 0,
            message: None,
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
        },
    );
    damage_building(&world, pos_a, 6000.0);
    let re = *world.cores.get(&5).unwrap();
    assert_eq!(re.position, pos_b, "team re-registers to surviving core");
}

fn enemy_splash_core_destruction_also_emits_game_over_call_packet() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("splash".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    *world.game_state.core_health.write() = 9.0;
    // Splash landing on the core destroys it through the
    // projectile-impact path (apply_enemy_splash_damage).
    assert!(apply_enemy_splash_damage(
        &world,
        &connections,
        core_x,
        core_y,
        9.0,
        100.0,
        1.0,
        -1,
        0.0,
    ));
    assert!(world.game_state.game_over.load(Ordering::Relaxed));
    assert!(world.persistence_dirty.load(Ordering::Relaxed));
    let mut game_over_frames = Vec::new();
    while let Ok(frame) = packet_rx.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            game_over_frames.push(packet);
        }
    }
    assert_eq!(game_over_frames.len(), 1);
    assert_eq!(&game_over_frames[0][1..], &[2]);
}

fn erekir_like_tile(position: i32, block: i16) -> DynamicTile {
    DynamicTile {
        position,
        block,
        team: 1,
        rotation: 0,
        config: vec![0],
        enabled: true,
        message: None,
        occupied: vec![position],
        health: 1000.0,
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

#[test]
fn thorium_reactor_overheat_destroys_reactor_and_applies_official_blast() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let reactor_position = (40 << 16) | 40;
    let victim_position = (45 << 16) | 40;
    let mut reactor_tile = erekir_like_tile(reactor_position, 315);
    reactor_tile.inventory = vec![(reactor::THORIUM_ITEM, reactor::ITEM_CAPACITY)];
    reactor_tile.output_liquid_amount = 0.99; // NuclearReactorBuild.heat
    reactor_tile.health = crate::game::content::block_health(315);
    let mut victim = erekir_like_tile(victim_position, 345);
    victim.health = crate::game::content::block_health(345);
    world.tiles.insert(reactor_position, reactor_tile);
    world.tiles.insert(victim_position, victim);

    assert!(simulate_reactors(&world, 1.0));
    assert_eq!(
        world.tiles.get(&reactor_position).unwrap().block,
        0,
        "heat >= .999 invokes kill()"
    );
    assert_eq!(
        world.tiles.get(&victim_position).unwrap().block,
        0,
        "5000 damage inside the 19-tile reactor radius is authoritative"
    );
}

#[test]
fn logic_cannot_take_items_from_enemy_buildings() {
    // SOL-007: a PvP processor (team 5) must not take items out of a
    // team-1 container through ucontrol itemTake.
    let (world, _, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let processor_pos = (40 << 16) | 40;
    let container_pos = (41 << 16) | 40;
    let mut processor = erekir_like_tile(processor_pos, 431);
    processor.team = 5;
    processor.config = vec![1]; // any non-empty config is fine here
    let mut container = erekir_like_tile(container_pos, 288);
    container.team = 1;
    container.inventory = vec![(0, 10)]; // 10 copper
    world.tiles.insert(processor_pos, processor);
    world.tiles.insert(container_pos, container);
    let view = crate::logic::WorldView {
        world: &world,
        processor_pos,
        out: &DashMap::new(),
    };
    let program = crate::logic::compile("").unwrap();
    let mut state = crate::logic::ExecutorState::new(program, vec![]);
    state.bound_unit = Some(3_000_100);
    // The processor tries to take 5 copper from the container.
    let target = crate::logic::LObject::Building(container_pos);
    view.ucontrol_itemtake(&state, &crate::logic::LVar::new_obj("target", target), 0, 5);
    let after_count = {
        let after = world.tiles.get(&container_pos).unwrap();
        inventory_count(&after.inventory, 0)
    };
    assert_eq!(after_count, 10, "enemy container untouched");
    // Same-team take works.
    let mut own = erekir_like_tile(container_pos, 288);
    own.team = 5;
    own.inventory = vec![(0, 10)];
    world.tiles.insert(container_pos, own);
    view.ucontrol_itemtake(
        &state,
        &crate::logic::LVar::new_obj("target", crate::logic::LObject::Building(container_pos)),
        0,
        3,
    );
    let after_count = {
        let after = world.tiles.get(&container_pos).unwrap();
        inventory_count(&after.inventory, 0)
    };
    assert_eq!(after_count, 7, "own take works");
}

#[test]
fn pvp_ownership_blocks_cross_team_config_and_break() {
    // SOL-002: a player may only configure/rotate/demolish their own
    // team's buildings. The legacy `tile.team != 1` check wrongly
    // blocked PvP players (team 5) from their own tiles.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let mut team5 = player();
    team5.id = 1_000_005;
    team5.unit_id = 2_000_005;
    team5.x = 360.0;
    team5.y = 800.0;
    world.players.insert(
        2_000_005,
        PlayerCombatState {
            uuid: "team5".into(),
            player_id: 1_000_005,
            unit_id: 2_000_005,
            x: 360.0,
            y: 800.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 5,
        },
    );
    let pos = (45 << 16) | 100;
    let occupied = block_footprint_in(300, 300, pos, 216).unwrap();
    world.tiles.insert(
        pos,
        DynamicTile {
            position: pos,
            block: 270,
            team: 5,
            health: 1000.0,
            occupied,
            enabled: true,
            config: vec![0],
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
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            rotation: 0,
            message: None,
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
        },
    );
    // The team-5 player CAN configure their own tile.
    assert!(apply_tile_config(&team5, &world, pos, &[5, 0, 0, 0]));
    // A team-1 player cannot configure the team-5 tile.
    let mut team1 = player();
    team1.x = 360.0;
    team1.y = 800.0;
    assert!(!apply_tile_config(&team1, &world, pos, &[5, 0, 0, 0]));
    // The team-5 player cannot rotate a team-1 tile (and vice versa).
    let pos1 = (46 << 16) | 100;
    let occupied1 = block_footprint_in(300, 300, pos1, 216).unwrap();
    world.tiles.insert(
        pos1,
        DynamicTile {
            position: pos1,
            block: 270,
            team: 1,
            health: 1000.0,
            occupied: occupied1,
            enabled: true,
            config: vec![0],
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
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            rotation: 0,
            message: None,
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
        },
    );
    assert!(apply_rotate_block(&team1, &world, pos1, true).is_some());
    assert!(
        apply_rotate_block(&team5, &world, pos1, true).is_none(),
        "team 5 cannot rotate team 1's tile"
    );
    // The break plan ownership check: team 1 cannot start breaking the
    // team-5 tile via apply_build_plans.
    let mut snap_player = team1.clone();
    let break_plan = BuildPlan {
        breaking: true,
        position: pos,
        block: 0,
        rotation: 0,
        config: Vec::new(),
    };
    let world_arc = Arc::new(world);
    let admin = test_admin();
    apply_build_plans(
        &mut snap_player,
        &[break_plan],
        &world_arc,
        &Arc::new(DashMap::<i32, PendingConnection>::new()),
        &admin,
        true,
    )
    .unwrap();
    assert!(
        world_arc.pending_breaks.get(&pos).is_none(),
        "cross-team break rejected"
    );
    // Item transfer ownership: the team-5 player can withdraw from their
    // own container but not from the team-1 one.
    let mut team5_carrier = team5.clone();
    assert!(
        withdraw_items_to_player(&mut team5_carrier, &world_arc, pos, 0, 5).is_none(),
        "team 5 cannot withdraw from team 1 container"
    );
    let mut own_carrier = team5.clone();
    // Give the team-5 player their own container (288) with items.
    let own_pos = (47 << 16) | 100;
    let mut own_tile = erekir_like_tile(own_pos, 345); // container
    own_tile.team = 5;
    own_tile.inventory = vec![(0, 10)];
    world_arc.tiles.insert(own_pos, own_tile);
    assert!(
        withdraw_items_to_player(&mut own_carrier, &world_arc, own_pos, 0, 3).is_some(),
        "own-team withdraw works"
    );
}

#[test]
fn request_block_snapshot_visibility_is_scoped_to_actor_team() {
    // SOL-002: official NetServer.requestBlockSnapshot sends the snapshot
    // only when `build.team == player.team()`. The legacy fixed `team ==
    // 1` gate denied valid PvP teams and leaked nothing; the actor-team
    // gate must accept the owning team and reject enemies for finished
    // buildings, in-progress builds and in-progress breaks alike.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let pos = (45 << 16) | 100;
    let mut own_tile = erekir_like_tile(pos, 270);
    own_tile.team = 5;
    world.tiles.insert(pos, own_tile);
    // Enemy team (1) must not see the team-5 building.
    assert!(
        matches!(
            request_block_snapshot_target(&world, pos, 1),
            SnapshotTarget::None
        ),
        "enemy team cannot see foreign building"
    );
    // The owning team sees the finished building.
    assert!(
        matches!(
            request_block_snapshot_target(&world, pos, 5),
            SnapshotTarget::Building(_)
        ),
        "owning team sees its building"
    );
    // An in-progress team-5 build is visible to team 5 but not team 1.
    let mut builder = player();
    builder.uuid = "builder-5".into();
    let pending = PendingBuild {
        position: pos,
        block: 216,
        rotation: 0,
        config: Vec::new(),
        occupied: vec![pos],
        team: 5,
        builder: builder.clone(),
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: 60.0,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(pos, pending.clone());
    assert!(
        matches!(
            request_block_snapshot_target(&world, pos, 5),
            SnapshotTarget::PendingBuild(_)
        ),
        "owning team sees its construct"
    );
    assert!(
        matches!(
            request_block_snapshot_target(&world, pos, 1),
            SnapshotTarget::None
        ),
        "enemy team cannot see foreign construct"
    );
    // An in-progress break of the team-5 tile follows the same gate.
    world.pending_builds.remove(&pos);
    world.pending_breaks.insert(
        pos,
        PendingBreak {
            position: pos,
            block: 270,
            occupied: vec![pos],
            dynamic: true,
            team: 5,
            builder: builder.clone(),
            last_seen: std::time::Instant::now(),
            remaining_ticks: 60.0,
        },
    );
    assert!(
        matches!(
            request_block_snapshot_target(&world, pos, 5),
            SnapshotTarget::PendingBreak(_)
        ),
        "owning team sees its deconstruct"
    );
    assert!(
        matches!(
            request_block_snapshot_target(&world, pos, 1),
            SnapshotTarget::None
        ),
        "enemy team cannot see foreign deconstruct"
    );
}

#[test]
fn unit_control_requires_friendly_alive_ai_unit_and_possession_rule() {
    // SOL-002: official InputHandler.unitControl gate is
    // possessionAllowed && allowAction(control) && unit.isAI() &&
    // unit.team == player.team() && !unit.dead. The legacy fixed
    // `unit.team == 1` denied PvP players their own AI units; the actor
    // team is derived from the live session.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let admin = test_admin();
    let mut team5 = player();
    team5.id = 1_000_005;
    team5.unit_id = 2_000_005;
    team5.uuid = "team5".into();
    world.players.insert(
        2_000_005,
        PlayerCombatState {
            uuid: "team5".into(),
            player_id: 1_000_005,
            unit_id: 2_000_005,
            x: 360.0,
            y: 800.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 5,
        },
    );
    // AI ally of team 5.
    let ally = EnemyUnit {
        id: 3_000_042,
        unit_type: 42,
        entity_class: 0,
        team: 5,
        x: 360.0,
        y: 800.0,
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
        status_agg: None,
    };
    world.enemies.insert(ally.id, ally);
    // Team-5 player can control their own AI unit.
    assert!(unit_control_allowed(&world, &admin, &team5, 2, 3_000_042));
    // A team-1 player cannot control the team-5 unit (no fixed team 1).
    let mut team1 = player();
    team1.uuid = "team1".into();
    world.players.insert(
        2,
        PlayerCombatState {
            uuid: "team1".into(),
            player_id: 1,
            unit_id: 2,
            x: 360.0,
            y: 800.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    assert!(
        !unit_control_allowed(&world, &admin, &team1, 2, 3_000_042),
        "enemy team cannot possess the unit"
    );
    // Dead units are not controllable.
    if let Some(mut dead) = world.enemies.get_mut(&3_000_042) {
        dead.health = 0.0;
    }
    assert!(
        !unit_control_allowed(&world, &admin, &team5, 2, 3_000_042),
        "dead units are not controllable"
    );
    if let Some(mut dead) = world.enemies.get_mut(&3_000_042) {
        dead.health = 100.0;
    }
    if let Some(mut combat) = world.players.get_mut(&team5.unit_id) {
        combat.dead = true;
    }
    assert!(
        !unit_control_allowed(&world, &admin, &team5, 2, 3_000_042),
        "dead players cannot control units"
    );
    if let Some(mut combat) = world.players.get_mut(&team5.unit_id) {
        combat.dead = false;
    }
    // possessionAllowed=false blocks possession entirely.
    world.wave_rules.write().possession_allowed = false;
    assert!(
        !unit_control_allowed(&world, &admin, &team5, 2, 3_000_042),
        "possessionAllowed=false rejects unit control"
    );
    // Unknown control types are rejected.
    world.wave_rules.write().possession_allowed = true;
    assert!(!unit_control_allowed(&world, &admin, &team5, 3, 3_000_042));
    // A player cannot control their own possessed unit id.
    assert!(!unit_control_allowed(&world, &admin, &team5, 2, 2_000_005));

    let previous = apply_unit_control(&world, &mut team5, 2, 3_000_042).unwrap();
    assert_eq!(previous, ControlledUnit::Core);
    assert_eq!(team5.controlled_unit, ControlledUnit::Standard(3_000_042));
    assert_eq!(
        controlling_player_for_unit(&world, 3_000_042),
        Some(team5.id)
    );
    assert!(
        !unit_control_allowed(&world, &admin, &team5, 2, 3_000_042),
        "a Player-controlled unit is no longer AI/possessable"
    );

    // TypeIO unit reference type 1 is a ControlBlock proxy. Friendly
    // turrets are possessable; non-ControlBlock and foreign tiles are not.
    for (x, block) in [(58, 356), (59, 359)] {
        let position = (x << 16) | 100;
        let mut non_control_block = erekir_like_tile(position, block);
        non_control_block.team = 5;
        world.tiles.insert(position, non_control_block);
        assert!(
            !unit_control_allowed(&world, &admin, &team5, 1, position),
            "block {block} does not implement ControlBlock"
        );
    }
    let turret_position = (60 << 16) | 100;
    let mut turret = erekir_like_tile(turret_position, 349); // duo
    turret.team = 5;
    world.tiles.insert(turret_position, turret);
    assert!(unit_control_allowed(
        &world,
        &admin,
        &team5,
        1,
        turret_position
    ));
    let previous = apply_unit_control(&world, &mut team5, 1, turret_position).unwrap();
    assert_eq!(previous, ControlledUnit::Standard(3_000_042));
    assert_eq!(
        team5.controlled_unit,
        ControlledUnit::Building(turret_position)
    );
    assert_eq!(
        controlling_player_for_building(&world, turret_position),
        Some(team5.id)
    );
    assert!(!unit_control_allowed(
        &world,
        &admin,
        &team1,
        1,
        turret_position
    ));

    // BuildingControlSelect back to a friendly core requires the player unit
    // (BlockUnit possession cannot core-respawn per official canControlSelect).
    let core_position = (61 << 16) | 100;
    let mut core = erekir_like_tile(core_position, 339);
    core.team = 5;
    world.tiles.insert(core_position, core);
    assert!(!building_control_select_allowed(
        &world,
        &team5,
        core_position
    ));
    team5.controlled_unit = ControlledUnit::Core;
    assert!(building_control_select_allowed(
        &world,
        &team5,
        core_position
    ));
    assert!(!building_control_select_allowed(
        &world,
        &team1,
        core_position
    ));
    let frame =
        encode_unit_building_control_select_frame(&team5, team5.controlled_unit, core_position)
            .unwrap();
    assert_eq!(frame[2], UNIT_BUILDING_CONTROL_SELECT_PACKET_ID);
    let payload = &frame[6..];
    assert_eq!(payload[0], 2); // TypeIO player unit (core respawn)
    assert_eq!(&payload[1..5], &team5.unit_id.to_be_bytes());
    assert_eq!(&payload[5..9], &core_position.to_be_bytes());
}

#[test]
fn unit_controller_sync_matches_typeio_command_ai_player_and_ground_layouts() {
    use crate::network::codec::Reads;

    let (world, _connections, _, _) = legacy_weapons_test_world();
    let mut ally = legacy_weapons_make_enemy(3_400_001, DAGGER, 360.0, 800.0, DAGGER.health);
    ally.team = 1;
    world.enemies.insert(ally.id, ally.clone());
    world.unit_orders.insert(
        ally.id,
        UnitOrder {
            unit_id: ally.id,
            command: 0,
            stances: (1 << 1) | (1 << 7),
            payload_cooldown: 0.0,
            target_kind: 2,
            target_id: 3_400_099,
            target_x: Some(500.0),
            target_y: Some(600.0),
            logic_control: 0,
            queue: vec![
                UnitOrderTarget {
                    kind: 1,
                    id: (40 << 16) | 100,
                    x: 320.0,
                    y: 800.0,
                },
                UnitOrderTarget {
                    kind: 2,
                    id: 3_400_098,
                    x: 400.0,
                    y: 700.0,
                },
                UnitOrderTarget {
                    kind: 0,
                    id: -1,
                    x: 700.0,
                    y: 900.0,
                },
            ],
        },
    );

    let mut encoded = Vec::new();
    write_unit_controller_sync(&mut encoded, Some(&world), &ally).unwrap();
    let mut input = std::io::Cursor::new(encoded);
    assert_eq!(input.read_b().unwrap(), 9);
    assert!(input.read_bool().unwrap());
    assert!(input.read_bool().unwrap());
    assert_eq!(input.read_f().unwrap(), 500.0);
    assert_eq!(input.read_f().unwrap(), 600.0);
    assert_eq!(input.read_b().unwrap(), 0); // attack target is Unit
    assert_eq!(input.read_i().unwrap(), 3_400_099);
    assert_eq!(input.read_b().unwrap(), 0); // move command
    assert_eq!(input.read_b().unwrap(), 3);
    assert_eq!(input.read_b().unwrap(), 0); // Building queue entry
    assert_eq!(input.read_i().unwrap(), (40 << 16) | 100);
    assert_eq!(input.read_b().unwrap(), 1); // Unit queue entry
    assert_eq!(input.read_i().unwrap(), 3_400_098);
    assert_eq!(input.read_b().unwrap(), 2); // Vec2 queue entry
    assert_eq!(input.read_f().unwrap(), 700.0);
    assert_eq!(input.read_f().unwrap(), 900.0);
    assert_eq!(input.read_b().unwrap(), 2);
    assert_eq!(input.read_b().unwrap(), 1);
    assert_eq!(input.read_b().unwrap(), 7);
    assert_eq!(input.position(), input.get_ref().len() as u64);

    // Possession changes the same unit to controller tag 0 + Player id.
    let mut controller = player();
    controller.controlled_unit = ControlledUnit::Standard(ally.id);
    world
        .player_sessions
        .insert(controller.unit_id, controller.clone());
    let mut encoded = Vec::new();
    write_unit_controller_sync(&mut encoded, Some(&world), &ally).unwrap();
    let mut input = std::io::Cursor::new(encoded);
    assert_eq!(input.read_b().unwrap(), 0);
    assert_eq!(input.read_i().unwrap(), controller.id);

    world.player_sessions.clear();
    ally.authority = crate::network::world::UnitAuthority::Logic {
        processor_pos: (12 << 16) | 8,
        remaining_ticks: 400.0,
        processor_generation: 0,
    };
    let mut encoded = Vec::new();
    write_unit_controller_sync(&mut encoded, Some(&world), &ally).unwrap();
    let mut input = std::io::Cursor::new(encoded.clone());
    assert_eq!(input.read_b().unwrap(), 3);
    assert_eq!(input.read_i().unwrap(), (12 << 16) | 8);
    let restored = read_unit_controller(&mut std::io::Cursor::new(encoded), ally.id).unwrap();
    assert!(matches!(
        restored.authority,
        crate::network::world::UnitAuthority::Logic {
            processor_pos, ..
        } if processor_pos == (12 << 16) | 8
    ));
    ally.authority = crate::network::world::UnitAuthority::DefaultAi;

    // The wave team remains generic AI even if an order happens to exist.
    world.player_sessions.clear();
    ally.team = world.wave_rules.read().wave_team;
    let mut encoded = Vec::new();
    write_unit_controller_sync(&mut encoded, Some(&world), &ally).unwrap();
    assert_eq!(encoded, vec![2]);

    // Player.writeSync references a possessed standard unit and does not
    // include a second stale Alpha entity.
    controller.controlled_unit = ControlledUnit::Standard(3_400_001);
    let payload = encode_initial_entity_snapshot(&controller, None).unwrap();
    let mut payload = std::io::Cursor::new(payload);
    assert_eq!(payload.read_s().unwrap(), 1);
    let data_len = payload.read_s().unwrap() as usize;
    let mut body = vec![0; data_len];
    std::io::Read::read_exact(&mut payload, &mut body).unwrap();
    assert_eq!(body[4], PLAYER_CLASS_ID);
    assert_eq!(body[body.len() - 13], 2);
    assert_eq!(
        i32::from_be_bytes(body[body.len() - 12..body.len() - 8].try_into().unwrap()),
        3_400_001
    );
}

#[test]
fn typeio_read_controller_roundtrip_and_adversarial_layouts() {
    use crate::network::codec::{Reads, Writes};

    let (world, _connections, _, _) = legacy_weapons_test_world();
    let mut ally = legacy_weapons_make_enemy(3_400_002, DAGGER, 360.0, 800.0, DAGGER.health);
    ally.team = 1;
    world.enemies.insert(ally.id, ally.clone());

    // Happy-path CommandAI roundtrip: flags, unit target, queue, stances.
    let mut encoded = Vec::new();
    encoded.write_b(9).unwrap();
    encoded.write_bool(true).unwrap();
    encoded.write_bool(true).unwrap();
    encoded.write_f(111.0).unwrap();
    encoded.write_f(222.0).unwrap();
    encoded.write_b(0).unwrap(); // Unit attack target
    encoded.write_i(3_400_099).unwrap();
    encoded.write_b(1).unwrap(); // repair
    encoded.write_b(3).unwrap();
    encoded.write_b(0).unwrap();
    encoded.write_i((40 << 16) | 100).unwrap();
    encoded.write_b(1).unwrap();
    encoded.write_i(3_400_098).unwrap();
    encoded.write_b(2).unwrap();
    encoded.write_f(50.0).unwrap();
    encoded.write_f(60.0).unwrap();
    encoded.write_b(2).unwrap();
    encoded.write_b(1).unwrap();
    encoded.write_b(7).unwrap();
    let restored = read_unit_controller(&mut std::io::Cursor::new(encoded), ally.id).unwrap();
    assert_eq!(restored.tag, 9);
    let order = restored.order.as_ref().unwrap();
    assert_eq!(order.command, 1);
    assert_eq!(order.target_kind, 2);
    assert_eq!(order.target_id, 3_400_099);
    assert_eq!(order.target_x, Some(111.0));
    assert_eq!(order.target_y, Some(222.0));
    assert_eq!(order.queue.len(), 3);
    assert_eq!(order.queue[0].kind, 1);
    assert_eq!(order.queue[1].kind, 2);
    assert_eq!(order.queue[2].kind, 0);
    assert_eq!(order.stances, (1 << 1) | (1 << 7));

    // Missing attack unit is kept (CommandAI.afterRead); missing queued
    // building/unit are dropped when the snapshot is applied.
    let mut encoded = Vec::new();
    encoded.write_b(9).unwrap();
    encoded.write_bool(true).unwrap();
    encoded.write_bool(false).unwrap();
    encoded.write_b(0).unwrap();
    encoded.write_i(9_999_999).unwrap();
    encoded.write_b(0).unwrap();
    encoded.write_b(2).unwrap();
    encoded.write_b(0).unwrap();
    encoded.write_i((99 << 16) | 99).unwrap();
    encoded.write_b(1).unwrap();
    encoded.write_i(8_888_888).unwrap();
    encoded.write_b(0).unwrap();
    let snapshot = read_unit_controller(&mut std::io::Cursor::new(encoded), ally.id).unwrap();
    assert_eq!(snapshot.order.as_ref().unwrap().target_id, 9_999_999);
    assert_eq!(snapshot.order.as_ref().unwrap().queue.len(), 2);
    apply_controller_snapshot(&world, ally.id, snapshot);
    // DashMap shard locks are not reentrant: the unit_orders Ref must be
    // dropped before the later apply_controller_snapshot (which inserts)
    // or a same-shard collision deadlocks on 2-CPU runners.
    {
        let applied = world.unit_orders.get(&ally.id).unwrap();
        assert_eq!(applied.target_id, 9_999_999);
        assert!(applied.queue.is_empty());
    }

    // Invalid / legacy command ids become move (0). 255 is signed -1.
    for raw in [50u8, 255] {
        let mut encoded = Vec::new();
        encoded.write_b(9).unwrap();
        encoded.write_bool(false).unwrap();
        encoded.write_bool(false).unwrap();
        encoded.write_b(raw).unwrap();
        encoded.write_b(0).unwrap();
        encoded.write_b(0).unwrap();
        let snapshot = read_unit_controller(&mut std::io::Cursor::new(encoded), ally.id).unwrap();
        assert_eq!(snapshot.order.unwrap().command, 0, "raw command {raw}");
    }

    // Garbage queue kind 3 is skipped without extra bytes.
    let mut encoded = Vec::new();
    encoded.write_b(9).unwrap();
    encoded.write_bool(false).unwrap();
    encoded.write_bool(false).unwrap();
    encoded.write_b(0).unwrap();
    encoded.write_b(2).unwrap();
    encoded.write_b(3).unwrap();
    encoded.write_b(2).unwrap();
    encoded.write_f(7.0).unwrap();
    encoded.write_f(8.0).unwrap();
    encoded.write_b(0).unwrap();
    let snapshot = read_unit_controller(&mut std::io::Cursor::new(encoded), ally.id).unwrap();
    assert_eq!(snapshot.order.unwrap().queue.len(), 1);

    // Missing Player id does not replace the live controller.
    ally.authority = crate::network::world::UnitAuthority::Command;
    world.enemies.insert(ally.id, ally.clone());
    let mut encoded = Vec::new();
    encoded.write_b(0).unwrap();
    encoded.write_i(1_234_567).unwrap();
    let snapshot = read_unit_controller(&mut std::io::Cursor::new(encoded), ally.id).unwrap();
    apply_controller_snapshot(&world, ally.id, snapshot);
    assert!(matches!(
        world.enemies.get(&ally.id).unwrap().authority,
        crate::network::world::UnitAuthority::Command
    ));
}

#[test]
fn controller_save_tag9_roundtrip_matches_java_afterread_semantics() {
    use crate::network::units::{unit_has_active_rts_command, unit_is_logic_controllable};

    let (world, _connections, _, _) = legacy_weapons_test_world();
    let wall_pos = (8 << 16) | 8;
    let mut wall = crate::network::world::DynamicTile {
        position: wall_pos,
        block: 22,
        team: 6,
        ..Default::default()
    };
    crate::network::world::stamp_new_building(&world, &mut wall);
    world.tiles.insert(wall_pos, wall);

    let unit_id = 3_400_200;
    let foe_id = 3_400_201;
    let mut ally = legacy_weapons_make_enemy(unit_id, DAGGER, 80.0, 80.0, 440.0);
    ally.unit_type = 22; // mega
    ally.team = 1;
    ally.authority = crate::network::world::UnitAuthority::Command;
    world.enemies.insert(unit_id, ally);
    let mut foe = legacy_weapons_make_enemy(foe_id, DAGGER, 120.0, 120.0, DAGGER.health);
    foe.team = 6;
    world.enemies.insert(foe_id, foe);
    world.unit_orders.insert(
        unit_id,
        UnitOrder {
            unit_id,
            command: 1,
            stances: (1 << 1) | (1 << 2),
            payload_cooldown: 0.0,
            target_kind: 2,
            target_id: foe_id,
            target_x: Some(120.0),
            target_y: Some(120.0),
            logic_control: 0,
            queue: vec![
                UnitOrderTarget {
                    kind: 1,
                    id: wall_pos,
                    x: 0.0,
                    y: 0.0,
                },
                UnitOrderTarget {
                    kind: 2,
                    id: foe_id,
                    x: 0.0,
                    y: 0.0,
                },
                UnitOrderTarget {
                    kind: 0,
                    id: -1,
                    x: 400.0,
                    y: 400.0,
                },
            ],
        },
    );

    let unit = world.enemies.get(&unit_id).unwrap().clone();
    let mut encoded = Vec::new();
    write_unit_controller_sync(&mut encoded, Some(&world), &unit).unwrap();
    assert_eq!(encoded[0], 9);
    let mut cursor = std::io::Cursor::new(&encoded);
    assert_eq!(cursor.read_b().unwrap(), 9);
    assert!(cursor.read_bool().unwrap());
    assert!(cursor.read_bool().unwrap());
    assert_eq!(cursor.read_f().unwrap(), 120.0);
    assert_eq!(cursor.read_f().unwrap(), 120.0);
    assert_eq!(cursor.read_b().unwrap(), 0);
    assert_eq!(cursor.read_i().unwrap(), foe_id);
    assert_eq!(cursor.read_b().unwrap(), 1);
    assert_eq!(cursor.read_b().unwrap(), 3);
    assert_eq!(cursor.read_b().unwrap(), 0);
    assert_eq!(cursor.read_i().unwrap(), wall_pos);
    assert_eq!(cursor.read_b().unwrap(), 1);
    assert_eq!(cursor.read_i().unwrap(), foe_id);
    assert_eq!(cursor.read_b().unwrap(), 2);
    assert_eq!(cursor.read_f().unwrap(), 400.0);
    assert_eq!(cursor.read_f().unwrap(), 400.0);
    assert_eq!(cursor.read_b().unwrap(), 2);
    assert_eq!(cursor.read_b().unwrap(), 1);
    assert_eq!(cursor.read_b().unwrap(), 2);

    roundtrip_controller_save(&world, unit_id).unwrap();
    {
        let order = world.unit_orders.get(&unit_id).unwrap();
        assert_eq!(order.command, 1);
        assert_eq!(order.target_kind, 2);
        assert_eq!(order.target_id, foe_id);
        assert_eq!(order.queue.len(), 3);
    }
    assert!(unit_has_active_rts_command(&world, unit_id));
    assert!(!unit_is_logic_controllable(&world, unit_id));

    // Phantom attack unit: id survives read, afterRead clears attack only.
    world.enemies.remove(&foe_id);
    roundtrip_controller_save(&world, unit_id).unwrap();
    {
        let order = world.unit_orders.get(&unit_id).unwrap();
        assert_eq!(order.target_kind, 0);
        assert_eq!(order.target_id, -1);
        assert_eq!(order.target_x, Some(120.0));
    }
    assert!(unit_has_active_rts_command(&world, unit_id));

    // Missing queued unit and destroyed building are dropped on read.
    world.unit_orders.insert(
        unit_id,
        UnitOrder {
            unit_id,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(100.0),
            target_y: Some(100.0),
            logic_control: 0,
            queue: vec![
                UnitOrderTarget {
                    kind: 2,
                    id: 9_999,
                    x: 0.0,
                    y: 0.0,
                },
                UnitOrderTarget {
                    kind: 0,
                    id: -1,
                    x: 200.0,
                    y: 200.0,
                },
            ],
        },
    );
    roundtrip_controller_save(&world, unit_id).unwrap();
    let order = world.unit_orders.get(&unit_id).unwrap();
    assert_eq!(order.queue.len(), 1);
    assert_eq!(order.queue[0].kind, 0);
    assert!((order.queue[0].x - 200.0).abs() < f32::EPSILON);
}

#[test]
fn controller_save_logic_tag3_only_persists_processor_pos() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let proc_pos = (8 << 16) | 8;
    let mut proc = crate::network::world::DynamicTile {
        position: proc_pos,
        block: 431,
        team: 1,
        ..Default::default()
    };
    crate::network::world::stamp_new_building(&world, &mut proc);
    world.tiles.insert(proc_pos, proc);

    let unit_id = 3_400_210;
    let mut ally = legacy_weapons_make_enemy(unit_id, DAGGER, 80.0, 80.0, DAGGER.health);
    ally.team = 1;
    ally.authority = crate::network::world::UnitAuthority::Logic {
        processor_pos: proc_pos,
        remaining_ticks: 123.45,
        processor_generation: 0,
    };
    world.enemies.insert(unit_id, ally);
    world.unit_orders.insert(
        unit_id,
        UnitOrder {
            unit_id,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: 2,
            queue: Vec::new(),
        },
    );

    let unit = world.enemies.get(&unit_id).unwrap().clone();
    let mut body = Vec::new();
    write_unit_controller_sync(&mut body, Some(&world), &unit).unwrap();
    assert_eq!(
        body,
        vec![3u8]
            .into_iter()
            .chain(proc_pos.to_be_bytes())
            .collect::<Vec<_>>()
    );

    roundtrip_controller_save(&world, unit_id).unwrap();
    let unit = world.enemies.get(&unit_id).unwrap();
    assert!(matches!(
        unit.authority,
        crate::network::world::UnitAuthority::Logic {
            processor_pos,
            remaining_ticks,
            ..
        } if processor_pos == proc_pos && (remaining_ticks - 600.0).abs() < f32::EPSILON
    ));
    assert!(!world.unit_orders.contains_key(&unit_id));
}

#[test]
fn controller_save_generic_tag2_fallback_preserved() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let unit_id = 3_400_220;
    let mut wave = legacy_weapons_make_enemy(unit_id, FLARE, 80.0, 80.0, FLARE.health);
    wave.team = world.wave_rules.read().wave_team;
    wave.authority = crate::network::world::UnitAuthority::DefaultAi;
    world.enemies.insert(unit_id, wave);

    let unit = world.enemies.get(&unit_id).unwrap().clone();
    let mut body = Vec::new();
    write_unit_controller_sync(&mut body, Some(&world), &unit).unwrap();
    assert_eq!(body, vec![2]);
}

#[test]
fn pvp_command_authority_is_scoped_to_authenticated_actor_team() {
    // SOL-002: CommandBuilding and the three unit-command RPCs use the
    // authenticated player's combat team, not a team encoded by the
    // client. A mixed request may only mutate the actor's own entries;
    // the other team's state remains unchanged until its own actor sends
    // a request.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;

    let combat = |id: i32, team: u8| PlayerCombatState {
        uuid: format!("command-authority-{id}"),
        player_id: 1_100_000 + id,
        unit_id: 2_100_000 + id,
        x: 360.0,
        y: 800.0,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    let mut team5_player = player();
    team5_player.id = 1_100_005;
    team5_player.unit_id = 2_100_005;
    let mut team1_player = player();
    team1_player.id = 1_100_001;
    team1_player.unit_id = 2_100_001;
    world.players.insert(team5_player.unit_id, combat(5, 5));
    world.players.insert(team1_player.unit_id, combat(1, 1));
    let team5 = player_team(&world, &team5_player);
    let team1 = player_team(&world, &team1_player);
    assert_eq!((team5, team1), (5, 1));

    let team1_building = (50 << 16) | 100;
    let team5_building = (51 << 16) | 100;
    let mut team1_factory = erekir_like_tile(team1_building, 377);
    team1_factory.team = 1;
    let mut team5_factory = erekir_like_tile(team5_building, 377);
    team5_factory.team = 5;
    world.tiles.insert(team1_building, team1_factory);
    world.tiles.insert(team5_building, team5_factory);

    // Team 5 cannot command team 1's factory, but can command its own.
    let mixed_buildings = [team1_building, team5_building];
    assert!(apply_command_building_for_team(
        &world,
        team5,
        &mixed_buildings,
        510.0,
        810.0,
    ));
    assert!(world.building_commands.get(&team1_building).is_none());
    assert_eq!(
        world
            .building_commands
            .get(&team5_building)
            .map(|command| (command.target_x, command.target_y)),
        Some((510.0, 810.0)),
    );
    // Team 1 remains authoritative for its own factory.
    assert!(apply_command_building_for_team(
        &world,
        team1,
        &[team1_building],
        110.0,
        810.0,
    ));
    assert_eq!(
        world
            .building_commands
            .get(&team1_building)
            .map(|command| (command.target_x, command.target_y)),
        Some((110.0, 810.0)),
    );

    let team1_unit_id = 3_100_001;
    let team5_unit_id = 3_100_005;
    let mut team1_unit = legacy_weapons_make_enemy(team1_unit_id, DAGGER, 360.0, 800.0, 150.0);
    team1_unit.team = 1;
    let mut team5_unit = legacy_weapons_make_enemy(team5_unit_id, DAGGER, 368.0, 800.0, 150.0);
    team5_unit.team = 5;
    world.enemies.insert(team1_unit_id, team1_unit);
    world.enemies.insert(team5_unit_id, team5_unit);

    let move_request = CommandUnitsRequest {
        unit_ids: vec![team1_unit_id, team5_unit_id],
        build_target: -1,
        unit_target_type: 0,
        unit_target_id: -1,
        pos_x: 900.0,
        pos_y: 901.0,
        queue_command: false,
        final_batch: true,
    };
    assert!(apply_command_units_for_team(&world, team5, &move_request));
    assert!(world.unit_orders.get(&team1_unit_id).is_none());
    assert_eq!(
        world
            .unit_orders
            .get(&team5_unit_id)
            .map(|order| (order.target_x, order.target_y)),
        Some((Some(900.0), Some(901.0))),
    );
    // Team 1's unit becomes commandable only from a team-1 actor.
    assert!(apply_command_units_for_team(&world, team1, &move_request));
    assert_eq!(
        world
            .unit_orders
            .get(&team1_unit_id)
            .map(|order| (order.target_x, order.target_y)),
        Some((Some(900.0), Some(901.0))),
    );

    // SetUnitCommand and SetUnitStance use the same ownership gate.
    assert!(apply_set_unit_command_for_team(
        &world,
        team5,
        &[team1_unit_id, team5_unit_id],
        5,
    ));
    assert_eq!(
        world
            .unit_orders
            .get(&team1_unit_id)
            .map(|order| order.command),
        Some(0),
        "team 5 cannot change team 1's command",
    );
    assert_eq!(
        world
            .unit_orders
            .get(&team5_unit_id)
            .map(|order| order.command),
        Some(5),
    );
    assert!(apply_set_unit_command_for_team(
        &world,
        team1,
        &[team1_unit_id],
        5,
    ));
    assert_eq!(
        world
            .unit_orders
            .get(&team1_unit_id)
            .map(|order| order.command),
        Some(5),
    );

    assert!(apply_set_unit_stance_for_team(
        &world,
        team5,
        &[team1_unit_id, team5_unit_id],
        1,
        true,
    ));
    assert!(!unit_has_stance(&world, team1_unit_id, 1));
    assert!(unit_has_stance(&world, team5_unit_id, 1));
    assert!(apply_set_unit_stance_for_team(
        &world,
        team1,
        &[team1_unit_id],
        1,
        true,
    ));
    assert!(unit_has_stance(&world, team1_unit_id, 1));
}

#[test]
fn power_node_config_write_updates_power_links_both_directions() {
    // SOL-010: a live TileConfig write to a power node mutates tile.power_links
    // exactly like PowerNode.config (PowerNode.java:60-94):
    //  - Point2[] (tag 8, packed relative points — TypeIO.writeObject
    //    TypeIO.java:72-79) replaces the link set; same-team targets get the
    //    reverse link (configure() addUnique on the other side);
    //  - Integer (tag 1, absolute packed position) toggles ONE link — the
    //    same bytes toggle it back OFF, removing the reverse link;
    //  - an empty Point2[] clears every link.
    let (world, _connections, core_x, core_y) = legacy_weapons_test_world();
    let mut actor = player();
    actor.x = core_x;
    actor.y = core_y;
    let node = (45 << 16) | 100; // power-node (302), team 1
    let laser = (46 << 16) | 100; // laser-drill (327), team 1, adjacent
    let solar = (49 << 16) | 100; // solar-panel (313), team 1 — 4 tiles away
                                  // (not adjacent to the laser, so only the node link reaches it)
    for (position, block) in [(node, 302), (laser, 327), (solar, 313)] {
        let mut tile = DynamicTile {
            position,
            block,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![position],
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
            health: f32::MAX,
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
        tile.occupied = vec![position];
        world.tiles.insert(position, tile);
    }

    // Point2[] { (1,0), (2,0) } — the client's auto-link payload
    // (PowerNodeBuild.onConfigureBuildTapped -> Point2[] relative points).
    let mut set_config = vec![8, 2];
    for dx in [1i32, 4] {
        set_config.extend_from_slice(&(dx << 16).to_be_bytes());
    }
    assert!(apply_tile_config(&actor, &world, node, &set_config));
    // Clone instead of binding the DashMap read guard: a bound guard stays
    // alive to the end of the test scope and deadlocks the next get_mut.
    let node_links = world
        .tiles
        .get(&node)
        .map(|t| t.power_links.clone())
        .unwrap();
    assert_eq!(
        node_links,
        vec![laser, solar],
        "Point2[] config replaces the node's links"
    );
    assert_eq!(
        world.tiles.get(&laser).unwrap().power_links,
        vec![node],
        "same-team target gets the reverse link (PowerNode.java:71-73)"
    );
    assert_eq!(
        world.tiles.get(&solar).unwrap().power_links,
        vec![node],
        "same-team solar gets the reverse link"
    );
    // The graph now reaches the laser: solar 0.12 / laser demand 1.1.
    let power = compute_power_efficiency(&world);
    assert!(
        (power.get(&laser).copied().unwrap_or(0.0) - 0.12 / 1.1).abs() < 0.0001,
        "configured links rewire the live power graph"
    );

    // Integer toggle of ONE link (the client's tap payload, PowerNode.java:
    // 419-428): toggles the laser link OFF and removes its reverse link.
    let mut toggle_config = vec![1];
    toggle_config.extend_from_slice(&laser.to_be_bytes());
    assert!(apply_tile_config(&actor, &world, node, &toggle_config));
    assert_eq!(
        world.tiles.get(&node).unwrap().power_links,
        vec![solar],
        "Integer toggle removes the laser link"
    );
    assert_eq!(
        world.tiles.get(&laser).unwrap().power_links,
        Vec::<i32>::new(),
        "reverse link removed on toggle-off"
    );

    // The SAME toggle bytes toggle the link back ON (stateful handler, so the
    // config-equality no-op check must not apply).
    assert!(apply_tile_config(&actor, &world, node, &toggle_config));
    assert_eq!(
        world.tiles.get(&node).unwrap().power_links,
        vec![solar, laser],
        "identical Integer config toggles the link back on"
    );
    assert_eq!(
        world.tiles.get(&laser).unwrap().power_links,
        vec![node],
        "reverse link re-added"
    );

    // Empty Point2[] clears every link (double-tap 'clear links',
    // PowerNode.java:429-438 -> configure(new Point2[0])).
    let clear_config = vec![8, 0];
    assert!(apply_tile_config(&actor, &world, node, &clear_config));
    assert_eq!(
        world.tiles.get(&node).unwrap().power_links,
        Vec::<i32>::new(),
        "empty Point2[] clears all links"
    );
    assert_eq!(
        world.tiles.get(&laser).unwrap().power_links,
        Vec::<i32>::new()
    );
    assert_eq!(
        world.tiles.get(&solar).unwrap().power_links,
        Vec::<i32>::new()
    );
    let power = compute_power_efficiency(&world);
    assert_eq!(
        power.get(&laser).copied(),
        Some(0.0),
        "cleared links cut the graph edge"
    );

    // Link validation mirrors PowerNode.linkValid: a far/absent target is
    // not linked, and the config is still accepted (echoed to the client).
    let mut far_config = vec![8, 1];
    far_config.extend_from_slice(&((100i32 << 16) | 200).to_be_bytes());
    assert!(apply_tile_config(&actor, &world, node, &far_config));
    assert_eq!(
        world.tiles.get(&node).unwrap().power_links,
        Vec::<i32>::new(),
        "an out-of-range/absent target is not linked"
    );
}

#[test]
fn constructions_advance_only_while_builder_active() {
    // SOL-003: build work advances with the world loop delta while the
    // placing player is active, pauses when they idle, and completes
    // through finish_pending_build (no wall-clock tokio timers).
    let (world, connections, _, _) = legacy_weapons_test_world();
    *world.game_state.core_items.write() = vec![100; 22];
    let pos = (44 << 16) | 104;
    let active = PendingBuild {
        position: pos,
        block: 257, // conveyor: 1 copper, fast build
        rotation: 0,
        config: vec![0],
        occupied: vec![pos],
        team: 1,
        builder: player(),
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: 60.0,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(pos, active);
    // Active builder: 6 ticks of work per 6.0 delta step.
    simulate_constructions(&world, &connections, 6.0);
    assert_eq!(
        world.pending_builds.get(&pos).unwrap().remaining_ticks,
        54.0,
        "active build advances"
    );
    // Idle builder (last_seen beyond the 5 s activity window): work
    // pauses.
    if let Some(mut build) = world.pending_builds.get_mut(&pos) {
        build.last_seen = std::time::Instant::now() - std::time::Duration::from_secs(6);
    }
    simulate_constructions(&world, &connections, 6.0);
    assert_eq!(
        world.pending_builds.get(&pos).unwrap().remaining_ticks,
        54.0,
        "idle build does not advance"
    );
    // Re-activate and burn the remaining work: the plan finishes and the
    // tile is created (finish_pending_build consumed the conveyor cost).
    if let Some(mut build) = world.pending_builds.get_mut(&pos) {
        build.last_seen = std::time::Instant::now();
        build.remaining_ticks = 6.0;
    }
    simulate_constructions(&world, &connections, 6.0);
    assert!(world.pending_builds.get(&pos).is_none(), "plan removed");
    let tile = world.tiles.get(&pos).unwrap();
    assert_eq!(tile.block, 257, "conveyor built");
    assert_eq!(world.game_state.core_items.read()[0], 99, "cost paid");
}

#[test]
fn attack_mode_adds_enemy_cores_as_spawn_points() {
    // SOL-006: in Attack the crux (team 2) cores are additional wave
    // spawn points (official WaveSpawner).
    let mut spawns = vec![(10, 10), (11, 10)];
    let buildings = [
        crate::engine::world_stream::NetworkBuilding {
            position: (40 << 16) | 100,
            block: 341,
            health: 6000.0,
            rotation: 0,
            team: 1,
            inventory: Vec::new(),
            power_links: Vec::new(),
            power_status: 0.0,
            liquids: Vec::new(),
            enabled: true,
            extra_data: Vec::new(),
        },
        crate::engine::world_stream::NetworkBuilding {
            position: (160 << 16) | 100,
            block: 341,
            health: 6000.0,
            rotation: 0,
            team: 2,
            inventory: Vec::new(),
            power_links: Vec::new(),
            power_status: 0.0,
            liquids: Vec::new(),
            enabled: true,
            extra_data: Vec::new(),
        },
        crate::engine::world_stream::NetworkBuilding {
            position: (90 << 16) | 100,
            block: 350, // not a core
            health: 500.0,
            rotation: 0,
            team: 2,
            inventory: Vec::new(),
            power_links: Vec::new(),
            power_status: 0.0,
            liquids: Vec::new(),
            enabled: true,
            extra_data: Vec::new(),
        },
    ];
    extend_attack_spawns(&mut spawns, &buildings);
    assert!(spawns.contains(&(160, 100)), "enemy core added");
    assert!(!spawns.contains(&(40, 100)), "player core not added");
    assert!(!spawns.contains(&(90, 100)), "non-core not added");
    assert_eq!(spawns.len(), 3);
    // Idempotent.
    extend_attack_spawns(&mut spawns, &buildings);
    assert_eq!(spawns.len(), 3);
}

#[test]
fn simulation_delta_matches_target_tps() {
    // SOL-005: 60 TPS -> 1 game tick per step (official); the legacy
    // default 10 TPS -> 6 ticks per 100 ms step (same accumulated time).
    assert_eq!(simulation_delta_for_tps(60), 1.0);
    assert_eq!(simulation_delta_for_tps(10), 6.0);
    assert_eq!(simulation_delta_for_tps(30), 2.0);
    assert_eq!(
        simulation_delta_for_tps(0),
        60.0,
        "tps 0 clamps to 1 -> delta 60"
    );
    // The tick-rate * delta product is constant: 60 ticks per second.
    for tps in [1u32, 10, 30, 60, 120] {
        let per_second = tps.max(1) as f32 * simulation_delta_for_tps(tps);
        assert!(
            (per_second - 60.0).abs() < 0.001,
            "tps {tps} -> {per_second}"
        );
    }
}

#[test]
fn simulation_delta_from_elapsed_matches_official_frame_delta() {
    // SOL-005: Java Logic.update advances the game by
    // `frameTime * 60f` game ticks per frame (arc caps the raw frame
    // delta at 1 s -> at most 60 game ticks per update). The world loop
    // derives its delta from the wall-clock span between processed
    // steps, so steady state equals the TPS helper and overloaded
    // servers catch up instead of drifting.
    // Steady state 60 TPS: a 1/60 s step is exactly 1 game tick.
    assert!((simulation_delta_from_elapsed(1.0 / 60.0) - 1.0).abs() < 1e-4);
    // Legacy 10 TPS step (100 ms): 6 game ticks, the same accumulated
    // time as simulation_delta_for_tps(10).
    assert!((simulation_delta_from_elapsed(0.1) - 6.0).abs() < 1e-4);
    // Under load the clock catches up with wall time: a 500 ms stall
    // produces 30 game ticks of work in the next processed step.
    assert!((simulation_delta_from_elapsed(0.5) - 30.0).abs() < 1e-4);
    // ...capped at 60 game ticks per loop (arc's 1 s frame cap).
    assert_eq!(simulation_delta_from_elapsed(2.0), 60.0);
    assert_eq!(simulation_delta_from_elapsed(60.0), 60.0);
    // Zero, negative and non-finite spans never produce work.
    assert_eq!(simulation_delta_from_elapsed(0.0), 0.0);
    assert_eq!(simulation_delta_from_elapsed(-1.0), 0.0);
    assert_eq!(simulation_delta_from_elapsed(f64::NAN), 0.0);
    assert_eq!(simulation_delta_from_elapsed(f64::INFINITY), 0.0);
    // Steady-state equivalence with the TPS helper at every supported
    // tick rate: per-step game time is 1/tps seconds either way.
    for tps in [1u32, 10, 30, 60, 120, 240] {
        let from_elapsed = simulation_delta_from_elapsed(1.0 / f64::from(tps));
        let from_tps = simulation_delta_for_tps(tps);
        assert!(
            (from_elapsed - from_tps).abs() < 1e-3,
            "tps {tps}: elapsed-derived {from_elapsed} != tps-derived {from_tps}"
        );
    }
}

fn pvp_assign_team_alternates_sharded_blue() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    // Register the teams the map actually owns (SOL-004): sharded (1),
    // blue (5) and a third core team (7) for the dynamic pool.
    for (team, pos) in [
        (1u8, (30 << 16) | 30),
        (5, (40 << 16) | 40),
        (7, (50 << 16) | 50),
    ] {
        world.cores.insert(
            team,
            TeamCore {
                position: pos,
                block: 339,
                health: 6000.0,
                max_health: 6000.0,
            },
        );
    }
    let player = |id: i32, team: u8| PlayerCombatState {
        uuid: format!("pvp-{id}"),
        player_id: 1_000_000 + id,
        unit_id: 2_000_000 + id,
        x: 0.0,
        y: 0.0,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    // Non-PvP modes always use the rules default team (sharded).
    assert_eq!(assign_team_for_join(&world, "p1", 1), 1);

    *world.game_state.mode.write() = GameMode::Pvp;
    // With three registered cores the pool is [1, 5, 7]: players
    // round-robin to the least populated team (ties -> lowest id).
    assert_eq!(assign_team_for_join(&world, "p1", 1), 1);
    world.players.insert(2_000_001, player(1, 1));
    assert_eq!(assign_team_for_join(&world, "p2", 1), 5);
    world.players.insert(2_000_002, player(2, 5));
    assert_eq!(assign_team_for_join(&world, "p3", 1), 7);
    world.players.insert(2_000_003, player(3, 7));
    // All at count 1: the tie-break picks the lowest team id (sharded).
    assert_eq!(assign_team_for_join(&world, "p4", 1), 1);
    world.players.insert(2_000_004, player(4, 1));
    // Counts 1:2, 5:1, 7:1 -> blue.
    assert_eq!(assign_team_for_join(&world, "p5", 1), 5);
    world.players.insert(2_000_005, player(5, 5));
    // Counts 1:2, 5:2, 7:1 -> team 7.
    assert_eq!(assign_team_for_join(&world, "p6", 1), 7);
    // A dead core team leaves the pool: destroying team 5's core shrinks
    // it to [1, 7]; the next player goes to the least populated (7:2,
    // 1:2 -> tie -> lowest id 1).
    world.cores.get_mut(&5).unwrap().health = 0.0;
    world.players.insert(2_000_006, player(6, 7));
    assert_eq!(assign_team_for_join(&world, "p7", 1), 1);
    // Team names parse like Team.get(id) in the official server.
    assert_eq!(parse_team_id("blue"), Some(5));
    assert_eq!(parse_team_id("sharded"), Some(1));
    assert_eq!(parse_team_id("7"), Some(7));
    assert_eq!(parse_team_id("nope"), None);
}

#[test]
fn pvp_two_joins_get_distinct_teams_when_two_cores() {
    let (world, _, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    for (team, pos) in [(1u8, (30 << 16) | 30), (5u8, (40 << 16) | 40)] {
        world.cores.insert(
            team,
            TeamCore {
                position: pos,
                block: 339,
                health: 6000.0,
                max_health: 6000.0,
            },
        );
    }
    let first = assign_team_for_join(&world, "join-a", 1);
    world.players.insert(
        2_000_100,
        PlayerCombatState {
            uuid: "join-a".into(),
            player_id: 1_000_100,
            unit_id: 2_000_100,
            x: 0.0,
            y: 0.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: first,
        },
    );
    let second = assign_team_for_join(&world, "join-b", 1);
    assert_ne!(
        first, second,
        "two PvP joins with ≥2 cores must split teams"
    );
    assert!(matches!(first, 1 | 5) && matches!(second, 1 | 5));
}

#[test]
fn pvp_state_snapshot_core_data_lists_every_core_team() {
    let (world, _, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    for (team, pos) in [(1u8, (30 << 16) | 30), (5u8, (40 << 16) | 40)] {
        world.cores.insert(
            team,
            TeamCore {
                position: pos,
                block: 339,
                health: 6000.0,
                max_health: 6000.0,
            },
        );
    }
    let payload = encode_state_snapshot_for(&world.game_state, Some(&world)).unwrap();
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    input.read_f().unwrap();
    input.read_i().unwrap();
    input.read_i().unwrap();
    input.read_bool().unwrap();
    input.read_bool().unwrap();
    input.read_i().unwrap();
    input.read_b().unwrap();
    input.read_l().unwrap();
    input.read_l().unwrap();
    let _len = input.read_s().unwrap();
    let pos = input.position() as usize;
    let mut core = std::io::Cursor::new(&input.get_ref()[pos..]);
    let count = core.read_b().unwrap();
    assert!(
        count > 1,
        "coreData must list every team that still has a core"
    );
    let mut seen = Vec::new();
    for _ in 0..count {
        seen.push(core.read_b().unwrap());
        let items = core.read_s().unwrap();
        for _ in 0..items {
            core.read_s().unwrap();
            core.read_i().unwrap();
        }
    }
    assert_eq!(seen, vec![1, 5]);
}

#[test]
fn pvp_shot_damages_enemy_core_not_own() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    *world.game_state.core_health.write() = 400.0;
    world.cores.insert(
        1,
        TeamCore {
            position: (80 << 16) | 80,
            block: 339,
            health: 400.0,
            max_health: 1100.0,
        },
    );
    world.cores.insert(
        5,
        TeamCore {
            position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
            block: 340,
            health: 500.0,
            max_health: 3000.0,
        },
    );
    let bullet = |_id, team, sx, sy, tx, ty| Projectile {
        target_id: -1,
        shooter_id: -1,
        team,
        bullet_id: 65,
        damage: 40.0,
        splash_damage: 0.0,
        splash_radius: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        pierce_units: 0,
        pierce_buildings: 0,
        spawn_reign_frags: false,
        homing_range: 0.0,
        enemy_target_position: None,
        enemy_target_core: false,
        apply_direct_on_impact: false,
        armor_multiplier: 1.0,
        remaining_ticks: 8.0,
        total_ticks: 8.0,
        source_x: sx,
        source_y: sy,
        target_x: tx,
        target_y: ty,
        lifetime_scale: 1.0,
        source_position: None,
        damage_interval: None,
        damage_timer: 0.0,
    };
    world.projectiles.insert(
        4_200_001,
        bullet(
            4_200_001,
            1,
            80.0 * 8.0 + 40.0,
            80.0 * 8.0,
            80.0 * 8.0 - 40.0,
            80.0 * 8.0,
        ),
    );
    assert!(!simulate_pvp_player_damage(&world, &connections));
    assert_eq!(core_health_for_team(&world, 1), 400.0);
    assert!(world.projectiles.contains_key(&4_200_001));
    world.projectiles.clear();

    world.projectiles.insert(
        4_200_002,
        bullet(4_200_002, 1, core_x + 40.0, core_y, core_x - 40.0, core_y),
    );
    assert!(simulate_pvp_player_damage(&world, &connections));
    assert_eq!(core_health_for_team(&world, 5), 460.0);
    assert_eq!(core_health_for_team(&world, 1), 400.0);
    assert!(!world.projectiles.contains_key(&4_200_002));
}

#[test]
fn survival_keeps_sharded_vs_crux() {
    let (world, _, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Survival;
    world.cores.insert(
        1,
        TeamCore {
            position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
            block: 339,
            health: 6000.0,
            max_health: 6000.0,
        },
    );
    assert_eq!(assign_team_for_join(&world, "surv", 1), 1);
    assert_eq!(world.wave_rules.read().default_team, 1);
    assert_eq!(world.wave_rules.read().wave_team, 2);
    assert_eq!(spawn_enemy_units(&world, 0, 1, Some(12), Some(12)), 1);
    let enemy = world.enemies.iter().next().unwrap();
    assert_eq!(enemy.team, 2);
    drop(enemy);

    let payload = encode_state_snapshot_for(&world.game_state, Some(&world)).unwrap();
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    input.read_f().unwrap();
    input.read_i().unwrap();
    input.read_i().unwrap();
    input.read_bool().unwrap();
    input.read_bool().unwrap();
    input.read_i().unwrap();
    input.read_b().unwrap();
    input.read_l().unwrap();
    input.read_l().unwrap();
    let _len = input.read_s().unwrap();
    let pos = input.position() as usize;
    let mut core = std::io::Cursor::new(&input.get_ref()[pos..]);
    let count = core.read_b().unwrap();
    let mut seen = Vec::new();
    for _ in 0..count {
        seen.push(core.read_b().unwrap());
        let items = core.read_s().unwrap();
        for _ in 0..items {
            core.read_s().unwrap();
            core.read_i().unwrap();
        }
    }
    assert!(seen.contains(&1), "survival coreData includes sharded");
    assert!(
        !seen.contains(&2),
        "crux is the wave enemy, not a player coreData team unless it owns a core"
    );
}

#[test]
fn pvp_projectiles_damage_players_of_other_teams_only() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let player = |id: i32, team: u8, x: f32| PlayerCombatState {
        uuid: format!("pvp-target-{id}"),
        player_id: 1_000_000 + id,
        unit_id: 2_000_000 + id,
        x,
        y: core_y,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    world
        .players
        .insert(2_000_010, player(10, 1, core_x + 20.0));
    world
        .players
        .insert(2_000_011, player(11, 5, core_x + 40.0));
    // Blue (5) fires at the sharded (1) player: the bullet segment from
    // (core_x+40) to (core_x+20) passes through the victim.
    world.projectiles.insert(
        4_100_001,
        Projectile {
            target_id: 2_000_010,
            shooter_id: -1,
            team: 5,
            bullet_id: 65,
            damage: 11.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: false,
            armor_multiplier: 1.0,
            remaining_ticks: 8.0,
            total_ticks: 8.0,
            source_x: core_x + 40.0,
            source_y: core_y,
            target_x: core_x + 20.0,
            target_y: core_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    assert!(simulate_pvp_player_damage(&world, &connections));
    assert_eq!(world.players.get(&2_000_010).unwrap().health, 150.0 - 11.0);
    // A non-beam bullet is consumed on the first player hit.
    assert!(!world.projectiles.contains_key(&4_100_001));

    // Same-team projectiles never damage allies (collidesTeam=false).
    world.projectiles.insert(
        4_100_002,
        Projectile {
            target_id: 2_000_010,
            shooter_id: -1,
            team: 1,
            bullet_id: 65,
            damage: 11.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: false,
            armor_multiplier: 1.0,
            remaining_ticks: 8.0,
            total_ticks: 8.0,
            source_x: core_x,
            source_y: core_y,
            target_x: core_x + 20.0,
            target_y: core_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    assert!(!simulate_pvp_player_damage(&world, &connections));
    assert!(world.projectiles.contains_key(&4_100_002));
    assert_eq!(world.players.get(&2_000_010).unwrap().health, 139.0);

    // Outside PvP the pass is a no-op (survival keeps enemy-only combat).
    *world.game_state.mode.write() = GameMode::Survival;
    assert!(!simulate_pvp_player_damage(&world, &connections));
    assert!(world.projectiles.contains_key(&4_100_002));
}

#[test]
fn pvp_auto_pause_waits_for_two_teams() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let player = |id: i32, team: u8| PlayerCombatState {
        uuid: format!("pause-{id}"),
        player_id: 1_000_000 + id,
        unit_id: 2_000_000 + id,
        x: 0.0,
        y: 0.0,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    assert!(!world.game_state.is_paused.load(Ordering::Relaxed));
    // Only one team connected: waiting for players -> paused.
    world.players.insert(2_000_020, player(20, 1));
    update_pvp_auto_pause(&world);
    assert!(world.game_state.is_paused.load(Ordering::Relaxed));
    assert!(world.game_state.pvp_auto_paused.load(Ordering::Relaxed));
    // A second team joins: the game resumes automatically.
    world.players.insert(2_000_021, player(21, 5));
    update_pvp_auto_pause(&world);
    assert!(!world.game_state.is_paused.load(Ordering::Relaxed));
    assert!(!world.game_state.pvp_auto_paused.load(Ordering::Relaxed));
    // A manual console pause is never overridden by the auto-resume.
    world.game_state.is_paused.store(true, Ordering::Relaxed);
    world
        .game_state
        .pvp_auto_paused
        .store(false, Ordering::Relaxed);
    update_pvp_auto_pause(&world);
    assert!(world.game_state.is_paused.load(Ordering::Relaxed));
}

#[test]
fn attack_victory_when_last_enemy_core_falls() {
    // SOL-006: destroying the last enemy (team 2) core in Attack is a
    // VICTORY for the player (winner 1), not a defeat (winner 2).
    let (world, connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Attack;
    world.cores.insert(
        2,
        TeamCore {
            position: (200 << 16) | 100,
            block: 341,
            health: 6000.0,
            max_health: 6000.0,
        },
    );
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("attack-win".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    assert!(damage_team_core(&world, &connections, 2, 6000.0));
    assert!(world.game_state.game_over.load(Ordering::Relaxed));
    let mut frames = Vec::new();
    while let Ok(frame) = packet_rx.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            frames.push(packet);
        }
    }
    assert_eq!(&frames[0][1..], &[1], "player victory winner 1");
    // The player core path still ends in defeat (winner = enemy 2).
    let (world2, connections2, _, _) = legacy_weapons_test_world();
    *world2.game_state.mode.write() = GameMode::Attack;
    let (packet_tx2, mut packet_rx2) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections2.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx2,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("attack-loss".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    assert!(damage_team_core(&world2, &connections2, 1, 6000.0));
    assert!(world2.game_state.game_over.load(Ordering::Relaxed));
    let mut frames2 = Vec::new();
    while let Ok(frame) = packet_rx2.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            frames2.push(packet);
        }
    }
    assert_eq!(&frames2[0][1..], &[2], "player defeat winner 2");
}

#[test]
fn attack_mode_waves_destroy_player_core_and_game_over() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    // `mode attack`: attackMode rules keep waves enabled and the wave
    // enemy targets the player core; losing the last core ends the game.
    *world.game_state.mode.write() = GameMode::Attack;
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("attack".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    *world.game_state.core_health.write() = 9.0;
    world.enemies.insert(
        3_000_300,
        legacy_weapons_make_enemy(3_000_300, DAGGER, core_x, core_y, DAGGER.health),
    );
    assert!(!world.game_state.game_over.load(Ordering::Relaxed));
    // Waves are active in attack mode: the dagger fires its bolt at the
    // core (core_world target) and the impact destroys it.
    simulate_waves_and_enemies(&world, &connections, DAGGER.attack_reload);
    assert_eq!(world.projectiles.len(), 1);
    assert!(simulate_projectiles(&world, &connections, 0.0));
    assert!(world.game_state.game_over.load(Ordering::Relaxed));
    assert!(world.persistence_dirty.load(Ordering::Relaxed));
    // GameOverCallPacket (48) with winner = waveTeam (crux, 2): the last
    // team standing when the player core falls in attack mode.
    let mut game_over_frames = Vec::new();
    while let Ok(frame) = packet_rx.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            game_over_frames.push(packet);
        }
    }
    assert_eq!(game_over_frames.len(), 1);
    assert_eq!(&game_over_frames[0][1..], &[2]);
}

#[test]
fn pvp_snapshots_carry_real_teams_not_fixed_sharded() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let pcs = |id: i32, team: u8| PlayerCombatState {
        uuid: format!("snapshot-{id}"),
        player_id: 1_000_000 + id,
        unit_id: 2_000_000 + id,
        x: 10.0,
        y: 20.0,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    world.players.insert(2_000_030, pcs(30, 1));
    world.players.insert(2_000_031, pcs(31, 5));

    // StateSnapshot coreData lists both teams (official StateSnapshot
    // writes every active team; the 158.1 client builds TeamData per team).
    let payload = encode_state_snapshot_for(&world.game_state, Some(&world)).unwrap();
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    input.read_f().unwrap();
    input.read_i().unwrap();
    input.read_i().unwrap();
    input.read_bool().unwrap();
    input.read_bool().unwrap();
    input.read_i().unwrap();
    input.read_b().unwrap();
    input.read_l().unwrap();
    input.read_l().unwrap();
    let core_data_len = input.read_s().unwrap() as usize;
    let pos = input.position() as usize;
    let mut core_data = std::io::Cursor::new(&input.get_ref()[pos..]);
    let teams = core_data.read_b().unwrap();
    let mut seen = Vec::new();
    for _ in 0..teams {
        let team = core_data.read_b().unwrap();
        seen.push(team);
        let count = core_data.read_s().unwrap();
        for _ in 0..count {
            core_data.read_s().unwrap();
            core_data.read_i().unwrap();
        }
    }
    assert_eq!(seen, vec![1, 5]);
    assert!(core_data.position() as usize <= core_data_len);

    // The player entity snapshot (unit + player syncs) carries the real
    // team bytes instead of the hardcoded 1.
    let session = player();
    let combat = pcs(31, 5);
    let snapshot = encode_initial_entity_snapshot(&session, Some(&combat)).unwrap();
    let mut input = std::io::Cursor::new(snapshot);
    let count = input.read_s().unwrap();
    let data_len = input.read_s().unwrap() as usize;
    assert_eq!(count, 2);
    let pos = input.position() as usize;
    let mut body = std::io::Cursor::new(&input.get_ref()[pos..]);
    // Unit sync: id, class, abilities, aimX, aimY, controller, player id,
    // elevation, flag, health, shooting, mining, mounts, mount flags, x, y,
    // plans, rotation, shield, spawned, item, amount, statuses, team.
    body.read_i().unwrap();
    body.read_b().unwrap();
    body.read_b().unwrap();
    body.read_f().unwrap();
    body.read_f().unwrap();
    body.read_b().unwrap();
    body.read_i().unwrap();
    body.read_f().unwrap();
    body.read_l().unwrap();
    body.read_f().unwrap();
    body.read_bool().unwrap();
    body.read_i().unwrap();
    body.read_b().unwrap();
    body.read_b().unwrap();
    body.read_f().unwrap();
    body.read_f().unwrap();
    body.read_i().unwrap();
    body.read_f().unwrap();
    body.read_f().unwrap();
    body.read_bool().unwrap();
    body.read_s().unwrap();
    body.read_i().unwrap();
    let statuses = body.read_i().unwrap();
    for _ in 0..statuses {
        body.read_s().unwrap();
        body.read_f().unwrap();
    }
    assert_eq!(body.read_b().unwrap(), 5); // unit team = blue
    body.read_s().unwrap();
    body.read_bool().unwrap();
    body.read_f().unwrap();
    body.read_f().unwrap();
    body.read_f().unwrap();
    body.read_f().unwrap();
    // Player sync: id, class, admin, boosting, color, mouse, name,
    // selected, rotation, shooting, team.
    body.read_i().unwrap();
    body.read_b().unwrap();
    body.read_bool().unwrap();
    body.read_bool().unwrap();
    body.read_i().unwrap();
    body.read_f().unwrap();
    body.read_f().unwrap();
    body.read_typeio_string().unwrap();
    body.read_s().unwrap();
    body.read_i().unwrap();
    body.read_bool().unwrap();
    assert_eq!(body.read_b().unwrap(), 5); // player team = blue
    assert!(body.position() as usize <= data_len);
}

#[test]
fn client_plan_snapshot_forward_is_scoped_to_actor_team() {
    // SOL-002: plan previews must only reach players of the SAME team.
    // Three connections (ids 1..3) with authoritative player state on
    // teams 1, 1 and 5. Forwarding from the team-1 actor (id 1) must
    // reach the team-1 peer (id 2) but NOT the team-5 player (id 3).
    use crate::network::world::PendingConnection;
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let connections = DashMap::new();
    let mut receivers = std::collections::HashMap::new();
    for (id, team) in [(1i32, 1u8), (2, 1), (3, 5)] {
        let (tx, rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
        connections.insert(
            id,
            PendingConnection {
                ip: std::net::IpAddr::V4(std::net::Ipv4Addr::new(10, 0, 0, id as u8)),
                outbound: tx,
                udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
                udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
                udp_socket: None,
                player_name: Arc::new(parking_lot::RwLock::new(Some(format!("p{id}")))),
                outbound_drops: Arc::new(AtomicU64::new(0)),
                critical_drops: Arc::new(AtomicU64::new(0)),
                last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
                last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
                outbound_queued: Arc::new(AtomicU64::new(0)),
            },
        );
        world.players.insert(
            2_000_000 + id,
            PlayerCombatState {
                uuid: format!("plan-{id}"),
                player_id: 1_000_000 + id,
                unit_id: 2_000_000 + id,
                x: 0.0,
                y: 0.0,
                health: 150.0,
                shield: 0.0,
                status_effect: -1,
                status_duration: 0.0,
                statuses: Vec::new(),
                dead: false,
                respawn_timer: 0.0,
                team,
            },
        );
        receivers.insert(id, rx);
    }
    broadcast_plan_snapshot_team(&world, &connections, 1, 1, vec![0x42]);
    let mut receivers = receivers;
    let mut count = |id: i32| -> usize {
        let rx = receivers.get_mut(&id).unwrap();
        let mut n = 0usize;
        while let Ok(_frame) = rx.try_recv() {
            n += 1;
        }
        n
    };
    assert_eq!(count(1), 0, "sender must be excluded");
    assert_eq!(count(2), 1, "team-1 peer must receive the plan preview");
    assert_eq!(count(3), 0, "team-5 player must NOT receive the preview");
}

#[test]
fn legacy_units_fire_authoritative_projectiles_without_instant_damage() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    // (unit_type, spawn distance, simulated ticks, (bullet_id, count, damage)).
    // Counts follow src/game/unit_weapons.tsv: multi-mount units fire each
    // weapon on its own reload timer; volley.shots spawns one projectile
    // per shot. After the fire call the core must be untouched: the attack
    // is converted into projectiles instead of snapshot.attack_damage.
    type LegacyWeaponCase = (i16, f32, f32, &'static [(i16, usize, f32)]);
    let cases: &[LegacyWeaponCase] = &[
        (3, 100.0, 45.0, &[(10, 3, 70.0), (9, 6, 20.0)]),
        (9, 100.0, 350.0, &[(20, 1, 560.0)]),
        (11, 90.0, 9.0, &[(22, 1, 13.0)]),
        (12, 60.0, 14.0, &[(23, 1, 23.0)]),
        (13, 100.0, 45.0, &[(26, 1, 12.0), (25, 10, 40.0)]),
        (14, 100.0, 30.0, &[(27, 2, 110.0)]),
        (15, 100.0, 80.0, &[(30, 3, 9.0)]),
        (19, 100.0, 45.0, &[(36, 1, 115.0), (35, 8, 15.0)]),
        (21, 100.0, 30.0, &[(37, 1, 12.0)]),
        (22, 100.0, 24.0, &[(38, 1, 10.0), (39, 1, 8.0)]),
        (23, 100.0, 55.0, &[(40, 1, 154.0)]),
    ];
    for (unit_type, distance, delta, expected) in cases {
        let spec = enemy_spec(*unit_type).unwrap();
        let enemy_id = 3_000_000 + i32::from(*unit_type);
        world.enemies.insert(
            enemy_id,
            legacy_weapons_make_enemy(enemy_id, spec, core_x + distance, core_y, spec.health),
        );
        let core_before = *world.game_state.core_health.read();
        simulate_waves_and_enemies(&world, &connections, *delta);
        assert_eq!(
            *world.game_state.core_health.read(),
            core_before,
            "unit {unit_type} must not deal instant fallback damage"
        );
        let projectiles: Vec<_> = world
            .projectiles
            .iter()
            .map(|entry| entry.value().clone())
            .collect();
        for (bullet_id, count, damage) in *expected {
            let hits: Vec<_> = projectiles
                .iter()
                .filter(|projectile| projectile.bullet_id == *bullet_id)
                .collect();
            assert_eq!(hits.len(), *count, "unit {unit_type} bullet {bullet_id}");
            assert!(
                hits.iter()
                    .all(|projectile| { projectile.team == 2 && projectile.target_id == enemy_id }),
                "unit {unit_type} bullet {bullet_id} must be team 2 from the shooter"
            );
            assert!(
                hits.iter()
                    .all(|projectile| (projectile.damage - *damage).abs() < 0.001),
                "unit {unit_type} bullet {bullet_id} damage"
            );
        }
        world.enemies.clear();
        world.projectiles.clear();
    }

    // Status effects ride on the projectiles (tsv status_id -> server id).
    let spec = enemy_spec(11).unwrap();
    world.enemies.insert(
        3_000_011,
        legacy_weapons_make_enemy(3_000_011, spec, core_x + 90.0, core_y, spec.health),
    );
    simulate_waves_and_enemies(&world, &connections, 9.0);
    let slag = world.projectiles.iter().next().unwrap().value().clone();
    assert_eq!(slag.bullet_id, 22);
    assert_eq!(slag.status_effect, 8); // melting
    assert_eq!(slag.status_duration, 120.0);
    world.enemies.clear();
    world.projectiles.clear();

    let spec = enemy_spec(12).unwrap();
    world.enemies.insert(
        3_000_012,
        legacy_weapons_make_enemy(3_000_012, spec, core_x + 60.0, core_y, spec.health),
    );
    simulate_waves_and_enemies(&world, &connections, 14.0);
    let sap = world.projectiles.iter().next().unwrap().value().clone();
    assert_eq!(sap.bullet_id, 23);
    assert_eq!(sap.status_effect, 9); // sapped
    assert_eq!(sap.status_duration, 180.0);
    world.enemies.clear();
    world.projectiles.clear();

    // SAP lifesteal: a damaged spiroct heals sapStrength * damage when its
    // beam expires; the player in the beam takes the damage and gets
    // sapped, and the beam reaching the core damages it.
    let spiroct_id = 3_000_012;
    world.enemies.insert(
        spiroct_id,
        legacy_weapons_make_enemy(spiroct_id, spec, core_x + 60.0, core_y, 500.0),
    );
    world.players.insert(
        2_600_001,
        PlayerCombatState {
            uuid: "sap-target".into(),
            player_id: 1_600_001,
            unit_id: 2_600_001,
            x: core_x,
            y: core_y,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    simulate_waves_and_enemies(&world, &connections, 14.0);
    assert!(simulate_projectiles(&world, &connections, 40.0));
    assert_eq!(world.players.get(&2_600_001).unwrap().health, 127.0);
    assert_eq!(world.players.get(&2_600_001).unwrap().status_effect, 9);
    assert_eq!(*world.game_state.core_health.read(), 6_000.0 - 23.0);
    assert_eq!(world.enemies.get(&spiroct_id).unwrap().health, 511.5);
}

#[test]
fn legacy_units_allied_volleys_match_official_tables() {
    let make = |id: i32, spec: EnemySpec, health: f32| EnemyUnit {
        id,
        unit_type: spec.unit_type,
        entity_class: spec.entity_class,
        team: 1,
        x: 0.0,
        y: 0.0,
        rotation: 0.0,
        health,
        shield: 0.0,
        status_effect: -1,
        status_duration: f32::MAX,
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
        move_speed: spec.speed,
        attack_damage: spec.attack_damage,
        attack_reload_time: spec.attack_reload,
        attack_range: spec.attack_range,
        authority: UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        status_agg: None,
    };
    let count = |fire: &[AlliedWeaponFire], bullet_id: i16| {
        fire.iter()
            .filter(|fire| {
                matches!(fire, AlliedWeaponFire::Projectile(volley) if volley.bullet_id == bullet_id)
            })
            .count()
    };

    let mut scepter = make(1, enemy_spec(3).unwrap(), 9_000.0);
    scepter.team = 1;
    let fire = collect_allied_weapon_fire(&mut scepter, 45.0, 50.0).unwrap();
    assert_eq!((count(&fire, 10), count(&fire, 9)), (1, 6));

    let mut spiroct = make(2, enemy_spec(12).unwrap(), 1_000.0);
    spiroct.team = 1;
    let fire = collect_allied_weapon_fire(&mut spiroct, 36.0, 50.0).unwrap();
    assert_eq!((count(&fire, 23), count(&fire, 24)), (2, 2));

    let mut arkyid = make(3, enemy_spec(13).unwrap(), 8_000.0);
    arkyid.team = 1;
    let fire = collect_allied_weapon_fire(&mut arkyid, 45.0, 50.0).unwrap();
    assert_eq!((count(&fire, 26), count(&fire, 25)), (1, 10));

    let mut toxopid = make(4, enemy_spec(14).unwrap(), 22_000.0);
    toxopid.team = 1;
    let fire = collect_allied_weapon_fire(&mut toxopid, 240.0, 50.0).unwrap();
    assert_eq!((count(&fire, 27), count(&fire, 28)), (8, 1));

    let mut eclipse = make(5, enemy_spec(19).unwrap(), 22_000.0);
    eclipse.team = 1;
    let fire = collect_allied_weapon_fire(&mut eclipse, 45.0, 50.0).unwrap();
    assert_eq!((count(&fire, 36), count(&fire, 35)), (1, 8));

    let mut mega = make(6, enemy_spec(22).unwrap(), 460.0);
    mega.team = 1;
    let fire = collect_allied_weapon_fire(&mut mega, 30.0, 50.0).unwrap();
    assert_eq!((count(&fire, 38), count(&fire, 39)), (1, 2));

    // Single-weapon legacy units resolve through the shared volley table.
    for (unit_type, bullet_id, damage) in [
        (9, 20, 560.0),
        (11, 22, 13.0),
        (15, 30, 9.0),
        (21, 37, 12.0),
        (23, 40, 154.0),
    ] {
        let volley = enemy_projectile_volley(unit_type).unwrap();
        assert_eq!(volley.bullet_id, bullet_id);
        assert_eq!(volley.direct_damage, damage);
    }

    // Beam/sap lengths and lifesteal strengths match UnitTypes.java.
    for (bullet_id, length) in [
        (20, 460.0),
        (23, 75.0),
        (24, 40.0),
        (25, 55.0),
        (27, 90.0),
        (36, 230.0),
        (40, 30.0),
    ] {
        assert_eq!(unit_weapon_beam_length(bullet_id), Some(length));
    }
    assert_eq!(
        (sap_strength(23), sap_strength(24), sap_strength(25)),
        (0.5, 0.8, 0.85)
    );

    // Dual-mount table entries for the legacy units.
    assert!(naval_weapon_volleys(12)
        .is_some_and(|((a, _), (b, _))| { (a - 14.0).abs() < 0.001 && (b - 18.0).abs() < 0.001 }));
    assert!(naval_weapon_volleys(14)
        .is_some_and(|((a, _), (b, _))| { (a - 30.0).abs() < 0.001 && (b - 210.0).abs() < 0.001 }));
    assert!(naval_weapon_volleys(22)
        .is_some_and(|((a, _), (b, _))| { (a - 24.0).abs() < 0.001 && (b - 15.0).abs() < 0.001 }));
}

#[test]
fn legacy_heal_bolts_repair_allied_buildings_and_quad_splash_heals_allies() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let wall_position = (45 << 16) | 100;
    let wall_max = crate::game::content::block_health(216);
    world.tiles.insert(
        wall_position,
        DynamicTile {
            position: wall_position,
            block: 216,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![wall_position],
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
            health: wall_max - 10.0,
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
        },
    );

    // Mega heal bolt (bullet 38) repairs the allied wall by 5.5% max.
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        -1,
        Some(wall_position),
        MEGA_HEAL_A,
        core_x + 100.0,
        core_y,
        core_x + 40.0,
        core_y,
        0,
    );
    assert!(simulate_projectiles(&world, &connections, 40.0));
    let wall_health = world.tiles.get(&wall_position).unwrap().health;
    let after_mega = (wall_max - 10.0 + wall_max * 0.055).min(wall_max);
    assert!((wall_health - after_mega).abs() < 0.01);

    // Allied quad bomb (bullet 40): official BulletType.createSplashDamage
    // (158.1) heals only damaged allied BUILDINGS (indexer.eachBlock +
    // heals()); allied units are neither healed (no Damage.healUnits in
    // this path) nor damaged by friendly splash.
    let ally_id = 3_100_001;
    let dagger = enemy_spec(0).unwrap();
    world.enemies.insert(
        ally_id,
        EnemyUnit {
            id: ally_id,
            unit_type: dagger.unit_type,
            entity_class: dagger.entity_class,
            team: 1,
            x: core_x,
            y: core_y,
            rotation: 0.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: dagger.speed,
            attack_damage: dagger.attack_damage,
            attack_reload_time: dagger.attack_reload,
            attack_range: dagger.attack_range,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    // The mega bolt already filled the wall; damage it again so the quad
    // splash has a damaged building to repair.
    world.tiles.get_mut(&wall_position).unwrap().health = wall_max - 20.0;
    spawn_allied_unit_projectile(
        &world,
        &connections,
        0,
        -1,
        None,
        QUAD_BOMB,
        core_x + 100.0,
        core_y,
        core_x,
        core_y,
        0,
    );
    assert!(simulate_projectiles(&world, &connections, 80.0));
    // Allied units keep their health: no unit healing, no friendly fire.
    assert_eq!(world.enemies.get(&ally_id).unwrap().health, 100.0);
    // The splash heals the nearby damaged allied wall by 15% max.
    let wall_health = world.tiles.get(&wall_position).unwrap().health;
    let after_quad = (wall_max - 20.0 + wall_max * 0.15).min(wall_max);
    assert!((wall_health - after_quad).abs() < 0.01);
}

#[test]
fn vela_beam_volley_matches_official_and_repair_beam_heals_allies() {
    // Part B: the vela(8) volley exposes the official vela-weapon
    // (UnitTypes.java): ContinuousLaserBulletType damage 35, lifetime 160,
    // length 180, pierceCap -1 (unlimited pierce), reload 155.
    let vela_volley = enemy_projectile_volley(8).unwrap();
    assert_eq!(vela_volley.bullet_id, 18);
    assert_eq!(vela_volley.direct_damage, 35.0);
    assert_eq!(vela_volley.splash_damage, 0.0);
    assert_eq!(vela_volley.lifetime, 160.0);
    assert_eq!(vela_volley.speed, 0.0);
    assert_eq!(vela_volley.pierce_units, u8::MAX);
    assert_eq!(vela_volley.pierce_buildings, 0);
    assert_eq!(unit_weapon_beam_length(18), Some(180.0));
    assert_eq!(enemy_spec(8).unwrap().attack_reload, 155.0);

    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let vela = enemy_spec(8).unwrap();
    world.enemies.insert(
        3_080_000,
        legacy_weapons_make_enemy(3_080_000, vela, core_x + 100.0, core_y, vela.health),
    );
    world.players.insert(
        2_600_000,
        PlayerCombatState {
            uuid: "vela-beam-target".into(),
            player_id: 1_600_000,
            unit_id: 2_600_000,
            x: core_x + 60.0,
            y: core_y,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    // In range: after one 155-tick reload the vela fires its beam.
    simulate_waves_and_enemies(&world, &connections, 155.0);
    let beam = world.projectiles.iter().next().unwrap().clone();
    assert_eq!(beam.bullet_id, 18);
    assert_eq!(beam.damage, 35.0);
    assert_eq!(beam.team, 2);
    assert_eq!(beam.pierce_units, u8::MAX);
    assert_eq!(beam.total_ticks, 160.0);
    // The 180-length beam pierces the player on its way to the core and
    // the direct impact reaches the core (apply_direct_on_impact).
    assert!(simulate_projectiles(&world, &connections, 160.0));
    assert_eq!(world.players.get(&2_600_000).unwrap().health, 115.0);
    assert_eq!(*world.game_state.core_health.read(), 6_000.0 - 35.0);
    world.enemies.clear();
    world.players.clear();
    world.projectiles.clear();

    // Allied repair beam: vela team 1 heals the nearest damaged allied
    // unit by repairSpeed 1.4 per tick within maxRange 120. Buildings are
    // NOT repaired (official weapon defaults targetBuildings=false).
    let mut allied_vela = legacy_weapons_make_enemy(3_080_001, vela, core_x, core_y, vela.health);
    allied_vela.team = 1;
    world.enemies.insert(3_080_001, allied_vela);
    let mut ally = legacy_weapons_make_enemy(3_080_002, DAGGER, core_x + 50.0, core_y, 100.0);
    ally.team = 1;
    world.enemies.insert(3_080_002, ally);
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&3_080_002).unwrap().health, 114.0);
    // Out of the 120 range the beam does not reach the ally.
    world.enemies.get_mut(&3_080_002).unwrap().x = core_x + 200.0;
    world.enemies.get_mut(&3_080_002).unwrap().health = 100.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&3_080_002).unwrap().health, 100.0);
    world.enemies.clear();

    // Enemy side: a team-2 vela repairs its own damaged ally as well.
    world.enemies.insert(
        3_080_003,
        legacy_weapons_make_enemy(3_080_003, vela, core_x, core_y, vela.health),
    );
    world.enemies.insert(
        3_080_004,
        legacy_weapons_make_enemy(3_080_004, DAGGER, core_x + 50.0, core_y, 100.0),
    );
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&3_080_004).unwrap().health, 114.0);
    // The repair beam ignores buildings (documented deviation).
    let wall_position = (45 << 16) | 100;
    let wall_max = crate::game::content::block_health(216);
    world.tiles.insert(
        wall_position,
        DynamicTile {
            position: wall_position,
            block: 216,
            rotation: 0,
            team: 2,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![wall_position],
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
            health: wall_max - 10.0,
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
        },
    );
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(
        world.tiles.get(&wall_position).unwrap().health,
        wall_max - 10.0
    );
}

#[test]
fn oct_force_field_absorbs_damage_and_regenerates() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let oct = enemy_spec(24).unwrap();
    world.enemies.insert(
        3_024_000,
        legacy_weapons_make_enemy(3_024_000, oct, core_x, core_y, oct.health),
    );
    // The field is created at full hp (7000) for every living oct.
    assert!(simulate_oct_force_fields(&world, 1.0));
    assert_eq!(world.force_fields.get(&3_024_000).unwrap().hp, 7_000.0);

    let projectile = |damage: f32, x: f32, y: f32| Projectile {
        target_id: -1,
        shooter_id: -1,
        team: 1,
        bullet_id: 6,
        damage,
        splash_damage: 0.0,
        splash_radius: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        pierce_units: 0,
        pierce_buildings: 0,
        spawn_reign_frags: false,
        homing_range: 0.0,
        enemy_target_position: None,
        enemy_target_core: false,
        apply_direct_on_impact: true,
        armor_multiplier: 1.0,
        remaining_ticks: 1.0,
        total_ticks: 1.0,
        source_x: core_x,
        source_y: core_y,
        target_x: x,
        target_y: y,
        lifetime_scale: 1.0,
        source_position: None,
        damage_interval: None,
        damage_timer: 0.0,
    };
    // An allied bullet expiring inside the 140 radius is absorbed by the
    // area shield: the projectile is consumed and the field loses 9 hp.
    world
        .projectiles
        .insert(4_024_001, projectile(9.0, core_x + 10.0, core_y));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    assert!(!world.projectiles.contains_key(&4_024_001));
    assert_eq!(
        world.force_fields.get(&3_024_000).unwrap().hp,
        7_000.0 - 9.0
    );
    // Outside the radius nothing is absorbed.
    assert!(!oct_force_field_absorb(
        &world,
        1,
        core_x + 200.0,
        core_y,
        9.0
    ));
    // Regen: 4 hp per tick back toward 7000.
    assert!(simulate_oct_force_fields(&world, 1.0));
    assert_eq!(
        world.force_fields.get(&3_024_000).unwrap().hp,
        7_000.0 - 5.0
    );
    // A hit exhausting the remaining pool breaks the field: 480 ticks of
    // cooldown during which absorption and regen are both suspended.
    // (The official ability sets the shield to -cooldown*regen on break;
    // this port keeps the negative remainder as debt instead.)
    world
        .projectiles
        .insert(4_024_002, projectile(7_000.0 - 5.0, core_x + 10.0, core_y));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    let field = world.force_fields.get(&3_024_000).unwrap();
    assert_eq!(field.hp, 0.0);
    assert_eq!(field.remaining_ticks, 480.0);
    drop(field);
    assert!(!oct_force_field_absorb(
        &world,
        1,
        core_x + 10.0,
        core_y,
        9.0
    ));
    assert!(simulate_oct_force_fields(&world, 10.0));
    let field = world.force_fields.get(&3_024_000).unwrap();
    assert_eq!(field.hp, 0.0);
    assert_eq!(field.remaining_ticks, 470.0);
    drop(field);
    // After the cooldown expires the shield regenerates from zero.
    assert!(simulate_oct_force_fields(&world, 470.0));
    assert_eq!(
        world.force_fields.get(&3_024_000).unwrap().remaining_ticks,
        0.0
    );
    assert!(simulate_oct_force_fields(&world, 1.0));
    assert_eq!(world.force_fields.get(&3_024_000).unwrap().hp, 4.0);
    // When the oct dies its field disappears.
    world.enemies.remove(&3_024_000);
    assert!(simulate_oct_force_fields(&world, 1.0));
    assert!(!world.force_fields.contains_key(&3_024_000));
}

#[test]
fn oct_repair_field_heals_allies_every_120_ticks() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let oct = enemy_spec(24).unwrap();
    world.enemies.insert(
        3_024_100,
        legacy_weapons_make_enemy(3_024_100, oct, core_x, core_y, oct.health),
    );
    world.enemies.insert(
        3_024_101,
        legacy_weapons_make_enemy(
            3_024_101,
            enemy_spec(1).unwrap(),
            core_x + 50.0,
            core_y,
            100.0,
        ),
    );
    world.enemies.insert(
        3_024_102,
        legacy_weapons_make_enemy(3_024_102, DAGGER, core_x + 200.0, core_y, 100.0),
    );
    // simulation_time 240 crosses the 120-tick period: every allied unit
    // within 140 is healed by the official amount of 130 HP.
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&3_024_101).unwrap().health, 230.0);
    assert_eq!(world.enemies.get(&3_024_102).unwrap().health, 100.0);
    // Full-health allies are not overhealed.
    world.enemies.get_mut(&3_024_101).unwrap().health = 550.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&3_024_101).unwrap().health, 550.0);
    // The pulse only fires when the 120-tick boundary is crossed.
    *world.game_state.simulation_time.write() = 239.0;
    world.enemies.get_mut(&3_024_101).unwrap().health = 100.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&3_024_101).unwrap().health, 100.0);
    *world.game_state.simulation_time.write() = 240.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&3_024_101).unwrap().health, 230.0);
    world.enemies.clear();

    // An allied (team 1) oct also heals players inside the field.
    let mut allied_oct = legacy_weapons_make_enemy(3_024_103, oct, core_x, core_y, oct.health);
    allied_oct.team = 1;
    world.enemies.insert(3_024_103, allied_oct);
    world.players.insert(
        2_600_000,
        PlayerCombatState {
            uuid: "oct-player".into(),
            player_id: 1_600_000,
            unit_id: 2_600_000,
            x: core_x + 50.0,
            y: core_y,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.players.get(&2_600_000).unwrap().health, 150.0);
}

#[test]
fn allied_sap_lifesteal_heals_the_shooter() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    assert_eq!(
        (sap_strength(23), sap_strength(24), sap_strength(25)),
        (0.5, 0.8, 0.85)
    );
    let spiroct = enemy_spec(12).unwrap();
    let mut shooter = legacy_weapons_make_enemy(3_012_000, spiroct, core_x + 60.0, core_y, 500.0);
    shooter.team = 1;
    world.enemies.insert(3_012_000, shooter);
    world.enemies.insert(
        3_012_001,
        legacy_weapons_make_enemy(3_012_001, DAGGER, core_x, core_y, 150.0),
    );
    let sap = |bullet_id: i16, damage: f32, ticks: f32| Projectile {
        target_id: 3_012_001,
        shooter_id: 3_012_000,
        team: 1,
        bullet_id,
        damage,
        splash_damage: 0.0,
        splash_radius: 0.0,
        status_effect: 9,
        status_duration: 180.0,
        pierce_units: 0,
        pierce_buildings: 0,
        spawn_reign_frags: false,
        homing_range: 0.0,
        enemy_target_position: None,
        enemy_target_core: false,
        apply_direct_on_impact: true,
        armor_multiplier: 1.0,
        remaining_ticks: ticks,
        total_ticks: ticks,
        source_x: core_x + 60.0,
        source_y: core_y,
        target_x: core_x,
        target_y: core_y,
        lifetime_scale: 1.0,
        source_position: None,
        damage_interval: None,
        damage_timer: 0.0,
    };
    // spiroct-weapon: 23 damage * sapStrength 0.5 = 11.5 healed to the
    // allied shooter; the enemy dagger takes the full 23.
    world.projectiles.insert(4_012_001, sap(23, 23.0, 5.0));
    assert!(simulate_projectiles(&world, &connections, 5.0));
    assert_eq!(world.enemies.get(&3_012_001).unwrap().health, 127.0);
    assert_eq!(world.enemies.get(&3_012_000).unwrap().health, 511.5);
    // mount-purple-weapon: 18 * 0.8 = 14.4; arkyid sap: 40 * 0.85 = 34.
    // After the first hit the dagger is sapped (healthMultiplier 0.8), so
    // ShieldComp.damage divides the next two shots: 18/0.8 + 40/0.8.
    world.projectiles.insert(4_012_002, sap(24, 18.0, 5.0));
    world.projectiles.insert(4_012_003, sap(25, 40.0, 5.0));
    assert!(simulate_projectiles(&world, &connections, 5.0));
    assert_eq!(world.enemies.get(&3_012_001).unwrap().health, 54.5);
    assert_eq!(world.enemies.get(&3_012_000).unwrap().health, 559.9);
    // A team-2 shooter never benefits from allied lifesteal.
    world.enemies.insert(
        3_012_002,
        legacy_weapons_make_enemy(3_012_002, spiroct, core_x + 100.0, core_y, 500.0),
    );
    let mut enemy_sap = sap(23, 23.0, 5.0);
    enemy_sap.shooter_id = 3_012_002;
    world.projectiles.insert(4_012_004, enemy_sap);
    assert!(simulate_projectiles(&world, &connections, 5.0));
    assert_eq!(world.enemies.get(&3_012_002).unwrap().health, 500.0);
    assert_eq!(world.enemies.get(&3_012_001).unwrap().health, 25.75);
}

#[test]
fn scepter_burst_shot_delay_spaces_impacts_by_four_ticks() {
    // UnitTypes.java scepter-weapon: shoot.shots 3, shoot.shotDelay 4.
    assert_eq!(volley_shot_delay(10), 4.0);
    assert_eq!(volley_shot_delay(30), 3.0); // flare burst
    assert_eq!(volley_shot_delay(6), 0.0);
    let scepter_volley = enemy_projectile_volley(3).unwrap();
    assert_eq!(scepter_volley.bullet_id, 10);
    assert_eq!(scepter_volley.shots, 3);
    assert_eq!(scepter_volley.direct_damage, 70.0);

    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    world.enemies.insert(
        3_003_000,
        legacy_weapons_make_enemy(3_003_000, DAGGER, core_x, core_y, 1_000.0),
    );
    for shot_index in 0..3 {
        spawn_allied_unit_projectile(
            &world,
            &connections,
            0,
            3_003_000,
            None,
            SCEPTER_BOLT,
            core_x + 100.0,
            core_y,
            core_x,
            core_y,
            shot_index,
        );
    }
    let mut lifetimes: Vec<_> = world
        .projectiles
        .iter()
        .map(|projectile| projectile.total_ticks)
        .collect();
    lifetimes.sort_unstable_by(f32::total_cmp);
    // 100 / 8 speed = 12.5 flight + 0/4/8 shot delay.
    assert_eq!(lifetimes, vec![12.5, 16.5, 20.5]);
    // The burst impacts are spaced 4 ticks apart: each step hits exactly
    // one 70-damage bolt.
    assert!(simulate_projectiles(&world, &connections, 12.5));
    assert_eq!(world.enemies.get(&3_003_000).unwrap().health, 930.0);
    assert!(simulate_projectiles(&world, &connections, 4.0));
    assert_eq!(world.enemies.get(&3_003_000).unwrap().health, 860.0);
    assert!(simulate_projectiles(&world, &connections, 4.0));
    assert_eq!(world.enemies.get(&3_003_000).unwrap().health, 790.0);
    assert!(world.projectiles.is_empty());
}

#[test]
fn toxopid_cannon_spawns_nine_fragments() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let cannon = |team: u8| Projectile {
        target_id: -1,
        shooter_id: -1,
        team,
        bullet_id: 28,
        damage: 50.0,
        splash_damage: 75.0,
        splash_radius: 80.0,
        status_effect: 9,
        status_duration: 600.0,
        pierce_units: 0,
        pierce_buildings: 0,
        spawn_reign_frags: false,
        homing_range: 0.0,
        enemy_target_position: None,
        enemy_target_core: false,
        apply_direct_on_impact: true,
        armor_multiplier: 1.0,
        remaining_ticks: 1.0,
        total_ticks: 1.0,
        source_x: core_x + 20.0,
        source_y: core_y,
        target_x: core_x + 100.0,
        target_y: core_y,
        lifetime_scale: 1.0,
        source_position: None,
        damage_interval: None,
        damage_timer: 0.0,
    };
    // Enemy toxopid-cannon: on expiry it spawns the official 9 fragments
    // (bullet 29, 30 damage, splash 40/70, sapped 600).
    world.projectiles.insert(4_014_001, cannon(2));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    let frags: Vec<_> = world
        .projectiles
        .iter()
        .filter(|projectile| projectile.bullet_id == 29)
        .map(|projectile| projectile.clone())
        .collect();
    assert_eq!(frags.len(), 9);
    assert!(frags
        .iter()
        .all(|frag| frag.team == 2 && frag.damage == 30.0));
    assert!(frags
        .iter()
        .all(|frag| frag.splash_damage == 40.0 && frag.splash_radius == 70.0));
    assert!(frags
        .iter()
        .all(|frag| frag.status_effect == 9 && frag.status_duration == 600.0));
    assert!(frags.iter().all(|frag| frag.pierce_units == 0));
    world.projectiles.clear();

    // Allied toxopid-cannon spawns its nine fragments with team 1.
    world.projectiles.insert(4_014_002, cannon(1));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    assert_eq!(
        world
            .projectiles
            .iter()
            .filter(|projectile| projectile.bullet_id == 29 && projectile.team == 1)
            .count(),
        9
    );
}

#[test]
fn corvus_beam_heals_allied_building_twenty_five_percent() {
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    // Official corvus-weapon: LaserBulletType healPercent 25 + collidesTeam.
    let corvus_volley = enemy_projectile_volley(9).unwrap();
    assert_eq!(corvus_volley.bullet_id, 20);
    assert_eq!(corvus_volley.direct_damage, 560.0);
    assert_eq!(unit_weapon_beam_length(20), Some(460.0));
    let wall_max = crate::game::content::block_health(216);
    let wall_position = (45 << 16) | 100;
    let wall = DynamicTile {
        position: wall_position,
        block: 216,
        rotation: 0,
        team: 1,
        config: vec![0],
        enabled: true,
        message: None,
        occupied: vec![wall_position],
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
        health: 100.0,
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
    world.tiles.insert(wall_position, wall);
    let beam = |position: i32, team: u8| Projectile {
        target_id: -1,
        shooter_id: 0,
        team,
        bullet_id: 20,
        damage: 560.0,
        splash_damage: 0.0,
        splash_radius: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        pierce_units: 0,
        pierce_buildings: 0,
        spawn_reign_frags: false,
        homing_range: 0.0,
        enemy_target_position: Some(position),
        enemy_target_core: false,
        apply_direct_on_impact: true,
        armor_multiplier: 1.0,
        remaining_ticks: 1.0,
        total_ticks: 1.0,
        source_x: core_x + 100.0,
        source_y: core_y,
        target_x: (position >> 16) as i16 as f32 * 8.0,
        target_y: position as i16 as f32 * 8.0,
        lifetime_scale: 1.0,
        source_position: None,
        damage_interval: None,
        damage_timer: 0.0,
    };
    // An allied beam reaching an allied damaged wall heals 25% of max.
    world.projectiles.insert(4_020_001, beam(wall_position, 1));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    assert_eq!(
        world.tiles.get(&wall_position).unwrap().health,
        100.0 + wall_max * 0.25
    );
    // The same beam never heals an enemy wall: it damages it instead.
    let enemy_wall_position = ((i32::from(SPAWN_X) + 45) << 16) | i32::from(SPAWN_Y);
    world.tiles.insert(
        enemy_wall_position,
        DynamicTile {
            position: enemy_wall_position,
            block: 216,
            rotation: 0,
            team: 2,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![enemy_wall_position],
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
            health: wall_max,
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
        },
    );
    world
        .projectiles
        .insert(4_020_002, beam(enemy_wall_position, 1));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    let tombstone = world.tiles.get(&enemy_wall_position).unwrap();
    assert_eq!(tombstone.block, 0);
    assert_eq!(tombstone.health, 0.0);
}

#[test]
fn atrax_ignores_burning_and_melting_statuses() {
    assert!(unit_immune_to_status(11, 1));
    assert!(unit_immune_to_status(11, 8));
    assert!(!unit_immune_to_status(11, 9));
    assert!(!unit_immune_to_status(0, 1));
    // Official UnitTypes immunities (JAR 158.1, round-73 A5): mace/vela/
    // atrax + precept/vanquish/conquer are immune to burning (atrax and
    // 40/41/42 also to melting). navanax (34) has NO immunity writes in
    // UnitTypes$35 and naval units (25-29) only receive wet — they CAN
    // burn (resistance comes from liquid conversion, not hard immunity).
    assert!(unit_immune_to_status(1, 1));
    assert!(unit_immune_to_status(8, 1));
    assert!(
        !unit_immune_to_status(34, 1),
        "navanax is NOT immune to burning (JAR)"
    );
    assert!(
        !unit_immune_to_status(25, 1),
        "naval is NOT immune to burning (JAR)"
    );
    assert!(
        !unit_immune_to_status(25, 8),
        "naval is NOT immune to melting (JAR)"
    );
    assert!(
        !unit_immune_to_status(29, 8),
        "naval is NOT immune to melting (JAR)"
    );
    assert!(
        unit_immune_to_status(40, 1),
        "precept is immune to burning (JAR)"
    );
    assert!(
        unit_immune_to_status(41, 8),
        "vanquish is immune to melting (JAR)"
    );
    assert!(
        unit_immune_to_status(42, 1),
        "conquer is immune to burning (JAR)"
    );
    assert!(
        !unit_immune_to_status(1, 8),
        "mace is NOT immune to melting"
    );
    assert!(
        !unit_immune_to_status(34, 8),
        "navanax is NOT immune to melting"
    );
    assert!(
        !unit_immune_to_status(25, 9),
        "naval is not immune to sapped"
    );

    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    let atrax = enemy_spec(11).unwrap();
    world.enemies.insert(
        3_011_000,
        legacy_weapons_make_enemy(3_011_000, atrax, core_x, core_y, atrax.health),
    );
    world.enemies.insert(
        3_011_001,
        legacy_weapons_make_enemy(3_011_001, DAGGER, core_x + 30.0, core_y, 150.0),
    );
    let status_projectile = |damage: f32, status: i16, splash: bool| Projectile {
        target_id: 3_011_000,
        shooter_id: 0,
        team: 1,
        bullet_id: 6,
        damage,
        splash_damage: if splash { damage } else { 0.0 },
        splash_radius: if splash { 30.0 } else { 0.0 },
        status_effect: status,
        status_duration: 300.0,
        pierce_units: 0,
        pierce_buildings: 0,
        spawn_reign_frags: false,
        homing_range: 0.0,
        enemy_target_position: None,
        enemy_target_core: false,
        apply_direct_on_impact: true,
        armor_multiplier: 1.0,
        remaining_ticks: 1.0,
        total_ticks: 1.0,
        source_x: core_x + 50.0,
        source_y: core_y,
        target_x: core_x + 15.0,
        target_y: core_y,
        lifetime_scale: 1.0,
        source_position: None,
        damage_interval: None,
        damage_timer: 0.0,
    };
    // Burning (1) direct hit: the atrax takes the damage but never the
    // status; the control dagger receives both the hit and the status.
    world
        .projectiles
        .insert(4_011_001, status_projectile(9.0, 1, false));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    // Atrax armor 3 reduces the 9-damage bolt to 6 (official 10% floor).
    assert_eq!(
        world.enemies.get(&3_011_000).unwrap().health,
        atrax.health - 6.0
    );
    assert_eq!(world.enemies.get(&3_011_000).unwrap().status_effect, -1);
    assert_eq!(world.enemies.get(&3_011_001).unwrap().health, 150.0);
    assert_eq!(world.enemies.get(&3_011_001).unwrap().status_effect, -1);
    // Melting (8) splash: same guard at the splash application site.
    world
        .projectiles
        .insert(4_011_002, status_projectile(13.0, 8, true));
    assert!(simulate_projectiles(&world, &connections, 1.0));
    let atrax_unit = world.enemies.get(&3_011_000).unwrap();
    // Armor 3 reduces 13 to 10 with the official 10% floor; the atrax is
    // hit by the direct impact and the splash (10 + 10) and still never
    // receives melting.
    assert_eq!(atrax_unit.health, atrax.health - 6.0 - 10.0 - 10.0);
    assert_eq!(atrax_unit.status_effect, -1);
    drop(atrax_unit);
    let dagger = world.enemies.get(&3_011_001).unwrap();
    assert_eq!(dagger.health, 150.0 - 13.0);
    assert_eq!(dagger.status_effect, 8);
    assert_eq!(dagger.status_duration, 300.0);
    drop(dagger);
}

#[test]
fn enemy_volley_table_covers_all_legacy_units() {
    // Primary weapon of every legacy unit resolves through the volley
    // table (values from src/game/unit_weapons.tsv / UnitTypes.java).
    for (unit_type, bullet_id, damage, shots) in [
        (3, 10, 70.0, 3),   // scepter scepter-weapon
        (12, 23, 23.0, 1),  // spiroct spiroct-weapon
        (13, 26, 12.0, 1),  // arkyid large-purple-mount (primary in this port)
        (14, 27, 110.0, 2), // toxopid large-purple-mount
        (19, 36, 115.0, 1), // eclipse large-laser-mount
        (22, 38, 10.0, 1),  // mega heal-weapon-mount (heal-only)
        (30, 51, 12.0, 1),  // retusa retusa-weapon
        (33, 58, 30.0, 1),  // aegires point-defense-mount (approximation)
    ] {
        let volley = enemy_projectile_volley(unit_type).unwrap();
        assert_eq!(volley.bullet_id, bullet_id, "unit {unit_type} bullet");
        assert_eq!(volley.direct_damage, damage, "unit {unit_type} damage");
        assert_eq!(volley.shots, shots, "unit {unit_type} shots");
    }
    // Secondary mounts remain in the dedicated branches / naval table and
    // are intentionally absent from the volley (documented).
    assert!(enemy_projectile_volley(10).is_none());
    assert!(enemy_projectile_volley(16).is_none());
    assert!(enemy_projectile_volley(20).is_none());
}

#[test]
fn item_bridge_transfers_across_linked_endpoints() {
    // Full bridge chain: drill -> conveyor -> bridge sender (262, link
    // config to receiver) -> bridge receiver (262) -> conveyor -> core.
    // The official ItemBridge only transports from the configured end;
    // linkValid(checkDouble=true) requires the OTHER end to NOT point
    // back (one-way). This regression proves items cross the bridge and
    // arrive at the receiving conveyor.
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("test".into(), GameMode::Survival);
    *state.core_items.write() = vec![0; 22];
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state.clone(),
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
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
        tiles: DashMap::new(),
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::new(),
        mono_mining_targets: DashMap::new(),
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("unused-bridge-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let tile = |position: i32, block: i16, rotation: u8, config: Vec<u8>| DynamicTile {
        enabled: true,
        message: None,
        position,
        block,
        rotation,
        team: 1,
        config,
        occupied: vec![position],
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
        health: f32::MAX,
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
    // Horizontal chain at y=97: drill(30,97) -> conv(31,97) ->
    // bridgeA(32,97) -> bridgeB(34,97) -> conv(35..37,97) ->
    // conv(38,97, rot 1 = down) -> core cell (38,98). Every hop is
    // between adjacent tiles.
    let conv_in = (31 << 16) | 97;
    let bridge_a = (32 << 16) | 97;
    let bridge_b = (34 << 16) | 97;
    let conv_out = (35 << 16) | 97;
    let conv_out2 = (36 << 16) | 97;
    let conv_out3 = (37 << 16) | 97;
    let conv_down = (38 << 16) | 97;
    // Seed the chain with copper at staggered positions. The items must
    // cross conv -> bridge A -> bridge B -> convs -> core.
    let mut seeded = tile(conv_in, 257, 0, vec![0]);
    seeded.conveyor_items = vec![(0, 0.9), (0, 0.5), (0, 0.1)];
    seeded.stored_item = 0;
    seeded.stored_amount = 3;
    seeded.transport_progress = 0.9;
    world.tiles.insert(conv_in, seeded);
    // Bridge A points at bridge B (TypeIO Point2 delta: +2, +0).
    world.tiles.insert(
        bridge_a,
        tile(bridge_a, 262, 0, vec![7, 0, 0, 0, 2, 0, 0, 0, 0]),
    );
    // Bridge B is the passive receiving end: no config (link = -1).
    world
        .tiles
        .insert(bridge_b, tile(bridge_b, 262, 0, vec![0]));
    world
        .tiles
        .insert(conv_out, tile(conv_out, 257, 0, vec![0]));
    world
        .tiles
        .insert(conv_out2, tile(conv_out2, 257, 0, vec![0]));
    world
        .tiles
        .insert(conv_out3, tile(conv_out3, 257, 0, vec![0]));
    world
        .tiles
        .insert(conv_down, tile(conv_down, 257, 1, vec![0]));

    // Run the logistics simulation: the drill fills, conveyors move, and
    // the bridge must transport the item from A to B to the output.
    let mut received_at_core = 0i32;
    for _ in 0..600 {
        let power = compute_power_efficiency(&world);
        simulate_logistics(&world, 6.0, &power);
        received_at_core = state.core_items.read()[0];
        if received_at_core >= 3 {
            break;
        }
    }
    for (label, key) in [
        ("conv_in", conv_in),
        ("bridge_a", bridge_a),
        ("bridge_b", bridge_b),
        ("conv_out", conv_out),
        ("conv_down", conv_down),
    ] {
        if let Some(t) = world.tiles.get(&key) {
            println!(
                "{label}: item={} amount={} progress={:.2}",
                t.stored_item, t.stored_amount, t.transport_progress
            );
        }
    }
    println!("core copper: {}", state.core_items.read()[0]);
    assert!(
        received_at_core >= 3,
        "bridge must deliver items to the core; got {received_at_core}"
    );
}

#[test]
fn chat_rate_limiter_allows_spam_limit_per_window() {
    // Official NetClient.chatRate: 2 messages per 2000 ms window.
    let mut limiter = ChatRateLimiter::new();
    assert!(limiter.allow(2000, 2));
    assert!(limiter.allow(2000, 2));
    assert!(!limiter.allow(2000, 2), "third message in window rejected");
    assert!(!limiter.allow(2000, 2));
}

#[test]
fn client_command_dispatch_votekick_requires_players() {
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("client-command-test".into(), GameMode::Survival);
    let width = i32::from(map.width);
    let height = i32::from(map.height);
    let total = (width * height) as usize;
    let world = DynamicWorld {
        game_state: state,
        width,
        height,
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: vec![0; total],
        base_centers: vec![true; total],
        tile_data: vec![0; total],
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: vec![0; total],
        overlays: vec![0; total],
        enemy_spawns: map.enemy_spawns(),
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_001),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("client-command-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let connections = DashMap::new();
    let mut player = player();
    // With <3 players no vote is started.
    handle_client_command(
        &world,
        &connections,
        &mut player,
        "/votekick bob griefing",
        &test_admin(),
    );
    assert!(
        world.votekick_target.read().is_none(),
        "no vote with <3 players"
    );
    // /help and unknown commands never panic.
    handle_client_command(&world, &connections, &mut player, "/help", &test_admin());
    handle_client_command(&world, &connections, &mut player, "/bogus x", &test_admin());
    handle_client_command(&world, &connections, &mut player, "/stop", &test_admin());
    assert!(
        world.game_state.is_active(),
        "client /stop must not run the server console stop command"
    );
    // With 3 connections and a target named "bob" the vote starts.
    for i in 1..=3 {
        let name = if i == 2 {
            Some("bob".to_string())
        } else {
            None
        };
        connections.insert(
            i,
            PendingConnection {
                ip: format!("10.0.0.{i}").parse().unwrap(),
                outbound: tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY).0,
                udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
                udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
                udp_socket: None,
                player_name: Arc::new(parking_lot::RwLock::new(name)),
                outbound_drops: Arc::new(AtomicU64::new(0)),
                critical_drops: Arc::new(AtomicU64::new(0)),
                last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
                last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
                outbound_queued: Arc::new(AtomicU64::new(0)),
            },
        );
    }
    handle_client_command(
        &world,
        &connections,
        &mut player,
        "/votekick bob griefing",
        &test_admin(),
    );
    assert_eq!(
        world.votekick_target.read().as_deref(),
        Some("bob"),
        "vote target stored with 3 players"
    );
    // The initiator already voted +1; a second /vote y from the same
    // player is rejected as a duplicate.
    handle_client_command(&world, &connections, &mut player, "/vote y", &test_admin());
    assert!(
        world.votekick_target.read().is_some(),
        "duplicate vote rejected, vote still open"
    );
    // A no-vote decrements; then a yes from a second distinct player
    // reaches the required count (2 with 3 players) and resolves.
    handle_client_command(&world, &connections, &mut player, "/vote n", &test_admin());
    // (duplicate no is also rejected)
    handle_client_command(&world, &connections, &mut player, "/vote n", &test_admin());
    assert!(world.votekick_target.read().is_some(), "vote still open");
    // Force resolution by clearing voters and re-voting yes once more is
    // not possible for the same player; instead cancel as admin.
    player.admin = true;
    handle_client_command(&world, &connections, &mut player, "/vote c", &test_admin());
    assert!(world.votekick_target.read().is_none(), "admin canceled");
}
// ------------------------------------------------------------------
// Round 30: per-team cores (PvP/Attack maps carry one core per team).
// ------------------------------------------------------------------

#[test]
fn msav_cores_are_registered_per_team_from_frontier() {
    // (a) The .msav core extraction: frontier.msav carries two core
    // buildings — crux (team 2) at (205,71) and sharded (team 1) at
    // (105,183), both core-shard (339) with 1100 HP — and
    // fresh_world_from_template must register one core per team.
    use crate::engine::world_stream::replace_map_from_msav;
    let Some(frontier) = official_msav("frontier.msav") else {
        return;
    };
    let stream = replace_map_from_msav(include_bytes!("../../dummy_world.dat"), &frontier).unwrap();
    let state = GameState::new();
    state.start_hosting("frontier".into(), GameMode::Attack);
    let world = fresh_world_from_template(
        &state,
        stream,
        "frontier".into(),
        std::env::temp_dir().join("frontier-cores-test.json"),
    )
    .unwrap();
    assert_eq!(registered_core_teams(&world), vec![1, 2]);
    let core = |team: u8| world.cores.get(&team).map(|entry| *entry.value()).unwrap();
    assert_eq!(core(1).position, (105 << 16) | 183);
    assert_eq!(core(1).block, 339);
    assert_eq!(core(1).health, 1100.0);
    assert_eq!(core(1).max_health, 1100.0);
    assert_eq!(core(2).position, (205 << 16) | 71);
    assert_eq!(core(2).block, 339);
    assert_eq!(core(2).health, 1100.0);
    // Spawn/position helpers resolve per team; the legacy single-core
    // view still points at the sharded core.
    assert_eq!(core_position_for_team(&world, 1), (105 << 16) | 183);
    assert_eq!(core_position_for_team(&world, 2), (205 << 16) | 71);
    assert_eq!(core_world_for_team(&world, 2), (205.0 * 8.0, 71.0 * 8.0));
    assert_eq!(core_team_at_position(&world, (205 << 16) | 71), Some(2));
    assert_eq!(core_team_at_position(&world, (105 << 16) | 183), Some(1));
    assert_eq!(world.core_position, (105 << 16) | 183);
    assert_eq!(core_world(&world), (105.0 * 8.0, 183.0 * 8.0));
}

#[test]
fn player_spawn_resolves_to_own_team_core() {
    // (b) Spawn: every spawn path (join, respawn, broadcast) resolves the
    // player's OWN team core. A blue (5) player must land on the blue
    // core, not the sharded one.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    world.cores.insert(
        1,
        TeamCore {
            position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
            block: 339,
            health: 1100.0,
            max_health: 1100.0,
        },
    );
    world.cores.insert(
        5,
        TeamCore {
            position: (120 << 16) | 60,
            block: 340,
            health: 3000.0,
            max_health: 3000.0,
        },
    );
    assert_eq!(core_position_for_team(&world, 5), (120 << 16) | 60);
    assert_eq!(core_world_for_team(&world, 5), (120.0 * 8.0, 60.0 * 8.0));
    assert_eq!(
        core_world_for_team(&world, 1),
        (f32::from(SPAWN_X) * 8.0, f32::from(SPAWN_Y) * 8.0)
    );

    // The actual respawn machinery uses the team core.
    let mut session = player();
    session.id = 1_000_001;
    session.unit_id = 2_000_001;
    session.uuid = "spawn-team-core".into();
    world
        .player_sessions
        .insert(session.unit_id, session.clone());
    world.players.insert(
        session.unit_id,
        PlayerCombatState {
            uuid: session.uuid.clone(),
            player_id: session.id,
            unit_id: session.unit_id,
            x: 0.0,
            y: 0.0,
            health: 1.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: true,
            respawn_timer: 0.0,
            team: 5,
        },
    );
    let old_unit = session.unit_id;
    respawn_session_player(&mut session, &world).unwrap();
    let new_unit = session.unit_id;
    assert_ne!(new_unit, old_unit);
    let combat = world.players.get(&new_unit).unwrap();
    assert_eq!(combat.x, 120.0 * 8.0, "team-5 respawn at blue core");
    assert_eq!(combat.y, 60.0 * 8.0);
    assert_eq!(combat.team, 5);
    drop(combat);
    // broadcast_respawn writes the same team core into PLAYER_SPAWN.
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let connections: DashMap<i32, PendingConnection> = DashMap::new();
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("spawn".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    broadcast_respawn(&connections, &session, &world, Some(old_unit)).unwrap();
    let mut spawn_position = None;
    while let Ok(frame) = packet_rx.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == PLAYER_SPAWN_PACKET_ID {
            use crate::network::codec::Reads;
            let mut cursor = std::io::Cursor::new(&packet[1..]);
            spawn_position = cursor.read_i().ok();
        }
    }
    assert_eq!(spawn_position, Some((120 << 16) | 60));
}

#[test]
fn pvp_game_over_is_per_team_elimination() {
    // (c) PvP game over: destroying one team's core eliminates that team;
    // the game ends when a single player team still has a live core, and
    // that team is the reported winner.
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let team_core = |x: f32| TeamCore {
        position: ((x / 8.0) as i32) << 16 | ((core_y / 8.0) as i32),
        block: 339,
        health: 100.0,
        max_health: 100.0,
    };
    world.cores.insert(1, team_core(core_x));
    world.cores.insert(5, team_core(core_x + 512.0));
    // GameState.core_health is the authority for the sharded core; keep
    // it in sync with the registered 100 HP (as map load does).
    *world.game_state.core_health.write() = 100.0;
    let player = |id: i32, team: u8, x: f32| PlayerCombatState {
        uuid: format!("pvp-core-{id}"),
        player_id: 1_000_000 + id,
        unit_id: 2_000_000 + id,
        x,
        y: core_y,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    world.players.insert(2_000_001, player(1, 1, core_x));
    world
        .players
        .insert(2_000_002, player(2, 5, core_x + 512.0));
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("pvp".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );

    // A third player team (3) with its own core: destroying team 1 leaves
    // two alive teams (3 and 5), so the game continues.
    world.cores.insert(
        3,
        TeamCore {
            position: (200 << 16) | 200,
            block: 339,
            health: 100.0,
            max_health: 100.0,
        },
    );
    world
        .players
        .insert(2_000_003, player(3, 3, core_x + 1024.0));
    assert!(damage_team_core(&world, &connections, 1, 100.0));
    assert_eq!(core_health_for_team(&world, 1), 0.0);
    assert!(
        !world.game_state.game_over.load(Ordering::Relaxed),
        "two player teams still alive: no game over yet"
    );
    // The dead team's core is removed from contention: destroying team 3
    // now leaves only team 5 alive -> game over, winner = blue (5).
    assert!(damage_team_core(&world, &connections, 3, 100.0));
    assert!(world.game_state.game_over.load(Ordering::Relaxed));
    assert!(world.persistence_dirty.load(Ordering::Relaxed));
    let mut game_over_frames = Vec::new();
    while let Ok(frame) = packet_rx.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            game_over_frames.push(packet);
        }
    }
    assert_eq!(
        game_over_frames.len(),
        1,
        "exactly one GameOverCallPacket for the PvP elimination"
    );
    assert_eq!(
        &game_over_frames[0][1..],
        &[5],
        "winner = last team standing"
    );
}

#[test]
fn pvp_projectiles_destroy_the_enemy_team_core() {
    // The PvP gameplay hook: a projectile fired by team A crossing the
    // enemy team's core footprint damages that core (BulletType
    // collidesTeam vs CoreBlock), eliminates the team and ends the game
    // when it is the last one standing.
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    *world.game_state.core_health.write() = 50.0;
    world.cores.insert(
        1,
        TeamCore {
            position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
            block: 339,
            health: 50.0,
            max_health: 1100.0,
        },
    );
    world.cores.insert(
        5,
        TeamCore {
            position: (120 << 16) | 60,
            block: 340,
            health: 500.0,
            max_health: 3000.0,
        },
    );
    let player = |id: i32, team: u8| PlayerCombatState {
        uuid: format!("pvp-core-bullet-{id}"),
        player_id: 1_000_000 + id,
        unit_id: 2_000_000 + id,
        x: core_x,
        y: core_y,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    world.players.insert(2_000_001, player(1, 1));
    world.players.insert(2_000_002, player(2, 5));
    // Blue fires through the sharded core tile: segment crosses it.
    world.projectiles.insert(
        4_100_001,
        Projectile {
            target_id: 2_000_001,
            shooter_id: -1,
            team: 5,
            bullet_id: 65,
            damage: 60.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: false,
            armor_multiplier: 1.0,
            remaining_ticks: 8.0,
            total_ticks: 8.0,
            source_x: core_x + 40.0,
            source_y: core_y,
            target_x: core_x - 40.0,
            target_y: core_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    assert!(simulate_pvp_player_damage(&world, &connections));
    assert!(
        !world.projectiles.contains_key(&4_100_001),
        "the projectile is consumed by the core hit"
    );
    assert_eq!(core_health_for_team(&world, 1), 0.0);
    assert!(world.game_state.game_over.load(Ordering::Relaxed));
    assert_eq!(
        core_health_for_team(&world, 5),
        500.0,
        "the blue core is untouched"
    );
}

#[test]
fn maps_without_cores_fall_back_to_the_single_core() {
    // (d) Custom maps without MSAV cores keep the legacy single core:
    // every team spawns there and the shared-core health governs.
    let (world, connections, core_x, core_y) = legacy_weapons_test_world();
    assert!(registered_core_teams(&world).is_empty());
    assert_eq!(core_position_for_team(&world, 5), world.core_position);
    assert_eq!(core_world_for_team(&world, 5), (core_x, core_y));
    assert_eq!(core_world_for_team(&world, 1), (core_x, core_y));
    assert_eq!(
        core_health_for_team(&world, 5),
        *world.game_state.core_health.read()
    );
    // Damage on an unregistered team still applies (shared core): in
    // Survival the sharded-core rules govern -> game over with winner 2.
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("fallback".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    // A partial hit applies damage without destroying the shared core.
    assert!(!damage_team_core(&world, &connections, 5, 10.0));
    assert_eq!(core_health_for_team(&world, 5), 6_000.0 - 10.0);
    assert!(!world.game_state.game_over.load(Ordering::Relaxed));
    assert!(damage_team_core(&world, &connections, 5, 999_999.0));
    assert!(world.game_state.game_over.load(Ordering::Relaxed));
    let mut game_over_frames = Vec::new();
    while let Ok(frame) = packet_rx.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            game_over_frames.push(packet);
        }
    }
    assert_eq!(&game_over_frames[0][1..], &[2]);
    // PvP shared-core fallback: a team without its own core is eliminated
    // together with the sharded core it spawns at (documented limitation).
    let (world2, _connections2, core_x2, core_y2) = legacy_weapons_test_world();
    *world2.game_state.mode.write() = GameMode::Pvp;
    world2.cores.insert(
        1,
        TeamCore {
            position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
            block: 339,
            health: 100.0,
            max_health: 100.0,
        },
    );
    *world2.game_state.core_health.write() = 100.0;
    world2.players.insert(
        2_000_010,
        PlayerCombatState {
            uuid: "shared-1".into(),
            player_id: 1_000_010,
            unit_id: 2_000_010,
            x: core_x2,
            y: core_y2,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    world2.players.insert(
        2_000_011,
        PlayerCombatState {
            uuid: "shared-5".into(),
            player_id: 1_000_011,
            unit_id: 2_000_011,
            x: core_x2 + 64.0,
            y: core_y2,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 5,
        },
    );
    let (packet_tx2, mut packet_rx2) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    _connections2.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: packet_tx2,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("shared".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    assert!(damage_team_core(&world2, &_connections2, 1, 100.0));
    assert!(world2.game_state.game_over.load(Ordering::Relaxed));
    let mut frames2 = Vec::new();
    while let Ok(frame) = packet_rx2.try_recv() {
        let packet = read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
        if packet[0] == GAME_OVER_PACKET_ID {
            frames2.push(packet);
        }
    }
    assert_eq!(frames2.len(), 1);
    // No surviving team -> wave-team fallback (defeat dialog for all).
    assert_eq!(&frames2[0][1..], &[2]);
}

#[test]
fn team_items_are_independent_between_teams() {
    // (a) PvP inventories: team 1 (legacy `core_items`) and team 5
    // (`team_items`) must not share any item slot.
    let (world, _, _, _) = legacy_weapons_test_world();
    world.game_state.core_items.write()[0] = 100; // team 1 copper
    world.game_state.team_items.insert(5, vec![0; 22]);
    world.game_state.team_items.get_mut(&5).unwrap()[0] = 200; // team 5 copper

    assert_eq!(items_for_team(&world, 1)[0], 100);
    assert_eq!(items_for_team(&world, 5)[0], 200);

    // Depositing into team 5 must not touch team 1 (and vice versa).
    if let Some(stored) = items_for_team_mut(&world, 5).get_mut(0) {
        *stored += 1;
    }
    assert_eq!(items_for_team(&world, 5)[0], 201);
    assert_eq!(items_for_team(&world, 1)[0], 100, "team 1 untouched");

    if let Some(stored) = items_for_team_mut(&world, 1).get_mut(0) {
        *stored += 1;
    }
    assert_eq!(items_for_team(&world, 1)[0], 101);
    assert_eq!(items_for_team(&world, 5)[0], 201, "team 5 untouched");

    // Unknown teams lazily default to the empty 22-slot official array.
    assert_eq!(items_for_team(&world, 3), vec![0; 22]);
    // Team 0 (neutral tiles) routes to the sharded default.
    if let Some(stored) = items_for_team_mut(&world, 0).get_mut(0) {
        *stored += 1;
    }
    assert_eq!(items_for_team(&world, 1)[0], 102);
}

#[test]
fn construction_consumes_the_placing_players_team() {
    // (b) PvP build cost: a team-5 player's construction must be paid
    // from team 5's inventory and the finished tile must belong to
    // team 5 — the legacy team-1 store stays untouched.
    let (world, connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    let mut session = player();
    session.id = 1_000_100;
    session.unit_id = 2_000_100;
    session.x = 100.0 * 8.0;
    session.y = 100.0 * 8.0;
    world.players.insert(
        session.unit_id,
        PlayerCombatState {
            uuid: session.uuid.clone(),
            player_id: session.id,
            unit_id: session.unit_id,
            x: session.x,
            y: session.y,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 5,
        },
    );
    world.game_state.core_items.write()[0] = 10; // team 1 copper
    world.game_state.team_items.insert(5, vec![0; 22]);
    world.game_state.team_items.get_mut(&5).unwrap()[0] = 10; // team 5 copper

    let position = (100 << 16) | 100;
    let pending = PendingBuild {
        position,
        block: 257, // conveyor: 1 copper
        rotation: 0,
        config: vec![0],
        occupied: vec![position],
        team: 5,
        builder: session.clone(),
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: 0.0,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(position, pending.clone());
    finish_pending_build(&world, &connections, pending).unwrap();

    assert_eq!(
        items_for_team(&world, 5)[0],
        9,
        "team 5 paid the conveyor cost"
    );
    assert_eq!(items_for_team(&world, 1)[0], 10, "team 1 untouched");
    let tile = world.tiles.get(&position).unwrap();
    assert_eq!(tile.block, 257);
    assert_eq!(tile.team, 5, "the finished tile belongs to team 5");
    drop(tile);

    // Direct requirement checks respect the team too.
    assert!(consume_requirements(&world.game_state, 5, 257));
    assert_eq!(items_for_team(&world, 5)[0], 8);
    assert_eq!(items_for_team(&world, 1)[0], 10);
}

#[test]
fn state_snapshot_core_data_emits_per_team_items_byte_exact() {
    // (c) StateSnapshot coreData (official `NetServer.writeStateSnapshot`):
    // `b teamCount`, then per team `b team` + ItemModule.write
    // (`s present; s itemId; i amount` in ascending item order).
    let (world, _, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    // Team 1 (legacy core_items): copper 100 + lead 25.
    world.game_state.core_items.write()[0] = 100;
    world.game_state.core_items.write()[1] = 25;
    // Team 5: copper 50 + titanium 7.
    world.game_state.team_items.insert(5, vec![0; 22]);
    {
        let mut team5 = world.game_state.team_items.get_mut(&5).unwrap();
        team5[0] = 50;
        team5[6] = 7;
    }
    let pcs = |id: i32, team: u8| PlayerCombatState {
        uuid: format!("core-data-{id}"),
        player_id: 1_000_000 + id,
        unit_id: 2_000_000 + id,
        x: 10.0,
        y: 20.0,
        health: 150.0,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
        statuses: Vec::new(),
        dead: false,
        respawn_timer: 0.0,
        team,
    };
    world.players.insert(2_000_050, pcs(50, 1));
    world.players.insert(2_000_051, pcs(51, 5));

    let payload = encode_state_snapshot_for(&world.game_state, Some(&world)).unwrap();
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    // 35-byte header: f waveTime, i wave, i enemies, b paused, b gameOver,
    // i timeData, b tps, l rand0, l rand1, then s coreDataLen.
    input.read_f().unwrap();
    input.read_i().unwrap();
    input.read_i().unwrap();
    input.read_bool().unwrap();
    input.read_bool().unwrap();
    input.read_i().unwrap();
    input.read_b().unwrap();
    input.read_l().unwrap();
    input.read_l().unwrap();
    let core_data_len = input.read_s().unwrap() as usize;
    let pos = input.position() as usize;
    let mut core = std::io::Cursor::new(&input.get_ref()[pos..pos + core_data_len]);

    assert_eq!(core.read_b().unwrap(), 2, "two active teams");
    // Team 1: b 1, present=2 (copper 100, lead 25).
    assert_eq!(core.read_b().unwrap(), 1);
    assert_eq!(core.read_s().unwrap(), 2);
    assert_eq!(core.read_s().unwrap(), 0);
    assert_eq!(core.read_i().unwrap(), 100);
    assert_eq!(core.read_s().unwrap(), 1);
    assert_eq!(core.read_i().unwrap(), 25);
    // Team 5: b 5, present=2 (copper 50, titanium 7).
    assert_eq!(core.read_b().unwrap(), 5);
    assert_eq!(core.read_s().unwrap(), 2);
    assert_eq!(core.read_s().unwrap(), 0);
    assert_eq!(core.read_i().unwrap(), 50);
    assert_eq!(core.read_s().unwrap(), 6);
    assert_eq!(core.read_i().unwrap(), 7);
    assert_eq!(
        core.position() as usize,
        core_data_len,
        "coreData fully consumed by the official layout"
    );
}

#[test]
fn team_items_survive_a_save_and_load_round_trip() {
    // (d) Persistence: revision 11 stores team 5's inventory next to the
    // legacy team-1 `core_items`; loading restores both and the world
    // apply path re-inserts them.
    let path = std::env::temp_dir().join("team-items-roundtrip.json");
    let _ = std::fs::remove_file(&path);
    let state = GameState::new();
    state.start_hosting("maze".into(), GameMode::Pvp);
    *state.core_items.write() = GameState::initial_core_items();
    state.core_items.write()[0] = 123;
    state.team_items.insert(5, vec![0; 22]);
    state.team_items.get_mut(&5).unwrap()[0] = 456;
    state.team_items.get_mut(&5).unwrap()[1] = 7;
    let tiles = DashMap::new();
    let enemies = DashMap::new();
    let base_buildings = DashMap::new();
    let player_profiles = DashMap::new();
    let building_commands = DashMap::new();
    let unit_orders = DashMap::new();
    let team_plans = crate::engine::typeio::TeamBlocks::default();
    let cores = DashMap::new();
    let logic_flags = DashMap::new();
    *state.simulation_time.write() = 12_345.0;
    logic_flags.insert("enemy_spotted".into(), 1.0);
    {
        let mut stats = state.game_stats.write();
        stats.enemy_units_destroyed = 7;
        stats.waves_lasted = 3;
        stats.buildings_built = 11;
        crate::state::game_state::GameStats::bump_block(&mut stats.placed_block_count, 98);
        crate::state::game_state::GameStats::bump_block(&mut stats.placed_block_count, 98);
        crate::state::game_state::GameStats::bump_block(&mut stats.placed_block_count, 99);
    }
    persist_tiles(
        &path,
        &tiles,
        &state,
        &enemies,
        &base_buildings,
        &player_profiles,
        &building_commands,
        &unit_orders,
        &team_plans,
        &cores,
        &logic_flags,
        &crate::network::buildings::puddles::PuddleSystem::new(),
    )
    .unwrap();

    // The saved file carries revision 14 (round 73) and the per-team block.
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(json["version"], 14);
    assert_eq!(json["simulation_time"], 12_345.0);
    assert_eq!(json["logic_flags"][0][0], "enemy_spotted");
    assert_eq!(json["logic_flags"][0][1], 1.0);
    assert_eq!(json["game_stats"]["enemy_units_destroyed"], 7);
    assert_eq!(json["game_stats"]["waves_lasted"], 3);
    assert_eq!(json["game_stats"]["buildings_built"], 11);
    assert_eq!(
        json["game_stats"]["placed_block_count"][0],
        serde_json::json!([98, 2])
    );
    assert_eq!(
        json["game_stats"]["placed_block_count"][1],
        serde_json::json!([99, 1])
    );
    assert_eq!(json["team_items"][0]["team"], 5);
    assert_eq!(json["team_items"][0]["items"][0], 456);
    assert_eq!(json["core_items"][0], 123);

    // Load: both inventories come back.
    let loaded = load_tiles(&path, None).unwrap();
    assert_eq!(loaded.simulation_time, Some(12_345.0));
    assert_eq!(loaded.logic_flags, vec![("enemy_spotted".to_string(), 1.0)]);
    assert_eq!(loaded.game_stats.enemy_units_destroyed, 7);
    assert_eq!(loaded.game_stats.waves_lasted, 3);
    assert_eq!(loaded.game_stats.placed_block_count, vec![(98, 2), (99, 1)]);
    assert_eq!(loaded.core_items.as_ref().unwrap()[0], 123);
    assert_eq!(loaded.team_items.len(), 1);
    assert_eq!(loaded.team_items[0].team, 5);
    assert_eq!(loaded.team_items[0].items[0], 456);
    assert_eq!(loaded.team_items[0].items[1], 7);

    // The world apply path restores the per-team map.
    let world = legacy_weapons_test_world().0;
    *world.game_state.core_items.write() = loaded.core_items.clone().unwrap();
    apply_loaded_team_items(&world, &loaded);
    assert_eq!(items_for_team(&world, 1)[0], 123);
    assert_eq!(items_for_team(&world, 5)[0], 456);
    assert_eq!(items_for_team(&world, 5)[1], 7);
    assert_eq!(
        items_for_team(&world, 2),
        vec![0; 22],
        "unpersisted teams default empty"
    );

    // v10 saves (no team_items field) still load: serde default kicks in.
    let legacy_json = json.as_object().unwrap().clone();
    let mut legacy = legacy_json.clone();
    legacy.remove("team_items");
    legacy.remove("simulation_time");
    legacy.remove("logic_flags");
    legacy.remove("game_stats");
    legacy.insert("version".into(), serde_json::json!(10));
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let loaded_legacy = load_tiles(&path, None).unwrap();
    assert_eq!(loaded_legacy.core_items.as_ref().unwrap()[0], 123);
    assert!(loaded_legacy.team_items.is_empty());
    std::fs::remove_file(&path).unwrap();
}
#[test]
fn sol002_team_cores_share_inventory_and_capacity() {
    let (world, _, _, _) = legacy_weapons_test_world();
    let core = |position, block| TeamCore {
        position,
        block,
        health: 1_000.0,
        max_health: 1_000.0,
    };
    crate::network::world::register_team_core(&world, 5, core(10 << 16 | 10, 339));
    crate::network::world::register_team_core(&world, 5, core(20 << 16 | 20, 341));
    assert_eq!(
        crate::network::world::team_core_snapshot(&world, 5).len(),
        2
    );
    world.game_state.team_items.insert(5, vec![0; 22]);
    world.game_state.team_items.get_mut(&5).unwrap()[0] = 7;
    // Both cores address TeamData.items, never independent inventories.
    assert_eq!(world.game_state.team_items.get(&5).unwrap()[0], 7);
    assert_eq!(crate::network::economy::team_unit_cap(&world, 5), 40);
    crate::network::world::unregister_team_core(&world, 5, 10 << 16 | 10);
    assert_eq!(
        crate::network::world::team_core_snapshot(&world, 5).len(),
        1
    );
    assert_eq!(crate::network::economy::team_unit_cap(&world, 5), 32);
    assert!(crate::network::world::registered_core_teams(&world).contains(&5));
}

#[test]
fn sol002_runtime_core_raises_cap_and_last_core_eliminates_team() {
    let (world, _, _, _) = legacy_weapons_test_world();
    let position = 30 << 16 | 30;
    crate::network::world::register_team_core(
        &world,
        7,
        TeamCore {
            position,
            block: 340,
            health: 100.0,
            max_health: 100.0,
        },
    );
    assert_eq!(crate::network::economy::team_unit_cap(&world, 7), 24);
    crate::network::world::register_team_core(
        &world,
        7,
        TeamCore {
            position: position + 1,
            block: 340,
            health: 100.0,
            max_health: 100.0,
        },
    );
    assert_eq!(crate::network::economy::team_unit_cap(&world, 7), 40);
    crate::network::world::unregister_team_core(&world, 7, position);
    assert!(crate::network::world::registered_core_teams(&world).contains(&7));
    crate::network::world::unregister_team_core(&world, 7, position + 1);
    assert!(!crate::network::world::registered_core_teams(&world).contains(&7));
}

#[test]
fn sol002_join_assignment_uses_only_active_core_teams() {
    let (world, _, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Pvp;
    crate::network::world::register_team_core(
        &world,
        5,
        TeamCore {
            position: 5 << 16 | 5,
            block: 339,
            health: 100.0,
            max_health: 100.0,
        },
    );
    crate::network::world::register_team_core(
        &world,
        7,
        TeamCore {
            position: 7 << 16 | 7,
            block: 339,
            health: 100.0,
            max_health: 100.0,
        },
    );
    // A coreless team id is never returned, even when supplied as the
    // caller's previous/default team.
    assert_ne!(assign_team_for_join(&world, "new", 9), 9);
    assert!(matches!(assign_team_for_join(&world, "new", 9), 5 | 7));
}
#[test]
fn set_mode_transition_is_transactional_and_preserves_map_rules() {
    // P0-6: a runtime mode switch re-derives the map's base rules from the
    // world template and applies the Gamemode preset on top. Switching to
    // sandbox must turn on infiniteResources/waves and off waveTimer WITHOUT
    // corrupting the map's other rules; switching back to survival restores
    // the map's original values (no inherited sandbox fields).
    let (world, _connections, _, _) = legacy_weapons_test_world();
    *world.game_state.mode.write() = GameMode::Survival;

    // Baseline: the dummy template's survival rules.
    let survival = mode_transition_rules(&world, GameMode::Survival).unwrap();
    assert!(
        !survival.infinite_resources,
        "survival is not resource-free"
    );
    assert!(
        !survival.waves_enabled || survival.wave_timer,
        "survival keeps the map wave contract"
    );
    assert!(survival.possession_allowed);
    assert_eq!(survival.wave_spacing, DEFAULT_WAVE_SPACING);

    // Sandbox preset (Gamemode.sandbox lambda$static$2 verified in the JAR).
    let sandbox = mode_transition_rules(&world, GameMode::Sandbox).unwrap();
    assert!(sandbox.infinite_resources, "sandbox sets infiniteResources");
    assert!(sandbox.waves_enabled, "sandbox sets waves=true");
    assert!(!sandbox.wave_timer, "sandbox sets waveTimer=false");
    assert!(
        sandbox.possession_allowed,
        "possession is untouched by sandbox"
    );
    assert_eq!(
        sandbox.spawn_groups.len(),
        survival.spawn_groups.len(),
        "sandbox must not rewrite the map's spawn groups"
    );

    // Switching back to survival restores the map's original values.
    let back = mode_transition_rules(&world, GameMode::Survival).unwrap();
    assert_eq!(back.infinite_resources, survival.infinite_resources);
    assert_eq!(back.wave_timer, survival.wave_timer);
    assert_eq!(back.waves_enabled, survival.waves_enabled);

    // PvP mode keeps the map rules but flips the team contract for join
    // handling; possession stays enabled.
    let pvp = mode_transition_rules(&world, GameMode::Pvp).unwrap();
    assert_eq!(pvp.infinite_resources, survival.infinite_resources);
    assert_eq!(pvp.default_team, survival.default_team);
    assert!(pvp.possession_allowed);
}

#[test]
fn set_mode_reconciliation_cancels_pending_work_without_item_side_effects() {
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let build_position = (44 << 16) | 104;
    let break_position = (45 << 16) | 104;
    let mut builder = player();
    builder.active_plans.insert((false, build_position, 257));
    builder.active_plans.insert((true, break_position, 257));
    world
        .player_sessions
        .insert(builder.unit_id, builder.clone());
    world.pending_builds.insert(
        build_position,
        PendingBuild {
            position: build_position,
            block: 257,
            rotation: 0,
            config: vec![0],
            occupied: vec![build_position],
            team: 1,
            builder: builder.clone(),
            last_seen: std::time::Instant::now(),
            assist_progress: 0.0,
            remaining_ticks: 120.0,
            applied_assist: 0.0,
        },
    );
    world.pending_breaks.insert(
        break_position,
        PendingBreak {
            position: break_position,
            block: 257,
            occupied: vec![break_position],
            dynamic: true,
            team: 1,
            builder,
            last_seen: std::time::Instant::now(),
            remaining_ticks: 120.0,
        },
    );
    world
        .team_build_plans
        .write()
        .teams
        .push(crate::engine::typeio::TeamPlans {
            team: 1,
            plans: vec![crate::engine::typeio::TeamBlockPlan {
                x: 44,
                y: 104,
                rotation: 0,
                block: 257,
                config: vec![0],
            }],
        });
    let before_items = world.game_state.core_items.read().clone();

    let _guard = world.persistence_lock.lock();
    let reconciled = crate::network::runtime::reconcile_mode_transition_actions(&world);

    assert_eq!(reconciled.builds, 1);
    assert_eq!(reconciled.breaks, 1);
    assert_eq!(reconciled.plans, 1);
    assert_eq!(reconciled.active_plans, 2);
    assert!(world.pending_builds.is_empty());
    assert!(world.pending_breaks.is_empty());
    assert!(world.team_build_plans.read().teams.is_empty());
    assert!(world
        .player_sessions
        .iter()
        .all(|session| session.active_plans.is_empty()));
    let restream_template = network_template_with_plans(&world).unwrap();
    assert!(
        crate::engine::world_stream::inspect_team_plans(&restream_template)
            .unwrap()
            .teams
            .is_empty(),
        "the post-cancellation world stream must not resurrect ghost plans"
    );
    assert_eq!(*world.game_state.core_items.read(), before_items);
}

#[test]
fn strict_mode_rejects_unknown_spawn_groups_and_warns_otherwise() {
    // P0-7: spawn groups with units the server cannot simulate are
    // reported, not dropped. Non-strict hosting warns; strict hosting
    // rejects the map with the diagnostic list.
    let rules =
        "{\"spawns\":[{\"type\":\"dagger\"},{\"type\":\"totallyFakeUnit\"},{\"type\":\"mace\"}]}";
    let (parsed, diagnostics) = crate::network::units::parse_wave_rules_report(rules);
    assert_eq!(parsed.spawn_groups.len(), 2, "supported groups survive");
    assert_eq!(diagnostics.len(), 1, "one unsupported group reported");
    assert!(
        diagnostics[0].contains("totallyFakeUnit"),
        "diagnostic names the unit: {}",
        diagnostics[0]
    );
    // Non-strict: warning only, hosting proceeds.
    assert!(enforce_strict_spawn_groups("test-map", &diagnostics, false).is_ok());
    // Strict: hosting fails with location and count.
    let err = enforce_strict_spawn_groups("test-map", &diagnostics, true).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("test-map"), "{message}");
    assert!(message.contains("totallyFakeUnit"), "{message}");
    assert!(message.contains("1 unsupported spawn group"), "{message}");
    // No diagnostics: strict hosting proceeds.
    assert!(enforce_strict_spawn_groups("test-map", &[], true).is_ok());
}

#[test]
fn strict_logic_compile_report_lists_unsupported_statements() {
    // P0-7: compile_report exposes every silently-degraded statement
    // with its line; plain compile keeps the historical behavior.
    let (program, diagnostics) = crate::logic::compile_report("set x 1\nfoo bar\nprint x\nend");
    assert!(program.is_some());
    assert!(
        diagnostics.iter().any(|d| d.contains("'foo'")),
        "unsupported statement named: {:?}",
        diagnostics
    );
    assert!(
        diagnostics.iter().any(|d| d.contains("line 2")),
        "diagnostic carries the source line: {:?}",
        diagnostics
    );
    // Supported statements produce no diagnostics.
    let (_, clean) = crate::logic::compile_report("set x 1\nprint x\nend");
    assert!(clean.is_empty(), "supported program is clean: {:?}", clean);
    // The old API still works and reports nothing extra.
    assert!(crate::logic::compile("set x 1\nend").is_some());
}

#[test]
fn strict_mode_rejects_unsupported_logic_processors() {
    // P0-7: with strict mode on, a processor whose program contains an
    // unsupported statement is marked rejected and never executes.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let pos = (30 << 16) | 30;
    // logic block 432 with a config carrying an unsupported statement.
    let mut tile = erekir_like_tile(pos, 432);
    // Build a TypeIO byte[] config: tag 14 envelope around a zlib
    // stream whose content is the LogicBlock.compress layout:
    // [1][source_len i32][source][link_count i32][links].
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;
    let source = b"foo bar";
    let mut content = vec![1];
    content.extend_from_slice(&(source.len() as i32).to_be_bytes());
    content.extend_from_slice(source);
    content.extend_from_slice(&0i32.to_be_bytes()); // no links
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&content).unwrap();
    let zlib = encoder.finish().unwrap();
    let mut config = vec![14];
    config.extend_from_slice(&(zlib.len() as i32).to_be_bytes());
    config.extend_from_slice(&zlib);
    tile.config = config;
    world.tiles.insert(pos, tile);
    // Non-strict: the program degrades to NoOp silently (historical).
    world.game_state.strict_mode.store(false, Ordering::Relaxed);
    assert!(simulate_logic(&world, &DashMap::new(), 6.0));
    if let Some(entry) = world.logic_executors.get(&pos) {
        assert!(!entry.rejected, "non-strict mode executes degraded program");
    }
    // Strict: the same processor is rejected with a located error and
    // the tick reports no state change (the rejected program never ran).
    world.logic_executors.remove(&pos);
    world.game_state.strict_mode.store(true, Ordering::Relaxed);
    assert!(!simulate_logic(&world, &DashMap::new(), 6.0));
    let entry = world.logic_executors.get(&pos).unwrap();
    assert!(entry.rejected, "strict mode rejects the degraded program");
}

#[test]
fn bounded_outbound_drops_and_marks_slow_consumers() {
    // P0-9: the outbound queue is bounded; frames beyond capacity are
    // dropped and counted, critical frames bump the critical counter,
    // and the world-loop teardown gate recognizes the slow consumer.
    let (tx, mut rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let connection = PendingConnection {
        ip: "127.0.0.1".parse().unwrap(),
        outbound: tx,
        udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
        udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
        udp_socket: None,
        player_name: Arc::new(parking_lot::RwLock::new(None)),
        outbound_drops: Arc::new(AtomicU64::new(0)),
        critical_drops: Arc::new(AtomicU64::new(0)),
        last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
        last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
        outbound_queued: Arc::new(AtomicU64::new(0)),
    };
    // Drain the queue until full: OUTBOUND_QUEUE_CAPACITY frames fit.
    let mut accepted = 0;
    while enqueue_outbound(&connection, vec![0xAA; 4], false) {
        accepted += 1;
    }
    assert_eq!(accepted, OUTBOUND_QUEUE_CAPACITY as i32);
    assert_eq!(
        connection.outbound_drops.load(Ordering::Relaxed),
        1,
        "the first overflow frame is dropped and counted"
    );
    assert_eq!(connection.critical_drops.load(Ordering::Relaxed), 0);
    // Critical frames also drop when full, but bump the critical counter
    // (which triggers teardown after 16 critical drops).
    for _ in 0..16 {
        enqueue_outbound(&connection, vec![0xBB; 4], true);
    }
    assert_eq!(connection.critical_drops.load(Ordering::Relaxed), 16);
    // The receiver still sees the accepted frames.
    assert_eq!(rx.try_recv().unwrap(), vec![0xAA; 4]);
    // After draining the queue, sends succeed again.
    assert!(enqueue_outbound(&connection, vec![0xCC; 4], false));
    // Dropped frames past the slow-consumer limit trigger teardown: the
    // sender drop limit check uses the counters, so set them directly.
    connection.outbound_drops.store(4096, Ordering::Relaxed);
    let connections = DashMap::new();
    connections.insert(7, connection.clone());
    // Simulate the world-loop gate: the connection is removed.
    let slow: Vec<i32> = connections
        .iter()
        .filter(|entry| {
            entry.value().outbound_drops.load(Ordering::Relaxed) >= SLOW_CONSUMER_DROP_LIMIT
                || entry.value().critical_drops.load(Ordering::Relaxed) >= 16
        })
        .map(|entry| *entry.key())
        .collect();
    assert_eq!(slow, vec![7]);
    for id in slow {
        connections.remove(&id);
    }
    assert!(connections.is_empty());
}

#[test]
fn p03_admin_request_decoders_match_desktop_158_layouts() {
    // AdminRequestCallPacket.handled: i entity + b action + objectSafe.
    use crate::network::codec::Writes;
    let mut bytes = Vec::new();
    bytes.write_i(1_000_042).unwrap();
    bytes.write_b(4).unwrap(); // switchTeam
    bytes.write_b(20).unwrap(); // objectSafe Team tag
    bytes.write_b(5).unwrap(); // team id
    assert_eq!(
        decode_admin_request(&bytes).unwrap(),
        (
            1_000_042,
            4,
            crate::network::decoders::AdminRequestParams::Team(5),
        )
    );
    // Action ordinal out of range is rejected.
    let mut bad = Vec::new();
    bad.write_i(1).unwrap();
    bad.write_b(7).unwrap();
    assert!(decode_admin_request(&bad).is_err());
    // ClientLogicData: string channel + objectSafe value.
    let mut logic = Vec::new();
    logic.write_typeio_string(Some("mod-channel")).unwrap();
    logic.write_b(1).unwrap();
    logic.write_i(42).unwrap();
    let (channel, value) = decode_client_logic_data(&logic).unwrap();
    assert_eq!(channel, "mod-channel");
    assert_eq!(value, vec![1, 0, 0, 0, 42]);
    // RequestBuildPayload: i position.
    let mut build = Vec::new();
    build.write_i((10 << 16) | 20).unwrap();
    assert_eq!(
        decode_request_build_payload(&build).unwrap(),
        (10 << 16) | 20
    );
    // RequestDropPayload: f x + f y; non-finite rejected.
    let mut drop = Vec::new();
    drop.write_f(12.5).unwrap();
    drop.write_f(-3.25).unwrap();
    assert_eq!(decode_request_drop_payload(&drop).unwrap(), (12.5, -3.25));
    let mut nan = Vec::new();
    nan.write_f(f32::NAN).unwrap();
    nan.write_f(0.0).unwrap();
    assert!(decode_request_drop_payload(&nan).is_err());
    // RequestUnitPayload: b type + i id.
    let mut unit = Vec::new();
    unit.write_b(2).unwrap();
    unit.write_i(3_000_042).unwrap();
    assert_eq!(decode_request_unit_payload(&unit).unwrap(), (2, 3_000_042));
    // SetPlayerTeamEditor: b team.
    assert_eq!(decode_set_player_team_editor(&[7]).unwrap(), 7);
    // ServerPacket: two strings; ServerBinary: string + bytes.
    let mut sp = Vec::new();
    sp.write_typeio_string(Some("type")).unwrap();
    sp.write_typeio_string(Some("payload")).unwrap();
    let (t, c) = decode_server_packet(&sp, false).unwrap();
    assert_eq!(t, "type");
    assert_eq!(c, b"payload");
    let mut sb = Vec::new();
    sb.write_typeio_string(Some("binary")).unwrap();
    sb.write_us(3).unwrap();
    sb.extend_from_slice(&[1, 2, 3]);
    let (t, c) = decode_server_packet(&sb, true).unwrap();
    assert_eq!((t.as_str(), c.as_slice()), ("binary", &[1, 2, 3][..]));
    // Trace info frame carries the entity id header and trace fields.
    let frame = encode_trace_info_frame(
        &player(),
        &PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY).0,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("target".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
        &test_admin(),
    )
    .unwrap();
    // PacketSerializer may LZ4-compress payloads >= 36 bytes (158.1).
    let packet = crate::network::codec::read_tcp_packet(std::io::Cursor::new(&frame)).unwrap();
    assert_eq!(packet[0], TRACE_INFO_PACKET_ID);
    // player() helper has id == 1.
    assert_eq!(&packet[1..5], &1i32.to_be_bytes(), "entity id header");
    // ip string follows the entity id (writeTypeIO string: b tag + u16 len).
    assert_eq!(packet[5], 1, "non-null string tag");
    assert_eq!(&packet[6..8], &9u16.to_be_bytes(), "127.0.0.1 length");
}

#[test]
fn persistence_worker_saves_snapshots_durably_and_orders_writes() {
    // P0-8: snapshot + worker keep I/O off the tick; the worker writes
    // the last submitted snapshot last, and the durable path fsyncs the
    // temp file before the atomic rename.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let path = std::env::temp_dir().join(format!(
        "mindustry-persist-worker-test-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let saved = snapshot_persisted_world(
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
    );
    assert_eq!(saved.map_name, "legacy-weapons");
    // Durable sync write works and produces a loadable file.
    let bytes_written = persist_world_sync(&path, &saved).unwrap();
    assert!(bytes_written > 0);
    let loaded = std::fs::read_to_string(&path).unwrap();
    assert!(loaded.contains("\"map_name\": \"legacy-weapons\""));
    // The worker thread persists submitted jobs (last wins).
    let worker = PersistenceWorker::spawn();
    let mut second = saved.clone();
    second.map_name = "legacy-weapons-2".into();
    assert!(worker.submit(PersistJob {
        path: path.clone(),
        world: saved
    }));
    assert!(worker.submit(PersistJob {
        path: path.clone(),
        world: second
    }));
    // Wait for the worker to drain (bounded retry loop).
    let mut ok = false;
    for _ in 0..100 {
        if std::fs::read_to_string(&path)
            .map(|text| text.contains("legacy-weapons-2"))
            .unwrap_or(false)
        {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(ok, "worker persisted the last snapshot");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn p010_factory_command_is_typed_metadata_not_config_suffix() {
    // P0-10: unit factories keep their command in the typed
    // factory_command field; `config` stays a pure TypeIO object and
    // serializers never emit the legacy [254, command] marker.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let pos = (45 << 16) | 100;
    let occupied = block_footprint_in(300, 300, pos, 377).unwrap();
    world.tiles.insert(
        pos,
        DynamicTile {
            position: pos,
            block: 377,
            team: 1,
            rotation: 0,
            config: vec![5, 6, 0, 0],
            enabled: true,
            message: None,
            occupied,
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
            health: 100.0,
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
        },
    );
    let mut player = player();
    player.x = 360.0;
    player.y = 800.0;
    world.players.insert(
        2,
        PlayerCombatState {
            uuid: "p010".into(),
            player_id: 1,
            unit_id: 2,
            x: 360.0,
            y: 800.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    // Setting the command stores typed metadata and keeps config clean.
    assert!(apply_tile_config(&player, &world, pos, &[23, 0, 4]));
    // NOTE: clone the tile (drop the DashMap Ref) BEFORE mutating again
    // (project rule: never hold a Ref/RefMut while mutating the map).
    let tile = world.tiles.get(&pos).unwrap().clone();
    assert_eq!(tile.factory_command, Some(4));
    assert!(
        !tile.config.contains(&FACTORY_COMMAND_MARKER),
        "config never carries the private marker"
    );
    assert_eq!(configured_unit_command(&tile), Some(4));
    // Clearing the command resets the field, not the config bytes.
    assert!(apply_tile_config(&player, &world, pos, &[0]));
    let tile = world.tiles.get(&pos).unwrap().clone();
    assert_eq!(tile.factory_command, None);
    assert_eq!(configured_unit_command(&tile), None);
    // unit_factory_plan still resolves the clean TypeIO config (the
    // selected Dagger plan), independent of the cleared command.
    assert_eq!(unit_factory_plan(377, &tile.config), Some((0, 0)));
    // Legacy in-memory tiles (marker suffix) are still readable.
    let mut legacy = tile.clone();
    legacy.config = vec![5, 6, 0, 0, FACTORY_COMMAND_MARKER, 2];
    assert_eq!(configured_unit_command(&legacy), Some(2));
    assert_eq!(unit_factory_plan(377, &legacy.config), Some((0, 0)));
    // Snapshot round-trip: the checkpoint writes the typed field.
    let mut sync = Vec::new();
    encode_dynamic_tile_sync(&mut sync, &tile, &HashMap::new(), None).unwrap();
    assert!(sync.len() >= 2);
    // Save/load migration splits a legacy marker into the typed field.
    let path = std::env::temp_dir().join(format!(
        "mindustry-p010-migration-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut legacy_tile = tile.clone();
    legacy_tile.factory_command = None;
    legacy_tile.config = vec![5, 6, 0, 0, FACTORY_COMMAND_MARKER, 3];
    let mut saved = snapshot_persisted_world(
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
    );
    saved.tiles.clear();
    saved.tiles.push(legacy_tile.clone());
    persist_world_sync(&path, &saved).unwrap();
    let loaded = load_tiles(&path, Some((300, 300))).unwrap();
    let migrated = loaded.tiles.get(&pos).unwrap();
    assert_eq!(
        migrated.factory_command,
        Some(3),
        "migration recovers the command"
    );
    assert!(
        !migrated.config.contains(&FACTORY_COMMAND_MARKER),
        "migration strips the marker from config"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn p05_banned_blocks_units_and_core_radius_gate_authority() {
    // P0-5: Rules.bannedBlocks/bannedUnits (with whitelist inversion)
    // and Rules.enemyCoreBuildRadius now gate placement, wave spawns
    // and AI builders.
    let rules = parse_wave_rules(
        "{\"bannedBlocks\":[\"copper-wall\",\"router\"],\"bannedUnits\":[\"dagger\"],\"enemyCoreBuildRadius\":600.0}",
    );
    assert!(rules.block_banned(216), "copper-wall is banned");
    assert!(rules.block_banned(266), "router is banned");
    assert!(!rules.block_banned(270), "unlisted block is allowed");
    assert!(rules.unit_banned(0), "dagger is banned");
    assert!(!rules.unit_banned(1), "mace is allowed");
    assert_eq!(rules.enemy_core_build_radius, 600.0);
    // Whitelist mode inverts the membership test (Rules.java:331/335).
    let whitelist = parse_wave_rules(
        "{\"bannedBlocks\":[\"copper-wall\"],\"blockWhitelist\":true,\"bannedUnits\":[\"dagger\"],\"unitWhitelist\":true}",
    );
    assert!(
        !whitelist.block_banned(216),
        "whitelist: listed block is allowed"
    );
    assert!(
        whitelist.block_banned(270),
        "whitelist: unlisted block is banned"
    );
    assert!(!whitelist.unit_banned(0));
    assert!(whitelist.unit_banned(1));
    // Spawn groups with banned units are skipped (WaveSpawner).
    let spawn_rules = parse_wave_rules(
        "{\"spawns\":[{\"type\":\"dagger\"},{\"type\":\"mace\"}],\"bannedUnits\":[\"dagger\"]}",
    );
    let spawned = map_wave_spawns(0, &spawn_rules);
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].spec.unit_type, 1, "only mace spawns");
    // Placement plans for banned blocks are rejected.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    world.wave_rules.write().banned_blocks = vec![216];
    let mut player = player();
    player.x = 360.0;
    player.y = 800.0;
    let plan = BuildPlan {
        breaking: false,
        position: (45 << 16) | 100,
        block: 216,
        rotation: 0,
        config: Vec::new(),
    };
    let world_arc = Arc::new(world);
    let admin = test_admin();
    apply_build_plans(
        &mut player,
        &[plan],
        &world_arc,
        &Arc::new(DashMap::<i32, PendingConnection>::new()),
        &admin,
        true,
    )
    .unwrap();
    assert!(
        world_arc.pending_builds.is_empty(),
        "banned block never becomes a pending build"
    );
}

#[test]
fn p05_team_rules_parse_and_gate_per_team_authority() {
    // P0-5: Rules.teams TeamRules parse per team and gate build speed,
    // unit health, mine speed and the enemy-core build radius.
    let rules = parse_wave_rules(
        "{\"teams\":{\"1\":{\"buildSpeedMultiplier\":2.0,\"unitMineSpeedMultiplier\":0.5,\"unitHealthMultiplier\":3.0,\"extraCoreBuildRadius\":100.0},\"2\":{\"protectCores\":false,\"unitDamageMultiplier\":1.5}}}",
    );
    assert_eq!(rules.team_rules.len(), 2);
    let team1 = rules.team_rule(1);
    assert_eq!(team1.build_speed_multiplier, 2.0);
    assert_eq!(team1.unit_mine_speed_multiplier, 0.5);
    assert_eq!(team1.unit_health_multiplier, 3.0);
    assert_eq!(team1.extra_core_build_radius, 100.0);
    assert!(team1.protect_cores);
    let team2 = rules.team_rule(2);
    assert!(!team2.protect_cores);
    assert_eq!(team2.unit_damage_multiplier, 1.5);
    // Unknown teams fall back to the official defaults.
    assert_eq!(rules.team_rule(9).build_speed_multiplier, 1.0);
    assert!(rules.team_rule(9).protect_cores);
    let defaults = crate::network::units::TeamRule::default();
    assert!(!defaults.prebuild_ai);
    assert!(!defaults.build_ai);
    assert_eq!(defaults.build_ai_tier, 1.0);
    assert!(!defaults.rts_ai);
    assert_eq!(defaults.rts_min_squad, 4);
    assert_eq!(defaults.rts_max_squad, 50);
    assert!((defaults.rts_min_weight - 1.2).abs() < f32::EPSILON);
    // buildSpeed(team) composes global * team multipliers.
    assert_eq!(rules.build_speed_for(1), 2.0);
    assert_eq!(rules.build_speed_for(2), 1.0);
    // enemyCoreBuildRadius(team): protected teams add extra radius,
    // unprotected teams get 0.
    assert_eq!(rules.enemy_core_radius_for(1), 500.0);
    assert_eq!(rules.enemy_core_radius_for(2), 0.0);
    let ai = parse_wave_rules(
        "{\"teams\":{\"2\":{\"rtsAi\":true,\"rtsMinSquad\":8,\"rtsMaxSquad\":12,\"rtsMinWeight\":2.5,\"buildAi\":true,\"buildAiTier\":0.4,\"prebuildAi\":true}}}",
    );
    assert!(!ai.team_rule(1).rts_ai);
    assert!(ai.team_rule(2).rts_ai);
    assert_eq!(ai.team_rule(2).rts_min_squad, 8);
    assert_eq!(ai.team_rule(2).rts_max_squad, 12);
    assert!((ai.team_rule(2).rts_min_weight - 2.5).abs() < f32::EPSILON);
    assert!(ai.team_rule(2).build_ai);
    assert!((ai.team_rule(2).build_ai_tier - 0.4).abs() < f32::EPSILON);
    assert!(ai.team_rule(2).prebuild_ai);
    // Schedule a build on team 1: remaining ticks halve (speed 2x).
    let (world, _connections, _, _) = legacy_weapons_test_world();
    world.wave_rules.write().team_rules = rules.team_rules.clone();
    let mut builder = player();
    builder.uuid = "team1-builder".into();
    let pos = (45 << 16) | 100;
    let pending = PendingBuild {
        position: pos,
        block: 216,
        rotation: 0,
        config: Vec::new(),
        occupied: vec![pos],
        team: 1,
        builder,
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: 0.0,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(pos, pending.clone());
    let base_time = crate::game::content::block_build_time(216) / 0.5;
    schedule_build(&world, &pending);
    let remaining = world.pending_builds.get(&pos).unwrap().remaining_ticks;
    // team 1 speed multiplier 2.0 -> half the base time.
    assert!(
        (remaining - base_time / 2.0).abs() < 1.0,
        "team build speed halves remaining ticks: {remaining} vs {}",
        base_time / 2.0
    );
}

#[test]
fn p1e1_team_rule_ai_fields_parse_defaults_global_delay_and_per_team() {
    // P1-E1: AI-facing TeamRule fields — defaults, per-team independence,
    // global+team factory delay composition, parser round-trip.
    let defaults = crate::network::units::TeamRule::default();
    assert!(!defaults.prebuild_ai);
    assert!(!defaults.build_ai);
    assert_eq!(defaults.build_ai_tier, 1.0);
    assert!(!defaults.rts_ai);
    assert_eq!(defaults.rts_min_squad, 4);
    assert_eq!(defaults.rts_max_squad, 50);
    assert!((defaults.rts_min_weight - 1.2).abs() < f32::EPSILON);
    assert!((defaults.unit_factory_activation_delay - 0.0).abs() < f32::EPSILON);

    let rules = parse_wave_rules(
        "{\"unitFactoryActivationDelay\":60.0,\"teams\":{\"1\":{\"unitFactoryActivationDelay\":30.0,\"rtsAi\":true,\"rtsMinSquad\":6,\"rtsMaxSquad\":20,\"rtsMinWeight\":1.5,\"buildAi\":true,\"buildAiTier\":0.7,\"prebuildAi\":true},\"2\":{\"rtsAi\":false}}}",
    );
    assert!((rules.unit_factory_activation_delay - 60.0).abs() < f32::EPSILON);
    assert_eq!(rules.unit_activation_delay_for(1), 90.0);
    assert_eq!(rules.unit_activation_delay_for(2), 60.0);
    assert_eq!(rules.unit_activation_delay_for(9), 60.0);
    let team1 = rules.team_rule(1);
    assert!(team1.rts_ai);
    assert_eq!(team1.rts_min_squad, 6);
    assert_eq!(team1.rts_max_squad, 20);
    assert!((team1.rts_min_weight - 1.5).abs() < f32::EPSILON);
    assert!(team1.build_ai);
    assert!((team1.build_ai_tier - 0.7).abs() < f32::EPSILON);
    assert!(team1.prebuild_ai);
    assert!(!rules.team_rule(2).rts_ai);
    // Unknown teams keep AI defaults.
    assert!(!rules.team_rule(9).rts_ai);
    assert_eq!(rules.team_rule(9).rts_min_squad, 4);
}

#[test]
fn p05_team_infinite_resources_completes_builds_and_breaks_immediately() {
    // A7: ConstructBlock.construct/deconstruct check
    // `team.rules().infiniteResources || state.rules.infiniteResources`
    // (JAR bytecode offsets 87-117 / deconstruct checkRequired 0-33).
    // A map with teams:{1:{infiniteResources:true}} must build instantly
    // even with the global flag off.
    let (world, connections, _, _) = legacy_weapons_test_world();
    world.wave_rules.write().team_rules = std::collections::HashMap::from([(
        1u8,
        crate::network::units::TeamRule {
            infinite_resources: true,
            ..Default::default()
        },
    )]);
    assert!(
        !world.game_state.infinite_resources.load(Ordering::Relaxed),
        "global infiniteResources stays off"
    );
    let position = (44 << 16) | 104;
    let pending = PendingBuild {
        position,
        block: 257,
        rotation: 0,
        config: vec![0],
        occupied: vec![position],
        team: 1,
        builder: player(),
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: f32::MAX,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(position, pending.clone());
    schedule_build(&world, &pending);
    assert_eq!(
        world.pending_builds.get(&position).unwrap().remaining_ticks,
        0.0,
        "team infiniteResources finishes the build immediately"
    );
    simulate_constructions(&world, &connections, 0.0);
    assert_eq!(world.tiles.get(&position).unwrap().block, 257);
    // Break path: the tile's team rule also makes deconstruction
    // immediate.
    let pending = PendingBreak {
        position,
        block: 257,
        occupied: vec![position],
        dynamic: true,
        team: 1,
        builder: player(),
        last_seen: std::time::Instant::now(),
        remaining_ticks: f32::MAX,
    };
    world.pending_breaks.insert(position, pending.clone());
    schedule_break(&world, &pending);
    assert_eq!(
        world.pending_breaks.get(&position).unwrap().remaining_ticks,
        0.0,
        "team infiniteResources finishes the break immediately"
    );
    *world.game_state.core_items.write() = vec![0; 22];
    simulate_breaks(&world, &connections, 0.0);
    assert!(world.tiles.get(&position).is_none());
    assert_eq!(
        world.game_state.core_items.read()[0],
        0,
        "free TeamRule deconstruction must not mint a refund"
    );
    // consume_requirements_for honors the team rule too (checkRequired).
    let state = GameState::new();
    *state.core_items.write() = vec![0; 22];
    assert!(consume_requirements_for(
        &state,
        &world.wave_rules.read(),
        1,
        257
    ));
    assert_eq!(state.core_items.read()[0], 0, "no items consumed");
}

#[test]
fn p05_team_block_multipliers_scale_damage_and_health_per_team() {
    // A7: Building.damage divides by Rules.blockHealth(team) and
    // BulletType.damageMultiplier uses Rules.blockDamage(team) — both
    // compose the global multiplier with the BUILDING's TeamRule.
    let (world, _, _, _) = legacy_weapons_test_world();
    let pos = (40 << 16) | 100;
    let occupied = block_footprint_in(300, 300, pos, 341).unwrap();
    world.tiles.insert(
        pos,
        DynamicTile {
            position: pos,
            block: 341,
            team: 1,
            health: 1000.0,
            occupied,
            ..Default::default()
        },
    );
    world.wave_rules.write().team_rules = std::collections::HashMap::from([(
        1u8,
        crate::network::units::TeamRule {
            block_health_multiplier: 2.0,
            block_damage_multiplier: 1.0,
            ..Default::default()
        },
    )]);
    // team health multiplier 2.0: 100 damage -> 100/2 = 50 -> 950.
    let (destroyed, health) = damage_building(&world, pos, 100.0).unwrap();
    assert!(!destroyed);
    assert!(
        (health - 950.0).abs() < 0.01,
        "team blockHealth 2x halves damage"
    );
    // A team-2 building uses team 2's rule (defaults: 1.0): full damage.
    world.tiles.get_mut(&pos).unwrap().health = 1000.0;
    world.tiles.get_mut(&pos).unwrap().team = 2;
    let (destroyed, health) = damage_building(&world, pos, 100.0).unwrap();
    assert!(!destroyed);
    assert!(
        (health - 900.0).abs() < 0.01,
        "team 2 defaults: full damage"
    );
}

#[test]
fn m5_session_gates_run_allow_action_for_the_five_rpcs() {
    use crate::state::administration::{ActionType, PlayerAction};
    // M5: DeletePlans/PingLocation/BuildingControlSelect/
    // RequestBuildPayload/RequestDropPayload now pass through
    // allowAction; a veto filter denies them and an over-limit actor
    // gets the official anti-spam kick outcome.
    let admin = test_admin();
    let veto = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let veto_clone = veto.clone();
    admin.add_action_filter(std::sync::Arc::new(move |action: &PlayerAction| {
        if action.action_type == ActionType::PingLocation {
            veto_clone.store(true, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        true
    }));
    let connections: Arc<DashMap<i32, PendingConnection>> = Arc::new(DashMap::new());
    let mut actor = player();
    actor.uuid = "m5-actor".into();
    // Vetoed PingLocation is denied without a kick.
    match session_action_allowed(
        &admin,
        &actor,
        &connections,
        ActionType::PingLocation,
        None,
        None,
        None,
    ) {
        Ok(false) => {}
        other => panic!("vetoed PingLocation must be denied, got {other:?}"),
    }
    assert!(veto.load(std::sync::atomic::Ordering::Relaxed));
    // The removePlanned variant populates the plan list for filters.
    let mut planned = crate::state::administration::PlayerAction::new(
        "m5-actor".into(),
        false,
        ActionType::RemovePlanned,
    );
    planned.plans = vec![(10 << 16) | 20];
    let saw_plans = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw_plans_clone = saw_plans.clone();
    let admin2 = test_admin();
    admin2.add_action_filter(std::sync::Arc::new(move |action: &PlayerAction| {
        if action.action_type == ActionType::RemovePlanned && action.plans == vec![(10 << 16) | 20]
        {
            saw_plans_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        true
    }));
    assert!(session_action_allowed_full(
        &admin2,
        &actor,
        &connections,
        ActionType::RemovePlanned,
        &[(10 << 16) | 20],
        &[],
        &[],
    )
    .unwrap());
    assert!(saw_plans.load(std::sync::atomic::Ordering::Relaxed));
    // Anti-spam: 61 rate-limited actions produce the kick outcome with
    // the official message frame.
    let admin3 = test_admin();
    let mut outcome = Ok(true);
    for _ in 0..61 {
        outcome = session_action_allowed(
            &admin3,
            &actor,
            &connections,
            ActionType::PingLocation,
            None,
            None,
            None,
        );
    }
    match outcome {
        Err(frame) => {
            assert_eq!(frame[2], KICK_PACKET_ID, "kick message frame id 58");
            // The actor uuid cooldown is registered even without a
            // connection entry (ip empty).
            assert!(admin3.kick_time(&actor.uuid, "").is_some());
        }
        other => panic!("61st action must kick, got {other:?}"),
    }
}

#[test]
fn p1_udp_datagram_matches_arcnet_serializer_without_tcp_prefix() {
    // P1: the unreliable transport uses the ArcNet PacketSerializer
    // layout minus the TCP length prefix: [b id][s payload_len]
    // [b compress][payload] (UdpConnection.send + PacketSerializer.write
    // of desktop 158.1). The TCP frame adds the u16 length in front.
    let payload = vec![0xAA, 0xBB, 0xCC];
    let mut datagram = Vec::new();
    crate::network::codec::write_packet(&mut datagram, 126, &payload, true).unwrap();
    // [id=126][s original_len][b compress=1][lz4 payload]
    assert_eq!(datagram[0], 126);
    let payload_len = u16::from_be_bytes([datagram[1], datagram[2]]) as usize;
    assert_eq!(payload_len, 3, "original payload length");
    assert_eq!(datagram[3], 1, "compressed flag");
    assert!(
        datagram.len() >= 4 + payload_len,
        "compressed payload follows"
    );
    // The TCP frame is exactly the length prefix + datagram.
    let mut frame = Vec::new();
    write_tcp_packet(&mut frame, 126, &payload, true).unwrap();
    assert_eq!(&frame[2..], &datagram[..], "TCP frame = u16 len + datagram");
    // Uncompressed variant keeps the same shape.
    let mut plain = Vec::new();
    crate::network::codec::write_packet(&mut plain, 74, &payload, false).unwrap();
    assert_eq!(plain[0], 74);
    assert_eq!(u16::from_be_bytes([plain[1], plain[2]]) as usize, 3);
    assert_eq!(plain[3], 0, "uncompressed flag");
    assert_eq!(&plain[4..], &payload[..]);
    assert_eq!(plain.len(), 4 + payload.len());
}

fn handshake_pending(
    ip: &str,
    outbound: tokio::sync::mpsc::Sender<Vec<u8>>,
    udp_inbound: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) -> PendingConnection {
    PendingConnection {
        ip: ip.parse().unwrap(),
        outbound,
        udp_inbound,
        udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
        udp_socket: None,
        player_name: Arc::new(parking_lot::RwLock::new(None)),
        outbound_drops: Arc::new(AtomicU64::new(0)),
        critical_drops: Arc::new(AtomicU64::new(0)),
        last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
        last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
        outbound_queued: Arc::new(AtomicU64::new(0)),
    }
}

#[test]
fn p011_register_tcp_connection_id_is_nonzero_and_unique() {
    let connections = DashMap::new();
    let mut ids = Vec::new();
    for _ in 0..16 {
        let id = allocate_connection_id(&connections);
        assert_ne!(id, 0, "RegisterTCP connectionID must not be 0");
        let frame = framework_registration(REGISTER_TCP, id);
        assert_eq!(&frame[..2], &(FRAMEWORK_PACKET_LEN as u16).to_be_bytes());
        assert_eq!(frame[2] as i8, FRAMEWORK_MESSAGE_ID);
        assert_eq!(frame[3], REGISTER_TCP);
        assert_eq!(&frame[4..8], &id.to_be_bytes());
        let (tx, _) = tokio::sync::mpsc::channel(1);
        connections.insert(
            id,
            handshake_pending("127.0.0.1", tx, tokio::sync::mpsc::unbounded_channel().0),
        );
        ids.push(id);
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 16);
}

#[test]
fn p011_register_udp_promotes_session_and_rejects_adversarial_ids() {
    let connections = DashMap::new();
    let (tcp_tx, mut tcp_rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let (udp_tx, mut udp_rx) = tokio::sync::mpsc::unbounded_channel();
    connections.insert(42, handshake_pending("127.0.0.1", tcp_tx, udp_tx));
    let source: SocketAddr = "127.0.0.1:40000".parse().unwrap();
    assert!(
        !udp_handshake_complete(&connections, 42),
        "session must stay pending until RegisterUDP"
    );
    assert_eq!(
        apply_register_udp(&connections, 99, source),
        RegisterUdpOutcome::UnknownId
    );
    assert_eq!(connections.len(), 1, "unknown id must not create a session");
    assert_eq!(
        apply_register_udp(&connections, 42, source),
        RegisterUdpOutcome::Bound
    );
    assert!(udp_handshake_complete(&connections, 42));
    assert_eq!(
        *connections.get(&42).unwrap().udp_endpoint.read(),
        Some(source)
    );
    let confirm = tcp_rx.try_recv().unwrap();
    assert_eq!(confirm, framework_registration(REGISTER_UDP, 0));
    let other: SocketAddr = "127.0.0.1:40001".parse().unwrap();
    assert_eq!(
        apply_register_udp(&connections, 42, other),
        RegisterUdpOutcome::AlreadyBound
    );
    assert_eq!(
        *connections.get(&42).unwrap().udp_endpoint.read(),
        Some(source),
        "second RegisterUDP must not duplicate or rebind"
    );
    assert!(tcp_rx.try_recv().is_err());
    let datagram = vec![46u8, 0, 1, 0, 0];
    assert!(
        !route_inbound_udp(&connections, other, &datagram),
        "unregistered UDP address is ignored"
    );
    assert!(udp_rx.try_recv().is_err());
    assert!(route_inbound_udp(&connections, source, &datagram));
    assert_eq!(udp_rx.try_recv().unwrap(), datagram);
    connections.remove(&42);
    assert_eq!(
        apply_register_udp(&connections, 42, source),
        RegisterUdpOutcome::UnknownId,
        "TCP close before UDP must drop the pending id"
    );
}

#[test]
fn p012_packet_serializer_threshold_and_reliable_routing() {
    use crate::network::codec::{
        read_packet, write_serialized_packet, PACKET_COMPRESS_MIN_BYTES, STREAM_CHUNK_WIRE_ID,
    };
    let small = vec![0xAAu8; PACKET_COMPRESS_MIN_BYTES - 1];
    let mut small_body = Vec::new();
    write_serialized_packet(&mut small_body, STATE_SNAPSHOT_PACKET_ID, &small).unwrap();
    assert_eq!(small_body[3], 0, "length < 36 must skip LZ4");

    let large = vec![0xAAu8; PACKET_COMPRESS_MIN_BYTES];
    let mut large_body = Vec::new();
    write_serialized_packet(&mut large_body, STATE_SNAPSHOT_PACKET_ID, &large).unwrap();
    assert_eq!(large_body[3], 1, "length >= 36 must LZ4");
    let mut large_tcp = Vec::new();
    write_tcp_packet(&mut large_tcp, STATE_SNAPSHOT_PACKET_ID, &large, true).unwrap();
    assert_eq!(
        u16::from_be_bytes([large_tcp[0], large_tcp[1]]) as usize,
        large_body.len()
    );
    assert_eq!(&large_tcp[2..], &large_body[..]);

    let mut chunk = Vec::new();
    write_serialized_packet(&mut chunk, STREAM_CHUNK_WIRE_ID, &large).unwrap();
    assert_eq!(chunk[3], 0, "StreamChunk is never compressed");

    assert!(packet_unreliable(ENTITY_SNAPSHOT_PACKET_ID));
    assert!(packet_unreliable(STATE_SNAPSHOT_PACKET_ID));
    assert!(packet_unreliable(CREATE_BULLET_PACKET_ID));
    assert!(!packet_unreliable(KICK_PACKET_ID));
    assert!(!packet_unreliable(WORLD_DATA_BEGIN_PACKET_ID));
    assert!(!packet_unreliable(CONSTRUCT_FINISH_PACKET_ID));

    let kick = frame_generated_packet(KICK_PACKET_ID, &[3], false).unwrap();
    let snapshot = frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &large, true).unwrap();
    let connections = DashMap::new();
    let (tx, mut rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    connections.insert(
        1,
        handshake_pending("127.0.0.1", tx, tokio::sync::mpsc::unbounded_channel().0),
    );
    broadcast(&connections, kick.clone());
    broadcast(&connections, snapshot.clone());
    assert_eq!(rx.try_recv().unwrap(), kick, "Kick stays on TCP");
    assert_eq!(
        rx.try_recv().unwrap(),
        snapshot,
        "unreliable falls back to TCP when UDP is not registered"
    );

    assert!(read_packet(std::io::Cursor::new(&[ENTITY_SNAPSHOT_PACKET_ID, 0, 8])).is_err());
    let framework = [FRAMEWORK_MESSAGE_ID as u8, KEEP_ALIVE];
    let decoded = read_packet(std::io::Cursor::new(framework)).unwrap();
    assert_eq!(decoded[0] as i8, FRAMEWORK_MESSAGE_ID);
    assert_ne!(decoded[0], ENTITY_SNAPSHOT_PACKET_ID);
}

#[test]
fn p012_unreliable_create_bullet_uses_udp_when_endpoint_is_bound() {
    let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    client
        .set_read_timeout(Some(std::time::Duration::from_millis(200)))
        .unwrap();
    let endpoint = client.local_addr().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let mut connection =
        handshake_pending("127.0.0.1", tx, tokio::sync::mpsc::unbounded_channel().0);
    *connection.udp_endpoint.write() = Some(endpoint);
    connection.udp_socket = Some(Arc::new(server));
    let connections = DashMap::new();
    connections.insert(7, connection);
    let payload = vec![0u8; 8];
    let frame = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false).unwrap();
    broadcast(&connections, frame.clone());
    assert!(
        rx.try_recv().is_err(),
        "CreateBullet must not take the TCP queue when UDP is registered"
    );
    let mut buf = [0u8; 256];
    let (len, _) = client.recv_from(&mut buf).expect("UDP datagram");
    assert_eq!(
        &buf[..len],
        &frame[2..],
        "UDP body is TCP frame without prefix"
    );
    let construct =
        frame_generated_packet(CONSTRUCT_FINISH_PACKET_ID, &[1, 2, 3, 4], false).unwrap();
    broadcast(&connections, construct.clone());
    assert_eq!(
        rx.try_recv().unwrap(),
        construct,
        "ConstructFinish is reliable TCP"
    );
}

#[test]
fn p108_snapshot_cadence_packing_and_health_coalesce() {
    assert_eq!(MAX_SNAPSHOT_SIZE, 800);
    assert_eq!(SNAPSHOT_INTERVAL, std::time::Duration::from_millis(50));
    assert_eq!(
        OFFICIAL_SNAPSHOT_INTERVAL,
        std::time::Duration::from_millis(200),
        "vanilla Config.snapshotInterval default is 200 ms; 50 ms is a documented deviation"
    );
    assert_eq!(BLOCK_SNAPSHOT_INTERVAL, std::time::Duration::from_secs(6));
    assert_eq!(HEALTH_SYNC_INTERVAL, std::time::Duration::from_millis(500));
    assert!(packet_unreliable(ENTITY_SNAPSHOT_PACKET_ID));
    assert!(packet_unreliable(STATE_SNAPSHOT_PACKET_ID));
    assert!(packet_unreliable(BLOCK_SNAPSHOT_PACKET_ID));

    for core in 339..=344 {
        assert!(is_core_block(core));
        assert!(
            !is_batch_snapshot_supported(core),
            "periodic block snapshots must skip cores"
        );
    }

    let mut pending = std::collections::HashMap::new();
    coalesce_build_health(&mut pending, 10, 90.0);
    coalesce_build_health(&mut pending, 10, 70.0);
    coalesce_build_health(&mut pending, 11, 50.0);
    let updates = take_coalesced_build_health(&mut pending);
    assert_eq!(updates, vec![(10, 70.0), (11, 50.0)]);
    assert!(pending.is_empty());
    let frame = encode_build_health_update_frame(&updates).unwrap();
    let packet = crate::network::codec::read_tcp_packet(std::io::Cursor::new(&frame)).unwrap();
    assert_eq!(packet[0], BUILD_HEALTH_UPDATE_PACKET_ID);
}

#[test]
fn p109_arcnet_keepalive_timeout_and_dos_gates() {
    use crate::network::session::{framework_ping, framework_ping_reply};

    assert_eq!(TCP_KEEPALIVE, std::time::Duration::from_millis(8000));
    assert_eq!(TCP_TIMEOUT, std::time::Duration::from_millis(12000));
    assert_eq!(UDP_KEEPALIVE, std::time::Duration::from_millis(19000));
    assert!(TCP_KEEPALIVE < TCP_TIMEOUT);

    let now = std::time::Instant::now();
    let last_write = now - TCP_KEEPALIVE;
    assert!(tcp_needs_keepalive(last_write, now));
    assert!(
        !tcp_idle_timed_out(last_write, now),
        "8 s idle is still under the 12 s TCP timeout"
    );
    assert!(tcp_idle_timed_out(now - TCP_TIMEOUT, now));

    let last_udp = now - UDP_KEEPALIVE;
    assert!(udp_needs_keepalive(last_udp, now));
    assert!(
        !tcp_idle_timed_out(now, now),
        "UDP keepalive must not be treated as a TCP timeout"
    );
    assert_eq!(
        framework_keepalive_udp(),
        [FRAMEWORK_MESSAGE_ID as u8, KEEP_ALIVE]
    );

    let ping = framework_ping(0x1122_3344);
    assert_eq!(&ping[..2], &7u16.to_be_bytes());
    assert_eq!(ping[2] as i8, FRAMEWORK_MESSAGE_ID);
    assert_eq!(ping[3], 0);
    assert_eq!(&ping[4..8], &0x1122_3344i32.to_be_bytes());
    assert_eq!(ping[8], 0);
    let decoded = read_packet(std::io::Cursor::new(&ping[2..])).unwrap();
    assert_eq!(parse_framework_ping(&decoded), Some((0x1122_3344, false)));
    let reply = framework_ping_reply(0x1122_3344);
    assert_eq!(reply.len(), ping.len());
    assert_eq!(reply[8], 1);
    let decoded_reply = read_packet(std::io::Cursor::new(&reply[2..])).unwrap();
    assert_eq!(
        parse_framework_ping(&decoded_reply),
        Some((0x1122_3344, true))
    );

    assert!(!connection_limit_blocks_accept(0, 100));
    assert!(!connection_limit_blocks_accept(1, 0));
    assert!(
        connection_limit_blocks_accept(1, 1),
        "playerLimit=1 rejects a second ArcNet TCP accept before world stream"
    );

    let admin = test_admin();
    admin.add_dos_ban("203.0.113.9");
    assert!(admin.is_dos_blacklisted("203.0.113.9"));
    assert!(!admin.is_dos_blacklisted("203.0.113.10"));

    let mut rate = ChatRateLimiter::new();
    let keepalive = vec![FRAMEWORK_MESSAGE_ID as u8, KEEP_ALIVE];
    for _ in 0..(PACKET_SPAM_LIMIT + 5) {
        assert!(
            record_inbound_game_packet(&mut rate, &keepalive),
            "framework KeepAlive must not count toward packet spam"
        );
    }
    let game = vec![CLIENT_SNAPSHOT_PACKET_ID, 0, 0];
    for _ in 0..PACKET_SPAM_LIMIT {
        assert!(record_inbound_game_packet(&mut rate, &game));
    }
    assert!(
        !record_inbound_game_packet(&mut rate, &game),
        "burst over packetSpamLimit in 3 s must close"
    );
}

#[test]
fn p110_send_message2_uses_player_name_not_ip() {
    let sender = player();
    let frame = encode_chat_message2_frame(&sender, "hello").unwrap();
    let packet = crate::network::codec::read_tcp_packet(std::io::Cursor::new(&frame)).unwrap();
    assert_eq!(packet[0], SEND_MESSAGE_2_PACKET_ID);
    let mut cursor = std::io::Cursor::new(&packet[1..]);
    use crate::network::codec::Reads;
    let formatted = cursor.read_typeio_string().unwrap().unwrap();
    let unformatted = cursor.read_typeio_string().unwrap().unwrap();
    assert_eq!(unformatted, "hello");
    assert!(
        !formatted.contains("127.0.0.1"),
        "chat must not embed the sender IP"
    );
    assert!(formatted.contains(&sender.name));
    let entity_id = i32::from_be_bytes(
        cursor.get_ref()[cursor.position() as usize..][..4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(entity_id, sender.id);

    let mut limiter = ChatRateLimiter::new();
    assert!(limiter.allow(2000, 2));
    assert!(limiter.allow(2000, 2));
    assert!(
        !limiter.allow(2000, 2),
        "second window of chatRate is 2 / 2000 ms"
    );
}

#[test]
fn p1_strict_anticheat_limits_client_movement_by_elapsed_time() {
    // M3: official NetServer.clientSnapshot anticheat (JAR offsets
    // 645-895) — strict movement is limited to
    // min(elapsedMs,1500)/1000*60*unit.speed()*1.1 world units with
    // alpha speed = 3.0 (UnitTypes.alpha), a SetPosition (110)
    // correction is returned beyond 112 units, and boosting is forced
    // off for alpha. Non-strict accepts the sanitized client position.
    let mut actor = player();
    actor.x = 0.0;
    actor.y = 0.0;
    let snapshot = ClientSnapshot {
        snapshot_id: 1,
        unit_id: actor.unit_id,
        dead: false,
        x: 10_000.0, // teleport attempt
        y: 0.0,
        mouse_x: 12.0,
        mouse_y: 13.0,
        rotation: 330.0,
        boosting: true, // alpha cannot boost
        shooting: false,
        building: true,
        mining_position: None,
        plans: Vec::new(),
    };
    // Non-strict accepts the client position (like the JAR); the
    // boosting clamp is unconditional in clientSnapshot (alpha cannot
    // boost, bytecode offsets 214-249).
    apply_client_snapshot(&mut actor, &snapshot, false, 50);
    assert_eq!(actor.x, 10_000.0);
    assert!(!actor.boosting, "alpha cannot boost: boosting forced off");
    // Strict with 50 ms elapsed: maxMove = 0.05*60*3.0*1.1 = 9.9 units.
    let mut strict_player = actor.clone();
    strict_player.x = 0.0;
    strict_player.y = 0.0;
    let corrected = apply_client_snapshot(&mut strict_player, &snapshot, true, 50);
    let expected = 0.05 * 60.0 * 3.0 * 1.1;
    assert!(
        (strict_player.x - expected).abs() < 0.01,
        "strict limits movement to 9.9 units: {} vs {expected}",
        strict_player.x
    );
    assert!(
        !strict_player.boosting,
        "alpha cannot boost: forcing boosting=false"
    );
    assert!(
        corrected.is_some(),
        "teleport beyond 112 units requests SetPosition(110)"
    );
    // Longer elapsed (1.5 s cap) allows more, still bounded.
    let mut capped = player();
    capped.x = 0.0;
    capped.y = 0.0;
    apply_client_snapshot(&mut capped, &snapshot, true, 5000);
    assert!(capped.x < 10_000.0, "elapsed is capped at 1500 ms");
    assert!((capped.x - 1.5 * 60.0 * 3.0 * 1.1).abs() < 0.01);
    // A clamped drift (9.9 < 112) stays within the correction distance:
    // no SetPosition.
    let mut near = player();
    near.x = 0.0;
    near.y = 0.0;
    let near_snapshot = ClientSnapshot {
        snapshot_id: 3,
        unit_id: near.unit_id,
        dead: false,
        x: 50.0,
        y: 0.0,
        mouse_x: 0.0,
        mouse_y: 0.0,
        rotation: 0.0,
        boosting: false,
        shooting: false,
        building: true,
        mining_position: None,
        plans: Vec::new(),
    };
    let corrected = apply_client_snapshot(&mut near, &near_snapshot, true, 50);
    assert!(
        (near.x - 9.9).abs() < 0.01,
        "50 > maxMove(9.9): movement clamped to 9.9"
    );
    assert!(
        corrected.is_none(),
        "within 112 units: no SetPosition correction"
    );
    // Dead snapshots never move the player.
    let mut dead = player();
    dead.x = 5.0;
    let dead_snapshot = ClientSnapshot {
        snapshot_id: 2,
        unit_id: dead.unit_id,
        dead: true,
        x: 9_999.0,
        y: 0.0,
        mouse_x: 0.0,
        mouse_y: 0.0,
        rotation: 0.0,
        boosting: false,
        shooting: false,
        building: true,
        mining_position: None,
        plans: Vec::new(),
    };
    apply_client_snapshot(&mut dead, &dead_snapshot, false, 50);
    assert_eq!(dead.x, 5.0);
}

#[test]
fn p1_connect_validation_rejects_name_bans_mods_and_strict_duplicates() {
    // P1: the connect gate now follows the official NetServer.connect
    // order: name bans -> recentKick -> playerLimit -> mods -> whitelist
    // -> version -> strict duplicates.
    let admin = test_admin();
    // Name ban kicks with reason 3 (banned).
    admin.ban_name("badplayer");
    assert!(admin.is_name_banned("BadPlayer"));
    assert!(admin.is_name_banned("  badplayer  "));
    assert!(!admin.is_name_banned("goodplayer"));
    admin.pardon_name("badplayer");
    assert!(!admin.is_name_banned("badplayer"));
    // Kick cooldown: handle_kicked(uuid, ip, duration) then
    // kick_time(uuid, ip) reports the cooldown (M4).
    assert!(admin.kick_time("uuid-1", "1.2.3.4").is_none());
    admin.handle_kicked("uuid-1", "1.2.3.4", std::time::Duration::from_secs(30));
    assert!(admin.kick_time("uuid-1", "1.2.3.4").is_some());
    // The message kick frame uses KickCallPacket (58) with a TypeIO string.
    let frame = kick_message_frame("Incompatible mods!\nUnnecessary mods:\n> test-mod").unwrap();
    let packet = crate::network::codec::read_tcp_packet(std::io::Cursor::new(&frame)).unwrap();
    assert_eq!(packet[0], KICK_PACKET_ID);
    assert!(packet.len() > 1, "string payload follows");
    // Reason kicks keep the ordinal layout (KickCallPacket2, id 59).
    let reason_frame = kick_reason_frame(3).unwrap();
    assert_eq!(reason_frame[2], KICK_2_PACKET_ID);
    assert_eq!(reason_frame[6], 3, "banned ordinal");
}

#[test]
fn p2_rules_overrides_apply_to_live_wave_rules() {
    // P2: the Administration global rules overrides (official `rules`
    // command) mutate the live WaveRules after map+mode resolution.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let admin_path = std::env::temp_dir().join(format!(
        "mindustry-p2-overrides-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&admin_path);
    let admin = crate::state::administration::Administration::with_file(admin_path.clone());
    admin
        .apply_rules_override("buildSpeedMultiplier", serde_json::json!(2.5))
        .unwrap();
    admin
        .apply_rules_override("infiniteResources", serde_json::json!(true))
        .unwrap();
    admin
        .apply_rules_override("waves", serde_json::json!(true))
        .unwrap();
    admin
        .apply_rules_override("waveTimer", serde_json::json!(false))
        .unwrap();
    admin
        .apply_rules_override("winWave", serde_json::json!(15))
        .unwrap();
    admin
        .apply_rules_override("bogusKey", serde_json::json!(1))
        .unwrap();
    apply_wave_rules_overrides(&world, &admin);
    let rules = world.wave_rules.read();
    assert_eq!(rules.build_speed_multiplier, 2.5);
    assert!(rules.infinite_resources);
    assert!(rules.waves_enabled);
    assert!(!rules.wave_timer);
    assert_eq!(rules.win_wave, 15);
    // Fields without overrides keep the map/mode values.
    assert_eq!(rules.default_team, 1);
    // No overrides: no-op (isolated admin file).
    let (world2, _, _, _) = legacy_weapons_test_world();
    let empty_path =
        std::env::temp_dir().join(format!("mindustry-p2-empty-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&empty_path);
    let empty = crate::state::administration::Administration::with_file(empty_path.clone());
    apply_wave_rules_overrides(&world2, &empty);
    assert_eq!(world2.wave_rules.read().build_speed_multiplier, 1.0);
    let _ = std::fs::remove_file(&admin_path);
    let _ = std::fs::remove_file(&empty_path);
}

#[test]
fn p2_map_rotation_advances_and_persists_index() {
    // P2: the rotation model used by the game-over lifecycle.
    let rotation_path =
        std::env::temp_dir().join(format!("mindustry-p2-rotation-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&rotation_path);
    let admin = crate::state::administration::Administration::with_file(rotation_path.clone());
    assert_eq!(admin.advance_map(), None, "empty rotation yields no map");
    admin.set_map_list(vec!["a".into(), "b".into()]);
    assert_eq!(admin.advance_map().as_deref(), Some("a"));
    assert_eq!(admin.advance_map().as_deref(), Some("b"));
    assert_eq!(admin.advance_map().as_deref(), Some("a"), "wraps around");
    // The autosave model reports due after the configured interval.
    assert!(!admin.autosave_due(1.0));
    admin.set_autosave_interval_secs(1);
    assert!(admin.autosave_due(1.5));
    let _ = std::fs::remove_file(&rotation_path);
}

#[test]
fn p2_decoders_never_panic_on_random_payloads() {
    // P2: decoder fuzz — arbitrary bounded payloads must yield Ok or a
    // structured Err, never a panic or an out-of-bounds access.
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};
    let mut rng = StdRng::seed_from_u64(0xD3C0D3);
    type Decoder = fn(&[u8]) -> std::io::Result<()>;
    let decoders: Vec<(&str, Decoder)> = vec![
        ("decode_tile_config", |p| decode_tile_config(p).map(|_| ())),
        ("decode_client_snapshot", |p| {
            decode_client_snapshot(p).map(|_| ())
        }),
        ("decode_admin_request", |p| {
            decode_admin_request(p).map(|_| ())
        }),
        ("decode_client_logic_data", |p| {
            decode_client_logic_data(p).map(|_| ())
        }),
        ("decode_request_drop_payload", |p| {
            decode_request_drop_payload(p).map(|_| ())
        }),
        ("decode_request_build_payload", |p| {
            decode_request_build_payload(p).map(|_| ())
        }),
        ("decode_request_unit_payload", |p| {
            decode_request_unit_payload(p).map(|_| ())
        }),
        ("decode_set_player_team_editor", |p| {
            decode_set_player_team_editor(p).map(|_| ())
        }),
        ("decode_server_packet_binary", |p| {
            decode_server_packet(p, true).map(|_| ())
        }),
        ("decode_server_packet_text", |p| {
            decode_server_packet(p, false).map(|_| ())
        }),
        ("decode_rotate_block", |p| {
            decode_rotate_block(p).map(|_| ())
        }),
        ("decode_delete_plans", |p| {
            decode_delete_plans(p).map(|_| ())
        }),
        ("decode_unit_control", |p| {
            decode_unit_control(p).map(|_| ())
        }),
        ("decode_command_building", |p| {
            decode_command_building(p).map(|_| ())
        }),
        ("decode_ping_location", |p| {
            decode_ping_location(p).map(|_| ())
        }),
        ("decode_building_control_select", |p| {
            decode_building_control_select(p).map(|_| ())
        }),
        ("decode_drop_item", |p| decode_drop_item(p).map(|_| ())),
        ("decode_typeio_string", |p| {
            decode_typeio_string(p).map(|_| ())
        }),
    ];
    for round in 0..2000 {
        let len = rng.gen_range(0..96);
        let payload: Vec<u8> = (0..len).map(|_| rng.gen()).collect();
        for (name, decoder) in &decoders {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decoder(&payload)));
            assert!(
                result.is_ok(),
                "{name} panicked on round {round} len {len}: {:?}",
                result.err()
            );
        }
    }
}

#[test]
fn p1_rules_fog_and_loadout_parse_and_override() {
    // P1: Rules.fog and Rules.loadout reach the authority (WaveRules)
    // and the console overrides mutate them.
    let rules = parse_wave_rules("{\"fog\":true,\"loadout\":\"copper-20/lead-10\",\"teams\":{}}");
    assert!(rules.fog);
    assert_eq!(rules.loadout, vec![(0, 20), (1, 10)]);
    // Entries without an amount are skipped; unknown item names fall
    // back to id 0 (the item registry's unknown-name fallback).
    let partial = parse_wave_rules("{\"loadout\":\"copper-5/nope-3/titanium\"}");
    assert_eq!(partial.loadout, vec![(0, 5), (0, 3)]);
    // Defaults.
    let default = parse_wave_rules("{}");
    assert!(!default.fog);
    assert!(default.loadout.is_empty());
    // The overrides helper applies both keys.
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let path = std::env::temp_dir().join(format!("mindustry-p1-fog-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let admin = crate::state::administration::Administration::with_file(path.clone());
    admin
        .apply_rules_override("fog", serde_json::json!(true))
        .unwrap();
    admin
        .apply_rules_override("loadout", serde_json::json!("copper-50"))
        .unwrap();
    apply_wave_rules_overrides(&world, &admin);
    let rules = world.wave_rules.read();
    assert!(rules.fog);
    assert_eq!(rules.loadout, vec![(0, 50)]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn p1_build_multipliers_zero_half_and_double_gate_construction() {
    // P1 (BuilderComp item): Rules.buildSpeed(team) with multipliers
    // 0, 0.5 and 2 must scale the remaining construction work, and 0
    // must never divide by zero or finish (official ConstructBlock
    // addProgress uses buildSpeed * delta; speed 0 never completes).
    let (world, _connections, _, _) = legacy_weapons_test_world();
    let pos = (45 << 16) | 100;
    let base_time = crate::game::content::block_build_time(216) / 0.5;
    let make = |_speed: f32| {
        let mut builder = player();
        builder.uuid = "speed-test".into();
        PendingBuild {
            position: pos,
            block: 216,
            rotation: 0,
            config: Vec::new(),
            occupied: vec![pos],
            team: 1,
            builder,
            last_seen: std::time::Instant::now(),
            assist_progress: 0.0,
            remaining_ticks: 0.0,
            applied_assist: 0.0,
        }
    };
    // x2: half the base time.
    {
        world.wave_rules.write().build_speed_multiplier = 2.0;
        let pending = make(2.0);
        world.pending_builds.insert(pos, pending.clone());
        schedule_build(&world, &pending);
        let remaining = world.pending_builds.get(&pos).unwrap().remaining_ticks;
        assert!(
            (remaining - base_time / 2.0).abs() < 1.0,
            "x2 halves remaining: {remaining} vs {}",
            base_time / 2.0
        );
        world.pending_builds.remove(&pos);
    }
    // x0.5: double the base time.
    {
        world.wave_rules.write().build_speed_multiplier = 0.5;
        let pending = make(0.5);
        world.pending_builds.insert(pos, pending.clone());
        schedule_build(&world, &pending);
        let remaining = world.pending_builds.get(&pos).unwrap().remaining_ticks;
        assert!(
            (remaining - base_time / 0.5).abs() < 1.0,
            "x0.5 doubles remaining: {remaining} vs {}",
            base_time / 0.5
        );
        world.pending_builds.remove(&pos);
    }
    // x0: never finishes, no divide-by-zero, plan still registered.
    {
        world.wave_rules.write().build_speed_multiplier = 0.0;
        let pending = make(0.0);
        world.pending_builds.insert(pos, pending.clone());
        schedule_build(&world, &pending);
        let remaining = world.pending_builds.get(&pos).unwrap().remaining_ticks;
        assert!(
            remaining.is_finite() && remaining > base_time * 100.0,
            "x0 leaves an effectively infinite build: {remaining}"
        );
        // The plan stays registered with an effectively infinite build.
        assert!(world.pending_builds.get(&pos).unwrap().remaining_ticks > 0.0);
        world.pending_builds.remove(&pos);
    }
    world.wave_rules.write().build_speed_multiplier = 1.0;
}

#[test]
fn p1_map_building_tail_only_becomes_config_when_typeio_valid() {
    // P1: the map's raw Building tail is config only when it parses as a
    // complete TypeIO object; non-TypeIO legacy tails stay out of
    // config (PROTOCOL-RULES rule 5).
    use crate::engine::world_stream::NetworkBuilding;
    let make = |extra_data: Vec<u8>| {
        let building = NetworkBuilding {
            position: (45 << 16) | 100,
            block: 264, // sorter
            health: 100.0,
            rotation: 0,
            team: 1,
            inventory: Vec::new(),
            power_links: Vec::new(),
            power_status: 0.0,
            liquids: Vec::new(),
            enabled: true,
            extra_data,
        };
        network_building_tile(&building, vec![(45 << 16) | 100])
    };
    // A valid TypeIO item selection survives as config.
    let tile = make(vec![5, 0, 0, 3]);
    assert_eq!(tile.config, vec![5, 0, 0, 3]);
    // A raw non-TypeIO tail (e.g. legacy subclass bytes) is NOT config.
    let tile = make(vec![0xAA, 0xBB, 0xCC, 0xDD]);
    assert_eq!(tile.config, vec![0], "legacy tail is not treated as config");
    // Empty tails stay null-config.
    let tile = make(Vec::new());
    assert_eq!(tile.config, vec![0]);
}

#[test]
fn core_inventory_enforces_topology_capacity_and_repairs_legacy_overflow() {
    let (world, _, _, _) = legacy_weapons_test_world();
    world.cores.clear();
    world.team_core_lists.clear();
    let nucleus = (100 << 16) | 100;
    crate::network::world::register_team_core(
        &world,
        1,
        TeamCore {
            position: nucleus,
            block: 341,
            health: 6_000.0,
            max_health: 6_000.0,
        },
    );
    assert_eq!(
        crate::network::core_inventory::core_item_capacity(&world, 1),
        13_000
    );

    world.game_state.core_items.write()[4] = 12_999;
    // Vanilla coreIncinerates consumes the whole source stack while its
    // shared ItemModule remains capped.
    assert_eq!(
        crate::network::core_inventory::deposit_core_items(&world, 1, 4, 30),
        30
    );
    assert_eq!(world.game_state.core_items.read()[4], 13_000);

    world.wave_rules.write().core_incinerates = false;
    world.game_state.core_items.write()[4] = 12_999;
    assert_eq!(
        crate::network::core_inventory::deposit_core_items(&world, 1, 4, 30),
        1
    );
    assert_eq!(world.game_state.core_items.read()[4], 13_000);

    // A Serpulo container touching the core has coreMerge=true and adds
    // 300 to the capacity; reinforced storage deliberately does not.
    let container_pos = (103 << 16) | 100;
    let container = DynamicTile {
        position: container_pos,
        block: 345,
        team: 1,
        occupied: block_footprint(&world, container_pos, 345).unwrap(),
        ..Default::default()
    };
    world.tiles.insert(container_pos, container);
    assert_eq!(
        crate::network::core_inventory::core_item_capacity(&world, 1),
        13_300
    );

    world.game_state.core_items.write()[4] = 117_720;
    assert!(crate::network::core_inventory::clamp_core_inventories(
        &world
    ));
    assert_eq!(
        world.game_state.core_items.read()[4],
        13_300,
        "the pruebas01-style overflow is healed on load"
    );
}

#[test]
fn completed_client_plans_never_rebroadcast_construct_finish() {
    let (world, _, _, _) = legacy_weapons_test_world();
    let world = Arc::new(world);
    let position = (45 << 16) | 100;
    let tile = DynamicTile {
        position,
        block: 257,
        team: 1,
        occupied: vec![position],
        ..Default::default()
    };
    world.tiles.insert(position, tile);

    let (tx, mut rx) = tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
    let connections = Arc::new(DashMap::new());
    connections.insert(
        1,
        PendingConnection {
            ip: "127.0.0.1".parse().unwrap(),
            outbound: tx,
            udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
            udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
            udp_socket: None,
            player_name: Arc::new(parking_lot::RwLock::new(Some("builder".into()))),
            outbound_drops: Arc::new(AtomicU64::new(0)),
            critical_drops: Arc::new(AtomicU64::new(0)),
            last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
            last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
            outbound_queued: Arc::new(AtomicU64::new(0)),
        },
    );
    let plan = BuildPlan {
        breaking: false,
        position,
        block: 257,
        rotation: 0,
        config: vec![0],
    };
    let mut builder = player();
    builder.active_plans.insert((false, position, 257));
    let admin = test_admin();
    for _ in 0..100 {
        apply_build_plans(
            &mut builder,
            std::slice::from_ref(&plan),
            &world,
            &connections,
            &admin,
            true,
        )
        .unwrap();
    }
    assert!(!builder.active_plans.contains(&(false, position, 257)));
    assert!(world.pending_builds.is_empty());
    assert!(
        rx.try_recv().is_err(),
        "no duplicate reliable finish frames"
    );
    assert_eq!(
        connections
            .get(&1)
            .unwrap()
            .outbound_queued
            .load(Ordering::Relaxed),
        0
    );
}

#[test]
fn non_solid_mass_builds_do_not_invalidate_navigation() {
    let (world, _, _, _) = legacy_weapons_test_world();
    for _ in 0..1_000 {
        invalidate_navigation_for_block(&world, 257); // conveyor
    }
    assert_eq!(world.navigation_revision.load(Ordering::Relaxed), 0);
    invalidate_navigation_for_block(&world, 216); // copper wall
    assert_eq!(world.navigation_revision.load(Ordering::Relaxed), 1);
}

// ------------------------------------------------------------------
// Wave generation from the loaded map's Rules (audit §4.1 CRÍTICA):
// spawn groups + waveSpacing must come from the MSAV, not the bundled
// maze table; SpawnGroup.max caps amounts; effect/shields are applied.
// ------------------------------------------------------------------

#[test]
fn map_wave_rules_parse_from_ground_zero_msav() {
    use crate::engine::world_stream::{inspect_metadata, replace_map_from_msav};
    let Some(msav) = official_msav("groundZero.msav") else {
        return;
    };
    let template = include_bytes!("../../dummy_world.dat");
    let replaced = replace_map_from_msav(template, &msav).unwrap();
    let metadata = inspect_metadata(&replaced).unwrap();
    let rules = parse_wave_rules(&metadata.rules);

    // groundZero.msav (official vanilla map) defines 5 spawn groups and no
    // explicit waveSpacing/initialWaveSpacing: the official 7200 default
    // applies and the first-wave delay resolves to waveSpacing * 2 (14400).
    assert_eq!(rules.wave_spacing, DEFAULT_WAVE_SPACING);
    assert_eq!(rules.initial_wave_spacing, 14400.0);
    assert!(!rules.is_default());
    assert_eq!(rules.spawn_groups.len(), 5);
    // Gameplay multipliers: groundZero leaves the official defaults.
    assert_eq!(rules.build_speed_multiplier, 1.0, "default buildSpeed");
    assert_eq!(rules.unit_mine_speed_multiplier, 1.0, "default mineSpeed");
    assert_eq!(rules.block_health_multiplier, 1.0, "default blockHealth");
    assert_eq!(rules.block_damage_multiplier, 1.0, "default blockDamage");
    assert_eq!(rules.unit_damage_multiplier, 1.0, "default unitDamage");
    assert_eq!(rules.unit_health_multiplier, 1.0, "default unitHealth");
    assert!(!rules.infinite_resources);
    assert!(rules.can_game_over);
    assert!(!rules.instant_build);

    let dagger = &rules.spawn_groups[0];
    assert_eq!(dagger.unit_type, 0); // dagger
    assert_eq!(dagger.begin, 0);
    assert_eq!(dagger.end, 1);
    assert_eq!(dagger.spacing, 1);
    assert_eq!(dagger.max, 40); // default when the JSON has no max
    assert_eq!(dagger.scaling, 2.0);
    assert_eq!(dagger.spawn, -1); // no per-tile spawn overlay
    assert_eq!(dagger.effect, -1); // effect:none

    // flare (15) is a wave-2 one-off group (begin=end=1, amount 2).
    let flare = &rules.spawn_groups[1];
    assert_eq!(flare.unit_type, 15);
    assert_eq!((flare.begin, flare.end), (1, 1));
    assert_eq!(flare.spacing, 3);
    assert_eq!(flare.unit_amount, 2);
    assert_eq!(flare.effect, -1);

    // mace (1) enters at wave 7 with spacing 3.
    let mace = rules
        .spawn_groups
        .iter()
        .find(|group| group.unit_type == 1)
        .expect("groundZero defines mace groups");
    assert_eq!(mace.begin, 7);
    assert_eq!(mace.spacing, 3);
    assert_eq!(mace.scaling, 2.0);
    assert_eq!(mace.effect, -1);
}

#[test]
fn map_wave_spawns_use_loaded_rules_not_maze_table() {
    use crate::engine::world_stream::{inspect_metadata, replace_map_from_msav};
    let Some(msav) = official_msav("groundZero.msav") else {
        return;
    };
    let template = include_bytes!("../../dummy_world.dat");
    let replaced = replace_map_from_msav(template, &msav).unwrap();
    let metadata = inspect_metadata(&replaced).unwrap();
    let rules = parse_wave_rules(&metadata.rules);

    // Wave 1 (index 0): both the groundZero rules and the bundled maze table
    // spawn a single dagger. This is the common baseline; the maps diverge
    // from wave 2 onward (verified against the real MSAV).
    let wave_one = map_wave_spawns(0, &rules);
    let maze_one = initial_official_wave_groups(0);
    assert_eq!(maze_one.len(), 1);
    assert_eq!(maze_one[0].spec.unit_type, 0);
    assert_eq!(
        wave_one.len(),
        1,
        "groundZero wave 1 = 1 dagger, got {wave_one:?}"
    );
    assert_eq!(wave_one[0].spec.unit_type, 0);
    assert_eq!(wave_one[0].amount, 1);
    assert_eq!(wave_one[0].status_effect, -1);
    assert_eq!(wave_one[0].spawn, -1);

    // Wave 2 (index 1): groundZero adds its flare one-off (2 flares); the
    // maze table still has only 1 dagger. The loaded map rules must win.
    let wave_two = map_wave_spawns(1, &rules);
    assert_eq!(
        wave_two.iter().map(|group| group.amount).sum::<u32>(),
        3,
        "groundZero wave 2 = 1 dagger + 2 flares, got {wave_two:?}"
    );
    let flares: u32 = wave_two
        .iter()
        .filter(|group| group.spec.unit_type == 15)
        .map(|group| group.amount)
        .sum();
    assert_eq!(flares, 2);
    assert!(wave_two
        .iter()
        .filter(|group| group.spec.unit_type == 15)
        .all(|group| group.status_effect == -1));
    assert_eq!(
        initial_official_wave_groups(1).len(),
        1,
        "maze wave 2 = 1 dagger"
    );

    // Wave 5 (index 4): groundZero scales its daggers to 3 (scaling 2,
    // spacing 1 -> +1 per 2 waves from the wave-2 group + base group);
    // the maze table fields 3 daggers + 2 crawlers. Check totals so a
    // regression to the maze fallback fails.
    let wave_five = map_wave_spawns(4, &rules);
    let maze_five = initial_official_wave_groups(4);
    assert_eq!(
        wave_five.iter().map(|group| group.amount).sum::<u32>(),
        3,
        "groundZero wave 5 = 3 daggers, got {wave_five:?}"
    );
    assert!(wave_five
        .iter()
        .all(|group| group.spec.unit_type == 0 && group.status_effect == -1));
    assert_eq!(maze_five.len(), 2, "maze wave 5 = daggers + crawlers");
    assert!(maze_five
        .iter()
        .any(|group| group.spec.unit_type == 10 && group.amount == 2));

    // Wave 8 (index 7): groundZero has 4 daggers + 3 flares + 1 mace;
    // the maze table fields 4 daggers + 4 crawlers + 1 mace.
    let wave_eight = map_wave_spawns(7, &rules);
    assert_eq!(
        wave_eight.iter().map(|group| group.amount).sum::<u32>(),
        8,
        "groundZero wave 8 = 4 daggers + 3 flares + 1 mace, got {wave_eight:?}"
    );
    let crawlers: u32 = wave_eight
        .iter()
        .filter(|group| group.spec.unit_type == 10)
        .map(|group| group.amount)
        .sum();
    assert_eq!(crawlers, 0, "groundZero has no crawlers");
    let maces: u32 = wave_eight
        .iter()
        .filter(|group| group.spec.unit_type == 1)
        .map(|group| group.amount)
        .sum();
    assert_eq!(maces, 1);
    assert!(wave_eight.iter().all(|group| group.status_effect == -1));

    let maze_eight = initial_official_wave_groups(7);
    assert_eq!(
        maze_eight.iter().map(|group| group.amount).sum::<u32>(),
        9,
        "maze wave 8 = 4 daggers + 4 crawlers + 1 mace"
    );
    assert!(maze_eight
        .iter()
        .any(|group| group.spec.unit_type == 1 && group.amount == 1));
}

#[test]
fn spawn_group_amount_respects_map_max_not_fixed_40() {
    use crate::network::units::MapSpawnGroup;

    let group = |begin: u32, end: u32, spacing: u32, max: u32, scaling: f32, amount: u32| {
        MapSpawnGroup {
            unit_type: 1, // mace
            begin,
            end,
            spacing,
            max,
            scaling,
            shields: 0.0,
            shield_scaling: 0.0,
            unit_amount: amount,
            spawn: -1,
            effect: -1,
        }
    };
    // mace max=120 (like the official `mace 120` late-wave group): the
    // old heuristic cap of 40 must not truncate it. 5 + 100/1/1 = 105.
    let big = group(0, u32::MAX, 1, 120, 1.0, 5);
    assert_eq!(map_spawn_group_amount(100, &big), 105);
    assert_eq!(map_spawn_group_amount(200, &big), 120); // capped by max
                                                        // capped group mirrors a map max (e.g. groundZero flare max=40,
                                                        // custom maps may set lower): the cap must be respected.
    let capped = group(0, u32::MAX, 1, 10, 1.0, 1);
    assert_eq!(map_spawn_group_amount(99, &capped), 10);
    // arkyid: begin 14, end 15, max 2, boss. With scaling=1 the amount
    // grows (1 then 2) until end=15; afterwards it stops.
    let boss = group(14, 15, 1, 2, 1.0, 1);
    assert_eq!(map_spawn_group_amount(13, &boss), 0);
    assert_eq!(map_spawn_group_amount(14, &boss), 1);
    assert_eq!(map_spawn_group_amount(15, &boss), 2);
    assert_eq!(map_spawn_group_amount(16, &boss), 0);
    // scaling absent (unitScaling=never): amount stays at unitAmount.
    let never_scaling = group(0, u32::MAX, 1, 40, i32::MAX as f32, 1);
    assert_eq!(map_spawn_group_amount(1_000, &never_scaling), 1);
    // spacing 0 in JSON is normalized to 1, like SpawnGroup.getSpawned.
    let zero_spacing = group(0, u32::MAX, 0, 10, i32::MAX as f32, 1);
    assert_eq!(map_spawn_group_amount(7, &zero_spacing), 1);
}

#[test]
fn spawn_wave_uses_loaded_map_groups_and_applies_effects() {
    use crate::engine::world_stream::replace_map_from_msav;
    let Some(msav) = official_msav("groundZero.msav") else {
        return;
    };
    let template = include_bytes!("../../dummy_world.dat");
    let replaced = replace_map_from_msav(template, &msav).unwrap();
    let state = GameState::new();
    state.start_hosting("groundZero".into(), GameMode::Survival);
    let world = fresh_world_from_template(
        &state,
        replaced,
        "groundZero".into(),
        std::env::temp_dir().join("groundzero-wave-test.json"),
    )
    .unwrap();
    assert!(!world.wave_rules.read().is_default());
    assert_eq!(*world.game_state.wave_time.read(), 14400.0); // waveSpacing * 2

    spawn_wave(&world);
    let wave = world.game_state.wave.load(Ordering::Relaxed);
    assert_eq!(wave, 2); // fetch_add: counter moves past the spawned wave
    let enemies: Vec<_> = world
        .enemies
        .iter()
        .map(|enemy| {
            (
                enemy.unit_type,
                enemy.status_effect,
                enemy.shield,
                enemy.move_speed,
                enemy.health,
            )
        })
        .collect();
    assert_eq!(
        enemies.len(),
        1,
        "groundZero wave 1 must be a single dagger"
    );
    let (unit_type, effect, shield, _speed, _health) = enemies[0];
    assert_eq!(unit_type, 0);
    assert_eq!(effect, -1);
    assert_eq!(shield, 0.0);

    // Maze fallback (no map spawns): unchanged single dagger on wave 1.
    let maze_state = GameState::new();
    maze_state.start_hosting("maze".into(), GameMode::Survival);
    let maze_world = fresh_world_from_template(
        &maze_state,
        include_bytes!("../../dummy_world.dat").to_vec(),
        "maze".into(),
        std::env::temp_dir().join("maze-wave-test.json"),
    )
    .unwrap();
    // The bundled maze template defines its own 27 spawns in its rules
    // JSON, so its wave-1 composition is the same single dagger.
    assert_eq!(maze_world.wave_rules.read().spawn_groups.len(), 27);
    assert!(!maze_world.wave_rules.read().is_default());
    spawn_wave(&maze_world);
    let maze_enemies: Vec<_> = maze_world
        .enemies
        .iter()
        .map(|enemy| enemy.unit_type)
        .collect();
    assert_eq!(maze_enemies, vec![0]); // one dagger

    // Maps whose rules define no spawns fall back to the bundled table
    // (official Map.rules(): `if(result.spawns.isEmpty()) result.spawns =
    // Vars.waves.get()`); the fallback composition itself is covered by
    // initial_wave_composition_matches_bundled_rules.
    assert!(parse_wave_rules("{\"waves\":true}").is_default());
    assert!(parse_wave_rules("{}").is_default());
    assert!(WaveRules::default().is_default());
    assert!(map_wave_spawns(0, &WaveRules::default()).is_empty());
}

#[test]
fn wave_spacing_after_spawn_uses_map_value() {
    use crate::engine::world_stream::replace_map_from_msav;
    let Some(msav) = official_msav("frozenForest.msav") else {
        return;
    };
    let template = include_bytes!("../../dummy_world.dat");
    let replaced = replace_map_from_msav(template, &msav).unwrap();
    let state = GameState::new();
    state.start_hosting("frozenForest".into(), GameMode::Survival);
    let world = fresh_world_from_template(
        &state,
        replaced,
        "frozenForest".into(),
        std::env::temp_dir().join("frozenforest-wave-timing-test.json"),
    )
    .unwrap();
    *world.game_state.wave_time.write() = 1.0;
    let connections = DashMap::new();
    simulate_waves_and_enemies(&world, &connections, 6.0);
    // Official Logic.runWave(): after spawning, wavetime = the map's
    // waveSpacing. frozenForest.msav sets waveSpacing:7800 explicitly.
    assert_eq!(*world.game_state.wave_time.read(), 7800.0);
    assert_eq!(world.game_state.wave.load(Ordering::Relaxed), 2);
    // The map's first-wave delay is waveSpacing * 2 (initialWaveSpacing absent).
    assert_eq!(world.wave_rules.read().initial_wave_spacing, 15600.0);

    // The maze template defines no waveSpacing: official 7200 default.
    let maze_state = GameState::new();
    maze_state.start_hosting("maze".into(), GameMode::Survival);
    let maze_world = fresh_world_from_template(
        &maze_state,
        include_bytes!("../../dummy_world.dat").to_vec(),
        "maze".into(),
        std::env::temp_dir().join("maze-wave-timing-test.json"),
    )
    .unwrap();
    assert_eq!(maze_world.wave_rules.read().wave_spacing, 7200.0);
    assert_eq!(*maze_world.game_state.wave_time.read(), 14400.0);
}

#[test]
fn ulocate_ore_finds_overlay_with_official_158_content_ids() {
    // Regression: ulocate ore used pre-v8 floor ids (73-80) that never match
    // the v158.1 overlay registry (ore-copper 167 .. ore-tungsten 174), so
    // `ulocate ore copper` could never find anything. The world below places
    // copper at (10,10) and lead at (20,20) using the official ids; the find
    // must return the nearest copper tile.
    use crate::logic::{UlocKind, UlocSpec};
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("ulocate-ore-test".into(), GameMode::Survival);
    let enemy_spawns = map.enemy_spawns();
    let width = i32::from(map.width);
    let mut overlays = map.overlays.clone();
    // ore-copper = 167 at (10,10); ore-lead = 168 at (20,20).
    overlays[10 * width as usize + 10] = 167;
    overlays[20 * width as usize + 20] = 168;
    let world = DynamicWorld {
        game_state: state,
        width,
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays,
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_001),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("ulocate-ore-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let connections = DashMap::new();
    let view = crate::logic::WorldView {
        world: &world,
        processor_pos: 0,
        out: &connections,
    };
    let spec = UlocSpec {
        kind: UlocKind::Ore,
        group: crate::logic::UlocGroup::All,
        enemy: crate::logic::Expr::Num(0.0),
        ore: Some("copper".into()),
        building: 0,
        x: 0,
        y: 0,
    };
    // Origin near (10,10): must find copper at exactly (10,10) -> world (80,80).
    let found = view
        .ulocate_find(&spec, false, 1)
        .expect("copper ore found");
    let (obj, x, y) = found;
    // Official LLocate.ore writes x/y and leaves the result null.
    assert!(
        matches!(obj, crate::logic::LObject::Null),
        "ore leaves result null"
    );
    assert!(
        (x - 80.0).abs() < 0.001,
        "copper x = 10 tiles * 8 = 80, got {x}"
    );
    assert!((y - 80.0).abs() < 0.001, "copper y = 80, got {y}");

    // Requesting lead from the same origin must find (20,20) -> world (160,160).
    let spec_lead = UlocSpec {
        ore: Some("lead".into()),
        ..spec
    };
    let found = view
        .ulocate_find(&spec_lead, false, 1)
        .expect("lead ore found");
    let (_, x, y) = found;
    assert!((x - 160.0).abs() < 0.001, "lead x = 160, got {x}");
    assert!((y - 160.0).abs() < 0.001, "lead y = 160, got {y}");
}

#[test]
fn unit_save_revision_matches_jar_158_entity_codecs() {
    // Table extracted from desktop.jar 158.1 by invoking each unit type's
    // save `write()` (DumpUnitRevisions.java): the first short is revision.
    let expected: &[(u8, i16)] = &[
        (0, 5),  // alpha (UnitEntityLegacyAlpha)
        (2, 9),  // block
        (3, 9),  // UnitEntity (aerial)
        (4, 9),  // MechUnit
        (5, 7),  // PayloadUnit
        (16, 8), // mono
        (17, 7), // nova
        (18, 7), // poly
        (19, 5), // pulsar
        (20, 9), // UnitWaterMove (naval)
        (21, 8), // spiroct
        (23, 8), // quad
        (24, 9), // LegsUnit
        (26, 7), // oct
        (29, 5), // arkyid
        (30, 5), // beta
        (31, 5), // gamma
        (32, 5), // quasar
        (33, 5), // toxopid
        (36, 3), // BuildingTetherPayloadUnit (assembly-drone)
        (39, 3), // TimedKillUnit (missiles)
        (43, 2), // TankUnit (erekir)
        (45, 2), // ElevationMoveUnit (elude)
        (46, 2), // CrawlUnit (latum/renale)
    ];
    for &(class, revision) in expected {
        assert_eq!(
            unit_save_revision(class),
            revision,
            "entity class {class} must use save revision {revision}"
        );
    }
}

#[test]
fn kick_call_packet2_layout_matches_jar() {
    // KickCallPacket2 (ID 59): TypeIO.writeKick = b reason ordinal.
    // Verified against desktop.jar (Net.getPacketId(KickCallPacket2)=59,
    // TypeIO.writeKick writes b ordinal).
    for (reason, expected) in [
        (0u8, 0u8), // kick
        (1, 1),     // clientOutdated
        (2, 2),     // serverOutdated
        (8, 8),     // nameEmpty
        (9, 9),     // customClient
        (12, 12),   // typeMismatch
    ] {
        let frame = kick_reason_frame(reason).unwrap();
        assert_eq!(frame[2], KICK_2_PACKET_ID, "packet id must be 59");
        // wire format: u16 len + id + u16 payload len + compress byte + payload
        let data = &frame[6..];
        assert_eq!(data, &[expected], "kick reason payload for {reason}");
    }
}

#[test]
fn enemy_projectile_collides_with_building_in_flight() {
    // Official BulletComp.update -> tileRaycast: a bullet collides with the
    // first solid enemy building its segment crosses, stopping there instead
    // of flying to the core. The enemy bullet travels from east toward the
    // core; a sharded wall sits midway -> the wall takes the hit, the core
    // survives.
    let map =
        crate::engine::world_stream::inspect_map(include_bytes!("../../dummy_world.dat")).unwrap();
    let state = GameState::new();
    state.start_hosting("projectile-collision-test".into(), GameMode::Survival);
    let enemy_spawns = map.enemy_spawns();
    let world = DynamicWorld {
        game_state: state,
        width: i32::from(map.width),
        height: i32::from(map.height),
        sharded_unit_cap: 8,
        core_position: (i32::from(SPAWN_X) << 16) | i32::from(SPAWN_Y),
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: map.blocks,
        base_centers: map.block_centers,
        tile_data: map.tile_data,
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: map.floors,
        overlays: map.overlays,
        enemy_spawns,
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_001),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("projectile-collision-test.json"),
        network_template: Arc::new(include_bytes!("../../dummy_world.dat").to_vec()),
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
    };
    let core_x = i32::from(SPAWN_X) as f32 * 8.0;
    let core_y = i32::from(SPAWN_Y) as f32 * 8.0;
    // Wall 6 tiles east of the core: block 216 (copper wall), team 1.
    let wall_tile = ((i32::from(SPAWN_X) + 6) << 16) | i32::from(SPAWN_Y);
    let mut wall = base_building_tombstone(&BaseBuildingState {
        position: wall_tile,
        block: 216,
        team: 1,
        health: crate::game::content::block_health(216),
        occupied: vec![wall_tile],
        inventory: Vec::new(),
    });
    wall.block = 216;
    wall.team = 1;
    wall.health = crate::game::content::block_health(216);
    world.tiles.insert(wall_tile, wall);

    // Enemy bullet from 30 tiles east, aimed at the core (20 ticks of flight).
    world.projectiles.insert(
        4_900_010,
        Projectile {
            target_id: 0,
            shooter_id: 0,
            team: 2,
            bullet_id: 31,
            damage: 500.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: true,
            apply_direct_on_impact: true,
            armor_multiplier: 1.0,
            remaining_ticks: 20.0,
            total_ticks: 20.0,
            source_x: core_x + 240.0,
            source_y: core_y,
            target_x: core_x,
            target_y: core_y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    let connections = DashMap::new();
    // Simulate 1 tick: the bullet moved 1/20 of the way (12 px) — far from
    // the wall (48 px) yet the wall footprint (8 px + hitSize 4) is not
    // reached. Then simulate the remaining 19 ticks: it must hit the wall,
    // not the core.
    simulate_projectiles(&world, &connections, 1.0);
    assert!(world.projectiles.contains_key(&4_900_010), "still flying");
    simulate_projectiles(&world, &connections, 19.0);
    assert!(world.projectiles.is_empty(), "bullet consumed by collision");
    let wall_health = world.tiles.get(&wall_tile).map(|t| t.health).unwrap_or(0.0);
    assert!(
        wall_health < crate::game::content::block_health(216),
        "wall took the hit: health={wall_health}"
    );
    assert_eq!(
        *world.game_state.core_health.read(),
        6_000.0,
        "core must survive the in-flight collision"
    );
}

#[test]
fn wave_rules_parse_gameplay_multipliers_from_json() {
    // Rules.java v158.1 fields parsed from the map's rules JSON.
    let rules = parse_wave_rules(
        "{\"buildSpeedMultiplier\":2.5,\"unitMineSpeedMultiplier\":0.5,\"blockHealthMultiplier\":3.0,\"blockDamageMultiplier\":0.25,\"unitDamageMultiplier\":1.5,\"unitHealthMultiplier\":2.0,\"infiniteResources\":true,\"canGameOver\":false,\"instantBuild\":true}",
    );
    assert_eq!(rules.build_speed_multiplier, 2.5);
    assert_eq!(rules.unit_mine_speed_multiplier, 0.5);
    assert_eq!(rules.block_health_multiplier, 3.0);
    assert_eq!(rules.block_damage_multiplier, 0.25);
    assert_eq!(rules.unit_damage_multiplier, 1.5);
    assert_eq!(rules.unit_health_multiplier, 2.0);
    assert!(rules.infinite_resources);
    assert!(!rules.can_game_over);
    assert!(rules.instant_build);

    // Zero is a valid multiplier in Rules.java (do not silently replace it
    // with the default 1.0), and the full wave/team contract is map data.
    let zero = parse_wave_rules(
        "{\"unitDamageMultiplier\":0.0,\"unitHealthMultiplier\":0.0,\"waves\":true,\"waveTimer\":false,\"waveSending\":false,\"waitEnemies\":true,\"winWave\":12,\"waveTeam\":7,\"defaultTeam\":5,\"waveSpacing\":7200.0}",
    );
    assert_eq!(zero.unit_damage_multiplier, 0.0);
    assert_eq!(zero.unit_health_multiplier, 0.0);
    assert!(zero.waves_enabled);
    assert!(!zero.wave_timer);
    assert!(!zero.wave_sending);
    assert!(zero.wait_enemies);
    assert_eq!(zero.win_wave, 12);
    assert_eq!(zero.wave_team, 7);
    assert_eq!(zero.default_team, 5);
    assert_eq!(zero.initial_wave_spacing, 14_400.0);
    // Explicit initialWaveSpacing=0 (the Java default) must still resolve to
    // waveSpacing * 2 per Logic.play(), never an immediate first wave.
    let zero_initial = parse_wave_rules("{\"waveSpacing\":10800.0,\"initialWaveSpacing\":0.0}");
    assert_eq!(zero_initial.initial_wave_spacing, 21_600.0);
    // Defaults remain official.
    let default = WaveRules::default();
    assert_eq!(default.build_speed_multiplier, 1.0);
    assert_eq!(default.unit_mine_speed_multiplier, 1.0);
    assert_eq!(default.block_health_multiplier, 1.0);
    assert_eq!(default.block_damage_multiplier, 1.0);
    assert_eq!(default.unit_damage_multiplier, 1.0);
    assert_eq!(default.unit_health_multiplier, 1.0);
    assert!(default.can_game_over);
    assert!(!default.infinite_resources);
    assert!(!default.instant_build);
    assert_eq!(default.wave_spacing, DEFAULT_WAVE_SPACING);
    // Rules.possessionAllowed defaults to true (Rules.java:61).
    assert!(default.possession_allowed);
}

#[test]
fn hot_host_template_populates_authoritative_building_tiles_and_modules() {
    let state = GameState::new();
    state.start_hosting("frontier".into(), GameMode::Survival);
    let Some(frontier) = official_msav("frontier.msav") else {
        return;
    };
    let network = crate::engine::world_stream::replace_map_from_msav(
        include_bytes!("../../dummy_world.dat"),
        &frontier,
    )
    .unwrap();
    let world = fresh_world_from_template(
        &state,
        network,
        "frontier".into(),
        std::env::temp_dir().join("sol001-hot-host.json"),
    )
    .unwrap();
    assert_eq!(
        world.base_buildings.len(),
        world.base_building_templates.len()
    );
    assert!(world.tiles.len() >= world.base_building_templates.len());
    assert!(
        world.tiles.iter().any(|tile| !tile.inventory.is_empty()),
        "map inventory missing"
    );
    assert!(
        world.tiles.iter().any(|tile| !tile.config.is_empty()),
        "map building module/subclass bytes missing"
    );
    assert!(
        world.tiles.iter().any(|tile| !tile.power_links.is_empty()),
        "map power links missing"
    );
}

fn core_at(world: &DynamicWorld, team: u8, x: i16, y: i16, block: i16) {
    let position = ((x as i32) << 16) | (y as i32 & 0xffff);
    let core = TeamCore {
        position,
        block,
        health: 1000.0,
        max_health: 1000.0,
    };
    world.team_core_lists.entry(team).or_default().push(core);
}

fn core_tile_at(world: &mut DynamicWorld, team: u8, x: i16, y: i16, block: i16) -> i32 {
    let position = ((x as i32) << 16) | (y as i32 & 0xffff);
    core_at(world, team, x, y, block);
    let mut tile = erekir_like_tile(position, block);
    tile.team = team;
    world.tiles.insert(position, tile);
    position
}

#[test]
fn best_core_foundation_beats_shard() {
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    core_at(&world, 1, 10, 10, 339); // shard size 3
    core_at(&world, 1, 50, 50, 340); // foundation size 4
    let best =
        crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 400.0, 400.0)
            .unwrap();
    assert_eq!(best, ((50i32) << 16) | 50);
}

#[test]
fn best_core_acropolis_beats_citadel() {
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    core_at(&world, 1, 10, 10, 343); // citadel size 5
    core_at(&world, 1, 80, 80, 344); // acropolis size 6
    let best = crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 0.0, 0.0)
        .unwrap();
    assert_eq!(best, ((80i32) << 16) | 80);
}

#[test]
fn best_core_prefers_larger_core_even_if_farther() {
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    core_at(&world, 1, 5, 5, 339);
    core_at(&world, 1, 100, 100, 341); // nucleus size 5
    let best = crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 6.0, 6.0)
        .unwrap();
    assert_eq!(best, ((100i32) << 16) | 100);
}

#[test]
fn best_core_uses_distance_as_tiebreaker() {
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    core_at(&world, 1, 10, 10, 340);
    core_at(&world, 1, 60, 60, 340);
    let best =
        crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 12.0, 12.0)
            .unwrap();
    assert_eq!(best, ((10i32) << 16) | 10);
}

#[test]
fn best_core_environment_preference() {
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    world.wave_rules.write().env = 16 | 1; // Erekir: scorching | terrestrial
    core_at(&world, 1, 10, 10, 342); // erekir bastion — evoke supports scorching
    core_at(&world, 1, 50, 50, 340); // serpulo foundation — beta disabled on scorching
    let best = crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 0.0, 0.0)
        .unwrap();
    assert_eq!(best, ((10i32) << 16) | 10);
}

#[test]
fn best_core_environment_filters_unsupported_core_unit() {
    // Multi-core: Java filters via unitType.supportsEnv when cores.size != 1.
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    world.wave_rules.write().env = 16 | 1; // scorching | terrestrial
                                           // Larger Serpulo nucleus (size 5) would win on size alone, but beta/gamma/alpha
                                           // disable scorching — only Erekir bastion (evoke) remains eligible.
    core_at(&world, 1, 80, 80, 341); // nucleus → gamma, envDisabled=scorching
    core_at(&world, 1, 10, 10, 342); // bastion → evoke, envDisabled=0
    let best = crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 0.0, 0.0)
        .unwrap();
    assert_eq!(
        best,
        ((10i32) << 16) | 10,
        "unsupported Serpulo core unit must be filtered when multiple cores exist"
    );
}

#[test]
fn best_core_single_core_ignores_environment_filter() {
    // Official: cores.size == 1 bypasses supportsEnv filtering.
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    world.wave_rules.write().env = 16 | 1; // scorching — Serpulo core units unsupported
    core_at(&world, 1, 40, 40, 340); // foundation → beta, envDisabled=scorching
    let best = crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 0.0, 0.0)
        .unwrap();
    assert_eq!(
        best,
        ((40i32) << 16) | 40,
        "sole core must remain selectable even when its unitType rejects the env"
    );
}

#[test]
fn best_core_equal_size_equal_distance_preserves_registration_order() {
    // Arc Seq.min(Comparator) keeps the first element on equal compare results.
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    core_at(&world, 1, 1, 10, 340); // first registered
    core_at(&world, 1, 10, 1, 340); // equal size + equal dist2 from (0,0)
    let best = crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 0.0, 0.0)
        .unwrap();
    let expected_first = ((1i32) << 16) | 10;
    assert_eq!(
        best, expected_first,
        "exact size/distance tie must keep registration order (first wins)"
    );
}

#[test]
fn building_control_select_when_possession_disabled_accepts_only_best_core() {
    let (mut world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    world.wave_rules.write().possession_allowed = false;
    let shard = core_tile_at(&mut world, 1, 10, 10, 339);
    let foundation = core_tile_at(&mut world, 1, 50, 50, 340);
    let mut player = player();
    player.x = 400.0;
    player.y = 400.0;
    assert!(building_control_select_allowed(&world, &player, foundation));
    assert!(!building_control_select_allowed(&world, &player, shard));
}

#[test]
fn building_control_select_rejects_non_best_core() {
    let (mut world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    world.wave_rules.write().possession_allowed = false;
    let near_shard = core_tile_at(&mut world, 1, 12, 12, 339);
    let far_nucleus = core_tile_at(&mut world, 1, 100, 100, 341);
    let mut player = player();
    player.x = 13.0;
    player.y = 13.0;
    assert!(!building_control_select_allowed(
        &world, &player, near_shard
    ));
    assert!(building_control_select_allowed(
        &world,
        &player,
        far_nucleus
    ));
}

#[test]
fn best_core_multi_core_regression() {
    let (mut world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    world.wave_rules.write().possession_allowed = false;
    let citadel = core_tile_at(&mut world, 1, 10, 10, 343);
    let acropolis = core_tile_at(&mut world, 1, 80, 80, 344);
    let mut player = player();
    player.x = 11.0;
    player.y = 11.0;
    let best = crate::network::wire::unit_control::best_core_position_for_team(
        &world, 1, player.x, player.y,
    )
    .unwrap();
    assert_eq!(best, acropolis);
    assert!(building_control_select_allowed(&world, &player, acropolis));
    assert!(!building_control_select_allowed(&world, &player, citadel));
    world.wave_rules.write().possession_allowed = true;
    assert!(building_control_select_allowed(&world, &player, citadel));
}

#[test]
fn building_control_select_rejects_block_unit_core_respawn() {
    let (mut world, _, _, _) = legacy_weapons_test_world();
    world.wave_rules.write().possession_allowed = true;
    let core_position = core_tile_at(&mut world, 1, 40, 40, 340);
    let mut player = player();
    player.x = 320.0;
    player.y = 320.0;
    player.controlled_unit = ControlledUnit::Building((50 << 16) | 100);
    assert!(!building_control_select_allowed(
        &world,
        &player,
        core_position
    ));
    player.controlled_unit = ControlledUnit::Core;
    assert!(building_control_select_allowed(
        &world,
        &player,
        core_position
    ));
}

#[test]
fn best_core_from_prebuilt_base_building_without_registry() {
    let (world, _, _, _) = legacy_weapons_test_world();
    world.team_core_lists.clear();
    world.cores.clear();
    let origin = ((55i32) << 16) | 55;
    world.base_buildings.insert(
        origin,
        BaseBuildingState {
            position: origin,
            block: 340,
            team: 1,
            health: 4000.0,
            occupied: vec![origin],
            inventory: Vec::new(),
        },
    );
    let best = crate::network::wire::unit_control::best_core_position_for_team(&world, 1, 0.0, 0.0)
        .unwrap();
    assert_eq!(best, origin);
}
