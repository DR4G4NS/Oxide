//! Parity differential probes — move_timing domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicWorld;

use serde_json::Value;
use std::sync::Arc;

use super::{fixture, parity_bare_world, validate_common};

use super::logic_takeover::drive_takeover_tick;

use super::logic_owner::stamp_micro_processor;

fn move_timing_float(block: &serde_json::Map<String, Value>, field: &str) -> Result<f32, String> {
    match block.get(field) {
        Some(Value::Number(number)) => number
            .as_f64()
            .map(|v| v as f32)
            .ok_or_else(|| format!("parity error: field '{field}' is not a number")),
        Some(Value::Null) => Ok(f32::NAN),
        None => Err(format!("parity error: missing field '{field}'")),
        _ => Err(format!(
            "parity error: field '{field}' must be number or null"
        )),
    }
}

fn compare_move_phase(
    fixture: &Value,
    probe: &str,
    scenario: &str,
    phase: &str,
    actual: crate::network::units::LogicMovementSnapshot,
) -> Result<(), String> {
    let block = fixture
        .get(scenario)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("parity error: fixture '{probe}' missing scenario '{scenario}'"))?;
    let expect = block
        .get(phase)
        .and_then(Value::as_object)
        .ok_or_else(|| format!("parity error: fixture '{probe}' missing '{scenario}.{phase}'"))?;
    let prefix = format!("{scenario}.{phase}");
    let expect_control = expect
        .get("control")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("parity error: missing '{prefix}.control'"))?;
    let expect_is_logic = expect
        .get("is_logic")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("parity error: missing '{prefix}.is_logic'"))?;
    let expect_processor = expect
        .get("processor_valid")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("parity error: missing '{prefix}.processor_valid'"))?;
    let (x, y, vel_x, vel_y, control, move_x, move_y, is_logic, processor_valid) = actual;
    let mut failures = Vec::new();
    const EPS: f32 = 0.002;
    for (name, actual, expected) in [
        ("x", x, move_timing_float(expect, "x")?),
        ("y", y, move_timing_float(expect, "y")?),
        ("vel_x", vel_x, move_timing_float(expect, "vel_x")?),
        ("vel_y", vel_y, move_timing_float(expect, "vel_y")?),
    ] {
        if (actual - expected).abs() > EPS {
            failures.push(format!(
                "{prefix}.{name}: java 158.1 = {expected:.4}, rust = {actual:.4}"
            ));
        }
    }
    if control != expect_control {
        failures.push(format!(
            "{prefix}.control: java 158.1 = {expect_control}, rust = {control}"
        ));
    }
    let expect_move_x = move_timing_float(expect, "move_x")?;
    let expect_move_y = move_timing_float(expect, "move_y")?;
    let actual_move_x = move_x.unwrap_or(f32::NAN);
    let actual_move_y = move_y.unwrap_or(f32::NAN);
    if actual_move_x.is_nan() != expect_move_x.is_nan()
        || (!actual_move_x.is_nan() && (actual_move_x - expect_move_x).abs() > EPS)
    {
        failures.push(format!(
            "{prefix}.move_x: java 158.1 = {expect_move_x:?}, rust = {actual_move_x:?}"
        ));
    }
    if actual_move_y.is_nan() != expect_move_y.is_nan()
        || (!actual_move_y.is_nan() && (actual_move_y - expect_move_y).abs() > EPS)
    {
        failures.push(format!(
            "{prefix}.move_y: java 158.1 = {expect_move_y:?}, rust = {actual_move_y:?}"
        ));
    }
    if is_logic != expect_is_logic {
        failures.push(format!(
            "{prefix}.is_logic: java 158.1 = {expect_is_logic}, rust = {is_logic}"
        ));
    }
    if processor_valid != expect_processor {
        failures.push(format!(
            "{prefix}.processor_valid: java 158.1 = {expect_processor}, rust = {processor_valid}"
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn compare_logic_move_timing_fixture(fixture: &Value) -> Result<(), String> {
    use crate::logic::{compile, ExecutorState};
    use crate::network::units::{
        apply_logic_unit_movement, logic_movement_snapshot, release_logic_control, DAGGER, FLARE,
    };
    use crate::network::world::{logic_control, UnitAuthority};

    let probe = validate_common(fixture)?;
    for scenario in [
        "flying",
        "grounded",
        "stop_to_move",
        "move_to_stop",
        "proc_destroyed",
    ] {
        for phase in [
            "n_minus_1",
            "n_after_ucontrol",
            "end_n",
            "end_n_plus_1",
            "end_n_plus_2",
        ] {
            if fixture.get(scenario).and_then(|s| s.get(phase)).is_none() {
                return Err(format!(
                    "parity error: fixture '{probe}' missing '{scenario}.{phase}'"
                ));
            }
        }
    }

    let processor_pos = (7 << 16) | 7;
    let team = 1u8;
    let start_x = fixture
        .get("start_x")
        .and_then(Value::as_f64)
        .unwrap_or(80.0) as f32;
    let start_y = fixture
        .get("start_y")
        .and_then(Value::as_f64)
        .unwrap_or(80.0) as f32;
    let target_x = fixture
        .get("target_x")
        .and_then(Value::as_f64)
        .unwrap_or(200.0) as f32;
    let target_y = fixture
        .get("target_y")
        .and_then(Value::as_f64)
        .unwrap_or(80.0) as f32;

    fn make_unit(
        id: i32,
        spec: crate::network::units::EnemySpec,
        elevation: f32,
        x: f32,
        y: f32,
    ) -> crate::network::world::EnemyUnit {
        crate::network::world::EnemyUnit {
            id,
            unit_type: spec.unit_type,
            entity_class: spec.entity_class,
            team: 1,
            x,
            y,
            rotation: 0.0,
            health: spec.health,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation,
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
            authority: UnitAuthority::Command,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        }
    }

    let drive_unit = |world: &DynamicWorld, unit_id: i32| {
        crate::network::simulation::simulate_logic_control_leases(world, 1.0);
        let snapshot = world.enemies.get(&unit_id).unwrap().clone();
        apply_logic_unit_movement(world, &snapshot, 1.0);
    };

    let drive_ucontrol = |world: &DynamicWorld, processor_pos: i32, unit_id: i32, src: &str| {
        let program = compile(&format!("{src}\nstop"))
            .unwrap_or_else(|| panic!("parity error: logic-move-timing must compile"));
        let mut state = ExecutorState::new(program, Vec::new());
        state.bound_unit = Some(unit_id);
        drive_takeover_tick(world, processor_pos, &mut state);
    };

    let acquire = |world: &DynamicWorld, unit_id: i32| {
        release_logic_control(world, unit_id);
        if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
            unit.authority = UnitAuthority::Command;
            unit.velocity_x = 0.0;
            unit.velocity_y = 0.0;
            unit.x = start_x;
            unit.y = start_y;
        }
        drive_ucontrol(world, processor_pos, unit_id, "ucontrol flag 0");
    };

    let set_idle = |world: &DynamicWorld, unit_id: i32| {
        if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
            order.logic_control = logic_control::STOP;
            order.target_x = None;
            order.target_y = None;
        }
    };

    let trace_move = |world: &DynamicWorld,
                      unit_id: i32|
     -> [Option<crate::network::units::LogicMovementSnapshot>; 5] {
        drive_unit(world, unit_id);
        let n_minus_1 = logic_movement_snapshot(world, unit_id);
        drive_unit(world, unit_id);
        drive_ucontrol(
            world,
            processor_pos,
            unit_id,
            &format!("ucontrol move {target_x} {target_y}"),
        );
        let n_after = logic_movement_snapshot(world, unit_id);
        drive_unit(world, unit_id);
        let end_n_plus_1 = logic_movement_snapshot(world, unit_id);
        drive_unit(world, unit_id);
        let end_n_plus_2 = logic_movement_snapshot(world, unit_id);
        [n_minus_1, n_after, n_after, end_n_plus_1, end_n_plus_2]
    };

    let mut failures = Vec::new();
    let world = Arc::new(parity_bare_world("parity-logic-move-timing.json"));
    stamp_micro_processor(&world, processor_pos, team);

    let mut run_simple = |world: &DynamicWorld,
                          unit_id: i32,
                          spec: crate::network::units::EnemySpec,
                          elevation: f32,
                          scenario: &str| {
        world.enemies.insert(
            unit_id,
            make_unit(unit_id, spec, elevation, start_x, start_y),
        );
        world.unit_orders.insert(unit_id, default_order(unit_id));
        acquire(world, unit_id);
        set_idle(world, unit_id);
        let phases = trace_move(world, unit_id);
        let names = [
            "n_minus_1",
            "n_after_ucontrol",
            "end_n",
            "end_n_plus_1",
            "end_n_plus_2",
        ];
        for (phase, snap) in names.into_iter().zip(phases) {
            if let Some(actual) = snap {
                if let Err(message) = compare_move_phase(fixture, &probe, scenario, phase, actual) {
                    failures.push(message);
                }
            } else {
                failures.push(format!("{scenario}.{phase}: rust snapshot missing"));
            }
        }
    };

    run_simple(&world, 3_000_201, FLARE, 1.0, "flying");
    run_simple(&world, 3_000_202, DAGGER, 0.0, "grounded");

    // stop → move
    {
        let unit_id = 3_000_203;
        world
            .enemies
            .insert(unit_id, make_unit(unit_id, DAGGER, 0.0, start_x, start_y));
        world.unit_orders.insert(unit_id, default_order(unit_id));
        acquire(&world, unit_id);
        drive_ucontrol(&world, processor_pos, unit_id, "ucontrol stop");
        drive_unit(&world, unit_id);
        let phases = trace_move(&world, unit_id);
        let names = [
            "n_minus_1",
            "n_after_ucontrol",
            "end_n",
            "end_n_plus_1",
            "end_n_plus_2",
        ];
        for (phase, snap) in names.into_iter().zip(phases) {
            if let Some(actual) = snap {
                if let Err(message) =
                    compare_move_phase(fixture, &probe, "stop_to_move", phase, actual)
                {
                    failures.push(message);
                }
            }
        }
    }

    // move → stop (pre-move two ticks so n_minus_1 matches fixture motion)
    {
        let unit_id = 3_000_204;
        world
            .enemies
            .insert(unit_id, make_unit(unit_id, DAGGER, 0.0, start_x, start_y));
        world.unit_orders.insert(unit_id, default_order(unit_id));
        acquire(&world, unit_id);
        drive_ucontrol(
            &world,
            processor_pos,
            unit_id,
            &format!("ucontrol move {target_x} {target_y}"),
        );
        drive_unit(&world, unit_id);
        drive_unit(&world, unit_id);
        drive_unit(&world, unit_id);
        let n_minus_1 = logic_movement_snapshot(&world, unit_id);
        drive_unit(&world, unit_id);
        drive_ucontrol(&world, processor_pos, unit_id, "ucontrol stop");
        let n_after = logic_movement_snapshot(&world, unit_id);
        drive_unit(&world, unit_id);
        let end_n_plus_1 = logic_movement_snapshot(&world, unit_id);
        drive_unit(&world, unit_id);
        let end_n_plus_2 = logic_movement_snapshot(&world, unit_id);
        let phases = [
            ("n_minus_1", n_minus_1),
            ("n_after_ucontrol", n_after),
            ("end_n", n_after),
            ("end_n_plus_1", end_n_plus_1),
            ("end_n_plus_2", end_n_plus_2),
        ];
        for (phase, snap) in phases {
            if let Some(actual) = snap {
                if let Err(message) =
                    compare_move_phase(fixture, &probe, "move_to_stop", phase, actual)
                {
                    failures.push(message);
                }
            }
        }
    }

    // processor destroyed after ucontrol move
    {
        let unit_id = 3_000_205;
        world
            .enemies
            .insert(unit_id, make_unit(unit_id, DAGGER, 0.0, start_x, start_y));
        world.unit_orders.insert(unit_id, default_order(unit_id));
        acquire(&world, unit_id);
        set_idle(&world, unit_id);
        drive_unit(&world, unit_id);
        let n_minus_1 = logic_movement_snapshot(&world, unit_id);
        drive_unit(&world, unit_id);
        drive_ucontrol(
            &world,
            processor_pos,
            unit_id,
            &format!("ucontrol move {target_x} {target_y}"),
        );
        world.tiles.remove(&processor_pos);
        let n_after = logic_movement_snapshot(&world, unit_id);
        drive_unit(&world, unit_id);
        let end_n_plus_1 = logic_movement_snapshot(&world, unit_id);
        drive_unit(&world, unit_id);
        let end_n_plus_2 = logic_movement_snapshot(&world, unit_id);
        let phases = [
            ("n_minus_1", n_minus_1),
            ("n_after_ucontrol", n_after),
            ("end_n", n_after),
            ("end_n_plus_1", end_n_plus_1),
            ("end_n_plus_2", end_n_plus_2),
        ];
        for (phase, snap) in phases {
            if let Some(actual) = snap {
                if let Err(message) =
                    compare_move_phase(fixture, &probe, "proc_destroyed", phase, actual)
                {
                    failures.push(message);
                }
            }
        }
        stamp_micro_processor(&world, processor_pos, team);
    }

    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' logic-move-timing diverges: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Bullet/status/weapon timing probe (ParBulletStatusTiming158): N/N+1/N+2
// ---------------------------------------------------------------------------

fn default_order(unit_id: i32) -> crate::network::world::UnitOrder {
    crate::network::world::UnitOrder {
        unit_id,
        ..Default::default()
    }
}

#[test]
fn logic_move_timing_matches_java_1581() {
    // P1-B1: ucontrol move affects movement on N+1 (velocity) and N+2
    // (position); stop/move boundaries and processor-destroy release.
    compare_logic_move_timing_fixture(&fixture("logic-move-timing.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
