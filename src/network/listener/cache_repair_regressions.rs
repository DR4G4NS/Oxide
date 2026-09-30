//! Isolated regressions for repair weapon authority and world-local path fields.
use super::*;
use crate::network::combat::enemy::{
    build_navigation_field_toward, navigation_field_toward, NavigationClass,
};

fn open_world(width: i32, height: i32) -> DynamicWorld {
    let (mut world, _, _, _) = legacy_weapons_test_world();
    world.width = width;
    world.height = height;
    world.base_blocks = vec![0; (width * height) as usize];
    world.base_centers = vec![false; (width * height) as usize];
    world.tile_data = vec![0; (width * height) as usize];
    world.floors = vec![0; (width * height) as usize];
    world.overlays = vec![0; (width * height) as usize];
    world
}

fn repair_world(team: u8, hold_fire: bool, disarmed: bool) -> (DynamicWorld, i32, i32) {
    let world = open_world(20, 20);
    *world.game_state.mode.write() = GameMode::Pvp;
    let position = (10 << 16) | 10;
    world.tiles.insert(
        position,
        DynamicTile {
            position,
            block: 216,
            team,
            health: 100.0,
            occupied: vec![position],
            ..Default::default()
        },
    );
    let id = 3_950_031;
    let mut unit = legacy_weapons_make_enemy(id, enemy_spec(31).unwrap(), 48.0, 80.0, 560.0);
    unit.team = team;
    unit.attack_reload = 5.0;
    if disarmed {
        crate::network::units::StatusContainer::apply_status(&mut unit, 20, 1000.0);
    }
    world.enemies.insert(id, unit);
    world.unit_orders.insert(
        id,
        UnitOrder {
            unit_id: id,
            command: 0,
            stances: if hold_fire { 1 << 1 } else { 0 },
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: 0,
            queue: Vec::new(),
        },
    );
    (world, id, position)
}

#[test]
fn f09_oxynoe_repair_keeps_actual_team_and_heals() {
    for team in [1, 5] {
        let (world, _, position) = repair_world(team, false, false);
        simulate_allied_units(&world, &crate::network::outbound::NOOP, 1.0);
        assert!(!world.projectiles.is_empty(), "repair weapon did not fire");
        assert!(
            world.projectiles.iter().all(|p| p.team == team),
            "repair changed team"
        );
        for _ in 0..20 {
            simulate_projectiles(&world, &crate::network::outbound::NOOP, 1.0);
        }
        assert!(
            world.tiles.get(&position).unwrap().health > 100.0,
            "own-team repair must heal"
        );
    }
}

#[test]
fn f09_oxynoe_repair_keeps_actual_owner() {
    let (world, id, _) = repair_world(1, false, false);
    simulate_allied_units(&world, &crate::network::outbound::NOOP, 1.0);
    assert!(!world.projectiles.is_empty());
    assert!(
        world.projectiles.iter().all(|p| p.shooter_id == id),
        "repair lost owner"
    );
}

#[test]
fn f09_oxynoe_hold_fire_blocks_repair_weapon() {
    let (world, _, position) = repair_world(5, true, false);
    simulate_allied_units(&world, &crate::network::outbound::NOOP, 10.0);
    assert!(
        world.projectiles.is_empty(),
        "hold-fire repair weapon fired"
    );
    assert_eq!(world.tiles.get(&position).unwrap().health, 100.0);
}

#[test]
fn f09_oxynoe_disarmed_blocks_repair_weapon() {
    let (world, _, position) = repair_world(5, false, true);
    simulate_allied_units(&world, &crate::network::outbound::NOOP, 10.0);
    assert!(world.projectiles.is_empty(), "disarmed repair weapon fired");
    assert_eq!(world.tiles.get(&position).unwrap().health, 100.0);
}

#[test]
fn f10_position_fields_do_not_cross_world_dimensions() {
    let first = open_world(3, 3);
    let second = open_world(5, 4);
    let a = navigation_field_toward(&first, NavigationClass::Ground, 1, 1, 1);
    let b = navigation_field_toward(&second, NavigationClass::Ground, 1, 1, 1);
    assert_eq!(a.len(), 9);
    assert_eq!(b.len(), 20, "new world reused previous world's field");
    assert_eq!(
        *b,
        build_navigation_field_toward(&second, NavigationClass::Ground, 1, 1, 1)
    );
}

#[test]
fn f10_position_fields_do_not_cross_same_size_worlds() {
    let first = open_world(5, 5);
    let second = open_world(5, 5);
    let position = (2 << 16) | 2;
    second.tiles.insert(
        position,
        DynamicTile {
            position,
            block: 216,
            team: 1,
            health: 100.0,
            occupied: vec![position],
            ..Default::default()
        },
    );
    let a = navigation_field_toward(&first, NavigationClass::Ground, 1, 1, 1);
    let b = navigation_field_toward(&second, NavigationClass::Ground, 1, 1, 1);
    assert_ne!(*a, *b, "different terrain must produce different fields");
    assert_eq!(
        *b,
        build_navigation_field_toward(&second, NavigationClass::Ground, 1, 1, 1)
    );
    assert!(Arc::ptr_eq(
        &a,
        &navigation_field_toward(&first, NavigationClass::Ground, 1, 1, 1)
    ));
}

#[test]
fn f10_position_fields_invalidate_on_revision_and_separate_team_class_goal() {
    let world = open_world(5, 5);
    let a = navigation_field_toward(&world, NavigationClass::Ground, 1, 1, 1);
    assert!(Arc::ptr_eq(
        &a,
        &navigation_field_toward(&world, NavigationClass::Ground, 1, 1, 1)
    ));
    let position = (2 << 16) | 2;
    world.tiles.insert(
        position,
        DynamicTile {
            position,
            block: 216,
            team: 1,
            health: 100.0,
            occupied: vec![position],
            ..Default::default()
        },
    );
    world.navigation_revision.fetch_add(1, Ordering::Relaxed);
    let b = navigation_field_toward(&world, NavigationClass::Ground, 1, 1, 1);
    assert!(!Arc::ptr_eq(&a, &b));
    assert_ne!(*a, *b);
    for class in [
        NavigationClass::Ground,
        NavigationClass::Legs,
        NavigationClass::Naval,
    ] {
        for team in [1, 5] {
            for (x, y) in [(1, 1), (3, 3)] {
                assert_eq!(
                    *navigation_field_toward(&world, class, x, y, team),
                    build_navigation_field_toward(&world, class, x, y, team)
                );
            }
        }
    }
}
