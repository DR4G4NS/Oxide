use super::*;

fn place(world: &DynamicWorld, pos: i32, block: i16, rotation: u8) {
    let mut tile = erekir_tile(pos, block, rotation);
    tile.occupied =
        crate::network::buildings::construction::block_footprint(world, pos, block).unwrap();
    for cell in &tile.occupied {
        assert!(
            world.tile_footprint.insert(*cell, pos).is_none(),
            "overlapping fixture"
        );
    }
    world.tiles.insert(pos, tile);
}

fn amount(tile: &DynamicTile, liquid: i16) -> f32 {
    tile.liquid_inventory
        .iter()
        .find(|(id, _)| *id == liquid)
        .map(|(_, n)| *n)
        .unwrap_or(if tile.stored_liquid == liquid {
            tile.liquid_amount
        } else {
            0.0
        })
}

#[test]
fn electrolyzer_real_footprint_outputs_both_gases_across_snapshots() {
    for rotation in 0..4 {
        let world = erekir_test_world();
        let pos = (20 << 16) | 20;
        place(&world, pos, 200, rotation);
        // 1x1 routers touch the 3x3 producer's external edges, never its interior.
        let ozone = erekir_offset_by(pos, (rotation + 1) % 4, 2);
        let hydrogen = erekir_offset_by(pos, (rotation + 3) % 4, 2);
        place(&world, ozone, 289, 0);
        place(&world, hydrogen, 289, 0);
        let power = HashMap::from([(pos, 1.0)]);
        for tick in 1..=480 {
            accept_liquid(&world, pos, 0, 10.0 / 60.0);
            simulate_erekir_crafters(&world, 1.0, &power);
            if tick % 360 == 0 {
                let tile = world.tiles.get(&pos).unwrap().clone();
                let mut bytes = Vec::new();
                crate::network::buildings::snapshot::encode_generic_crafter_sync(
                    &mut bytes, &tile, &power,
                )
                .unwrap();
                assert!(!bytes.is_empty());
            }
        }
        assert!(
            amount(&world.tiles.get(&ozone).unwrap(), 7) > 1.0,
            "rotation {rotation}: ozone never left footprint"
        );
        assert!(
            amount(&world.tiles.get(&hydrogen).unwrap(), 8) > 1.0,
            "rotation {rotation}: hydrogen never left footprint"
        );
    }
}

#[test]
fn electrolyzer_buffers_outputs_and_stops_when_both_full() {
    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    place(&world, pos, 200, 0);
    let power = HashMap::from([(pos, 1.0)]);
    for _ in 0..1000 {
        accept_liquid(&world, pos, 0, 1.0);
        simulate_erekir_crafters(&world, 1.0, &power);
    }
    let before = world.tiles.get(&pos).unwrap().clone();
    assert!((amount(&before, 7) - 50.0).abs() < 0.01);
    assert!((amount(&before, 8) - 50.0).abs() < 0.01);
    simulate_erekir_crafters(&world, 60.0, &power);
    let after = world.tiles.get(&pos).unwrap().clone();
    assert_eq!(
        amount(&before, 0),
        amount(&after, 0),
        "full outputs must stop water consumption"
    );
}

#[test]
fn electrolyzer_fractional_water_cannot_create_extra_gas() {
    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    place(&world, pos, 200, 0);
    assert_eq!(accept_liquid(&world, pos, 0, 0.05), 0.05);
    simulate_erekir_crafters(&world, 600.0, &HashMap::from([(pos, 1.0)]));
    let tile = world.tiles.get(&pos).unwrap();
    assert!((amount(&tile, 7) - 0.02).abs() < 0.0001);
    assert!((amount(&tile, 8) - 0.03).abs() < 0.0001);
}

#[test]
fn electrolyzer_rejects_non_input_liquids() {
    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    place(&world, pos, 200, 0);
    assert_eq!(accept_liquid(&world, pos, 7, 1.0), 0.0);
    assert_eq!(accept_liquid(&world, pos, 8, 1.0), 0.0);
}

#[test]
fn liquid_crafter_snapshot_keeps_all_three_liquids_without_duplication() {
    let mut tile = erekir_tile((20 << 16) | 20, 200, 0);
    tile.stored_liquid = 0;
    tile.liquid_amount = 12.0;
    tile.liquid_inventory = vec![(0, 12.0), (7, 8.0), (8, 9.0)];
    let mut bytes = Vec::new();
    crate::network::buildings::snapshot::write_liquid_module(&mut bytes, &tile).unwrap();
    assert_eq!(i16::from_be_bytes([bytes[0], bytes[1]]), 3);
    assert_eq!(bytes.len(), 20);
}

#[test]
fn atmospheric_concentrator_real_heat_and_single_continuous_output() {
    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    let source = (17 << 16) | 20;
    let receiver = (22 << 16) | 20;
    place(&world, pos, 201, 0);
    place(&world, source, 205, 0); // phase heater, same-size full contact
    place(&world, receiver, 289, 0);
    world.tiles.get_mut(&source).unwrap().mass_driver_rotation = 12.0;
    assert!((erekir_heat_at(&world, pos) - 12.0).abs() < 0.001);
    let power = HashMap::from([(pos, 1.0)]);
    for tick in 1..=480 {
        // Scheduler order: heat pass then crafter; phase heater is frozen
        // at the controlled input for this fractional-heat fixture.
        simulate_heat_network(&world, 1.0, &power);
        world.tiles.get_mut(&source).unwrap().mass_driver_rotation = 12.0;
        simulate_erekir_crafters(&world, 1.0, &power);
        if tick % 360 == 0 {
            let tile = world.tiles.get(&pos).unwrap().clone();
            let mut bytes = Vec::new();
            crate::network::buildings::snapshot::encode_generic_crafter_sync(
                &mut bytes, &tile, &power,
            )
            .unwrap();
            assert!(!bytes.is_empty());
        }
    }
    let produced = amount(&world.tiles.get(&pos).unwrap(), 9)
        + amount(&world.tiles.get(&receiver).unwrap(), 9);
    assert!(
        (produced - 64.0).abs() < 0.01,
        "half heat must yield exactly64 nitrogen, got {produced}"
    );
    assert!(amount(&world.tiles.get(&receiver).unwrap(), 9) > 1.0);
}

#[test]
fn export_liquid_crafter_current_jar_fixtures() {
    let path = std::path::Path::new("target/liquid-crafter-fixtures");
    std::fs::create_dir_all(path).unwrap();
    for (block, name, liquids) in [
        (200, "electrolyzer", vec![(0, 12.0), (7, 8.0), (8, 9.0)]),
        (201, "concentrator", vec![(9, 15.0)]),
    ] {
        let mut tile = erekir_tile((20 << 16) | 20, block, 0);
        tile.liquid_inventory = liquids;
        tile.stored_liquid = if block == 200 { 0 } else { 9 };
        tile.liquid_amount = if block == 200 { 12.0 } else { 15.0 };
        let mut sync = Vec::new();
        crate::network::buildings::snapshot::encode_generic_crafter_sync(
            &mut sync,
            &tile,
            &HashMap::from([(tile.position, 1.0)]),
        )
        .unwrap();
        std::fs::write(path.join(format!("{name}-sync.bin")), sync).unwrap();
        let save = crate::engine::save_io::write_msav_building_chunk(&tile).unwrap();
        std::fs::write(path.join(format!("{name}-save.bin")), save).unwrap();
    }
}

#[test]
fn electrolyzer_near_full_matches_official_input_drain() {
    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    place(&world, pos, 200, 0);
    {
        let mut tile = world.tiles.get_mut(&pos).unwrap();
        tile.liquid_inventory = vec![(0, 12.0), (7, 49.99), (8, 49.99)];
        tile.stored_liquid = 0;
        tile.liquid_amount = 12.0;
    }
    simulate_erekir_crafters(&world, 1.0, &HashMap::from([(pos, 1.0)]));
    let tile = world.tiles.get(&pos).unwrap();
    assert!((amount(&tile, 0) - (12.0 - 10.0 / 60.0)).abs() < 0.00001);
    assert!((amount(&tile, 7) - 50.0).abs() < 0.00001);
    assert!((amount(&tile, 8) - 50.0).abs() < 0.00001);
}

#[test]
fn liquid_crafter_save_restores_normalized_progress() {
    for (block, time) in [(200, 10.0), (201, 80.0)] {
        let mut tile = erekir_tile((20 << 16) | 20, block, 0);
        tile.production_progress = time * 0.4;
        tile.liquid_inventory = vec![(9, 15.0)];
        let chunk = crate::engine::save_io::write_msav_building_chunk(&tile).unwrap();
        let tail = &chunk[chunk.len() - 8..];
        assert!((f32::from_be_bytes(tail[..4].try_into().unwrap()) - 0.4).abs() < 0.0001);
        tile.production_progress = 0.0;
        crate::engine::save_io::apply_msav_building_tail(&mut tile, tail).unwrap();
        assert!((tile.production_progress - time * 0.4).abs() < 0.0001);
    }
}

#[test]
fn liquid_crafter_full_outputs_stop_power_demand() {
    let world = erekir_test_world();
    let mut tile = erekir_tile((20 << 16) | 20, 200, 0);
    tile.liquid_inventory = vec![(0, 12.0), (7, 50.0), (8, 50.0)];
    assert!(!should_consume_power(&world, &tile));
    tile.liquid_inventory[1].1 = 10.0;
    assert!(should_consume_power(&world, &tile));
    tile.block = 201;
    tile.liquid_inventory = vec![(9, 60.0)];
    assert!(!should_consume_power(&world, &tile));
}

#[test]
fn export_electrolyzer_prediction_across_snapshot_fixtures() {
    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    place(&world, pos, 200, 0);
    let power = HashMap::from([(pos, 1.0)]);
    let path = std::path::Path::new("target/liquid-crafter-fixtures");
    std::fs::create_dir_all(path).unwrap();
    for tick in 1..=480 {
        accept_liquid(&world, pos, 0, 50.0);
        simulate_erekir_crafters(&world, 1.0, &power);
        if tick == 360 || tick == 480 {
            let tile = world.tiles.get(&pos).unwrap().clone();
            let mut bytes = Vec::new();
            crate::network::buildings::snapshot::encode_generic_crafter_sync(
                &mut bytes, &tile, &power,
            )
            .unwrap();
            std::fs::write(path.join(format!("electrolyzer-{tick}-sync.bin")), bytes).unwrap();
            assert!((amount(&tile, 7) - tick as f32 * 4.0 / 60.0).abs() < 0.001);
            assert!((amount(&tile, 8) - tick as f32 * 6.0 / 60.0).abs() < 0.001);
        }
    }
}
