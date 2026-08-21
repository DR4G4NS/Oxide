//! Parity differential probes — logic_owner domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::logic::compile;
use crate::logic::ExecutorState;
use crate::logic::LObject;
use crate::network::world::DynamicWorld;
use crate::network::world::UnitAuthority;
use dashmap::DashMap;

use serde_json::Value;
use std::sync::Arc;

use super::{
    as_bool, as_str, as_u64, fixture, parity_bare_world, require_fields, tile_at_pos,
    validate_common,
};

use super::lease::lease_num;

use super::ubind::ubind_probe_unit;

pub(super) fn owner_logic_of(world: &DynamicWorld, unit_id: i32) -> Option<(i32, u64)> {
    match world.enemies.get(&unit_id).map(|unit| unit.authority) {
        Some(UnitAuthority::Logic {
            processor_pos,
            processor_generation,
            ..
        }) => Some((processor_pos, processor_generation)),
        _ => None,
    }
}

pub(super) fn stamp_micro_processor(world: &DynamicWorld, pos: i32, team: u8) -> u64 {
    let mut tile = tile_at_pos(pos, 431);
    tile.team = team;
    crate::network::world::stamp_new_building(world, &mut tile);
    let generation = tile.generation;
    world.tiles.insert(pos, tile);
    generation
}

const OWNER_AGE_BEFORE_EXPIRY: usize = 599;

fn owner_unit_var_kept(state: &ExecutorState, unit_id: i32) -> bool {
    state.bound_unit == Some(unit_id)
        && state.vars[state.program.unit_var].objval == LObject::Unit(unit_id)
}

fn owner_timer_of(world: &DynamicWorld, unit_id: i32) -> Option<f32> {
    match world.enemies.get(&unit_id).map(|unit| unit.authority) {
        Some(UnitAuthority::Logic {
            remaining_ticks, ..
        }) => Some(remaining_ticks),
        _ => None,
    }
}

fn owner_lease_tick(world: &DynamicWorld) {
    crate::network::simulation::simulate_logic_control_leases(world, 1.0);
}

fn run_owner_instruction(
    world: &DynamicWorld,
    processor_pos: i32,
    program: &str,
    unit_id: i32,
) -> ExecutorState {
    let compiled = compile(program)
        .unwrap_or_else(|| panic!("parity error: logic-owner program must compile in rust"));
    let mut state = ExecutorState::new(compiled, Vec::new());
    state.bound_unit = Some(unit_id);
    let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
    let view = crate::logic::WorldView {
        world,
        processor_pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    state
}

fn acquire_owner_lease(world: &DynamicWorld, processor_pos: i32, program: &str, unit_id: i32) {
    crate::network::units::release_logic_control(world, unit_id);
    run_owner_instruction(world, processor_pos, program, unit_id);
    assert!(
        owner_timer_of(world, unit_id).is_some(),
        "parity error: logic-owner lease acquisition failed in rust"
    );
}

fn drive_owner_acquire(
    world: &DynamicWorld,
    processor_pos: i32,
    program: &str,
    unit_id: i32,
) -> (bool, i64, Option<u64>) {
    let compiled = compile(program)
        .unwrap_or_else(|| panic!("parity error: logic-owner program must compile in rust"));
    let mut state = ExecutorState::new(compiled, Vec::new());
    let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
    let view = crate::logic::WorldView {
        world,
        processor_pos,
        out: &connections,
    };
    let mut acquired = false;
    let mut acquire_tick = -1i64;
    let mut generation = None;
    for tick in 1..=4i64 {
        crate::network::simulation::simulate_logic_control_leases(world, 1.0);
        state.run_tick(Some(&view), 1);
        if !acquired {
            if let Some((_, gen)) = owner_logic_of(world, unit_id) {
                acquired = true;
                acquire_tick = tick;
                generation = Some(gen);
            }
        }
    }
    (acquired, acquire_tick, generation)
}

fn compare_logic_owner_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &[
            "program",
            "a_acquired",
            "a_controller_valid",
            "a_controller_is_build",
            "a_valid_after_b_placed",
            "b_is_a",
            "still_logic_before_b_tick",
            "still_logic_after_b_tick",
            "b_acquired",
            "b_controller_is_b",
            "b_controller_is_a",
            "c_is_b",
            "still_logic_after_c_tick",
            "wall_acquired",
            "still_logic_after_wall",
            "control_program",
            "move_program",
            "unbind_program",
            "lease_timer_at_acquire",
            "lease_timer_at_599",
            "lease_controlled_at_599",
            "lease_timer_at_600",
            "lease_controlled_at_600",
            "lease_controlled_at_601",
            "refresh_timer_before",
            "refresh_timer_after",
            "refresh_controlled_at_600",
            "refresh_controlled_at_601",
            "gate_timer_after",
            "gate_flag_written",
            "gate_controlled_at_600",
            "gate_controlled_at_601",
            "unbind_logic_after",
            "unbind_unit_var_kept",
            "unbind_has_command",
            "unbind_logic_controllable",
            "unbind_reacquired",
            "unbind_reacquire_timer",
            "rts_same_controller",
            "rts_became_logic",
            "rts_has_command_after",
            "rts_unit_var_kept",
        ],
    )?;
    let program = as_str(fixture, &probe, "program")?;

    let world = parity_bare_world("parity-logic-owner.json");
    let processor_pos = (8 << 16) | 8;
    let team = 1u8;
    let unit_id = 3_000_001;
    world
        .enemies
        .insert(unit_id, ubind_probe_unit(unit_id, team, 0.0));
    let default_authority = {
        let unit = world.enemies.get(&unit_id).unwrap();
        // dashmap-guard: allow DM900 reason="default_unit_authority reads wave rules and game state only; it does not access world.enemies"
        crate::network::units::default_unit_authority(&world, &unit)
    };
    world.enemies.get_mut(&unit_id).unwrap().authority = default_authority;
    let world = Arc::new(world);

    let gen_a = stamp_micro_processor(&world, processor_pos, team);
    let (a_acquired, acquire_tick, a_gen) =
        drive_owner_acquire(&world, processor_pos, program, unit_id);
    let a_controller_is_build = owner_logic_of(&world, unit_id)
        .is_some_and(|(pos, gen)| pos == processor_pos && gen == gen_a);
    let a_controller_valid = a_gen.is_some_and(|gen| {
        crate::network::units::processor_lease_valid(&world, processor_pos, gen)
    });

    let still_logic_before_b_tick = owner_logic_of(&world, unit_id).is_some();
    let gen_b = stamp_micro_processor(&world, processor_pos, team);
    let a_valid_after_b_placed =
        crate::network::units::processor_lease_valid(&world, processor_pos, gen_a);
    let b_is_a = gen_b == gen_a;
    crate::network::simulation::simulate_logic_control_leases(&world, 1.0);
    let still_logic_after_b_tick = owner_logic_of(&world, unit_id).is_some();

    let (b_acquired, _, b_gen) = drive_owner_acquire(&world, processor_pos, program, unit_id);
    let b_controller_is_b = b_gen == Some(gen_b);
    let b_controller_is_a = b_gen == Some(gen_a);

    let gen_c = stamp_micro_processor(&world, processor_pos, team);
    let c_is_b = gen_c == gen_b;
    crate::network::simulation::simulate_logic_control_leases(&world, 1.0);
    let still_logic_after_c_tick = owner_logic_of(&world, unit_id).is_some();

    crate::network::units::release_logic_control(&world, unit_id);
    stamp_micro_processor(&world, processor_pos, team);
    let (wall_acquired, _, _) = drive_owner_acquire(&world, processor_pos, program, unit_id);
    let mut wall = tile_at_pos(processor_pos, 216);
    wall.team = team;
    crate::network::world::stamp_new_building(&world, &mut wall);
    world.tiles.insert(processor_pos, wall);
    crate::network::simulation::simulate_logic_control_leases(&world, 1.0);
    let still_logic_after_wall = owner_logic_of(&world, unit_id).is_some();

    // === P0-C3: exact lease boundary, refresh points and unbind ============
    let control = as_str(fixture, &probe, "control_program")?;
    let move_program = as_str(fixture, &probe, "move_program")?;
    let unbind_program = as_str(fixture, &probe, "unbind_program")?;
    stamp_micro_processor(&world, processor_pos, team);

    // (L) No refresh: the guard tests `controlTimer > 0` BEFORE decrementing,
    // so age 600 is still controlled with a timer of exactly 0.
    acquire_owner_lease(&world, processor_pos, control, unit_id);
    let lease_timer_at_acquire = owner_timer_of(&world, unit_id);
    for _ in 0..OWNER_AGE_BEFORE_EXPIRY {
        owner_lease_tick(&world);
    }
    let lease_timer_599 = owner_timer_of(&world, unit_id);
    let lease_controlled_599 = lease_timer_599.is_some();
    owner_lease_tick(&world);
    let lease_timer_600 = owner_timer_of(&world, unit_id);
    let lease_controlled_600 = lease_timer_600.is_some();
    owner_lease_tick(&world);
    let lease_controlled_601 = owner_timer_of(&world, unit_id).is_some();

    // (R) A valid ucontrol on the last tick before expiry restarts it.
    acquire_owner_lease(&world, processor_pos, control, unit_id);
    for _ in 0..OWNER_AGE_BEFORE_EXPIRY {
        owner_lease_tick(&world);
    }
    let refresh_timer_before = owner_timer_of(&world, unit_id);
    run_owner_instruction(&world, processor_pos, control, unit_id);
    let refresh_timer_after = owner_timer_of(&world, unit_id);
    for _ in 0..600 {
        owner_lease_tick(&world);
    }
    let refresh_controlled_600 = owner_timer_of(&world, unit_id).is_some();
    owner_lease_tick(&world);
    let refresh_controlled_601 = owner_timer_of(&world, unit_id).is_some();

    // (G) The same instruction with a failing checkLogicAI writes nothing.
    acquire_owner_lease(&world, processor_pos, control, unit_id);
    for _ in 0..OWNER_AGE_BEFORE_EXPIRY {
        owner_lease_tick(&world);
    }
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.flag = 0.0;
        unit.team = 2; // crux: `unit.team == exec.team` fails
    }
    run_owner_instruction(&world, processor_pos, control, unit_id);
    let gate_timer_after = owner_timer_of(&world, unit_id);
    let gate_flag_written = world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| unit.flag != 0.0);
    owner_lease_tick(&world);
    let gate_controlled_600 = owner_timer_of(&world, unit_id).is_some();
    owner_lease_tick(&world);
    let gate_controlled_601 = owner_timer_of(&world, unit_id).is_some();
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.team = team;
    }

    // (U) unbind resets the controller and preserves @unit. The acquiring
    // instruction is a `move` so the released unit carries a LogicAI
    // destination: the fresh controller must still report no command.
    crate::network::units::release_logic_control(&world, unit_id);
    run_owner_instruction(&world, processor_pos, move_program, unit_id);
    let unbind_logic_before = owner_timer_of(&world, unit_id).is_some();
    let unbind_state = run_owner_instruction(&world, processor_pos, unbind_program, unit_id);
    let unbind_logic_after = owner_timer_of(&world, unit_id).is_some();
    let unbind_unit_var_kept = owner_unit_var_kept(&unbind_state, unit_id);
    let unbind_has_command =
        matches!(
            world.enemies.get(&unit_id).map(|unit| unit.authority),
            Some(UnitAuthority::Command)
        ) && crate::network::units::unit_has_active_rts_command(&world, unit_id);
    let unbind_logic_controllable =
        crate::network::units::unit_is_logic_controllable(&world, unit_id);
    run_owner_instruction(&world, processor_pos, control, unit_id);
    let unbind_reacquire_timer = owner_timer_of(&world, unit_id);
    let unbind_reacquired = unbind_reacquire_timer.is_some();

    // (T) unbind of an actively RTS-commanded unit fails the gate entirely.
    crate::network::units::release_logic_control(&world, unit_id);
    if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
        crate::network::units::set_order_active_target(
            &mut order,
            crate::network::world::UnitOrderTarget {
                kind: 0,
                id: -1,
                x: 200.0,
                y: 200.0,
            },
        );
    }
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.authority = UnitAuthority::Command;
    }
    let rts_state = run_owner_instruction(&world, processor_pos, unbind_program, unit_id);
    let rts_same_controller = matches!(
        world.enemies.get(&unit_id).map(|unit| unit.authority),
        Some(UnitAuthority::Command)
    );
    let rts_became_logic = owner_timer_of(&world, unit_id).is_some();
    let rts_has_command_after = crate::network::units::unit_has_active_rts_command(&world, unit_id);
    let rts_unit_var_kept = owner_unit_var_kept(&rts_state, unit_id);

    let mut failures = Vec::new();
    let check_bool =
        |field: &str, actual: bool, failures: &mut Vec<String>| -> Result<(), String> {
            let expected = as_bool(fixture, &probe, field)?;
            if actual != expected {
                failures.push(format!("{field}: java 158.1 = {expected}, rust = {actual}"));
            }
            Ok(())
        };
    check_bool("a_acquired", a_acquired, &mut failures)?;
    check_bool("a_controller_valid", a_controller_valid, &mut failures)?;
    check_bool(
        "a_controller_is_build",
        a_controller_is_build,
        &mut failures,
    )?;
    check_bool(
        "a_valid_after_b_placed",
        a_valid_after_b_placed,
        &mut failures,
    )?;
    check_bool("b_is_a", b_is_a, &mut failures)?;
    check_bool(
        "still_logic_before_b_tick",
        still_logic_before_b_tick,
        &mut failures,
    )?;
    check_bool(
        "still_logic_after_b_tick",
        still_logic_after_b_tick,
        &mut failures,
    )?;
    check_bool("b_acquired", b_acquired, &mut failures)?;
    check_bool("b_controller_is_b", b_controller_is_b, &mut failures)?;
    check_bool("b_controller_is_a", b_controller_is_a, &mut failures)?;
    check_bool("c_is_b", c_is_b, &mut failures)?;
    check_bool(
        "still_logic_after_c_tick",
        still_logic_after_c_tick,
        &mut failures,
    )?;
    check_bool("wall_acquired", wall_acquired, &mut failures)?;
    check_bool(
        "still_logic_after_wall",
        still_logic_after_wall,
        &mut failures,
    )?;
    let expected_tick = as_u64(fixture, &probe, "tick")? as i64;
    if acquire_tick != expected_tick {
        failures.push(format!(
            "tick: java 158.1 = {expected_tick}, rust = {acquire_tick}"
        ));
    }

    // --- P0-C3 boundary / refresh / unbind ---------------------------------
    if !unbind_logic_before {
        failures.push(
            "unbind scenario incomplete: rust did not acquire the lease with `ucontrol move`"
                .to_string(),
        );
    }
    // A released unit reports NaN in the probe and `None` here; both compare
    // by the paired `*_controlled_*` booleans, so an absent timer only
    // matters where java published a number.
    let check_timer =
        |field: &str, actual: Option<f32>, failures: &mut Vec<String>| -> Result<(), String> {
            let expected = lease_num(fixture, &probe, field)? as f32;
            match actual {
                Some(value) if (value - expected).abs() < 0.0001 => {}
                actual => failures.push(format!(
                    "{field}: java 158.1 = {expected}, rust = {}",
                    actual.map_or("released".to_string(), |v| v.to_string())
                )),
            }
            Ok(())
        };
    check_timer(
        "lease_timer_at_acquire",
        lease_timer_at_acquire,
        &mut failures,
    )?;
    check_timer("lease_timer_at_599", lease_timer_599, &mut failures)?;
    check_bool(
        "lease_controlled_at_599",
        lease_controlled_599,
        &mut failures,
    )?;
    check_timer("lease_timer_at_600", lease_timer_600, &mut failures)?;
    check_bool(
        "lease_controlled_at_600",
        lease_controlled_600,
        &mut failures,
    )?;
    check_bool(
        "lease_controlled_at_601",
        lease_controlled_601,
        &mut failures,
    )?;
    check_timer("refresh_timer_before", refresh_timer_before, &mut failures)?;
    check_timer("refresh_timer_after", refresh_timer_after, &mut failures)?;
    check_bool(
        "refresh_controlled_at_600",
        refresh_controlled_600,
        &mut failures,
    )?;
    check_bool(
        "refresh_controlled_at_601",
        refresh_controlled_601,
        &mut failures,
    )?;
    check_timer("gate_timer_after", gate_timer_after, &mut failures)?;
    check_bool("gate_flag_written", gate_flag_written, &mut failures)?;
    check_bool("gate_controlled_at_600", gate_controlled_600, &mut failures)?;
    check_bool("gate_controlled_at_601", gate_controlled_601, &mut failures)?;
    check_bool("unbind_logic_after", unbind_logic_after, &mut failures)?;
    check_bool("unbind_unit_var_kept", unbind_unit_var_kept, &mut failures)?;
    check_bool("unbind_has_command", unbind_has_command, &mut failures)?;
    check_bool(
        "unbind_logic_controllable",
        unbind_logic_controllable,
        &mut failures,
    )?;
    check_bool("unbind_reacquired", unbind_reacquired, &mut failures)?;
    check_timer(
        "unbind_reacquire_timer",
        unbind_reacquire_timer,
        &mut failures,
    )?;
    check_bool("rts_same_controller", rts_same_controller, &mut failures)?;
    check_bool("rts_became_logic", rts_became_logic, &mut failures)?;
    check_bool(
        "rts_has_command_after",
        rts_has_command_after,
        &mut failures,
    )?;
    check_bool("rts_unit_var_kept", rts_unit_var_kept, &mut failures)?;

    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' logic lease lifecycle diverges: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Logic-takeover probe (ParLogicTakeover158): first LogicAI wipe of mining
// and BuilderComp.plans, refresh that does not repeat it, failed gate no-op
// ---------------------------------------------------------------------------

#[test]
fn logic_owner_matches_java_1581() {
    // Differential P0-C1: same-tile processor replacement must drop the
    // old Logic lease (Building.isValid / tile.build == this) and a new
    // ucontrol from the replacement acquires a fresh instance.
    compare_logic_owner_fixture(&fixture("logic-owner.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
