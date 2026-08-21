//! Parity differential probes — payload domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::ControlledUnit;
use crate::network::world::DynamicTile;
use crate::network::world::DynamicWorld;
use crate::network::world::EnemyUnit;
use crate::network::world::PlayerCombatState;
use crate::network::world::SessionPlayer;
use crate::network::world::UnitAuthority;

use serde_json::Value;

use super::{
    approx_json_f32, compare_bool, fixture, parity_bare_world, require_fields, validate_common,
};

const PAYLOAD_SCENARIOS: &[&str] = &[
    "build_in_range",
    "build_out_of_range",
    "build_hidden",
    "build_can_pickup_false",
    "build_enemy_team",
    "build_internal_payload",
    "build_exact_capacity",
    "build_over_capacity",
    "unit_ai_grounded",
    "unit_player_controller",
    "unit_flying",
    "unit_out_of_range",
    "drop_within_four",
    "drop_clamp_far",
    "drop_blocked",
    "drop_unit_payload",
    "drop_build_payload",
    "power_pickup_drop",
    "race_two_build",
    "drop_dead_player",
];

#[derive(Debug, Clone, serde::Serialize)]
struct PayloadScenarioOut {
    success: bool,
    carrier_payload_count: usize,
    carrier_payload_types: Vec<String>,
    origin_building_exists: bool,
    target_unit_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    drop_x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drop_y: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drop_tile_x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drop_tile_y: Option<i32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    footprint: Vec<[i32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_links_after_pickup: Option<Vec<[i32; 2]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_links_after_drop: Option<Vec<[i32; 2]>>,
    world_buildings: usize,
    world_units: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    second_success: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    second_payload_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loader_still_exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_origin_exists: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dropped_unit_exists: Option<bool>,
}

fn parity_payload_world() -> DynamicWorld {
    let mut world = parity_bare_world("parity-payload-rpc.json");
    world.width = 80;
    world.height = 80;
    world.base_blocks = vec![0i16; (world.width * world.height) as usize];
    world.base_centers = vec![false; (world.width * world.height) as usize];
    world
}

fn payload_pos(x: i32, y: i32) -> i32 {
    (x << 16) | (y & 0xffff)
}

fn payload_tile_xy(position: i32) -> (i32, i32) {
    ((position >> 16) as i16 as i32, position as i16 as i32)
}

fn payload_insert_building(world: &DynamicWorld, x: i32, y: i32, block: i16, team: u8) -> i32 {
    use crate::game::content::block_size;
    use crate::network::buildings::placement::after_placement;
    let origin = payload_pos(x, y);
    let size = i32::from(block_size(block));
    let mut occupied = Vec::new();
    for dx in 0..size {
        for dy in 0..size {
            occupied.push(payload_pos(x + dx, y + dy));
        }
    }
    let tile = DynamicTile {
        position: origin,
        block,
        team,
        occupied: occupied.clone(),
        health: 1.0,
        ..DynamicTile::default()
    };
    world.tiles.insert(origin, tile);
    for cell in occupied {
        world.tile_footprint.insert(cell, origin);
    }
    after_placement(world, origin, &[]);
    origin
}

fn payload_mega(id: i32, x: f32, y: f32) -> EnemyUnit {
    EnemyUnit {
        id,
        unit_type: 22,
        entity_class: 0,
        team: 1,
        x,
        y,
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
        authority: UnitAuthority::Player { player_id: 1 },
        build_plans: Vec::new(),
        update_building: true,
        status_agg: Default::default(),
    }
}

fn payload_dagger(id: i32, x: f32, y: f32, elevation: f32, authority: UnitAuthority) -> EnemyUnit {
    let mut unit = payload_mega(id, x, y);
    unit.unit_type = 0;
    unit.elevation = elevation;
    unit.authority = authority;
    unit
}

fn payload_player_for(carrier_id: i32) -> SessionPlayer {
    SessionPlayer {
        id: 1,
        controlled_unit: ControlledUnit::Standard(carrier_id),
        unit_id: 2_500_001,
        uuid: "payload-parity".into(),
        name: "payload".into(),
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
        active_plans: std::collections::HashSet::new(),
        mining_position: None,
        mining_progress: 0.0,
        mining_updated: std::time::Instant::now(),
        carried_item: -1,
        carried_amount: 0,
        preview_plan_group: -1,
        preview_plans: Vec::new(),
        last_shot: std::time::Instant::now() - std::time::Duration::from_secs(1),
        admin: false,
        chat_rate: crate::network::wire::ChatRateLimiter::new(),
    }
}

fn payload_give_wall(world: &DynamicWorld, carrier_id: i32, block: i16, team: u8) {
    use crate::network::world::{CarriedBuildPayload, CarriedPayload};
    if let Some(mut live) = world.enemies.get_mut(&carrier_id) {
        live.payloads
            .push(CarriedPayload::Build(CarriedBuildPayload {
                version: 0,
                tile: DynamicTile {
                    block,
                    team,
                    health: 1.0,
                    ..DynamicTile::default()
                },
                sync: Vec::new(),
            }));
    }
}

fn payload_types(unit: &EnemyUnit) -> Vec<String> {
    use crate::network::world::CarriedPayload;
    unit.payloads
        .iter()
        .map(|payload| match payload {
            CarriedPayload::Unit(_) => "unit".to_string(),
            CarriedPayload::Build(_) => "build".to_string(),
        })
        .collect()
}

fn payload_origin_exists(world: &DynamicWorld, origin: i32) -> bool {
    world.tiles.contains_key(&origin)
}

fn payload_footprint_at(
    world: &DynamicWorld,
    tile_x: i32,
    tile_y: i32,
    block: i16,
) -> Vec<[i32; 2]> {
    use crate::game::content::block_size;
    let size = i32::from(block_size(block));
    let mut out = Vec::new();
    for dx in 0..size {
        for dy in 0..size {
            out.push([tile_x + dx, tile_y + dy]);
        }
    }
    out.sort_unstable();
    let placed = world
        .tiles
        .iter()
        .find(|entry| payload_tile_xy(entry.position) == (tile_x, tile_y));
    if let Some(tile) = placed {
        return tile
            .occupied
            .iter()
            .map(|cell| {
                let (x, y) = payload_tile_xy(*cell);
                [x, y]
            })
            .collect();
    }
    out
}

fn payload_node_links(world: &DynamicWorld, node_x: i32, node_y: i32) -> Vec<[i32; 2]> {
    let origin = payload_pos(node_x, node_y);
    let mut out = world
        .tiles
        .get(&origin)
        .map(|tile| {
            tile.power_links
                .iter()
                .filter_map(|link| {
                    world
                        .tiles
                        .get(link)
                        .map(|linked| payload_tile_xy(linked.position))
                })
                .map(|(x, y)| [x, y])
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    out.sort_unstable();
    out
}

fn payload_request_build(world: &DynamicWorld, player: &SessionPlayer, origin: i32) -> bool {
    use crate::network::economy::payload::apply_request_build_payload;
    let before = world
        .enemies
        .get(&player.controlled_unit.standard_id().unwrap())
        .map(|unit| unit.payloads.len())
        .unwrap_or(0);
    let frames = apply_request_build_payload(world, player, origin);
    let after = world
        .enemies
        .get(&player.controlled_unit.standard_id().unwrap())
        .map(|unit| unit.payloads.len())
        .unwrap_or(0);
    !frames.is_empty() || after > before
}

fn payload_request_unit(world: &DynamicWorld, player: &SessionPlayer, target_id: i32) -> bool {
    use crate::network::economy::payload::apply_request_unit_payload;
    let carrier_id = player.controlled_unit.standard_id().unwrap();
    let before = world
        .enemies
        .get(&carrier_id)
        .map(|u| u.payloads.len())
        .unwrap_or(0);
    let frames = apply_request_unit_payload(world, player, target_id);
    let after = world
        .enemies
        .get(&carrier_id)
        .map(|u| u.payloads.len())
        .unwrap_or(0);
    !frames.is_empty() || after > before
}

fn payload_request_drop(world: &DynamicWorld, player: &SessionPlayer, x: f32, y: f32) -> bool {
    use crate::network::economy::payload::apply_request_drop_payload;
    let carrier_id = player.controlled_unit.standard_id().unwrap();
    let before = world
        .enemies
        .get(&carrier_id)
        .map(|u| u.payloads.len())
        .unwrap_or(0);
    let frames = apply_request_drop_payload(world, player, x, y);
    let after = world
        .enemies
        .get(&carrier_id)
        .map(|u| u.payloads.len())
        .unwrap_or(0);
    !frames.is_empty() || after < before
}

fn run_payload_scenario(name: &str) -> PayloadScenarioOut {
    use crate::network::buildings::placement::after_placement;
    use crate::network::world::{CarriedBuildPayload, CarriedPayload};

    let world = parity_payload_world();
    let mut out = PayloadScenarioOut {
        success: false,
        carrier_payload_count: 0,
        carrier_payload_types: Vec::new(),
        origin_building_exists: false,
        target_unit_exists: false,
        drop_x: None,
        drop_y: None,
        drop_tile_x: None,
        drop_tile_y: None,
        footprint: Vec::new(),
        node_links_after_pickup: None,
        node_links_after_drop: None,
        world_buildings: 0,
        world_units: 0,
        second_success: None,
        second_payload_count: None,
        loader_still_exists: None,
        extra_origin_exists: None,
        dropped_unit_exists: None,
    };

    match name {
        "build_in_range" => {
            let origin = payload_insert_building(&world, 5, 5, 218, 1);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            let player = payload_player_for(10);
            out.success = payload_request_build(&world, &player, origin);
        }
        "build_out_of_range" => {
            let origin = payload_insert_building(&world, 60, 60, 218, 1);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            out.success = payload_request_build(&world, &payload_player_for(10), origin);
            out.origin_building_exists = payload_origin_exists(&world, origin);
        }
        "build_hidden" => {
            // BuildVisibility.hidden: no Building handle exists to pass to the RPC.
            let origin = payload_pos(5, 5);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            out.success = payload_request_build(&world, &payload_player_for(10), origin);
            out.origin_building_exists = payload_origin_exists(&world, origin);
        }
        "build_can_pickup_false" => {
            let origin = payload_insert_building(&world, 5, 5, 339, 1);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            out.success = payload_request_build(&world, &payload_player_for(10), origin);
            out.origin_building_exists = payload_origin_exists(&world, origin);
        }
        "build_enemy_team" => {
            let origin = payload_insert_building(&world, 5, 5, 218, 2);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            out.success = payload_request_build(&world, &payload_player_for(10), origin);
            out.origin_building_exists = payload_origin_exists(&world, origin);
        }
        "build_internal_payload" => {
            let origin = payload_insert_building(&world, 5, 5, 408, 1);
            if let Some(mut tile) = world.tiles.get_mut(&origin) {
                tile.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
                    version: 0,
                    tile: DynamicTile {
                        block: 218,
                        team: 1,
                        health: 1.0,
                        ..DynamicTile::default()
                    },
                    sync: Vec::new(),
                })));
            }
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            out.success = payload_request_build(&world, &payload_player_for(10), origin);
            out.origin_building_exists = payload_origin_exists(&world, origin);
            out.loader_still_exists = Some(true);
        }
        "build_exact_capacity" => {
            let origin = payload_insert_building(&world, 5, 5, 219, 1);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            out.success = payload_request_build(&world, &payload_player_for(10), origin);
        }
        "build_over_capacity" => {
            let large = payload_insert_building(&world, 5, 5, 219, 1);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            payload_request_build(&world, &payload_player_for(10), large);
            let extra = payload_insert_building(&world, 8, 5, 218, 1);
            out.success = payload_request_build(&world, &payload_player_for(10), extra);
            out.origin_building_exists = payload_origin_exists(&world, extra);
            out.extra_origin_exists = Some(true);
            out.loader_still_exists = Some(false);
        }
        "unit_ai_grounded" => {
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            world.enemies.insert(
                20,
                payload_dagger(20, 40.0, 40.0, 0.0, UnitAuthority::DefaultAi),
            );
            world.register_unit_group(20);
            out.success = payload_request_unit(&world, &payload_player_for(10), 20);
            out.target_unit_exists = world.enemies.contains_key(&20);
        }
        "unit_player_controller" => {
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            world.enemies.insert(
                20,
                payload_dagger(20, 40.0, 40.0, 0.0, UnitAuthority::Player { player_id: 2 }),
            );
            world.register_unit_group(20);
            out.success = payload_request_unit(&world, &payload_player_for(10), 20);
            out.target_unit_exists = world.enemies.contains_key(&20);
        }
        "unit_flying" => {
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            world.enemies.insert(
                21,
                payload_dagger(21, 40.0, 40.0, 1.0, UnitAuthority::DefaultAi),
            );
            world.register_unit_group(21);
            out.success = payload_request_unit(&world, &payload_player_for(10), 21);
            out.target_unit_exists = world.enemies.contains_key(&21);
        }
        "unit_out_of_range" => {
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            world.enemies.insert(
                21,
                payload_dagger(21, 120.0, 40.0, 0.0, UnitAuthority::DefaultAi),
            );
            world.register_unit_group(21);
            out.success = payload_request_unit(&world, &payload_player_for(10), 21);
            out.target_unit_exists = world.enemies.contains_key(&21);
        }
        "drop_within_four" => {
            world.enemies.insert(10, payload_mega(10, 80.0, 80.0));
            payload_give_wall(&world, 10, 218, 1);
            out.success = payload_request_drop(&world, &payload_player_for(10), 88.0, 80.0);
            out.drop_x = Some(88.0);
            out.drop_y = Some(80.0);
            out.drop_tile_x = Some(11);
            out.drop_tile_y = Some(10);
            out.footprint = payload_footprint_at(&world, 11, 10, 218);
        }
        "drop_clamp_far" => {
            world.enemies.insert(10, payload_mega(10, 80.0, 80.0));
            payload_give_wall(&world, 10, 218, 1);
            out.success = payload_request_drop(&world, &payload_player_for(10), 240.0, 80.0);
            out.drop_x = Some(112.0);
            out.drop_y = Some(80.0);
            out.drop_tile_x = Some(14);
            out.drop_tile_y = Some(10);
            out.footprint = payload_footprint_at(&world, 14, 10, 218);
        }
        "drop_blocked" => {
            payload_insert_building(&world, 10, 10, 218, 1);
            world.enemies.insert(12, payload_mega(12, 80.0, 80.0));
            payload_give_wall(&world, 12, 218, 1);
            out.success = payload_request_drop(&world, &payload_player_for(12), 80.0, 80.0);
        }
        "drop_unit_payload" => {
            world.enemies.insert(10, payload_mega(10, 80.0, 80.0));
            let dagger = payload_dagger(30, 0.0, 0.0, 0.0, UnitAuthority::DefaultAi);
            world.enemies.insert(30, dagger.clone());
            world.register_unit_group(30);
            if let Some(mut carrier) = world.enemies.get_mut(&10) {
                carrier.payloads.push(CarriedPayload::Unit(dagger));
            }
            world.enemies.remove(&30);
            out.success = payload_request_drop(&world, &payload_player_for(10), 88.0, 80.0);
            out.dropped_unit_exists = Some(
                world
                    .enemies
                    .iter()
                    .any(|entry| entry.value().unit_type == 0 && *entry.key() != 30),
            );
        }
        "drop_build_payload" => {
            world.enemies.insert(10, payload_mega(10, 80.0, 80.0));
            payload_give_wall(&world, 10, 218, 1);
            out.success = payload_request_drop(&world, &payload_player_for(10), 80.0, 80.0);
            out.drop_x = Some(80.0);
            out.drop_y = Some(80.0);
            out.drop_tile_x = Some(10);
            out.drop_tile_y = Some(10);
            out.footprint = payload_footprint_at(&world, 10, 10, 218);
        }
        "power_pickup_drop" => {
            payload_insert_building(&world, 5, 5, 302, 1);
            let battery = payload_insert_building(&world, 8, 5, 306, 1);
            after_placement(&world, battery, &[]);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            let picked = payload_request_build(&world, &payload_player_for(10), battery);
            out.node_links_after_pickup = Some(payload_node_links(&world, 5, 5));
            if let Some(mut live) = world.enemies.get_mut(&10) {
                live.x = 80.0;
                live.y = 80.0;
            }
            let dropped = payload_request_drop(&world, &payload_player_for(10), 80.0, 80.0);
            out.success = picked && dropped;
            out.drop_x = Some(80.0);
            out.drop_y = Some(80.0);
            out.drop_tile_x = Some(10);
            out.drop_tile_y = Some(10);
            out.node_links_after_drop = Some(payload_node_links(&world, 5, 5));
        }
        "race_two_build" => {
            let origin = payload_insert_building(&world, 5, 5, 218, 1);
            world.enemies.insert(10, payload_mega(10, 40.0, 40.0));
            world.enemies.insert(11, payload_mega(11, 40.0, 40.0));
            let first = payload_request_build(&world, &payload_player_for(10), origin);
            let second = if payload_origin_exists(&world, origin) {
                payload_request_build(&world, &payload_player_for(11), origin)
            } else {
                false
            };
            out.success = first;
            out.second_success = Some(second);
            out.second_payload_count = Some(
                world
                    .enemies
                    .get(&11)
                    .map(|unit| unit.payloads.len())
                    .unwrap_or(0),
            );
        }
        "drop_dead_player" => {
            world.enemies.insert(10, payload_mega(10, 80.0, 80.0));
            payload_give_wall(&world, 10, 218, 1);
            let player = payload_player_for(10);
            world.players.insert(
                player.unit_id,
                PlayerCombatState {
                    uuid: player.uuid.clone(),
                    player_id: player.id,
                    unit_id: player.unit_id,
                    x: 80.0,
                    y: 80.0,
                    health: 0.0,
                    shield: 0.0,
                    status_effect: -1,
                    status_duration: 0.0,
                    statuses: Vec::new(),
                    dead: true,
                    respawn_timer: 0.0,
                    team: 1,
                },
            );
            out.success = payload_request_drop(&world, &player, 80.0, 80.0);
        }
        other => panic!("unknown payload scenario {other}"),
    }

    if let Some(carrier_id) = match name {
        "drop_blocked" => Some(12),
        _ => Some(10),
    } {
        if let Some(unit) = world.enemies.get(&carrier_id) {
            out.carrier_payload_count = unit.payloads.len();
            out.carrier_payload_types = payload_types(&unit);
        }
    }

    out.world_buildings = world.tiles.len();
    out.world_units = world.enemies.len();
    out
}

fn compare_payload_scenario(
    probe: &str,
    name: &str,
    expected: &Value,
    actual: &PayloadScenarioOut,
) -> Result<(), String> {
    let prefix = format!("scenarios.{name}");
    compare_bool(
        expected,
        probe,
        &format!("{prefix}.success"),
        actual.success,
        expected["success"].as_bool().unwrap_or(false),
    )?;
    if actual.carrier_payload_count
        != expected["carrier_payload_count"].as_u64().unwrap_or(0) as usize
    {
        return Err(format!(
            "parity mismatch: field '{prefix}.carrier_payload_count': java 158.1 = {}, rust = {}",
            expected["carrier_payload_count"], actual.carrier_payload_count
        ));
    }
    let expected_types: Vec<String> = expected["carrier_payload_types"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if actual.carrier_payload_types != expected_types {
        return Err(format!(
            "parity mismatch: field '{prefix}.carrier_payload_types': java 158.1 = {expected_types:?}, rust = {:?}",
            actual.carrier_payload_types
        ));
    }
    compare_bool(
        expected,
        probe,
        &format!("{prefix}.origin_building_exists"),
        actual.origin_building_exists,
        expected["origin_building_exists"]
            .as_bool()
            .unwrap_or(false),
    )?;
    compare_bool(
        expected,
        probe,
        &format!("{prefix}.target_unit_exists"),
        actual.target_unit_exists,
        expected["target_unit_exists"].as_bool().unwrap_or(false),
    )?;

    if name != "drop_unit_payload" {
        if let (Some(ex), Some(ey)) = (expected.get("drop_x"), expected.get("drop_y")) {
            let ax = actual
                .drop_x
                .ok_or_else(|| format!("parity mismatch: field '{prefix}.drop_x': rust missing"))?;
            let ay = actual
                .drop_y
                .ok_or_else(|| format!("parity mismatch: field '{prefix}.drop_y': rust missing"))?;
            if !approx_json_f32(ex, ax) || !approx_json_f32(ey, ay) {
                return Err(format!(
                    "parity mismatch: field '{prefix}.drop': java 158.1 = ({}, {}), rust = ({ax}, {ay})",
                    ex, ey
                ));
            }
        }
    }
    if let (Some(ex), Some(ey)) = (expected.get("drop_tile_x"), expected.get("drop_tile_y")) {
        if actual.drop_tile_x != Some(ex.as_i64().unwrap_or(-1) as i32)
            || actual.drop_tile_y != Some(ey.as_i64().unwrap_or(-1) as i32)
        {
            return Err(format!(
                "parity mismatch: field '{prefix}.drop_tile': java 158.1 = ({}, {}), rust = ({:?}, {:?})",
                ex, ey, actual.drop_tile_x, actual.drop_tile_y
            ));
        }
    }
    if let Some(expected_fp) = expected.get("footprint").and_then(Value::as_array) {
        let rust_fp: Vec<[i32; 2]> = actual.footprint.clone();
        if expected_fp.len() != rust_fp.len() {
            return Err(format!(
                "parity mismatch: field '{prefix}.footprint' length: java = {}, rust = {}",
                expected_fp.len(),
                rust_fp.len()
            ));
        }
        for (index, cell) in expected_fp.iter().enumerate() {
            let xs = cell.as_array().expect("footprint cell");
            let rx = rust_fp[index][0];
            let ry = rust_fp[index][1];
            if rx != xs[0].as_i64().unwrap_or(-1) as i32
                || ry != xs[1].as_i64().unwrap_or(-1) as i32
            {
                return Err(format!(
                    "parity mismatch: field '{prefix}.footprint[{index}]': java = {cell}, rust = [{rx}, {ry}]"
                ));
            }
        }
    }
    for phase in ["node_links_after_pickup", "node_links_after_drop"] {
        if let Some(expected_links) = expected.get(phase).and_then(Value::as_array) {
            let rust_links = match phase {
                "node_links_after_pickup" => actual.node_links_after_pickup.as_ref(),
                _ => actual.node_links_after_drop.as_ref(),
            }
            .cloned()
            .unwrap_or_default();
            if expected_links.len() != rust_links.len() {
                return Err(format!(
                    "parity mismatch: field '{prefix}.{phase}' length: java = {}, rust = {}",
                    expected_links.len(),
                    rust_links.len()
                ));
            }
        }
    }
    if let Some(second) = expected.get("second_success").and_then(Value::as_bool) {
        compare_bool(
            expected,
            probe,
            &format!("{prefix}.second_success"),
            actual.second_success.unwrap_or(false),
            second,
        )?;
    }
    if let Some(second_count) = expected.get("second_payload_count").and_then(Value::as_u64) {
        if actual.second_payload_count.unwrap_or(0) != second_count as usize {
            return Err(format!(
                "parity mismatch: field '{prefix}.second_payload_count': java 158.1 = {second_count}, rust = {:?}",
                actual.second_payload_count
            ));
        }
    }
    if let Some(loader) = expected.get("loader_still_exists").and_then(Value::as_bool) {
        compare_bool(
            expected,
            probe,
            &format!("{prefix}.loader_still_exists"),
            actual.loader_still_exists.unwrap_or(false),
            loader,
        )?;
    }
    if expected.get("extra_origin_exists").and_then(Value::as_bool) == Some(true) {
        compare_bool(
            expected,
            probe,
            &format!("{prefix}.extra_origin_exists"),
            actual.extra_origin_exists.unwrap_or(false),
            true,
        )?;
    }
    if let Some(dropped) = expected.get("dropped_unit_exists").and_then(Value::as_bool) {
        compare_bool(
            expected,
            probe,
            &format!("{prefix}.dropped_unit_exists"),
            actual.dropped_unit_exists.unwrap_or(false),
            dropped,
        )?;
    }
    Ok(())
}

fn compare_payload_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(fixture, &probe, &["world_size", "scenarios"])?;
    let scenarios = fixture
        .get("scenarios")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!("parity error: fixture '{probe}' field 'scenarios' must be an object")
        })?;
    for name in PAYLOAD_SCENARIOS {
        let expected = scenarios.get(*name).ok_or_else(|| {
            format!("parity error: fixture '{probe}' is missing scenario '{name}'")
        })?;
        let actual = run_payload_scenario(name);
        compare_payload_scenario(&probe, name, expected, &actual)?;
    }
    Ok(())
}

#[test]
fn payload_rpc_matches_java_1581() {
    compare_payload_fixture(&fixture("payload-rpc.json")).unwrap_or_else(|error| panic!("{error}"));
}
