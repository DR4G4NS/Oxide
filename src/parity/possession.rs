//! Parity differential probes — possession domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicWorld;

use serde_json::Value;

use super::{fixture, parity_bare_world, require_fields, validate_common};

use super::ubind::ubind_probe_unit;

fn compare_possession_fixture(fixture: &Value) -> Result<(), String> {
    use crate::network::units::{
        detach_unit_control, queue_unit_target, switch_player_unit, unit_is_logic_controllable,
        unit_order_has_active_rts_target,
    };
    use crate::network::world::{
        ControlledUnit, PlayerCombatState, SessionPlayer, UnitAuthority, UnitOrder, UnitOrderTarget,
    };
    use serde_json::json;

    let probe = validate_common(fixture)?;
    let world = parity_bare_world("parity-possession.json");
    let fresh_order = |unit_id: i32, command: u8| UnitOrder {
        unit_id,
        command,
        stances: 0,
        payload_cooldown: 0.0,
        target_kind: 0,
        target_id: -1,
        target_x: None,
        target_y: None,
        logic_control: 0,
        queue: Vec::new(),
    };
    let player = |id: i32| SessionPlayer {
        id,
        controlled_unit: ControlledUnit::Core,
        unit_id: 2_600_000 + id,
        uuid: format!("parity-possession-{id}"),
        name: "parity".into(),
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
        last_shot: std::time::Instant::now(),
        admin: false,
        chat_rate: crate::network::wire::ChatRateLimiter::new(),
    };
    let add_combat = |world: &DynamicWorld, player: &SessionPlayer, team: u8| {
        world.players.insert(
            player.unit_id,
            PlayerCombatState {
                uuid: player.uuid.clone(),
                player_id: player.id,
                unit_id: player.unit_id,
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
    };
    let add_unit =
        |world: &DynamicWorld, id: i32, unit_type: i16, team: u8, command: Option<u8>| {
            let mut unit = ubind_probe_unit(id, team, 0.0);
            unit.unit_type = unit_type;
            unit.authority = if command.is_some() {
                UnitAuthority::Command
            } else {
                UnitAuthority::DefaultAi
            };
            world.enemies.insert(id, unit);
            if let Some(command) = command {
                world.unit_orders.insert(id, fresh_order(id, command));
            }
        };
    let authority = |world: &DynamicWorld, id: i32| {
        world
            .enemies
            .get(&id)
            .map_or(UnitAuthority::DefaultAi, |unit| unit.authority)
    };
    let controller = |world: &DynamicWorld, id: i32| -> i64 {
        match authority(world, id) {
            UnitAuthority::Player { .. } => 0,
            UnitAuthority::Command => 1,
            _ => 2,
        }
    };
    let command = |world: &DynamicWorld, id: i32| -> i64 {
        if authority(world, id) == UnitAuthority::Command {
            world
                .unit_orders
                .get(&id)
                .map_or(-1, |order| i64::from(order.command))
        } else {
            -1
        }
    };
    let queue = |world: &DynamicWorld, id: i32| -> i64 {
        if authority(world, id) == UnitAuthority::Command {
            world
                .unit_orders
                .get(&id)
                .map_or(0, |order| order.queue.len() as i64)
        } else {
            -1
        }
    };
    let target = |world: &DynamicWorld, id: i32| -> i64 {
        if authority(world, id) != UnitAuthority::Command {
            -1
        } else if world
            .unit_orders
            .get(&id)
            .is_some_and(|order| unit_order_has_active_rts_target(&order))
        {
            1
        } else {
            0
        }
    };
    let stances = |world: &DynamicWorld, id: i32| -> i64 {
        if authority(world, id) == UnitAuthority::Command {
            world
                .unit_orders
                .get(&id)
                .map_or(0, |order| i64::from(order.stances))
        } else {
            -1
        }
    };
    let team = |world: &DynamicWorld, id: i32| -> i64 {
        world
            .enemies
            .get(&id)
            .map_or(-1, |unit| i64::from(unit.team))
    };

    let mega = 3_010_001;
    let poly = 3_010_002;
    let flare = 3_010_003;
    add_unit(&world, mega, 22, 1, Some(1));
    add_unit(&world, poly, 21, 1, Some(2));
    add_unit(&world, flare, 9, 2, None);
    let mut main_player = player(31);
    add_combat(&world, &main_player, 1);

    world.unit_orders.get_mut(&mega).unwrap().stances = 1 << 2;
    queue_unit_target(
        &mut world.unit_orders.get_mut(&mega).unwrap(),
        UnitOrderTarget {
            kind: 0,
            id: -1,
            x: 100.0,
            y: 100.0,
        },
    );
    queue_unit_target(
        &mut world.unit_orders.get_mut(&mega).unwrap(),
        UnitOrderTarget {
            kind: 0,
            id: -1,
            x: 150.0,
            y: 150.0,
        },
    );
    let mut actual = serde_json::Map::new();
    actual.insert("a_initial_command".into(), json!(1));
    actual.insert(
        "a_setup_has_command".into(),
        json!(unit_order_has_active_rts_target(
            &world.unit_orders.get(&mega).unwrap()
        )),
    );
    actual.insert("a_setup_queue".into(), json!(1));
    actual.insert("a_setup_stance".into(), json!(true));

    switch_player_unit(&world, &mut main_player, Some(mega));
    actual.insert(
        "possess_ctrl_player".into(),
        json!(controller(&world, mega) == 0),
    );
    actual.insert(
        "possess_last_command".into(),
        json!(main_player.last_command.map_or(-1, i64::from)),
    );
    switch_player_unit(&world, &mut main_player, None);
    actual.insert(
        "release_ctrl_command_ai".into(),
        json!(controller(&world, mega) == 1),
    );
    actual.insert("release_command".into(), json!(command(&world, mega)));
    actual.insert("release_queue".into(), json!(queue(&world, mega)));
    actual.insert(
        "release_has_command".into(),
        json!(target(&world, mega) != 0),
    );
    actual.insert("release_target".into(), json!(target(&world, mega)));
    actual.insert("release_stances".into(), json!(stances(&world, mega)));
    actual.insert(
        "release_stances_empty".into(),
        json!(stances(&world, mega) == 0),
    );
    actual.insert(
        "release_stance_gone".into(),
        json!(stances(&world, mega) & 4 == 0),
    );
    actual.insert(
        "release_logic_controllable".into(),
        json!(unit_is_logic_controllable(&world, mega)),
    );

    switch_player_unit(&world, &mut main_player, Some(mega));
    actual.insert("b_initial_command".into(), json!(command(&world, poly)));
    switch_player_unit(&world, &mut main_player, Some(poly));
    actual.insert(
        "switch_ctrl_player".into(),
        json!(controller(&world, poly) == 0),
    );
    actual.insert(
        "switch_mega_is_command_ai".into(),
        json!(controller(&world, mega) == 1),
    );
    actual.insert("switch_mega_command".into(), json!(command(&world, mega)));
    actual.insert(
        "switch_player_last_command".into(),
        json!(main_player.last_command.map_or(-1, i64::from)),
    );
    switch_player_unit(&world, &mut main_player, None);
    actual.insert(
        "b_release_ctrl_command_ai".into(),
        json!(controller(&world, poly) == 1),
    );
    actual.insert("b_release_command".into(), json!(command(&world, poly)));
    actual.insert("b_release_queue".into(), json!(queue(&world, poly)));

    switch_player_unit(&world, &mut main_player, Some(mega));
    world.enemies.get_mut(&mega).unwrap().team = 5;
    actual.insert("team_propagated".into(), json!(team(&world, mega) == 5));
    actual.insert(
        "team_possession_survives".into(),
        json!(controller(&world, mega) == 0),
    );
    world.enemies.get_mut(&mega).unwrap().team = 1;
    switch_player_unit(&world, &mut main_player, None);

    actual.insert(
        "wave_ctrl_command_ai_before".into(),
        json!(controller(&world, flare) == 1),
    );
    let before_wave = main_player.last_command;
    switch_player_unit(&world, &mut main_player, Some(flare));
    actual.insert(
        "wave_possess_ctrl_player".into(),
        json!(controller(&world, flare) == 0),
    );
    actual.insert("wave_possess_team".into(), json!(team(&world, flare)));
    actual.insert(
        "wave_last_command_unchanged".into(),
        json!(main_player.last_command == before_wave),
    );
    switch_player_unit(&world, &mut main_player, None);
    actual.insert(
        "wave_release_ctrl_command_ai".into(),
        json!(controller(&world, flare) == 1),
    );
    actual.insert("wave_release_command".into(), json!(command(&world, flare)));
    actual.insert("wave_release_queue".into(), json!(queue(&world, flare)));
    actual.insert("wave_release_target".into(), json!(target(&world, flare)));
    actual.insert("wave_release_stances".into(), json!(stances(&world, flare)));
    actual.insert("wave_release_team".into(), json!(team(&world, flare)));

    let same_a = 3_010_010;
    let same_b = 3_010_011;
    add_unit(&world, same_a, 21, 1, Some(0));
    add_unit(&world, same_b, 21, 1, Some(2));
    for (id, stance) in [(same_a, 1 << 2), (same_b, 1 << 1)] {
        let mut order = world.unit_orders.get_mut(&id).unwrap();
        order.stances = stance;
        queue_unit_target(
            &mut order,
            UnitOrderTarget {
                kind: 0,
                id: -1,
                x: 128.0,
                y: 128.0,
            },
        );
        queue_unit_target(
            &mut order,
            UnitOrderTarget {
                kind: 0,
                id: -1,
                x: 136.0,
                y: 136.0,
            },
        );
    }
    actual.insert("same_initial_a_command".into(), json!(0));
    actual.insert("same_initial_b_command".into(), json!(2));
    actual.insert("same_move_supported".into(), json!(true));
    actual.insert("same_rebuild_supported".into(), json!(true));
    let mut same_player = player(32);
    add_combat(&world, &same_player, 1);
    switch_player_unit(&world, &mut same_player, Some(same_a));
    switch_player_unit(&world, &mut same_player, Some(same_b));
    for (name, value) in [
        ("same_switch_a_controller", controller(&world, same_a)),
        ("same_switch_a_command", command(&world, same_a)),
        ("same_switch_a_queue", queue(&world, same_a)),
        ("same_switch_a_target", target(&world, same_a)),
        ("same_switch_a_stances", stances(&world, same_a)),
        ("same_switch_a_team", team(&world, same_a)),
        ("same_switch_b_controller", controller(&world, same_b)),
        ("same_switch_b_team", team(&world, same_b)),
        (
            "same_switch_last_command",
            same_player.last_command.map_or(-1, i64::from),
        ),
    ] {
        actual.insert(name.into(), json!(value));
    }
    switch_player_unit(&world, &mut same_player, Some(same_a));
    for (name, value) in [
        ("same_back_a_controller", controller(&world, same_a)),
        ("same_back_a_team", team(&world, same_a)),
        ("same_back_b_controller", controller(&world, same_b)),
        ("same_back_b_command", command(&world, same_b)),
        ("same_back_b_queue", queue(&world, same_b)),
        ("same_back_b_target", target(&world, same_b)),
        ("same_back_b_stances", stances(&world, same_b)),
        ("same_back_b_team", team(&world, same_b)),
        (
            "same_back_last_command",
            same_player.last_command.map_or(-1, i64::from),
        ),
    ] {
        actual.insert(name.into(), json!(value));
    }
    switch_player_unit(&world, &mut same_player, None);

    let team_unit = 3_010_020;
    add_unit(&world, team_unit, 21, 1, Some(0));
    let mut team_player = player(33);
    add_combat(&world, &team_player, 1);
    switch_player_unit(&world, &mut team_player, Some(team_unit));
    world.players.get_mut(&team_player.unit_id).unwrap().team = 5;
    world.enemies.get_mut(&team_unit).unwrap().team = 5;
    switch_player_unit(&world, &mut team_player, None);
    for (name, value) in [
        ("team_release_controller", controller(&world, team_unit)),
        ("team_release_command", command(&world, team_unit)),
        ("team_release_queue", queue(&world, team_unit)),
        ("team_release_target", target(&world, team_unit)),
        ("team_release_stances", stances(&world, team_unit)),
        ("team_release_team", team(&world, team_unit)),
        (
            "team_release_last_command",
            team_player.last_command.map_or(-1, i64::from),
        ),
    ] {
        actual.insert(name.into(), json!(value));
    }

    let disconnect_unit = 3_010_030;
    add_unit(&world, disconnect_unit, 21, 1, Some(0));
    let mut disconnect_player = player(34);
    add_combat(&world, &disconnect_player, 1);
    switch_player_unit(&world, &mut disconnect_player, Some(disconnect_unit));
    switch_player_unit(&world, &mut disconnect_player, None);
    actual.insert(
        "disconnect_player_unit_null".into(),
        json!(disconnect_player.controlled_unit == ControlledUnit::Core),
    );
    for (name, value) in [
        ("disconnect_controller", controller(&world, disconnect_unit)),
        ("disconnect_command", command(&world, disconnect_unit)),
        ("disconnect_queue", queue(&world, disconnect_unit)),
        ("disconnect_target", target(&world, disconnect_unit)),
        ("disconnect_stances", stances(&world, disconnect_unit)),
        ("disconnect_team", team(&world, disconnect_unit)),
        (
            "disconnect_last_command",
            disconnect_player.last_command.map_or(-1, i64::from),
        ),
    ] {
        actual.insert(name.into(), json!(value));
    }

    let death_unit = 3_010_040;
    add_unit(&world, death_unit, 21, 1, Some(2));
    let mut death_player = player(35);
    add_combat(&world, &death_player, 1);
    switch_player_unit(&world, &mut death_player, Some(death_unit));
    // Java resets the detached object's controller before it becomes
    // unreachable; record that ephemeral state, then mirror Rust removal.
    switch_player_unit(&world, &mut death_player, None);
    actual.insert(
        "death_player_unit_null".into(),
        json!(death_player.controlled_unit == ControlledUnit::Core),
    );
    for (name, value) in [
        ("death_controller", controller(&world, death_unit)),
        ("death_command", command(&world, death_unit)),
        ("death_queue", queue(&world, death_unit)),
        ("death_target", target(&world, death_unit)),
        ("death_stances", stances(&world, death_unit)),
        ("death_team", team(&world, death_unit)),
        (
            "death_last_command",
            death_player.last_command.map_or(-1, i64::from),
        ),
    ] {
        actual.insert(name.into(), json!(value));
    }
    world.enemies.remove(&death_unit);
    detach_unit_control(&world, death_unit);

    let required: Vec<&str> = actual.keys().map(String::as_str).collect();
    require_fields(fixture, &probe, &required)?;
    let mut failures = Vec::new();
    for (field, rust) in actual {
        let java = fixture.get(&field).expect("required field checked");
        if java != &rust {
            failures.push(format!("{field}: java 158.1 = {java}, rust = {rust}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "parity mismatch: fixture '{probe}' possession semantics diverge: {}",
            failures.join("; ")
        ))
    }
}

#[test]
fn possession_matches_java_1581() {
    // Differential P0-05: the player <-> unit possession lifecycle —
    // lastCommand save/restore, destruction of the CommandAI object state
    // (queue, targets, stances) across a possess/release round-trip, the
    // switch ordering, possess-time team adoption and the non-CommandAI
    // possession path — replayed by the Rust authority model.
    compare_possession_fixture(&fixture("ParPossess158.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
