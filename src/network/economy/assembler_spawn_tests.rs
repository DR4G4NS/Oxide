use super::*;

const ASSEMBLER_POSITION: i32 = (20 << 16) | 20;

fn expected_spawn(rotation: u8, distance: f32) -> (f32, f32) {
    match rotation {
        0 => (160.0 + distance, 160.0),
        1 => (160.0, 160.0 + distance),
        2 => (160.0 - distance, 160.0),
        3 => (160.0, 160.0 - distance),
        _ => unreachable!(),
    }
}

pub(super) fn seed_positioned_assembler_drones(world: &DynamicWorld, position: i32) {
    let assembler = world.tiles.get(&position).unwrap().clone();
    let (cx, cy) = building_center(position, assembler.block);
    let (ox, oy) = match assembler.rotation {
        0 => (72.0, 0.0),
        1 => (0.0, 72.0),
        2 => (-72.0, 0.0),
        3 => (0.0, -72.0),
        _ => unreachable!(),
    };
    let mut ids = Vec::new();
    for (index, (dx, dy)) in [(52.0, 52.0), (-52.0, 52.0), (-52.0, -52.0), (52.0, -52.0)]
        .into_iter()
        .enumerate()
    {
        let id = -100 - index as i32;
        let mut unit = ground_unit_on_tile(id, 63, position, 100.0, 1.0);
        unit.team = assembler.team;
        unit.x = cx + ox + dx;
        unit.y = cy + oy + dy;
        unit.rotation = 225.0 + index as f32 * 90.0;
        world.enemies.insert(id, unit);
        ids.push(id);
    }
    world.game_state.extras.assembler_drones.lock().insert(
        position,
        crate::state::game_state::AssemblerDroneBind {
            unit_ids: ids,
            progress: 0.0,
        },
    );
}

fn ready_assembler(block: i16, rotation: u8) -> DynamicWorld {
    let world = erekir_test_world();
    let (_, time, requirements) = assembler_plan(block, 0).unwrap();
    let mut tile = erekir_tile(ASSEMBLER_POSITION, block, rotation);
    tile.production_progress = time - 1.0;
    tile.payload_inventory = requirements
        .iter()
        .map(|(id, amount)| (*id, amount * 2))
        .collect();
    tile.stored_liquid = CYANOGEN_LIQUID;
    tile.liquid_amount = 100.0;
    world.tiles.insert(ASSEMBLER_POSITION, tile);
    items_for_team_mut(&world, 1).fill(1000);
    seed_positioned_assembler_drones(&world, ASSEMBLER_POSITION);
    world
}

fn produced_units(world: &DynamicWorld) -> Vec<EnemyUnit> {
    world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type != 63)
        .map(|unit| unit.clone())
        .collect()
}

#[test]
fn assembler_completion_spawns_at_checked_destination_and_consumes_once_all_rotations() {
    // Independent expected coordinates: official 160.5 size5/area13 gives
    // 72 world units from the odd-sized block's centre, for all three blocks.
    for block in 393..=395 {
        for rotation in 0..4 {
            let world = ready_assembler(block, rotation);
            let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
            assert!(simulate_erekir_assemblers(
                &world,
                &DashMap::new(),
                1.0,
                &power
            ));
            let units = produced_units(&world);
            assert_eq!(units.len(), 1);
            let (unit_type, _, requirements) = assembler_plan(block, 0).unwrap();
            assert_eq!(units[0].unit_type, unit_type);
            let (x, y) = expected_spawn(rotation, 72.0);
            assert!(
                (units[0].x - x).abs() < 0.001 && (units[0].y - y).abs() < 0.001,
                "block {block} rotation {rotation}: actual ({}, {}), expected ({x}, {y})",
                units[0].x,
                units[0].y
            );
            let after = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
            assert_eq!(after.production_progress, 0.0);
            assert_eq!(after.payload_inventory, requirements);
            let cost = if block == 393 {
                9.0 / 60.0
            } else {
                12.0 / 60.0
            };
            assert!((after.liquid_amount - (100.0 - cost)).abs() < 0.001);
            assert!(items_for_team(&world, 1)
                .iter()
                .all(|amount| *amount == 1000));
        }
    }
}

#[test]
fn assembler_spawn_itself_blocks_next_craft_across_snapshot_all_rotations() {
    for block in 393..=395 {
        for rotation in 0..4 {
            let world = ready_assembler(block, rotation);
            let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
            assert!(simulate_erekir_assemblers(
                &world,
                &DashMap::new(),
                1.0,
                &power
            ));
            let before = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
            let mut expected_sync = Vec::new();
            crate::network::buildings::snapshot::encode_unit_assembler_sync(
                &mut expected_sync,
                &before,
                Some(&world),
                &power,
            )
            .unwrap();
            // Do not move the newly spawned unit onto a separate manually
            // chosen clearance point: its actual position must block reuse.
            for tick in 1..=361 {
                assert!(!simulate_erekir_assemblers(
                    &world,
                    &DashMap::new(),
                    1.0,
                    &power
                ));
                if tick == 180 || tick == 361 {
                    let snapshot = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
                    let mut sync = Vec::new();
                    crate::network::buildings::snapshot::encode_unit_assembler_sync(
                        &mut sync,
                        &snapshot,
                        Some(&world),
                        &power,
                    )
                    .unwrap();
                    assert_eq!(
                        sync, expected_sync,
                        "occupied output must preserve snapshot state"
                    );
                }
            }
            let after = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
            assert_eq!(after.production_progress, before.production_progress);
            assert_eq!(after.payload_inventory, before.payload_inventory);
            assert_eq!(after.liquid_amount, before.liquid_amount);
            assert_eq!(produced_units(&world).len(), 1);
            let id = produced_units(&world)[0].id;
            world.enemies.get_mut(&id).unwrap().x += 200.0;
            assert!(!simulate_erekir_assemblers(
                &world,
                &DashMap::new(),
                1.0,
                &power
            ));
            assert_eq!(
                world
                    .tiles
                    .get(&ASSEMBLER_POSITION)
                    .unwrap()
                    .production_progress,
                1.0
            );
        }
    }
}

#[test]
fn assembler_core_materials_cannot_replace_missing_payloads() {
    for block in 393..=395 {
        let world = ready_assembler(block, 0);
        world
            .tiles
            .get_mut(&ASSEMBLER_POSITION)
            .unwrap()
            .payload_inventory
            .clear();
        let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
        assert!(!simulate_erekir_assemblers(
            &world,
            &DashMap::new(),
            1.0,
            &power
        ));
        assert!(produced_units(&world).is_empty());
        let tile = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
        assert_eq!(
            tile.production_progress,
            assembler_plan(block, 0).unwrap().1 - 1.0
        );
        assert_eq!(tile.liquid_amount, 100.0);
        assert!(items_for_team(&world, 1)
            .iter()
            .all(|amount| *amount == 1000));
    }
}

#[test]
fn assembler_progress_scales_with_positioned_drones_and_team_build_speed() {
    for positioned in 0..=4 {
        let world = ready_assembler(393, 0);
        world
            .tiles
            .get_mut(&ASSEMBLER_POSITION)
            .unwrap()
            .production_progress = 0.0;
        for (index, id) in world
            .assembler_drone_ids(ASSEMBLER_POSITION)
            .iter()
            .enumerate()
        {
            if index >= positioned {
                // Still tethered and alive, but pointing away from its slot.
                world.enemies.get_mut(id).unwrap().rotation += 90.0;
            }
        }
        {
            let mut rules = world.wave_rules.write();
            rules.unit_build_speed_multiplier = 2.0;
            rules
                .team_rules
                .entry(1)
                .or_default()
                .unit_build_speed_multiplier = 1.5;
        }
        let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
        assert!(!simulate_erekir_assemblers(
            &world,
            &DashMap::new(),
            8.0,
            &power
        ));
        let after = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
        assert!((after.production_progress - 6.0 * positioned as f32).abs() < 0.001);
        assert!(
            (after.liquid_amount - 98.8).abs() < 0.001,
            "drone fraction/build speed scale progress, not continuous liquid consumption"
        );
    }
}

#[test]
fn reconstructor_handoff_rejects_output_face_all_rotations() {
    for rotation in 0..4 {
        for source_face in 0..4 {
            let world = erekir_test_world();
            let receiver = erekir_tile(ASSEMBLER_POSITION, 380, rotation);
            let source_position = offset_position_by(ASSEMBLER_POSITION, source_face, 3);
            let mut source = erekir_tile(source_position, 398, (source_face + 2) % 4);
            source.payload = Some(Box::new(CarriedPayload::Unit(ground_unit_on_tile(
                91,
                0,
                source_position,
                150.0,
                0.0,
            ))));
            world.tiles.insert(receiver.position, receiver);
            world.tiles.insert(source.position, source.clone());
            let transferred = transfer_payload_forward(&world, &source);
            assert_eq!(
                transferred,
                source_face != rotation,
                "receiver rotation {rotation}, source face {source_face}"
            );
            assert_eq!(
                world
                    .tiles
                    .get(&ASSEMBLER_POSITION)
                    .unwrap()
                    .payload
                    .is_some(),
                transferred
            );
            assert_eq!(
                world.tiles.get(&source_position).unwrap().payload.is_some(),
                !transferred
            );
        }
    }
}

#[test]
fn nonassembler_direct_spawns_keep_existing_twenty_unit_offset() {
    for block in [377, 386] {
        for rotation in 0..4 {
            let world = erekir_test_world();
            let factory = erekir_tile(ASSEMBLER_POSITION, block, rotation);
            spawn_factory_unit(&world, &DashMap::new(), &factory, 0);
            let unit = world.enemies.iter().next().unwrap().clone();
            let (x, y) = expected_spawn(rotation, 20.0);
            assert!((unit.x - x).abs() < 0.001 && (unit.y - y).abs() < 0.001);
        }
    }
}

#[test]
fn assembler_modules_must_face_the_area_perimeter_all_rotations() {
    for rotation in 0..4 {
        let world = erekir_test_world();
        let assembler = erekir_tile(ASSEMBLER_POSITION, 393, rotation);
        let spawn_tile = offset_position_by(ASSEMBLER_POSITION, rotation, 9);
        for face in 0..4 {
            let position = offset_position_by(spawn_tile, face, 9);
            let mut module = erekir_tile(position, 396, (face + 2) % 4);
            world.tiles.insert(position, module.clone());
            assert_eq!(
                assembler_tier(&world, &assembler),
                1,
                "assembler rotation {rotation}, module on face {face}"
            );
            module.rotation = face;
            world.tiles.insert(position, module);
            assert_eq!(
                assembler_tier(&world, &assembler),
                0,
                "outward-facing module"
            );
            world.tiles.remove(&position);
            let near_position = offset_position_by(spawn_tile, face, 8);
            world.tiles.insert(
                near_position,
                erekir_tile(near_position, 396, (face + 2) % 4),
            );
            assert_eq!(
                assembler_tier(&world, &assembler),
                0,
                "one tile inside perimeter"
            );
            world.tiles.remove(&near_position);
        }
    }
}

#[test]
fn assembler_clearance_respects_output_hitbox_air_ground_legs_and_core_units() {
    for (block, unit_type, elevation, expected) in [
        (393, 0, 0.0, true),
        (393, 15, 1.0, false),
        (393, 35, 0.0, false),
        (393, 5, 0.05, false),
        (394, 0, 0.0, false),
        (394, 15, 1.0, true),
        (394, 35, 1.0, false),
        (395, 44, 0.5, true),
        (395, 15, 1.0, false),
    ] {
        let world = ready_assembler(block, 0);
        let assembler = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
        let (x, y) = expected_spawn(0, 72.0);
        let mut blocker = ground_unit_on_tile(90, unit_type, ASSEMBLER_POSITION, 100.0, elevation);
        blocker.x = x;
        blocker.y = y;
        world.enemies.insert(blocker.id, blocker);
        assert_eq!(
            crate::network::economy::erekir::assembler_output_occupied(&world, &assembler),
            expected,
            "block {block}, unit {unit_type}, elevation {elevation}"
        );
    }
    // Vanquish output's28*1.4 square overlaps a dagger tile-hitbox at a
    // diagonal24-unit displacement even though the old24-radius circle did not.
    let world = ready_assembler(393, 0);
    let assembler = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
    let mut blocker = ground_unit_on_tile(90, 0, ASSEMBLER_POSITION, 100.0, 0.0);
    blocker.x = 232.0 + 20.0;
    blocker.y = 160.0 + 20.0;
    world.enemies.insert(blocker.id, blocker);
    assert!(crate::network::economy::erekir::assembler_output_occupied(
        &world, &assembler
    ));
}

#[test]
fn assembler_ground_output_checks_solid_tiles_across_entire_hitbox() {
    for block in [393, 394, 395] {
        for rotation in 0..4 {
            let world = ready_assembler(block, rotation);
            let assembler = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
            let spawn_tile = offset_position_by(ASSEMBLER_POSITION, rotation, 9);
            let wall_position = offset_position_by(spawn_tile, rotation, 2);
            world
                .tiles
                .insert(wall_position, erekir_tile(wall_position, 216, 0));
            assert_eq!(
                crate::network::economy::erekir::assembler_output_occupied(&world, &assembler),
                block != 394,
                "solid tile touches ground output hitbox, ship output may fly over it"
            );
        }
    }
}

#[test]
fn assembler_positioned_drone_distance_is_strict_and_angle_inclusive() {
    for (distance, angle, ready) in [
        (9.999, 15.0, true),
        (10.0, 15.0, false),
        (0.0, 15.01, false),
    ] {
        let world = ready_assembler(393, 0);
        world
            .tiles
            .get_mut(&ASSEMBLER_POSITION)
            .unwrap()
            .production_progress = 0.0;
        let id = world.assembler_drone_ids(ASSEMBLER_POSITION)[0];
        {
            let mut drone = world.enemies.get_mut(&id).unwrap();
            drone.x += distance;
            drone.rotation += angle;
        }
        let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
        assert!(!simulate_erekir_assemblers(
            &world,
            &DashMap::new(),
            4.0,
            &power
        ));
        assert_eq!(
            world
                .tiles
                .get(&ASSEMBLER_POSITION)
                .unwrap()
                .production_progress,
            if ready { 4.0 } else { 3.0 }
        );
    }
}

#[test]
fn assembler_drone_construction_restarts_each_240_tick_interval() {
    let world = erekir_test_world();
    world
        .tiles
        .insert(ASSEMBLER_POSITION, erekir_tile(ASSEMBLER_POSITION, 393, 0));
    let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
    crate::network::economy::simulate_repair_and_cargo(&world, 240.0, &power);
    assert_eq!(world.assembler_drone_ids(ASSEMBLER_POSITION).len(), 1);
    crate::network::economy::simulate_repair_and_cargo(&world, 239.0, &power);
    assert_eq!(world.assembler_drone_ids(ASSEMBLER_POSITION).len(), 1);
    crate::network::economy::simulate_repair_and_cargo(&world, 1.0, &power);
    assert_eq!(world.assembler_drone_ids(ASSEMBLER_POSITION).len(), 2);
}

#[test]
fn assembler_progress_and_typed_payloads_survive_480_tick_snapshot_window() {
    let world = ready_assembler(393, 0);
    world
        .tiles
        .get_mut(&ASSEMBLER_POSITION)
        .unwrap()
        .production_progress = 0.0;
    let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
    for tick in 1..=480 {
        assert!(!simulate_erekir_assemblers(
            &world,
            &DashMap::new(),
            1.0,
            &power
        ));
        if tick == 360 || tick == 480 {
            let tile = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
            assert_eq!(tile.production_progress, tick as f32);
            assert_eq!(tile.payload_inventory, [(38, 8), (238, 20)]);
            let mut bytes = Vec::new();
            encode_dynamic_tile_sync(&mut bytes, &tile, &power, Some(&world)).unwrap();
            // Last24 bytes: PayloadSeq(-2), two(type,id,amount) entries, then
            // nullable commandPos. This independently asserts wire types.
            let start = bytes.len() - 24;
            assert_eq!(&bytes[start..start + 2], &(-2i16).to_be_bytes());
            assert_eq!(bytes[start + 2], 6, "stell is a unit payload");
            assert_eq!(bytes[start + 9], 1, "tungsten wall is a block payload");
            if let Ok(directory) = std::env::var("OXIDE_ASSEMBLER_FIXTURE_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(
                    std::path::Path::new(&directory).join(format!("assembler-{tick}-sync.bin")),
                    bytes,
                )
                .unwrap();
            }
        }
    }
}

fn assembler_input_payload(content: i16, is_unit: bool) -> CarriedPayload {
    if is_unit {
        let mut unit = ground_unit_on_tile(900, content, ASSEMBLER_POSITION, 100.0, 0.0);
        unit.team = 1;
        CarriedPayload::Unit(unit)
    } else {
        let tile = erekir_tile(ASSEMBLER_POSITION, content, 0);
        let mut sync = Vec::new();
        crate::network::buildings::snapshot::encode_simple_wall_sync(&mut sync, &tile).unwrap();
        CarriedPayload::Build(CarriedBuildPayload {
            tile,
            version: 0,
            sync,
        })
    }
}

fn empty_assembler_with_input(block: i16, tier: usize, rotation: u8) -> (DynamicWorld, i32) {
    let mut world = erekir_test_world();
    world.width = 60;
    world.height = 60;
    world.base_blocks.resize(3600, 0);
    world.base_centers.resize(3600, false);
    world.floors.resize(3600, 0);
    world.overlays.resize(3600, 0);
    let mut assembler = erekir_tile(ASSEMBLER_POSITION, block, rotation);
    assembler.occupied = block_footprint(&world, ASSEMBLER_POSITION, block).unwrap();
    world.tiles.insert(ASSEMBLER_POSITION, assembler);
    if tier == 1 {
        let module_position = offset_position_by(ASSEMBLER_POSITION, rotation, 18);
        let mut module = erekir_tile(module_position, 396, (rotation + 2) % 4);
        module.occupied = block_footprint(&world, module_position, 396).unwrap();
        world.tiles.insert(module_position, module);
    }
    let source_position = offset_position_by(ASSEMBLER_POSITION, (rotation + 2) % 4, 4);
    let mut source = erekir_tile(source_position, 400, rotation);
    source.occupied = block_footprint(&world, source_position, 400).unwrap();
    world.tiles.insert(source_position, source);
    (world, source_position)
}

#[test]
fn assembler_real_conveyor_ingress_feeds_all_six_plans_and_rotations() {
    for block in 393..=395 {
        for tier in 0..=1 {
            for rotation in 0..4 {
                let (world, source) = empty_assembler_with_input(block, tier, rotation);
                let (output, time, requirements) = assembler_plan(block, tier).unwrap();
                for (index, &(content, amount)) in requirements.iter().enumerate() {
                    for count in 1..=amount {
                        world.tiles.get_mut(&source).unwrap().payload =
                            Some(Box::new(assembler_input_payload(content, index == 0)));
                        assert!(crate::network::simulation::simulate_payload_conveyors(
                            &world,
                            &DashMap::new(),
                            35.0
                        ));
                        assert!(world.tiles.get(&source).unwrap().payload.is_none());
                        assert!(world
                            .tiles
                            .get(&ASSEMBLER_POSITION)
                            .unwrap()
                            .payload
                            .is_some());
                        assert!(simulate_erekir_assemblers(
                            &world,
                            &DashMap::new(),
                            30.0,
                            &HashMap::new()
                        ));
                        let asm = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
                        assert!(asm.payload.is_none());
                        assert_eq!(inventory_count(&asm.payload_inventory, content), count);
                        assert_eq!(
                            asm.production_progress, 0.0,
                            "receiving does not fabricate progress"
                        );
                    }
                    // A full local requirement refuses one more real input.
                    world.tiles.get_mut(&source).unwrap().payload =
                        Some(Box::new(assembler_input_payload(content, index == 0)));
                    assert!(!crate::network::simulation::simulate_payload_conveyors(
                        &world,
                        &DashMap::new(),
                        35.0
                    ));
                    assert!(world.tiles.get(&source).unwrap().payload.is_some());
                    world.tiles.get_mut(&source).unwrap().payload = None;
                }
                seed_positioned_assembler_drones(&world, ASSEMBLER_POSITION);
                {
                    let mut assembler = world.tiles.get_mut(&ASSEMBLER_POSITION).unwrap();
                    assembler.production_progress = time - 1.0;
                    assembler.stored_liquid = CYANOGEN_LIQUID;
                    assembler.liquid_amount = 100.0;
                }
                let power = HashMap::from([(ASSEMBLER_POSITION, 1.0)]);
                assert!(simulate_erekir_assemblers(
                    &world,
                    &DashMap::new(),
                    1.0,
                    &power
                ));
                assert!(world
                    .tiles
                    .get(&ASSEMBLER_POSITION)
                    .unwrap()
                    .payload_inventory
                    .is_empty());
                assert_eq!(produced_units(&world).len(), 1);
                assert_eq!(produced_units(&world)[0].unit_type, output);
            }
        }
    }
}

#[test]
fn assembler_ingress_rejects_enemy_wrong_type_and_occupied_input() {
    let (world, source) = empty_assembler_with_input(393, 0, 0);
    world.tiles.get_mut(&source).unwrap().payload =
        Some(Box::new(assembler_input_payload(38, true)));
    world.tiles.get_mut(&source).unwrap().team = 2;
    assert!(!crate::network::simulation::simulate_payload_conveyors(
        &world,
        &DashMap::new(),
        35.0
    ));
    world.tiles.get_mut(&source).unwrap().team = 1;
    world.tiles.get_mut(&source).unwrap().payload =
        Some(Box::new(assembler_input_payload(38, false)));
    assert!(!crate::network::simulation::simulate_payload_conveyors(
        &world,
        &DashMap::new(),
        35.0
    ));
    world.tiles.get_mut(&source).unwrap().payload =
        Some(Box::new(assembler_input_payload(38, true)));
    assert!(crate::network::simulation::simulate_payload_conveyors(
        &world,
        &DashMap::new(),
        35.0
    ));
    world.tiles.get_mut(&source).unwrap().payload =
        Some(Box::new(assembler_input_payload(38, true)));
    assert!(!crate::network::simulation::simulate_payload_conveyors(
        &world,
        &DashMap::new(),
        35.0
    ));
    assert!(world.tiles.get(&source).unwrap().payload.is_some());
}

#[test]
fn assembler_admission_and_consumption_share_unit_cost_scaling() {
    let (world, source) = empty_assembler_with_input(393, 0, 0);
    {
        let mut rules = world.wave_rules.write();
        rules.unit_cost_multiplier = 2.0;
        rules.team_rules.entry(1).or_default().unit_cost_multiplier = 0.25;
    }
    for (index, (content, amount)) in [(38, 2), (238, 5)].into_iter().enumerate() {
        for _ in 0..amount {
            world.tiles.get_mut(&source).unwrap().payload =
                Some(Box::new(assembler_input_payload(content, index == 0)));
            assert!(crate::network::simulation::simulate_payload_conveyors(
                &world,
                &DashMap::new(),
                35.0
            ));
            simulate_erekir_assemblers(&world, &DashMap::new(), 30.0, &HashMap::new());
        }
        world.tiles.get_mut(&source).unwrap().payload =
            Some(Box::new(assembler_input_payload(content, index == 0)));
        assert!(!crate::network::simulation::simulate_payload_conveyors(
            &world,
            &DashMap::new(),
            35.0
        ));
        world.tiles.get_mut(&source).unwrap().payload = None;
    }
    seed_positioned_assembler_drones(&world, ASSEMBLER_POSITION);
    {
        let mut asm = world.tiles.get_mut(&ASSEMBLER_POSITION).unwrap();
        asm.production_progress = 2999.0;
        asm.stored_liquid = CYANOGEN_LIQUID;
        asm.liquid_amount = 100.0;
    }
    assert!(simulate_erekir_assemblers(
        &world,
        &DashMap::new(),
        1.0,
        &HashMap::from([(ASSEMBLER_POSITION, 1.0)])
    ));
    let asm = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
    assert!(asm.payload_inventory.is_empty());
    assert!((asm.liquid_amount - 99.925).abs() < 0.0001);
}

#[test]
fn assembler_receives_existing_logic_carrier_drop_route() {
    let (world, _) = empty_assembler_with_input(393, 0, 0);
    let mut carrier = ground_unit_on_tile(901, 22, ASSEMBLER_POSITION, 100.0, 1.0);
    carrier.team = 1;
    carrier.payloads.push(assembler_input_payload(38, true));
    world.enemies.insert(carrier.id, carrier);
    assert!(logic_unit_drop_payload(&world, 901).is_some());
    assert!(world.enemies.get(&901).unwrap().payloads.is_empty());
    assert!(world
        .tiles
        .get(&ASSEMBLER_POSITION)
        .unwrap()
        .payload
        .is_some());
    simulate_erekir_assemblers(&world, &DashMap::new(), 30.0, &HashMap::new());
    assert_eq!(
        world
            .tiles
            .get(&ASSEMBLER_POSITION)
            .unwrap()
            .payload_inventory,
        [(38, 1)]
    );
}

#[test]
fn assembler_fractional_cyanogen_caps_progress_at_available_scaled_supply() {
    for cost in [0.5, 1.0, 2.0] {
        let world = ready_assembler(393, 0);
        world.wave_rules.write().unit_cost_multiplier = cost;
        {
            let mut tile = world.tiles.get_mut(&ASSEMBLER_POSITION).unwrap();
            tile.production_progress = 0.0;
            tile.liquid_amount = 0.001;
        }
        assert!(!simulate_erekir_assemblers(
            &world,
            &DashMap::new(),
            60.0,
            &HashMap::from([(ASSEMBLER_POSITION, 0.5)])
        ));
        let tile = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
        assert!((tile.production_progress - 0.001 / (0.15 * cost)).abs() < 0.000001);
        assert_eq!(tile.liquid_amount, 0.0);
    }
}

#[test]
fn assembler_json_roundtrip_retains_loaded_and_in_transit_typed_payloads() {
    for (content, is_unit) in [(38, true), (238, false)] {
        let world = ready_assembler(393, 0);
        let mut tile = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
        tile.payload = Some(Box::new(assembler_input_payload(content, is_unit)));
        tile.payload_accum = vec![-12.0, 3.0];
        tile.payload_rotation = 95.0;
        tile.production_progress = 417.0;
        tile.health = crate::game::content::block_health(tile.block);
        let mut saved_world = erekir_test_world();
        saved_world.tiles.insert(ASSEMBLER_POSITION, tile.clone());
        let saved = crate::network::wire::persistence::snapshot_persisted_world(
            &saved_world.tiles,
            &saved_world.game_state,
            &saved_world.enemies,
            &saved_world.base_buildings,
            &saved_world.player_profiles,
            &saved_world.building_commands,
            &saved_world.unit_orders,
            &saved_world.team_build_plans.read(),
            &saved_world.cores,
            &saved_world.logic_flags,
            &saved_world.puddles,
            String::new(),
        );
        let path = std::env::temp_dir().join(format!(
            "oxide-assembler-json-{}-{}-{content}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        crate::network::wire::persistence::persist_world_sync(&path, &saved).unwrap();
        let loaded = crate::network::wire::persistence::load_tiles(&path, Some((40, 40))).unwrap();
        std::fs::remove_file(path).unwrap();
        let restored = loaded
            .tiles
            .get(&ASSEMBLER_POSITION)
            .expect("assembler survived actual checkpoint validation")
            .clone();
        saved_world.tiles = loaded.tiles;
        assert_eq!(restored.payload_inventory, tile.payload_inventory);
        assert_eq!(restored.production_progress, 417.0);
        assert_eq!(restored.payload_accum, [-12.0, 3.0]);
        assert_eq!(restored.payload_rotation, 95.0);
        assert_eq!(restored.liquid_amount, 100.0);
        match restored.payload.as_deref().unwrap() {
            CarriedPayload::Unit(unit) => {
                assert!(is_unit);
                assert_eq!(unit.unit_type, content);
            }
            CarriedPayload::Build(build) => {
                assert!(!is_unit);
                assert_eq!(build.tile.block, content);
            }
        }
        let count = inventory_count(&restored.payload_inventory, content);
        assert!(simulate_erekir_assemblers(
            &saved_world,
            &DashMap::new(),
            30.0,
            &HashMap::new()
        ));
        let resumed = saved_world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
        assert!(resumed.payload.is_none());
        assert_eq!(
            inventory_count(&resumed.payload_inventory, content),
            count + 1
        );
        assert_eq!(resumed.production_progress, 417.0);
    }
}

#[test]
fn assembler_received_unit_and_block_payloads_have_complete_in_transit_snapshots() {
    let (world, source) = empty_assembler_with_input(393, 0, 0);
    for (content, is_unit, name) in [(38, true, "unit"), (238, false, "block")] {
        world.tiles.get_mut(&source).unwrap().payload =
            Some(Box::new(assembler_input_payload(content, is_unit)));
        assert!(crate::network::simulation::simulate_payload_conveyors(
            &world,
            &DashMap::new(),
            35.0
        ));
        let snapshot = world.tiles.get(&ASSEMBLER_POSITION).unwrap().clone();
        assert_eq!(snapshot.payload_accum, [-20.0, 0.0]);
        assert!(snapshot.payload.is_some());
        let mut bytes = Vec::new();
        encode_dynamic_tile_sync(&mut bytes, &snapshot, &HashMap::new(), Some(&world)).unwrap();
        if let Ok(directory) = std::env::var("OXIDE_ASSEMBLER_FIXTURE_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                std::path::Path::new(&directory)
                    .join(format!("assembler-in-transit-{name}-sync.bin")),
                bytes,
            )
            .unwrap();
        }
        assert!(simulate_erekir_assemblers(
            &world,
            &DashMap::new(),
            30.0,
            &HashMap::new()
        ));
    }
}

#[test]
fn assembler_module_conveyor_ingress_feeds_tier_one_plans_all_rotations() {
    for block in 393..=395 {
        for rotation in 0..4 {
            let (world, _) = empty_assembler_with_input(block, 1, rotation);
            let module = offset_position_by(ASSEMBLER_POSITION, rotation, 18);
            let source = offset_position_by(module, (rotation + 1) % 4, 4);
            let mut conveyor = erekir_tile(source, 400, (rotation + 3) % 4);
            conveyor.occupied = block_footprint(&world, source, 400).unwrap();
            world.tiles.insert(source, conveyor);
            let power = HashMap::from([(module, 1.0)]);
            let (output, time, requirements) = assembler_plan(block, 1).unwrap();
            for (index, &(content, amount)) in requirements.iter().enumerate() {
                for count in 1..=amount {
                    world.tiles.get_mut(&source).unwrap().payload =
                        Some(Box::new(assembler_input_payload(content, index == 0)));
                    assert!(crate::network::simulation::simulate_payload_conveyors(
                        &world,
                        &DashMap::new(),
                        35.0
                    ));
                    assert!(world.tiles.get(&module).unwrap().payload.is_some());
                    assert!(simulate_erekir_assemblers(
                        &world,
                        &DashMap::new(),
                        30.0,
                        &power
                    ));
                    assert!(world.tiles.get(&module).unwrap().payload.is_none());
                    assert_eq!(
                        inventory_count(
                            &world
                                .tiles
                                .get(&ASSEMBLER_POSITION)
                                .unwrap()
                                .payload_inventory,
                            content
                        ),
                        count
                    );
                }
            }
            seed_positioned_assembler_drones(&world, ASSEMBLER_POSITION);
            {
                let mut assembler = world.tiles.get_mut(&ASSEMBLER_POSITION).unwrap();
                assembler.production_progress = time - 1.0;
                assembler.stored_liquid = CYANOGEN_LIQUID;
                assembler.liquid_amount = 100.0;
            }
            let power = HashMap::from([(module, 1.0), (ASSEMBLER_POSITION, 1.0)]);
            assert!(simulate_erekir_assemblers(
                &world,
                &DashMap::new(),
                1.0,
                &power
            ));
            assert_eq!(produced_units(&world).len(), 1);
            assert_eq!(produced_units(&world)[0].unit_type, output);
            assert!(world
                .tiles
                .get(&ASSEMBLER_POSITION)
                .unwrap()
                .payload_inventory
                .is_empty());
        }
    }
}

#[test]
fn assembler_module_reserves_pending_assembler_input_and_requires_power() {
    let (world, source) = empty_assembler_with_input(393, 1, 0);
    let module_position = offset_position_by(ASSEMBLER_POSITION, 0, 18);
    let module = world.tiles.get(&module_position).unwrap().clone();
    let payload = assembler_input_payload(39, true);
    {
        let mut assembler = world.tiles.get_mut(&ASSEMBLER_POSITION).unwrap();
        assembler.payload_inventory = vec![(39, 5)];
        assembler.payload = Some(Box::new(payload.clone()));
        assembler.payload_accum = vec![-20.0, 0.0];
    }
    let source_tile = world.tiles.get(&source).unwrap().clone();
    assert!(
        !front_accepts_payload(&world, &module, &source_tile, &payload),
        "last slot reserved by matching input already in assembler"
    );
    world.tiles.get_mut(&ASSEMBLER_POSITION).unwrap().payload = None;
    assert!(front_accepts_payload(
        &world,
        &module,
        &source_tile,
        &payload
    ));
    {
        let mut live = world.tiles.get_mut(&module_position).unwrap();
        live.payload = Some(Box::new(payload));
        live.payload_accum = vec![20.0, 0.0];
    }
    simulate_erekir_assemblers(&world, &DashMap::new(), 30.0, &HashMap::new());
    assert!(
        world.tiles.get(&module_position).unwrap().payload.is_some(),
        "unpowered module holds payload at centre"
    );
    assert_eq!(
        inventory_count(
            &world
                .tiles
                .get(&ASSEMBLER_POSITION)
                .unwrap()
                .payload_inventory,
            39
        ),
        5
    );
    simulate_erekir_assemblers(
        &world,
        &DashMap::new(),
        1.0,
        &HashMap::from([(module_position, 1.0)]),
    );
    assert!(world.tiles.get(&module_position).unwrap().payload.is_none());
    assert_eq!(
        inventory_count(
            &world
                .tiles
                .get(&ASSEMBLER_POSITION)
                .unwrap()
                .payload_inventory,
            39
        ),
        6
    );
}

#[test]
fn assembler_module_in_transit_snapshots_and_actual_json_checkpoint_keep_payloads() {
    assert!(
        crate::network::buildings::snapshot::is_batch_snapshot_supported(396),
        "module inputs must be included in periodic BlockSnapshot batches"
    );
    for (content, is_unit, name) in [(39, true, "unit"), (243, false, "block")] {
        let (world, _) = empty_assembler_with_input(393, 1, 0);
        let module = offset_position_by(ASSEMBLER_POSITION, 0, 18);
        let source = offset_position_by(module, 1, 4);
        let mut conveyor = erekir_tile(source, 400, 3);
        conveyor.occupied = block_footprint(&world, source, 400).unwrap();
        conveyor.payload = Some(Box::new(assembler_input_payload(content, is_unit)));
        world.tiles.insert(source, conveyor);
        assert!(crate::network::simulation::simulate_payload_conveyors(
            &world,
            &DashMap::new(),
            35.0
        ));
        let mut snapshot = world.tiles.get(&module).unwrap().clone();
        assert_eq!(snapshot.payload_accum, [0.0, 20.0]);
        let mut bytes = Vec::new();
        encode_dynamic_tile_sync(&mut bytes, &snapshot, &HashMap::new(), Some(&world)).unwrap();
        if let Ok(directory) = std::env::var("OXIDE_ASSEMBLER_FIXTURE_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                std::path::Path::new(&directory)
                    .join(format!("assembler-module-in-transit-{name}-sync.bin")),
                bytes,
            )
            .unwrap();
        }
        snapshot.health = crate::game::content::block_health(396);
        let tiles = DashMap::new();
        tiles.insert(module, snapshot);
        let saved = crate::network::wire::persistence::snapshot_persisted_world(
            &tiles,
            &world.game_state,
            &world.enemies,
            &world.base_buildings,
            &world.player_profiles,
            &world.building_commands,
            &world.unit_orders,
            &world.team_build_plans.read(),
            &world.cores,
            &world.logic_flags,
            &world.puddles,
            String::new(),
        );
        let path = std::env::temp_dir().join(format!(
            "oxide-module-json-{}-{content}.json",
            std::process::id()
        ));
        crate::network::wire::persistence::persist_world_sync(&path, &saved).unwrap();
        let restored =
            crate::network::wire::persistence::load_tiles(&path, Some((60, 60))).unwrap();
        std::fs::remove_file(path).unwrap();
        let loaded = restored
            .tiles
            .get(&module)
            .expect("module input survives load validation");
        assert_eq!(loaded.payload_accum, [0.0, 20.0]);
        assert!(loaded.payload.is_some());
    }
}
