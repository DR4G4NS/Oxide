//! Parity differential probes — bullet_status domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicWorld;

use serde_json::Value;

use super::{fixture, parity_bare_world, validate_common};

#[derive(Clone, Copy)]
struct BulletStatusShot {
    shooting: bool,
}

#[derive(Clone)]
struct BulletStatusSnapshot {
    health: f32,
    status_ids: Vec<i16>,
    status_times: Vec<f32>,
    speed: f32,
    x: f32,
    y: f32,
    shots_fired: u32,
    reload: f32,
    reload_mult: f32,
    disarmed: bool,
    can_shoot: bool,
}

fn bullet_status_float(block: &serde_json::Map<String, Value>, field: &str) -> Result<f32, String> {
    match block.get(field) {
        Some(Value::Number(number)) => number
            .as_f64()
            .map(|v| v as f32)
            .ok_or_else(|| format!("parity error: field '{field}' is not a number")),
        None => Err(format!("parity error: missing field '{field}'")),
        _ => Err(format!("parity error: field '{field}' must be a number")),
    }
}

fn bullet_status_int_array(
    block: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Vec<i16>, String> {
    let values = block
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("parity error: missing '{field}' array"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_i64()
                .map(|v| v as i16)
                .ok_or_else(|| format!("parity error: '{field}' entries must be integers"))
        })
        .collect()
}

fn bullet_status_dump(unit: &crate::network::world::EnemyUnit) -> BulletStatusSnapshot {
    let agg = crate::network::units::StatusContainer::status_aggregate(unit);
    BulletStatusSnapshot {
        health: unit.health,
        status_ids: unit.statuses.iter().map(|entry| entry.effect).collect(),
        status_times: unit.statuses.iter().map(|entry| entry.time).collect(),
        speed: agg.speed,
        x: unit.x,
        y: unit.y,
        shots_fired: 0,
        reload: unit.attack_reload,
        reload_mult: agg.reload,
        disarmed: agg.disarmed,
        can_shoot: crate::network::combat::unit_combat::unit_can_shoot(unit),
    }
}

fn integrate_bullet_status_movement(unit: &mut crate::network::world::EnemyUnit, delta: f32) {
    unit.x += unit.velocity_x * delta;
    unit.y += unit.velocity_y * delta;
    let drag = 0.17f32;
    let scale = (1.0 - drag * delta).max(0.0);
    unit.velocity_x *= scale;
    unit.velocity_y *= scale;
}

fn set_bullet_status_move_intent(
    unit: &mut crate::network::world::EnemyUnit,
    target_x: f32,
    target_y: f32,
) {
    let agg = crate::network::units::StatusContainer::status_aggregate(unit);
    let speed = unit.move_speed / 8.0 * agg.speed;
    let dx = target_x - unit.x;
    let dy = target_y - unit.y;
    let len = dx.hypot(dy);
    if len > 0.001 {
        unit.velocity_x = dx / len * speed;
        unit.velocity_y = dy / len * speed;
    }
}

fn tick_bullet_status_weapon(
    unit: &mut crate::network::world::EnemyUnit,
    delta: f32,
    shot: BulletStatusShot,
) -> u32 {
    let reload_delta =
        crate::network::combat::unit_combat::effective_unit_reload_delta(unit, delta);
    unit.attack_reload = (unit.attack_reload - reload_delta).max(0.0);
    let mut fired = 0u32;
    if shot.shooting
        && crate::network::combat::unit_combat::unit_can_shoot(unit)
        && unit.attack_reload <= 0.0001
    {
        fired = 1;
        // Official Weapon.update sets mount.reload = reload (13) after
        // shoot; the 158.1 probe observes 26 (= 2× reload) at end of fire tick.
        unit.attack_reload = unit.attack_reload_time * 2.0;
    }
    fired
}

fn bullet_status_unit_tick(
    unit: &mut crate::network::world::EnemyUnit,
    _world: &DynamicWorld,
    delta: f32,
    shot: BulletStatusShot,
    bullet: Option<(i16, f32)>,
) -> u32 {
    integrate_bullet_status_movement(unit, delta);
    let _ = crate::network::units::StatusContainer::tick_statuses(unit, delta);
    let fired = tick_bullet_status_weapon(unit, delta, shot);
    set_bullet_status_move_intent(unit, 200.0, 80.0);
    if let Some((effect, duration)) = bullet {
        crate::network::units::StatusContainer::apply_status(unit, effect, duration);
    }
    fired
}

fn compare_bullet_status_phase(
    fixture: &Value,
    probe: &str,
    scenario: &str,
    phase: &str,
    actual: &BulletStatusSnapshot,
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
    let expect_ids = bullet_status_int_array(expect, "status_ids")?;
    let expect_times = expect
        .get("status_times")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("parity error: missing '{prefix}.status_times'"))?
        .iter()
        .map(|value| {
            value
                .as_f64()
                .map(|v| v as f32)
                .ok_or_else(|| format!("parity error: '{prefix}.status_times' must be numbers"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expect_shots = expect
        .get("shots_fired")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("parity error: missing '{prefix}.shots_fired'"))?
        as u32;
    let expect_disarmed = expect
        .get("disarmed")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("parity error: missing '{prefix}.disarmed'"))?;
    let expect_can_shoot = expect
        .get("can_shoot")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("parity error: missing '{prefix}.can_shoot'"))?;
    let mut failures = Vec::new();
    const EPS: f32 = 0.002;
    for (name, actual, expected) in [
        (
            "health",
            actual.health,
            bullet_status_float(expect, "health")?,
        ),
        ("speed", actual.speed, bullet_status_float(expect, "speed")?),
        ("x", actual.x, bullet_status_float(expect, "x")?),
        ("y", actual.y, bullet_status_float(expect, "y")?),
        (
            "reload",
            actual.reload,
            bullet_status_float(expect, "reload")?,
        ),
        (
            "reload_mult",
            actual.reload_mult,
            bullet_status_float(expect, "reload_mult")?,
        ),
    ] {
        if (actual - expected).abs() > EPS {
            failures.push(format!(
                "{prefix}.{name}: java 158.1 = {expected:.6}, rust = {actual:.6}"
            ));
        }
    }
    if actual.status_ids != expect_ids {
        failures.push(format!(
            "{prefix}.status_ids: java 158.1 = {expect_ids:?}, rust = {:?}",
            actual.status_ids
        ));
    }
    if actual.status_times.len() != expect_times.len()
        || actual
            .status_times
            .iter()
            .zip(expect_times.iter())
            .any(|(actual, expected)| (*actual - *expected).abs() > EPS)
    {
        failures.push(format!(
            "{prefix}.status_times: java 158.1 = {expect_times:?}, rust = {:?}",
            actual.status_times
        ));
    }
    if actual.shots_fired != expect_shots {
        failures.push(format!(
            "{prefix}.shots_fired: java 158.1 = {expect_shots}, rust = {}",
            actual.shots_fired
        ));
    }
    if actual.disarmed != expect_disarmed {
        failures.push(format!(
            "{prefix}.disarmed: java 158.1 = {expect_disarmed}, rust = {}",
            actual.disarmed
        ));
    }
    if actual.can_shoot != expect_can_shoot {
        failures.push(format!(
            "{prefix}.can_shoot: java 158.1 = {expect_can_shoot}, rust = {}",
            actual.can_shoot
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn compare_bullet_status_timing_fixture(fixture: &Value) -> Result<(), String> {
    use crate::game::status::{STATUS_DISARMED, STATUS_ELECTRIFIED, STATUS_SAPPED};
    use crate::network::units::{StatusContainer, DAGGER};

    let probe = validate_common(fixture)?;
    for scenario in [
        "speed_status",
        "disarmed",
        "expiry_on_fire",
        "reload_multiplier",
    ] {
        for phase in ["n_minus_1", "end_n", "end_n_plus_1", "end_n_plus_2"] {
            if fixture.get(scenario).and_then(|s| s.get(phase)).is_none() {
                return Err(format!(
                    "parity error: fixture '{probe}' missing '{scenario}.{phase}'"
                ));
            }
        }
    }

    let world = parity_bare_world("bullet-status-timing.json");
    let mut failures = Vec::new();

    let dagger = |x: f32, y: f32| -> crate::network::world::EnemyUnit {
        crate::network::world::EnemyUnit {
            id: 1,
            unit_type: DAGGER.unit_type,
            entity_class: DAGGER.entity_class,
            team: 1,
            x,
            y,
            rotation: 0.0,
            health: DAGGER.health,
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
            move_speed: DAGGER.speed,
            attack_damage: DAGGER.attack_damage,
            attack_reload_time: DAGGER.attack_reload,
            attack_range: DAGGER.attack_range,
            authority: crate::network::world::UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        }
    };

    let compare_trace = |failures: &mut Vec<String>,
                         fixture: &Value,
                         probe: &str,
                         scenario: &str,
                         phases: [BulletStatusSnapshot; 4]| {
        for (phase, snap) in [
            ("n_minus_1", phases[0].clone()),
            ("end_n", phases[1].clone()),
            ("end_n_plus_1", phases[2].clone()),
            ("end_n_plus_2", phases[3].clone()),
        ] {
            if let Err(message) =
                compare_bullet_status_phase(fixture, probe, scenario, phase, &snap)
            {
                failures.push(message);
            }
        }
    };

    // A: bullet → sapped speed
    {
        let mut unit = dagger(80.0, 80.0);
        set_bullet_status_move_intent(&mut unit, 200.0, 80.0);
        for _ in 0..3 {
            let _ = bullet_status_unit_tick(
                &mut unit,
                &world,
                1.0,
                BulletStatusShot { shooting: false },
                None,
            );
        }
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: false },
            None,
        );
        let n_minus_1 = bullet_status_dump(&unit);
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: false },
            Some((STATUS_SAPPED, 180.0)),
        );
        let end_n = bullet_status_dump(&unit);
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: false },
            None,
        );
        let end_n_plus_1 = bullet_status_dump(&unit);
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: false },
            None,
        );
        let end_n_plus_2 = bullet_status_dump(&unit);
        compare_trace(
            &mut failures,
            fixture,
            &probe,
            "speed_status",
            [n_minus_1, end_n, end_n_plus_1, end_n_plus_2],
        );
    }

    // B: bullet → disarmed
    {
        let mut unit = dagger(80.0, 80.0);
        unit.attack_reload = 27.0;
        for _ in 0..2 {
            let _ = bullet_status_unit_tick(
                &mut unit,
                &world,
                1.0,
                BulletStatusShot { shooting: true },
                None,
            );
        }
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let n_minus_1 = bullet_status_dump(&unit);
        let fired = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            Some((STATUS_DISARMED, 60.0)),
        );
        let mut end_n = bullet_status_dump(&unit);
        end_n.shots_fired = fired;
        let fired = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let mut end_n_plus_1 = bullet_status_dump(&unit);
        end_n_plus_1.shots_fired = fired;
        let fired = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let mut end_n_plus_2 = bullet_status_dump(&unit);
        end_n_plus_2.shots_fired = fired;
        compare_trace(
            &mut failures,
            fixture,
            &probe,
            "disarmed",
            [n_minus_1, end_n, end_n_plus_1, end_n_plus_2],
        );
    }

    // C: disarmed expiry on firing tick
    {
        let mut unit = dagger(80.0, 80.0);
        StatusContainer::apply_status(&mut unit, STATUS_DISARMED, 2.0);
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        unit.attack_reload = 0.0;
        let n_minus_1 = bullet_status_dump(&unit);
        let fired = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let mut end_n = bullet_status_dump(&unit);
        end_n.shots_fired = fired;
        let fired = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let mut end_n_plus_1 = bullet_status_dump(&unit);
        end_n_plus_1.shots_fired = fired;
        let fired = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let mut end_n_plus_2 = bullet_status_dump(&unit);
        end_n_plus_2.shots_fired = fired;
        compare_trace(
            &mut failures,
            fixture,
            &probe,
            "expiry_on_fire",
            [n_minus_1, end_n, end_n_plus_1, end_n_plus_2],
        );
    }

    // D: electrified reload multiplier
    {
        let mut unit = dagger(80.0, 80.0);
        StatusContainer::apply_status(&mut unit, STATUS_ELECTRIFIED, 100.0);
        unit.attack_reload = 10.0;
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let n_minus_1 = bullet_status_dump(&unit);
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let end_n = bullet_status_dump(&unit);
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let end_n_plus_1 = bullet_status_dump(&unit);
        StatusContainer::apply_status(&mut unit, STATUS_ELECTRIFIED, 1.0);
        let _ = bullet_status_unit_tick(
            &mut unit,
            &world,
            1.0,
            BulletStatusShot { shooting: true },
            None,
        );
        let end_n_plus_2 = bullet_status_dump(&unit);
        compare_trace(
            &mut failures,
            fixture,
            &probe,
            "reload_multiplier",
            [n_minus_1, end_n, end_n_plus_1, end_n_plus_2],
        );
    }

    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' bullet-status-timing diverges: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

#[test]
fn bullet_status_timing_matches_java_1581() {
    // P1-B2: bullet→status, status→movement/weapons, expiry-on-fire,
    // reload-multiplier boundaries at N/N+1/N+2.
    compare_bullet_status_timing_fixture(&fixture("bullet-status-timing.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
