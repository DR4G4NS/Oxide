//! Parity differential probes — status domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicWorld;

use crate::network::world::UnitAuthority;

use serde_json::Value;

use super::{fixture, parity_bare_world, require_fields, validate_common};

pub(super) fn compare_status_fixture(fixture: &Value) -> Result<(), String> {
    use crate::game::status::{
        DynamicStatus, STATUS_BLASTED, STATUS_BOSS, STATUS_BURNING, STATUS_CORRODED,
        STATUS_DISARMED, STATUS_DYNAMIC, STATUS_FREEZING, STATUS_MELTING, STATUS_OVERDRIVE,
        STATUS_SAPPED, STATUS_SHOCKED, STATUS_TARRED, STATUS_WET,
    };
    use crate::network::units::{tick_unit_statuses_with_floor, StatusContainer, ATRAX, DAGGER};
    use crate::network::world::EnemyUnit;
    use serde_json::json;

    let probe = validate_common(fixture)?;
    const CASES: &[&str] = &[
        "burning_tarred",
        "tarred_burning",
        "melting_tarred",
        "tarred_melting",
        "wet_shocked",
        "freezing_blasted",
        "opposites",
        "first_match",
        "corroded_timer",
        "corroded_phases",
        "corroded_delta_gt_interval",
        "corroded_expiry_on_fire",
        "disarmed",
        "overdrive",
        "boss",
        "infinity",
        "dynamic",
        "floor",
        "hovering",
    ];
    require_fields(fixture, &probe, CASES)?;

    fn parity_status_world() -> DynamicWorld {
        let mut world = parity_bare_world("parity-status.json");
        world.width = 16;
        world.height = 16;
        world.floors = vec![0; (world.width * world.height) as usize];
        world.overlays = vec![0; (world.width * world.height) as usize];
        let set_floor = |world: &mut DynamicWorld, x: i32, y: i32, floor: i16| {
            world.floors[(y * world.width + x) as usize] = floor;
        };
        set_floor(&mut world, 5, 5, 42);
        set_floor(&mut world, 6, 5, 22);
        set_floor(&mut world, 7, 5, 30);
        world
    }

    fn unit_at(spec: crate::network::units::EnemySpec, tile_x: i32, tile_y: i32) -> EnemyUnit {
        EnemyUnit {
            id: 1,
            unit_type: spec.unit_type,
            entity_class: spec.entity_class,
            team: 2,
            x: (tile_x as f32) * 8.0,
            y: (tile_y as f32) * 8.0,
            rotation: 0.0,
            health: spec.health,
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

    fn dump(unit: &EnemyUnit) -> StatusDump {
        let agg = StatusContainer::status_aggregate(unit);
        StatusDump {
            ids: unit.statuses.iter().map(|entry| entry.effect).collect(),
            times: unit.statuses.iter().map(|entry| entry.time).collect(),
            damage_times: unit
                .statuses
                .iter()
                .map(|entry| entry.damage_time)
                .collect(),
            health: unit.health,
            health_mult: agg.health,
            speed: agg.speed,
            damage: agg.damage,
            reload: agg.reload,
            build_speed: agg.build_speed,
            drag: agg.drag,
            armor_override: agg.armor_override,
            disarmed: agg.disarmed,
            can_shoot: !agg.disarmed,
            extra: serde_json::Map::new(),
        }
    }

    fn apply(unit: &mut EnemyUnit, effect: i16, time: f32) {
        StatusContainer::apply_status(unit, effect, time);
    }

    let world = parity_status_world();
    let mut cases: serde_json::Map<String, Value> = serde_json::Map::new();

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_BURNING, 200.0);
    apply(&mut u, STATUS_TARRED, 200.0);
    cases.insert("burning_tarred".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_TARRED, 200.0);
    apply(&mut u, STATUS_BURNING, 200.0);
    cases.insert("tarred_burning".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_MELTING, 150.0);
    apply(&mut u, STATUS_TARRED, 100.0);
    cases.insert("melting_tarred".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_TARRED, 150.0);
    apply(&mut u, STATUS_MELTING, 100.0);
    cases.insert("tarred_melting".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_WET, 60.0);
    apply(&mut u, STATUS_SHOCKED, 1.0);
    cases.insert("wet_shocked".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_FREEZING, 60.0);
    apply(&mut u, STATUS_BLASTED, 1.0);
    cases.insert("freezing_blasted".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_BURNING, 10.0);
    apply(&mut u, STATUS_WET, 20.0);
    cases.insert("opposites".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_SAPPED, 50.0);
    apply(&mut u, STATUS_BURNING, 10.0);
    apply(&mut u, STATUS_WET, 20.0);
    cases.insert("first_match".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_CORRODED, 100.0);
    StatusContainer::tick_statuses(&mut u, 1.0);
    cases.insert("corroded_timer".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_CORRODED, 100.0);
    StatusContainer::tick_statuses(&mut u, 14.0);
    let hp14 = u.health;
    let dt14 = u
        .statuses
        .iter()
        .find(|entry| entry.effect == STATUS_CORRODED)
        .map(|entry| entry.damage_time)
        .unwrap_or(0.0);
    StatusContainer::tick_statuses(&mut u, 1.0);
    let hp15 = u.health;
    let dt15 = u
        .statuses
        .iter()
        .find(|entry| entry.effect == STATUS_CORRODED)
        .map(|entry| entry.damage_time)
        .unwrap_or(0.0);
    StatusContainer::tick_statuses(&mut u, 15.0);
    let mut d = dump(&u);
    d.extra.insert("hp_after_14".into(), json!(hp14));
    d.extra.insert("dt_after_14".into(), json!(dt14));
    d.extra.insert("hp_after_15".into(), json!(hp15));
    d.extra.insert("dt_after_15".into(), json!(dt15));
    d.extra
        .insert("fired_at_15".into(), json!(hp15 < hp14 - 0.01));
    d.extra
        .insert("fired_at_30".into(), json!(u.health < hp15 - 0.01));
    cases.insert("corroded_phases".into(), json_dump(&d));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_CORRODED, 100.0);
    StatusContainer::tick_statuses(&mut u, 40.0);
    cases.insert("corroded_delta_gt_interval".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_CORRODED, 5.0);
    let before = u.health;
    StatusContainer::tick_statuses(&mut u, 10.0);
    let mut d = dump(&u);
    d.extra
        .insert("fired_on_expiry".into(), json!(u.health < before - 0.01));
    cases.insert("corroded_expiry_on_fire".into(), json_dump(&d));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_DISARMED, 60.0);
    StatusContainer::tick_statuses(&mut u, 1.0);
    cases.insert("disarmed".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_OVERDRIVE, 1.0);
    StatusContainer::tick_statuses(&mut u, 1.0);
    cases.insert("overdrive".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_BOSS, 1.0);
    StatusContainer::tick_statuses(&mut u, 1.0);
    cases.insert("boss".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_OVERDRIVE, f32::INFINITY);
    apply(&mut u, STATUS_BOSS, 0.0);
    StatusContainer::tick_statuses(&mut u, 1_000_000.0);
    cases.insert("infinity".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 10, 10);
    apply(&mut u, STATUS_DYNAMIC, f32::INFINITY);
    if let Some(entry) = u.statuses.first_mut() {
        entry.dynamic = Some(DynamicStatus {
            speed: 2.0,
            health: 0.5,
            damage: 3.0,
            reload: 0.25,
            build_speed: 4.0,
            drag: 1.5,
            armor_override: Some(10.0),
        });
    }
    StatusContainer::tick_statuses(&mut u, 1.0);
    cases.insert("dynamic".into(), json_dump(&dump(&u)));

    let mut u = unit_at(DAGGER, 5, 5);
    tick_unit_statuses_with_floor(&mut u, &world, 1.0);
    cases.insert("floor".into(), json_dump(&dump(&u)));

    let mut u = unit_at(ATRAX, 5, 5);
    tick_unit_statuses_with_floor(&mut u, &world, 1.0);
    cases.insert("hovering".into(), json_dump(&dump(&u)));

    let mut failures = Vec::new();
    for name in CASES {
        let java = fixture.get(*name).ok_or_else(|| {
            format!("parity error: fixture '{probe}' is missing required field '{name}'")
        })?;
        let rust = cases
            .get(*name)
            .ok_or_else(|| format!("parity error: rust dump missing case '{name}'"))?;
        if let Err(error) = compare_status_case(name, java, rust) {
            failures.push(error);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "parity mismatch: fixture '{probe}' status semantics diverge: {}",
            failures.join("; ")
        ))
    }
}

struct StatusDump {
    ids: Vec<i16>,
    times: Vec<f32>,
    damage_times: Vec<f32>,
    health: f32,
    health_mult: f32,
    speed: f32,
    damage: f32,
    reload: f32,
    build_speed: f32,
    drag: f32,
    armor_override: Option<f32>,
    disarmed: bool,
    can_shoot: bool,
    extra: serde_json::Map<String, Value>,
}

fn json_dump(dump: &StatusDump) -> Value {
    use serde_json::json;
    let mut map = serde_json::Map::new();
    map.insert("ids".into(), json!(dump.ids));
    map.insert(
        "times".into(),
        json!(dump
            .times
            .iter()
            .map(|time| encode_status_time(*time))
            .collect::<Vec<_>>()),
    );
    map.insert("damage_times".into(), json!(dump.damage_times));
    map.insert("health".into(), json!(dump.health));
    map.insert(
        "health_mult".into(),
        if dump.health_mult.is_infinite() {
            Value::Null
        } else {
            json!(dump.health_mult)
        },
    );
    map.insert(
        "health_infinite".into(),
        json!(dump.health_mult.is_infinite()),
    );
    map.insert("speed".into(), json!(dump.speed));
    map.insert("damage".into(), json!(dump.damage));
    map.insert("reload".into(), json!(dump.reload));
    map.insert("build_speed".into(), json!(dump.build_speed));
    map.insert("drag".into(), json!(dump.drag));
    map.insert(
        "armor_override".into(),
        dump.armor_override
            .map_or(Value::Null, |armor| json!(armor)),
    );
    map.insert("disarmed".into(), json!(dump.disarmed));
    map.insert("can_shoot".into(), json!(dump.can_shoot));
    for (key, value) in &dump.extra {
        map.insert(key.clone(), value.clone());
    }
    Value::Object(map)
}

fn encode_status_time(time: f32) -> f64 {
    if time.is_infinite() {
        1e38
    } else {
        f64::from(time)
    }
}

fn compare_status_case(name: &str, java: &Value, rust: &Value) -> Result<(), String> {
    let fields = [
        "ids",
        "times",
        "damage_times",
        "health",
        "health_mult",
        "health_infinite",
        "speed",
        "damage",
        "reload",
        "build_speed",
        "drag",
        "armor_override",
        "disarmed",
        "can_shoot",
    ];
    for field in fields {
        compare_status_value(name, field, &java[field], &rust[field])?;
    }
    if let Some(object) = java.as_object() {
        for key in object.keys() {
            if fields.contains(&key.as_str()) {
                continue;
            }
            compare_status_value(name, key, &java[key], &rust[key])?;
        }
    }
    Ok(())
}

fn compare_status_value(case: &str, field: &str, java: &Value, rust: &Value) -> Result<(), String> {
    let path = format!("{case}.{field}");
    match (java, rust) {
        (Value::Array(java_items), Value::Array(rust_items)) => {
            if java_items.len() != rust_items.len() {
                return Err(format!(
                    "{path}: java 158.1 len = {}, rust len = {}",
                    java_items.len(),
                    rust_items.len()
                ));
            }
            for (index, (left, right)) in java_items.iter().zip(rust_items).enumerate() {
                compare_status_value(case, &format!("{field}[{index}]"), left, right)?;
            }
            Ok(())
        }
        (Value::Bool(left), Value::Bool(right)) if left == right => Ok(()),
        (Value::Null, Value::Null) => Ok(()),
        (left, right) if numbers_close(left, right) => Ok(()),
        _ => Err(format!("{path}: java 158.1 = {java}, rust = {rust}")),
    }
}

fn numbers_close(java: &Value, rust: &Value) -> bool {
    let Some(left) = java.as_f64() else {
        return false;
    };
    let Some(right) = rust.as_f64() else {
        return false;
    };
    if left.abs() >= 1e30 && right.abs() >= 1e30 {
        return left.is_sign_positive() == right.is_sign_positive();
    }
    (left - right).abs() <= 1e-3f64.max(left.abs() * 1e-5)
}

#[test]
fn status_matches_java_1581() {
    compare_status_fixture(&fixture("status.json")).unwrap_or_else(|error| panic!("{error}"));
}
