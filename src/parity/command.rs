//! Parity differential probes — command domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicWorld;

use serde_json::Value;
use std::sync::Arc;

use super::{as_bool, fixture, parity_bare_world, require_fields, tile_at_pos, validate_common};

use super::logic_takeover::drive_takeover_tick;

use super::lease::lease_num;

use super::logic_owner::stamp_micro_processor;

use super::ubind::ubind_probe_unit;

pub(super) fn compare_command_fixture(fixture: &Value) -> Result<(), String> {
    use crate::network::units::{
        clear_order_active_target, queue_unit_target, set_order_active_target,
        unit_is_logic_controllable,
    };
    use crate::network::world::{UnitAuthority, UnitOrder, UnitOrderTarget};

    let probe = validate_common(fixture)?;
    let required: Vec<&str> = vec![
        "promote_has_command",
        "promote_queue_size",
        "promote_target_x",
        "promote_target_y",
        "promote_logic_controllable",
        "dedup_after_second",
        "dedup_after_third",
        "cap_queue_size",
        "cap_has_command",
        "building_queue_before",
        "building_queue_after_destroy",
        "attack_active_after_update_has_command",
        "attack_active_after_destroy_has_command",
        "attack_active_logic_after_destroy",
        "unit_queue_before",
        "unit_queue_after_remove",
        "exhaust_blocked_logic",
        "exhaust_after_first_has_command",
        "exhaust_restored_logic",
        "exhaust_final_has_command",
    ];
    // Arrival fields are generated names; checked in a second pass.
    let arrival_fields: Vec<String> = (0..4)
        .flat_map(|step| {
            ["has_command", "queue_size", "target_x", "target_y"]
                .iter()
                .map(move |what| format!("arrival_{step}_{what}"))
        })
        .collect();
    let arrival_refs: Vec<&str> = arrival_fields.iter().map(String::as_str).collect();
    require_fields(fixture, &probe, &required)?;
    require_fields(fixture, &probe, &arrival_refs)?;

    let world = parity_bare_world("parity-command-queue.json");
    let unit_id = 3_000_001;
    let foe_id = 3_000_002;
    let wall_position = (8 << 16) | 8;
    {
        let mut probe_unit = ubind_probe_unit(unit_id, 1, 0.0); // sharded flare stand-in
        probe_unit.x = 80.0;
        probe_unit.y = 80.0;
        // A sharded player-commandable unit's factory controller IS a
        // CommandAI: Command authority without an active target.
        probe_unit.authority = UnitAuthority::Command;
        world.enemies.insert(unit_id, probe_unit);
        let mut foe = ubind_probe_unit(foe_id, 6, 0.0); // crux dagger stand-in
        foe.x = 120.0;
        foe.y = 120.0;
        world.enemies.insert(foe_id, foe);
        // The CommandAI state (fresh: no command, empty queue).
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
                logic_control: 0,
                queue: Vec::new(),
            },
        );
    }
    let insert_wall = |world: &DynamicWorld| {
        let mut wall = tile_at_pos(wall_position, 22); // copperWall
        wall.team = 6; // crux
        world.tiles.insert(wall_position, wall);
    };

    let order = |world: &DynamicWorld| -> UnitOrder {
        world
            .unit_orders
            .get(&unit_id)
            .map(|order| order.clone())
            .unwrap_or_else(|| UnitOrder {
                unit_id,
                command: 0,
                stances: 0,
                payload_cooldown: 0.0,
                target_kind: 0,
                target_id: -1,
                target_x: None,
                target_y: None,
                logic_control: 0,
                queue: Vec::new(),
            })
    };
    let clear = |world: &DynamicWorld| {
        if let Some(mut live) = world.unit_orders.get_mut(&unit_id) {
            clear_order_active_target(&mut live);
            live.queue.clear();
        }
    };
    let queue_target = |world: &DynamicWorld, target: UnitOrderTarget| {
        let mut live = world.unit_orders.get_mut(&unit_id).unwrap();
        queue_unit_target(&mut live, target)
    };
    let set_active = |world: &DynamicWorld, target: UnitOrderTarget| {
        let mut live = world.unit_orders.get_mut(&unit_id).unwrap();
        set_order_active_target(&mut live, target)
    };
    let position = |x: f32, y: f32| UnitOrderTarget {
        kind: 0,
        id: -1,
        x,
        y,
    };
    let teleport = |world: &DynamicWorld, x: f32, y: f32| {
        if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
            unit.x = x;
            unit.y = y;
        }
    };
    // One CommandAI update tick: the ordered-movement path is the port's
    // CommandAI.update (queue sweep, target validity, finishPath).
    let tick = |world: &DynamicWorld| {
        let snapshot = world.enemies.get(&unit_id).unwrap().clone();
        crate::network::units::unit_orders::apply_ordered_unit_movement(world, &snapshot, 1.0)
    };
    let has_command =
        |world: &DynamicWorld| crate::network::units::unit_has_active_rts_command(world, unit_id);

    // --- A: first queued command is promoted to active -------------------
    queue_target(&world, position(100.0, 100.0));
    let a_has = has_command(&world);
    let a_queue = order(&world).queue.len();
    let (a_x, a_y) = (
        order(&world).target_x.unwrap_or(f64::NAN as f32),
        order(&world).target_y.unwrap_or(f64::NAN as f32),
    );
    let a_logic = unit_is_logic_controllable(&world, unit_id);

    // --- B: dedup of distinct-but-equal Vec2 instances --------------------
    queue_target(&world, position(100.0, 100.0));
    let b_after_second = order(&world).queue.len();
    queue_target(&world, position(100.0, 100.0));
    let b_after_third = order(&world).queue.len();

    // --- C: queue cap ------------------------------------------------------
    clear(&world);
    set_active(&world, position(200.0, 200.0));
    for i in 0..60_i32 {
        queue_target(&world, position(300.0 + i as f32 * 7.0, 300.0));
    }
    let c_queue = order(&world).queue.len();
    let c_has = has_command(&world);

    // --- D: arrival pops the queue in FIFO order ---------------------------
    let mut d_has = [false; 4];
    let mut d_queue = [0_usize; 4];
    let mut d_x = [0.0_f32; 4];
    let mut d_y = [0.0_f32; 4];
    for step in 0..4 {
        let current = order(&world);
        teleport(
            &world,
            current.target_x.unwrap_or(0.0),
            current.target_y.unwrap_or(0.0),
        );
        tick(&world);
        let after = order(&world);
        d_has[step] = has_command(&world);
        d_queue[step] = after.queue.len();
        d_x[step] = after.target_x.unwrap_or(f64::NAN as f32);
        d_y[step] = after.target_y.unwrap_or(f64::NAN as f32);
    }

    // --- E: building targets ------------------------------------------------
    clear(&world);
    insert_wall(&world);
    set_active(&world, position(200.0, 200.0));
    queue_target(
        &world,
        UnitOrderTarget {
            kind: 1,
            id: wall_position,
            x: 68.0,
            y: 68.0,
        },
    );
    queue_target(&world, position(400.0, 400.0));
    let e_before = order(&world).queue.len();
    world.tiles.remove(&wall_position); // destroyed
    tick(&world);
    let e_after = order(&world).queue.len();

    // Active attack building target: compared post-update (T2+). The Java
    // T1 transient (attackTarget-only) is documented in command-timing.json.
    insert_wall(&world);
    clear(&world);
    set_active(
        &world,
        UnitOrderTarget {
            kind: 1,
            id: wall_position,
            x: 68.0,
            y: 68.0,
        },
    );
    tick(&world);
    let e_active_after = has_command(&world);
    world.tiles.remove(&wall_position); // destroyed
    tick(&world);
    let e_active_destroyed = has_command(&world);
    let e_active_logic = unit_is_logic_controllable(&world, unit_id);

    // --- F: queued enemy unit -----------------------------------------------
    clear(&world);
    set_active(&world, position(200.0, 200.0));
    queue_target(
        &world,
        UnitOrderTarget {
            kind: 2,
            id: foe_id,
            x: 120.0,
            y: 120.0,
        },
    );
    let f_before = order(&world).queue.len();
    world.enemies.remove(&foe_id); // Groups removal -> invalid
    tick(&world);
    let f_after = order(&world).queue.len();

    // --- G: exhausting the queue restores logic control ----------------------
    clear(&world);
    queue_target(&world, position(80.0, 80.0)); // promoted: active
    queue_target(&world, position(90.0, 90.0)); // queued
    let g_blocked = unit_is_logic_controllable(&world, unit_id);
    teleport(&world, 80.0, 80.0);
    tick(&world);
    let g_after_first = has_command(&world);
    teleport(&world, 90.0, 90.0);
    tick(&world);
    let g_restored = unit_is_logic_controllable(&world, unit_id);
    let g_final_has = has_command(&world);

    // --- comparison -----------------------------------------------------------
    let mut failures = Vec::new();
    let check_bool = |failures: &mut Vec<String>, field: &str, actual: bool| {
        let expected = as_bool(fixture, &probe, field).unwrap_or_default();
        if actual != expected {
            failures.push(format!("{field}: java 158.1 = {expected}, rust = {actual}"));
        }
    };
    let check_num = |failures: &mut Vec<String>, field: &str, actual: f64| {
        let expected = lease_num(fixture, &probe, field).unwrap_or_default();
        if (actual - expected).abs() > 0.0001 {
            failures.push(format!("{field}: java 158.1 = {expected}, rust = {actual}"));
        }
    };
    check_bool(&mut failures, "promote_has_command", a_has);
    check_num(&mut failures, "promote_queue_size", a_queue as f64);
    check_num(&mut failures, "promote_target_x", f64::from(a_x));
    check_num(&mut failures, "promote_target_y", f64::from(a_y));
    check_bool(&mut failures, "promote_logic_controllable", a_logic);
    check_num(&mut failures, "dedup_after_second", b_after_second as f64);
    check_num(&mut failures, "dedup_after_third", b_after_third as f64);
    check_num(&mut failures, "cap_queue_size", c_queue as f64);
    check_bool(&mut failures, "cap_has_command", c_has);
    for step in 0..4 {
        check_bool(
            &mut failures,
            &format!("arrival_{step}_has_command"),
            d_has[step],
        );
        check_num(
            &mut failures,
            &format!("arrival_{step}_queue_size"),
            d_queue[step] as f64,
        );
        check_num(
            &mut failures,
            &format!("arrival_{step}_target_x"),
            f64::from(d_x[step]),
        );
        check_num(
            &mut failures,
            &format!("arrival_{step}_target_y"),
            f64::from(d_y[step]),
        );
    }
    check_num(&mut failures, "building_queue_before", e_before as f64);
    check_num(
        &mut failures,
        "building_queue_after_destroy",
        e_after as f64,
    );
    check_bool(
        &mut failures,
        "attack_active_after_update_has_command",
        e_active_after,
    );
    check_bool(
        &mut failures,
        "attack_active_after_destroy_has_command",
        e_active_destroyed,
    );
    check_bool(
        &mut failures,
        "attack_active_logic_after_destroy",
        e_active_logic,
    );
    check_num(&mut failures, "unit_queue_before", f_before as f64);
    check_num(&mut failures, "unit_queue_after_remove", f_after as f64);
    check_bool(&mut failures, "exhaust_blocked_logic", g_blocked);
    check_bool(
        &mut failures,
        "exhaust_after_first_has_command",
        g_after_first,
    );
    check_bool(&mut failures, "exhaust_restored_logic", g_restored);
    check_bool(&mut failures, "exhaust_final_has_command", g_final_has);
    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' CommandAI semantics diverge: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Command timing probe (ParCommandTiming158): attackTarget/targetPos transient
// ---------------------------------------------------------------------------

fn compare_command_timing_fixture(fixture: &Value) -> Result<(), String> {
    use crate::logic::{compile, ExecutorState};
    use crate::network::units::unit_orders::apply_ordered_unit_movement;
    use crate::network::units::{
        acquire_command_control, clear_order_active_target, set_order_active_target,
        unit_has_active_rts_command, unit_is_logic_controllable,
    };
    use crate::network::world::{UnitAuthority, UnitOrder, UnitOrderTarget};

    let probe = validate_common(fixture)?;
    for scenario in ["building", "unit", "vec2", "invalid"] {
        for phase in ["t2", "t3", "t4"] {
            timing_phase(fixture, &probe, scenario, phase)?;
        }
    }
    timing_phase(fixture, &probe, "vec2", "t1")?;

    let world = Arc::new(parity_bare_world("parity-command-timing.json"));
    let unit_id = 3_000_001;
    let foe_id = 3_000_002;
    let processor_pos = (7 << 16) | 7;
    let wall_position = (8 << 16) | 8;
    let team = 1u8;

    {
        let mut flare = ubind_probe_unit(unit_id, team, 0.0);
        flare.unit_type = 15; // flare — flying; avoids ControlPathfinder in bare world
        flare.elevation = 1.0;
        flare.x = 80.0;
        flare.y = 80.0;
        flare.authority = UnitAuthority::Command;
        world.enemies.insert(unit_id, flare);
        let mut foe = ubind_probe_unit(foe_id, 6, 0.0);
        foe.x = 120.0;
        foe.y = 120.0;
        world.enemies.insert(foe_id, foe);
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
                logic_control: 0,
                queue: Vec::new(),
            },
        );
        stamp_micro_processor(&world, processor_pos, team);
    }

    let insert_wall = |world: &DynamicWorld| {
        let mut wall = tile_at_pos(wall_position, 22);
        wall.team = 6;
        world.tiles.insert(wall_position, wall);
    };

    let reset_unit = |world: &DynamicWorld| {
        if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
            clear_order_active_target(&mut order);
            order.queue.clear();
            order.command = 0;
        }
        if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
            unit.authority = UnitAuthority::Command;
        }
        crate::network::units::release_logic_control(world, unit_id);
    };

    let snapshot = |world: &DynamicWorld| -> (bool, bool) {
        let has_command = world.enemies.get(&unit_id).is_some_and(|unit| {
            unit.authority == UnitAuthority::Command && unit_has_active_rts_command(world, unit_id)
        });
        (has_command, unit_is_logic_controllable(world, unit_id))
    };

    let unit_tick = |world: &DynamicWorld| {
        let snapshot = world.enemies.get(&unit_id).unwrap().clone();
        apply_ordered_unit_movement(world, &snapshot, 1.0);
    };

    let ucontrol_tick = |world: &DynamicWorld| -> bool {
        let program = compile("ucontrol move 50 50\nstop")
            .unwrap_or_else(|| panic!("parity error: command-timing ucontrol must compile"));
        let mut state = ExecutorState::new(program, Vec::new());
        state.bound_unit = Some(unit_id);
        drive_takeover_tick(world, processor_pos, &mut state);
        matches!(
            world.enemies.get(&unit_id).map(|unit| unit.authority),
            Some(UnitAuthority::Logic { .. })
        )
    };

    let issue_building = |world: &DynamicWorld| {
        insert_wall(world);
        if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
            set_order_active_target(
                &mut order,
                UnitOrderTarget {
                    kind: 1,
                    id: wall_position,
                    x: 68.0,
                    y: 68.0,
                },
            );
        }
        acquire_command_control(world, unit_id);
    };

    let issue_unit = |world: &DynamicWorld| {
        if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
            set_order_active_target(
                &mut order,
                UnitOrderTarget {
                    kind: 2,
                    id: foe_id,
                    x: 120.0,
                    y: 120.0,
                },
            );
        }
        acquire_command_control(world, unit_id);
    };

    let issue_vec2 = |world: &DynamicWorld| {
        if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
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
        acquire_command_control(world, unit_id);
    };

    let issue_invalid = |world: &DynamicWorld| {
        insert_wall(world);
        if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
            set_order_active_target(
                &mut order,
                UnitOrderTarget {
                    kind: 1,
                    id: wall_position,
                    x: 68.0,
                    y: 68.0,
                },
            );
        }
        acquire_command_control(world, unit_id);
        world.tiles.remove(&wall_position);
    };

    struct TimingRun {
        t1: (bool, bool),
        t2: (bool, bool),
        t3: (bool, bool, bool),
        t4: (bool, bool),
    }

    let run_scenario = |world: &DynamicWorld, issue: &dyn Fn(&DynamicWorld)| -> TimingRun {
        reset_unit(world);
        issue(world);
        let t1 = snapshot(world);
        unit_tick(world);
        let t2 = snapshot(world);
        let became_logic = ucontrol_tick(world);
        let t3_has = snapshot(world);
        unit_tick(world);
        let t4 = snapshot(world);
        TimingRun {
            t1,
            t2,
            t3: (t3_has.0, t3_has.1, became_logic),
            t4,
        }
    };

    let building = run_scenario(&world, &issue_building);
    let unit_target = run_scenario(&world, &issue_unit);
    let vec2 = run_scenario(&world, &issue_vec2);
    let invalid = run_scenario(&world, &issue_invalid);

    let mut failures = Vec::new();
    for (name, run) in [
        ("building", &building),
        ("unit", &unit_target),
        ("vec2", &vec2),
        ("invalid", &invalid),
    ] {
        for (phase, has, logic, ucontrol) in [
            ("t2", run.t2.0, run.t2.1, None),
            ("t3", run.t3.0, run.t3.1, Some(run.t3.2)),
            ("t4", run.t4.0, run.t4.1, None),
        ] {
            if let Err(message) =
                check_timing_phase(fixture, &probe, name, phase, has, logic, ucontrol)
            {
                failures.push(message);
            }
        }
    }
    if let Err(message) =
        check_timing_phase(fixture, &probe, "vec2", "t1", vec2.t1.0, vec2.t1.1, None)
    {
        failures.push(message);
    }

    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' command-timing diverges: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Logic move timing probe (ParLogicMoveTiming158): ucontrol move tick N/N+1/N+2
// ---------------------------------------------------------------------------

fn check_timing_phase(
    fixture: &Value,
    probe: &str,
    scenario: &str,
    phase: &str,
    actual_has: bool,
    actual_logic: bool,
    actual_ucontrol: Option<bool>,
) -> Result<(), String> {
    let prefix = format!("{scenario}.{phase}");
    let expect_has = timing_bool(fixture, probe, scenario, phase, "has_command")?;
    let expect_logic = timing_bool(fixture, probe, scenario, phase, "logic_controllable")?;
    let mut failures = Vec::new();
    if actual_has != expect_has {
        failures.push(format!(
            "{prefix}.has_command: java 158.1 = {expect_has}, rust = {actual_has}"
        ));
    }
    if actual_logic != expect_logic {
        failures.push(format!(
            "{prefix}.logic_controllable: java 158.1 = {expect_logic}, rust = {actual_logic}"
        ));
    }
    if let Some(actual_ucontrol) = actual_ucontrol {
        let expect_ucontrol =
            timing_bool(fixture, probe, scenario, phase, "ucontrol_became_logic")?;
        if actual_ucontrol != expect_ucontrol {
            failures.push(format!(
                "{prefix}.ucontrol_became_logic: java 158.1 = {expect_ucontrol}, rust = {actual_ucontrol}"
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn timing_phase(fixture: &Value, probe: &str, scenario: &str, phase: &str) -> Result<(), String> {
    let block = fixture
        .get(scenario)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            format!("parity error: fixture '{probe}' is missing scenario '{scenario}'")
        })?;
    let phase_value = block.get(phase).and_then(Value::as_object).ok_or_else(|| {
        format!("parity error: fixture '{probe}' is missing '{scenario}.{phase}'")
    })?;
    for field in ["has_command", "logic_controllable"] {
        if !phase_value.contains_key(field) {
            return Err(format!(
                "parity error: fixture '{probe}' is missing required field '{scenario}.{phase}.{field}'"
            ));
        }
    }
    if phase == "t3" && !phase_value.contains_key("ucontrol_became_logic") {
        return Err(format!(
            "parity error: fixture '{probe}' is missing required field '{scenario}.t3.ucontrol_became_logic'"
        ));
    }
    Ok(())
}

fn timing_bool(
    fixture: &Value,
    probe: &str,
    scenario: &str,
    phase: &str,
    field: &str,
) -> Result<bool, String> {
    fixture
        .get(scenario)
        .and_then(|scenario| scenario.get(phase))
        .and_then(|phase| phase.get(field))
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            format!("parity error: fixture '{probe}' is missing '{scenario}.{phase}.{field}'")
        })
}

// ---------------------------------------------------------------------------
// Possession probe (ParPossess158): player <-> unit lifecycle
// ---------------------------------------------------------------------------

#[test]
fn command_timing_matches_java_1581() {
    // P1-A1: CommandAI attackTarget/targetPos transient — Logic observes
    // post-unit-update state only (T2+); same-tick ucontrol gate matches.
    compare_command_timing_fixture(&fixture("command-timing.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn command_queue_matches_java_1581() {
    // Differential P0-04: first-queued-target promotion, Vec2 dedup, the
    // 50-entry cap, FIFO finishPath progression on arrival, queued/active
    // building and unit target invalidation, and hasCommand()/
    // isLogicControllable() across the whole sequence, replayed by the Rust
    // order model.
    compare_command_fixture(&fixture("command-queue.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
