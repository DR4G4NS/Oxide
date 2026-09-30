//! Regression scenarios for the 160.5 projectile creation contract.
use super::*;

#[test]
fn parity_spread_rotates_authoritative_velocity_and_flies_past_aim() {
    let (world, out) = tests::test_world();
    let mut volley = FLARE_BOLT;
    volley.inaccuracy = 40.0;
    let id = spawn_unit_projectile_for_team(
        &world, &out, 42, -1, None, volley, 100.0, 100.0, 120.0, 100.0, 0.0, 0, 1,
    );
    let p = world.projectiles.get(&id).unwrap();
    let angle = (p.target_y - p.source_y)
        .atan2(p.target_x - p.source_x)
        .to_degrees();
    assert!((angle + 20.0).abs() < 0.001, "authoritative angle {angle}");
    assert!((p.total_ticks - volley.lifetime).abs() < 0.001);
    assert!(
        p.target_x > 120.0,
        "physical projectile must not expire at mouse aim"
    );
}

#[test]
fn parity_burst_delays_creation_instead_of_slowing_flight() {
    let (world, out) = tests::test_world();
    world.enemies.insert(42, tests::enemy_unit(42));
    let id = spawn_unit_projectile_for_team(
        &world, &out, 42, -1, None, FLARE_BOLT, 100.0, 100.0, 120.0, 100.0, 0.0, 2, 1,
    );
    assert!(
        !world.projectiles.contains_key(&id),
        "last burst shot exists before its six-tick delay"
    );
    simulate_projectiles(&world, &out, 5.0);
    assert!(!world.projectiles.contains_key(&id));
    simulate_projectiles(&world, &out, 1.0);
    let p = world.projectiles.get(&id).unwrap();
    assert!((p.total_ticks - FLARE_BOLT.lifetime).abs() < 0.001);
    assert!((p.remaining_ticks - p.total_ticks).abs() < 0.001);
}

#[test]
fn parity_direct_missile_launchers_spawn_at_muzzle_without_bullet() {
    for (bullet, unit_type) in [(92, 46), (106, 55), (186, 65), (189, 66), (192, 67)] {
        let (world, out) = tests::test_world();
        spawn_projectile_for_team(
            &world,
            &out,
            Some(0),
            -1,
            bullet,
            100.0,
            100.0,
            200.0,
            100.0,
            0.0,
            1.0,
            100.0,
            1.0,
            2,
        );
        assert!(
            world.projectiles.is_empty(),
            "launcher {bullet} cannot become a bullet entity"
        );
        let missile = world
            .enemies
            .iter()
            .next()
            .expect("missile must exist at fire time");
        assert_eq!(missile.unit_type, unit_type);
        assert_eq!((missile.x, missile.y, missile.team), (100.0, 100.0, 2));
    }
}

#[test]
fn parity_homing_searches_near_original_aim_not_near_bullet() {
    let (world, out) = tests::test_world();
    let mut near_muzzle = tests::enemy_unit(101);
    near_muzzle.x = 110.0;
    near_muzzle.y = 95.0;
    let mut near_aim = tests::enemy_unit(102);
    near_aim.x = 300.0;
    near_aim.y = 110.0;
    world.enemies.insert(101, near_muzzle);
    world.enemies.insert(102, near_aim);
    let id = spawn_unit_projectile_for_team(
        &world,
        &out,
        42,
        -1,
        None,
        RISSO_MISSILE,
        100.0,
        100.0,
        300.0,
        100.0,
        0.0,
        0,
        1,
    );
    simulate_projectiles(&world, &out, 1.0);
    let p = world.projectiles.get(&id).expect("still flying");
    assert!(
        p.target_y > 100.0,
        "must steer toward target near aim rather than below muzzle"
    );
}

#[test]
fn parity_navanax_manual_and_ai_match_initialized_jar_mount_trace() {
    let mut manual = tests::enemy_unit(1);
    manual.unit_type = 34;
    let mut ai = manual.clone();
    let mut manual_trace = Vec::new();
    let mut ai_trace = Vec::new();
    for tick in 1..=400 {
        for (unit, trace, controlled) in [
            (&mut manual, &mut manual_trace, true),
            (&mut ai, &mut ai_trace, false),
        ] {
            let shots = if controlled {
                collect_manual_weapon_fire(unit, 1.0, 200.0)
            } else {
                collect_allied_weapon_fire_tick(unit, 1.0, 200.0)
            }
            .unwrap();
            for shot in shots {
                {
                    let AlliedWeaponFire::Projectile(v) = shot;
                    if v.bullet_id == 60 {
                        assert_eq!(v.mirrored_mounts, 1);
                        trace.push((tick, v.mount_offset < 0.0));
                    }
                }
            }
        }
    }
    let expected = vec![
        (1, false),
        (66, true),
        (132, false),
        (197, true),
        (263, false),
        (328, true),
        (394, false),
    ];
    assert_eq!(manual_trace, expected);
    assert_eq!(ai_trace, expected);
}

#[test]
fn parity_navanax_lasers_keep_independent_mounts_and_live_beam_reload() {
    let (world, out) = tests::test_world();
    let mut shooter = tests::enemy_unit(42);
    shooter.unit_type = 34;
    shooter.team = 1;
    shooter.x = 200.0;
    shooter.y = 200.0;
    shooter.rotation = 90.0;
    let mut target = tests::enemy_unit(43);
    target.x = 200.0;
    target.y = 250.0;
    target.health = 1_000_000.0;
    world.enemies.insert(42, shooter);
    world.enemies.insert(43, target);
    let mut previous = [None; 4];
    let mut trace = Vec::new();
    for tick in 1..=400 {
        simulate_projectiles(&world, &out, 1.0);
        if tick == 8 {
            assert_eq!(world.enemies.get(&43).unwrap().health, 999_946.0);
        }
        let shooter = world.enemies.get(&42).unwrap();
        for (mount, state) in shooter.navanax_lasers.iter().enumerate() {
            if state.beam.is_some() && state.beam != previous[mount] {
                trace.push((tick, mount));
            }
            previous[mount] = state.beam;
        }
    }
    // Actual initialized official Weapon.update oracle, stationary target50px forward.
    assert_eq!(
        trace,
        vec![
            (3, 0),
            (3, 1),
            (7, 2),
            (7, 3),
            (327, 0),
            (327, 1),
            (331, 2),
            (331, 3)
        ]
    );
}

#[test]
fn parity_homing_unset_aim_falls_back_per_coordinate() {
    let (world, out) = tests::test_world();
    let mut target = tests::enemy_unit(43);
    target.x = 110.0;
    target.y = 110.0;
    world.enemies.insert(43, target);
    let id = spawn_unit_projectile_for_team(
        &world,
        &out,
        -1,
        -1,
        None,
        RISSO_MISSILE,
        100.0,
        100.0,
        300.0,
        100.0,
        0.0,
        0,
        1,
    );
    {
        let mut p = world.projectiles.get_mut(&id).unwrap();
        p.aim_x = -1.0;
        p.aim_y = -1.0;
    }
    simulate_projectiles(&world, &out, 1.0);
    assert!(world.projectiles.get(&id).unwrap().target_y > 100.0);
}

#[test]
fn parity_delayed_shot_is_cancelled_when_shooter_dies() {
    let (world, out) = tests::test_world();
    world.enemies.insert(42, tests::enemy_unit(42));
    let id = spawn_unit_projectile_for_team(
        &world, &out, 42, -1, None, FLARE_BOLT, 100.0, 100.0, 300.0, 100.0, 0.0, 2, 1,
    );
    assert!(world.pending_projectiles.contains_key(&id));
    world.enemies.remove(&42);
    simulate_projectiles(&world, &out, 6.0);
    assert!(!world.pending_projectiles.contains_key(&id));
    assert!(!world.projectiles.contains_key(&id));
}

#[test]
fn parity_navanax_autonomous_mounts_select_separate_targets() {
    let (world, out) = tests::test_world();
    let mut shooter = tests::enemy_unit(42);
    shooter.unit_type = 34;
    shooter.team = 1;
    shooter.x = 200.0;
    shooter.y = 200.0;
    shooter.rotation = 90.0;
    let mut left = tests::enemy_unit(43);
    left.x = 155.0;
    left.y = 220.0;
    let mut right = tests::enemy_unit(44);
    right.x = 245.0;
    right.y = 220.0;
    world.enemies.insert(42, shooter);
    world.enemies.insert(43, left);
    world.enemies.insert(44, right);
    simulate_projectiles(&world, &out, 1.0);
    let shooter = world.enemies.get(&42).unwrap();
    assert_eq!(
        shooter.navanax_lasers[0].target,
        Some(ProjectileHit::Unit(43))
    );
    assert_eq!(
        shooter.navanax_lasers[1].target,
        Some(ProjectileHit::Unit(44))
    );
    assert_eq!(
        shooter.navanax_lasers[2].target,
        Some(ProjectileHit::Unit(43))
    );
    assert_eq!(
        shooter.navanax_lasers[3].target,
        Some(ProjectileHit::Unit(44))
    );
}

#[test]
fn parity_delayed_shot_uses_live_shooter_position_and_aim() {
    let (world, out) = tests::test_world();
    let mut shooter = tests::enemy_unit(42);
    shooter.team = 1;
    shooter.x = 100.0;
    shooter.y = 100.0;
    world.enemies.insert(42, shooter);
    let id = spawn_unit_projectile_for_team(
        &world, &out, 42, -1, None, FLARE_BOLT, 100.0, 100.0, 300.0, 100.0, 0.0, 2, 1,
    );
    {
        let mut shooter = world.enemies.get_mut(&42).unwrap();
        shooter.x = 200.0;
        shooter.y = 200.0;
    }
    world.weapon_aims.insert((false, 42), (200.0, 400.0));
    simulate_projectiles(&world, &out, 6.0);
    let p = world.projectiles.get(&id).unwrap();
    assert_eq!((p.source_x, p.source_y), (200.0, 200.0));
    assert_eq!((p.aim_x, p.aim_y), (200.0, 400.0));
    let angle = (p.target_y - p.source_y)
        .atan2(p.target_x - p.source_x)
        .to_degrees();
    assert!(
        (angle - 92.0).abs() < 0.001,
        "last flare shot retains its two-degree pattern offset"
    );
    assert_eq!(p.remaining_ticks, p.total_ticks);
}

#[test]
fn parity_missile_follows_owner_aim_after_delay_and_keeps_heading_after_owner_loss() {
    let (world, out) = tests::test_world();
    let mut shooter = tests::enemy_unit(42);
    shooter.team = 1;
    shooter.x = 100.0;
    shooter.y = 100.0;
    world.enemies.insert(42, shooter);
    let mut volley = RISSO_MISSILE;
    volley.bullet_id = 92;
    let id = spawn_unit_projectile_for_team(
        &world, &out, 42, -1, None, volley, 100.0, 100.0, 300.0, 100.0, 0.0, 0, 1,
    );
    world.weapon_aims.insert((false, 42), (100.0, 500.0));
    for _ in 0..9 {
        unit_combat::simulate_missile_units(&world, &out, 1.0);
    }
    assert_eq!(world.enemies.get(&id).unwrap().rotation, 0.0);
    unit_combat::simulate_missile_units(&world, &out, 1.0);
    assert!((world.enemies.get(&id).unwrap().rotation - 2.5).abs() < 0.001);
    world.enemies.remove(&42);
    unit_combat::simulate_missile_units(&world, &out, 1.0);
    assert!((world.enemies.get(&id).unwrap().rotation - 2.5).abs() < 0.001);
}

#[test]
fn parity_scathe_acceleration_uses_elapsed_age_squared() {
    let (world, out) = tests::test_world();
    let id = spawn_projectile_for_team(
        &world, &out, None, -1, 186, 100.0, 100.0, 400.0, 100.0, 0.0, 1.0, 300.0, 1.0, 1,
    );
    {
        let mut missile = world.enemies.get_mut(&id).unwrap();
        missile.missile_time = 321.0;
    }
    unit_combat::simulate_missile_units(&world, &out, 1.0);
    let missile = world.enemies.get(&id).unwrap();
    // JAR MissileAI at age10, zero starting velocity:4.6*(10/50)^2*0.5.
    assert!((missile.velocity_x - 0.092).abs() < 0.0001);
    assert_eq!(missile.velocity_y, 0.0);
}

#[test]
fn parity_missile_passes_over_conveyor_unless_owner_aims_at_it() {
    for aimed in [false, true] {
        let (world, out) = tests::test_world();
        let position = (25 << 16) | 25;
        tests::insert_wall_at_team(&world, position, 257, 2, 1000.0);
        let mut shooter = tests::enemy_unit(42);
        shooter.team = 1;
        world.enemies.insert(42, shooter);
        let mut volley = RISSO_MISSILE;
        volley.bullet_id = 92;
        let id = spawn_unit_projectile_for_team(
            &world, &out, 42, -1, None, volley, 200.0, 200.0, 400.0, 200.0, 0.0, 0, 1,
        );
        world.weapon_aims.insert(
            (false, 42),
            if aimed {
                (200.0, 200.0)
            } else {
                (400.0, 200.0)
            },
        );
        unit_combat::simulate_missile_units(&world, &out, 1.0);
        assert_eq!(!world.enemies.contains_key(&id), aimed);
    }
}

#[test]
fn parity_rebuilt_turret_cannot_take_over_existing_missile() {
    let (world, out) = tests::test_world();
    let position = (10 << 16) | 10;
    tests::insert_wall_at_team(&world, position, 374, 1, 1000.0);
    world.tiles.get_mut(&position).unwrap().generation = 100;
    let id = spawn_projectile_for_team(
        &world,
        &out,
        Some(position),
        -1,
        186,
        160.0,
        160.0,
        300.0,
        160.0,
        0.0,
        1.0,
        200.0,
        1.0,
        1,
    );
    {
        let mut missile = world.enemies.get_mut(&id).unwrap();
        missile.missile_time = 320.0;
    }
    // Replacement on the same coordinate is a different Building object.
    world.tiles.get_mut(&position).unwrap().generation = 101;
    world.weapon_aims.insert((true, position), (160.0, 400.0));
    unit_combat::simulate_missile_units(&world, &out, 1.0);
    assert_eq!(world.enemies.get(&id).unwrap().rotation, 0.0);
}

#[test]
fn parity_weapon_aim_cache_prunes_removed_unit_and_building_owners() {
    let (world, out) = tests::test_world();
    world.weapon_aims.insert((false, 42), (100.0, 100.0));
    world.weapon_aims.insert((true, 123), (100.0, 100.0));
    simulate_projectiles(&world, &out, 1.0);
    assert!(
        world.weapon_aims.is_empty(),
        "payload/despawned units and removed buildings must not leak aim entries"
    );
}

#[test]
fn parity_missile_proximity_detonation_matches_official_acquisition_and_layers() {
    // Actual indexed JAR oracle: Dagger at5px fires; at10px strict boundary
    // rejects. Flare at5/10px is eligible only for Anthicus's air weapon.
    for missile_type in [46, 53, 55, 65, 66, 67, 68] {
        for (target_type, distance, team, expected) in [
            (0, 5.0, 2, true),
            (0, 10.0, 2, false),
            (0, 5.0, 1, false),
            (15, 5.0, 2, missile_type == 46),
            (15, 10.0, 2, missile_type == 46),
        ] {
            let (world, out) = tests::test_world();
            let id =
                crate::network::units::spawn_unit_world(&world, missile_type, 1, 100.0, 100.0, 0.0)
                    .unwrap();
            let mut target = tests::enemy_unit(42);
            target.unit_type = target_type;
            target.team = team;
            target.x = 100.0 + distance;
            target.y = 100.0;
            target.health = 10_000.0;
            world.enemies.insert(42, target);
            // Public controller scheduler, rather than calling the helper directly.
            crate::network::simulation::simulate_waves_and_enemies(&world, &out, 0.0);
            assert_eq!(
                !world.enemies.contains_key(&id),
                expected,
                "missile{missile_type} target{target_type} distance{distance} team{team}"
            );
            if expected {
                assert!(world.enemies.get(&42).unwrap().health < 10_000.0);
            }
        }
    }
}

#[test]
fn parity_missile_large_hitbox_matches_strict_unit_tree_broadphase() {
    for (dx, dy, expected) in [
        (18.0, 0.0, true),
        (18.99, 0.0, true),
        (19.0, 0.0, false),
        (20.0, 0.0, false),
        (18.0, 18.0, true),
        (18.99, 18.99, false),
        (18.0, 19.0, false),
    ] {
        let (world, out) = tests::test_world();
        let id = crate::network::units::spawn_unit_world(&world, 46, 1, 100.0, 100.0, 0.0).unwrap();
        let mut target = tests::enemy_unit(42);
        target.unit_type = 14;
        target.x = 100.0 + dx;
        target.y = 100.0 + dy;
        target.health = 10_000.0;
        world.enemies.insert(42, target);
        crate::network::simulation::simulate_waves_and_enemies(&world, &out, 0.0);
        assert_eq!(
            !world.enemies.contains_key(&id),
            expected,
            "toxopid at({dx},{dy})"
        );
    }
}

#[test]
fn parity_missile_target_priority_then_nearest_adjusted_distance() {
    let (world, _) = tests::test_world();
    let mut missile = tests::enemy_unit(1);
    missile.team = 1;
    missile.x = 100.0;
    missile.y = 100.0;
    let mut core_ship = tests::enemy_unit(42);
    core_ship.unit_type = 60;
    core_ship.x = 100.0;
    core_ship.y = 100.0;
    let mut farther = tests::enemy_unit(43);
    farther.x = 106.0;
    farther.y = 100.0;
    let mut nearer = tests::enemy_unit(44);
    nearer.x = 105.0;
    nearer.y = 100.0;
    world.enemies.insert(42, core_ship);
    world.enemies.insert(43, farther);
    world.enemies.insert(44, nearer);
    let spec = crate::game::unit_types::unit_missile_spec(46).unwrap();
    assert_eq!(
        unit_combat::missile_proximity_target(&world, &missile, spec, None),
        Some(ProjectileHit::Unit(44))
    );
}

#[test]
fn parity_missile_player_target_uses_current_core_body_size_and_priority() {
    let (world, _) = tests::test_world();
    register_team_core(
        &world,
        2,
        TeamCore {
            position: (20 << 16) | 20,
            block: 344,
            health: 6000.0,
            max_health: 6000.0,
        },
    );
    world.players.insert(
        42,
        PlayerCombatState {
            uuid: "missile-target".into(),
            player_id: 1,
            unit_id: 42,
            x: 111.0,
            y: 100.0,
            health: 1000.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 2,
        },
    );
    let mut missile = tests::enemy_unit(1);
    missile.team = 1;
    missile.x = 100.0;
    missile.y = 100.0;
    let spec = crate::game::unit_types::unit_missile_spec(46).unwrap();
    assert_eq!(
        unit_combat::missile_proximity_target(&world, &missile, spec, None),
        Some(ProjectileHit::Player(42)),
        "Emanate hitSize12 must be acquired at11px"
    );
    world.players.get_mut(&42).unwrap().x = 100.0;
    let mut dagger = tests::enemy_unit(43);
    dagger.x = 108.0;
    dagger.y = 100.0;
    world.enemies.insert(43, dagger);
    assert_eq!(
        unit_combat::missile_proximity_target(&world, &missile, spec, None),
        Some(ProjectileHit::Unit(43)),
        "priority0 Dagger outranks closer Erekir core ship priority-2"
    );
}
