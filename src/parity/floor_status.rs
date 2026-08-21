//! Parity differential probes — floor_status domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicWorld;

use crate::network::world::UnitAuthority;

use serde_json::Value;

use super::{compare_bool, fixture, parity_bare_world, require_fields, validate_common};

fn compare_floor_status_fixture(fixture: &Value) -> Result<(), String> {
    use crate::game::status::STATUS_BURNING;
    use crate::network::units::{tick_unit_statuses_with_floor, StatusContainer};
    use crate::network::world::EnemyUnit;

    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &[
            "a_muddy_duration",
            "a_has_muddy",
            "b_muddy_duration",
            "b_has_muddy",
            "c_after_one",
            "c_after_two",
            "d_wet_duration",
            "d_has_burning",
            "e_muddy_duration",
            "f_muddy_duration",
            "g_melt_duration",
            "g_has_melt",
            "h_muddy_duration",
        ],
    )?;

    fn parity_floor_world() -> DynamicWorld {
        let mut world = parity_bare_world("parity-floor-status.json");
        world.width = 16;
        world.height = 16;
        world.floors = vec![0; (world.width * world.height) as usize];
        world.overlays = vec![0; (world.width * world.height) as usize];
        let set_floor = |world: &mut DynamicWorld, x: i32, y: i32, floor: i16| {
            world.floors[(y * world.width + x) as usize] = floor;
        };
        set_floor(&mut world, 5, 5, 42); // mud
        set_floor(&mut world, 6, 5, 22); // water
        set_floor(&mut world, 7, 5, 30); // slag
        world
    }

    fn unit_at(tile_x: i32, tile_y: i32, unit_type: i16) -> EnemyUnit {
        EnemyUnit {
            id: 1,
            unit_type,
            entity_class: 0,
            team: 2,
            x: (tile_x as f32) * 8.0,
            y: (tile_y as f32) * 8.0,
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
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        }
    }

    fn muddy_duration(unit: &EnemyUnit) -> f32 {
        unit.statuses
            .iter()
            .find(|entry| entry.effect == 7)
            .map(|entry| entry.time)
            .unwrap_or(0.0)
    }

    fn wet_duration(unit: &EnemyUnit) -> f32 {
        unit.statuses
            .iter()
            .find(|entry| entry.effect == 6)
            .map(|entry| entry.time)
            .unwrap_or(0.0)
    }

    fn melt_duration(unit: &EnemyUnit) -> f32 {
        unit.statuses
            .iter()
            .find(|entry| entry.effect == 8)
            .map(|entry| entry.time)
            .unwrap_or(0.0)
    }

    fn has_effect(unit: &EnemyUnit, effect: i16) -> bool {
        unit.statuses.iter().any(|entry| entry.effect == effect)
    }

    let world = parity_floor_world();

    let mut dagger = unit_at(5, 5, 0);
    tick_unit_statuses_with_floor(&mut dagger, &world, 1.0);
    let a_muddy = muddy_duration(&dagger);
    let a_has = has_effect(&dagger, 7);

    let mut atrax = unit_at(5, 5, 11);
    tick_unit_statuses_with_floor(&mut atrax, &world, 1.0);
    let b_muddy = muddy_duration(&atrax);
    let b_has = has_effect(&atrax, 7);

    let mut extend = unit_at(5, 5, 0);
    StatusContainer::apply_status(&mut extend, 7, 10.0);
    tick_unit_statuses_with_floor(&mut extend, &world, 1.0);
    let c_one = muddy_duration(&extend);
    tick_unit_statuses_with_floor(&mut extend, &world, 1.0);
    let c_two = muddy_duration(&extend);

    let mut wet = unit_at(6, 5, 0);
    StatusContainer::apply_status(&mut wet, STATUS_BURNING, 5.0);
    tick_unit_statuses_with_floor(&mut wet, &world, 1.0);
    let d_wet = wet_duration(&wet);
    let d_burn = has_effect(&wet, STATUS_BURNING);

    let mut roam = unit_at(5, 5, 0);
    tick_unit_statuses_with_floor(&mut roam, &world, 1.0);
    roam.x = 80.0;
    roam.y = 40.0;
    for _ in 0..5 {
        tick_unit_statuses_with_floor(&mut roam, &world, 1.0);
    }
    let e_muddy = muddy_duration(&roam);
    roam.x = 40.0;
    roam.y = 40.0;
    tick_unit_statuses_with_floor(&mut roam, &world, 1.0);
    let f_muddy = muddy_duration(&roam);

    let mut precept = unit_at(7, 5, 40);
    tick_unit_statuses_with_floor(&mut precept, &world, 1.0);
    let g_melt = melt_duration(&precept);
    let g_has = has_effect(&precept, 8);

    let mut half = unit_at(5, 5, 0);
    tick_unit_statuses_with_floor(&mut half, &world, 0.5);
    let h_muddy = muddy_duration(&half);

    let compare_f32 = |field: &str, rust: f32, java: f64| -> Result<(), String> {
        if (f64::from(rust) - java).abs() > 1e-4 {
            Err(format!(
                "parity mismatch: field '{field}': java 158.1 = {java}, rust = {rust}"
            ))
        } else {
            Ok(())
        }
    };

    compare_f32(
        "a_muddy_duration",
        a_muddy,
        fixture["a_muddy_duration"].as_f64().unwrap(),
    )?;
    compare_bool(
        fixture,
        &probe,
        "a_has_muddy",
        a_has,
        fixture["a_has_muddy"].as_bool().unwrap(),
    )?;
    compare_f32(
        "b_muddy_duration",
        b_muddy,
        fixture["b_muddy_duration"].as_f64().unwrap(),
    )?;
    compare_bool(
        fixture,
        &probe,
        "b_has_muddy",
        b_has,
        fixture["b_has_muddy"].as_bool().unwrap(),
    )?;
    compare_f32(
        "c_after_one",
        c_one,
        fixture["c_after_one"].as_f64().unwrap(),
    )?;
    compare_f32(
        "c_after_two",
        c_two,
        fixture["c_after_two"].as_f64().unwrap(),
    )?;
    compare_f32(
        "d_wet_duration",
        d_wet,
        fixture["d_wet_duration"].as_f64().unwrap(),
    )?;
    compare_bool(
        fixture,
        &probe,
        "d_has_burning",
        d_burn,
        fixture["d_has_burning"].as_bool().unwrap(),
    )?;
    compare_f32(
        "e_muddy_duration",
        e_muddy,
        fixture["e_muddy_duration"].as_f64().unwrap(),
    )?;
    compare_f32(
        "f_muddy_duration",
        f_muddy,
        fixture["f_muddy_duration"].as_f64().unwrap(),
    )?;
    compare_f32(
        "g_melt_duration",
        g_melt,
        fixture["g_melt_duration"].as_f64().unwrap(),
    )?;
    compare_bool(
        fixture,
        &probe,
        "g_has_melt",
        g_has,
        fixture["g_has_melt"].as_bool().unwrap(),
    )?;
    compare_f32(
        "h_muddy_duration",
        h_muddy,
        fixture["h_muddy_duration"].as_f64().unwrap(),
    )?;
    Ok(())
}

#[test]
fn floor_status_matches_java_1581() {
    compare_floor_status_fixture(&fixture("floor-status.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
