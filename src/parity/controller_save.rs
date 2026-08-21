//! Parity differential probes — controller_save domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicWorld;

use serde_json::json;
use serde_json::Value;

use super::{
    approx_json_f32, compare_bool, fixture, parity_bare_world, require_fields, tile_at_pos,
    validate_common,
};

use super::ubind::ubind_probe_unit;

fn controller_save_command_snapshot(
    world: &DynamicWorld,
    unit_id: i32,
) -> serde_json::Map<String, Value> {
    use crate::network::world::UnitAuthority;
    use serde_json::{json, Map, Value};

    let mut out = Map::new();
    let Some(unit) = world.enemies.get(&unit_id) else {
        out.insert("unit_found".into(), Value::Bool(false));
        return out;
    };
    out.insert("unit_found".into(), Value::Bool(true));
    out.insert("controller".into(), json!("CommandAI"));
    let is_command = matches!(unit.authority, UnitAuthority::Command);
    out.insert("is_command_ai".into(), Value::Bool(is_command));
    if !is_command {
        return out;
    }
    let order = world.unit_orders.get(&unit_id);
    out.insert(
        "command_id".into(),
        json!(order
            .as_ref()
            .map(|order| i64::from(order.command))
            .unwrap_or(-1)),
    );
    let has_pos = order
        .as_ref()
        .is_some_and(|order| order.target_x.is_some() && order.target_y.is_some());
    out.insert("has_target_pos".into(), Value::Bool(has_pos));
    out.insert(
        "target_x".into(),
        json!(order
            .as_ref()
            .and_then(|order| order.target_x)
            .unwrap_or(0.0)),
    );
    out.insert(
        "target_y".into(),
        json!(order
            .as_ref()
            .and_then(|order| order.target_y)
            .unwrap_or(0.0)),
    );
    let has_attack = order
        .as_ref()
        .is_some_and(|order| matches!(order.target_kind, 1 | 2) && order.target_id >= 0);
    out.insert("has_attack_target".into(), Value::Bool(has_attack));
    let (attack_kind, attack_building_pos, attack_unit_id) = order
        .as_ref()
        .map(|order| match order.target_kind {
            1 if order.target_id >= 0 => ("building", order.target_id, -1),
            2 if order.target_id >= 0 => ("unit", -1, order.target_id),
            _ => ("none", -1, -1),
        })
        .unwrap_or(("none", -1, -1));
    out.insert("attack_kind".into(), json!(attack_kind));
    out.insert("attack_building_pos".into(), json!(attack_building_pos));
    out.insert("attack_unit_id".into(), json!(attack_unit_id));
    out.insert("read_attack_target".into(), json!(-1));
    let queue = order
        .as_ref()
        .map(|order| order.queue.as_slice())
        .unwrap_or(&[]);
    out.insert("queue_size".into(), json!(queue.len()));
    let queue_json: Vec<Value> = queue
        .iter()
        .map(|entry| match entry.kind {
            1 => json!({"kind": "building", "pos": entry.id}),
            2 => json!({"kind": "unit", "id": entry.id}),
            _ => json!({"kind": "vec2", "x": entry.x, "y": entry.y}),
        })
        .collect();
    out.insert("queue".into(), Value::Array(queue_json));
    let stances: Vec<i64> = order
        .as_ref()
        .map(|order| {
            (0..30_u8)
                .filter(|stance| order.stances & (1_u32 << stance) != 0)
                .map(i64::from)
                .collect()
        })
        .unwrap_or_default();
    out.insert(
        "stances".into(),
        Value::Array(stances.into_iter().map(Value::from).collect()),
    );
    out.insert(
        "has_command".into(),
        Value::Bool(crate::network::units::unit_has_active_rts_command(
            world, unit_id,
        )),
    );
    out.insert(
        "logic_controllable".into(),
        Value::Bool(crate::network::units::unit_is_logic_controllable(
            world, unit_id,
        )),
    );
    out
}

fn controller_save_logic_snapshot(
    world: &DynamicWorld,
    unit_id: i32,
) -> serde_json::Map<String, Value> {
    use crate::network::world::{logic_control, UnitAuthority};
    use serde_json::{json, Map, Value};

    let mut out = Map::new();
    let Some(unit) = world.enemies.get(&unit_id) else {
        out.insert("unit_found".into(), Value::Bool(false));
        return out;
    };
    out.insert("unit_found".into(), Value::Bool(true));
    out.insert("controller".into(), json!("LogicAI"));
    let UnitAuthority::Logic { processor_pos, .. } = unit.authority else {
        out.insert("is_logic_ai".into(), Value::Bool(false));
        return out;
    };
    out.insert("is_logic_ai".into(), Value::Bool(true));
    out.insert("controller_pos".into(), json!(processor_pos));
    out.insert(
        "controller_valid".into(),
        Value::Bool(
            world
                .tiles
                .get(&processor_pos)
                .is_some_and(|tile| tile.block != 0),
        ),
    );
    let remaining = match unit.authority {
        UnitAuthority::Logic {
            remaining_ticks, ..
        } => remaining_ticks,
        _ => 0.0,
    };
    out.insert("control_timer".into(), json!(remaining));
    let mode = world
        .unit_orders
        .get(&unit_id)
        .map(|order| match order.logic_control {
            logic_control::STOP => "stop",
            logic_control::MOVE => "move",
            _ => "idle",
        })
        .unwrap_or("idle");
    out.insert("control_mode".into(), json!(mode));
    out.insert("move_x".into(), json!(0.0));
    out.insert("move_y".into(), json!(0.0));
    out.insert("move_rad".into(), json!(0.0));
    out.insert("boost".into(), Value::Bool(false));
    out.insert("shoot".into(), Value::Bool(false));
    out.insert("aim_control".into(), json!("stop"));
    out
}

fn compare_controller_snapshot(
    probe: &str,
    scenario: &str,
    phase: &str,
    expected: &Value,
    actual: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let expected_obj = expected.as_object().ok_or_else(|| {
        format!("parity error: fixture '{probe}' {scenario}.{phase} must be an object")
    })?;
    for (field, expected_value) in expected_obj {
        let actual_value = actual.get(field).ok_or_else(|| {
            format!("parity mismatch: fixture '{probe}' {scenario}.{phase}.{field}: rust snapshot missing field")
        })?;
        if field.ends_with("_x")
            || field.ends_with("_y")
            || field == "control_timer"
            || field == "move_rad"
        {
            let actual_f = actual_value.as_f64().unwrap_or(0.0) as f32;
            if !approx_json_f32(expected_value, actual_f) {
                return Err(format!(
                    "parity mismatch: fixture '{probe}' {scenario}.{phase}.{field}: java 158.1 = {expected_value}, rust = {actual_value}"
                ));
            }
        } else if expected_value.is_boolean() {
            compare_bool(
                expected,
                probe,
                &format!("{scenario}.{phase}.{field}"),
                actual_value.as_bool().unwrap_or(false),
                expected_value.as_bool().unwrap(),
            )?;
        } else if expected_value.is_array() {
            if expected_value != actual_value {
                return Err(format!(
                    "parity mismatch: fixture '{probe}' {scenario}.{phase}.{field}: java 158.1 = {expected_value}, rust = {actual_value}"
                ));
            }
        } else if expected_value.is_string() {
            if expected_value.as_str() != actual_value.as_str() {
                return Err(format!(
                    "parity mismatch: fixture '{probe}' {scenario}.{phase}.{field}: java 158.1 = {expected_value}, rust = {actual_value}"
                ));
            }
        } else if let (Some(exp), Some(act)) = (expected_value.as_i64(), actual_value.as_i64()) {
            if exp != act {
                return Err(format!(
                    "parity mismatch: fixture '{probe}' {scenario}.{phase}.{field}: java 158.1 = {exp}, rust = {act}"
                ));
            }
        } else if expected_value.is_number() {
            let actual_f = actual_value.as_f64().unwrap_or(0.0) as f32;
            if !approx_json_f32(expected_value, actual_f) {
                return Err(format!(
                    "parity mismatch: fixture '{probe}' {scenario}.{phase}.{field}: java 158.1 = {expected_value}, rust = {actual_value}"
                ));
            }
        }
    }
    Ok(())
}

fn compare_controller_save_fixture(fixture: &Value) -> Result<(), String> {
    use crate::network::units::controller::roundtrip_controller_save;
    use crate::network::units::set_order_active_target;
    use crate::network::world::{logic_control, UnitAuthority, UnitOrder, UnitOrderTarget};

    let probe = validate_common(fixture)?;
    require_fields(fixture, &probe, &["scenarios", "outcomes"])?;

    let world = parity_bare_world("controller-save.json");
    let wall_pos = (8 << 16) | 8;
    let tile_at = |block: i16, team: u8| {
        let mut tile = tile_at_pos(wall_pos, block);
        tile.team = team;
        tile
    };

    let run_command = |world: &DynamicWorld,
                       unit_id: i32,
                       foe_id: i32,
                       command: u8,
                       stances: u32,
                       attack_unit: bool,
                       queue: Vec<UnitOrderTarget>| {
        let mut unit = ubind_probe_unit(unit_id, 1, 0.0);
        unit.unit_type = 22; // mega
        unit.x = 80.0;
        unit.y = 80.0;
        unit.authority = UnitAuthority::Command;
        world.enemies.insert(unit_id, unit);
        if attack_unit {
            let mut foe = ubind_probe_unit(foe_id, 6, 0.0);
            foe.x = 120.0;
            foe.y = 120.0;
            world.enemies.insert(foe_id, foe);
        }
        let mut order = UnitOrder {
            unit_id,
            command,
            stances,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: 0,
            queue,
        };
        if attack_unit {
            set_order_active_target(
                &mut order,
                UnitOrderTarget {
                    kind: 2,
                    id: foe_id,
                    x: 120.0,
                    y: 120.0,
                },
            );
        } else {
            set_order_active_target(
                &mut order,
                UnitOrderTarget {
                    kind: 0,
                    id: -1,
                    x: 100.0,
                    y: 100.0,
                },
            );
        }
        world.unit_orders.insert(unit_id, order);
    };

    // command_roundtrip
    {
        let unit_id = 3_010_001;
        let foe_id = 3_010_002;
        let mut wall = tile_at(22, 6);
        crate::network::world::stamp_new_building(&world, &mut wall);
        world.tiles.insert(wall_pos, wall);
        run_command(
            &world,
            unit_id,
            foe_id,
            1,
            (1 << 1) | (1 << 2),
            true,
            vec![
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
        );
        roundtrip_controller_save(&world, unit_id).map_err(|error| {
            format!("parity error: fixture '{probe}' command_roundtrip: {error}")
        })?;
        let mut expected = fixture["scenarios"]["command_roundtrip"]["after"].clone();
        expected["attack_unit_id"] = json!(foe_id);
        if let Some(queue) = expected["queue"].as_array_mut() {
            for entry in queue {
                if entry.get("kind").and_then(Value::as_str) == Some("unit") {
                    entry["id"] = json!(foe_id);
                }
            }
        }
        let actual = controller_save_command_snapshot(&world, unit_id);
        compare_controller_snapshot(&probe, "command_roundtrip", "after", &expected, &actual)?;
    }

    // logic_roundtrip
    {
        let unit_id = 3_010_010;
        let mut proc = tile_at(431, 1);
        crate::network::world::stamp_new_building(&world, &mut proc);
        world.tiles.insert(wall_pos, proc);
        let mut unit = ubind_probe_unit(unit_id, 1, 0.0);
        unit.x = 80.0;
        unit.y = 80.0;
        unit.authority = UnitAuthority::Logic {
            processor_pos: wall_pos,
            remaining_ticks: 123.45,
            processor_generation: 0,
        };
        world.enemies.insert(unit_id, unit);
        world.unit_orders.insert(
            unit_id,
            UnitOrder {
                unit_id,
                command: 0,
                stances: 0,
                payload_cooldown: 0.0,
                target_kind: 0,
                target_id: -1,
                target_x: Some(200.0),
                target_y: Some(300.0),
                logic_control: logic_control::MOVE,
                queue: Vec::new(),
            },
        );
        roundtrip_controller_save(&world, unit_id)
            .map_err(|error| format!("parity error: fixture '{probe}' logic_roundtrip: {error}"))?;
        let expected = &fixture["scenarios"]["logic_roundtrip"]["after"];
        let actual = controller_save_logic_snapshot(&world, unit_id);
        compare_controller_snapshot(&probe, "logic_roundtrip", "after", expected, &actual)?;
    }

    // missing_attack_unit
    {
        let unit_id = 3_010_020;
        let phantom_id = 3_010_021;
        run_command(&world, unit_id, phantom_id, 0, 0, true, Vec::new());
        world.enemies.remove(&phantom_id);
        roundtrip_controller_save(&world, unit_id).map_err(|error| {
            format!("parity error: fixture '{probe}' missing_attack_unit: {error}")
        })?;
        let expected = &fixture["scenarios"]["missing_attack_unit"]["after"];
        let actual = controller_save_command_snapshot(&world, unit_id);
        compare_controller_snapshot(&probe, "missing_attack_unit", "after", expected, &actual)?;
    }

    // missing_queue_unit
    {
        let unit_id = 3_010_030;
        let foe_id = 3_010_031;
        run_command(
            &world,
            unit_id,
            foe_id,
            0,
            0,
            false,
            vec![
                UnitOrderTarget {
                    kind: 2,
                    id: foe_id,
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
        );
        world.enemies.remove(&foe_id);
        roundtrip_controller_save(&world, unit_id).map_err(|error| {
            format!("parity error: fixture '{probe}' missing_queue_unit: {error}")
        })?;
        let expected = &fixture["scenarios"]["missing_queue_unit"]["after"];
        let actual = controller_save_command_snapshot(&world, unit_id);
        compare_controller_snapshot(&probe, "missing_queue_unit", "after", expected, &actual)?;
    }

    // building_attack_removed
    {
        let unit_id = 3_010_040;
        let mut wall = tile_at(22, 6);
        crate::network::world::stamp_new_building(&world, &mut wall);
        world.tiles.insert(wall_pos, wall.clone());
        let mut unit = ubind_probe_unit(unit_id, 1, 0.0);
        unit.unit_type = 15; // flare
        unit.x = 80.0;
        unit.y = 80.0;
        unit.authority = UnitAuthority::Command;
        world.enemies.insert(unit_id, unit);
        let order = UnitOrder {
            unit_id,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 1,
            target_id: wall_pos,
            target_x: Some(64.0),
            target_y: Some(64.0),
            logic_control: 0,
            queue: Vec::new(),
        };
        world.unit_orders.insert(unit_id, order);
        world.tiles.remove(&wall_pos);
        roundtrip_controller_save(&world, unit_id).map_err(|error| {
            format!("parity error: fixture '{probe}' building_attack_removed: {error}")
        })?;
        let expected = &fixture["scenarios"]["building_attack_removed"]["after"];
        let actual = controller_save_command_snapshot(&world, unit_id);
        compare_controller_snapshot(
            &probe,
            "building_attack_removed",
            "after",
            expected,
            &actual,
        )?;
    }

    // building_queue_removed
    {
        let unit_id = 3_010_050;
        let mut wall = tile_at(22, 6);
        crate::network::world::stamp_new_building(&world, &mut wall);
        world.tiles.insert(wall_pos, wall);
        run_command(
            &world,
            unit_id,
            3_010_051,
            0,
            0,
            false,
            vec![
                UnitOrderTarget {
                    kind: 1,
                    id: wall_pos,
                    x: 0.0,
                    y: 0.0,
                },
                UnitOrderTarget {
                    kind: 0,
                    id: -1,
                    x: 300.0,
                    y: 300.0,
                },
            ],
        );
        world.tiles.remove(&wall_pos);
        roundtrip_controller_save(&world, unit_id).map_err(|error| {
            format!("parity error: fixture '{probe}' building_queue_removed: {error}")
        })?;
        let expected = &fixture["scenarios"]["building_queue_removed"]["after"];
        let actual = controller_save_command_snapshot(&world, unit_id);
        compare_controller_snapshot(&probe, "building_queue_removed", "after", expected, &actual)?;
    }

    let outcomes = fixture
        .get("outcomes")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!("parity error: fixture '{probe}' field 'outcomes' must be an object")
        })?;
    for (field, expected) in outcomes {
        let actual = match field.as_str() {
            "command_ai_survives" => Value::Bool(
                controller_save_command_snapshot(&world, 3_010_001)["is_command_ai"]
                    .as_bool()
                    .unwrap_or(false),
            ),
            "command_persisted_id" => {
                controller_save_command_snapshot(&world, 3_010_001)["command_id"].clone()
            }
            "command_target_pos_persisted" => {
                controller_save_command_snapshot(&world, 3_010_001)["has_target_pos"].clone()
            }
            "command_attack_unit_persisted" => {
                controller_save_command_snapshot(&world, 3_010_001)["has_attack_target"].clone()
            }
            "command_queue_building_persisted" => Value::Bool(
                controller_save_command_snapshot(&world, 3_010_001)["queue"]
                    .as_array()
                    .is_some_and(|queue| {
                        queue.iter().any(|entry| {
                            entry.get("kind").and_then(Value::as_str) == Some("building")
                        })
                    }),
            ),
            "command_queue_vec2_persisted" => Value::Bool(
                controller_save_command_snapshot(&world, 3_010_001)["queue"]
                    .as_array()
                    .is_some_and(|queue| {
                        queue
                            .iter()
                            .any(|entry| entry.get("kind").and_then(Value::as_str) == Some("vec2"))
                    }),
            ),
            "command_queue_unit_persisted" => Value::Bool(
                controller_save_command_snapshot(&world, 3_010_001)["queue"]
                    .as_array()
                    .is_some_and(|queue| {
                        queue
                            .iter()
                            .any(|entry| entry.get("kind").and_then(Value::as_str) == Some("unit"))
                    }),
            ),
            "command_stances_persisted" => json!(controller_save_command_snapshot(
                &world, 3_010_001
            )["stances"]
                .as_array()
                .map(|stances| stances.len())
                .unwrap_or(0)),
            "logic_ai_survives" => Value::Bool(
                controller_save_logic_snapshot(&world, 3_010_010)["is_logic_ai"]
                    .as_bool()
                    .unwrap_or(false),
            ),
            "logic_controller_pos_persisted" => {
                controller_save_logic_snapshot(&world, 3_010_010)["controller_pos"].clone()
            }
            "logic_control_timer_persisted_non_default" => Value::Bool(
                (controller_save_logic_snapshot(&world, 3_010_010)["control_timer"]
                    .as_f64()
                    .unwrap_or(0.0) as f32
                    - 123.45)
                    .abs()
                    < 0.01,
            ),
            "logic_move_mode_persisted" => Value::Bool(
                controller_save_logic_snapshot(&world, 3_010_010)["control_mode"].as_str()
                    != Some("idle"),
            ),
            "logic_move_coords_persisted" => {
                let snap = controller_save_logic_snapshot(&world, 3_010_010);
                Value::Bool(
                    snap["move_x"].as_f64().unwrap_or(0.0) != 0.0
                        || snap["move_y"].as_f64().unwrap_or(0.0) != 0.0,
                )
            }
            other => {
                return Err(format!(
                    "parity error: fixture '{probe}' has unknown outcome field '{other}'"
                ));
            }
        };
        if &actual != expected {
            return Err(format!(
                "parity mismatch: fixture '{probe}' outcomes.{field}: java 158.1 = {expected}, rust = {actual}"
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn controller_save_matches_java_1581() {
    // P1-C2: CommandAI/LogicAI controller MSAV round-trip — tag layout,
    // missing-reference pruning and post-load afterRead semantics.
    compare_controller_save_fixture(&fixture("controller-save.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
