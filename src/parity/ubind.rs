//! Parity differential probes — ubind domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::logic::compile;
use crate::logic::ExecutorState;
use crate::logic::LObject;
use crate::network::units::unit_is_logic_controllable;
use crate::network::world::DynamicWorld;
use crate::network::world::UnitAuthority;
use crate::network::world::UnitOrder;
use dashmap::DashMap;

use serde_json::Value;
use std::sync::Arc;

use super::{
    as_str, as_u64, compare_executor_state, fixture, parity_bare_world, require_fields,
    tile_at_pos, validate_common,
};

pub(super) fn ubind_probe_unit(id: i32, team: u8, flag: f64) -> crate::network::world::EnemyUnit {
    crate::network::world::EnemyUnit {
        id,
        unit_type: 0, // dagger
        entity_class: 0,
        team,
        x: 80.0,
        y: 80.0,
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
        flag,
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
        authority: crate::network::world::UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        status_agg: Default::default(),
    }
}

pub(super) fn compare_ubind_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &[
            "executions",
            "program",
            "counter",
            "text",
            "vars",
            "unit_count",
            "processor_team",
        ],
    )?;
    let program = as_str(fixture, &probe, "program")?;
    let ticks = as_u64(fixture, &probe, "tick")?;
    let executions = as_u64(fixture, &probe, "executions")?;
    if ticks != executions {
        return Err(format!(
            "parity error: fixture '{probe}' is inconsistent: tick={ticks} but executions={executions}"
        ));
    }
    let unit_count = as_u64(fixture, &probe, "unit_count")?;
    let processor_team = as_u64(fixture, &probe, "processor_team")? as u8;

    // Same scenario as the probe: a micro processor (431) of the executor
    // team, five daggers created in id order with flags 11..15.
    let world = parity_bare_world("parity-ubind-20.json");
    let processor_pos = (1 << 16) | 1;
    let mut processor_tile = tile_at_pos(processor_pos, 431);
    processor_tile.team = processor_team;
    world.tiles.insert(processor_pos, processor_tile);
    for index in 1..=unit_count as i32 {
        let id = 3_000_000 + index;
        world.enemies.insert(
            id,
            ubind_probe_unit(id, processor_team, 10.0 + f64::from(index)),
        );
    }

    let world = Arc::new(world);
    let compiled = compile(program)
        .unwrap_or_else(|| panic!("parity error: fixture '{probe}' program must compile in rust"));
    let mut state = ExecutorState::new(compiled, Vec::new());
    let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
    let view = crate::logic::WorldView {
        world: &world,
        processor_pos,
        out: &connections,
    };
    // One instruction per tick, exactly like the probe's runOnce loop.
    for _ in 0..ticks {
        state.run_tick(Some(&view), 1);
    }
    compare_executor_state(&state, fixture, &probe)
}

// ---------------------------------------------------------------------------
// Ubind-object probe (ParUbindObject158): Unit-object ubind, six 158.1 cases
// ---------------------------------------------------------------------------

const UBIND_OBJECT_CASES: &[&str] = &[
    "same_team_default_ai",
    "same_team_command_ai_active",
    "same_team_player",
    "enemy_nonprivileged",
    "enemy_privileged",
    "not_logic_controllable",
];

fn ubind_object_probe_unit(
    id: i32,
    team: u8,
    unit_type: i16,
    flag: f64,
    authority: UnitAuthority,
) -> crate::network::world::EnemyUnit {
    let mut unit = ubind_probe_unit(id, team, flag);
    unit.unit_type = unit_type;
    unit.authority = authority;
    unit
}

fn replay_ubind_object(
    world: &DynamicWorld,
    processor_pos: i32,
    unit_id: i32,
    privileged: bool,
) -> Option<i32> {
    let compiled = compile("ubind u").expect("ubind u must compile");
    let u_idx = compiled.var_index("u").expect("variable u");
    let mut state = ExecutorState::new(compiled, Vec::new());
    state.privileged = privileged;
    state.vars[u_idx].isobj = true;
    state.vars[u_idx].objval = LObject::Unit(unit_id);
    let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
    let view = crate::logic::WorldView {
        world,
        processor_pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    state.bound_unit
}

pub(super) fn compare_ubind_object_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &["executions", "program", "processor_team", "cases"],
    )?;
    let cases = fixture
        .get("cases")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!("parity error: fixture '{probe}' field 'cases' must be an object")
        })?;
    for name in UBIND_OBJECT_CASES {
        if !cases.contains_key(*name) {
            return Err(format!(
                "parity error: fixture '{probe}' is missing required field 'cases.{name}'"
            ));
        }
    }

    let processor_team = as_u64(fixture, &probe, "processor_team")? as u8;
    let world = parity_bare_world("parity-ubind-object.json");
    let processor_pos = (1 << 16) | 1;
    let mut processor_tile = tile_at_pos(processor_pos, 431);
    processor_tile.team = processor_team;
    world.tiles.insert(processor_pos, processor_tile);

    // Same six units the Java probe creates (flags 11..16).
    world.enemies.insert(
        3_000_001,
        ubind_object_probe_unit(3_000_001, 1, 0, 11.0, UnitAuthority::DefaultAi),
    );
    world.enemies.insert(
        3_000_002,
        ubind_object_probe_unit(3_000_002, 1, 0, 12.0, UnitAuthority::Command),
    );
    world.unit_orders.insert(
        3_000_002,
        UnitOrder {
            unit_id: 3_000_002,
            command: 0,
            target_kind: 0,
            target_x: Some(100.0),
            target_y: Some(100.0),
            target_id: -1,
            stances: 0,
            payload_cooldown: 0.0,
            logic_control: 0,
            queue: Vec::new(),
        },
    );
    world.enemies.insert(
        3_000_003,
        ubind_object_probe_unit(
            3_000_003,
            1,
            0,
            13.0,
            UnitAuthority::Player { player_id: 1 },
        ),
    );
    world.enemies.insert(
        3_000_004,
        ubind_object_probe_unit(3_000_004, 2, 0, 14.0, UnitAuthority::DefaultAi),
    );
    world.enemies.insert(
        3_000_005,
        ubind_object_probe_unit(3_000_005, 2, 0, 15.0, UnitAuthority::DefaultAi),
    );
    world.enemies.insert(
        3_000_006,
        ubind_object_probe_unit(3_000_006, 1, 63, 16.0, UnitAuthority::DefaultAi),
    );

    let setups: [(&str, i32, bool); 6] = [
        ("same_team_default_ai", 3_000_001, false),
        ("same_team_command_ai_active", 3_000_002, false),
        ("same_team_player", 3_000_003, false),
        ("enemy_nonprivileged", 3_000_004, false),
        ("enemy_privileged", 3_000_005, true),
        ("not_logic_controllable", 3_000_006, false),
    ];

    let mut failures = Vec::new();
    for (name, unit_id, privileged) in setups {
        let case = &cases[name];
        for field in [
            "bound",
            "type_logic_controllable",
            "controller_logic_controllable",
            "controller_unchanged",
            "acquired_logic",
        ] {
            if case.get(field).is_none() {
                return Err(format!(
                    "parity error: fixture '{probe}' is missing required field 'cases.{name}.{field}'"
                ));
            }
        }
        let expected_bound = case.get("bound").and_then(Value::as_bool).ok_or_else(|| {
            format!("parity error: fixture '{probe}' field 'cases.{name}.bound' must be a boolean")
        })?;
        let expected_type = case
            .get("type_logic_controllable")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!(
                    "parity error: fixture '{probe}' field 'cases.{name}.type_logic_controllable' must be a boolean"
                )
            })?;
        let expected_ctrl = case
            .get("controller_logic_controllable")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!(
                    "parity error: fixture '{probe}' field 'cases.{name}.controller_logic_controllable' must be a boolean"
                )
            })?;
        let expected_unchanged = case
            .get("controller_unchanged")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!(
                    "parity error: fixture '{probe}' field 'cases.{name}.controller_unchanged' must be a boolean"
                )
            })?;
        let expected_acquired = case
            .get("acquired_logic")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!(
                    "parity error: fixture '{probe}' field 'cases.{name}.acquired_logic' must be a boolean"
                )
            })?;

        let before = world
            .enemies
            .get(&unit_id)
            .map(|unit| unit.authority)
            .expect("probe unit must exist");
        let type_ok = crate::game::unit_types::unit_type_logic_controllable(
            world.enemies.get(&unit_id).unwrap().unit_type,
        );
        let ctrl_ok = unit_is_logic_controllable(&world, unit_id);
        let bound = replay_ubind_object(&world, processor_pos, unit_id, privileged);
        let after = world
            .enemies
            .get(&unit_id)
            .map(|unit| unit.authority)
            .expect("probe unit must exist after bind");
        let unchanged = before == after;
        let acquired = matches!(after, UnitAuthority::Logic { .. });
        let rust_bound = bound == Some(unit_id);

        if expected_bound != rust_bound {
            failures.push(format!(
                "parity mismatch: field 'cases.{name}.bound': java 158.1 = {expected_bound}, rust = {rust_bound}"
            ));
        }
        if expected_type != type_ok {
            failures.push(format!(
                "parity mismatch: field 'cases.{name}.type_logic_controllable': java 158.1 = {expected_type}, rust = {type_ok}"
            ));
        }
        if expected_ctrl != ctrl_ok {
            failures.push(format!(
                "parity mismatch: field 'cases.{name}.controller_logic_controllable': java 158.1 = {expected_ctrl}, rust = {ctrl_ok}"
            ));
        }
        if expected_unchanged != unchanged {
            failures.push(format!(
                "parity mismatch: field 'cases.{name}.controller_unchanged': java 158.1 = {expected_unchanged}, rust = {unchanged}"
            ));
        }
        if expected_acquired != acquired {
            failures.push(format!(
                "parity mismatch: field 'cases.{name}.acquired_logic': java 158.1 = {expected_acquired}, rust = {acquired}"
            ));
        }
        if rust_bound {
            let flag = world.enemies.get(&unit_id).unwrap().flag;
            match case.get("flag").and_then(Value::as_f64) {
                Some(expected_flag) if (expected_flag - flag).abs() > f64::EPSILON => {
                    failures.push(format!(
                        "parity mismatch: field 'cases.{name}.flag': java 158.1 = {expected_flag}, rust = {flag}"
                    ));
                }
                None => {
                    return Err(format!(
                        "parity error: fixture '{probe}' field 'cases.{name}.flag' must be a number when bound"
                    ));
                }
                Some(_) => {}
            }
        }
    }
    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Ubind-reinsert probe (ParUbindReinsert158): Groups.unit order after
// remove/re-add, payload, death, and cursor % seq.size
// ---------------------------------------------------------------------------

const UBIND_REINSERT_SCENARIOS: &[&str] = &[
    "baseline",
    "readd",
    "payload",
    "death_spawn",
    "cursor_len",
    "four_remove_b",
];

fn reinsert_processor(world: &DynamicWorld, team: u8) -> i32 {
    let processor_pos = (1 << 16) | 1;
    let mut processor_tile = tile_at_pos(processor_pos, 431);
    processor_tile.team = team;
    world.tiles.insert(processor_pos, processor_tile);
    processor_pos
}

fn reinsert_spawn(world: &DynamicWorld, team: u8, flags: &[f64]) -> Vec<i32> {
    let mut ids = Vec::new();
    for (index, flag) in flags.iter().enumerate() {
        let id = 3_000_001 + index as i32;
        world.enemies.insert(id, ubind_probe_unit(id, team, *flag));
        world.register_unit_group(id);
        ids.push(id);
    }
    ids
}

fn reinsert_remove(world: &DynamicWorld, id: i32) -> crate::network::world::EnemyUnit {
    world.unregister_unit_group(id);
    world
        .enemies
        .remove(&id)
        .map(|(_, unit)| unit)
        .expect("reinsert unit must exist")
}

fn reinsert_add(world: &DynamicWorld, unit: crate::network::world::EnemyUnit) {
    world.register_unit_group(unit.id);
    world.enemies.insert(unit.id, unit);
}

fn run_ubind_reinsert(
    world: &DynamicWorld,
    processor_pos: i32,
    program: &str,
    ticks: u64,
) -> ExecutorState {
    let compiled = compile(program).expect("reinsert program must compile");
    let mut state = ExecutorState::new(compiled, Vec::new());
    let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
    let view = crate::logic::WorldView {
        world,
        processor_pos,
        out: &connections,
    };
    for _ in 0..ticks {
        state.run_tick(Some(&view), 1);
    }
    state
}

fn scenario_text<'a>(
    scenarios: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    scenarios
        .get(name)
        .and_then(|s| s.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "parity error: fixture 'ubind-reinsert' is missing required field 'scenarios.{name}.text'"
            )
        })
}

pub(super) fn compare_ubind_reinsert_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &["executions", "program", "processor_team", "scenarios"],
    )?;
    let scenarios = fixture
        .get("scenarios")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!("parity error: fixture '{probe}' field 'scenarios' must be an object")
        })?;
    for name in UBIND_REINSERT_SCENARIOS {
        if !scenarios.contains_key(*name) {
            return Err(format!(
                "parity error: fixture '{probe}' is missing required field 'scenarios.{name}'"
            ));
        }
        if scenarios[*name].get("text").is_none() {
            return Err(format!(
                "parity error: fixture '{probe}' is missing required field 'scenarios.{name}.text'"
            ));
        }
    }
    let program = as_str(fixture, &probe, "program")?;
    let ticks = as_u64(fixture, &probe, "tick")?;
    let processor_team = as_u64(fixture, &probe, "processor_team")? as u8;

    let mut failures = Vec::new();

    // --- baseline: spawn A,B,C, 12× ubind --------------------------------
    {
        let world = parity_bare_world("parity-ubind-reinsert-baseline.json");
        let pos = reinsert_processor(&world, processor_team);
        reinsert_spawn(&world, processor_team, &[11.0, 12.0, 13.0]);
        let state = run_ubind_reinsert(&world, pos, program, ticks * 6 + 10);
        let expected = scenario_text(scenarios, "baseline")?;
        if state.text_buffer != expected {
            failures.push(format!(
                "parity mismatch: field 'scenarios.baseline.text':\n  java 158.1 = {expected:?}\n  rust       = {:?}",
                state.text_buffer
            ));
        }
    }

    // --- readd: B.remove() + B.add() (same id) ---------------------------
    {
        let world = parity_bare_world("parity-ubind-reinsert-readd.json");
        let pos = reinsert_processor(&world, processor_team);
        reinsert_spawn(&world, processor_team, &[11.0, 12.0, 13.0]);
        let b = reinsert_remove(&world, 3_000_002);
        reinsert_add(&world, b);
        let state = run_ubind_reinsert(&world, pos, program, ticks * 6 + 10);
        let expected = scenario_text(scenarios, "readd")?;
        if state.text_buffer != expected {
            failures.push(format!(
                "parity mismatch: field 'scenarios.readd.text':\n  java 158.1 = {expected:?}\n  rust       = {:?}",
                state.text_buffer
            ));
        }
    }

    // --- payload: B leaves Groups, returns with a new id (dropUnit) ------
    {
        let world = parity_bare_world("parity-ubind-reinsert-payload.json");
        let pos = reinsert_processor(&world, processor_team);
        reinsert_spawn(&world, processor_team, &[11.0, 12.0, 13.0]);
        let mut b = reinsert_remove(&world, 3_000_002);
        b.id = 3_000_100;
        reinsert_add(&world, b);
        let state = run_ubind_reinsert(&world, pos, program, ticks * 6 + 10);
        let expected = scenario_text(scenarios, "payload")?;
        if state.text_buffer != expected {
            failures.push(format!(
                "parity mismatch: field 'scenarios.payload.text':\n  java 158.1 = {expected:?}\n  rust       = {:?}",
                state.text_buffer
            ));
        }
    }

    // --- death + spawn D -------------------------------------------------
    {
        let world = parity_bare_world("parity-ubind-reinsert-death.json");
        let pos = reinsert_processor(&world, processor_team);
        reinsert_spawn(&world, processor_team, &[11.0, 12.0, 13.0]);
        let _ = reinsert_remove(&world, 3_000_002);
        reinsert_spawn_one(&world, processor_team, 3_000_004, 14.0);
        let state = run_ubind_reinsert(&world, pos, program, ticks * 6 + 10);
        let expected = scenario_text(scenarios, "death_spawn")?;
        if state.text_buffer != expected {
            failures.push(format!(
                "parity mismatch: field 'scenarios.death_spawn.text':\n  java 158.1 = {expected:?}\n  rust       = {:?}",
                state.text_buffer
            ));
        }
    }

    // --- cursor when seq.len changes: 2 binds, remove A, 4 more ----------
    {
        let world = parity_bare_world("parity-ubind-reinsert-cursor.json");
        let pos = reinsert_processor(&world, processor_team);
        reinsert_spawn(&world, processor_team, &[11.0, 12.0, 13.0]);
        let cursor_program = "ubind @dagger\n\
sensor n @unit @flag\n\
print n\n\
print \" \"\n\
op add i i 1\n\
jump 0 lessThan i 100\n\
stop";
        let compiled = compile(cursor_program).expect("cursor program must compile");
        let mut state = ExecutorState::new(compiled, Vec::new());
        let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
        let view = crate::logic::WorldView {
            world: &world,
            processor_pos: pos,
            out: &connections,
        };
        for _ in 0..12 {
            state.run_tick(Some(&view), 1);
        }
        let expected_before = scenarios["cursor_len"]
            .get("text_before")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!(
                    "parity error: fixture '{probe}' is missing required field 'scenarios.cursor_len.text_before'"
                )
            })?;
        if state.text_buffer != expected_before {
            failures.push(format!(
                "parity mismatch: field 'scenarios.cursor_len.text_before':\n  java 158.1 = {expected_before:?}\n  rust       = {:?}",
                state.text_buffer
            ));
        }
        let _ = reinsert_remove(&world, 3_000_001);
        for _ in 0..24 {
            state.run_tick(Some(&view), 1);
        }
        let expected = scenario_text(scenarios, "cursor_len")?;
        if state.text_buffer != expected {
            failures.push(format!(
                "parity mismatch: field 'scenarios.cursor_len.text':\n  java 158.1 = {expected:?}\n  rust       = {:?}",
                state.text_buffer
            ));
        }
    }

    // --- four-unit remove B: swap-remove with last, not id order ---------
    {
        let world = parity_bare_world("parity-ubind-reinsert-four.json");
        let pos = reinsert_processor(&world, processor_team);
        reinsert_spawn(&world, processor_team, &[11.0, 12.0, 13.0, 14.0]);
        let _ = reinsert_remove(&world, 3_000_002);
        let state = run_ubind_reinsert(&world, pos, program, ticks * 6 + 10);
        let expected = scenario_text(scenarios, "four_remove_b")?;
        if state.text_buffer != expected {
            failures.push(format!(
                "parity mismatch: field 'scenarios.four_remove_b.text':\n  java 158.1 = {expected:?}\n  rust       = {:?}",
                state.text_buffer
            ));
        }
    }

    if !failures.is_empty() {
        return Err(failures.join("; "));
    }
    Ok(())
}

fn reinsert_spawn_one(world: &DynamicWorld, team: u8, id: i32, flag: f64) {
    world.enemies.insert(id, ubind_probe_unit(id, team, flag));
    world.register_unit_group(id);
}

// ---------------------------------------------------------------------------
// Lease probe (ParLease158): LogicAI acquisition + 600-tick lease lifecycle
// ---------------------------------------------------------------------------

#[test]
fn ubind_20_matches_java_1581() {
    // Differential P0-02: the official 20-ubind round-robin sequence over
    // five sharded daggers (creation order), replayed by the Rust executor.
    compare_ubind_fixture(&fixture("ubind-20.json")).unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn ubind_object_matches_java_1581() {
    // Differential P0-B1: Unit-object ubind — team/privilege + type.logicControllable
    // only; CommandAI/Player controllers do not prevent bind.
    compare_ubind_object_fixture(&fixture("ubind-object.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn ubind_reinsert_matches_java_1581() {
    // Differential P0-B2: Groups.unit order after remove/re-add, payload
    // rejoin, death+spawn, and cursor %= seq.size when the cache shrinks.
    compare_ubind_reinsert_fixture(&fixture("ubind-reinsert.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
