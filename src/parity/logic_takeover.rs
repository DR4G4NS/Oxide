//! Parity differential probes — logic_takeover domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::logic::compile;
use crate::logic::ExecutorState;
use crate::network::world::DynamicWorld;
use crate::network::world::UnitAuthority;
use crate::network::world::UnitOrder;
use dashmap::DashMap;

use serde_json::Value;
use std::sync::Arc;

use super::{as_bool, as_str, as_u64, fixture, parity_bare_world, require_fields, validate_common};

use super::logic_owner::owner_logic_of;

use super::logic_owner::stamp_micro_processor;

use super::ubind::ubind_probe_unit;

fn takeover_dirty_plan() -> crate::network::world::UnitBuildPlan {
    crate::network::world::UnitBuildPlan {
        breaking: false,
        position: (2 << 16) | 2,
        block: 216,
        rotation: 0,
        config: Vec::new(),
    }
}

fn seed_takeover_dirty(world: &DynamicWorld, unit_id: i32, logic_build: bool) {
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.mine_progress = 42.0;
        unit.build_plans = vec![takeover_dirty_plan()];
    }
    if logic_build {
        if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
            order.target_kind = 9;
            order.target_id = 216;
            order.target_x = Some(16.0);
            order.target_y = Some(16.0);
        } else {
            world.unit_orders.insert(
                unit_id,
                UnitOrder {
                    unit_id,
                    command: 2,
                    stances: 0,
                    payload_cooldown: 0.0,
                    target_kind: 9,
                    target_id: 216,
                    target_x: Some(16.0),
                    target_y: Some(16.0),
                    logic_control: 0,
                    queue: Vec::new(),
                },
            );
        }
    }
}

pub(super) fn drive_takeover_tick(
    world: &DynamicWorld,
    processor_pos: i32,
    state: &mut ExecutorState,
) {
    let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
    let view = crate::logic::WorldView {
        world,
        processor_pos,
        out: &connections,
    };
    crate::network::simulation::simulate_logic_control_leases(world, 1.0);
    state.run_tick(Some(&view), 1);
}

fn compare_logic_takeover_fixture(fixture: &Value) -> Result<(), String> {
    use crate::network::units::{
        release_logic_control, set_order_active_target, unit_has_active_rts_command,
    };
    use crate::network::world::UnitOrderTarget;

    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &[
            "program",
            "first_logic",
            "first_mine_null",
            "first_plans_empty",
            "first_plans",
            "second_logic",
            "second_mine_kept",
            "second_plans_kept",
            "second_plans",
            "fail_was_command",
            "fail_has_command",
            "fail_still_command",
            "fail_mine_kept",
            "fail_plans_kept",
            "fail_became_logic",
        ],
    )?;
    let program = as_str(fixture, &probe, "program")?;

    let world = parity_bare_world("parity-logic-takeover.json");
    let processor_pos = (8 << 16) | 8;
    let team = 1u8;
    let unit_id = 3_000_001;
    let mut poly = ubind_probe_unit(unit_id, team, 0.0);
    poly.unit_type = 21;
    poly.authority = crate::network::units::default_unit_authority(&world, &poly);
    world.enemies.insert(unit_id, poly);
    world.register_unit_group(unit_id);
    stamp_micro_processor(&world, processor_pos, team);
    seed_takeover_dirty(&world, unit_id, true);

    let compiled = compile(program)
        .unwrap_or_else(|| panic!("parity error: logic-takeover program must compile in rust"));
    let mut state = ExecutorState::new(compiled, Vec::new());
    let world = Arc::new(world);

    drive_takeover_tick(&world, processor_pos, &mut state); // ubind
    drive_takeover_tick(&world, processor_pos, &mut state); // first move
    let first_logic = owner_logic_of(&world, unit_id).is_some();
    let first_mine_null = world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| unit.mine_progress == 0.0)
        && !world
            .unit_orders
            .get(&unit_id)
            .is_some_and(|order| order.target_kind == 6);
    let first_plans = world
        .enemies
        .get(&unit_id)
        .map(|unit| unit.build_plans.len() as u64)
        .unwrap_or(0);
    let first_plans_empty = first_plans == 0;

    seed_takeover_dirty(&world, unit_id, true);
    drive_takeover_tick(&world, processor_pos, &mut state); // second move
    let second_logic = owner_logic_of(&world, unit_id).is_some();
    let second_mine_kept = world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| unit.mine_progress == 42.0);
    let second_plans = world
        .enemies
        .get(&unit_id)
        .map(|unit| unit.build_plans.len() as u64)
        .unwrap_or(0);
    let second_plans_kept = second_plans > 0;

    release_logic_control(&world, unit_id);
    world.enemies.get_mut(&unit_id).unwrap().authority = UnitAuthority::Command;
    let mut fail_order = world
        .unit_orders
        .get(&unit_id)
        .map(|order| order.clone())
        .unwrap_or(UnitOrder {
            unit_id,
            command: 2,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: 0,
            queue: Vec::new(),
        });
    set_order_active_target(
        &mut fail_order,
        UnitOrderTarget {
            kind: 0,
            id: -1,
            x: 400.0,
            y: 400.0,
        },
    );
    world.unit_orders.insert(unit_id, fail_order);
    seed_takeover_dirty(&world, unit_id, false);
    let fail_was_command = world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| unit.authority == UnitAuthority::Command);
    let fail_has_command = unit_has_active_rts_command(&world, unit_id);

    let fail_program = compile("ucontrol move 30 30\nstop")
        .unwrap_or_else(|| panic!("parity error: logic-takeover fail program must compile"));
    let mut fail_state = ExecutorState::new(fail_program, Vec::new());
    fail_state.bound_unit = Some(unit_id);
    drive_takeover_tick(&world, processor_pos, &mut fail_state);
    let fail_still_command = world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| unit.authority == UnitAuthority::Command);
    let fail_became_logic = owner_logic_of(&world, unit_id).is_some();
    let fail_mine_kept = world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| unit.mine_progress == 42.0);
    let fail_plans_kept = world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| !unit.build_plans.is_empty());

    let mut failures = Vec::new();
    let check_bool =
        |field: &str, actual: bool, failures: &mut Vec<String>| -> Result<(), String> {
            let expected = as_bool(fixture, &probe, field)?;
            if actual != expected {
                failures.push(format!("{field}: java 158.1 = {expected}, rust = {actual}"));
            }
            Ok(())
        };
    check_bool("first_logic", first_logic, &mut failures)?;
    check_bool("first_mine_null", first_mine_null, &mut failures)?;
    check_bool("first_plans_empty", first_plans_empty, &mut failures)?;
    check_bool("second_logic", second_logic, &mut failures)?;
    check_bool("second_mine_kept", second_mine_kept, &mut failures)?;
    check_bool("second_plans_kept", second_plans_kept, &mut failures)?;
    check_bool("fail_was_command", fail_was_command, &mut failures)?;
    check_bool("fail_has_command", fail_has_command, &mut failures)?;
    check_bool("fail_still_command", fail_still_command, &mut failures)?;
    check_bool("fail_mine_kept", fail_mine_kept, &mut failures)?;
    check_bool("fail_plans_kept", fail_plans_kept, &mut failures)?;
    check_bool("fail_became_logic", fail_became_logic, &mut failures)?;
    let expected_first_plans = as_u64(fixture, &probe, "first_plans")?;
    if first_plans != expected_first_plans {
        failures.push(format!(
            "first_plans: java 158.1 = {expected_first_plans}, rust = {first_plans}"
        ));
    }
    let expected_second_plans = as_u64(fixture, &probe, "second_plans")?;
    if second_plans != expected_second_plans {
        failures.push(format!(
            "second_plans: java 158.1 = {expected_second_plans}, rust = {second_plans}"
        ));
    }
    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' first-takeover wipe diverges: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Command probe (ParCommand158): CommandAI queue/authority semantics
// ---------------------------------------------------------------------------

#[test]
fn logic_takeover_matches_java_1581() {
    // Differential P0-C2: first LogicAI acquisition on a builder (poly)
    // clears mineTile + BuilderComp.plans; a later ucontrol move only
    // refreshes the lease; a failed checkLogicAI gate is a no-op.
    compare_logic_takeover_fixture(&fixture("logic-takeover.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
