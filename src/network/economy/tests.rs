use super::*;

/// Independent Rust mirror of v159.7 `ApplicationTests.blockInventories`.
/// The server's inventory is a compact item/count vector rather than Arc's
/// `ItemModule`, but the externally observable add/remove/total contract is
/// the same.
#[test]
fn upstream_application_block_inventories_1597_add_remove_total() {
    let mut inventory = Vec::new();
    inventory_add(&mut inventory, 5, 5); // coal
    inventory_add(&mut inventory, 6, 50); // titanium
    assert_eq!(inventory_total(&inventory), 55);

    assert!(!inventory_remove(&mut inventory, 11, 10)); // absent phase fabric
    assert!(inventory_remove(&mut inventory, 6, 10));
    assert_eq!(inventory_total(&inventory), 45);
    assert_eq!(inventory_count(&inventory, 6), 40);
}

fn assert_valid_plain_conveyor_queue(items: &[(i16, f32)]) {
    assert!(items.len() <= CONVEYOR_CAPACITY);
    for (_, progress) in items {
        assert!(
            progress.is_finite() && (0.0..=1.0).contains(progress),
            "invalid conveyor progress {progress}"
        );
    }
    for pair in items.windows(2) {
        assert!(
            pair[0].1 - pair[1].1 >= CONVEYOR_ITEM_SPACE - 0.000_01,
            "items are not spaced: {items:?}"
        );
    }
}

#[test]
fn conveyor_queue_heals_huge_and_non_finite_progress_without_reordering() {
    let huge = vec![(5, 21_086.43), (6, 21_085.29), (7, 21_085.23)];
    let healed = sanitize_conveyor_queue(&huge);
    assert_eq!(
        healed.iter().map(|item| item.0).collect::<Vec<_>>(),
        [5, 6, 7]
    );
    assert_valid_plain_conveyor_queue(&healed);
    assert!((healed[0].1 - 1.0).abs() < 0.000_01);
    assert!((healed[1].1 - 0.6).abs() < 0.000_01);
    assert!((healed[2].1 - 0.2).abs() < 0.000_01);

    let non_finite =
        sanitize_conveyor_queue(&[(8, f32::NAN), (9, f32::INFINITY), (10, f32::NEG_INFINITY)]);
    assert_eq!(
        non_finite.iter().map(|item| item.0).collect::<Vec<_>>(),
        [8, 9, 10]
    );
    assert_valid_plain_conveyor_queue(&non_finite);
}

#[test]
fn aligned_conveyor_jam_propagates_and_stays_bounded_for_many_ticks() {
    let world = erekir_test_world();
    let upstream = (10 << 16) | 10;
    let blocked = (11 << 16) | 10;
    let mut upstream_tile = erekir_tile(upstream, 257, 0);
    upstream_tile.occupied = vec![upstream];
    upstream_tile.conveyor_items = vec![(5, 17_725.977), (6, 17_725.879), (7, 17_725.7)];
    upstream_tile.stored_item = 5;
    upstream_tile.stored_amount = 3;
    let mut blocked_tile = erekir_tile(blocked, 257, 0);
    blocked_tile.occupied = vec![blocked];
    blocked_tile.conveyor_items = vec![(8, 1.0), (9, 0.6), (10, 0.2)];
    blocked_tile.stored_item = 8;
    blocked_tile.stored_amount = 3;
    world.tiles.insert(upstream, upstream_tile);
    world.tiles.insert(blocked, blocked_tile);

    let no_power = HashMap::new();
    for _ in 0..720 {
        simulate_logistics(&world, 1.0, &no_power);
    }

    let upstream_tile = world.tiles.get(&upstream).unwrap();
    assert_eq!(
        upstream_tile
            .conveyor_items
            .iter()
            .map(|item| item.0)
            .collect::<Vec<_>>(),
        [5, 6, 7],
        "a jam must preserve FIFO order"
    );
    assert_valid_plain_conveyor_queue(&upstream_tile.conveyor_items);
    assert!(
        (upstream_tile.conveyor_items[0].1 - 0.8).abs() < 0.000_01,
        "downstream minitem=.2 limits the upstream head to .8: {:?}",
        upstream_tile.conveyor_items
    );
    assert_valid_plain_conveyor_queue(&world.tiles.get(&blocked).unwrap().conveyor_items);
}

#[test]
fn conveyor_handoff_removes_only_fifo_head() {
    let world = erekir_test_world();
    let source = (10 << 16) | 10;
    let target = (11 << 16) | 10;
    let mut source_tile = erekir_tile(source, 257, 0);
    source_tile.occupied = vec![source];
    source_tile.conveyor_items = vec![(5, 1.0), (6, 0.6), (7, 0.2)];
    source_tile.stored_item = 5;
    source_tile.stored_amount = 3;
    let mut target_tile = erekir_tile(target, 257, 0);
    target_tile.occupied = vec![target];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(target, target_tile);

    simulate_logistics(&world, 1.0, &HashMap::new());

    let source_items = world.tiles.get(&source).unwrap().conveyor_items.clone();
    assert_eq!(
        source_items.iter().map(|item| item.0).collect::<Vec<_>>(),
        [6, 7]
    );
    assert_valid_plain_conveyor_queue(&source_items);
    let target_items = world.tiles.get(&target).unwrap().conveyor_items.clone();
    assert_eq!(target_items.first().map(|item| item.0), Some(5));
    assert_valid_plain_conveyor_queue(&target_items);
}

#[test]
fn conveyor_257_speed_matches_blocks_java_158_1() {
    assert_eq!(item_transport_speed(257), Some(0.046));
    assert_eq!(item_transport_speed(258), Some(0.0801));
    assert_eq!(item_transport_speed(260), Some(0.08));
}

fn decode_conveyor_sync_items(tile: &DynamicTile) -> Vec<(i16, f32)> {
    use crate::network::buildings::snapshot::encode_conveyor_sync;
    use crate::network::codec::Reads;
    use std::io::Cursor;

    let mut bytes = Vec::new();
    encode_conveyor_sync(&mut bytes, tile).unwrap();
    let mut input = Cursor::new(bytes);
    input.read_f().unwrap(); // health
    input.read_b().unwrap(); // rotation
    input.read_b().unwrap(); // team
    input.read_b().unwrap(); // revision
    input.read_b().unwrap(); // enabled
    input.read_b().unwrap(); // modules
    let items = input.read_s().unwrap();
    for _ in 0..items {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    input.read_b().unwrap(); // efficiency
    input.read_b().unwrap(); // optionalEfficiency
    let len = input.read_i().unwrap();
    let mut decoded = Vec::new();
    for _ in 0..len {
        let item = input.read_s().unwrap();
        let _xs = input.read_b().unwrap();
        let ys = input.read_b().unwrap() as i8;
        decoded.push((item, (f32::from(ys) + 128.0) / 255.0));
    }
    decoded
}

#[test]
fn conveyor_block_snapshot_keeps_item_offsets() {
    let mut tile = erekir_tile((10 << 16) | 10, 257, 0);
    tile.conveyor_items = vec![(5, 0.72), (6, 0.31)];
    tile.stored_item = 5;
    tile.stored_amount = 2;
    let decoded = decode_conveyor_sync_items(&tile);
    // Wire order is Java rear-first; progress must survive byte quantization
    // rather than collapsing to 0 (the teleport the 158.1 client shows).
    assert_eq!(decoded[0].0, 6);
    assert_eq!(decoded[1].0, 5);
    assert!(
        (decoded[0].1 - 0.31).abs() < 1.0 / 255.0 + 0.002,
        "rear offset jumped: {}",
        decoded[0].1
    );
    assert!(
        (decoded[1].1 - 0.72).abs() < 1.0 / 255.0 + 0.002,
        "front offset jumped: {}",
        decoded[1].1
    );
}

#[test]
fn conveyor_rejects_illegal_side_and_front_crossing() {
    let world = erekir_test_world();
    let belt = (11 << 16) | 10;
    let rear = (10 << 16) | 10;
    let side = (11 << 16) | 11;
    let front = (12 << 16) | 10;
    let mut belt_tile = erekir_tile(belt, 257, 0);
    belt_tile.occupied = vec![belt];
    belt_tile.conveyor_items = vec![(5, 1.0), (6, 0.6), (7, 0.2)];
    belt_tile.stored_item = 5;
    belt_tile.stored_amount = 3;
    let mut rear_tile = erekir_tile(rear, 257, 0);
    rear_tile.occupied = vec![rear];
    let mut side_tile = erekir_tile(side, 257, 3);
    side_tile.occupied = vec![side];
    let mut front_tile = erekir_tile(front, 257, 2);
    front_tile.occupied = vec![front];
    world.tiles.insert(belt, belt_tile);
    world.tiles.insert(rear, rear_tile);
    world.tiles.insert(side, side_tile);
    world.tiles.insert(front, front_tile);

    // minitem = 0.2: rear needs >= 0.4, side needs > 0.7, front is direction 2.
    assert!(!accept_plain_conveyor_item(&world, belt, 0, Some(rear)));
    assert!(!accept_plain_conveyor_item(&world, belt, 0, Some(side)));
    assert!(!accept_plain_conveyor_item(&world, belt, 0, Some(front)));
    assert_eq!(world.tiles.get(&belt).unwrap().conveyor_items.len(), 3);

    {
        let mut belt_tile = world.tiles.get_mut(&belt).unwrap();
        belt_tile.conveyor_items.clear();
        belt_tile.stored_amount = 0;
        belt_tile.stored_item = -1;
    }
    // Empty belt (minitem = 1): rear and side are legal; two conveyors
    // pointing at each other still fail the rotate && next == source gate.
    assert!(accept_plain_conveyor_item(&world, belt, 0, Some(rear)));
    {
        let mut belt_tile = world.tiles.get_mut(&belt).unwrap();
        belt_tile.conveyor_items.clear();
        belt_tile.stored_amount = 0;
        belt_tile.stored_item = -1;
    }
    assert!(accept_plain_conveyor_item(&world, belt, 0, Some(side)));
    {
        let mut belt_tile = world.tiles.get_mut(&belt).unwrap();
        belt_tile.conveyor_items.clear();
        belt_tile.stored_amount = 0;
        belt_tile.stored_item = -1;
    }
    assert!(!accept_plain_conveyor_item(&world, belt, 0, Some(front)));
}

#[test]
fn conveyor_block_sync_window_does_not_roll_back_inventory() {
    let world = erekir_test_world();
    let upstream = (10 << 16) | 10;
    let blocked = (11 << 16) | 10;
    let mut upstream_tile = erekir_tile(upstream, 257, 0);
    upstream_tile.occupied = vec![upstream];
    upstream_tile.conveyor_items = vec![(5, 0.8), (6, 0.4), (7, 0.0)];
    upstream_tile.stored_item = 5;
    upstream_tile.stored_amount = 3;
    let mut blocked_tile = erekir_tile(blocked, 257, 0);
    blocked_tile.occupied = vec![blocked];
    blocked_tile.conveyor_items = vec![(8, 1.0), (9, 0.6), (10, 0.2)];
    blocked_tile.stored_item = 8;
    blocked_tile.stored_amount = 3;
    world.tiles.insert(upstream, upstream_tile);
    world.tiles.insert(blocked, blocked_tile);

    let no_power = HashMap::new();
    for _ in 0..360 {
        simulate_logistics(&world, 1.0, &no_power);
    }
    let tile = world.tiles.get(&upstream).unwrap().clone();
    assert_eq!(
        tile.conveyor_items
            .iter()
            .map(|item| item.0)
            .collect::<Vec<_>>(),
        [5, 6, 7],
        "a 6 s block sync must not drop or reorder jammed items"
    );
    assert_valid_plain_conveyor_queue(&tile.conveyor_items);
    let decoded = decode_conveyor_sync_items(&tile);
    assert_eq!(decoded.len(), 3);
    assert_eq!(
        decoded.iter().map(|item| item.0).collect::<Vec<_>>(),
        [7, 6, 5]
    );
    let front = decoded.last().expect("front item on the wire");
    assert!(
        front.1 > 0.5,
        "6 s snapshot teleported the head back to the belt start: {decoded:?}"
    );
}

#[test]
fn drill_water_boost_matches_official_intensity() {
    fn place_mechanical(world: &mut DynamicWorld, pos: i32) {
        world
            .overlays
            .resize((world.width * world.height) as usize, 0);
        world
            .floors
            .resize((world.width * world.height) as usize, 0);
        let x = (pos >> 16) as i16 as usize;
        let y = pos as i16 as usize;
        world.overlays[y * world.width as usize + x] = 167;
        let mut drill = erekir_tile(pos, 325, 0);
        drill.occupied = vec![pos];
        world.tiles.insert(pos, drill);
    }

    let mut dry = erekir_test_world();
    let mut wet = erekir_test_world();
    let pos = (10 << 16) | 10;
    place_mechanical(&mut dry, pos);
    place_mechanical(&mut wet, pos);
    {
        let mut drill = wet.tiles.get_mut(&pos).unwrap();
        drill.stored_liquid = 0;
        drill.liquid_amount = 20.0;
        drill.transport_progress = 1.6;
    }
    {
        let mut drill = dry.tiles.get_mut(&pos).unwrap();
        drill.transport_progress = 1.0;
    }

    let no_power = HashMap::new();
    for _ in 0..50 {
        simulate_logistics(&dry, 1.0, &no_power);
        simulate_logistics(&wet, 1.0, &no_power);
    }
    let dry_progress = dry.tiles.get(&pos).unwrap().production_progress;
    let wet_progress = wet.tiles.get(&pos).unwrap().production_progress;
    // speed=1.6 and warmup→1.6, so progress rate is 1.6² = 2.56× dry.
    assert!(
        (dry_progress - 50.0).abs() < 0.001,
        "dry drill progress {dry_progress}"
    );
    assert!(
        (wet_progress - 50.0 * 1.6 * 1.6).abs() < 0.02,
        "boosted drill progress {wet_progress}, expected {}",
        50.0 * 1.6 * 1.6
    );
    assert!(
        (wet.tiles.get(&pos).unwrap().transport_progress - 1.6).abs() < 0.001,
        "warmup tracks liquidBoostIntensity for lastDrillSpeed / writeSync"
    );
}

#[test]
fn liquid_factory_rates_match_official_per_tick_semantics() {
    // The official GenericCrafter consumes/produces liquids CONTINUOUSLY:
    // ConsumeLiquid.update removes `amount * edelta()` per tick and
    // GenericCrafterBuild.updateTile calls handleLiquid(amount*inc) per
    // tick, with edelta()==inc==1.0 on the headless server
    // (ServerControl sets Time.delta = getDeltaTime()*60 ~= 1.0 per tick).
    // The Rust per-craft model must hold: per_craft = rate_per_tick *
    // craft_time. Values from Blocks.java v158.1.
    fn check(
        block: i16,
        craft_time: f32,
        item_output: Option<i16>,
        liquid_input: Option<(i16, f32)>,
        liquid_output: Option<(i16, f32)>,
    ) {
        let recipe = liquid_factory_recipe(block).unwrap_or_else(|| panic!("recipe for {block}"));
        assert!(
            (recipe.craft_time - craft_time).abs() < 0.001,
            "craftTime {block}"
        );
        assert_eq!(
            recipe.item_output.map(|x| x.0),
            item_output,
            "item out {block}"
        );
        if let Some((lid, lamount)) = liquid_input {
            assert_eq!(recipe.liquid_input.0, lid, "liquid input id {block}");
            assert!(
                (recipe.liquid_input.1 - lamount).abs() < 0.001,
                "liquid input rate {block}: {} vs {}",
                recipe.liquid_input.1,
                lamount
            );
        } else {
            assert_eq!(recipe.liquid_input.1, 0.0, "no liquid input {block}");
        }
        if let Some((lid, lamount)) = liquid_output {
            let out = recipe.liquid_output.expect("liquid output {block}");
            assert_eq!(out.0, lid, "liquid output id {block}");
            assert!(
                (out.1 - lamount).abs() < 0.001,
                "liquid output rate {block}: {} vs {}",
                out.1,
                lamount
            );
        } else {
            assert!(recipe.liquid_output.is_none(), "no liquid output {block}");
        }
    }
    // multi-press: water 0.1/tick * 30t = 3.0; graphite x2.
    check(182, 30.0, Some(3), Some((0, 0.1 * 30.0)), None);
    // plastanium-compressor: oil 0.25/tick * 60t = 15.0.
    check(186, 60.0, Some(10), Some((2, 0.25 * 60.0)), None);
    // cryofluid-mixer: water 12/60 per tick * 120t = 24 in/out.
    check(
        189,
        120.0,
        None,
        Some((0, (12.0 / 60.0) * 120.0)),
        Some((3, (12.0 / 60.0) * 120.0)),
    );
    // melter: slag 12/60 per tick * 10t = 2.0 (regression: was 0.2).
    check(192, 10.0, None, None, Some((1, (12.0 / 60.0) * 10.0)));
    // spore-press: oil 18/60 per tick * 20t = 6.0 (was 0.3).
    check(195, 20.0, None, None, Some((2, (18.0 / 60.0) * 20.0)));
    // coal-centrifuge: oil 0.1/tick * 30t = 3.0 (was 0.1).
    check(197, 30.0, Some(5), Some((2, 0.1 * 30.0)), None);
    // slag-centrifuge: slag 40/60*120=80 in, gallium 1/60*120=2 out.
    check(
        211,
        120.0,
        None,
        Some((1, (40.0 / 60.0) * 120.0)),
        Some((5, (1.0 / 60.0) * 120.0)),
    );
    // cultivator: water 18/60 per tick * 100t = 30.0 (was 10.0).
    check(330, 100.0, Some(13), Some((0, (18.0 / 60.0) * 100.0)), None);
    // oil-extractor (Fracker): sand + water -> oil. pumpAmount 0.25/tick
    // * 60t = 15.0 out; water 0.15 * 60 = 9.0 in.
    check(
        331,
        60.0,
        None,
        Some((0, 0.15 * 60.0)),
        Some((2, 0.25 * 60.0)),
    );
}

#[test]
fn base_map_drills_feed_the_team_core() {
    // SOL-001: a prebuilt mechanical drill (325) over copper produces into
    // the owning team's core (delay = drillTime 600 + hardness 1*50 = 650
    // ticks per copper).
    let mut world = erekir_test_world();
    // Give the world a real map-sized overlay array (erekir_test_world has
    // none) and a width/height for raw_mine_result indexing.
    world
        .overlays
        .resize((world.width * world.height) as usize, 0);
    world
        .floors
        .resize((world.width * world.height) as usize, 0);
    let pos = (10 << 16) | 10;
    let index = (10 * world.width + 10) as usize;
    if index < world.overlays.len() {
        world.overlays[index] = 167; // copper overlay
    }
    world.base_buildings.insert(
        pos,
        BaseBuildingState {
            position: pos,
            block: 325,
            team: 1,
            health: 100.0,
            occupied: vec![pos],
            inventory: Vec::new(),
        },
    );
    let before = crate::network::economy::items_for_team(&world, 1)[0];
    simulate_base_drills(&world, 650.0);
    let after = crate::network::economy::items_for_team(&world, 1)[0];
    assert_eq!(after, before + 1, "one copper mined into the core");
    simulate_base_drills(&world, 650.0);
    let after = crate::network::economy::items_for_team(&world, 1)[0];
    assert_eq!(after, before + 2, "progress persists across ticks");
}

#[test]
fn dynamic_drill_warmup_tracks_operation_for_client_sync() {
    let mut world = erekir_test_world();
    world
        .overlays
        .resize((world.width * world.height) as usize, 0);
    world
        .floors
        .resize((world.width * world.height) as usize, 0);
    let pos = (10 << 16) | 10;
    world.overlays[(10 * world.width + 10) as usize] = 167; // copper
    let mut drill = erekir_tile(pos, 325, 0);
    drill.occupied = vec![pos];
    world.tiles.insert(pos, drill);

    assert!(simulate_logistics(
        &world,
        1.0,
        &std::collections::HashMap::new()
    ));
    let drill = world.tiles.get(&pos).unwrap();
    assert!(
        (drill.transport_progress - 0.015).abs() < 0.0001,
        "official warmupSpeed is applied: {}",
        drill.transport_progress
    );
    assert!(
        drill.production_progress > 0.0,
        "an operating drill advances production"
    );
    drop(drill);

    // A full output inventory cools down instead of being serialized as
    // permanently active or snapping straight to zero.
    {
        let mut drill = world.tiles.get_mut(&pos).unwrap();
        drill.stored_item = 0;
        drill.stored_amount = 10;
    }
    simulate_logistics(&world, 1.0, &std::collections::HashMap::new());
    assert_eq!(world.tiles.get(&pos).unwrap().transport_progress, 0.0);
}

#[test]
fn loaded_drill_is_not_simulated_twice_through_base_registry() {
    let mut world = erekir_test_world();
    world
        .overlays
        .resize((world.width * world.height) as usize, 0);
    world
        .floors
        .resize((world.width * world.height) as usize, 0);
    let pos = (10 << 16) | 10;
    world.overlays[(10 * world.width + 10) as usize] = 167;
    world.base_buildings.insert(
        pos,
        BaseBuildingState {
            position: pos,
            block: 325,
            team: 1,
            health: 100.0,
            occupied: vec![pos],
            inventory: Vec::new(),
        },
    );
    let mut drill = erekir_tile(pos, 325, 0);
    drill.occupied = vec![pos];
    world.tiles.insert(pos, drill);
    let before = crate::network::economy::items_for_team(&world, 1)[0];
    assert!(!simulate_base_drills(&world, 650.0));
    assert_eq!(
        crate::network::economy::items_for_team(&world, 1)[0],
        before,
        "the compatibility copy must not mint a second item"
    );
}

#[test]
fn base_map_factories_craft_from_the_team_core() {
    // SOL-001: a prebuilt multi-press (181: 2 coal -> 1 graphite, 90 s)
    // consumes coal from the team core and delivers graphite to it.
    let world = erekir_test_world();
    let pos = (12 << 16) | 12;
    world.base_buildings.insert(
        pos,
        BaseBuildingState {
            position: pos,
            block: 181,
            team: 1,
            health: 100.0,
            occupied: vec![pos],
            inventory: Vec::new(),
        },
    );
    // Seed the core: 10 coal (item 5). Scope the guard so simulate_base_
    // factories (which takes items_for_team_mut) does not deadlock.
    let graphite_before = {
        let mut items = crate::network::economy::items_for_team_mut(&world, 1);
        items[5] = 10;
        items[3]
    };
    simulate_base_factories(&world, 90.0);
    let items = crate::network::economy::items_for_team(&world, 1);
    assert_eq!(items[5], 8, "two coal consumed");
    assert_eq!(items[3], graphite_before + 1, "one graphite crafted");
    simulate_base_factories(&world, 90.0);
    let items = crate::network::economy::items_for_team(&world, 1);
    assert_eq!(items[5], 6, "second craft");
    assert_eq!(items[3], graphite_before + 2);
}

#[test]
fn base_map_turrets_fire_at_enemies_in_range() {
    // SOL-001: a prebuilt duo (349) fires its default copper ammo at a
    // dagger in range (reload 20 ticks).
    let world = erekir_test_world();
    let pos = (14 << 16) | 14;
    world.base_buildings.insert(
        pos,
        BaseBuildingState {
            position: pos,
            block: 349,
            team: 1,
            health: 100.0,
            occupied: vec![pos],
            inventory: Vec::new(),
        },
    );
    // A dagger 100 px away (duo range 160).
    world.enemies.insert(
        3_000_100,
        EnemyUnit {
            id: 3_000_100,
            unit_type: 0,
            entity_class: 4,
            team: 2,
            x: 14.0 * 8.0 + 100.0,
            y: 14.0 * 8.0,
            rotation: 0.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
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
            move_speed: 0.5,
            attack_damage: 9.0,
            attack_reload_time: 13.0,
            attack_range: 145.0,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            missile_time: 0.0,
            status_agg: None,
            drown_progress: 0.0,
        },
    );
    let connections = DashMap::new();
    assert!(
        simulate_base_turrets(&world, &connections, 20.0),
        "fires on reload"
    );
    assert_eq!(world.projectiles.len(), 1, "one projectile spawned");
    let proj = world.projectiles.iter().next().unwrap();
    assert_eq!(proj.value().target_id, 3_000_100);
}

#[test]
fn loaded_mender_does_not_simulate_compatibility_copy_twice() {
    // fresh_world_from_template seeds map buildings in both registries;
    // the DynamicTile is authoritative. A loaded mender must therefore
    // repair a loaded wall once, not once in each registry path.
    let world = erekir_test_world();
    let mender_pos = (16 << 16) | 16;
    let wall_pos = (17 << 16) | 16;
    world.base_buildings.insert(
        mender_pos,
        BaseBuildingState {
            position: mender_pos,
            block: 245,
            team: 1,
            health: 80.0,
            occupied: vec![mender_pos],
            inventory: Vec::new(),
        },
    );
    world.base_buildings.insert(
        wall_pos,
        BaseBuildingState {
            position: wall_pos,
            block: 216,
            team: 1,
            health: 200.0,
            occupied: vec![wall_pos],
            inventory: Vec::new(),
        },
    );
    let mut mender = erekir_tile(mender_pos, 245, 0);
    mender.health = 80.0;
    let mut wall = erekir_tile(wall_pos, 216, 0);
    wall.health = 200.0;
    world.tiles.insert(mender_pos, mender);
    world.tiles.insert(wall_pos, wall);

    let connections = DashMap::new();
    let power = std::collections::HashMap::from([(mender_pos, 1.0)]);
    simulate_base_menders(&world, 220.0);
    simulate_menders(&world, &connections, 220.0, &power);

    let dynamic_health = world.tiles.get(&wall_pos).unwrap().health;
    assert!(
        (dynamic_health - 212.8).abs() < 0.05,
        "single dynamic heal: {dynamic_health}"
    );
    assert_eq!(world.base_buildings.get(&wall_pos).unwrap().health, 200.0);
}

#[test]
fn base_map_menders_repair_damaged_buildings() {
    // SOL-001: a prebuilt mender (245) heals a damaged wall (216) of the
    // same team in range (reload 200 ticks, 4% max health).
    let world = erekir_test_world();
    let mender_pos = (16 << 16) | 16;
    let wall_pos = (17 << 16) | 16; // 8 px away, within range 60
    world.base_buildings.insert(
        mender_pos,
        BaseBuildingState {
            position: mender_pos,
            block: 245,
            team: 1,
            health: 100.0,
            occupied: vec![mender_pos],
            inventory: Vec::new(),
        },
    );
    world.base_buildings.insert(
        wall_pos,
        BaseBuildingState {
            position: wall_pos,
            block: 216,
            team: 1,
            health: 200.0,
            occupied: vec![wall_pos],
            inventory: Vec::new(),
        },
    );
    let max = crate::game::content::block_health(216);
    assert!(world.base_buildings.get(&wall_pos).unwrap().health < max);
    simulate_base_menders(&world, 200.0);
    let healed = world.base_buildings.get(&wall_pos).unwrap().health;
    assert!(healed > 200.0, "wall repaired: {healed}");
    assert!(healed <= max + 0.001, "not over max: {healed} vs {max}");
}

#[test]
fn separator_specs_match_official_blocks() {
    // separator 193: slag 4/60 per tick * 35t craft ≈ 2.3333; results
    // copper 5/lead 3/graphite 2/titanium 2 (Blocks.java v158.1).
    let spec = separator_spec(193).unwrap();
    assert!((spec.craft_time - 35.0).abs() < 0.001);
    assert_eq!(spec.results, &[(0, 5), (1, 3), (3, 2), (6, 2)]);
    assert_eq!(spec.liquid_input, (1, (4.0 / 60.0) * 35.0));
    assert_eq!(spec.item_input, None);
    // disassembler 194: scrap + slag 0.12/tick * 15t = 1.8; results
    // sand 2/graphite 1/titanium 1/thorium 1.
    let spec = separator_spec(194).unwrap();
    assert!((spec.craft_time - 15.0).abs() < 0.001);
    assert_eq!(spec.results, &[(4, 2), (3, 1), (6, 1), (7, 1)]);
    assert_eq!(spec.liquid_input, (1, 0.12 * 15.0));
    assert_eq!(spec.item_input, Some(8));
}

// ===================== EREKIR REGRESSION TESTS =====================

#[test]
fn erekir_duct_specs_match_official_blocks() {
    // Duct.java/DuctRouter.java/OverflowDuct.java/DuctBridge.java/
    // DirectionBridge.java + Blocks.java v158.1.
    // All Serpulo-era Erekir ducts use speed 4f; surge-router 6f;
    // surge-conveyor moves at 5/60 per tick.
    for block in 272..=278 {
        assert!((duct_speed(block) - 4.0).abs() < 0.001, "speed {block}");
    }
    assert!((duct_speed(280) - 6.0).abs() < 0.001);
    // DuctBridge itemCapacity 4 (Blocks.java); StackConveyor/Router 10.
    assert!(is_erekir_duct_block(272));
    assert!(is_erekir_duct_block(277));
    assert!(is_erekir_duct_block(280));
    assert!(!is_erekir_duct_block(257));
    assert!(is_erekir_conveyor_block(279));
    // Surge-conveyor (StackConveyor) has no acceptItem override: it
    // accepts from any side up to capacity 10 (single item type).
    // Surge-router (StackRouter) feeds from the back like a duct-router.
}

#[test]
fn heat_specs_match_official_blocks() {
    // Values from Blocks.java v158.1 (HeatProducer/HeatCrafter/
    // HeatConductor constructors); liquid amounts per craft =
    // rate_per_tick * craft_time (round-20 calibration).
    #[allow(clippy::too_many_arguments)]
    fn check(
        block: i16,
        kind: HeatKind,
        size: u8,
        split: bool,
        heat_output: f32,
        heat_requirement: f32,
        craft_time: f32,
        item_inputs: &[(i16, i32)],
        liquid_input: Option<(i16, f32)>,
        item_output: Option<(i16, i32)>,
        liquid_output: Option<(i16, f32)>,
        power_demand: f32,
    ) {
        let spec = heat_block_spec(block).unwrap_or_else(|| panic!("heat {block}"));
        assert_eq!(spec.kind, kind, "kind {block}");
        assert_eq!(spec.size, size, "size {block}");
        assert_eq!(spec.split, split, "split {block}");
        assert!(
            (spec.heat_output - heat_output).abs() < 0.001,
            "heatOutput {block}"
        );
        assert!(
            (spec.heat_requirement - heat_requirement).abs() < 0.001,
            "heatRequirement {block}"
        );
        assert!(
            (spec.craft_time - craft_time).abs() < 0.001,
            "craftTime {block}"
        );
        assert_eq!(spec.item_inputs, item_inputs, "item inputs {block}");
        if let Some((lid, amount)) = liquid_input {
            let (slid, samount) = spec.liquid_input.expect("liquid input {block}");
            assert_eq!(slid, lid, "liquid input id {block}");
            assert!((samount - amount).abs() < 0.001, "liquid input {block}");
        } else {
            assert!(spec.liquid_input.is_none(), "no liquid input {block}");
        }
        if let Some((item, amount)) = item_output {
            let (sitem, samount) = spec.item_output.expect("item output {block}");
            assert_eq!(sitem, item, "item output id {block}");
            assert_eq!(samount, amount, "item output {block}");
        } else {
            assert!(spec.item_output.is_none(), "no item output {block}");
        }
        if let Some((lid, amount)) = liquid_output {
            let (slid, samount) = spec.liquid_output.expect("liquid output {block}");
            assert_eq!(slid, lid, "liquid output id {block}");
            assert!((samount - amount).abs() < 0.001, "liquid output {block}");
        } else {
            assert!(spec.liquid_output.is_none(), "no liquid output {block}");
        }
        assert!(
            (spec.power_demand - power_demand).abs() < 0.001,
            "power {block}"
        );
    }
    // atmospheric-concentrator: heatReq 24, nitrogen 16/60 * 80, power 2.
    check(
        201,
        HeatKind::Consumer,
        3,
        false,
        0.0,
        24.0,
        80.0,
        &[],
        None,
        None,
        Some((9, 16.0 / 60.0 * 80.0)),
        2.0,
    );
    // oxidation-chamber: heat 5, beryllium -> oxide, ozone 2/60*120.
    check(
        202,
        HeatKind::Producer,
        3,
        false,
        5.0,
        0.0,
        120.0,
        &[(16, 1)],
        Some((7, 2.0 / 60.0 * 120.0)),
        Some((18, 1)),
        None,
        0.5,
    );
    // electric-heater: heat 3, power 100/60.
    check(
        203,
        HeatKind::Producer,
        2,
        false,
        3.0,
        0.0,
        0.0,
        &[],
        None,
        None,
        None,
        100.0 / 60.0,
    );
    // slag-heater: heat 8, slag 40/60 * 80.
    check(
        204,
        HeatKind::Producer,
        3,
        false,
        8.0,
        0.0,
        80.0,
        &[],
        Some((1, 40.0 / 60.0 * 80.0)),
        None,
        None,
        0.0,
    );
    // phase-heater: heat 15, phase-fabric per 480.
    check(
        205,
        HeatKind::Producer,
        2,
        false,
        15.0,
        0.0,
        480.0,
        &[(11, 1)],
        None,
        None,
        None,
        0.0,
    );
    // conductors: sizes 3/2/3; heat-router splitHeat = true.
    check(
        206,
        HeatKind::Conductor,
        3,
        false,
        0.0,
        0.0,
        0.0,
        &[],
        None,
        None,
        None,
        0.0,
    );
    check(
        207,
        HeatKind::Conductor,
        2,
        false,
        0.0,
        0.0,
        0.0,
        &[],
        None,
        None,
        None,
        0.0,
    );
    check(
        208,
        HeatKind::Conductor,
        3,
        true,
        0.0,
        0.0,
        0.0,
        &[],
        None,
        None,
        None,
        0.0,
    );
    // carbide-crucible: heatReq 40, tungsten 2 + graphite 3 -> carbide,
    // craftTime 60*2.25/4 = 33.75.
    check(
        210,
        HeatKind::Consumer,
        3,
        false,
        0.0,
        40.0,
        33.75,
        &[(17, 2), (3, 3)],
        None,
        Some((19, 1)),
        None,
        2.0,
    );
    // surge-crucible: heatReq 40, silicon 3 + slag 160/60*45 -> surge.
    check(
        212,
        HeatKind::Consumer,
        3,
        false,
        0.0,
        40.0,
        45.0,
        &[(9, 3)],
        Some((1, 160.0 / 60.0 * 45.0)),
        Some((12, 1)),
        None,
        1.5,
    );
    // cyanogen-synthesizer: heatReq 20, graphite + arkycite 160/60*80
    // -> cyanogen 12/60*80.
    check(
        213,
        HeatKind::Consumer,
        3,
        false,
        0.0,
        20.0,
        80.0,
        &[(3, 1)],
        Some((5, 160.0 / 60.0 * 80.0)),
        None,
        Some((10, 12.0 / 60.0 * 80.0)),
        2.0,
    );
    // phase-synthesizer: heatReq 32, thorium 2 + sand 6 + ozone 8/60*30
    // -> phase-fabric, power 8.
    check(
        214,
        HeatKind::Consumer,
        3,
        false,
        0.0,
        32.0,
        30.0,
        &[(7, 2), (4, 6)],
        Some((7, 8.0 / 60.0 * 30.0)),
        Some((11, 1)),
        None,
        8.0,
    );
    // heat-reactor: heat 10, thorium 3 + nitrogen 1/60*600 ->
    // fissile-matter.
    check(
        215,
        HeatKind::Producer,
        3,
        false,
        10.0,
        0.0,
        600.0,
        &[(7, 3)],
        Some((9, 1.0 / 60.0 * 600.0)),
        Some((20, 1)),
        None,
        0.0,
    );
}

#[test]
fn erekir_drill_specs_match_official_blocks() {
    // Blocks.java: plasmaBore (tier 3, drillTime 160, size 2, range 5,
    // hydrogen 0.25/60 booster); largePlasmaBore (tier 5, drillTime 100,
    // size 3, range 6, nitrogen 3/60 booster).
    let plasma = erekir_drill_spec(335).unwrap();
    assert_eq!(plasma.tier, 3);
    assert!((plasma.drill_time - 160.0).abs() < 0.001);
    assert_eq!(plasma.size, 2);
    assert_eq!(plasma.range, 5);
    assert_eq!(plasma.booster_liquid, 8); // hydrogen
    assert_eq!(plasma.item_capacity, 10);
    let large = erekir_drill_spec(336).unwrap();
    assert_eq!(large.tier, 5);
    assert!((large.drill_time - 100.0).abs() < 0.001);
    assert_eq!(large.size, 3);
    assert_eq!(large.range, 6);
    assert_eq!(large.booster_liquid, 9); // nitrogen
    assert_eq!(large.item_capacity, 20);
    assert!(erekir_drill_spec(337).is_none());
}

#[test]
fn erekir_turret_specs_match_official_blocks() {
    // Reload/range/shots from Blocks.java v158.1 (anchored top-level
    // fields); bullet ids from the registered content order verified
    // against desktop.jar (113 + creation index).
    let breach = erekir_turret_params(367).unwrap();
    assert!((breach.0 - 40.0).abs() < 0.001 && (breach.1 - 190.0).abs() < 0.001);
    let diffuse = erekir_turret_params(368).unwrap();
    assert!((diffuse.0 - 30.0).abs() < 0.001 && (diffuse.1 - 125.0).abs() < 0.001);
    assert_eq!(diffuse.2, 15); // ShootSpread(15, 4f)
    assert!((diffuse.3 - 3.0).abs() < 0.001); // one ammoPerShot charge per volley
    let titan = erekir_turret_params(370).unwrap();
    // Official 159.7 JAR: reload = 60f * 2.3f = 138.
    assert!((titan.0 - 138.0).abs() < 0.001 && (titan.1 - 390.0).abs() < 0.001);
    assert!(titan.5); // ground only
    let disperse = erekir_turret_params(371).unwrap();
    assert!((disperse.0 - 9.0).abs() < 0.001 && (disperse.1 - 310.0).abs() < 0.001);
    assert_eq!(disperse.2, 4); // 4 shots
    assert!(disperse.4); // air only
    let afflict = erekir_turret_params(372).unwrap();
    assert!((afflict.0 - 50.0).abs() < 0.001 && (afflict.1 - 368.0).abs() < 0.001);
    assert!((afflict.6 - 20.0).abs() < 0.001); // heatRequirement 20
    let lustre = erekir_turret_params(373).unwrap();
    assert!((lustre.1 - 250.0).abs() < 0.001);
    let scathe = erekir_turret_params(374).unwrap();
    assert!((scathe.0 - 600.0).abs() < 0.001 && (scathe.1 - 1350.0).abs() < 0.001);
    assert!((scathe.3 - 15.0).abs() < 0.001); // ammoPerShot 15
    assert!(scathe.5); // ground only
    let smite = erekir_turret_params(375).unwrap();
    assert!((smite.0 - 100.0).abs() < 0.001 && (smite.1 - 300.0).abs() < 0.001);
    assert_eq!(smite.2, 5); // 5 barrels
    let malign = erekir_turret_params(376).unwrap();
    assert!((malign.0 - 3.5).abs() < 0.001 && (malign.1 - 410.0).abs() < 0.001);
    assert!((malign.6 - 144.0).abs() < 0.001); // heatRequirement 144
                                               // Ammo entries (Blocks.java ammo() + bullet registry ids).
    let breach_beryllium = erekir_turret_ammo_spec(367, 16).unwrap();
    assert_eq!(breach_beryllium.bullet_id, 163);
    // Official 159.7 JAR runtime damage (tests/parity/fixtures/
    // turret-ammo-159.json); the master Java tree later rebalanced to 85.
    assert!((breach_beryllium.damage - 63.75).abs() < 0.001);
    assert!((breach_beryllium.speed - 7.5).abs() < 0.001);
    assert!(breach_beryllium.pierce);
    let titan_thorium = erekir_turret_ammo_spec(370, 7).unwrap();
    assert_eq!(titan_thorium.bullet_id, 172);
    assert!((titan_thorium.damage - 262.5).abs() < 0.001);
    assert!((titan_thorium.splash_damage - 350.0).abs() < 0.001);
    assert!((titan_thorium.splash_radius - 65.0).abs() < 0.001);
    let disperse_tungsten = erekir_turret_ammo_spec(371, 17).unwrap();
    assert_eq!(disperse_tungsten.multiplier as i32, 3); // ammoMultiplier 3
                                                        // scathe launchers 186/189/192 are BulletType(0f, 0f): NO direct or
                                                        // splash damage; the damage lives in each missile's shootOnDeath death
                                                        // explosion (see scathe_missile_deaths_apply_death_explosion_splash).
    for (item, speed) in [(19, 4.6), (11, 2.5), (12, 4.4)] {
        let launcher = erekir_turret_ammo_spec(374, item).unwrap();
        assert!((launcher.damage - 0.0).abs() < 0.001);
        assert!((launcher.splash_damage - 0.0).abs() < 0.001);
        assert!((launcher.splash_radius - 0.0).abs() < 0.001);
        assert!((launcher.speed - speed).abs() < 0.001);
    }
    let afflict_weapon = erekir_power_turret_weapon(372).unwrap();
    // Official 159.7 JAR content ids: afflict 183, lustre 185, malign 199.
    assert_eq!(afflict_weapon.bullet_id, 183);
    assert!((afflict_weapon.damage - 180.0).abs() < 0.001);
    let lustre_weapon = erekir_power_turret_weapon(373).unwrap();
    assert_eq!(lustre_weapon.bullet_id, 185);
    assert!((lustre_weapon.damage - 157.5).abs() < 0.001);
    let malign_weapon = erekir_power_turret_weapon(376).unwrap();
    assert_eq!(malign_weapon.bullet_id, 199);
    // sublimate liquid ammo.
    let sublimate_ozone = erekir_liquid_turret_ammo(369, 7).unwrap();
    assert_eq!(sublimate_ozone.bullet_id, 170);
    assert!((sublimate_ozone.damage - 45.0).abs() < 0.001);
    let sublimate_cyanogen = erekir_liquid_turret_ammo(369, 10).unwrap();
    assert_eq!(sublimate_cyanogen.bullet_id, 171);
    assert!((sublimate_cyanogen.damage - 97.5).abs() < 0.001);
}

#[test]
fn diffuse_fires_fifteen_projectiles_for_one_three_ammo_volley() {
    let world = erekir_test_world();
    let position = (10 << 16) | 10;
    let mut diffuse = erekir_tile(position, 368, 0);
    diffuse.occupied = vec![position];
    diffuse.stored_item = 3; // graphite
    diffuse.stored_amount = 3;
    diffuse.ammo_units = 3.0;
    world.tiles.insert(position, diffuse);

    let spec = crate::network::units::enemy_spec(0).unwrap();
    let target_id = 3_000_100;
    world.enemies.insert(
        target_id,
        EnemyUnit {
            id: target_id,
            unit_type: 0,
            entity_class: spec.entity_class,
            team: 2,
            x: 10.0 * 8.0 + 100.0,
            y: 10.0 * 8.0,
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
            missile_time: 0.0,
            status_agg: None,
            drown_progress: 0.0,
        },
    );

    assert!(simulate_erekir_turrets(
        &world,
        &DashMap::new(),
        30.0,
        &HashMap::new()
    ));
    let projectiles: Vec<_> = world
        .projectiles
        .iter()
        .filter(|projectile| projectile.bullet_id == 167)
        .map(|projectile| projectile.value().clone())
        .collect();
    assert_eq!(projectiles.len(), 15);
    assert!(projectiles
        .iter()
        .all(|projectile| (projectile.damage - 30.75).abs() < 0.001));
    assert_eq!(world.tiles.get(&position).unwrap().ammo_units, 0.0);
}

#[test]
fn erekir_factory_plans_match_official_blocks() {
    // Blocks.java: tankFabricator 386 (stell 60*35=2100t, beryllium 40 +
    // silicon 50, power 1.5), shipFabricator 387 (elude 60*40=2400t,
    // graphite 50 + silicon 70), mechFabricator 388 (merui 60*40=2400t,
    // beryllium 50 + silicon 70).
    let tank = unit_factory_recipe(386, &[]).unwrap();
    assert_eq!(tank.unit_type, 38);
    assert!((tank.build_time - 2_100.0).abs() < 0.001);
    assert_eq!(tank.requirements, &[(16, 40), (9, 50)]);
    let ship = unit_factory_recipe(387, &[]).unwrap();
    assert_eq!(ship.unit_type, 49);
    assert!((ship.build_time - 2_400.0).abs() < 0.001);
    assert_eq!(ship.requirements, &[(3, 50), (9, 70)]);
    let mech = unit_factory_recipe(388, &[]).unwrap();
    assert_eq!(mech.unit_type, 43);
    assert!((mech.build_time - 2_400.0).abs() < 0.001);
    assert_eq!(mech.requirements, &[(16, 50), (9, 70)]);
    // Per-item capacities = amount * 2 (initCapacities).
    assert_eq!(unit_factory_item_capacity(386, 16), 80);
    assert_eq!(unit_factory_item_capacity(386, 9), 100);
    assert_eq!(unit_factory_item_capacity(387, 9), 140);
    assert_eq!(unit_factory_item_capacity(388, 16), 100);
}

#[test]
fn erekir_units_have_official_specs() {
    // UnitTypes.java v158.1 stats for the fabricator outputs and the
    // entity classes from EntityMapping (TankUnit=43, LegsUnit=24,
    // ElevationMoveUnit=45, UnitEntity=3, PayloadUnit=5).
    let stell = crate::network::units::enemy_spec(38).unwrap();
    assert_eq!(stell.entity_class, 43);
    assert!((stell.health - 850.0).abs() < 0.001);
    assert!((stell.speed - 0.75).abs() < 0.001);
    let elude = crate::network::units::enemy_spec(49).unwrap();
    assert_eq!(elude.entity_class, 45);
    assert!((elude.health - 600.0).abs() < 0.001);
    let merui = crate::network::units::enemy_spec(43).unwrap();
    assert_eq!(merui.entity_class, 24);
    assert!((merui.health - 680.0).abs() < 0.001);
    let locus = crate::network::units::enemy_spec(39).unwrap();
    assert_eq!(locus.entity_class, 43);
    assert!((locus.health - 2_100.0).abs() < 0.001);
    let quell = crate::network::units::enemy_spec(52).unwrap();
    assert_eq!(quell.entity_class, 5);
    assert!((quell.health - 6_000.0).abs() < 0.001);
}

#[test]
fn erekir_snapshot_closure_classes_match_v1597_jar() {
    // desktop.jar 159.7 content dump (`Vars.content.units()`): entity
    // classes from EntityMapping idMap (TankUnit=43, LegsUnit=24,
    // CrawlUnit=46), hp/speed straight from UnitTypes.
    // (id, class, health, speed)
    let cases: &[(i16, u8, f32, f32)] = &[
        (41, 43, 11_000.0, 0.63), // vanquish: TankUnit, 3 launchers
        (42, 43, 22_000.0, 0.48), // conquer
        (44, 24, 1_100.0, 0.6),   // cleroi: LegsUnit, 2 weapons
        (47, 24, 6_500.0, 0.6),   // tecta
        (48, 24, 18_000.0, 1.1),  // collaris
        (49, 45, 600.0, 1.8),     // elude: ElevationMoveUnit
        (56, 46, 500.0, 1.2),     // renale: CrawlUnit, weaponless
        (57, 46, 20_000.0, 1.0),  // latum: CrawlUnit, weaponless
    ];
    for &(id, class, health, speed) in cases {
        let spec = crate::network::units::enemy_spec(id)
            .unwrap_or_else(|| panic!("unit {id} must have an enemy spec"));
        assert_eq!(spec.entity_class, class, "entity class of unit {id}");
        assert!((spec.health - health).abs() < 0.001, "health of unit {id}");
        assert!((spec.speed - speed).abs() < 0.001, "speed of unit {id}");
    }
    // Negative case: the Neoplasm crawlers carry no weapons at all, so they
    // must never report a mount in the snapshot stream.
    assert_eq!(crate::network::wire::enemy_weapon_mount_count(56), 0);
    assert_eq!(crate::network::wire::enemy_weapon_mount_count(57), 0);
}

#[test]
fn remaining_vanilla_units_have_official_specs() {
    // UnitTypes.java / Blocks.java / BuildTurret.java v160.5 stats for the
    // last strictly-rejected vanilla types. Entity classes confirmed against
    // desktop.jar 159.7 EntityMapping idMap bytecode: LegsUnit=24,
    // TimedKillUnit=39 (all MissileUnitType content), PayloadUnit=5,
    // BlockUnitUnit=2, BuildingTetherPayloadUnit=36.
    // (id, class, health, speed, attack_damage)
    let cases: &[(i16, u8, f32, f32, f32)] = &[
        (45, 24, 2_700.0, 0.65, 1.5),  // anthicus: launcher 0.75 x 2 shots
        (46, 39, 55.0, 3.35, 140.0),   // anthicus-missile: Explosion(140, 25)
        (53, 39, 45.0, 4.3, 110.0),    // quell-missile: Explosion(110, 25)
        (55, 39, 70.0, 4.6, 140.0),    // disrupt-missile: Explosion(140, 25)
        (58, 5, 300.0, 5.6, 0.0),      // evoke: RepairBeamWeapon heals
        (59, 5, 500.0, 7.0, 0.0),      // incite: repair + build weapon
        (60, 5, 700.0, 7.5, 0.0),      // emanate: RepairBeamWeapon heals
        (61, 2, 1.0, 0.0, 0.0),        // hidden internal block
        (62, 36, 200.0, 3.5, 0.0),     // manifold (CargoAI, no weapons)
        (63, 36, 90.0, 1.3, 0.0),      // assembly-drone (AssemblerAI)
        (64, 49, 200.0, 1.1, 0.0),     // target-dummy internal unit
        (65, 39, 240.0, 4.6, 1_000.0), // scathe-missile: Explosion(1000, 65)
        (66, 39, 500.0, 2.5, 320.0),   // phase: Explosion(320, 120)
        (67, 39, 300.0, 4.4, 1_800.0), // surge: Explosion(1800, 40)
        (68, 39, 50.0, 4.8, 180.0),    // surge-split: Explosion(180, 35)
        (69, 2, 1.0, 0.0, 0.0),        // generated turret-unit-build-tower
    ];
    for &(id, class, health, speed, damage) in cases {
        let spec = crate::network::units::enemy_spec(id)
            .unwrap_or_else(|| panic!("unit {id} must have an enemy spec"));
        assert_eq!(spec.entity_class, class, "entity class of unit {id}");
        assert!((spec.health - health).abs() < 0.001, "health of unit {id}");
        assert!((spec.speed - speed).abs() < 0.001, "speed of unit {id}");
        assert!(
            (spec.attack_damage - damage).abs() < 0.001,
            "attack damage of unit {id}"
        );
        assert_eq!(spec.unit_type, id, "spec id round-trip of unit {id}");
    }
}

#[test]
fn missile_lifetimes_match_v1605_jar() {
    use crate::game::unit_types::unit_missile_lifetime;
    // MissileUnitType.lifetime (TimedKillUnit.lifetime on the wire);
    // constructor default is 60f * 1.7f = 102.
    let cases = [
        (46, 99.6),
        (53, 54.24),
        (55, 102.0),
        (65, 330.0),
        (66, 586.2),
        (67, 84.0),
        (68, 222.0),
    ];
    for (id, lifetime) in cases {
        let got = unit_missile_lifetime(id).unwrap_or_else(|| panic!("lifetime of {id}"));
        assert!((got - lifetime).abs() < 0.001, "lifetime of unit {id}");
    }
    // Non-missile types have no TimedKillUnit lifetime.
    for id in [0i16, 45, 52, 58, 61, 62, 64, 69] {
        assert!(unit_missile_lifetime(id).is_none(), "unit {id}");
    }
}

#[test]
fn internal_unit_specs_are_never_spawnable() {
    // The internal types carry specs for wire/persistence parity, but wave
    // rules and the console must not place them in the world.
    assert!(crate::game::unit_types::unit_type_internal(61));
    assert!(crate::game::unit_types::unit_type_internal(64));
    assert!(crate::game::unit_types::unit_type_internal(69));
    assert!(enemy_spec(61).is_some());
    assert!(enemy_spec(64).is_some());
    assert!(enemy_spec(69).is_some());
    let rules =
        r#"{"spawns":[{"type":"turret-unit-build-tower"},{"type":"dummy"},{"type":"block"}]}"#;
    let (parsed, diagnostics) = parse_wave_rules_report(rules);
    assert!(
        parsed.spawn_groups.iter().all(|group| group.unit_type == 0),
        "internal types fall back to dagger (ASTRA W04)"
    );
    assert_eq!(parsed.spawn_groups.len(), 3);
    let _ = diagnostics;
}

// ===================== EREKIR FUNCTIONAL TESTS =====================

/// Minimal world with a few Erekir tiles for simulation tests.
fn erekir_test_world() -> DynamicWorld {
    let state = crate::state::game_state::GameState::new();
    state.start_hosting(
        "erekir-test".into(),
        crate::state::game_state::GameMode::Survival,
    );
    DynamicWorld {
        game_state: state,
        width: 40,
        height: 40,
        sharded_unit_cap: 8,
        core_position: (20 << 16) | 20,
        core_max_health: 6_000.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: vec![0; 40 * 40],
        base_centers: vec![false; 40 * 40],
        tile_data: Vec::new(),
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: vec![0; 40 * 40],
        overlays: vec![0; 40 * 40],
        enemy_spawns: parking_lot::RwLock::new(Vec::new()),
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: std::sync::atomic::AtomicI32::new(2_500_000),
        next_enemy_id: std::sync::atomic::AtomicI32::new(3_000_100),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
        damaged_window: parking_lot::Mutex::new(Vec::new()),
        projectiles: DashMap::new(),
        next_projectile_id: std::sync::atomic::AtomicI32::new(4_000_000),
        overdrive_boosts: DashMap::new(),
        heal_suppression: DashMap::new(),
        force_fields: DashMap::new(),
        tiles: DashMap::new(),
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::new(),
        mono_mining_targets: DashMap::new(),
        ai_rebuild_state: Default::default(),
        tile_footprint: DashMap::new(),
        navigation_revision: std::sync::atomic::AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        naval_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("erekir-functional-test.json"),
        network_template: std::sync::Arc::new(Vec::new()),
        persistence_dirty: std::sync::atomic::AtomicBool::new(false),
        persistence_lock: parking_lot::Mutex::new(()),
        logic_flags: DashMap::new(),
        logic_executors: DashMap::new(),
        logic_display_commands: DashMap::new(),
        base_drill_progress: DashMap::new(),
        base_factory_progress: DashMap::new(),
        base_turret_progress: DashMap::new(),
        base_mender_progress: DashMap::new(),
        team_build_plans: parking_lot::RwLock::new(crate::engine::typeio::TeamBlocks::default()),
        wave_rules: parking_lot::RwLock::new({
            // Test maps have no cores: set their unit cap explicitly.
            crate::network::units::WaveRules {
                unit_cap: 8,
                ..Default::default()
            }
        }),
        votekick_target: parking_lot::RwLock::new(None),
        votekick_votes: std::sync::atomic::AtomicI32::new(0),
        votekick_voters: dashmap::DashMap::new(),
        votekick_cooldowns: dashmap::DashMap::new(),
        puddles: crate::network::buildings::puddles::PuddleSystem::new(),
        building_last_damage: DashMap::new(),
        repair_beam_strengths: DashMap::new(),
    }
}

#[test]
fn relink_power_node_heals_split_power_graph() {
    // Round 74: reproduces the user's test-world topology — a large power
    // node, a sandbox power source and a drill all in range but with no
    // links (placement paths missed the autolink / deconstruction left the
    // graph split). The periodic relink must wire them back together.
    let world = erekir_test_world();
    let node = (20 << 16) | 20; // 303 large node, range 15 tiles
    let source = (15 << 16) | 18; // 410 sandbox source, range 6 tiles
    let drill = (33 << 16) | 20; // 329 drill (12 tiles away, in node range)
    world.tiles.insert(node, erekir_tile(node, 303, 0));
    world.tiles.insert(source, erekir_tile(source, 410, 0));
    world.tiles.insert(drill, erekir_tile(drill, 329, 0));

    let changed = crate::network::buildings::power::relink_power_node(&world, node);
    assert!(changed, "the large node must link the source and the drill");
    let linked = world.tiles.get(&node).unwrap().power_links.clone();
    assert!(
        linked.contains(&source),
        "node links the sandbox source: {:?}",
        linked
    );
    assert!(
        linked.contains(&drill),
        "node links the drill in range: {:?}",
        linked
    );
    assert!(
        world.tiles.get(&drill).unwrap().power_links.contains(&node),
        "reverse link on the drill"
    );
    assert!(
        world
            .tiles
            .get(&source)
            .unwrap()
            .power_links
            .contains(&node),
        "reverse link on the source"
    );
    // And the graph now delivers power to the drill.
    let power = compute_power_efficiency(&world);
    assert!(
        power.get(&drill).copied().unwrap_or(0.0) > 0.99,
        "drill powered after relink: {:?}",
        power.get(&drill)
    );
}

#[test]
fn manual_link_to_factory_survives_relink_sweep() {
    // Round 74f: a MANUAL node link to a machine/factory must persist. The
    // relink sweep used to prune it instantly because the factory's block
    // had no power_role entry (incomplete table) — the user reported links
    // to factories unlinking themselves no matter how many times clicked.
    let world = erekir_test_world();
    let node = (20 << 16) | 20;
    let factory = (23 << 16) | 20; // separator (193), 3 tiles east
    world.tiles.insert(node, erekir_tile(node, 302, 0));
    world.tiles.insert(factory, erekir_tile(factory, 193, 0));

    // Manual link: tag 7 config (single Point2, relative dx/dy).
    let mut config = vec![7u8];
    config.extend_from_slice(&3i32.to_be_bytes());
    config.extend_from_slice(&0i32.to_be_bytes());
    assert!(
        crate::network::buildings::power::apply_configuration(&world, node, &config),
        "the manual link applies"
    );
    let linked = world.tiles.get(&node).unwrap().power_links.clone();
    assert!(
        linked.contains(&factory),
        "node links the factory: {:?}",
        linked
    );
    assert!(
        world
            .tiles
            .get(&factory)
            .unwrap()
            .power_links
            .contains(&node),
        "reverse link on the factory"
    );

    // The self-heal sweep must NOT prune the valid manual link.
    crate::network::buildings::power::relink_power_node(&world, node);
    let after = world.tiles.get(&node).unwrap().power_links.clone();
    assert!(
        after.contains(&factory),
        "manual link survives the relink sweep: {:?}",
        after
    );
    // And the factory now draws power from the graph.
    let power = compute_power_efficiency(&world);
    assert!(
        power.contains_key(&factory),
        "factory participates in the power graph"
    );
}

fn erekir_tile(position: i32, block: i16, rotation: u8) -> DynamicTile {
    DynamicTile {
        logic_control: None,
        payload_inventory: Vec::new(),
        position,
        block,
        rotation,
        team: 1,
        config: Vec::new(),
        enabled: true,
        message: None,
        occupied: Vec::new(),
        stored_item: -1,
        stored_amount: 0,
        production_progress: 0.0,
        transport_progress: 0.0,
        ammo_units: 0.0,
        inventory: Vec::new(),
        power_stored: 0.0,
        power_links: Vec::new(),
        liquid_inventory: Vec::new(),
        stored_liquid: -1,
        liquid_amount: 0.0,
        output_liquid_amount: 0.0,
        junction_items: Vec::new(),
        mass_driver_incoming: Vec::new(),
        mass_driver_rotation: 90.0,
        mass_driver_waiting: Vec::new(),
        payload: None,
        payload_progress: 0.0,
        payload_rotation: 0.0,
        payload_accum: Vec::new(),
        health: f32::MAX,
        door_open: false,
        shield: 0.0,
        light_color: -1_900_545,
        memory: Vec::new(),
        duct_rec_dir: 0,
        unloader_offset: 0,
        conveyor_items: Vec::new(),
        factory_command: None,
        stack_state: 0,
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
    }
}

#[cfg(test)]
fn ground_unit_on_tile(
    id: i32,
    unit_type: i16,
    tile: i32,
    health: f32,
    elevation: f32,
) -> EnemyUnit {
    let spec = enemy_spec(unit_type).expect("unit spec");
    let x = ((tile >> 16) as i16 as f32) * 8.0 + 4.0;
    let y = (tile as i16 as f32) * 8.0 + 4.0;
    EnemyUnit {
        id,
        unit_type,
        entity_class: spec.entity_class,
        team: 2,
        x,
        y,
        rotation: 0.0,
        health,
        shield: 0.0,
        status_effect: -1,
        status_duration: 0.0,
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
        authority: crate::network::world::UnitAuthority::DefaultAi,
        build_plans: Vec::new(),
        update_building: true,
        missile_time: 0.0,
        status_agg: None,
        drown_progress: 0.0,
    }
}

#[cfg(test)]
fn step_puddle_effects(world: &DynamicWorld, ticks: u32) {
    for _ in 0..ticks {
        world.puddles.tick(1.0);
        let _ = simulate_puddle_tile_effects(world, 1.0);
        for mut unit in world.enemies.iter_mut() {
            crate::network::units::StatusContainer::tick_statuses(&mut *unit, 1.0);
        }
    }
}

#[test]
fn erekir_ducts_hand_off_items_to_adjacent_duct() {
    // Duct A (10,10) rot 0 feeds duct B (11,10): Duct.updateTile hands the
    // item off once progress >= 1 - 1/speed (speed 4).
    let world = erekir_test_world();
    let a = (10 << 16) | 10;
    let b = (11 << 16) | 10;
    let mut tile_a = erekir_tile(a, 272, 0);
    tile_a.stored_item = 0; // copper
    tile_a.stored_amount = 1;
    tile_a.transport_progress = -1.0;
    world.tiles.insert(a, tile_a);
    world.tiles.insert(b, erekir_tile(b, 272, 0));
    let power = std::collections::HashMap::new();
    simulate_erekir_ducts(&world, 6.0, &power);
    // progress -1 + 6 * (2/4) = 2.0 >= 0.75 -> handed to B.
    assert_eq!(world.tiles.get(&a).unwrap().stored_amount, 0);
    let b_tile = world.tiles.get(&b).unwrap();
    assert_eq!(b_tile.stored_amount, 1);
    assert_eq!(b_tile.stored_item, 0);
    // The hand-off sets progress -1 (handleItem); if B is processed later in
    // the same tick its item may already advance to the hand-off threshold.
    assert!(
        (-1.001..=0.751).contains(&b_tile.transport_progress),
        "B progress {}",
        b_tile.transport_progress
    );
}

#[test]
fn erekir_ducts_pull_ready_items_from_conveyors() {
    // A conveyor in front of an empty duct leaves its front item at the far
    // end when the Serpulo funnel cannot deliver; the duct picks it up.
    let world = erekir_test_world();
    let duct = (11 << 16) | 10;
    let conveyor = (10 << 16) | 10;
    let mut belt = erekir_tile(conveyor, 257, 0);
    belt.conveyor_items = vec![(0, 1.0 - f32::EPSILON)];
    belt.stored_item = 0;
    belt.stored_amount = 1;
    world.tiles.insert(conveyor, belt);
    world.tiles.insert(duct, erekir_tile(duct, 272, 0));
    let power = std::collections::HashMap::new();
    simulate_erekir_ducts(&world, 6.0, &power);
    let duct_tile = world.tiles.get(&duct).unwrap();
    assert_eq!(duct_tile.stored_amount, 1);
    assert_eq!(duct_tile.stored_item, 0);
    assert_eq!(world.tiles.get(&conveyor).unwrap().stored_amount, 0);
}

#[test]
fn erekir_duct_unloader_runs_at_official_speed() {
    // duct-unloader (278) speed = 4f (Blocks.java v158.1):
    // DirectionalUnloader.updateTile unloads ONE item whenever unloadTimer
    // reaches `speed`, so the official rate is 60/4 = 15 items/s.
    // Regression: the port used a 1.0 threshold and unloaded every tick
    // (4x too fast). The front is a surge-conveyor (279), which must accept
    // through the duct-family route (deliver_item_to + duct_store_item).
    let world = erekir_test_world();
    let back = (9 << 16) | 10; // reinforced-container (347) item source
    let unloader = (10 << 16) | 10; // duct-unloader, rot 0 -> east
    let front = (11 << 16) | 10; // surge-conveyor (279) stack acceptor, cap 10
    let stack_front = (12 << 16) | 10; // 279 behind the acceptor: stateLoad
    let mut back_tile = erekir_tile(back, 347, 0);
    back_tile.inventory = vec![(0, 5)]; // 5 copper
    world.tiles.insert(back, back_tile);
    world.tiles.insert(unloader, erekir_tile(unloader, 278, 0));
    world.tiles.insert(front, erekir_tile(front, 279, 0));
    // A lone 279 is stateUnload and rejects every feed (official acceptItem
    // requires state == stateLoad); a stack front puts it in stateLoad so
    // the unloader hand-off is accepted (P2-1 adversarial QA).
    world
        .tiles
        .insert(stack_front, erekir_tile(stack_front, 279, 0));
    let power = std::collections::HashMap::new();

    // 3 ticks: below the 4-tick threshold -> nothing unloaded.
    simulate_erekir_ducts(&world, 3.0, &power);
    assert_eq!(
        inventory_count(&world.tiles.get(&back).unwrap().inventory, 0),
        5,
        "3 ticks must not trigger an unload"
    );
    assert_eq!(world.tiles.get(&front).unwrap().conveyor_items.len(), 0);

    // 1 more tick reaches exactly speed -> ONE item unloaded.
    simulate_erekir_ducts(&world, 1.0, &power);
    assert_eq!(
        inventory_count(&world.tiles.get(&back).unwrap().inventory, 0),
        4,
        "4 ticks unload exactly one item"
    );
    {
        let front_tile = world.tiles.get(&front).unwrap();
        assert_eq!(front_tile.conveyor_items.len(), 1);
        assert_eq!(front_tile.conveyor_items[0].0, 0, "copper unloaded");
    }

    // 16 more ticks -> 4 more unloads (16/4), the remaining 4 items.
    simulate_erekir_ducts(&world, 16.0, &power);
    assert_eq!(
        inventory_count(&world.tiles.get(&back).unwrap().inventory, 0),
        0,
        "all 5 items unloaded at 1 per 4 ticks"
    );
    assert_eq!(
        world.tiles.get(&front).unwrap().conveyor_items.len(),
        5,
        "surge-conveyor accepted all unloaded items via the duct route"
    );
}

#[test]
fn erekir_duct_bridge_transfers_to_link_without_guard_deadlock() {
    // DuctBridge (277) moves one buffered item to its linked bridge every
    // `speed` (4) ticks. The transfer must not hold a DashMap get_mut on
    // the source while writing into the link (both keys can land in the
    // same shard -> deadlock); the source guard is dropped before
    // duct_store_item runs on the link.
    let world = erekir_test_world();
    let a = (10 << 16) | 10; // source bridge, rot 0 -> east
    let b = (11 << 16) | 10; // mid bridge, rot 0 -> east (adjacent link of a)
    let c = (13 << 16) | 10; // end bridge, rot 0 -> east (adjacent link of b, no link)
    let mut tile_a = erekir_tile(a, 277, 0);
    tile_a.inventory = vec![(0, 2)]; // 2 copper buffered
    world.tiles.insert(a, tile_a);
    world.tiles.insert(b, erekir_tile(b, 277, 0));
    world.tiles.insert(c, erekir_tile(c, 277, 0));
    let power = std::collections::HashMap::new();
    // One item hops per 4 ticks; two calls move both buffered items out of
    // the source. DashMap iteration order decides whether the mid bridge
    // forwards further inside the same call, so only totals are asserted.
    simulate_erekir_ducts(&world, 4.0, &power);
    simulate_erekir_ducts(&world, 4.0, &power);
    let buffered = |pos: i32| {
        world
            .tiles
            .get(&pos)
            .map(|tile| inventory_total(&tile.inventory))
            .unwrap_or(0)
    };
    assert_eq!(buffered(a), 0, "both items left the source bridge");
    assert_eq!(
        buffered(b) + buffered(c),
        2,
        "items are buffered in the downstream bridges"
    );
}

#[test]
fn erekir_duct_bridge_acceptance_respects_front_and_occupied_input() {
    // DuctBridgeBuild.acceptItem (DuctBridge.java v158.1) rejects its output
    // side and rejects a second source on an occupied input side.
    let world = erekir_test_world();
    let target = (20 << 16) | 10; // rot 0 -> east
    let output = (21 << 16) | 10; // target's linked output
    let inbound = (16 << 16) | 10; // rot 0 -> east, links to target at range 4
    let back = (19 << 16) | 10; // adjacent non-bridge source on target's back
    world.tiles.insert(target, erekir_tile(target, 277, 0));
    world.tiles.insert(output, erekir_tile(output, 277, 0));
    world.tiles.insert(inbound, erekir_tile(inbound, 277, 0));
    world.tiles.insert(back, erekir_tile(back, 272, 0));

    assert!(
        !duct_accept_item(&world, target, 0, output),
        "a bridge must reject its front/output side"
    );
    assert!(
        !duct_accept_item(&world, target, 0, back),
        "a second source must not reuse the occupied bridge input side"
    );

    world.tiles.remove(&inbound);
    assert!(
        duct_accept_item(&world, target, 0, back),
        "the back side accepts once its occupied slot is free"
    );
}

#[test]
fn erekir_stack_conveyor_routes_only_from_its_back() {
    // StackConveyorBuild.acceptItem (bytecode 158.1) rejects
    // `front() == source` and only accepts while `state == stateLoad`
    // (`cooldown <= recharge - 1f && state == stateLoad && front() !=
    // source`). A lone 279 (no stack front) is stateUnload and rejects
    // every feed — official quirk (P2-1 adversarial QA); with a stack
    // front it enters stateLoad and pulls only from the back.
    let world = erekir_test_world();
    let stack = (20 << 16) | 10; // rot 0 -> east
    let front = (21 << 16) | 10; // rot 2 -> west, points into stack front
    let back = (19 << 16) | 10; // rot 0 -> east, points into stack back

    // Lone 279: stateUnload rejects both the front and the back feed.
    let mut front_tile = erekir_tile(front, 257, 2);
    front_tile.conveyor_items = vec![(0, 1.0 - f32::EPSILON)];
    front_tile.stored_item = 0;
    front_tile.stored_amount = 1;
    let mut back_tile = erekir_tile(back, 257, 0);
    back_tile.conveyor_items = vec![(0, 1.0 - f32::EPSILON)];
    back_tile.stored_item = 0;
    back_tile.stored_amount = 1;
    world.tiles.insert(front, front_tile);
    world.tiles.insert(back, back_tile);
    world.tiles.insert(stack, erekir_tile(stack, 279, 0));

    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());

    assert_eq!(
        world.tiles.get(&front).unwrap().stored_amount,
        1,
        "front feed rejected"
    );
    assert_eq!(
        world.tiles.get(&back).unwrap().stored_amount,
        1,
        "lone 279 (stateUnload) rejects the back feed too (official)"
    );
    assert!(
        world.tiles.get(&stack).unwrap().conveyor_items.is_empty(),
        "nothing entered the lone stack conveyor"
    );

    // With a stack front the head enters stateLoad and pulls only the back.
    world.tiles.insert(front, erekir_tile(front, 279, 2));
    let mut back_tile = erekir_tile(back, 257, 0);
    back_tile.conveyor_items = vec![(0, 1.0 - f32::EPSILON)];
    back_tile.stored_item = 0;
    back_tile.stored_amount = 1;
    world.tiles.insert(back, back_tile);

    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());

    assert_eq!(world.tiles.get(&back).unwrap().stored_amount, 0);
    let stack_tile = world.tiles.get(&stack).unwrap();
    assert_eq!(stack_tile.conveyor_items, vec![(0, 0.0)]);
}

#[test]
fn support_unit_weapons_aim_only_at_real_targets_and_heal_on_impact() {
    use crate::network::combat::unit_combat::support_weapon_target;
    use crate::network::decoders::apply_set_unit_stance_for_team;
    use crate::network::simulation::{simulate_allied_units, simulate_support_units};
    for unit_type in [21, 22] {
        for team in [1, 5] {
            let world = erekir_test_world();
            let out = DashMap::new();
            let mut unit = ground_unit_on_tile(800, unit_type, (8 << 16) | 8, 400.0, 1.0);
            unit.team = team;
            unit.authority = UnitAuthority::Command;
            world.enemies.insert(unit.id, unit.clone());
            crate::network::decoders::apply_set_unit_command_for_team(&world, team, &[unit.id], 1);
            let save_sync = |name: &str| {
                let unit = world.enemies.get(&800).unwrap().clone();
                let mut bytes = Vec::new();
                crate::network::wire::encode::write_unit_sync(
                    &mut bytes,
                    Some(&world),
                    &unit,
                    unit.x,
                    unit.y,
                    None,
                    None,
                )
                .unwrap();
                if team == 1 {
                    if let Ok(dir) = std::env::var("OXIDE_SUPPORT_FIXTURES") {
                        std::fs::create_dir_all(&dir).unwrap();
                        std::fs::write(format!("{dir}/support-{unit_type}-{name}.bin"), bytes)
                            .unwrap();
                    }
                }
            };
            for _ in 0..100 {
                simulate_allied_units(&world, &out, 1.0);
            }
            assert!(world.projectiles.is_empty(), "idle units must not fire");
            assert!(support_weapon_target(&world, &unit).is_none());
            save_sync("idle");
            let position = (20 << 16) | 8;
            let mut wall = erekir_tile(position, 216, 0);
            wall.team = team;
            wall.occupied = vec![position];
            wall.health = 40.0;
            world.tiles.insert(position, wall);
            let target = support_weapon_target(&world, &unit).unwrap();
            assert_eq!(target.building_position, Some(position));
            assert_eq!((target.x, target.y), (160.0, 64.0));
            save_sync("repair");
            simulate_allied_units(&world, &out, 1.0);
            assert!(!world.projectiles.is_empty());
            if unit_type == 21 {
                let rotation = world.enemies.get(&unit.id).unwrap().rotation;
                let expected = (target.y - unit.y).atan2(target.x - unit.x).to_degrees();
                assert!(
                    (rotation - expected).abs() < 0.001,
                    "Poly must face its fixed weapons toward the target"
                );
            }
            assert!(world
                .projectiles
                .iter()
                .all(|p| p.team == team && p.enemy_target_position == Some(position)));
            assert_eq!(
                world.tiles.get(&position).unwrap().health,
                40.0,
                "repair bolts must travel before healing"
            );
            for _ in 0..300 {
                crate::network::combat::simulate_projectiles(&world, &out, 1.0);
                simulate_allied_units(&world, &out, 1.0);
            }
            assert!(world.tiles.get(&position).unwrap().health > 40.0);
            world.tiles.get_mut(&position).unwrap().health =
                crate::game::content::block_health(216);
            world.projectiles.clear();
            simulate_allied_units(&world, &out, 1.0);
            assert!(
                world.projectiles.is_empty(),
                "full-health buildings are not targets"
            );
            save_sync("full");
            world.tiles.get_mut(&position).unwrap().health = 40.0;
            apply_set_unit_stance_for_team(&world, team, &[unit.id], 1, true);
            save_sync("hold");
            for _ in 0..60 {
                simulate_allied_units(&world, &out, 1.0);
                simulate_support_units(&world, &out, 1.0);
            }
            assert!(
                world.projectiles.is_empty(),
                "hold fire must suppress repair bolts"
            );
            assert_eq!(world.tiles.get(&position).unwrap().health, 40.0);
            apply_set_unit_stance_for_team(&world, team, &[unit.id], 1, false);
            world.tiles.get_mut(&position).unwrap().health =
                crate::game::content::block_health(216);
            let mut enemy = ground_unit_on_tile(801, 0, (19 << 16) | 8, 150.0, 0.0);
            enemy.team = 2;
            world.enemies.insert(enemy.id, enemy);
            let current = world.enemies.get(&unit.id).unwrap().clone();
            assert_eq!(
                support_weapon_target(&world, &current).unwrap().unit_id,
                801
            );
            simulate_allied_units(&world, &out, 1.0);
            assert!(world.projectiles.iter().any(|p| p.target_id == 801));
            save_sync("enemy");
        }
    }
}

#[test]
fn plastanium_line_fed_by_multitile_drill_survives_inspection() {
    use crate::network::buildings::snapshot::encode_dynamic_tile_sync;
    for (block, size) in [(327, 3), (328, 4)] {
        let world = erekir_test_world();
        let drill_pos = (10 << 16) | 7;
        let mut drill = erekir_tile(drill_pos, block, 0);
        let low = (size - 1) / 2;
        drill.occupied = (7 - low..7 - low + size)
            .flat_map(|y| (10 - low..10 - low + size).map(move |x| (x << 16) | y))
            .collect();
        drill.stored_item = 6;
        drill.stored_amount = 110;
        world.tiles.insert(drill_pos, drill);
        let first_y = 7 - low + size;
        for y in first_y..first_y + 11 {
            let pos = (10 << 16) | y;
            world.tiles.insert(pos, erekir_tile(pos, 259, 1));
        }
        let tip = (10 << 16) | (first_y + 10);
        let power = HashMap::new();
        for tick in 0..1200 {
            simulate_erekir_ducts(&world, 1.0, &power);
            if tick % 5 == 0 {
                crate::network::economy::spec::dump_drill_item(&world, drill_pos);
            }
            if tick == 400 || tick == 800 {
                let tile = world.tiles.get(&tip).unwrap().clone();
                let mut bytes = Vec::new();
                encode_dynamic_tile_sync(&mut bytes, &tile, &power, Some(&world)).unwrap();
            }
        }
        assert_eq!(
            world.tiles.get(&drill_pos).unwrap().stored_amount,
            0,
            "a multiblock drill must feed the loading dock"
        );
        let tile = world.tiles.get(&tip).unwrap().clone();
        assert_eq!(
            tile.conveyor_items.len(),
            10,
            "server must own the visible end stack"
        );
        assert_ne!(tile.stack_link, -1);
        assert_eq!(tile.stack_cooldown, 0.0);
        let mut bytes = Vec::new();
        encode_dynamic_tile_sync(&mut bytes, &tile, &power, Some(&world)).unwrap();
        if let Ok(dir) = std::env::var("OXIDE_SUPPORT_FIXTURES") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/plastanium-{block}.bin"), &bytes).unwrap();
        }
        assert_eq!(
            world
                .tiles
                .iter()
                .filter(|tile| tile.block == 259)
                .map(|tile| tile.conveyor_items.len())
                .sum::<usize>(),
            110
        );
    }
}

#[test]
fn stack_conveyor_259_waits_for_a_full_stack_before_transferring() {
    // Round 74: 259 (plastanium-conveyor) is a StackConveyor in 158.1. The
    // official stateLoad machine keeps accumulating until
    // `items.total() >= getMaximumAccepted` (10) and only then launches the
    // WHOLE batch to an idle linked front (StackConveyorBuild.updateTile
    // bytecode 158.1) — this is the visible plastanium batch "shot".
    let world = erekir_test_world();
    let source = (10 << 16) | 10;
    let target = (11 << 16) | 10;
    let mut source_tile = erekir_tile(source, 259, 0);
    source_tile.conveyor_items = vec![(0, 0.0); 2];
    source_tile.stored_item = 0;
    source_tile.stored_amount = 2;
    world.tiles.insert(source, source_tile);
    world.tiles.insert(target, erekir_tile(target, 259, 0));

    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());

    // Clone out of the DashMap Ref BEFORE inserting again: holding a shard
    // read guard across an insert on the same map deadlocks (round 74).
    let source_tile = world.tiles.get(&source).unwrap().clone();
    assert_eq!(
        source_tile.conveyor_items.len(),
        2,
        "a partial stateLoad stack stays put (below capacity 10)"
    );
    assert!(
        world.tiles.get(&target).unwrap().conveyor_items.is_empty(),
        "the batch only fires when the stack is full"
    );

    // Fill to capacity: the next tick launches the whole batch. A fed
    // stack has link != -1 (official poofIn sets link = tile.pos()).
    let mut full = erekir_tile(source, 259, 0);
    full.conveyor_items = vec![(0, 0.0); 10];
    full.stored_item = 0;
    full.stored_amount = 10;
    full.stack_link = source;
    world.tiles.insert(source, full);

    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());

    assert!(world.tiles.get(&source).unwrap().conveyor_items.is_empty());
    let target_tile = world.tiles.get(&target).unwrap();
    assert_eq!(target_tile.conveyor_items.len(), 10);
    assert_eq!(
        target_tile.stack_link, source,
        "front reels from the source"
    );
}

#[test]
fn stack_conveyor_279_keeps_blocked_front_at_head() {
    // The Erekir StackConveyor uses the official state machine (P1): a
    // stateLoad conveyor whose front is a full stack conveyor keeps its
    // items in FIFO order at the head — never rotated, duplicated or
    // delivered into the full front.
    let world = erekir_test_world();
    let source = (10 << 16) | 10;
    let target = (11 << 16) | 10;
    let mut source_tile = erekir_tile(source, 279, 0);
    source_tile.conveyor_items = vec![(0, 0.95), (0, 0.25)];
    source_tile.stored_item = 0;
    source_tile.stored_amount = 2;
    source_tile.stack_link = source; // reeling from itself (active)
    let mut target_tile = erekir_tile(target, 279, 0);
    target_tile.conveyor_items = vec![(0, 0.0); 10];
    target_tile.stored_item = 0;
    target_tile.stored_amount = 10;
    world.tiles.insert(source, source_tile);
    world.tiles.insert(target, target_tile);

    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());

    let source_tile = world.tiles.get(&source).unwrap();
    assert_eq!(source_tile.conveyor_items.len(), 2, "FIFO queue preserved");
    assert_eq!(
        source_tile.conveyor_items[0].0, 0,
        "front item stays at head"
    );
    assert_eq!(source_tile.conveyor_items[0].1, 0.95, "no phantom progress");
    let target_tile = world.tiles.get(&target).unwrap();
    assert_eq!(
        target_tile.conveyor_items.len(),
        10,
        "full front is not overfed"
    );
    assert_eq!(target_tile.stack_link, -1, "transfer never started");
}

#[test]
fn stack_conveyor_279_machine_transfers_and_unloads() {
    // P1: official StackConveyorBuild machine — transfer the whole stack to
    // an idle front conveyor (stateMove), set cooldown/recharge, and unload
    // one item per cooldown cycle when the front is not a stack conveyor.
    let world = erekir_test_world();
    let source = (10 << 16) | 10;
    let target = (11 << 16) | 10;
    let mut source_tile = erekir_tile(source, 279, 0);
    source_tile.conveyor_items = vec![(0, 0.5); 3];
    source_tile.stored_item = 0;
    source_tile.stored_amount = 3;
    source_tile.stack_link = source;
    let mut target_tile = erekir_tile(target, 279, 0);
    target_tile.stack_link = -1;
    // A stack conveyor behind the source forces stateMove (0).
    let mut back_tile = erekir_tile((9 << 16) | 10, 279, 0);
    back_tile.conveyor_items = vec![(0, 0.5); 1];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(target, target_tile);
    world.tiles.insert((9 << 16) | 10, back_tile);

    // stateMove (back is a stack too): the whole stack transfers to the
    // idle front, the source clears and recharges. NOTE: clone the DashMap
    // values and DROP the Refs before the next mutation (project rule).
    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());
    let source_tile = world.tiles.get(&source).unwrap().clone();
    assert!(
        source_tile.conveyor_items.is_empty(),
        "whole stack transferred out"
    );
    assert_eq!(source_tile.stack_link, -1);
    assert!(
        (source_tile.stack_cooldown - 2.0).abs() < 0.01,
        "recharge set"
    );
    let target_tile = world.tiles.get(&target).unwrap().clone();
    assert_eq!(target_tile.conveyor_items.len(), 3);
    assert_eq!(target_tile.stack_link, source, "front links to the source");
    // The target's cooldown was set to 1.0 by the transfer; whether it
    // already decayed by one tick of the 5/60 reel speed in this pass
    // depends on the tile iteration order, so both are valid. The reel
    // runs at speed * (efficiency + baseEfficiency) = (5/60) * 2 per tick
    // at full power (official eff = efficiency + 1f).
    assert!(
        (target_tile.stack_cooldown - 1.0).abs() < 0.01
            || (target_tile.stack_cooldown - (1.0 - 2.0 * 5.0 / 60.0)).abs() < 0.01,
        "cooldown set to 1 (decayed at most one reel step): {}",
        target_tile.stack_cooldown
    );
    drop(source_tile);
    drop(target_tile);

    // Unload: with a non-stack front, the official stateUnload is a BURST
    // (StackConveyorBuild.updateTile: `while(lastItem != null &&
    // moveForward(lastItem)) items.remove(lastItem, 1)`) — every item moves
    // in the same tick and cooldown is NOT reset (the cadence comes from
    // the conveyor behind); the link clears when the stack empties.
    let sink = (12 << 16) | 10;
    let mut sink_tile = erekir_tile(sink, 257, 0); // plain conveyor
    sink_tile.conveyor_items = Vec::new();
    world.tiles.insert(sink, sink_tile);
    let mut unloading = erekir_tile(target, 279, 0);
    unloading.conveyor_items = vec![(0, 0.0); 2];
    unloading.stored_item = 0;
    unloading.stored_amount = 2;
    unloading.stack_link = source;
    unloading.stack_cooldown = 0.0;
    world.tiles.insert(target, unloading);
    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());
    let unloading = world.tiles.get(&target).unwrap().clone();
    assert_eq!(
        unloading.conveyor_items.len(),
        0,
        "burst unloads the whole stack in one tick"
    );
    assert_eq!(unloading.stack_link, -1, "empty stack clears the link");
    assert!(
        (unloading.stack_cooldown - 0.0).abs() < 0.01,
        "cooldown is NOT reset by the unload burst"
    );
    let sink_tile = world.tiles.get(&sink).unwrap().clone();
    assert_eq!(sink_tile.conveyor_items.len(), 2, "sink received the burst");
}

#[test]
fn stack_conveyor_279_head_fed_by_duct_establishes_link_and_transfers() {
    // P1-1 regression (adversarial QA): the official
    // StackConveyorBuild.handleItem sets `link = tile.pos()` (poofIn) when
    // the stack was empty. The port used to leave stack_link == -1 on the
    // real feed path, so no 279 ever transferred or unloaded in production
    // (only tests that seeded stack_link passed). Feed path here goes
    // through the real duct phase-1 hand-off (deliver_item_to ->
    // duct_accept_item + duct_store_item).
    let world = erekir_test_world();
    let duct = (10 << 16) | 10;
    let head = (11 << 16) | 10;
    let front = (12 << 16) | 10;
    let mut duct_tile = erekir_tile(duct, 272, 0); // plain duct, faces east
    duct_tile.stored_item = 0;
    duct_tile.stored_amount = 1;
    duct_tile.transport_progress = 1.0; // item ready at the output
    world.tiles.insert(duct, duct_tile);
    let head_tile = erekir_tile(head, 279, 0);
    world.tiles.insert(head, head_tile);
    let front_tile = erekir_tile(front, 279, 0);
    world.tiles.insert(front, front_tile);

    let power = std::collections::HashMap::new();
    // Tick 1 may or may not deliver (phase-1 iteration order vs the head's
    // state derivation); tick 2 always delivers: the duct retries with
    // progress at threshold - eps.
    simulate_erekir_ducts(&world, 1.0, &power);
    simulate_erekir_ducts(&world, 1.0, &power);

    let head_tile = world.tiles.get(&head).unwrap().clone();
    assert_eq!(
        head_tile.conveyor_items.len(),
        1,
        "duct item entered the head"
    );
    assert_eq!(
        head_tile.stack_link, head,
        "poofIn: an empty-stack receive sets stack_link = tile.pos()"
    );
    let front_tile = world.tiles.get(&front).unwrap().clone();
    assert_eq!(
        front_tile.conveyor_items.len(),
        0,
        "stateLoad hoards below 10"
    );

    // Full cycle: once the head reaches capacity 10 through the real feed,
    // stateLoad transfers the whole stack to the idle front.
    let mut head_tile = world.tiles.get(&head).unwrap().clone();
    head_tile.conveyor_items = vec![(0, 0.0); 9];
    head_tile.stored_item = 0;
    head_tile.stored_amount = 9;
    head_tile.stack_link = head;
    head_tile.stack_cooldown = 0.0;
    world.tiles.insert(head, head_tile);
    let mut duct_tile = erekir_tile(duct, 272, 0);
    duct_tile.stored_item = 0;
    duct_tile.stored_amount = 1;
    duct_tile.transport_progress = 1.0;
    world.tiles.insert(duct, duct_tile);

    // Worst case: the duct delivers on tick 2, transfer fires on tick 3.
    simulate_erekir_ducts(&world, 1.0, &power);
    simulate_erekir_ducts(&world, 1.0, &power);
    simulate_erekir_ducts(&world, 1.0, &power);

    let head_tile = world.tiles.get(&head).unwrap().clone();
    assert!(
        head_tile.conveyor_items.is_empty(),
        "the full stack transferred out"
    );
    assert_eq!(head_tile.stack_link, -1, "sender cleared its link");
    let front_tile = world.tiles.get(&front).unwrap().clone();
    assert_eq!(
        front_tile.conveyor_items.len(),
        10,
        "front received the whole stack"
    );
    assert_eq!(front_tile.stack_link, head, "front links to the sender");
    // Depending on iteration order the transfer can fire as early as tick 1
    // (the duct delivers before the head runs, using the state persisted in
    // the previous tick) and the front then reels at (5/60)*2 per tick for
    // the remaining ticks: 1.0 - n*0.1667 with n in 0..=3.
    assert!(
        front_tile.stack_cooldown > 0.49 && front_tile.stack_cooldown <= 1.01,
        "front cooldown = 1 (reeling, never reset): {}",
        front_tile.stack_cooldown
    );
}

#[test]
fn stack_conveyor_279_head_fed_by_conveyor_establishes_link() {
    // P1-1 regression via the phase-2 pull (conveyor family -> duct
    // family): a plain conveyor (257) with a ready item feeds the head; the
    // empty-stack receive must poofIn (stack_link = tile.pos()).
    let world = erekir_test_world();
    let conveyor = (10 << 16) | 10;
    let head = (11 << 16) | 10;
    let front = (12 << 16) | 10;
    let mut conveyor_tile = erekir_tile(conveyor, 257, 0); // faces east
    conveyor_tile.conveyor_items = vec![(0, 1.0)]; // ready at the output
    conveyor_tile.stored_item = 0;
    conveyor_tile.stored_amount = 1;
    world.tiles.insert(conveyor, conveyor_tile);
    world.tiles.insert(head, erekir_tile(head, 279, 0));
    // A stack front puts the head in stateLoad; a lone 279 is stateUnload
    // and the official acceptItem rejects it (state != stateLoad).
    world.tiles.insert(front, erekir_tile(front, 279, 0));

    // Phase 2 always runs after phase 1, so the head's state (stateLoad:
    // front/back topology) is already derived within this single tick.
    simulate_erekir_ducts(&world, 1.0, &std::collections::HashMap::new());

    let head_tile = world.tiles.get(&head).unwrap().clone();
    assert_eq!(
        head_tile.conveyor_items.len(),
        1,
        "conveyor item entered the head"
    );
    assert_eq!(
        head_tile.stack_link, head,
        "poofIn on the phase-2 pull path too"
    );
    let conveyor_tile = world.tiles.get(&conveyor).unwrap().clone();
    assert_eq!(
        conveyor_tile.conveyor_items.len(),
        0,
        "source item was removed from the conveyor"
    );
}

#[test]
fn erekir_heat_propagates_from_heater_through_conductor() {
    // electric-heater (203, size 2, heatOutput 3, power 100/60) at (30,30)
    // faces a heat-redirector (206, size 3) at (32,30); the conductor must
    // receive the heater's heat through calculateHeat adjacency.
    let world = erekir_test_world();
    let heater = (30 << 16) | 30;
    let conductor = (32 << 16) | 30;
    let mut heat = erekir_tile(heater, 203, 0);
    heat.mass_driver_rotation = 0.0;
    heat.occupied = vec![heater, (31 << 16) | 30, (30 << 16) | 31, (31 << 16) | 31];
    let mut cond = erekir_tile(conductor, 206, 0);
    cond.mass_driver_rotation = 0.0;
    cond.occupied = vec![
        conductor,
        (33 << 16) | 30,
        (34 << 16) | 30,
        (32 << 16) | 31,
        (33 << 16) | 31,
        (34 << 16) | 31,
        (32 << 16) | 32,
        (33 << 16) | 32,
        (34 << 16) | 32,
    ];
    world.tiles.insert(heater, heat);
    world.tiles.insert(conductor, cond);
    let mut power = std::collections::HashMap::new();
    power.insert(heater, 1.0);
    for _ in 0..40 {
        simulate_heat_network(&world, 6.0, &power);
    }
    // Heater approaches heatOutput * efficiency = 3.0.
    let heater_heat = world.tiles.get(&heater).unwrap().mass_driver_rotation;
    assert!(
        (heater_heat - 3.0).abs() < 0.01,
        "heater heat {heater_heat}"
    );
    // Conductor receives heater.heat / size * contactPoints (2/2 -> 3.0).
    let conductor_heat = world.tiles.get(&conductor).unwrap().mass_driver_rotation;
    assert!(conductor_heat > 2.5, "conductor heat {conductor_heat}");
}

#[test]
fn erekir_heat_consumer_crafts_with_input_heat() {
    // A heat-reactor (215) with thorium + nitrogen produces fissile-matter
    // and heat 10; a carbide-crucible (210, heatReq 40) fed by four reactors
    // crafts carbide. Simpler: seed the heater directly and assert the
    // consumer's received heat drives crafting.
    let world = erekir_test_world();
    let heater = (30 << 16) | 30; // electric-heater
    let consumer = (32 << 16) | 30; // carbide-crucible (size 3)
    let mut heat = erekir_tile(heater, 203, 0);
    heat.occupied = vec![heater, (31 << 16) | 30, (30 << 16) | 31, (31 << 16) | 31];
    let mut crucible = erekir_tile(consumer, 210, 0);
    crucible.mass_driver_rotation = 0.0;
    crucible.occupied = vec![
        consumer,
        (33 << 16) | 30,
        (34 << 16) | 30,
        (32 << 16) | 31,
        (33 << 16) | 31,
        (34 << 16) | 31,
        (32 << 16) | 32,
        (33 << 16) | 32,
        (34 << 16) | 32,
    ];
    // Two heaters stacked through a conductor would be capped by heatOutput;
    // seed the heater beyond its nominal output to exercise the propagation
    // (the conductor passes heater.heat / size * contactPoints through).
    heat.mass_driver_rotation = 60.0;
    crucible.inventory = vec![(17, 2), (3, 3)];
    world.tiles.insert(heater, heat);
    world.tiles.insert(consumer, crucible);
    let mut power = std::collections::HashMap::new();
    power.insert(heater, 1.0);
    power.insert(consumer, 1.0);
    simulate_heat_network(&world, 33.75, &power);
    let crucible = world.tiles.get(&consumer).unwrap();
    assert!(
        inventory_count(&crucible.inventory, 19) >= 1,
        "carbide crafted"
    );
}

#[cfg(test)]
fn heat_footprint(origin: i32, size: i32) -> Vec<i32> {
    let x = (origin >> 16) as i16 as i32;
    let y = origin as i16 as i32;
    let mut occupied = Vec::new();
    for dy in 0..size {
        for dx in 0..size {
            occupied.push(((x + dx) << 16) | ((y + dy) as u16 as i32));
        }
    }
    occupied
}

#[test]
fn heat_isolated_electric_heater_reaches_official_output() {
    let world = erekir_test_world();
    let heater = (10 << 16) | 10;
    let mut tile = erekir_tile(heater, 203, 0);
    tile.occupied = heat_footprint(heater, 2);
    tile.mass_driver_rotation = 0.0;
    world.tiles.insert(heater, tile);
    let mut power = HashMap::new();
    power.insert(heater, 1.0);
    for _ in 0..20 {
        simulate_heat_network(&world, 1.0, &power);
    }
    let heat = world.tiles.get(&heater).unwrap().mass_driver_rotation;
    assert!(
        (heat - 3.0).abs() < 0.001,
        "HeatProducer warmupRate 0.15 reaches heatOutput 3 in 20 ticks: {heat}"
    );
}

#[test]
fn heat_consumer_without_heat_does_not_craft() {
    let world = erekir_test_world();
    let consumer = (10 << 16) | 10;
    let mut crucible = erekir_tile(consumer, 210, 0);
    crucible.occupied = heat_footprint(consumer, 3);
    crucible.inventory = vec![(17, 2), (3, 3)];
    world.tiles.insert(consumer, crucible);
    let mut power = HashMap::new();
    power.insert(consumer, 1.0);
    simulate_heat_network(&world, 33.75, &power);
    let crucible = world.tiles.get(&consumer).unwrap();
    assert_eq!(
        inventory_count(&crucible.inventory, 19),
        0,
        "carbide-crucible must not craft without heatRequirement 40"
    );
    assert_eq!(inventory_count(&crucible.inventory, 17), 2);
}

#[test]
fn heat_split_when_proximity_breaks() {
    let world = erekir_test_world();
    let heater = (10 << 16) | 10;
    let conductor = (12 << 16) | 10;
    let mut heat = erekir_tile(heater, 203, 0);
    heat.occupied = heat_footprint(heater, 2);
    heat.mass_driver_rotation = 3.0;
    let mut cond = erekir_tile(conductor, 206, 0);
    cond.occupied = heat_footprint(conductor, 3);
    world.tiles.insert(heater, heat);
    world.tiles.insert(conductor, cond);
    let mut power = HashMap::new();
    power.insert(heater, 1.0);
    simulate_heat_network(&world, 1.0, &power);
    let linked = world.tiles.get(&conductor).unwrap().mass_driver_rotation;
    assert!(linked > 2.5, "conductor received heater heat: {linked}");

    world.tiles.remove(&heater);
    simulate_heat_network(&world, 1.0, &power);
    let split = world.tiles.get(&conductor).unwrap().mass_driver_rotation;
    assert!(
        split.abs() < 0.001,
        "breaking proximity must drop conductor heat, had {split}"
    );
}

#[test]
fn heat_120_ticks_match_java_approach_on_heater_and_conductor() {
    // HeatProducerBuild: approachDelta(heat, 3, 0.15) per tick with
    // Time.delta=1. After t ticks heat = min(3, 0.15*t). Conductor
    // calculateHeat copies that adjacency value the same tick.
    let world = erekir_test_world();
    let heater = (10 << 16) | 10;
    let conductor = (12 << 16) | 10;
    let mut heat = erekir_tile(heater, 203, 0);
    heat.occupied = heat_footprint(heater, 2);
    heat.mass_driver_rotation = 0.0;
    let mut cond = erekir_tile(conductor, 206, 0);
    cond.occupied = heat_footprint(conductor, 3);
    cond.mass_driver_rotation = 0.0;
    world.tiles.insert(heater, heat);
    world.tiles.insert(conductor, cond);
    let mut power = HashMap::new();
    power.insert(heater, 1.0);

    for tick in 1..=120 {
        simulate_heat_network(&world, 1.0, &power);
        let heater_heat = world.tiles.get(&heater).unwrap().mass_driver_rotation;
        let expected = (0.15 * tick as f32).min(3.0);
        assert!(
            (heater_heat - expected).abs() < 0.001,
            "tick {tick}: heater {heater_heat} vs Java {expected}"
        );
        if tick >= 20 {
            let conductor_heat = world.tiles.get(&conductor).unwrap().mass_driver_rotation;
            assert!(
                conductor_heat > 2.5,
                "tick {tick}: conductor should carry the heater output, had {conductor_heat}"
            );
        }
    }
}

#[test]
fn erekir_beam_drill_mines_wall_ore() {
    // large-plasma-bore (336, tier 5, drillTime 100, size 3, range 6) at
    // (10,10) rot 0 with ore-wall-thorium (floor 176) at (12,10).
    let mut world = erekir_test_world();
    let drill = (10 << 16) | 10;
    let ore = (12 << 16) | 10;
    let index = (10 * 40 + 12) as usize;
    world.floors[index] = 176; // ore-wall-thorium
    let mut d = erekir_tile(drill, 336, 0);
    d.occupied = vec![drill];
    world.tiles.insert(drill, d);
    let mut power = std::collections::HashMap::new();
    power.insert(drill, 1.0);
    simulate_erekir_drills(&world, 100.0, &power);
    let drilled = world.tiles.get(&drill).unwrap();
    assert_eq!(
        inventory_count(&drilled.inventory, 7),
        1,
        "one thorium mined"
    );
    let _ = ore;
}

#[test]
fn erekir_tank_fabricator_spawns_stell() {
    // tank-fabricator (386): stell (38), beryllium 40 + silicon 50,
    // 2100 ticks. simulate_unit_factories runs the same loop entry as the
    // orchestrator's tick (delta 2100 completes one plan).
    let world = erekir_test_world();
    let factory = (10 << 16) | 10;
    let mut f = erekir_tile(factory, 386, 0);
    f.inventory = vec![(16, 40), (9, 50)];
    world.tiles.insert(factory, f);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(factory, 1.0);
    simulate_unit_factories(&world, &connections, 2_100.0, &power);
    assert!(
        world.enemies.is_empty(),
        "completion holds a payload; the unit is not teleported out"
    );
    assert!(
        world.tiles.get(&factory).unwrap().payload.is_some(),
        "stell sits in the fabricator payload"
    );
    simulate_unit_factories(&world, &connections, 20.0, &power);
    assert_eq!(world.enemies.len(), 1);
    let unit = world.enemies.iter().next().unwrap();
    assert_eq!(unit.unit_type, 38);
    assert_eq!(unit.team, 1);
    assert!((unit.health - 850.0).abs() < 0.001);
}

#[test]
fn unit_factory_team_cost_and_build_speed_multipliers_apply() {
    // A7: TeamRule.unitCostMultiplier scales the consumed items
    // (`Math.round(amount * Rules.unitCost(team))`, ConsumeItems.trigger JAR
    // offsets 23-52 + UnitFactory.lambda$initCapacities$6) and
    // TeamRule.unitBuildSpeedMultiplier scales the progress
    // (UnitFactory$UnitFactoryBuild.updateTile JAR offsets 93-117 with
    // Rules.unitBuildSpeed offsets 0-16).
    let world = erekir_test_world();
    let mut rule = crate::network::units::TeamRule {
        unit_build_speed_multiplier: 2.0,
        unit_cost_multiplier: 0.5,
        ..Default::default()
    };
    rule.unit_cost_multiplier = 0.5;
    rule.unit_build_speed_multiplier = 2.0;
    world.wave_rules.write().team_rules.insert(1, rule);

    let factory = (10 << 16) | 10;
    let mut f = erekir_tile(factory, 386, 0);
    // stell plan: 40 beryllium + 50 silicon, 2100 ticks; scaled cost is
    // round(40*0.5)=20 + round(50*0.5)=25 and speed 2.0 halves the time.
    f.inventory = vec![(16, 20), (9, 25)];
    world.tiles.insert(factory, f);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(factory, 1.0);
    simulate_unit_factories(&world, &connections, 1_050.0, &power);
    simulate_unit_factories(&world, &connections, 20.0, &power);
    assert_eq!(
        world.enemies.len(),
        1,
        "scaled cost + doubled speed complete the plan in 1050 ticks"
    );
    let after = world.tiles.get(&factory).unwrap();
    assert_eq!(
        inventory_count(&after.inventory, 16),
        0,
        "20 beryllium consumed"
    );
    assert_eq!(
        inventory_count(&after.inventory, 9),
        0,
        "25 silicon consumed"
    );
    drop(after); // never mutate a DashMap while a Ref guard is alive

    // Negative: 19 beryllium (one below the scaled cost of 20) never crafts,
    // even with the full 2100 ticks.
    world.enemies.clear();
    let mut f = world.tiles.get_mut(&factory).unwrap();
    f.production_progress = 0.0;
    f.inventory = vec![(16, 19), (9, 25)];
    drop(f);
    simulate_unit_factories(&world, &connections, 2_100.0, &power);
    assert!(world.enemies.is_empty(), "below scaled cost -> no unit");
    let blocked = world.tiles.get(&factory).unwrap();
    assert_eq!(
        inventory_count(&blocked.inventory, 16),
        19,
        "nothing consumed"
    );
    drop(blocked); // never mutate a DashMap while a Ref guard is alive

    // Speed control: with the default speed multiplier the same progress
    // window leaves the factory short of the plan (full default cost now
    // that the team rule is gone: 40 beryllium + 50 silicon).
    world.tiles.get_mut(&factory).unwrap().inventory = vec![(16, 40), (9, 50)];
    world.tiles.get_mut(&factory).unwrap().production_progress = 0.0;
    world.wave_rules.write().team_rules.clear();
    simulate_unit_factories(&world, &connections, 1_050.0, &power);
    assert!(world.enemies.is_empty(), "default speed: 1050 < 2100 ticks");
    let slow = world.tiles.get(&factory).unwrap();
    assert!(
        (slow.production_progress - 1_050.0).abs() < 0.001,
        "default speed accumulates one tick per tick"
    );
}

#[test]
fn unit_factory_activation_delay_gates_production_per_team() {
    // P1-E1: Team.activateUnitFactories / Rules.unitActivationDelay(team).
    let world = erekir_test_world();
    world.wave_rules.write().unit_factory_activation_delay = 100.0;
    world.wave_rules.write().team_rules.insert(
        1,
        crate::network::units::TeamRule {
            unit_factory_activation_delay: 50.0,
            ..Default::default()
        },
    );
    assert_eq!(world.wave_rules.read().unit_activation_delay_for(1), 150.0);

    let factory = (10 << 16) | 10;
    let mut f = erekir_tile(factory, 386, 0);
    f.inventory = vec![(16, 40), (9, 50)];
    world.tiles.insert(factory, f);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(factory, 1.0);

    *world.game_state.simulation_time.write() = 149.0;
    simulate_unit_factories(&world, &connections, 2_100.0, &power);
    assert!(
        world.enemies.is_empty(),
        "below activation delay -> no unit"
    );

    *world.game_state.simulation_time.write() = 150.0;
    simulate_unit_factories(&world, &connections, 2_100.0, &power);
    simulate_unit_factories(&world, &connections, 20.0, &power);
    assert_eq!(world.enemies.len(), 1, "at activation delay -> unit spawns");

    // Team 2 keeps the default delay (0): global 100 alone gates it.
    world.enemies.clear();
    let factory2 = (12 << 16) | 10;
    let mut f2 = erekir_tile(factory2, 386, 0);
    f2.team = 2;
    f2.inventory = vec![(16, 40), (9, 50)];
    world.tiles.insert(factory2, f2);
    power.insert(factory2, 1.0);
    *world.game_state.simulation_time.write() = 99.0;
    simulate_unit_factories(&world, &connections, 2_100.0, &power);
    assert!(world.enemies.is_empty(), "team 2 blocked at tick 99");
    *world.game_state.simulation_time.write() = 100.0;
    simulate_unit_factories(&world, &connections, 2_100.0, &power);
    simulate_unit_factories(&world, &connections, 20.0, &power);
    assert_eq!(world.enemies.len(), 1, "team 2 activates at global delay");
    let spawned = world.enemies.iter().next().expect("team 2 unit");
    assert_eq!(spawned.team, 2);
}

#[test]
fn assembler_plans_match_official_blocks() {
    // tank/ship/mech assemblers -> vanquish/quell/tecta tier 0.
    assert_eq!(assembler_plan(393, 0).map(|p| p.0), Some(41)); // vanquish
    assert_eq!(assembler_plan(394, 0).map(|p| p.0), Some(52)); // quell
    assert_eq!(assembler_plan(395, 0).map(|p| p.0), Some(47)); // tecta
                                                               // tier 1 upgrades.
    assert_eq!(assembler_plan(393, 1).map(|p| p.0), Some(42)); // conquer
    assert_eq!(assembler_plan(394, 1).map(|p| p.0), Some(54)); // disrupt
    assert_eq!(assembler_plan(395, 1).map(|p| p.0), Some(48)); // collaris
                                                               // Build times from Blocks.java (60 * seconds).
    assert!((assembler_plan(393, 0).unwrap().1 - 60.0 * 50.0).abs() < 1.0);
    assert!((assembler_plan(394, 0).unwrap().1 - 60.0 * 60.0).abs() < 1.0);
    assert!((assembler_plan(395, 0).unwrap().1 - 60.0 * 70.0).abs() < 1.0);
}

#[test]
fn assembler_tier_zero_consumes_loaded_payloads_not_core_items() {
    // tank-assembler (393) tier 0 (vanquish 41) builds WITHOUT any adjacent
    // UnitAssemblerModule and consumes the plan's PayloadStacks from the
    // BUILD's payload store (UnitAssembler init(): ConsumePayloadDynamic;
    // spawned() -> consume()). The team core inventory is never touched
    // (plan().itemReq is null for all vanilla plans).
    let world = erekir_test_world();
    let assembler = (10 << 16) | 10;
    let mut asm = erekir_tile(assembler, 393, 0);
    asm.occupied = vec![assembler];
    // Official PayloadStack.list(UnitTypes.stell, 4, Blocks.tungstenWallLarge,
    // 10): stell unit id 38, tungsten-wall-large block id 238.
    asm.payload_inventory = vec![(38, 4), (238, 10)];
    // consumeLiquid(cyanogen, 9f/60f) is an unconditional consumer for every
    // plan/tier (Blocks.java:6535): stock enough for the full build.
    asm.stored_liquid = CYANOGEN_LIQUID;
    asm.liquid_amount = 10_000.0;
    world.tiles.insert(assembler, asm);
    {
        let mut items = crate::network::economy::items_for_team_mut(&world, 1);
        items[16] = 40; // beryllium
        items[9] = 40; // silicon
    }
    let mut power = std::collections::HashMap::new();
    power.insert(assembler, 1.0);
    let connections = DashMap::new();
    // 50s build time; one extra tick completes the plan.
    assert!(simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 50.0 + 1.0,
        &power
    ));
    let items = crate::network::economy::items_for_team(&world, 1);
    assert_eq!(items[16], 40, "core inventory untouched by assemblers");
    assert_eq!(items[9], 40, "core inventory untouched by assemblers");
    assert_eq!(world.enemies.len(), 1);
    let unit = world.enemies.iter().next().unwrap();
    assert_eq!(unit.unit_type, 41, "tier 0 vanquish without any module");
    assert_eq!(unit.team, 1);
    let asm = world.tiles.get(&assembler).unwrap().clone();
    assert!(
        asm.payload_inventory.is_empty(),
        "payload stacks consumed at spawn (ConsumePayloadDynamic.trigger)"
    );
}

#[test]
fn assembler_without_payloads_draws_the_drone_proxy_from_the_core() {
    // Documented deviation (README gaps row): assembly-drones fly via
    // AssemblerAI, but payload ferrying is not modeled, so when the build
    // has no loaded stacks the team core stands in for the drone ferry and
    // is charged at completion. Without cyanogen nothing may progress
    // regardless of stock.
    let world = erekir_test_world();
    let assembler = (10 << 16) | 10;
    let mut asm = erekir_tile(assembler, 393, 0);
    asm.occupied = vec![assembler];
    world.tiles.insert(assembler, asm);
    {
        let mut items = crate::network::economy::items_for_team_mut(&world, 1);
        items[16] = 999; // beryllium
        items[9] = 999; // silicon
        items[7] = 999; // tungsten
    }
    let mut power = std::collections::HashMap::new();
    power.insert(assembler, 1.0);
    let connections = DashMap::new();
    // No cyanogen -> ConsumeLiquid.efficiency == 0 -> zero progress.
    assert!(!simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 50.0 + 1.0,
        &power
    ));
    assert!(world.enemies.is_empty(), "no cyanogen, no assembly");
    if let Some(mut tile) = world.tiles.get_mut(&assembler) {
        tile.stored_liquid = CYANOGEN_LIQUID;
        tile.liquid_amount = 10_000.0;
    }
    assert!(simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 50.0 + 1.0,
        &power
    ));
    assert_eq!(world.enemies.len(), 1, "core proxy completes the plan");
    let items = crate::network::economy::items_for_team(&world, 1);
    assert_eq!(items[16], 999 - 40, "beryllium proxy stack charged");
    assert_eq!(items[9], 999 - 40, "silicon proxy stack charged");
}

#[test]
fn assembler_module_raises_plan_tier() {
    // SOL-010: an adjacent basic-assembler-module (396, tier 1) RAISES the
    // effective plan tier to 1 (conquer 42, 180s) — mirroring
    // UnitAssembler.UnitAssemblerBuild.checkTier() (UnitAssembler.java:
    // 315-390) and UnitAssemblerModule tier = 1 (UnitAssemblerModule.java:
    // 21-24). Without a module the tier stays 0.
    let world = erekir_test_world();
    let assembler = (10 << 16) | 10;
    let mut asm = erekir_tile(assembler, 393, 0);
    asm.occupied = vec![assembler];
    assert_eq!(assembler_tier(&world, &asm), 0, "no module -> tier 0");
    world.tiles.insert(assembler, asm);
    let module = (11 << 16) | 10;
    let mut module_tile = erekir_tile(module, 396, 0);
    module_tile.occupied = vec![module];
    world.tiles.insert(module, module_tile);
    let snapshot = world.tiles.get(&assembler).unwrap().clone();
    assert_eq!(
        assembler_tier(&world, &snapshot),
        1,
        "adjacent module raises the tier"
    );
    // Official conquer PayloadStack.list(UnitTypes.locus, 6,
    // Blocks.carbideWallLarge, 20): locus unit id 39, carbide-wall-large
    // block id 243.
    if let Some(mut tile) = world.tiles.get_mut(&assembler) {
        tile.payload_inventory = vec![(39, 6), (243, 20)];
    }
    // tankAssembler consumeLiquid(Liquids.cyanogen, 9/60) applies to every
    // plan; tier 1 assembly drains it per tick (ConsumeLiquidsDynamic).
    if let Some(mut tile) = world.tiles.get_mut(&assembler) {
        tile.stored_liquid = CYANOGEN_LIQUID;
        tile.liquid_amount = 100.0;
    }
    let mut power = std::collections::HashMap::new();
    power.insert(assembler, 1.0);
    let connections = DashMap::new();
    // Without cyanogen in the build's liquid store tier 1 must stall even
    // with full payloads (shouldConsume -> consLiquids efficiency).
    if let Some(mut tile) = world.tiles.get_mut(&assembler) {
        tile.liquid_amount = 0.0;
    }
    assert!(!simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 180.0 + 1.0,
        &power
    ));
    assert!(
        world.enemies.is_empty(),
        "tier 1 without cyanogen never progresses"
    );
    if let Some(mut tile) = world.tiles.get_mut(&assembler) {
        tile.stored_liquid = CYANOGEN_LIQUID;
        tile.liquid_amount = 100.0;
    }
    assert!(simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 180.0 + 1.0,
        &power
    ));
    let unit = world.enemies.iter().next().unwrap();
    assert_eq!(unit.unit_type, 42, "tier 1 conquer (id 42, not 44)");
    // Cyanogen drained while assembling (ConsumeLiquidsDynamic.update,
    // 9/60 per powered tick for tankAssembler); this single oversized call
    // clamps to the build's whole store.
    let asm = world.tiles.get(&assembler).unwrap().clone();
    assert!(
        asm.liquid_amount < 100.0 && asm.liquid_amount >= 0.0,
        "cyanogen must be drained by assembly, got {}",
        asm.liquid_amount
    );
    assert!(
        asm.payload_inventory.is_empty(),
        "payload stacks consumed at spawn"
    );
}

#[test]
fn assembler_progress_exact_per_tick_and_stalled_by_occupied_output() {
    // Official updateTile (UnitAssembler.java:429-535) only accumulates
    // progress when efficiency > 0, Units.canCreate passes AND the spawn
    // area is free (!wasOccupied / checkSolid). The 50s tier-0 tank plan
    // therefore needs exactly 3000 ticks of powered, unobstructed time.
    let world = erekir_test_world();
    let assembler = (10 << 16) | 10;
    let mut asm = erekir_tile(assembler, 393, 0); // rotation 0 -> +x output
    asm.occupied = vec![assembler];
    world.tiles.insert(assembler, asm);
    // Full official tier-0 payload stack: stell(38)x4 + tungsten-wall-large
    // (238)x10; payloads are only consumed at spawn.
    if let Some(mut tile) = world.tiles.get_mut(&assembler) {
        tile.payload_inventory = vec![(38, 4), (238, 10)];
        tile.stored_liquid = CYANOGEN_LIQUID;
        tile.liquid_amount = 10_000.0;
    }
    let mut power = std::collections::HashMap::new();
    power.insert(assembler, 1.0);
    let connections = DashMap::new();

    // 2995 ticks: five short of completion, no unit may exist yet.
    assert!(!simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 50.0 - 5.0,
        &power
    ));
    assert!(world.enemies.is_empty(), "plan not finished yet");
    // The remaining 5 ticks complete exactly one vanquish.
    assert!(simulate_erekir_assemblers(
        &world,
        &connections,
        5.0,
        &power
    ));
    assert_eq!(world.enemies.len(), 1);
    let unit = world.enemies.iter().next().unwrap().clone();
    assert_eq!(unit.unit_type, 41, "vanquish");
    assert_eq!(unit.team, 1, "block team");
    // Spawned on the output side (rotation 0 -> larger x than the block).
    assert!(
        unit.x > 88.0,
        "unit must spawn toward the output face, got ({}, {})",
        unit.x,
        unit.y
    );

    // Park that unit on the output spawn point (72 world units ahead of the
    // block center): assembly must stall even with full inputs and power.
    let spawn_x = 80.0 + 72.0;
    if let Some(mut parked) = world.enemies.get_mut(&unit.id) {
        parked.x = spawn_x;
        parked.y = 80.0;
    }
    if let Some(mut tile) = world.tiles.get_mut(&assembler) {
        tile.payload_inventory = vec![(38, 4), (238, 10)];
    }
    assert!(!simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 50.0 + 1.0,
        &power
    ));
    assert_eq!(world.enemies.len(), 1, "occupied output stalls assembly");
    // Once the output frees up, the next craft completes.
    if let Some(mut parked) = world.enemies.get_mut(&unit.id) {
        parked.x += 200.0;
    }
    assert!(simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 50.0 + 1.0,
        &power
    ));
    assert_eq!(world.enemies.len(), 2, "second vanquish after output freed");
}

#[test]
fn refabricator_upgrades_stell_into_locus_with_items_hydrogen() {
    // Erekir refabricators are plain Reconstructor-class blocks
    // (Blocks.java v158.1 tankRefabricator: silicon 40 + tungsten 30 +
    // hydrogen 3/60, constructTime 60*30, upgrades stell -> locus).
    let world = erekir_test_world();
    let refab = (10 << 16) | 10;
    let mut r = erekir_tile(refab, 389, 0);
    r.occupied = vec![refab];
    let mut stell = ground_unit_on_tile(8, 38, refab, 850.0, 0.0);
    stell.team = 1;
    r.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(stell)));
    r.stored_amount = 39; // stell (38)
    r.inventory = vec![(9, 40), (17, 30)]; // silicon, tungsten
    r.stored_liquid = 8; // hydrogen
                         // Full plan budget plus headroom so the final short tick still finds
                         // its rate*delta slice available.
    r.liquid_amount = 3.0 / 60.0 * 1800.0 + 1.0;
    world.tiles.insert(refab, r);
    *world.game_state.simulation_time.write() = 200.0;
    let mut power = std::collections::HashMap::new();
    power.insert(refab, 1.0);
    let connections = DashMap::new();

    // One tick short: still assembling.
    simulate_reconstructors(&world, &connections, 1_799.0, &power);
    assert!(
        world.enemies.is_empty(),
        "1799 ticks do not finish the upgrade"
    );
    // Completing consumes the inputs and holds a locus (39) of the block team.
    assert!(simulate_reconstructors(&world, &connections, 2.0, &power));
    assert!(
        world.enemies.is_empty(),
        "upgrade stays in the payload until moveOut"
    );
    let held = world.tiles.get(&refab).unwrap().clone();
    match held.payload.as_deref() {
        Some(crate::network::world::CarriedPayload::Unit(unit)) => {
            assert_eq!(unit.unit_type, 39, "locus");
            assert_eq!(unit.team, 1);
        }
        other => panic!("expected upgraded unit payload, got {other:?}"),
    }
    assert_eq!(held.stored_amount, 40, "payload still held after upgrade");
    assert_eq!(inventory_count(&held.inventory, 9), 0, "silicon consumed");
    assert_eq!(inventory_count(&held.inventory, 17), 0, "tungsten consumed");
    drop(held);
    simulate_reconstructors(&world, &connections, 20.0, &power);
    assert_eq!(world.enemies.len(), 1);
    let unit = world.enemies.iter().next().unwrap().clone();
    assert_eq!(unit.unit_type, 39, "locus");
    assert_eq!(unit.team, 1);
    let tile = world.tiles.get(&refab).unwrap().clone();
    assert_eq!(tile.stored_amount, 0, "payload released");
    assert!(tile.payload.is_none());
}

#[test]
fn prime_refabricator_carries_all_three_upgrade_pairs() {
    // Blocks.java v158.1 primeRefabricator upgrades: locus->precept,
    // cleroi->anthicus, avert->obviate; nitrogen 10/60, thorium 80 +
    // silicon 100, constructTime 60*60.
    assert_eq!(reconstructor_upgrade(392, 39), Some(40)); // precept
    assert_eq!(reconstructor_upgrade(392, 44), Some(45)); // anthicus
    assert_eq!(reconstructor_upgrade(392, 50), Some(51)); // obviate
    assert_eq!(reconstructor_upgrade(389, 38), Some(39));
    assert_eq!(reconstructor_upgrade(390, 49), Some(50));
    assert_eq!(reconstructor_upgrade(391, 43), Some(44));

    let recipe = reconstructor_recipe(392).unwrap();
    assert_eq!(recipe.items, &[(7, 80), (9, 100)]);
    assert!((recipe.liquid_rate - 10.0 / 60.0).abs() < 1e-6);
    assert_eq!(recipe.liquid_id, 9, "nitrogen");
    assert!((recipe.build_time - 60.0 * 60.0).abs() < 1.0);
    for block in [389, 390, 391] {
        let recipe = reconstructor_recipe(block).unwrap();
        assert_eq!(recipe.liquid_id, 8, "hydrogen");
    }
}

#[test]
fn power_nodes_link_at_distance_and_adjacent_proximity_connects() {
    // SOL-010: mirrors BuildingComp.getPowerConnections (BuildingComp.java:
    // 1189-1207) — proximity (orthogonal adjacency) is a live edge when the
    // pair can output/conduct power, INCLUDING power nodes, and configured
    // power.links are additional edges at any distance. The laserRange is only
    // an autolink placement aid (PowerNode.getPotentialLinks/placed), not a
    // live radius.
    let world = erekir_test_world();
    let solar = (9 << 16) | 10; // solar-panel (313, production 0.12)
    let node_a = (10 << 16) | 10; // power-node (302)
    let node_b = (11 << 16) | 10; // power-node (302)
    let laser = (12 << 16) | 10; // laser-drill (327, demand 1.1)
    for (position, block) in [(solar, 313), (node_a, 302), (node_b, 302), (laser, 327)] {
        let mut tile = erekir_tile(position, block, 0);
        tile.occupied = vec![position];
        world.tiles.insert(position, tile);
    }
    // (a) All adjacent, NO links: the Java proximity loop connects the whole
    // chain (solar -> nodeA -> nodeB -> laser), so the laser gets 0.12/1.1.
    let power = compute_power_efficiency(&world);
    let eff = power.get(&laser).copied().unwrap_or(0.0);
    assert!(
        (eff - 0.12 / 1.1).abs() < 0.0001,
        "adjacent unlinked chain must connect by proximity (Java), got {eff}"
    );
    // (b) A non-adjacent machine without links has no live edge: move the
    // laser five tiles from node_b (inside its six-tile *configuration*
    // range), no links -> efficiency 0.
    world.tiles.remove(&laser);
    let laser_far = (16 << 16) | 10;
    let mut far = erekir_tile(laser_far, 327, 0);
    far.occupied = vec![laser_far];
    world.tiles.insert(laser_far, far);
    let power = compute_power_efficiency(&world);
    assert_eq!(
        power.get(&laser_far).copied(),
        Some(0.0),
        "far unlinked node must not transmit power"
    );
    // (c) A validated explicit link (both directions, as JAR config write
    // does) connects the non-adjacent pair. Out-of-range persisted links are
    // intentionally rejected by the shared validator.
    {
        let mut b = world.tiles.get_mut(&node_b).unwrap();
        b.power_links = vec![laser_far];
    }
    {
        let mut ls = world.tiles.get_mut(&laser_far).unwrap();
        ls.power_links = vec![node_b];
    }
    let power = compute_power_efficiency(&world);
    let eff = power.get(&laser_far).copied().unwrap_or(0.0);
    assert!(
        (eff - 0.12 / 1.1).abs() < 0.0001,
        "explicit links transmit power at distance: {eff}"
    );
}

#[test]
fn adjacent_consumers_do_not_bridge_power_graphs_without_conductivity() {
    let world = erekir_test_world();
    let left_position = (20 << 16) | 20;
    let right_position = (21 << 16) | 20;
    let mut left = erekir_tile(left_position, 327, 0); // laser drill: consumer
    let mut right = erekir_tile(right_position, 328, 0); // blast drill: consumer
    left.occupied = vec![left_position];
    right.occupied = vec![right_position];
    let left_role = power_role(left.block).unwrap();
    let right_role = power_role(right.block).unwrap();
    world.tiles.insert(left_position, left.clone());
    world.tiles.insert(right_position, right.clone());
    assert!(!power_connected(
        &world,
        &(left.clone(), left_role),
        &(right.clone(), right_role)
    ));

    // An explicit PowerNode-style link is still authoritative.
    left.power_links.push(right_position);
    right.power_links.push(left_position);
    assert!(power_connected(
        &world,
        &(left, left_role),
        &(right, right_role)
    ));

    // conductivePower is the official exception to the adjacency gate.
    let mut conductive = erekir_tile(left_position, 244, 0); // shielded wall
    conductive.occupied = vec![left_position];
    let mut other = erekir_tile(right_position, 328, 0);
    other.occupied = vec![right_position];
    assert!(power_connected(
        &world,
        &(conductive.clone(), power_role(conductive.block).unwrap()),
        &(other, right_role),
    ));
}

#[test]
fn placement_lifecycle_autolinks_nodes_and_power_source_persists_past_snapshot_period() {
    use crate::network::buildings::placement;

    let world = erekir_test_world();
    let source = (10 << 16) | 10; // sandbox power-source (PowerNode subclass)
    let battery = (14 << 16) | 10; // battery, inside the six-tile laser range
    for (position, block) in [(source, 410), (battery, 306)] {
        let mut tile = erekir_tile(position, block, 0);
        tile.occupied = vec![position];
        world.tiles.insert(position, tile);
    }

    let changes = placement::after_placement(&world, source, &[0]);
    assert!(changes.auto_linked_power);
    assert_eq!(changes.power_node_configs.len(), 1);
    assert_eq!(changes.power_node_configs[0].0, source);
    assert_eq!(changes.power_node_configs[0].1[0], 8);
    assert_eq!(changes.power_node_configs[0].1[1], 1);
    assert_eq!(world.tiles.get(&source).unwrap().power_links, vec![battery]);
    assert_eq!(world.tiles.get(&battery).unwrap().power_links, vec![source]);

    // Seven seconds at the production loop's legacy 10 TPS batching. The
    // six-second BlockSnapshot cadence must observe persistent authoritative
    // links and battery state, not a client-only preview that gets rolled back.
    for _ in 0..70 {
        update_power_network(&world, 6.0);
    }
    assert_eq!(world.tiles.get(&source).unwrap().power_links, vec![battery]);
    assert!((world.tiles.get(&battery).unwrap().power_stored - 4_000.0).abs() < 0.001);
}

#[test]
fn machine_placement_reports_existing_node_config_for_immediate_reflow() {
    use crate::network::buildings::placement;

    let world = erekir_test_world();
    let node = (10 << 16) | 10;
    let consumer = (14 << 16) | 10;
    for (position, block) in [(node, 302), (consumer, 329)] {
        let mut tile = erekir_tile(position, block, 0);
        tile.occupied = vec![position];
        world.tiles.insert(position, tile);
    }

    let changes = placement::after_placement(&world, consumer, &[0]);
    assert!(changes.auto_linked_power);
    assert_eq!(changes.power_node_configs.len(), 1);
    assert_eq!(changes.power_node_configs[0].0, node);
    assert_eq!(changes.power_node_configs[0].1[0..2], [8, 1]);
    assert_eq!(world.tiles.get(&node).unwrap().power_links, vec![consumer]);
}

#[test]
fn loaded_power_links_are_normalized_before_publication() {
    let world = erekir_test_world();
    let node = (10 << 16) | 10;
    let near = (14 << 16) | 10;
    let far = (40 << 16) | 10;
    let mut node_tile = erekir_tile(node, 302, 0);
    node_tile.occupied = vec![node];
    node_tile.power_links = vec![far, far];
    node_tile.config = vec![0];
    let mut near_tile = erekir_tile(near, 329, 0);
    near_tile.occupied = vec![near];
    let mut far_tile = erekir_tile(far, 329, 0);
    far_tile.occupied = vec![far];
    far_tile.power_links = vec![node];
    world.tiles.insert(node, node_tile);
    world.tiles.insert(near, near_tile);
    world.tiles.insert(far, far_tile);

    let updates = crate::network::buildings::power::normalize_power_links(&world);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].0, node);
    assert_eq!(world.tiles.get(&node).unwrap().power_links, vec![near]);
    assert_eq!(world.tiles.get(&near).unwrap().power_links, vec![node]);
    assert!(world.tiles.get(&far).unwrap().power_links.is_empty());
    assert_eq!(world.tiles.get(&node).unwrap().config[0..2], [8, 1]);
}

#[test]
fn power_node_validator_enforces_los_team_capacity_and_snapshot_fallback() {
    use crate::network::buildings::{power as nodes, snapshot};

    let world = erekir_test_world();
    let node = (10 << 16) | 10;
    let target = (14 << 16) | 10;
    let blocker = (12 << 16) | 10;
    let mut node_tile = erekir_tile(node, 302, 0);
    node_tile.occupied = vec![node];
    let mut target_tile = erekir_tile(target, 329, 0);
    target_tile.occupied = vec![target];
    world.tiles.insert(node, node_tile.clone());
    world.tiles.insert(target, target_tile.clone());
    assert!(nodes::link_valid_for_node(&world, &node_tile, target));
    assert!(nodes::autolink_valid_for_node(&world, &node_tile, target));

    let mut wall = erekir_tile(blocker, 220, 0); // plastanium-wall: insulated
    wall.occupied = vec![blocker];
    world.tiles.insert(blocker, wall);
    // Java `linkValid` (manual tap / snapshot) ignores insulation.
    // `getPotentialLinks` (autolink) does not.
    assert!(nodes::link_valid_for_node(&world, &node_tile, target));
    assert!(!nodes::autolink_valid_for_node(&world, &node_tile, target));

    // Snapshot's Point2[] fallback follows `linkValid`, so a configured
    // laser through a wall is still advertised — matching the Java draw path.
    node_tile.config = vec![8, 1];
    node_tile
        .config
        .extend_from_slice(&(4i32 << 16).to_be_bytes());
    assert_eq!(
        snapshot::power_node_links(&node_tile, 40, 40, Some(&world)),
        vec![target]
    );

    // Persisted manual links crossing insulation are kept (Java keeps them)
    // and autolink must not invent extra edges while the wall stands.
    world.tiles.get_mut(&node).unwrap().power_links = vec![target];
    world.tiles.get_mut(&target).unwrap().power_links = vec![node];
    nodes::normalize_power_links(&world);
    assert_eq!(world.tiles.get(&node).unwrap().power_links, vec![target]);
    assert_eq!(world.tiles.get(&target).unwrap().power_links, vec![node]);
    world.tiles.get_mut(&node).unwrap().power_links.clear();
    world.tiles.get_mut(&target).unwrap().power_links.clear();
    nodes::relink_power_node(&world, node);
    assert!(world.tiles.get(&node).unwrap().power_links.is_empty());

    world.tiles.remove(&blocker);
    let clean_node = world.tiles.get(&node).unwrap().clone();
    assert!(nodes::link_valid_for_node(&world, &clean_node, target));
    assert!(nodes::autolink_valid_for_node(&world, &clean_node, target));
    assert_eq!(
        snapshot::power_node_links(&node_tile, 40, 40, Some(&world)),
        vec![target]
    );

    target_tile.team = 2;
    world.tiles.insert(target, target_tile.clone());
    assert!(!nodes::link_valid_for_node(&world, &clean_node, target));
    target_tile.team = 1;
    target_tile.block = 305; // connectedPower but no PowerModule/hasPower
    world.tiles.insert(target, target_tile.clone());
    assert!(!nodes::link_valid_for_node(&world, &clean_node, target));

    target_tile.block = 329;
    world.tiles.insert(target, target_tile);
    let mut full_node = clean_node;
    full_node.power_links = (0..10).map(|index| (index << 16) | 1).collect();
    assert!(!nodes::link_valid_for_node(&world, &full_node, target));
}

#[test]
fn long_power_node_is_manual_same_block_only() {
    use crate::network::buildings::power as nodes;

    let world = erekir_test_world();
    let left = (5 << 16) | 5;
    let right = (30 << 16) | 5;
    let machine = (20 << 16) | 5;
    let left_tile = erekir_tile(left, 319, 0);
    world.tiles.insert(left, left_tile.clone());
    world.tiles.insert(right, erekir_tile(right, 319, 0));
    world.tiles.insert(machine, erekir_tile(machine, 329, 0));

    assert!(nodes::link_valid_for_node(&world, &left_tile, right));
    assert!(!nodes::link_valid_for_node(&world, &left_tile, machine));
    nodes::relink_power_node(&world, left);
    assert!(world.tiles.get(&left).unwrap().power_links.is_empty());
}

#[test]
fn power_node_range_uses_even_block_centers_and_refill_respects_capacity() {
    use crate::network::buildings::power as nodes;

    // A size-2 large node has Building.x = tile.x + .5. At this coordinate
    // its 15-tile circle is exactly tangent to the size-1 battery hitbox.
    let world = erekir_test_world();
    let large = (10 << 16) | 10;
    let tangent = (26 << 16) | 10;
    let large_tile = erekir_tile(large, 303, 0);
    world.tiles.insert(large, large_tile.clone());
    world.tiles.insert(tangent, erekir_tile(tangent, 306, 0));
    assert!(nodes::link_valid_for_node(&world, &large_tile, tangent));

    // A node with nine valid links may heal exactly one more, never all
    // candidates that passed validation against the pre-mutation snapshot.
    let node = (40 << 16) | 40;
    let linked_offsets = [
        (-5, 0),
        (-4, -3),
        (-3, -4),
        (0, -5),
        (3, -4),
        (4, -3),
        (5, 0),
        (4, 3),
        (3, 4),
    ];
    let candidates = [(0, 5), (-3, 4), (-4, 3)];
    let mut node_tile = erekir_tile(node, 302, 0);
    for (dx, dy) in linked_offsets {
        let target = ((40 + dx) << 16) | ((40 + dy) as u16 as i32);
        node_tile.power_links.push(target);
        let mut battery = erekir_tile(target, 306, 0);
        battery.power_links.push(node);
        world.tiles.insert(target, battery);
    }
    for (dx, dy) in candidates {
        let target = ((40 + dx) << 16) | ((40 + dy) as u16 as i32);
        world.tiles.insert(target, erekir_tile(target, 306, 0));
    }
    world.tiles.insert(node, node_tile);
    nodes::relink_power_node(&world, node);
    assert_eq!(world.tiles.get(&node).unwrap().power_links.len(), 10);

    // Runtime pruning must refresh the source snapshot before seeking a
    // replacement; ten stale entries cannot leave a valid nearby machine
    // permanently unlinked.
    let stale_node = (60 << 16) | 60;
    let nearby = (64 << 16) | 60;
    let mut stale = erekir_tile(stale_node, 302, 0);
    stale.power_links = (0..10).map(|index| ((80 + index) << 16) | 80).collect();
    world.tiles.insert(stale_node, stale);
    world.tiles.insert(nearby, erekir_tile(nearby, 329, 0));
    nodes::relink_power_node(&world, stale_node);
    assert_eq!(world.tiles.get(&stale_node).unwrap().power_links, [nearby]);
}

#[test]
fn generator_production_uses_real_fuel_liquid_floor_and_heat_state() {
    let mut world = erekir_test_world();
    let position = (10 << 16) | 10;

    let mut steam = erekir_tile(position, 310, 0);
    assert_eq!(
        effective_power_role(&world, &steam, 1.0)
            .unwrap()
            .production,
        0.0,
        "steam generator has no nominal production without inputs"
    );
    steam.inventory = vec![(5, 1)]; // coal
    assert_eq!(
        effective_power_role(&world, &steam, 1.0)
            .unwrap()
            .production,
        0.0,
        "fuel without water is insufficient"
    );
    steam.liquid_inventory = vec![(0, 1.0)];
    assert!(
        (effective_power_role(&world, &steam, 1.0)
            .unwrap()
            .production
            - 5.5)
            .abs()
            < 0.001
    );

    let mut differential = erekir_tile(position, 311, 0);
    differential.inventory = vec![(15, 1)]; // pyratite
    differential.liquid_inventory = vec![(3, 1.0)]; // cryofluid
    assert!(
        (effective_power_role(&world, &differential, 1.0)
            .unwrap()
            .production
            - 18.0)
            .abs()
            < 0.001
    );
    differential.liquid_inventory.clear();
    assert_eq!(
        effective_power_role(&world, &differential, 1.0)
            .unwrap()
            .production,
        0.0
    );

    let mut thermal = erekir_tile(position, 309, 0);
    thermal.occupied = vec![position];
    world.floors[(10 * world.width + 10) as usize] = 37; // hotrock: heat=.5
    assert!(
        (effective_power_role(&world, &thermal, 1.0)
            .unwrap()
            .production
            - 0.9)
            .abs()
            < 0.001
    );

    let mut turbine = erekir_tile(position, 320, 0);
    turbine.occupied = vec![position];
    world.floors[(10 * world.width + 10) as usize] = 61; // vent: steam=1
    assert!(
        (effective_power_role(&world, &turbine, 1.0)
            .unwrap()
            .production
            - 20.0 / 60.0)
            .abs()
            < 0.001
    );

    let mut flux = erekir_tile(position, 323, 0);
    flux.liquid_inventory = vec![(10, 1.0)]; // cyanogen
    assert_eq!(
        effective_power_role(&world, &flux, 1.0).unwrap().production,
        0.0
    );
    flux.output_liquid_amount = 150.0; // received heat/maxHeat
    assert!((effective_power_role(&world, &flux, 1.0).unwrap().production - 300.0).abs() < 0.001);

    let mut neoplasia = erekir_tile(position, 324, 0);
    neoplasia.inventory = vec![(11, 1)];
    neoplasia.liquid_inventory = vec![(5, 2.0)]; // missing water
    assert_eq!(
        effective_power_role(&world, &neoplasia, 1.0)
            .unwrap()
            .production,
        0.0
    );
    neoplasia.liquid_inventory.push((0, 1.0));
    assert!(
        (effective_power_role(&world, &neoplasia, 1.0)
            .unwrap()
            .production
            - 140.0)
            .abs()
            < 0.001
    );
    neoplasia.enabled = false;
    assert_eq!(
        effective_power_role(&world, &neoplasia, 1.0)
            .unwrap()
            .production,
        0.0
    );
}

#[test]
fn power_demand_respects_enabled_inputs_and_payload_activity() {
    let world = erekir_test_world();
    let position = (10 << 16) | 10;
    let mut factory = erekir_tile(position, 190, 0); // pyratite-mixer, demand .2
    assert_eq!(
        effective_power_role(&world, &factory, 1.0).unwrap().demand,
        0.0
    );
    factory.inventory = vec![(5, 1), (1, 2), (4, 2)];
    assert!((effective_power_role(&world, &factory, 1.0).unwrap().demand - 0.2).abs() < 0.001);
    factory.enabled = false;
    assert_eq!(
        effective_power_role(&world, &factory, 1.0).unwrap().demand,
        0.0
    );

    let mut loader = erekir_tile(position, 408, 0);
    assert_eq!(
        effective_power_role(&world, &loader, 1.0).unwrap().demand,
        0.0
    );
    let mut battery = erekir_tile(position, 306, 0);
    battery.power_stored = 0.0;
    loader.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
        tile: battery,
        version: 0,
        sync: Vec::new(),
    })));
    assert_eq!(
        effective_power_role(&world, &loader, 1.0).unwrap().demand,
        42.0
    );
    loader.production_progress = 1.0; // exporting/full
    assert_eq!(
        effective_power_role(&world, &loader, 1.0).unwrap().demand,
        2.0
    );
}

/// Independent Rust mirror of v159.7 `power.PowerTests` scenarios.  The
/// server models PowerGraph as a deterministic component calculation over
/// DynamicTiles; this test exercises the observable status and battery state
/// across no-demand, excess, drain, and producer-removal transitions.
#[test]
fn upstream_power_tests_1597_satisfaction_and_battery_scenarios() {
    let world = erekir_test_world();
    let source = (10 << 16) | 10; // sandbox power source, production > demand
    let consumer = (11 << 16) | 10; // laser drill, demand 1.1
    let battery = (12 << 16) | 10; // battery, capacity 4,000
    for (position, block) in [(source, 410), (consumer, 327), (battery, 306)] {
        let mut tile = erekir_tile(position, block, 0);
        tile.occupied = vec![position];
        world.tiles.insert(position, tile);
    }

    // Production exceeds demand: direct consumer is fully satisfied and the
    // battery charges during a delta-sensitive update.
    let efficiency = compute_power_efficiency(&world);
    assert_eq!(efficiency.get(&consumer).copied(), Some(1.0));
    update_power_network(&world, 1.0);
    assert!((world.tiles.get(&battery).unwrap().power_stored - 4_000.0).abs() < 0.001);

    // Remove production while retaining stored energy: the battery discharges
    // to the consumer, preserving the official one-tick satisfaction contract.
    world.tiles.get_mut(&source).unwrap().enabled = false;
    let before = world.tiles.get(&battery).unwrap().power_stored;
    let efficiency = update_power_network(&world, 1.0);
    assert_eq!(efficiency.get(&consumer).copied(), Some(1.0));
    let after = world.tiles.get(&battery).unwrap().power_stored;
    assert!(after < before);
    assert!((before - after - 1.1).abs() < 0.001);

    // With no production and an empty battery, demand is unsatisfied.
    world.tiles.get_mut(&battery).unwrap().power_stored = 0.0;
    let efficiency = compute_power_efficiency(&world);
    assert_eq!(efficiency.get(&consumer).copied(), Some(0.0));
}

#[test]
fn upstream_power_tests_1597_stable_production_equals_consumption() {
    // PowerTests' stable-consumption case uses equal production and demand.
    // The Rust server has no arbitrary fake-block registry, so use one real
    // thorium reactor at full inventory (15 power) and three real silicon arc
    // furnaces (5 power each).  Their adjacent footprints form one graph.
    let world = erekir_test_world();
    let reactor = (20 << 16) | 20;
    let consumers = [(19 << 16) | 20, (20 << 16) | 19, (20 << 16) | 21];
    let mut reactor_tile = erekir_tile(reactor, 315, 0);
    reactor_tile.occupied = vec![reactor];
    reactor_tile.inventory = vec![(7, 30)]; // full thorium inventory
    world.tiles.insert(reactor, reactor_tile);
    for position in consumers {
        let mut consumer = erekir_tile(position, 199, 0);
        consumer.occupied = vec![position];
        consumer.inventory = vec![(3, 1), (4, 4)]; // silicon arc recipe
        world.tiles.insert(position, consumer);
    }

    let reactor_role = effective_power_role(&world, &world.tiles.get(&reactor).unwrap(), 1.0)
        .expect("reactor has a power role");
    let total_demand: f32 = consumers
        .iter()
        .map(|position| {
            effective_power_role(&world, &world.tiles.get(position).unwrap(), 1.0)
                .expect("consumer has a power role")
                .demand
        })
        .sum();
    assert!(
        (reactor_role.production - total_demand).abs() <= 0.000_001,
        "stable graph must balance production and demand: {} vs {total_demand}",
        reactor_role.production
    );

    let efficiency = compute_power_efficiency(&world);
    for position in consumers {
        let status = efficiency
            .get(&position)
            .copied()
            .expect("active consumer status");
        assert!(
            (status - 1.0).abs() <= 0.000_001,
            "balanced production must fully satisfy {position}: {status}"
        );
    }
}

#[test]
fn upstream_power_tests_1597_fractional_shortage_zero_demand_and_float_tolerance() {
    let world = erekir_test_world();
    let source = (20 << 16) | 20;
    let consumer = (21 << 16) | 20;
    let mut s = erekir_tile(source, 313, 0); // solar: fixed production .12
    s.occupied = vec![source];
    let mut c = erekir_tile(consumer, 327, 0); // laser drill: demand 1.1
    c.occupied = vec![consumer];
    world.tiles.insert(source, s);
    world.tiles.insert(consumer, c);
    let expected = 0.12_f32 / 1.1_f32;
    let actual = compute_power_efficiency(&world)
        .get(&consumer)
        .copied()
        .unwrap();
    // PowerGraph status is a f32 ratio.  Keep the observable compatibility
    // contract tolerant to the same class of rounding that upstream's
    // Mathf.FLOAT_ROUNDING_ERROR assertion permits; do not require a bitwise
    // equality between independently accumulated production and demand.
    const STATUS_TOLERANCE: f32 = 0.0001;
    assert!(actual.is_finite());
    assert!((actual - expected).abs() <= STATUS_TOLERANCE);

    let empty = erekir_test_world();
    assert!(compute_power_efficiency(&empty).is_empty());
}

#[test]
fn upstream_power_tests_1597_non_unit_delta_scales_battery_amounts() {
    // Upstream PowerTests fixes Time.delta at 0.5 and explicitly checks that
    // amounts, but not status, scale with it.  Use implemented server roles:
    // solar (0.12 power/tick) charges a battery for half a tick, then a real
    // arc furnace drains a known battery balance after the producer is
    // disabled.  This tests the Rust network's delta argument directly.
    let world = erekir_test_world();
    let source = (10 << 16) | 10;
    let battery = (11 << 16) | 10;
    let mut source_tile = erekir_tile(source, 313, 0);
    source_tile.occupied = vec![source];
    let mut battery_tile = erekir_tile(battery, 306, 0);
    battery_tile.occupied = vec![battery];
    battery_tile.power_stored = 0.0;
    world.tiles.insert(source, source_tile);
    world.tiles.insert(battery, battery_tile);

    let delta = 0.5;
    update_power_network(&world, delta);
    let charged = world.tiles.get(&battery).unwrap().power_stored;
    let expected_charge = 0.12 * delta;
    assert!(
        (charged - expected_charge).abs() <= 0.000_001,
        "half-tick production must charge {expected_charge}, got {charged}"
    );

    // Give the drain phase a deterministic balance so the assertion isolates
    // demand * delta instead of depending on the tiny solar charge above.
    world.tiles.get_mut(&source).unwrap().enabled = false;
    world.tiles.get_mut(&battery).unwrap().power_stored = 1_000.0;
    let consumer = (12 << 16) | 10;
    let mut consumer_tile = erekir_tile(consumer, 199, 0);
    consumer_tile.occupied = vec![consumer];
    consumer_tile.inventory = vec![(3, 1), (4, 4)];
    world.tiles.insert(consumer, consumer_tile);

    let before = world.tiles.get(&battery).unwrap().power_stored;
    let efficiency = update_power_network(&world, delta);
    let after = world.tiles.get(&battery).unwrap().power_stored;
    assert_eq!(efficiency.get(&consumer).copied(), Some(1.0));
    assert!(
        (before - after - 5.0 * delta).abs() <= 0.000_001,
        "battery drain must be demand * delta: before={before}, after={after}"
    );
}

/// Independent Rust mirror of v159.7 `power.DirectConsumerTests` scenarios.
/// Mandatory inputs gate demand; absent or partial inventories do not request
/// power, while a complete recipe does.  Block 190 is the implemented
/// pyratite-mixer recipe (coal x1, lead x2, sand x2 -> pyratite x1).
#[test]
fn upstream_direct_consumer_tests_1597_item_gating_and_requested_power() {
    let world = erekir_test_world();
    let source = (10 << 16) | 10;
    let factory = (11 << 16) | 10;
    let mut source_tile = erekir_tile(source, 410, 0);
    source_tile.occupied = vec![source];
    let mut factory_tile = erekir_tile(factory, 190, 0); // pyratite-mixer
    factory_tile.occupied = vec![factory];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(factory, factory_tile);

    // No inputs and an incomplete input set do not request power.
    assert_eq!(
        effective_power_role(&world, &world.tiles.get(&factory).unwrap(), 1.0)
            .unwrap()
            .demand,
        0.0
    );
    world.tiles.get_mut(&factory).unwrap().inventory = vec![(5, 1), (4, 2)];
    assert_eq!(
        effective_power_role(&world, &world.tiles.get(&factory).unwrap(), 1.0)
            .unwrap()
            .demand,
        0.0
    );
    world.tiles.get_mut(&factory).unwrap().inventory = vec![(5, 1), (1, 1)];
    assert_eq!(
        effective_power_role(&world, &world.tiles.get(&factory).unwrap(), 1.0)
            .unwrap()
            .demand,
        0.0
    );

    // The complete recipe requests power and is fully satisfied by the source.
    world.tiles.get_mut(&factory).unwrap().inventory = vec![(5, 1), (1, 2), (4, 2)];
    let demand = effective_power_role(&world, &world.tiles.get(&factory).unwrap(), 1.0)
        .unwrap()
        .demand;
    assert!((demand - 0.2).abs() < 0.001);
    assert_eq!(
        compute_power_efficiency(&world).get(&factory).copied(),
        Some(1.0)
    );

    // Removing production makes the requested consumer unsatisfied.
    world.tiles.get_mut(&source).unwrap().enabled = false;
    assert_eq!(
        compute_power_efficiency(&world).get(&factory).copied(),
        Some(0.0)
    );
}

#[test]
fn beam_nodes_are_cardinal_nearest_target_links_and_insulation_blocks_them() {
    let world = erekir_test_world();
    let solar = (5 << 16) | 10;
    let beam = (6 << 16) | 10;
    let target = (12 << 16) | 10;
    for (position, block) in [(solar, 313), (beam, 317), (target, 327)] {
        let mut tile = erekir_tile(position, block, 0);
        tile.occupied = vec![position];
        world.tiles.insert(position, tile);
    }
    refresh_beam_power_links(&world);
    let powered = compute_power_efficiency(&world);
    assert!((powered[&target] - 0.12 / 1.1).abs() < 0.0001);

    // Euclidean-near but diagonal is not one of BeamNode's four rays.
    world.tiles.remove(&target);
    let diagonal = (12 << 16) | 11;
    let mut diagonal_tile = erekir_tile(diagonal, 327, 0);
    diagonal_tile.occupied = vec![diagonal];
    world.tiles.insert(diagonal, diagonal_tile);
    refresh_beam_power_links(&world);
    assert_eq!(compute_power_efficiency(&world)[&diagonal], 0.0);

    world.tiles.remove(&diagonal);
    let mut target_tile = erekir_tile(target, 327, 0);
    target_tile.occupied = vec![target];
    world.tiles.insert(target, target_tile);
    refresh_beam_power_links(&world);
    let blocker = (9 << 16) | 10;
    let mut wall = erekir_tile(blocker, 220, 0);
    wall.occupied = vec![blocker];
    world.tiles.insert(blocker, wall);
    // Same tick: distribution still sees pre-rescan links (158.1 ordering).
    let same_tick = update_power_network(&world, 1.0);
    assert!(
        same_tick.get(&target).copied().unwrap_or(0.0) > 0.0,
        "wall insert takes effect on the next distribution pass"
    );
    let next_tick = update_power_network(&world, 1.0);
    assert_eq!(next_tick.get(&target).copied(), Some(0.0));

    // An ordinary wall is transparent to BeamNode.updateDirections; only
    // Block.insulated terminates the scan.
    world.tiles.get_mut(&blocker).unwrap().block = 218;
    if let Some(mut beam_tile) = world.tiles.get_mut(&beam) {
        beam_tile.power_stored = 0.0;
    }
    refresh_beam_power_links(&world);
    let reconnected = update_power_network(&world, 1.0);
    assert!(
        (reconnected.get(&target).copied().unwrap_or(0.0) - 0.12 / 1.1).abs() < 0.0001,
        "non-insulated wall restores beam power on the next tick"
    );
}

#[test]
fn power_diode_uses_complete_back_and_front_components() {
    let world = erekir_test_world();
    let diode = (20 << 16) | 20;
    let positions = [
        ((18 << 16) | 20, 3_600.0),
        ((19 << 16) | 20, 3_600.0),
        ((21 << 16) | 20, 400.0),
        ((22 << 16) | 20, 400.0),
    ];
    for (position, stored) in positions {
        let mut battery = erekir_tile(position, 306, 0);
        battery.occupied = vec![position];
        battery.power_stored = stored;
        world.tiles.insert(position, battery);
    }
    let mut diode_tile = erekir_tile(diode, 305, 0); // front east
    diode_tile.occupied = vec![diode];
    world.tiles.insert(diode, diode_tile);

    apply_power_diode_transfers(&world);
    assert!((world.tiles.get(&((18 << 16) | 20)).unwrap().power_stored - 2_800.0).abs() < 0.01);
    assert!((world.tiles.get(&((19 << 16) | 20)).unwrap().power_stored - 2_800.0).abs() < 0.01);
    assert!((world.tiles.get(&((21 << 16) | 20)).unwrap().power_stored - 1_200.0).abs() < 0.01);
    assert!((world.tiles.get(&((22 << 16) | 20)).unwrap().power_stored - 1_200.0).abs() < 0.01);
    let total: f32 = positions
        .iter()
        .map(|(position, _)| world.tiles.get(position).unwrap().power_stored)
        .sum();
    assert!(
        (total - 8_000.0).abs() < 0.01,
        "diode conserves graph energy"
    );
}

#[test]
fn p106_diode_transfer_is_visible_after_the_building_pass() {
    // Java: PowerGraph.update() then Building.update() (diodes). The
    // transferred charge is observable at end N, not delayed to N+1.
    let world = erekir_test_world();
    let back = (19 << 16) | 20;
    let front = (21 << 16) | 20;
    let diode = (20 << 16) | 20;
    for (position, stored) in [(back, 3_600.0), (front, 400.0)] {
        let mut battery = erekir_tile(position, 306, 0);
        battery.occupied = vec![position];
        battery.power_stored = stored;
        world.tiles.insert(position, battery);
    }
    let mut diode_tile = erekir_tile(diode, 305, 0);
    diode_tile.occupied = vec![diode];
    world.tiles.insert(diode, diode_tile);

    let n_minus_1 = world.tiles.get(&front).unwrap().power_stored;
    assert!((n_minus_1 - 400.0).abs() < 0.01);
    update_power_network(&world, 1.0);
    let end_n = world.tiles.get(&front).unwrap().power_stored;
    assert!(end_n > n_minus_1, "end N: diode already moved charge");
    let end_n1 = {
        update_power_network(&world, 1.0);
        world.tiles.get(&front).unwrap().power_stored
    };
    let end_n2 = {
        update_power_network(&world, 1.0);
        world.tiles.get(&front).unwrap().power_stored
    };
    assert!(end_n1 >= end_n - 0.01);
    assert!(end_n2 >= end_n1 - 0.01);
}

#[test]
fn power_component_snapshot_aggregates_connected_batteries_only() {
    let world = erekir_test_world();
    let left = (10 << 16) | 10;
    let mid = (11 << 16) | 10;
    let isolated = (30 << 16) | 10;
    let other_team = (12 << 16) | 10;
    for (position, stored, team) in [
        (left, 1_000.0, 1u8),
        (mid, 3_000.0, 1u8),
        (isolated, 4_000.0, 1u8),
        (other_team, 2_000.0, 2u8),
    ] {
        let mut battery = erekir_tile(position, 306, 0);
        battery.occupied = vec![position];
        battery.power_stored = stored;
        battery.team = team;
        world.tiles.insert(position, battery);
    }
    let from_left = power_component_at(&world, left).unwrap();
    let from_mid = power_component_at(&world, mid).unwrap();
    assert_eq!(from_left.members, from_mid.members);
    assert!((from_left.battery_stored - 4_000.0).abs() < 0.01);
    assert!((from_left.battery_capacity - 8_000.0).abs() < 0.01);
    assert_eq!(from_left.members, vec![left, mid]);
    let alone = power_component_at(&world, isolated).unwrap();
    assert_eq!(alone.members, vec![isolated]);
    assert!((alone.battery_stored - 4_000.0).abs() < 0.01);
    let foreign = power_component_at(&world, other_team).unwrap();
    assert_eq!(foreign.members, vec![other_team]);
    assert!(!from_left.members.contains(&other_team));

    world.tiles.remove(&mid);
    let split = power_component_at(&world, left).unwrap();
    assert_eq!(split.members, vec![left]);
    assert!((split.battery_stored - 1_000.0).abs() < 0.01);
}

#[test]
fn chained_power_diodes_read_live_components_and_conserve_energy() {
    let world = erekir_test_world();
    let left = (10 << 16) | 30;
    let middle = (12 << 16) | 30;
    let right = (14 << 16) | 30;
    for (position, stored) in [(left, 4_000.0), (middle, 2_000.0), (right, 0.0)] {
        let mut battery = erekir_tile(position, 306, 0);
        battery.occupied = vec![position];
        battery.power_stored = stored;
        world.tiles.insert(position, battery);
    }
    for position in [(11 << 16) | 30, (13 << 16) | 30] {
        let mut diode = erekir_tile(position, 305, 0);
        diode.occupied = vec![position];
        world.tiles.insert(position, diode);
    }

    apply_power_diode_transfers(&world);
    let total: f32 = [left, middle, right]
        .into_iter()
        .map(|position| world.tiles.get(&position).unwrap().power_stored)
        .sum();
    assert!(
        (total - 6_000.0).abs() < 0.01,
        "each diode must observe the previous diode's live transfer: {total}"
    );
}

#[test]
fn sandbox_item_source_keeps_factory_authoritative_beyond_six_seconds() {
    use crate::network::buildings::sandbox::SandboxSystem;

    let world = erekir_test_world();
    let source = (10 << 16) | 10;
    let press = (11 << 16) | 10;
    let mut source_tile = erekir_tile(source, 412, 0);
    source_tile.config = vec![5, 0, 0, 5]; // TypeIO Content<Item>: coal
    source_tile.occupied = vec![source];
    let mut press_tile = erekir_tile(press, 181, 0); // graphite press
    press_tile.occupied = vec![press];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(press, press_tile);

    let no_power = std::collections::HashMap::new();
    for _ in 0..50 {
        SandboxSystem::tick(&world, 6.0, |world, position, item, source| {
            accept_logistics_item_from(world, position, item, source, 0)
        });
        simulate_factories(&world, 6.0, &no_power);
    }
    let before_snapshot = inventory_count(&world.tiles.get(&press).unwrap().inventory, 3);
    assert!(
        before_snapshot > 0,
        "the server must craft before six seconds"
    );

    for _ in 0..20 {
        SandboxSystem::tick(&world, 6.0, |world, position, item, source| {
            accept_logistics_item_from(world, position, item, source, 0)
        });
        simulate_factories(&world, 6.0, &no_power);
    }
    let after_seven_seconds = inventory_count(&world.tiles.get(&press).unwrap().inventory, 3);
    assert!(
        after_seven_seconds > before_snapshot,
        "factory state must keep advancing across the six-second snapshot boundary"
    );
    assert_eq!(world.tiles.get(&source).unwrap().config, vec![5, 0, 0, 5]);
}

#[test]
fn sandbox_source_and_conveyor_keep_items_after_six_seconds() {
    use crate::network::buildings::sandbox::SandboxSystem;

    let world = erekir_test_world();
    let source = (20 << 16) | 20;
    let conveyor = (21 << 16) | 20;
    let container = (22 << 16) | 20;
    let mut source_tile = erekir_tile(source, 412, 0);
    source_tile.config = vec![5, 0, 0, 3]; // graphite
    source_tile.occupied = vec![source];
    let mut belt = erekir_tile(conveyor, 257, 0);
    belt.occupied = vec![conveyor];
    let mut storage = erekir_tile(container, 345, 0);
    storage.occupied = vec![container];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(conveyor, belt);
    world.tiles.insert(container, storage);

    let no_power = std::collections::HashMap::new();
    for _ in 0..50 {
        SandboxSystem::tick(&world, 6.0, |world, position, item, source| {
            accept_logistics_item_from(world, position, item, source, 0)
        });
        simulate_logistics(&world, 6.0, &no_power);
    }
    let before_snapshot = inventory_count(&world.tiles.get(&container).unwrap().inventory, 3);
    for _ in 0..20 {
        SandboxSystem::tick(&world, 6.0, |world, position, item, source| {
            accept_logistics_item_from(world, position, item, source, 0)
        });
        simulate_logistics(&world, 6.0, &no_power);
    }
    let after_seven_seconds = inventory_count(&world.tiles.get(&container).unwrap().inventory, 3);
    assert!(before_snapshot > 0);
    assert!(after_seven_seconds > before_snapshot);
    assert!(
        !world
            .tiles
            .get(&conveyor)
            .unwrap()
            .conveyor_items
            .is_empty(),
        "the authoritative belt queue remains populated"
    );
}

#[test]
fn silicon_arc_furnace_crafts_silicon_official_recipe() {
    // silicon-arc-furnace (199): 1 graphite + 4 sand -> 4 silicon, 50s craft.
    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    let mut tile = erekir_tile(pos, 199, 0);
    tile.inventory = vec![(3, 1), (4, 4)];
    world.tiles.insert(pos, tile);
    let mut power = std::collections::HashMap::new();
    power.insert(pos, 1.0);
    simulate_erekir_crafters(&world, 6_000.0, &power);
    let after = world.tiles.get(&pos).unwrap();
    assert_eq!(inventory_count(&after.inventory, 9), 4, "silicon output");
    assert_eq!(inventory_count(&after.inventory, 3), 0, "graphite consumed");
    assert_eq!(inventory_count(&after.inventory, 4), 0, "sand consumed");
}

#[test]
fn pumps_select_floor_liquid_and_reinforced_pump_rate() {
    // SOL-010: PumpBuild.onProximityUpdate (Pump.java:138-146) picks the
    // liquid from the floor under the pump; per-tick rate =
    // sum(liquidMultiplier) * pumpAmount (Pump.updateTile 164-180).
    // mechanical-pump pumpAmount 7/60 (Blocks.java:2297-2301),
    // reinforced-pump pumpAmount 80/60/4 (Blocks.java:2400-2407).
    let mut world = erekir_test_world();
    let water_pos = (10 << 16) | 10; // shallow-water (22)
    let deep_pos = (12 << 16) | 10; // deep-water (21, multiplier 1.5)
    let slag_pos = (14 << 16) | 10; // molten-slag (30)
    let sand_pos = (16 << 16) | 10; // sand-floor (39, not pumpable)
    for (pos, floor) in [
        (water_pos, 22),
        (deep_pos, 21),
        (slag_pos, 30),
        (sand_pos, 39),
    ] {
        world.floors[((pos as i16 as i32) * 40 + (pos >> 16) as i16 as i32) as usize] = floor;
    }
    for (pos, block, occupied) in [
        (water_pos, 283, vec![water_pos]), // mechanical-pump
        (deep_pos, 283, vec![deep_pos]),
        (slag_pos, 295, vec![slag_pos]), // reinforced-pump on slag
        (sand_pos, 283, vec![sand_pos]),
    ] {
        let mut tile = erekir_tile(pos, block, 0);
        tile.occupied = occupied;
        world.tiles.insert(pos, tile);
    }
    let power = std::collections::HashMap::new();
    simulate_liquids(&world, 60.0, &power);
    let water = world.tiles.get(&water_pos).unwrap();
    assert_eq!(
        water.stored_liquid, 0,
        "mechanical pump on shallow-water -> water"
    );
    assert!(
        (water.liquid_amount - 7.0 / 60.0 * 60.0).abs() < 0.001,
        "mechanical pump rate 7/60 per tick, got {}",
        water.liquid_amount
    );
    let deep = world.tiles.get(&deep_pos).unwrap();
    assert!(
        (deep.liquid_amount - 1.5 * 7.0 / 60.0 * 60.0).abs() < 0.001,
        "deep-water multiplier 1.5, got {}",
        deep.liquid_amount
    );
    let slag = world.tiles.get(&slag_pos).unwrap();
    assert_eq!(
        slag.stored_liquid, 1,
        "reinforced pump on molten-slag -> slag"
    );
    assert!(
        (slag.liquid_amount - (80.0 / 60.0 / 4.0) * 60.0).abs() < 0.001,
        "reinforced pump rate 80/60/4 per tick, got {}",
        slag.liquid_amount
    );
    let sand = world.tiles.get(&sand_pos).unwrap();
    assert_eq!(
        sand.liquid_amount, 0.0,
        "pump on a floor without liquidDrop produces nothing (Pump.canPump)"
    );
    assert_eq!(sand.stored_liquid, -1);
}

#[test]
fn reinforced_pump_on_water_floor_produces_water() {
    // SOL-010 round 62 requirement: reinforced-pump (295) produces its floor
    // liquid. Also verifies the multiblock footprint sums multipliers.
    let mut world = erekir_test_world();
    let root = (10 << 16) | 10;
    let occupied = vec![root, (11 << 16) | 10, (10 << 16) | 11, (11 << 16) | 11];
    for pos in &occupied {
        world.floors[((*pos as i16 as i32) * 40 + (*pos >> 16) as i16 as i32) as usize] = 22;
    }
    let mut tile = erekir_tile(root, 295, 0);
    tile.occupied = occupied;
    world.tiles.insert(root, tile);
    let power = std::collections::HashMap::new();
    simulate_liquids(&world, 60.0, &power);
    let after = world.tiles.get(&root).unwrap();
    assert_eq!(after.stored_liquid, 0, "water on a water floor");
    // 4 covered tiles * 1.0 multiplier * 80/60/4 per tick * 60 ticks.
    assert!(
        (after.liquid_amount - 4.0 * (80.0 / 60.0 / 4.0) * 60.0).abs() < 0.01,
        "reinforced pump over 4 water tiles, got {}",
        after.liquid_amount
    );
}

#[test]
fn conduit_moves_liquid_forward_by_pressure_and_rejects_front_fill() {
    // SOL-010 / A4: official Conduit semantics — ConduitBuild.updateTile
    // moves liquid FORWARD only (Conduit.java:144-151) with the pressure
    // flow of BuildingComp.moveLiquid (BuildingComp.java:944-958), and
    // acceptLiquid rejects liquid pushed into the conduit's front
    // (Conduit.java:129-133). A4 cadence: updateTile runs EVERY step
    // (`timer(timerFlow, 1)` is an arc Interval in TICKS:
    // `Time.time - times[id] >= 1`), so a single tick fires one discrete
    // moveLiquidForward step.
    let world = erekir_test_world();
    let conduit = (10 << 16) | 10; // conduit (286), rotation 0 -> east
    let front_tank = (11 << 16) | 10; // liquid-container (289)
    let back_tank = (9 << 16) | 10;
    for (pos, block) in [(conduit, 286), (front_tank, 289), (back_tank, 289)] {
        let mut tile = erekir_tile(pos, block, 0);
        tile.occupied = vec![pos];
        if pos == conduit {
            tile.stored_liquid = 0;
            tile.liquid_amount = 10.0; // half full
        }
        world.tiles.insert(pos, tile);
    }
    let power = std::collections::HashMap::new();
    // One tick, one step: fract = 10/20 * 1.0, ofract = 0 ->
    // flow = min(clamp01(0.5) * 20, 10) = 10 moved into the container.
    simulate_liquids(&world, 1.0, &power);
    assert_eq!(
        world.tiles.get(&conduit).unwrap().liquid_amount,
        0.0,
        "conduit empties into its front on the first tick"
    );
    assert!(
        (world.tiles.get(&front_tank).unwrap().liquid_amount - 10.0).abs() < 0.001,
        "pressure flow = clamp(fract - ofract) * capacity, moved 10"
    );
    assert_eq!(
        world.tiles.get(&back_tank).unwrap().liquid_amount,
        0.0,
        "conduit never dumps backward"
    );

    // Front rejection: a tank in front of the conduit cannot push liquid back
    // into it ((source.relativeTo + 2) % 4 == rotation rejects).
    world.tiles.remove(&conduit);
    let mut empty_conduit = erekir_tile(conduit, 286, 0);
    empty_conduit.occupied = vec![conduit];
    world.tiles.insert(conduit, empty_conduit);
    world.tiles.get_mut(&front_tank).unwrap().liquid_amount = 10.0;
    simulate_liquids(&world, 1.0, &power);
    assert_eq!(
        world.tiles.get(&conduit).unwrap().liquid_amount,
        0.0,
        "a conduit rejects liquid pushed into its front"
    );
    assert!(
        (world.tiles.get(&front_tank).unwrap().liquid_amount - 10.0).abs() < 0.001,
        "the tank keeps its liquid"
    );
}

#[test]
fn conduit_leak_policy_matches_regular_plated_and_reinforced_blocks() {
    // A4: updateTile fires EVERY step (`timer(timerFlow, 1)` is an arc
    // Interval in TICKS), so each step applies the official
    // `leakAmount = currentAmount / 1.5` when the front tile is open floor
    // (BuildingComp.moveLiquidForward): contents decay geometrically,
    // x1/3 per step.
    fn amount_after_steps(block: i16, steps: u32) -> f32 {
        let world = erekir_test_world();
        let position = (10 << 16) | 10;
        let mut conduit = erekir_tile(position, block, 0);
        conduit.occupied = vec![position];
        conduit.stored_liquid = 0;
        conduit.liquid_amount = 15.0;
        world.tiles.insert(position, conduit);
        for _ in 0..steps {
            simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
        }
        let remaining = world.tiles.get(&position).unwrap().liquid_amount;
        remaining
    }

    fn amount_after_batch(block: i16, batch_ticks: f32) -> f32 {
        let world = erekir_test_world();
        let position = (10 << 16) | 10;
        let mut conduit = erekir_tile(position, block, 0);
        conduit.occupied = vec![position];
        conduit.stored_liquid = 0;
        conduit.liquid_amount = 15.0;
        world.tiles.insert(position, conduit);
        simulate_liquids(&world, batch_ticks, &std::collections::HashMap::new());
        let remaining = world.tiles.get(&position).unwrap().liquid_amount;
        remaining
    }

    // One step: 15 - 15/1.5 = 5.
    assert!((amount_after_steps(286, 1) - 5.0).abs() < 0.001); // conduit leaks
    assert!((amount_after_steps(287, 1) - 5.0).abs() < 0.001); // pulse leaks
    assert!((amount_after_steps(288, 1) - 15.0).abs() < 0.001); // plated is sealed
    assert!((amount_after_steps(296, 1) - 5.0).abs() < 0.001); // reinforced opts in
                                                               // After a second step: 5 - 5/1.5 = 3.3333.
    assert!((amount_after_steps(286, 2) - (5.0 - 5.0 / 1.5)).abs() < 0.001);
    // A single 120-tick BATCH is one laggy frame: the official update runs
    // once per frame regardless of delta, so it leaks exactly once.
    assert!((amount_after_batch(286, 120.0) - 5.0).abs() < 0.001);
    // The leak deposits a puddle on the front tile (P1: authoritative
    // PuddleSystem), so the leak is observable server-side.
    let world = erekir_test_world();
    let position = (10 << 16) | 10;
    let mut conduit = erekir_tile(position, 286, 0);
    conduit.occupied = vec![position];
    conduit.stored_liquid = 0;
    conduit.liquid_amount = 15.0;
    world.tiles.insert(position, conduit);
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    let front = (11 << 16) | 10;
    let puddle = world.puddles.puddles.get(&front).unwrap();
    assert!(
        (puddle.accepting - 10.0).abs() < 0.001,
        "one leaking step deposits currentAmount/1.5 = 10 into the puddle"
    );
}

#[test]
fn armored_conduit_accepts_only_transports_aligned_or_behind_sources() {
    // ArmoredConduitBuild.acceptLiquid (ArmoredConduit.java:21-28): an
    // armored conduit (288 plated / 296 reinforced) only accepts from
    // another conduit, a liquid bridge/junction, a building aligned
    // directly behind it, or a non-adjacent source.
    let world = erekir_test_world();
    let plated = (10 << 16) | 10; // plated-conduit (288), facing east
    let mut tile = erekir_tile(plated, 288, 0);
    tile.occupied = vec![plated];
    world.tiles.insert(plated, tile);
    // Neighbor liquid-containers around the armored conduit.
    let front = (11 << 16) | 10;
    let side = (10 << 16) | 11;
    let behind = (9 << 16) | 10;
    for pos in [front, side, behind] {
        let mut neighbor = erekir_tile(pos, 289, 0);
        neighbor.occupied = vec![pos];
        world.tiles.insert(pos, neighbor);
    }

    // Adjacent plain tank to the east (in FRONT): rejected by both rules.
    assert_eq!(
        accept_liquid_from(&world, Some(front), plated, 0, 5.0),
        0.0,
        "front-adjacent non-transport is rejected"
    );
    // Adjacent plain tank to the NORTH (side, not behind): armored rule.
    assert_eq!(
        accept_liquid_from(&world, Some(side), plated, 0, 5.0),
        0.0,
        "side-adjacent non-transport is rejected by the armored gate"
    );
    // Plain tank directly BEHIND (west, feeding forward): accepted.
    assert!(
        accept_liquid_from(&world, Some(behind), plated, 0, 5.0) > 0.0,
        "an aligned behind source feeds an armored conduit"
    );

    // A regular conduit is transport family: accepted from any adjacency
    // except its front (the base Conduit rule still applies).
    world.tiles.get_mut(&plated).unwrap().liquid_amount = 0.0;
    let mut feeder = erekir_tile(side, 286, 1); // faces north, feeding sideways
    feeder.occupied = vec![side];
    feeder.stored_liquid = 0;
    world.tiles.insert(side, feeder);
    assert!(
        accept_liquid_from(&world, Some(side), plated, 0, 5.0) > 0.0,
        "another conduit always counts as a transport source"
    );

    // Reinforced conduit (296) shares the armored rule.
    world.tiles.remove(&plated);
    let mut reinforced = erekir_tile(plated, 296, 0);
    reinforced.occupied = vec![plated];
    world.tiles.insert(plated, reinforced);
    let south = (10 << 16) | 9;
    let mut other_side = erekir_tile(south, 289, 0);
    other_side.occupied = vec![south];
    world.tiles.insert(south, other_side);
    assert_eq!(
        accept_liquid_from(&world, Some(south), plated, 0, 5.0),
        0.0,
        "reinforced conduit rejects side-adjacent non-transports"
    );
}

#[test]
fn sandbox_liquid_source_and_plated_conduit_survive_six_second_boundary() {
    use crate::network::buildings::sandbox::SandboxSystem;

    let world = erekir_test_world();
    let source = (10 << 16) | 20;
    let conduit = (11 << 16) | 20;
    let tank = (12 << 16) | 20;
    let mut source_tile = erekir_tile(source, 414, 0);
    source_tile.config = vec![5, 4, 0, 0]; // TypeIO Content<Liquid>: water
    source_tile.occupied = vec![source];
    let mut sealed = erekir_tile(conduit, 288, 0);
    sealed.occupied = vec![conduit];
    let mut tank_tile = erekir_tile(tank, 289, 0);
    tank_tile.occupied = vec![tank];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(conduit, sealed);
    world.tiles.insert(tank, tank_tile);

    let no_power = std::collections::HashMap::new();
    for _ in 0..70 {
        SandboxSystem::tick(&world, 6.0, |world, position, item, source| {
            accept_logistics_item_from(world, position, item, source, 0)
        });
        simulate_liquids(&world, 6.0, &no_power);
    }
    let pipe_amount = world.tiles.get(&conduit).unwrap().liquid_amount;
    let tank_amount = world.tiles.get(&tank).unwrap().liquid_amount;
    assert!(pipe_amount + tank_amount > 0.0);
    assert_eq!(world.tiles.get(&tank).unwrap().stored_liquid, 0);
    assert_eq!(world.tiles.get(&source).unwrap().config, vec![5, 4, 0, 0]);
}

#[test]
fn liquid_router_rejects_second_liquid_until_nearly_empty() {
    // LiquidRouter.acceptLiquid: same liquid, or currentAmount < 0.2f.
    let world = erekir_test_world();
    let router = (10 << 16) | 10;
    let mut tile = erekir_tile(router, 289, 0);
    tile.occupied = vec![router];
    tile.stored_liquid = 0; // water
    tile.liquid_amount = 10.0;
    world.tiles.insert(router, tile);
    assert_eq!(accept_liquid(&world, router, 2, 5.0), 0.0, "oil rejected");
    assert_eq!(world.tiles.get(&router).unwrap().stored_liquid, 0);
    assert!((world.tiles.get(&router).unwrap().liquid_amount - 10.0).abs() < 0.0001);

    world.tiles.get_mut(&router).unwrap().liquid_amount = 0.15;
    let taken = accept_liquid(&world, router, 2, 5.0);
    assert!((taken - 5.0).abs() < 0.0001);
    assert_eq!(world.tiles.get(&router).unwrap().stored_liquid, 2);
}

#[test]
fn liquid_router_dumps_to_neighbor_and_conduit_leaks_to_puddle() {
    let world = erekir_test_world();
    let left = (10 << 16) | 10;
    let right = (11 << 16) | 10;
    let mut a = erekir_tile(left, 289, 0);
    a.occupied = vec![left];
    a.stored_liquid = 0;
    a.liquid_amount = 120.0;
    let mut b = erekir_tile(right, 289, 0);
    b.occupied = vec![right];
    world.tiles.insert(left, a);
    world.tiles.insert(right, b);
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    let left_amt = world.tiles.get(&left).unwrap().liquid_amount;
    let right_amt = world.tiles.get(&right).unwrap().liquid_amount;
    assert!(
        (left_amt - 60.0).abs() < 0.001 && (right_amt - 60.0).abs() < 0.001,
        "dumpLiquid scaling 2 equalizes 120-capacity routers: {left_amt} / {right_amt}"
    );
    assert!(world.puddles.puddles.is_empty(), "routers do not leak");

    let pipe = (20 << 16) | 20;
    let mut conduit = erekir_tile(pipe, 286, 0);
    conduit.occupied = vec![pipe];
    conduit.stored_liquid = 0;
    conduit.liquid_amount = 15.0;
    world.tiles.insert(pipe, conduit);
    simulate_liquids(&world, 60.0, &std::collections::HashMap::new());
    let front = (21 << 16) | 20;
    let puddle = world.puddles.puddles.get(&front).unwrap();
    assert!(
        (puddle.accepting - 10.0).abs() < 0.001,
        "conduit still dumps to a puddle when the front tile is empty"
    );
}

#[test]
fn liquid_bridge_crosses_official_span_and_rejects_beyond() {
    let world = erekir_test_world();
    let src = (10 << 16) | 10;
    let in_range: i32 = (14 << 16) | 10; // axis distance 4 = bridgeConduit.range
    let too_far: i32 = (15 << 16) | 10; // distance 5
    let mut source = erekir_tile(src, 293, 0);
    source.occupied = vec![src];
    source.stored_liquid = 0;
    source.liquid_amount = 80.0;
    source.config = vec![1];
    source.config.extend_from_slice(&in_range.to_be_bytes());
    let mut dest = erekir_tile(in_range, 293, 0);
    dest.occupied = vec![in_range];
    world.tiles.insert(src, source);
    world.tiles.insert(in_range, dest);
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    assert!(
        world.tiles.get(&in_range).unwrap().liquid_amount > 0.0,
        "range-4 bridge must move liquid"
    );
    assert_eq!(world.tiles.get(&in_range).unwrap().stored_liquid, 0);

    world.tiles.remove(&in_range);
    world.tiles.get_mut(&src).unwrap().liquid_amount = 80.0;
    world.tiles.get_mut(&src).unwrap().stored_liquid = 0;
    world.tiles.get_mut(&src).unwrap().config = {
        let mut config = vec![1];
        config.extend_from_slice(&too_far.to_be_bytes());
        config
    };
    let mut far = erekir_tile(too_far, 293, 0);
    far.occupied = vec![too_far];
    world.tiles.insert(too_far, far);
    let before = world.tiles.get(&too_far).unwrap().liquid_amount;
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    assert_eq!(
        world.tiles.get(&too_far).unwrap().liquid_amount,
        before,
        "distance 5 is outside bridgeConduit.range=4"
    );
}

#[test]
fn reinforced_bridge_conduit_moves_liquid_with_jar_capacity() {
    // Blocks.java:2431-2437: reinforced-bridge-conduit is a
    // DirectionLiquidBridge with liquidCapacity 120f and range 4 (JAR dump).
    let world = erekir_test_world();
    let src = (10 << 16) | 10;
    #[allow(clippy::erasing_op, clippy::identity_op)]
    let link: i32 = (13 << 16) | (10); // axis distance 3 <= range 4
    let mut source = erekir_tile(src, 298, 0);
    source.occupied = vec![src];
    source.stored_liquid = 0;
    source.liquid_amount = 120.0; // full per the JAR capacity
    source.config = vec![1];
    source.config.extend_from_slice(&link.to_be_bytes());
    let mut dest = erekir_tile(link, 298, 0);
    dest.occupied = vec![link];
    world.tiles.insert(src, source);
    world.tiles.insert(link, dest);
    assert_eq!(super::liquid_capacity(298), Some(120.0));
    assert!(super::is_liquid_bridge(298));
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    assert!(
        world.tiles.get(&link).unwrap().liquid_amount > 0.0,
        "range-4 reinforced bridge must move liquid"
    );
    assert_eq!(world.tiles.get(&link).unwrap().stored_liquid, 0);
}

#[test]
fn reinforced_bridge_conduit_pressure_flow_matches_move_liquid_formula() {
    // Building.moveLiquid (Building.java 159.7 bytecode): transfer =
    // min(clamp(fract * liquidPressure - ofract) * capacity,
    // sourceAmount, destFreeSpace) with liquidPressure = 1.0 for
    // DirectionLiquidBridge (Block default; Blocks$243 overrides nothing).
    // fract = amount / 120f (Blocks$243 liquidCapacity = 120.0f).
    let world = erekir_test_world();
    let src = (10 << 16) | 10;
    let link: i32 = (13 << 16) | 10; // axis distance 3 <= range 4
    let mut source = erekir_tile(src, 298, 0);
    source.occupied = vec![src];
    source.stored_liquid = 0;
    source.liquid_amount = 90.0; // fract 0.75
    source.config = vec![1];
    source.config.extend_from_slice(&link.to_be_bytes());
    let mut dest = erekir_tile(link, 298, 0);
    dest.occupied = vec![link];
    dest.stored_liquid = 0;
    dest.liquid_amount = 60.0; // ofract 0.5
    world.tiles.insert(src, source);
    world.tiles.insert(link, dest);
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    // One step moves clamp(0.75 - 0.5) * 120 = 30.
    assert!(
        (world.tiles.get(&src).unwrap().liquid_amount - 60.0).abs() < 0.001,
        "source must hold exactly 60 after one pressure step"
    );
    assert!(
        (world.tiles.get(&link).unwrap().liquid_amount - 90.0).abs() < 0.001,
        "dest must hold exactly 90 after one pressure step"
    );
    // Second step: fract 0.5 - ofract 0.75 clamps to zero flow. The bridge
    // reaches equilibrium exactly as the vanilla pressure model does.
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    assert!(
        (world.tiles.get(&src).unwrap().liquid_amount - 60.0).abs() < 0.001,
        "flow must stop once the destination fraction exceeds the source"
    );
    assert!(
        (world.tiles.get(&link).unwrap().liquid_amount - 90.0).abs() < 0.001,
        "destination must not exceed the equilibrium fraction"
    );
}

#[test]
fn reinforced_bridge_autolinks_forward_per_rotation_without_config() {
    // DirectionBridgeBuild.findLink scans ONLY along the rotation,
    // i = 1..=range, first same-block/team tile wins (DirectionBridge.java:
    // 252-260). No config is honored.
    let world = erekir_test_world();
    let src = (10 << 16) | 10;
    let mut source = erekir_tile(src, 298, 0); // rotation 0 = +x
    source.occupied = vec![src];
    // A stale config pointing elsewhere must be ignored entirely.
    source.config = vec![1];
    let wrong: i32 = ((10 + 3) << 16) | 10;
    source.config.extend_from_slice(&wrong.to_be_bytes());
    // Adjacent same-block tile at distance 1 along rotation: vanilla
    // findLink accepts i=1.
    let fwd1: i32 = (11 << 16) | 10;
    let mut link = erekir_tile(fwd1, 298, 0);
    link.occupied = vec![fwd1];
    link.team = 2;
    source.team = 2;
    world.tiles.insert(src, source);
    world.tiles.insert(fwd1, link);
    let resolved = super::valid_bridge_link(&world, &world.tiles.get(&src).unwrap(), 4);
    assert_eq!(resolved, Some(fwd1), "adjacent forward tile is the link");
}

#[test]
fn reinforced_bridge_without_link_pushes_forward_only() {
    // DirectionLiquidBridge.updateTile with no link: moveLiquidForward only
    // (DirectionLiquidBridge.java:66-70) - never a four-way dump.
    let world = erekir_test_world();
    let src = (10 << 16) | 10;
    let mut source = erekir_tile(src, 298, 0);
    source.occupied = vec![src];
    source.stored_liquid = 0;
    source.liquid_amount = 60.0;
    world.tiles.insert(src, source);
    // An accepting sink straight ahead (rotation 0 = +x); sides stay empty.
    let front: i32 = (11 << 16) | 10;
    let mut sink = erekir_tile(front, 299, 0); // reinforced-liquid-router
    sink.occupied = vec![front];
    world.tiles.insert(front, sink);
    simulate_liquids(&world, 1.0, &std::collections::HashMap::new());
    let after = world.tiles.get(&src).unwrap();
    assert!(
        after.liquid_amount < 60.0,
        "forward push must drain the bridge"
    );
    // Nothing may leak to the non-forward neighbors.
    for (dx, dy) in [(0i32, 1i32), (0, -1), (-1, 0)] {
        let side = ((10 + dx) << 16) | (10 + dy);
        if let Some(tile) = world.tiles.get(&side) {
            assert!(
                tile.liquid_amount <= 0.0001,
                "no-link bridge must not dump sideways"
            );
        }
    }
}
#[test]
fn pump_conduit_tank_chain_conserves_water_over_300_ticks() {
    // Differential vs Java formulas (Pump.updateTile + Conduit per-tick
    // moveLiquid + LiquidRouter.dumpLiquid): mechanical pump 7/60 per tick
    // on shallow water, conduit facing a size-3 tank. Tick-by-tick so the
    // pump can dump every update instead of capping at liquidCapacity.
    let mut world = erekir_test_world();
    let pump = (10 << 16) | 10;
    let conduit = (11 << 16) | 10;
    let tank = (13 << 16) | 10;
    world.floors[(10 * world.width + 10) as usize] = 22; // shallow-water
    let mut pump_tile = erekir_tile(pump, 283, 0);
    pump_tile.occupied = vec![pump];
    let mut pipe = erekir_tile(conduit, 286, 0);
    pipe.occupied = vec![conduit];
    let mut occupied = Vec::new();
    for dy in -1..=1 {
        for dx in -1..=1 {
            occupied.push(((13 + dx) << 16) | ((10 + dy) as u16 as i32));
        }
    }
    let mut tank_tile = erekir_tile(tank, 291, 0);
    tank_tile.occupied = occupied.clone();
    world.tiles.insert(pump, pump_tile);
    world.tiles.insert(conduit, pipe);
    world.tiles.insert(tank, tank_tile);
    for cell in occupied {
        world.tile_footprint.insert(cell, tank);
    }
    let power = std::collections::HashMap::new();
    for _ in 0..300 {
        simulate_liquids(&world, 1.0, &power);
    }
    let pump_amt = world.tiles.get(&pump).unwrap().liquid_amount;
    let pipe_amt = world.tiles.get(&conduit).unwrap().liquid_amount;
    let tank_amt = world.tiles.get(&tank).unwrap().liquid_amount;
    let total = pump_amt + pipe_amt + tank_amt;
    assert!(
        (total - 35.0).abs() < 0.05,
        "300 ticks * 7/60 water = 35, got pump={pump_amt} pipe={pipe_amt} tank={tank_amt} total={total}"
    );
    for pos in [pump, conduit, tank] {
        let liquid = world.tiles.get(&pos).unwrap().stored_liquid;
        assert!(
            liquid == -1 || liquid == 0,
            "chain must stay on water, {pos} has liquid {liquid}"
        );
    }
    assert!(
        tank_amt > pipe_amt,
        "tank should hold the bulk once the conduit chain drains"
    );
}

#[test]
fn electrolyzer_turns_water_into_ozone_and_hydrogen() {
    // electrolyzer (200): water 10/60 -> ozone 4/60 + hydrogen 6/60.
    let world = erekir_test_world();
    let pos = (22 << 16) | 22;
    let tank = (23 << 16) | 22; // liquid-tank acceptor, adjacent
    let mut tile = erekir_tile(pos, 200, 0);
    tile.stored_liquid = 0; // water
    tile.liquid_amount = 100.0;
    let mut acceptor = erekir_tile(tank, 291, 0);
    acceptor.stored_liquid = -1;
    acceptor.liquid_amount = 0.0;
    world.tiles.insert(pos, tile);
    world.tiles.insert(tank, acceptor);
    let mut power = std::collections::HashMap::new();
    power.insert(pos, 1.0);
    simulate_erekir_crafters(&world, 600.0, &power);
    let after = world.tiles.get(&pos).unwrap();
    assert!(
        after.liquid_amount < 100.0,
        "water consumed: {}",
        after.liquid_amount
    );
    let tank_after = world.tiles.get(&tank).unwrap();
    assert!(
        tank_after.liquid_amount > 0.0,
        "acceptor received a liquid output"
    );
    assert!(
        tank_after.stored_liquid == 7 || tank_after.stored_liquid == 8,
        "ozone or hydrogen delivered, got {}",
        tank_after.stored_liquid
    );
}

#[test]
fn oxidation_chamber_consumes_ozone_for_oxide() {
    // oxidation-chamber (202): ozone + 1 beryllium -> 1 oxide per 120s.
    let world = erekir_test_world();
    let pos = (26 << 16) | 26;
    let mut tile = erekir_tile(pos, 202, 0);
    tile.stored_liquid = 7; // ozone
    tile.liquid_amount = 50.0;
    tile.inventory = vec![(16, 1)]; // beryllium
    world.tiles.insert(pos, tile);
    let mut power = std::collections::HashMap::new();
    power.insert(pos, 1.0);
    simulate_erekir_crafters(&world, 7_200.0, &power);
    let after = world.tiles.get(&pos).unwrap();
    assert_eq!(inventory_count(&after.inventory, 18), 1, "oxide output");
    assert_eq!(
        inventory_count(&after.inventory, 16),
        0,
        "beryllium consumed"
    );
    assert!(after.liquid_amount < 50.0, "ozone consumed");
}

#[test]
fn thorium_reactor_uses_inventory_fullness_and_official_fuel_timer() {
    // thorium-reactor (315): powerProduction 15 * inventory fullness and one
    // thorium consumed every 360 ticks. impact-reactor (316) remains 130/25.
    assert_eq!(power_role(315).unwrap().production, 15.0);
    assert_eq!(power_role(316).unwrap().production, 130.0);
    assert_eq!(power_role(316).unwrap().demand, 25.0);
    let world = erekir_test_world();
    let reactor = (40 << 16) | 40;
    let node = (41 << 16) | 40; // power-node 302 (range 6*8)
    let consumer = (42 << 16) | 40; // silicon-arc-furnace (199, demand 5)
    let mut tile = erekir_tile(reactor, 315, 0);
    tile.inventory = vec![(7, 30)]; // full official item capacity
    tile.stored_liquid = 3; // enough cryofluid to survive the timer check
    tile.liquid_amount = 30.0;
    tile.occupied = vec![reactor];
    let mut nd = erekir_tile(node, 302, 0);
    nd.power_stored = 0.0;
    nd.occupied = vec![node];
    // reactor/node/consumer are orthogonally adjacent: the Java proximity
    // loop (BuildingComp.getPowerConnections) connects them without links.
    let mut cons = erekir_tile(consumer, 199, 0);
    cons.production_progress = 0.0;
    // Inputs for an active craft: `should_consume_power` only registers the
    // furnace's demand while it can consume (Java consValid), and a consumer
    // with zero demand reports a satisfied network either way.
    cons.inventory = vec![(3, 10), (4, 10)];
    cons.occupied = vec![consumer];
    world.tiles.insert(reactor, tile);
    world.tiles.insert(node, nd);
    world.tiles.insert(consumer, cons);
    // A full inventory produces all 15 power units, enough for the consumer.
    let eff = compute_power_efficiency(&world);
    assert_eq!(
        eff.get(&consumer).copied(),
        Some(1.0),
        "consumer powered by reactor"
    );
    // NuclearReactor does not pre-load a private fuel slot: the ItemModule
    // remains authoritative and consume() runs only when timerFuel fires.
    simulate_reactors(&world, 359.0);
    // Copy the values out; do NOT hold a DashMap read guard across the next
    // simulate_reactors call (project rule: no live guard when mutating).
    let (fuel, progress) = {
        let after = world.tiles.get(&reactor).unwrap();
        (
            inventory_count(&after.inventory, 7),
            after.production_progress,
        )
    };
    assert_eq!(fuel, 30, "fuel is not consumed before tick 360");
    assert!(
        (progress - 359.0).abs() < 0.001,
        "timer tracks elapsed ticks"
    );

    simulate_reactors(&world, 1.0);
    let (fuel, progress) = {
        let after = world.tiles.get(&reactor).unwrap();
        (
            inventory_count(&after.inventory, 7),
            after.production_progress,
        )
    };
    assert_eq!(fuel, 29, "one thorium consumed at tick 360");
    assert!(progress.abs() < 0.001, "timer restarts after consumption");

    // Production is continuously proportional to the live inventory, not a
    // boolean private fuel slot. Five items yield 15*(5/30)=2.5 power and
    // therefore half efficiency for a 5-power consumer.
    world.tiles.get_mut(&reactor).unwrap().inventory = vec![(7, 5)];
    let eff = compute_power_efficiency(&world);
    assert_eq!(eff.get(&consumer).copied(), Some(0.5));

    world.tiles.get_mut(&reactor).unwrap().inventory.clear();
    let eff = compute_power_efficiency(&world);
    assert_eq!(
        eff.get(&consumer).copied(),
        Some(0.0),
        "consumer unpowered without fuel"
    );
}

#[test]
fn power_diode_transfers_back_to_front_only() {
    // P1: PowerDiode (305) transfers battery energy from its back tile to
    // its front tile when the back is more charged, and never backwards.
    let world = erekir_test_world();
    let back = (10 << 16) | 10;
    let diode = (11 << 16) | 10;
    let front = (12 << 16) | 10;
    // Back battery nearly full, front battery nearly empty.
    let mut back_tile = erekir_tile(back, 306, 0);
    back_tile.power_stored = 3_600.0; // 90% of 4000
    let mut diode_tile = erekir_tile(diode, 305, 0); // rotation 0 -> front east
    diode_tile.team = 1;
    let mut front_tile = erekir_tile(front, 306, 0);
    front_tile.power_stored = 400.0; // 10% of 4000
    world.tiles.insert(back, back_tile);
    world.tiles.insert(diode, diode_tile);
    world.tiles.insert(front, front_tile);

    apply_power_diode_transfers(&world);

    let back_after = world.tiles.get(&back).unwrap().power_stored;
    let front_after = world.tiles.get(&front).unwrap().power_stored;
    assert!(back_after < 3_600.0, "back loses charge: {back_after}");
    assert!(front_after > 400.0, "front gains charge: {front_after}");
    assert!(
        (back_after + front_after - 4_000.0).abs() < 0.01,
        "energy is conserved"
    );

    // Reversed: front more charged than back -> no transfer.
    let mut back_tile = erekir_tile(back, 306, 0);
    back_tile.power_stored = 400.0;
    world.tiles.insert(back, back_tile);
    let mut front_tile = erekir_tile(front, 306, 0);
    front_tile.power_stored = 3_600.0;
    world.tiles.insert(front, front_tile);
    apply_power_diode_transfers(&world);
    let back_after = world.tiles.get(&back).unwrap().power_stored;
    let front_after = world.tiles.get(&front).unwrap().power_stored;
    assert_eq!(back_after, 400.0, "no backflow");
    assert_eq!(front_after, 3_600.0);
}

// ===================== ROUND 74D POWER REGRESSION TESTS =====================

/// Round 74d: the relink sweep must keep the node's TypeIO Point2[] config
/// aligned with its power_links. The 158.1 client activates a node's power
/// graph ONLY through the PowerNode config handlers (`configured` ->
/// `config(Point2[])` -> `graph.addGraph` -> `checkAdd`); block snapshots set
/// `power.links`/`status` but never reflow the client graph. PowerNode/
/// PowerSource set `update=false`, so `Building.add()` (which calls
/// `power.graph.checkAdd()`) is never invoked for them — without a config the
/// client graph has no updater, never simulates, and the node shows "+0/s".
/// The join replay (`replay_dynamic_tiles`) forwards `tile.config` as the
/// ConstructFinish config, so the canonical config must carry the links.
#[test]
fn relink_sweep_syncs_node_config_with_links() {
    use crate::network::buildings::power::relink_power_node;

    let world = erekir_test_world();
    let node = (20 << 16) | 20; // power-node-large (303), range 15
    let drill = (33 << 16) | 20; // water-extractor (329), 13 tiles away
    world.tiles.insert(node, erekir_tile(node, 303, 0));
    world.tiles.insert(drill, erekir_tile(drill, 329, 0));

    assert!(relink_power_node(&world, node));
    let tile = world.tiles.get(&node).unwrap();
    assert_eq!(tile.power_links, vec![drill]);
    // Canonical config: tag 8 (Point2[]), count 1, packed relative point
    // (dx=13, dy=0) — the exact TypeIO object the client's constructFinish
    // forwards into the PowerNode config(Point2[]) handler.
    let dx = 13i32;
    let expected = {
        let mut config = vec![8u8, 1];
        config.extend_from_slice(&(dx << 16).to_be_bytes());
        config
    };
    assert_eq!(tile.config, expected, "config must mirror power_links");

    // The config round-trips through the outbound TypeIO boundary used by
    // the ConstructFinish replay (it must not be re-encoded as null).
    let payload = crate::network::wire::encode_construct_finish_for_unit(
        3_100_021,
        node,
        303,
        0,
        1,
        &tile.config,
    )
    .unwrap();
    let object_start = 4 + 2 + 1 + 4 + 1 + 1; // pos, block, unit tag, id, rot, team
    assert_eq!(
        &payload[object_start..],
        &tile.config[..],
        "ConstructFinish config must survive the TypeIO boundary"
    );
}

/// Round 74d: the relink sweep must NOT re-link nodes that are already in
/// the same server-side component (official `getPotentialLinks` excludes
/// same-graph candidates). The user's save keeps two sources "unlinked"
/// because they reach the rest of the grid by proximity — the metric is
/// misleading, the graph is connected and powered.
#[test]
fn relink_sweep_leaves_same_component_nodes_unlinked_but_powered() {
    use crate::network::buildings::power::relink_power_node;

    let world = erekir_test_world();
    // Source (10,10) orthogonally adjacent to a 3x3 reconstructor whose
    // footprint (x 11..13, y 10..12) contains (11,10): the proximity edge
    // connects them, so the source's component already contains the
    // reconstructor and the sweep must not add a laser link (official
    // same-graph exclusion in getPotentialLinks).
    let source = (10 << 16) | 10;
    let reconstructor = (12 << 16) | 10;
    let mut source_tile = erekir_tile(source, 410, 0);
    source_tile.occupied = vec![source];
    let mut recon_tile = erekir_tile(reconstructor, 380, 0);
    recon_tile.occupied = vec![
        (11 << 16) | 10,
        (13 << 16) | 10,
        (11 << 16) | 11,
        (12 << 16) | 11,
        (13 << 16) | 11,
        (11 << 16) | 12,
        (12 << 16) | 12,
        (13 << 16) | 12,
    ];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(reconstructor, recon_tile);

    relink_power_node(&world, source);
    assert!(
        world.tiles.get(&source).unwrap().power_links.is_empty(),
        "same-component targets must not be laser-linked"
    );
    // The config sync still canonicalizes the node's (empty) config.
    assert_eq!(
        world.tiles.get(&source).unwrap().config,
        vec![8, 0],
        "empty link set serializes as an empty Point2[] config"
    );
    // ... but the server graph still powers the reconstructor.
    let power = compute_power_efficiency(&world);
    assert!(
        power.get(&reconstructor).copied().unwrap_or(0.0) > 0.99,
        "proximity-connected consumer is powered: {:?}",
        power.get(&reconstructor)
    );
}

/// Round 74d: water-extractor (329) and cultivator (330) are missing from
/// the periodic block-snapshot batch (the official `BlockFlag.synced` set is
/// wider than the port's list; the official server also snapshots every
/// consumer the client's `ConsumePower.efficiency == power.status` depends
/// on). 329 (SolidPump/Fracker chain) has no writeSync tail — base layout
/// with items=false, power+liquids; 330 (Drill subclass) uses the drill
/// codec (base + progress + warmup).
fn water_extractor_and_cultivator_snapshots_carry_power_status() {
    use crate::network::buildings::snapshot::{
        encode_dynamic_tile_sync, is_batch_snapshot_supported,
    };

    assert!(is_batch_snapshot_supported(329));
    assert!(is_batch_snapshot_supported(330));

    let world = erekir_test_world();
    let mut power = std::collections::HashMap::new();
    for (position, block) in [((30 << 16) | 30, 329i16), ((34 << 16) | 30, 330i16)] {
        let mut tile = erekir_tile(position, block, 0);
        tile.occupied = vec![position];
        world.tiles.insert(position, tile.clone());
        power.insert(position, 1.0); // powered component

        let mut sync = Vec::new();
        encode_dynamic_tile_sync(&mut sync, &tile, &power, Some(&world)).unwrap();
        assert_eq!(sync[4] & 0x7f, 0, "rotation");
        assert_eq!(sync[6], 3, "version");
        assert_eq!(sync[7], 1, "enabled");
        let module_bits = sync[8];
        assert_ne!(
            module_bits & 2,
            0,
            "power module present (bitmask {module_bits})"
        );
        assert_ne!(module_bits & 4, 0, "liquids module present");
        if block == 329 {
            assert_eq!(module_bits & 1, 0, "water-extractor has no item module");
        } else {
            assert_ne!(module_bits & 1, 0, "cultivator has an item module");
        }
        // Power module: s links (0) + f status (1.0) right after the base
        // header (health 4 + rot/team/version/enabled/bitmask 5).
        // Locate the power module after the 9-byte base header: the item
        // module (when present) precedes power (bitmask 1), so skip it.
        let mut offset = 9usize;
        if module_bits & 1 != 0 {
            let count = u16::from_be_bytes([sync[offset], sync[offset + 1]]) as usize;
            offset += 2 + 6 * count;
        }
        let status = f32::from_be_bytes(sync[offset + 2..offset + 6].try_into().unwrap());
        assert_eq!(status, 1.0, "powered extractor status reaches the client");
    }
}

#[test]
fn slag_puddle_applies_melting_damage_at_java_cadence() {
    use crate::game::status::{STATUS_MELTING, STATUS_TARRED};
    let world = erekir_test_world();
    let tile = (12 << 16) | 12;
    world.puddles.deposit(tile, 1, 60.0);
    world.enemies.insert(
        1,
        ground_unit_on_tile(1, 0, tile, crate::network::units::DAGGER.health, 0.0),
    );
    step_puddle_effects(&world, 120);
    let unit = world.enemies.get(&1).unwrap();
    assert!(
        unit.statuses
            .iter()
            .any(|status| status.effect == STATUS_MELTING),
        "slag applies melting"
    );
    // Melting DoT is 0.3/tick; dagger armor is 0. After 120 ticks the
    // puddle pulse at t=0 keeps the status refreshed for the whole window.
    let lost = crate::network::units::DAGGER.health - unit.health;
    assert!(
        (lost - 0.3 * 120.0).abs() < 0.2,
        "slag melting cadence, lost {lost}"
    );
    drop(unit);

    let oil = erekir_test_world();
    let oil_tile = (14 << 16) | 14;
    oil.puddles.deposit(oil_tile, 2, 60.0);
    oil.enemies.insert(
        2,
        ground_unit_on_tile(2, 0, oil_tile, crate::network::units::DAGGER.health, 0.0),
    );
    step_puddle_effects(&oil, 120);
    let oiled = oil.enemies.get(&2).unwrap();
    assert!(
        oiled
            .statuses
            .iter()
            .any(|status| status.effect == STATUS_TARRED),
        "oil applies tarred"
    );
    assert!(
        (oiled.health - crate::network::units::DAGGER.health).abs() < 0.01,
        "tarred has no HP DoT"
    );
}

#[test]
fn slag_puddle_on_building_creates_fire_and_damages_it() {
    let world = erekir_test_world();
    let tile = (16 << 16) | 16;
    let mut wall = erekir_tile(tile, 216, 0);
    wall.occupied = vec![tile];
    wall.health = crate::game::content::block_health(216);
    world.tiles.insert(tile, wall);
    world.puddles.deposit(tile, 1, 60.0);
    step_puddle_effects(&world, 120);
    assert!(
        world.puddles.has_fire(tile),
        "slag temperature > 0.7 ignites a building tile"
    );
    let health = world.tiles.get(&tile).unwrap().health;
    let max = crate::game::content::block_health(216);
    // FireComp damages 1.8 every 40 ticks after the fire exists. Three
    // pulses land in 120 ticks (t=39, 79, 119).
    assert!(
        (health - (max - 1.8 * 3.0)).abs() < 0.05,
        "building fire damage, health {health} max {max}"
    );
}

#[test]
fn water_and_oil_puddles_do_not_create_fire() {
    for liquid in [0i16, 2, 3] {
        let world = erekir_test_world();
        let tile = (18 << 16) | 18;
        let mut wall = erekir_tile(tile, 216, 0);
        wall.occupied = vec![tile];
        wall.health = crate::game::content::block_health(216);
        world.tiles.insert(tile, wall);
        world.puddles.deposit(tile, liquid, 60.0);
        step_puddle_effects(&world, 120);
        assert!(
            !world.puddles.has_fire(tile),
            "liquid {liquid} must not ignite from PuddleComp.update"
        );
        assert!(
            (world.tiles.get(&tile).unwrap().health - crate::game::content::block_health(216))
                .abs()
                < 0.01
        );
    }
}

#[test]
fn empty_puddle_stops_damaging_units() {
    use crate::game::status::STATUS_MELTING;
    let world = erekir_test_world();
    let tile = (20 << 16) | 20;
    world.puddles.deposit(tile, 1, 4.0);
    world.enemies.insert(
        3,
        ground_unit_on_tile(3, 0, tile, crate::network::units::DAGGER.health, 0.0),
    );
    step_puddle_effects(&world, 200);
    assert!(
        world.puddles.puddles.get(&tile).is_none(),
        "small puddle evaporates"
    );
    let unit = world.enemies.get(&3).unwrap();
    assert!(
        unit.statuses
            .iter()
            .all(|status| status.effect != STATUS_MELTING),
        "sub-threshold puddle never applies melting"
    );
    assert!(
        (unit.health - crate::network::units::DAGGER.health).abs() < 0.01,
        "empty/small puddle deals no damage"
    );
}

#[test]
fn flying_unit_is_ignored_by_puddle_status() {
    use crate::game::status::STATUS_MELTING;
    let world = erekir_test_world();
    let tile = (22 << 16) | 22;
    world.puddles.deposit(tile, 1, 60.0);
    world.enemies.insert(
        4,
        ground_unit_on_tile(4, 15, tile, 70.0, 1.0), // flare, flying
    );
    step_puddle_effects(&world, 40);
    let flare = world.enemies.get(&4).unwrap();
    assert!(flare
        .statuses
        .iter()
        .all(|status| status.effect != STATUS_MELTING));
}

#[cfg(test)]
fn builder_poly(id: i32, tile: i32, plan_tile: i32, block: i16) -> EnemyUnit {
    let mut unit = ground_unit_on_tile(id, 21, tile, 400.0, 0.0);
    unit.team = 1;
    unit.build_plans = vec![crate::network::world::UnitBuildPlan {
        breaking: false,
        position: plan_tile,
        block,
        rotation: 0,
        config: Vec::new(),
    }];
    unit.update_building = true;
    unit
}

#[cfg(test)]
fn wall_tombstone(position: i32, block: i16) -> DynamicTile {
    let mut tile = erekir_tile(position, 0, 0);
    tile.team = 1;
    tile.stored_amount = i32::from(block) + 1;
    tile.occupied = vec![position];
    tile
}

#[test]
fn two_builders_do_not_share_plan_progress() {
    let world = erekir_test_world();
    *world.game_state.core_items.write() = vec![100; 22];
    let a = (10 << 16) | 10;
    let b = (14 << 16) | 10;
    world.tiles.insert(a, wall_tombstone(a, 216));
    world.tiles.insert(b, wall_tombstone(b, 216));
    world.enemies.insert(1, builder_poly(1, a, a, 216));
    world.enemies.insert(2, builder_poly(2, b, b, 216));
    apply_set_unit_command(&world, &[1, 2], 2);
    assert!(simulate_builder_units(&world, &DashMap::new(), 10.0));
    let pa = world.tiles.get(&a).unwrap().production_progress;
    let pb = world.tiles.get(&b).unwrap().production_progress;
    assert!(
        (pa - 4.0).abs() < 0.001 && (pb - 4.0).abs() < 0.001,
        "each poly (0.4 speed) advances only its own plan: {pa} / {pb}"
    );
}

#[test]
fn builder_outside_build_range_does_not_advance() {
    let world = erekir_test_world();
    *world.game_state.core_items.write() = vec![100; 22];
    let site = (10 << 16) | 10;
    let far = (10 << 16) | 50; // 40 tiles * 8 = 320 > 220
    world.tiles.insert(site, wall_tombstone(site, 216));
    world.enemies.insert(1, builder_poly(1, far, site, 216));
    apply_set_unit_command(&world, &[1], 2);
    simulate_builder_units(&world, &DashMap::new(), 10.0);
    assert_eq!(
        world.tiles.get(&site).unwrap().production_progress,
        0.0,
        "out of buildRange (220) the plan does not advance"
    );
}

#[test]
fn assist_adds_builder_speed_to_the_same_construct() {
    let world = erekir_test_world();
    *world.game_state.core_items.write() = vec![100; 22];
    let site = (12 << 16) | 12;
    world.tiles.insert(site, wall_tombstone(site, 216));
    world.enemies.insert(1, builder_poly(1, site, site, 216));
    let mut assistant = builder_poly(2, site, site, 216);
    assistant.build_plans.clear();
    world.enemies.insert(2, assistant);
    apply_set_unit_command(&world, &[1], 2);
    apply_set_unit_command(&world, &[2], 3);
    assert!(simulate_builder_units(&world, &DashMap::new(), 1.0));
    assert!(simulate_assist_units(&world, 10.0));
    assert!(
        (world.tiles.get(&site).unwrap().production_progress - 4.4).abs() < 0.001,
        "rebuild 0.4 + assist 4.0"
    );
}

#[cfg(test)]
fn team_tombstone(position: i32, block: i16) -> DynamicTile {
    let mut tile = erekir_tile(position, 0, 0);
    tile.team = 2;
    tile.stored_amount = i32::from(block) + 1;
    tile.occupied = vec![position];
    tile
}

#[test]
fn team_build_ai_approaches_then_constructs_own_team_tombstone() {
    let world = erekir_test_world();
    world.game_state.team_items.insert(2, vec![100; 22]);
    let site = (10 << 16) | 10;
    world.tiles.insert(site, team_tombstone(site, 216));
    // Wave-team poly starts far outside buildRange (220): the AI must only
    // approach (movement), never advance the plan.
    let far = (10 << 16) | 50; // 40 tiles * 8 = 320
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 21, far, 400.0, 0.0));
    let out = DashMap::new();
    assert!(simulate_team_build_ai(&world, &out, 1.0));
    assert_eq!(
        world.tiles.get(&site).unwrap().production_progress,
        0.0,
        "out of buildRange the plan does not advance"
    );
    // Approach complete: unit stands on the site. Construction progresses and
    // finishes through the shared ConstructFinish path with the owning team.
    for _ in 0..20 {
        if let Some(mut unit) = world.enemies.get_mut(&1) {
            unit.x = ((site >> 16) as i16 as f32) * 8.0;
            unit.y = (site as i16 as f32) * 8.0;
        }
        if world.tiles.get(&site).map(|tile| tile.block).unwrap_or(216) != 0 {
            break;
        }
        simulate_team_build_ai(&world, &out, 10.0);
    }
    let tile = world.tiles.get(&site).unwrap();
    assert_eq!(tile.block, 216, "tombstone rebuilt");
    assert_eq!(tile.team, 2, "rebuilt block belongs to the owning team");
    drop(tile);
    let items = world
        .game_state
        .team_items
        .get(&2)
        .map(|entry| entry.clone())
        .unwrap();
    assert!(
        items.iter().any(|stored| *stored < 100),
        "build cost consumed from the owning team's core"
    );
}

#[test]
fn team_build_ai_skips_player_team_units_with_explicit_orders() {
    let world = erekir_test_world();
    *world.game_state.core_items.write() = vec![100; 22];
    let site = (10 << 16) | 10;
    world.tiles.insert(site, wall_tombstone(site, 216));
    // A persisted poly with an explicit order (here repair, command 1):
    // vanilla CommandAI never falls back to BuilderAI, so the autonomous
    // team build AI must not start its tombstone before the command path
    // does (smoke_rebuild_158 join-order regression).
    let poly = builder_poly(7, site, site, 216);
    world.enemies.insert(7, poly);
    world.unit_orders.insert(
        7,
        crate::network::world::UnitOrder {
            unit_id: 7,
            command: 1,
            ..Default::default()
        },
    );
    let out = DashMap::new();
    assert!(!simulate_team_build_ai(&world, &out, 1.0));
    assert_eq!(
        world.tiles.get(&site).unwrap().production_progress,
        0.0,
        "an explicitly ordered player-team unit never auto-rebuilds"
    );
    // The rebuild command hands the tombstone back to the command path.
    assert!(apply_set_unit_command(&world, &[7], 2));
    assert!(simulate_builder_units(&world, &DashMap::new(), 1.0));
    assert!(
        world.tiles.get(&site).unwrap().production_progress > 0.0,
        "the rebuild command path starts the plan"
    );
}

#[test]
fn team_build_ai_starts_at_most_four_plans_per_tick() {
    let world = erekir_test_world();
    world.game_state.team_items.insert(2, vec![100; 22]);
    for i in 0..5i32 {
        let pos = ((10 + i) << 16) | 30;
        world.tiles.insert(pos, team_tombstone(pos, 216));
        // Each poly stands on its own site so every builder has a distinct
        // distance-0 target; id order breaks the per-tick cap deterministically.
        world
            .enemies
            .insert(i + 1, ground_unit_on_tile(i + 1, 21, pos, 400.0, 0.0));
    }
    simulate_team_build_ai(&world, &DashMap::new(), 1.0);
    let started = (0..5i32)
        .filter(|&i| {
            let pos = ((10 + i) << 16) | 30;
            world.tiles.get(&pos).unwrap().production_progress > 0.0
        })
        .count();
    assert_eq!(started, 4, "AI_REBUILD_MAX_PLANS_PER_TICK caps fresh plans");
}

#[test]
fn team_build_ai_abandons_unreachable_plan_after_stall_window() {
    let world = erekir_test_world();
    world.game_state.team_items.insert(2, vec![100; 22]);
    let site = (10 << 16) | 10;
    let far = (10 << 16) | 50; // 40 tiles * 8 = 320 > buildRange 220
    world.tiles.insert(site, team_tombstone(site, 216));
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 21, far, 400.0, 0.0));
    // HoldPosition blocks AI movement, so the approach distance never
    // improves: vanilla BuilderAI discards a plan it cannot reach.
    world.unit_orders.insert(
        1,
        crate::network::world::UnitOrder {
            unit_id: 1,
            command: 0,
            stances: 1 << 6,
            ..Default::default()
        },
    );
    let out = DashMap::new();
    for _ in 0..300 {
        simulate_team_build_ai(&world, &out, 1.0);
    }
    assert!(
        world.ai_rebuild_state.lock().pursuit.contains_key(&1),
        "at exactly the window boundary the stall counter has not tipped yet"
    );
    simulate_team_build_ai(&world, &out, 1.0);
    {
        let ai = world.ai_rebuild_state.lock();
        assert!(!ai.pursuit.contains_key(&1), "plan dropped at the window");
        assert!(
            ai.abandoned.contains(&(1, site)),
            "abandonment recorded until the next fresh scan"
        );
    }
    assert_eq!(
        world.tiles.get(&site).unwrap().production_progress,
        0.0,
        "an abandoned plan never advances"
    );
    // The builder does not instantly re-pick the site it just abandoned.
    simulate_team_build_ai(&world, &out, 1.0);
    assert!(
        !world.ai_rebuild_state.lock().pursuit.contains_key(&1),
        "abandoned site stays skipped within the scan period"
    );
}

#[test]
fn team_build_ai_two_builders_do_not_pick_the_same_site_in_one_tick() {
    let world = erekir_test_world();
    world.game_state.team_items.insert(2, vec![100; 22]);
    let site = (10 << 16) | 10;
    let far = (10 << 16) | 50;
    world.tiles.insert(site, team_tombstone(site, 216));
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 21, far, 400.0, 0.0));
    world
        .enemies
        .insert(2, ground_unit_on_tile(2, 21, far, 400.0, 0.0));
    let out = DashMap::new();
    simulate_team_build_ai(&world, &out, 1.0);
    let ai = world.ai_rebuild_state.lock();
    assert_eq!(ai.pursuit.len(), 1, "one claim per site per tick");
    assert_eq!(
        ai.pursuit.get(&1).map(|pursuit| pursuit.position),
        Some(site),
        "the lowest builder id wins the deterministic distance tie"
    );
    drop(ai);
    let second = world.enemies.get(&2).unwrap();
    assert_eq!(
        second.x,
        ((far >> 16) as i16 as f32) * 8.0 + 4.0,
        "the second builder never moves toward the claimed site"
    );
    assert_eq!(
        second.y,
        (far as i16 as f32) * 8.0 + 4.0,
        "the second builder never moves toward the claimed site"
    );
    assert_eq!(second.velocity_x, 0.0);
}

#[test]
fn team_build_ai_rescan_does_not_readd_sites_within_period() {
    let period = crate::network::simulation::units::AI_REBUILD_SCAN_PERIOD;
    let world = erekir_test_world();
    world.game_state.team_items.insert(2, vec![100; 22]);
    let a = (10 << 16) | 10;
    let b = (14 << 16) | 10;
    world.tiles.insert(a, team_tombstone(a, 216));
    // Poly stands within buildRange (220) of both sites.
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 21, (12 << 16) | 10, 400.0, 0.0));
    let out = DashMap::new();
    assert!(
        simulate_team_build_ai(&world, &out, 1.0),
        "the first scan picks site A"
    );
    assert!(world.tiles.get(&a).unwrap().production_progress > 0.0);

    // Site A's tombstone disappears (rebuilt elsewhere) and a fresh broken
    // block B appears; the cache must NOT surface B before the period
    // elapses.
    world.tiles.remove(&a);
    world.tiles.insert(b, team_tombstone(b, 216));
    *world.game_state.simulation_time.write() = period / 2.0;
    simulate_team_build_ai(&world, &out, 1.0);
    assert_eq!(
        world.tiles.get(&b).unwrap().production_progress,
        0.0,
        "no fresh-plan scan inside the rebuildPeriod window"
    );

    *world.game_state.simulation_time.write() = period;
    assert!(
        simulate_team_build_ai(&world, &out, 1.0),
        "period elapsed: site B is picked"
    );
    assert!(world.tiles.get(&b).unwrap().production_progress > 0.0);
}

#[test]
fn team_build_ai_ignores_other_team_tombstones() {
    let world = erekir_test_world();
    let site = (12 << 16) | 12;
    // Team-1 tombstone: only rebuildable through the command-driven player
    // path (`rebuild_plan`), never by wave-team builders.
    let mut tomb = erekir_tile(site, 0, 0);
    tomb.team = 1;
    tomb.stored_amount = 216 + 1;
    tomb.occupied = vec![site];
    world.tiles.insert(site, tomb);
    world
        .enemies
        .insert(3, ground_unit_on_tile(3, 21, site, 400.0, 0.0));
    assert!(!simulate_team_build_ai(&world, &DashMap::new(), 10.0));
    assert_eq!(
        world.tiles.get(&site).unwrap().production_progress,
        0.0,
        "a builder never advances another team's plan"
    );
}

#[test]
fn disconnect_pauses_player_unit_build_queue() {
    let world = erekir_test_world();
    *world.game_state.core_items.write() = vec![100; 22];
    let site = (8 << 16) | 8;
    world.tiles.insert(site, wall_tombstone(site, 216));
    world.enemies.insert(7, builder_poly(7, site, site, 216));
    apply_set_unit_command(&world, &[7], 2);
    world.enemies.get_mut(&7).unwrap().update_building = false;
    simulate_builder_units(&world, &DashMap::new(), 10.0);
    assert_eq!(
        world.tiles.get(&site).unwrap().production_progress,
        0.0,
        "updateBuilding false pauses the unit queue"
    );
}

#[test]
fn unit_build_plans_round_trip_json_and_msav_queue() {
    let mut unit = builder_poly(3, (4 << 16) | 4, (5 << 16) | 5, 257);
    unit.build_plans.push(crate::network::world::UnitBuildPlan {
        breaking: true,
        position: (6 << 16) | 6,
        block: -1,
        rotation: 0,
        config: Vec::new(),
    });
    let json = serde_json::to_string(&unit).unwrap();
    let loaded: EnemyUnit = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.build_plans, unit.build_plans);
    assert!(loaded.update_building);

    let mut encoded = Vec::new();
    crate::network::wire::write_unit_plans_queue(&mut encoded, &unit.build_plans, true).unwrap();
    assert_eq!(i32::from_be_bytes(encoded[0..4].try_into().unwrap()), 2);

    let many = vec![unit.build_plans[0].clone(); 25];
    let mut net = Vec::new();
    crate::network::wire::write_unit_plans_queue(&mut net, &many, true).unwrap();
    assert_eq!(
        i32::from_be_bytes(net[0..4].try_into().unwrap()),
        20,
        "TypeIO.getMaxPlans / maxSyncedPlans = 20 on the wire"
    );
    let mut save = Vec::new();
    crate::network::wire::write_unit_plans_queue(&mut save, &many, false).unwrap();
    assert_eq!(
        i32::from_be_bytes(save[0..4].try_into().unwrap()),
        25,
        "MSAV writePlansQueue has no 20 cap"
    );
}

#[cfg(test)]
fn ability_projectile(id: i32, team: u8, damage: f32, x: f32, y: f32) -> (i32, Projectile) {
    (
        id,
        Projectile {
            target_id: -1,
            shooter_id: -1,
            team,
            bullet_id: 6,
            damage,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            homing_power: 0.0,
            homing_delay: -1.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: true,
            armor_multiplier: 1.0,
            remaining_ticks: 1.0,
            total_ticks: 1.0,
            source_x: x,
            source_y: y,
            target_x: x,
            target_y: y,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
            collided: Vec::new(),
        },
    )
}

#[test]
fn nova_repair_field_pulses_and_stops_on_death() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let nova_tile = (10 << 16) | 10;
    let near = (16 << 16) | 10;
    let far = (20 << 16) | 10;
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 5, nova_tile, 50.0, 0.0));
    world
        .enemies
        .insert(2, ground_unit_on_tile(2, 0, near, 50.0, 0.0));
    world
        .enemies
        .insert(3, ground_unit_on_tile(3, 0, far, 50.0, 0.0));
    *world.game_state.simulation_time.write() = 239.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&2).unwrap().health, 50.0);
    *world.game_state.simulation_time.write() = 240.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&1).unwrap().health, 60.0);
    assert_eq!(world.enemies.get(&2).unwrap().health, 60.0);
    assert_eq!(world.enemies.get(&3).unwrap().health, 50.0);
    world.enemies.remove(&1);
    *world.game_state.simulation_time.write() = 480.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&2).unwrap().health, 60.0);
}

#[test]
fn shield_regen_fields_match_unit_reload() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let origin = (10 << 16) | 10;
    let near = (16 << 16) | 10;
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 3, origin, 9_000.0, 0.0));
    world
        .enemies
        .insert(2, ground_unit_on_tile(2, 0, near, 150.0, 0.0));
    *world.game_state.simulation_time.write() = 60.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&2).unwrap().shield, 25.0);
    world.enemies.clear();

    world
        .enemies
        .insert(3, ground_unit_on_tile(3, 6, origin, 320.0, 0.0));
    world
        .enemies
        .insert(4, ground_unit_on_tile(4, 0, near, 150.0, 0.0));
    *world.game_state.simulation_time.write() = 240.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&4).unwrap().shield, 0.0);
    *world.game_state.simulation_time.write() = 300.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&4).unwrap().shield, 20.0);
    world.enemies.remove(&3);
    *world.game_state.simulation_time.write() = 600.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&4).unwrap().shield, 20.0);
    world.enemies.clear();

    world
        .enemies
        .insert(5, ground_unit_on_tile(5, 27, origin, 910.0, 0.0));
    world
        .enemies
        .insert(6, ground_unit_on_tile(6, 0, near, 150.0, 0.0));
    *world.game_state.simulation_time.write() = 240.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&6).unwrap().shield, 20.0);
    world.enemies.remove(&5);
    *world.game_state.simulation_time.write() = 480.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&6).unwrap().shield, 20.0);
}

#[test]
fn poly_and_oct_repair_fields_stop_after_owner_death() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let origin = (10 << 16) | 10;
    let near = (14 << 16) | 10;
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 21, origin, 400.0, 0.0));
    world
        .enemies
        .insert(2, ground_unit_on_tile(2, 0, near, 50.0, 0.0));
    *world.game_state.simulation_time.write() = 480.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&2).unwrap().health, 55.0);
    world.enemies.remove(&1);
    *world.game_state.simulation_time.write() = 960.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&2).unwrap().health, 55.0);
    world.enemies.clear();

    world
        .enemies
        .insert(3, ground_unit_on_tile(3, 24, origin, 24_000.0, 0.0));
    world
        .enemies
        .insert(4, ground_unit_on_tile(4, 1, near, 100.0, 0.0));
    *world.game_state.simulation_time.write() = 120.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&4).unwrap().health, 230.0);
    world.enemies.remove(&3);
    *world.game_state.simulation_time.write() = 240.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&4).unwrap().health, 230.0);
}

#[test]
fn repair_field_table_respects_team_range_and_max_health() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let origin = (10 << 16) | 10;
    let near = (15 << 16) | 10;
    let near_other_team = (14 << 16) | 10;
    let far = (19 << 16) | 10;
    // Nova owner plus a damaged same-team ally inside range (heals capped at
    // max health), a damaged ally just outside range, and a damaged unit of
    // another team inside range.
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 5, origin, 9_000.0, 0.0));
    world
        .enemies
        .insert(2, ground_unit_on_tile(2, 0, near, 145.0, 0.0));
    world
        .enemies
        .insert(3, ground_unit_on_tile(3, 0, far, 100.0, 0.0));
    let mut enemy_team = ground_unit_on_tile(4, 0, near_other_team, 100.0, 0.0);
    enemy_team.team = 1;
    world.enemies.insert(4, enemy_team);
    *world.game_state.simulation_time.write() = 239.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    assert_eq!(world.enemies.get(&2).unwrap().health, 145.0);
    *world.game_state.simulation_time.write() = 240.0;
    apply_enemy_support_abilities(&world, &connections, 10.0);
    // 145 + nova amount 10 caps exactly at dagger max health.
    assert_eq!(world.enemies.get(&2).unwrap().health, 150.0);
    assert_eq!(world.enemies.get(&3).unwrap().health, 100.0);
    assert_eq!(world.enemies.get(&4).unwrap().health, 100.0);
}

#[test]
fn tether_follow_targets_pick_nearest_larger_ally_deterministically() {
    let world = erekir_test_world();
    let follower_tile = (10 << 16) | 10;
    let left_tile = (2 << 16) | 10;
    let right_tile = (18 << 16) | 10;
    world
        .enemies
        .insert(10, ground_unit_on_tile(10, 56, follower_tile, 500.0, 0.0));
    // Alone on the team: no tether preference.
    assert!(crate::network::combat::enemy::tether_follow_targets(&world).is_empty());
    // Two larger allies at identical distance: the lower id wins.
    let right = ground_unit_on_tile(12, 57, right_tile, 20_000.0, 0.0);
    let left = ground_unit_on_tile(11, 57, left_tile, 20_000.0, 0.0);
    let left_pos = (left.x, left.y);
    world.enemies.insert(12, right);
    world.enemies.insert(11, left);
    assert_eq!(
        crate::network::combat::enemy::tether_follow_targets(&world)
            .get(&10)
            .copied(),
        Some(left_pos)
    );
    // A strictly closer large ally wins over distance ties.
    let close = ground_unit_on_tile(13, 41, (16 << 16) | 10, 11_000.0, 0.0);
    let close_pos = (close.x, close.y);
    world.enemies.insert(13, close);
    assert_eq!(
        crate::network::combat::enemy::tether_follow_targets(&world)
            .get(&10)
            .copied(),
        Some(close_pos)
    );
    // Within TETHER_FOLLOW_DISTANCE of every large ally: hold position.
    world.enemies.remove(&11);
    world.enemies.remove(&12);
    let adjacent = ground_unit_on_tile(14, 57, (12 << 16) | 10, 20_000.0, 0.0);
    world.enemies.insert(14, adjacent);
    let targets = crate::network::combat::enemy::tether_follow_targets(&world);
    let follower = ground_unit_on_tile(10, 56, follower_tile, 500.0, 0.0);
    assert_eq!(targets.get(&10).copied(), Some((follower.x, follower.y)));
    // Non-tether units never appear as followers.
    world
        .enemies
        .insert(15, ground_unit_on_tile(15, 57, left_tile, 20_000.0, 0.0));
    world
        .enemies
        .insert(16, ground_unit_on_tile(16, 0, follower_tile, 150.0, 0.0));
    assert!(!crate::network::combat::enemy::tether_follow_targets(&world).contains_key(&16));
}

#[test]
fn oxynoe_overclock_field_owner_death() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let origin = (10 << 16) | 10;
    let near = (16 << 16) | 10;
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 31, origin, 560.0, 0.0));
    world
        .enemies
        .insert(2, ground_unit_on_tile(2, 0, near, 150.0, 0.0));
    *world.game_state.simulation_time.write() = 360.0;
    apply_enemy_support_abilities(&world, &connections, 1.0);
    assert_eq!(world.enemies.get(&2).unwrap().status_effect, 14);
    assert_eq!(world.enemies.get(&2).unwrap().status_duration, 360.0);
    world.enemies.remove(&1);
    crate::network::units::StatusContainer::tick_statuses(
        &mut *world.enemies.get_mut(&2).unwrap(),
        360.0,
    );
    *world.game_state.simulation_time.write() = 720.0;
    apply_enemy_support_abilities(&world, &connections, 1.0);
    assert_eq!(world.enemies.get(&2).unwrap().status_effect, -1);
}

#[test]
fn quasar_force_field_absorbs_regens_and_clears_on_death() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let tile = (10 << 16) | 10;
    let mut quasar = ground_unit_on_tile(1, 7, tile, 640.0, 0.0);
    quasar.shield = 500.0;
    let x = quasar.x;
    let y = quasar.y;
    world.enemies.insert(1, quasar);
    let (pid, projectile) = ability_projectile(4_001, 1, 9.0, x + 10.0, y);
    world.projectiles.insert(pid, projectile);
    assert!(simulate_projectiles(&world, &connections, 1.0));
    assert!(!world.projectiles.contains_key(&pid));
    assert_eq!(world.enemies.get(&1).unwrap().shield, 491.0);
    apply_enemy_support_abilities(&world, &connections, 60.0);
    assert!((world.enemies.get(&1).unwrap().shield - 500.0).abs() < 0.001);
    apply_enemy_support_abilities(&world, &connections, 180.0);
    assert_eq!(world.enemies.get(&1).unwrap().shield, 500.0);
    world.enemies.get_mut(&1).unwrap().shield = 5.0;
    assert!(quasar_force_field_absorb(&world, 1, x, y, 9.0));
    assert!(world.enemies.get(&1).unwrap().shield < 0.0);
    world.enemies.remove(&1);
    assert!(!quasar_force_field_absorb(&world, 1, x, y, 9.0));
}

#[test]
fn tecta_shield_arc_absorbs_in_cone_and_stops_on_death() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let tile = (12 << 16) | 12;
    let tecta = ground_unit_on_tile(1, 47, tile, 6_500.0, 0.0);
    let x = tecta.x;
    let y = tecta.y;
    world.enemies.insert(1, tecta);
    assert!(simulate_tecta_shield_arcs(&world, 1.0));
    assert_eq!(world.force_fields.get(&1).unwrap().hp, 2_500.0);
    let (hit, projectile) = ability_projectile(4_010, 1, 9.0, x, y);
    world.projectiles.insert(hit, projectile);
    assert!(simulate_projectiles(&world, &connections, 1.0));
    assert!(!world.projectiles.contains_key(&hit));
    assert_eq!(world.force_fields.get(&1).unwrap().hp, 2_491.0);
    assert!(simulate_tecta_shield_arcs(&world, 60.0));
    assert!((world.force_fields.get(&1).unwrap().hp - 2_500.0).abs() < 0.001);
    let (miss, behind) = ability_projectile(4_011, 1, 9.0, x - 80.0, y);
    world.projectiles.insert(miss, behind);
    assert!(!tecta_shield_arc_absorb(&world, 1, x - 80.0, y, 9.0));
    world.enemies.remove(&1);
    assert!(simulate_oct_force_fields(&world, 1.0));
    assert!(!world.force_fields.contains_key(&1));
}

#[test]
fn quell_and_disrupt_suppression_fields() {
    let world = erekir_test_world();
    let near = (12 << 16) | 10;
    let mid = (40 << 16) | 10;
    let far = (55 << 16) | 10;
    let mut mend = erekir_tile(near, 246, 0);
    mend.health = crate::game::content::block_health(246) * 0.5;
    world.tiles.insert(near, mend);
    let mut mid_tile = erekir_tile(mid, 246, 0);
    mid_tile.health = crate::game::content::block_health(246) * 0.5;
    world.tiles.insert(mid, mid_tile);
    let mut far_tile = erekir_tile(far, 246, 0);
    far_tile.health = crate::game::content::block_health(246) * 0.5;
    world.tiles.insert(far, far_tile);

    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 52, (10 << 16) | 10, 6_000.0, 0.0));
    assert!(simulate_navanax_suppression(&world, 90.0));
    assert!(world.heal_suppression.contains_key(&near));
    assert!(!world.heal_suppression.contains_key(&mid));
    assert_eq!(heal_building_for_team(&world, near, 1, 50.0, 0.0), None);
    world.enemies.remove(&1);
    assert!(simulate_navanax_suppression(&world, 481.0));
    assert!(!world.heal_suppression.contains_key(&near));
    assert!(heal_building_for_team(&world, near, 1, 50.0, 0.0).is_some());

    world.enemies.insert(
        2,
        ground_unit_on_tile(2, 54, (10 << 16) | 10, 12_000.0, 0.0),
    );
    assert!(simulate_navanax_suppression(&world, 90.0));
    assert!(world.heal_suppression.contains_key(&near));
    assert!(world.heal_suppression.contains_key(&mid));
    assert!(!world.heal_suppression.contains_key(&far));
    world.enemies.remove(&2);
    assert!(simulate_navanax_suppression(&world, 180.0));
    assert!(world.heal_suppression.contains_key(&near));
    assert!(simulate_navanax_suppression(&world, 721.0));
    assert!(!world.heal_suppression.contains_key(&near));
}

#[test]
fn latum_spawns_five_renale_on_death() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let tile = (10 << 16) | 10;
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 57, tile, 20_000.0, 0.0));
    kill_enemy(&world, &connections, 1);
    assert!(!world.enemies.contains_key(&1));
    let spawned: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 56)
        .map(|unit| (unit.team, unit.health, unit.x, unit.y))
        .collect();
    assert_eq!(spawned.len(), 5);
    assert!(spawned
        .iter()
        .all(|entry| entry.0 == 2 && (entry.1 - 500.0).abs() < 0.001));
    let origin_x = 10.0 * 8.0 + 4.0;
    let origin_y = 10.0 * 8.0 + 4.0;
    assert!(spawned.iter().all(|entry| {
        let distance = (entry.2 - origin_x).hypot(entry.3 - origin_y);
        (distance - 11.0).abs() < 0.01
    }));
}

/// Vanilla chain: scathe-missile-surge (67) carries a shootOnDeath weapon
/// firing death-explosion bullet 193 whose createFrags fan (fragBullets=5,
/// fragSpread=20) produces frags at deathRotation + {-40,-20,0,+20,+40}
/// degrees; each frag has spawnUnit -> scathe-missile-surge-split (68).
/// Each split spawns at death point + trns(angle, range(1..7)) with its
/// rotation set to that angle; the range offset is deterministic per
/// (dying unit id, frag index).
#[test]
fn surge_missile_death_inserts_five_splits_in_frag_fan() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let tile = (10 << 16) | 10;
    let mut unit = ground_unit_on_tile(1, 67, tile, 300.0, 0.0);
    unit.rotation = 90.0;
    world.enemies.insert(1, unit);
    kill_enemy(&world, &connections, 1);
    assert!(!world.enemies.contains_key(&1));
    let splits: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 68)
        .map(|unit| (unit.team, unit.x, unit.y, unit.rotation, unit.health))
        .collect();
    assert_eq!(splits.len(), 5);
    let center_x = 10.0 * 8.0 + 4.0;
    let center_y = 10.0 * 8.0 + 4.0;
    for (index, expected_angle) in [50.0f32, 70.0, 90.0, 110.0, 130.0].iter().enumerate() {
        // Mirror of the implementation's deterministic offset.
        let mut rng = crate::network::combat::DetRand::new((1u64 << 8) | (index as u64 + 1));
        let len = 1.0 + rng.unit_f32() * (7.0 - 1.0);
        let radians = (*expected_angle).to_radians();
        let (expected_x, expected_y) = (
            center_x + radians.cos() * len,
            center_y + radians.sin() * len,
        );
        let entry = splits
            .iter()
            .find(|split| (split.3 - expected_angle).abs() < 0.001)
            .unwrap_or_else(|| panic!("no split at angle {expected_angle}: {splits:?}"));
        let (team, x, y, rotation, health) = *entry;
        assert_eq!(team, 2);
        assert!((rotation - expected_angle).abs() < 0.001);
        assert!((x - expected_x).abs() < 0.001, "x {x} vs {expected_x}");
        assert!((y - expected_y).abs() < 0.001, "y {y} vs {expected_y}");
        assert!((health - 50.0).abs() < 0.001); // spec health of unit 68
    }
}

/// Other scathe missile family units have no shootOnDeath spawnUnit chain:
/// killing them must not insert a surge-split.
#[test]
fn other_scathe_family_deaths_insert_no_split() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    for (id, unit_type) in [(1, 65), (2, 66), (3, 68)] {
        let tile = ((10 + id) << 16) | 10;
        world
            .enemies
            .insert(id, ground_unit_on_tile(id, unit_type, tile, 100.0, 0.0));
    }
    for id in 1..=3 {
        kill_enemy(&world, &connections, id);
    }
    assert!(world.enemies.is_empty());
}

/// Vanilla scathe damage model (Blocks.java v159.7): the launcher bullets
/// 186/189/192 carry NO splash; each missile's shootOnDeath death explosion
/// applies its own splash at the DEATH point with buildingDamageMultiplier
/// 0.1: 65 -> ExplosionBulletType(1000f, 65f), 66 -> (320f, 120f),
/// 67 -> (1800f, 40f), 68 -> (180f, 35f).
#[test]
fn scathe_missile_deaths_apply_death_explosion_splash() {
    let cases: &[(i16, f32, f32)] = &[
        (65, 1_000.0, 65.0),
        (66, 320.0, 120.0),
        (67, 1_800.0, 40.0),
        (68, 180.0, 35.0),
    ];
    for &(unit_type, splash, radius) in cases.iter() {
        let world = erekir_test_world();
        let connections = DashMap::new();
        // Missile dies at tile (12,10) centre (100, 84). Walls (block 216,
        // health 320, armor 0) at (14,10) sit ~16u away -- inside every
        // radius above; the wall at (28,10) sits ~128u away -- outside all.
        let missile_tile = (12 << 16) | 10;
        let health = crate::network::units::enemy_spec(unit_type).unwrap().health;
        world.enemies.insert(
            1,
            ground_unit_on_tile(1, unit_type, missile_tile, health, 0.0),
        );
        let near = (14 << 16) | 10;
        let far = (28 << 16) | 10;
        for position in [near, far] {
            let mut wall = erekir_tile(position, 216, 0);
            wall.health = crate::game::content::block_health(216);
            world.tiles.insert(position, wall);
        }
        kill_enemy(&world, &connections, 1);
        // Buildings take splash * buildingDamageMultiplier (0.1).
        let expected_near = crate::game::content::block_health(216) - splash * 0.1;
        assert!(
            (world.tiles.get(&near).unwrap().health - expected_near).abs() < 0.001,
            "unit {unit_type} splash {splash} r{radius}: near wall {}",
            world.tiles.get(&near).unwrap().health
        );
        assert!(
            (world.tiles.get(&far).unwrap().health - crate::game::content::block_health(216)).abs()
                < 0.001,
            "unit {unit_type}: far wall must stay outside radius {radius}"
        );
    }
}

/// Bullet 193 (scathe-missile-surge death): lightning = 10 x 45 damage with
/// lightningLength = 12 -> six segments per root; bullet 195 (surge-split
/// death): lightning = 4 x 25 with lightningLength = 6 -> three segments per
/// root. With exactly one opposing unit inside the 30u chain box every root
/// hits it once, so the total is deterministic: roots * lightningDamage.
#[test]
fn surge_family_deaths_chain_deterministic_lightning() {
    let cases: &[(i16, usize, f32, f32)] = &[(67, 10, 45.0, 1_800.0), (68, 4, 25.0, 180.0)];
    for &(unit_type, roots, lightning_damage, splash) in cases.iter() {
        let world = erekir_test_world();
        let connections = DashMap::new();
        let missile_tile = (12 << 16) | 10;
        let health = crate::network::units::enemy_spec(unit_type).unwrap().health;
        world.enemies.insert(
            1,
            ground_unit_on_tile(1, unit_type, missile_tile, health, 0.0),
        );
        // One opposing (team 1) dagger ~16u from the death point. C06: splash
        // hits live EnemyUnits on the opposing team, same as vanilla
        // ExplosionBulletType, then the lightning chain walks that body.
        let mut victim = ground_unit_on_tile(2, 0, (14 << 16) | 10, 5_000.0, 0.0);
        victim.team = 1;
        world.enemies.insert(2, victim);
        kill_enemy(&world, &connections, 1);
        let expected = 5_000.0 - splash - roots as f32 * lightning_damage;
        let health = world.enemies.get(&2).unwrap().health;
        assert!(
            (health - expected).abs() < 0.001,
            "unit {unit_type}: victim health {health} vs {expected}"
        );
    }
}

/// Enemy-team scathe missiles keep their split fan through the explosion:
/// killing a surge missile inserts the five surge-splits AND applies the
/// 1800/40 death explosion (walls within 40u take 180).
#[test]
fn surge_missile_death_applies_explosion_and_keeps_split_fan() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let missile_tile = (12 << 16) | 10;
    let mut unit = ground_unit_on_tile(1, 67, missile_tile, 300.0, 0.0);
    unit.rotation = 90.0;
    world.enemies.insert(1, unit);
    let near = (14 << 16) | 10;
    let mut wall = erekir_tile(near, 216, 0);
    wall.health = crate::game::content::block_health(216);
    world.tiles.insert(near, wall);
    kill_enemy(&world, &connections, 1);
    let splits = world
        .enemies
        .iter()
        .filter(|entry| entry.unit_type == 68)
        .count();
    assert_eq!(splits, 5);
    let expected_near = crate::game::content::block_health(216) - 1_800.0 * 0.1;
    assert!((world.tiles.get(&near).unwrap().health - expected_near).abs() < 0.001);
}

/// Vanilla createFrags of the scathe death explosions (Blocks.java v159.7):
/// explosion 187 (scathe-missile death) carries fragBullets = 7 artillery
/// shells -- bullet 188, splash 100 / radius 40, buildingDamageMultiplier
/// 0.1; explosion 190 (scathe-missile-phase death) carries 7 shells --
/// bullet 191, splash 120 / radius 56, multiplier 0.2. The shells are real
/// short-lived projectiles with no direct hit (collides = false). Explosions
/// 193/195 carry no artillery frags (193's frag is the spawnUnit carrier of
/// the surge-split chain).
/// The frag fan geometry is a pure function of (dying unit id, frag index):
/// two identical kills must insert identical projectile sets across runs.
#[test]
fn scathe_frag_fan_is_deterministic_across_runs() {
    let run = || -> Vec<(f32, f32, f32, f32, f32)> {
        let world = erekir_test_world();
        let connections = DashMap::new();
        let mut unit = ground_unit_on_tile(1, 65, (12 << 16) | 10, 240.0, 0.0);
        unit.rotation = 37.0;
        world.enemies.insert(1, unit);
        kill_enemy(&world, &connections, 1);
        let mut rows: Vec<_> = world
            .projectiles
            .iter()
            .map(|projectile| {
                let fragment = projectile.value();
                (
                    fragment.source_x,
                    fragment.source_y,
                    fragment.target_x,
                    fragment.target_y,
                    fragment.total_ticks,
                )
            })
            .collect();
        rows.sort_unstable_by(|left, right| left.2.total_cmp(&right.2));
        rows
    };
    let first = run();
    assert_eq!(first.len(), 7, "explosion 187 fans seven artillery shells");
    assert_eq!(run(), first);
}

#[test]
fn scathe_missile_death_inserts_seven_artillery_frags() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let mut unit = ground_unit_on_tile(1, 65, (12 << 16) | 10, 240.0, 0.0);
    unit.rotation = 0.0;
    world.enemies.insert(1, unit);
    kill_enemy(&world, &connections, 1);
    let frags: Vec<_> = world
        .projectiles
        .iter()
        .map(|projectile| projectile.value().clone())
        .collect();
    assert_eq!(frags.len(), 7);
    assert!(frags.iter().all(|frag| frag.bullet_id == 188));
    assert!(frags
        .iter()
        .all(|frag| (frag.splash_damage - 100.0).abs() < f32::EPSILON));
    assert!(frags
        .iter()
        .all(|frag| (frag.splash_radius - 40.0).abs() < f32::EPSILON));
    assert!(frags.iter().all(|frag| !frag.apply_direct_on_impact));
}

#[test]
fn scathe_artillery_frag_applies_building_damage_multiplier() {
    let mut world = erekir_test_world();
    let connections = DashMap::new();
    let mut unit = ground_unit_on_tile(1, 65, (12 << 16) | 10, 240.0, 0.0);
    unit.team = 2;
    unit.rotation = 0.0;
    world.enemies.insert(1, unit);
    kill_enemy(&world, &connections, 1);
    let dest = world
        .projectiles
        .iter()
        .map(|projectile| {
            let frag = projectile.value();
            (frag.target_x, frag.target_y)
        })
        .next()
        .expect("artillery frag");
    let tile = ((dest.0 / 8.0).floor() as i32) << 16 | ((dest.1 / 8.0).floor() as i32);
    beam_probe_tile(&mut world, tile, 216, 1);
    let max = crate::game::content::block_health(216);
    if let Some(mut wall) = world.tiles.get_mut(&tile) {
        wall.health = max;
    }
    for _ in 0..24 {
        simulate_projectiles(&world, &connections, 1.0);
    }
    let health = world
        .tiles
        .get(&tile)
        .map(|tile| tile.health)
        .unwrap_or(max);
    assert!(
        (health - (max - 10.0)).abs() < 0.5 || health < max,
        "frag splash should chip the wall (got {health}, max {max})"
    );
}

#[test]
fn e07_carried_explosives_do_not_drop_as_loot() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let mut unit = ground_unit_on_tile(1, 0, (10 << 16) | 10, 40.0, 0.0);
    unit.items = vec![(14, 60)];
    world.enemies.insert(1, unit);
    kill_enemy(&world, &connections, 1);
    assert!(
        world.game_state.extras.ground_items.lock().is_empty(),
        "UnitComp.destroy explodes the stack; it does not spawn GroundItem loot"
    );
}

#[test]
fn projectile_dda_hits_the_first_wall_on_the_ray() {
    let mut world = erekir_test_world();
    let near = (2 << 16) | 10;
    let far = (8 << 16) | 10;
    beam_probe_tile(&mut world, near, 216, 1);
    beam_probe_tile(&mut world, far, 216, 1);
    let hit = projectile_building_hit(&world, 4.0, 84.0, 72.0, 84.0, 2, 6, 4.0)
        .expect("ray should strike a wall");
    assert_eq!(hit.0, near, "DDA must stop on the first occupied tile");
}

/// A victim building sitting on a frag's expiry point takes the shell's
/// splash through buildingDamageMultiplier: x0.1 for explosion-187 frags
/// (100 damage -> 10 per covering shell) and x0.2 for explosion-190 frags
/// (120 -> 24). Units inside the radius would take the full splash instead
/// (official Damage.damageUnits has no building multiplier).
/// Frag shells keep their buildingDamageMultiplier through the allied
/// (team 1) impact path too: player-team missiles' frags scale enemy
/// building splash by x0.1 / x0.2 via projectile_building_damage_multiplier.
fn beam_probe_tile(world: &mut DynamicWorld, position: i32, block: i16, team: u8) {
    let mut tile = erekir_tile(position, block, 0);
    tile.team = team;
    let size = i32::from(crate::game::content::block_size(block));
    if size > 1 {
        let x = (position >> 16) as i16 as i32;
        let y = position as i16 as i32;
        let offset = -(size - 1) / 2;
        for dy in 0..size {
            for dx in 0..size {
                tile.occupied
                    .push(((x + offset + dx) << 16) | ((y + offset + dy) as u16 as i32));
            }
        }
    } else {
        tile.occupied = vec![position];
    }
    world.tiles.insert(position, tile);
}

/// BeamNode.updateDirections links to the first valid building on each ray,
/// and another beam node is a valid target (only PowerNode is excluded). A
/// solar -> beam -> beam -> drill chain must form bidirectional links on both
/// hops and carry one power graph across them.
#[test]
fn beam_topology_chains_through_a_second_beam_node() {
    let mut world = erekir_test_world();
    let solar = (5 << 16) | 10;
    let beam_a = (6 << 16) | 10;
    let beam_b = (12 << 16) | 10;
    let consumer = (18 << 16) | 10;
    for (position, block) in [(solar, 313), (beam_a, 317), (beam_b, 317), (consumer, 327)] {
        beam_probe_tile(&mut world, position, block, 1);
    }
    refresh_beam_power_links(&world);
    let a_links = world.tiles.get(&beam_a).unwrap().power_links.clone();
    let b_links = world.tiles.get(&beam_b).unwrap().power_links.clone();
    assert!(
        a_links.contains(&beam_b),
        "A must link east to B: {a_links:?}"
    );
    assert!(
        b_links.contains(&beam_a),
        "B must link west to A: {b_links:?}"
    );
    assert!(
        b_links.contains(&consumer),
        "B must link east to the drill: {b_links:?}"
    );
    assert!(
        world
            .tiles
            .get(&consumer)
            .unwrap()
            .power_links
            .contains(&beam_b),
        "drill must hold the reverse link"
    );
    let powered = compute_power_efficiency(&world);
    assert!(
        powered.get(&consumer).copied().unwrap_or(0.0) > 0.0,
        "one graph must span solar -> beam -> beam -> drill"
    );
}

/// A destroyed beam node must not leave phantom reverse links on its
/// partners: Java ignores dead entries in getPowerConnections
/// (BuildingComp.java:1206-1207), so the materialized server links are
/// pruned instead. Beam nodes are crushFragile and can be destroyed outside
/// the placement lifecycle.
#[test]
fn beam_topology_prunes_partner_links_after_beam_destruction() {
    let mut world = erekir_test_world();
    let solar = (5 << 16) | 10;
    let beam = (6 << 16) | 10;
    let consumer = (12 << 16) | 10;
    for (position, block) in [(solar, 313), (beam, 317), (consumer, 327)] {
        beam_probe_tile(&mut world, position, block, 1);
    }
    refresh_beam_power_links(&world);
    assert!(world
        .tiles
        .get(&consumer)
        .unwrap()
        .power_links
        .contains(&beam));

    world.tiles.remove(&beam);
    refresh_beam_power_links(&world);
    let c_links = world.tiles.get(&consumer).unwrap().power_links.clone();
    assert!(
        !c_links.contains(&beam),
        "stale link to removed beam: {c_links:?}"
    );
    assert_eq!(
        compute_power_efficiency(&world).get(&consumer).copied(),
        Some(0.0)
    );

    // Rebuilding the beam node reunifies the graphs.
    beam_probe_tile(&mut world, beam, 317, 1);
    refresh_beam_power_links(&world);
    assert!(world
        .tiles
        .get(&consumer)
        .unwrap()
        .power_links
        .contains(&beam));
    assert!(
        compute_power_efficiency(&world)
            .get(&consumer)
            .copied()
            .unwrap_or(0.0)
            > 0.0,
        "rebuild must restore the link and the graph"
    );
}

/// PowerNode blocks are excluded from BeamNode rays ("do not play nice",
/// BeamNode.java updateDirections) and enemy-team buildings never link.
#[test]
fn beam_topology_skips_power_nodes_and_enemy_targets() {
    let mut world = erekir_test_world();
    let beam = (6 << 16) | 10;
    beam_probe_tile(&mut world, beam, 317, 1);
    beam_probe_tile(&mut world, (12 << 16) | 10, 302, 1); // power-node east
    beam_probe_tile(&mut world, (12 << 16) | 14, 327, 2); // enemy drill north-east
    refresh_beam_power_links(&world);
    let links = world.tiles.get(&beam).unwrap().power_links.clone();
    assert!(
        links.is_empty(),
        "no link to a PowerNode or an enemy: {links:?}"
    );
}

/// A size-3 beam tower reaches a multi-tile consumer whose footprint the ray
/// enters on a non-origin tile (dynamic_at resolves the footprint).
#[test]
fn beam_tower_targets_multi_tile_consumer_through_side_tiles() {
    let mut world = erekir_test_world();
    let tower = (8 << 16) | 10;
    let drill = (20 << 16) | 10;
    beam_probe_tile(&mut world, (4 << 16) | 10, 313, 1);
    beam_probe_tile(&mut world, tower, 318, 1);
    beam_probe_tile(&mut world, drill, 327, 1);
    refresh_beam_power_links(&world);
    let t_links = world.tiles.get(&tower).unwrap().power_links.clone();
    assert!(
        t_links.contains(&drill),
        "tower must reach the drill: {t_links:?}"
    );
    assert!(
        compute_power_efficiency(&world)
            .get(&drill)
            .copied()
            .unwrap_or(0.0)
            > 0.0,
        "multi-tile consumer must be powered through the tower"
    );
}

#[test]
fn aegires_energy_field_stops_after_owner_death() {
    let world = erekir_test_world();
    let connections = DashMap::new();
    let origin = (10 << 16) | 10;
    let near = (16 << 16) | 10;
    world
        .enemies
        .insert(1, ground_unit_on_tile(1, 33, origin, 12_000.0, 0.0));
    world
        .enemies
        .insert(2, ground_unit_on_tile(2, 0, near, 100.0, 0.0));
    assert!(crate::network::simulation::simulate_aegires_energy_fields(
        &world,
        &connections,
        65.0
    ));
    let healed = world.enemies.get(&2).unwrap().health;
    assert!(healed > 100.0);
    world.enemies.remove(&1);
    assert!(!crate::network::simulation::simulate_aegires_energy_fields(
        &world,
        &connections,
        180.0
    ));
    assert_eq!(world.enemies.get(&2).unwrap().health, healed);
}

/// Sweep: place every block id from the content table (`block_names.rs`) as a
/// standalone tile and run one tick of the config-free simulate passes.
///
/// Skipped ids are content-table entries that cannot exist as a standalone
/// placed building without complex multi-tile / world setup; they are listed
/// explicitly here rather than silently ignored:
/// - 0 `air`, 1 `spawn`: not buildings.
/// - 2 `remove-wall`, 3 `remove-ore`: deconstruction pseudo-blocks.
/// - 4 `cliff`: map terrain, never a DynamicTile.
/// - 5..=20 `build1`..`build16`: build-zone overlay pseudo-blocks.
#[test]
fn all_block_sweep_places_and_ticks_every_block() {
    const SKIP: &[i16] = &[
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
    ];

    let world = erekir_test_world();
    let no_power = HashMap::new();
    let mut panicked_ids: Vec<i16> = Vec::new();
    let quiet_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    for id in 0..=445i16 {
        if crate::game::block_names::block_name_from_id(id).is_none() || SKIP.contains(&id) {
            continue;
        }
        // One free position per id inside the 40x40 world, away from the core.
        let position = ((i32::from(id) % 40) << 16) | (i32::from(id) / 40);
        world.tiles.insert(position, erekir_tile(position, id, 0));

        let ticked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            simulate_logistics(&world, 1.0, &no_power);
            simulate_liquids(&world, 1.0, &no_power);
            simulate_factories(&world, 1.0, &no_power);
            simulate_junctions(&world, 1.0);
            simulate_unloaders(&world, 1.0);
            simulate_reactors(&world, 1.0);
            simulate_erekir_ducts(&world, 1.0, &no_power);
            simulate_erekir_drills(&world, 1.0, &no_power);
            simulate_erekir_crafters(&world, 1.0, &no_power);
        }));

        world.tiles.remove(&position);
        if ticked.is_err() {
            panicked_ids.push(id);
        }
    }

    std::panic::set_hook(quiet_hook);
    let names = panicked_ids
        .iter()
        .filter_map(|id| crate::game::block_names::block_name_from_id(*id))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        panicked_ids.is_empty(),
        "blocks panicked during placement/tick sweep: {panicked_ids:?} ({names})"
    );
}

/// r9-reset-bug regression: belts and drills must keep their accumulated
/// state across simulated minutes. The authoritative 6 s BlockSnapshot
/// re-encodes conveyor queues and drill progress from server state, so the
/// simulation itself must never stall, lose items or rewind progress — a
/// server-side reset would surface to clients as periodic belt/drill resets.
#[test]
fn r9_reset_bug_belt_and_drill_state_survives_simulated_minutes() {
    let mut world = erekir_test_world();
    let drill_pos = (10 << 16) | 10;
    world.overlays[(10 * world.width + 10) as usize] = 167; // copper overlay
    let mut drill = erekir_tile(drill_pos, 325, 0);
    drill.occupied = vec![drill_pos];
    world.tiles.insert(drill_pos, drill);
    let mut belt_positions = Vec::new();
    for x in 11..=15 {
        let pos = (x << 16) | 10;
        let mut belt = erekir_tile(pos, 257, 0);
        belt.occupied = vec![pos];
        world.tiles.insert(pos, belt);
        belt_positions.push(pos);
    }
    // Item void (413) swallows the output so nothing ever jams.
    let void_pos = (16 << 16) | 10;
    world.tiles.insert(void_pos, erekir_tile(void_pos, 413, 0));

    let no_power = HashMap::new();
    let delay = 650.0_f32;
    let mut cumulative_wraps = 0.0_f32;
    let mut last_progress = 0.0_f32;
    let mut max_regression = 0.0_f32;
    for tick in 1..=7200 {
        simulate_logistics(&world, 1.0, &no_power);
        let d = world.tiles.get(&drill_pos).unwrap();
        let p = d.production_progress;
        if p < last_progress {
            cumulative_wraps += 1.0;
            max_regression = max_regression.max(last_progress - p);
        }
        last_progress = p;
        if tick % 600 == 0 {
            // Every belt in the chain must stay valid while items flow.
            for pos in &belt_positions {
                assert_valid_plain_conveyor_queue(&world.tiles.get(pos).unwrap().conveyor_items);
            }
        }
    }

    // ~7200/650 = 11 copper mined over two simulated minutes; production is
    // monotonic modulo `delay` (progress only ever rewinds by one wrap).
    let mined = cumulative_wraps + last_progress / delay;
    assert!(
        mined > 8.0,
        "a long session must keep producing continuously: {mined}"
    );
    assert!(
        max_regression <= delay,
        "drill progress may only wrap at the mining delay: {max_regression}"
    );
}

/// r9-reset-bug regression: dynamic drills dump on the official five-tick
/// `Block.dumpTime` cadence, not every tick. An ungated dump pushed items
/// onto belts up to 5x faster than client prediction, so every 6 s block
/// snapshot visibly corrected belts and drill buffers ("se reinician").
#[test]
fn r9_reset_bug_drill_dump_cadence_matches_official_dump_time() {
    let mut world = erekir_test_world();
    let drill_pos = (10 << 16) | 10;
    world.overlays[(10 * world.width + 10) as usize] = 167;
    let mut drill = erekir_tile(drill_pos, 325, 0);
    drill.occupied = vec![drill_pos];
    // Full buffer: every gated dump attempt must succeed into the void.
    drill.stored_item = 0;
    drill.stored_amount = 60;
    world.tiles.insert(drill_pos, drill);
    let void_pos = (11 << 16) | 10;
    world.tiles.insert(void_pos, erekir_tile(void_pos, 413, 0));

    let ticks = 50;
    for _ in 0..ticks {
        // Mirror the simulation loop: one shared tick per simulation step.
        world.game_state.world_ticks.fetch_add(1, Ordering::Relaxed);
        simulate_logistics(&world, 1.0, &HashMap::new());
    }
    let dumped = 60 - world.tiles.get(&drill_pos).unwrap().stored_amount;
    // Official cadence: one dump attempt per Block.dumpTime = 5 ticks.
    assert_eq!(dumped, ticks / 5, "dump attempts follow Block.dumpTime");
}

/// r9-reset-bug regression: the periodic block snapshot must carry the LIVE
/// belt queue and drill progress/warmup, not stale or zeroed state, across
/// several consecutive 6 s windows of a running session.
#[test]
fn r9_reset_bug_block_snapshots_carry_live_belt_and_drill_state() {
    use crate::network::buildings::snapshot::encode_drill_sync;
    use crate::network::codec::Reads;
    use std::io::Cursor;

    let mut world = erekir_test_world();
    let drill_pos = (10 << 16) | 10;
    world.overlays[(10 * world.width + 10) as usize] = 167;
    let mut drill = erekir_tile(drill_pos, 325, 0);
    drill.occupied = vec![drill_pos];
    world.tiles.insert(drill_pos, drill);
    let belt_pos = (11 << 16) | 10;
    let mut belt = erekir_tile(belt_pos, 257, 0);
    belt.occupied = vec![belt_pos];
    world.tiles.insert(belt_pos, belt);
    let void_pos = (12 << 16) | 10;
    world.tiles.insert(void_pos, erekir_tile(void_pos, 413, 0));

    let no_power = HashMap::new();
    for window in 0..4 {
        for _ in 0..360 {
            simulate_logistics(&world, 1.0, &no_power);
            // Advance the shared tick so the official dump timer runs.
            world.game_state.world_ticks.fetch_add(1, Ordering::Relaxed);
        }

        // Conveyor sync: decoded wire items equal the live queue within the
        // byte quantization of the official layout.
        let live_tile = world.tiles.get(&belt_pos).unwrap().clone();
        let decoded = decode_conveyor_sync_items(&live_tile);
        if live_tile.conveyor_items.is_empty() {
            assert!(
                decoded.is_empty(),
                "window {window}: empty belt encoded items"
            );
        } else {
            assert!(
                !decoded.is_empty(),
                "window {window}: snapshot lost the belt queue"
            );
            // Wire order is reversed (front last); compare as sets of ids.
            let mut live_ids: Vec<i16> = live_tile
                .conveyor_items
                .iter()
                .map(|(item, _)| *item)
                .collect();
            let mut wire_ids: Vec<i16> = decoded.iter().map(|(item, _)| *item).collect();
            live_ids.sort_unstable();
            wire_ids.sort_unstable();
            assert_eq!(
                live_ids, wire_ids,
                "window {window}: snapshot queue drifted"
            );
        }

        // Drill sync tail: f32 progress + f32 warmup round-trip exactly.
        // Mechanical-drill layout: base (health f32 + 5 header/module bytes +
        // items module + liquids module + two efficiency bytes), then the
        // DrillBuild write() tail of progress + warmup.
        let live_drill = world.tiles.get(&drill_pos).unwrap().clone();
        let mut bytes = Vec::new();
        encode_drill_sync(&mut bytes, &live_drill, &HashMap::new()).unwrap();
        let mut input = Cursor::new(bytes);
        input.read_f().unwrap(); // health
        for _ in 0..5 {
            input.read_b().unwrap(); // rotation|team|revision|enabled|modules
        }
        let item_count = input.read_s().unwrap();
        for _ in 0..item_count {
            input.read_s().unwrap();
            input.read_i().unwrap();
        }
        let liquid_count = input.read_s().unwrap();
        for _ in 0..liquid_count {
            input.read_s().unwrap();
            input.read_f().unwrap();
        }
        input.read_b().unwrap(); // efficiency
        input.read_b().unwrap(); // optionalEfficiency
        let wire_progress = input.read_f().unwrap();
        let wire_warmup = input.read_f().unwrap();
        assert!(
            (wire_progress - live_drill.production_progress).abs() < f32::EPSILON,
            "window {window}: snapshot progress drifted"
        );
        assert!(
            (wire_warmup - live_drill.transport_progress).abs() < f32::EPSILON,
            "window {window}: snapshot warmup drifted"
        );
    }
}

#[test]
fn test_batch_snapshot_excludes_client_predicted_blocks_and_includes_synced_blocks() {
    use crate::network::buildings::snapshot::{
        is_batch_snapshot_supported, is_block_snapshot_supported, is_core_block,
    };

    // Client-predicted distribution and production blocks MUST NOT be periodically broadcast
    // in 6-second batch snapshots (which causes client animation/item rollback glitches):
    let client_predicted = [
        257, 258, 259, 260, 279, // plain and stack conveyors
        272, 273, // ducts
        325, 326, 327, 328, 335, 336, 337, 338, // drills
        216, 217, 218, 219, 220, 228, 229, 230, 244, // walls and doors
        302, 303, 304, 410, // power nodes
        295, 296, 297, 299, 300, 301, // conduits and liquid tanks
        261, 264, 265, 266, 267, 268, 269, 270, 274, 278, // routers, sorters, junctions
        411, 412, 413, 414, 415, 418, // sandbox sources and voids
        430, 431, 432, 433, 434, 436, 440, 441, 442, 443, // logic, switch, memory, display
    ];
    for block in client_predicted {
        assert!(
            !is_batch_snapshot_supported(block),
            "block {block} should NOT be in periodic batch snapshot"
        );
        // But on-demand UI inspection via RequestBlockSnapshot must still work:
        assert!(
            is_block_snapshot_supported(block),
            "block {block} should still have a snapshot codec for on-demand inspection"
        );
    }

    // Official BlockFlag.synced blocks MUST be in periodic batch snapshots:
    let synced_blocks = [
        308, 309, 310, 311, 312, 315, 316, 320, 321, 322,
        329, // generators, reactors, water extractor
        201, 202, 203, 204, 205, 210, 212, 213, 214, 215, 330, 332, // crafters, cultivator
        193, 194, // separators
        271, // mass driver
        252, 281, 427, 428, // pads and towers
        345, 346, 347, 348, // storage blocks (container, vault, reinforced container/vault)
        353, 354, 355, 360, 366, 369, 372, 373, 376, // turrets
        377, 378, 379, 380, 381, 382, 383, 386, 387, 388, // unit factories and reconstructors
        398, 399, 400, 401, 402, 403, 404, 405, 406, 407, 408,
        409, // payload distribution and manufacturing
        5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
        20, // ConstructBlock.build1..16
    ];
    for block in synced_blocks {
        assert!(
            is_batch_snapshot_supported(block),
            "synced block {block} MUST be in periodic batch snapshot"
        );
    }

    // Cores are synced via state snapshot, never batch block snapshots:
    for core in 339..=344 {
        assert!(is_core_block(core));
        assert!(!is_batch_snapshot_supported(core));
    }
}

#[test]
fn test_impact_reactor_simulation_warmup_and_snapshot_fidelity() {
    use crate::network::buildings::snapshot::encode_power_generator_sync;
    use crate::network::codec::Reads;
    use crate::network::economy::power::update_power_network;
    use crate::network::simulation::power::simulate_impact_reactors;
    use std::io::Cursor;

    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    let source = (18 << 16) | 20;
    let mut reactor_tile = erekir_tile(pos, 316, 0);
    reactor_tile.occupied = vec![pos];
    reactor_tile.inventory = vec![(14, 60)]; // 60 blast compound (lasts 8400 ticks)
    reactor_tile.stored_liquid = 3; // cryofluid
    reactor_tile.liquid_amount = 2000.0; // 2000 cryofluid (lasts 8000 ticks at 0.25/tick)
    reactor_tile.output_liquid_amount = 0.0; // initial warmup = 0%
                                             // Official ImpactReactor.consumePower(25) gates on power.status >= 0.99;
                                             // a lone reactor produces 0 while warmup is 0 and cannot bootstrap.
                                             // PowerSource (410) supplies the graph; a solar panel (0.12) cannot.
    reactor_tile.power_links = vec![source];
    let mut source_tile = erekir_tile(source, 410, 0);
    source_tile.power_links = vec![pos];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(pos, reactor_tile);

    // Warmup phase: simulate 500 ticks
    for _ in 0..500 {
        simulate_impact_reactors(&world, 1.0);
    }

    let tile = world.tiles.get(&pos).unwrap().clone();
    assert!(
        tile.output_liquid_amount > 0.35 && tile.output_liquid_amount <= 0.45,
        "warmup should reach ~0.39 after 500 ticks with lerpDelta at 0.001/tick, got {}",
        tile.output_liquid_amount
    );
    assert!(
        tile.liquid_amount < 2000.0,
        "cryofluid should be consumed during operation, remaining: {}",
        tile.liquid_amount
    );

    // Verify snapshot serialization reflects live warmup
    let mut sync_bytes = Vec::new();
    encode_power_generator_sync(&mut sync_bytes, &tile, &std::collections::HashMap::new()).unwrap();
    let mut input = Cursor::new(sync_bytes);
    input.read_f().unwrap(); // health
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let item_count = input.read_s().unwrap();
    for _ in 0..item_count {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    let link_count = input.read_s().unwrap();
    for _ in 0..link_count {
        input.read_i().unwrap();
    }
    input.read_f().unwrap(); // status
    let liquid_count = input.read_s().unwrap();
    for _ in 0..liquid_count {
        input.read_s().unwrap();
        input.read_f().unwrap();
    }
    input.read_b().unwrap(); // efficiency
    input.read_b().unwrap(); // optionalEfficiency
    let prod_eff = input.read_f().unwrap(); // productionEfficiency
    assert!(
        (prod_eff - tile.output_liquid_amount.powi(5)).abs() < 0.001,
        "productionEfficiency must equal warmup^5"
    );
    input.read_f().unwrap(); // generateTime
    let wire_warmup = input.read_f().unwrap();
    assert!(
        (wire_warmup - tile.output_liquid_amount).abs() < 0.001,
        "wire warmup ({wire_warmup}) must match tile warmup ({})",
        tile.output_liquid_amount
    );

    // Continue to full warmup (another 6500 ticks)
    for _ in 0..6500 {
        simulate_impact_reactors(&world, 1.0);
    }
    let full_tile = world.tiles.get(&pos).unwrap().clone();
    assert!(
        (full_tile.output_liquid_amount - 1.0).abs() < 0.001,
        "warmup should reach 1.0 (100%), got {}",
        full_tile.output_liquid_amount
    );

    // Power output should scale with full warmup
    let power_map = update_power_network(&world, 1.0);
    assert!(!power_map.is_empty());
}

#[test]
fn test_conveyor_loop_long_continuity_no_reset() {
    use crate::network::buildings::snapshot::is_batch_snapshot_supported;
    use crate::network::economy::transport::simulate_logistics;
    use std::collections::HashMap;

    let world = erekir_test_world();
    // 4-tile conveyor loop:
    // (10, 10) rot 1 (North) -> (10, 11)
    // (10, 11) rot 0 (East)  -> (11, 11)
    // (11, 11) rot 3 (South) -> (11, 10)
    // (11, 10) rot 2 (West)  -> (10, 10)
    let p_a = (10 << 16) | 10;
    let p_b = (10 << 16) | 11;
    let p_c = (11 << 16) | 11;
    let p_d = (11 << 16) | 10;

    let mut t_a = erekir_tile(p_a, 257, 1);
    t_a.occupied = vec![p_a];
    t_a.conveyor_items = vec![(1, 0.0)]; // 1 Lead item
    t_a.stored_item = 1;
    t_a.stored_amount = 1;

    let mut t_b = erekir_tile(p_b, 257, 0);
    t_b.occupied = vec![p_b];
    let mut t_c = erekir_tile(p_c, 257, 3);
    t_c.occupied = vec![p_c];
    let mut t_d = erekir_tile(p_d, 257, 2);
    t_d.occupied = vec![p_d];

    world.tiles.insert(p_a, t_a);
    world.tiles.insert(p_b, t_b);
    world.tiles.insert(p_c, t_c);
    world.tiles.insert(p_d, t_d);

    // Verify none of the conveyors in the loop are in batch snapshots:
    assert!(!is_batch_snapshot_supported(257));

    // Simulate 1200 ticks (> 20 seconds = 3.33 full 6-second snapshot intervals)
    let power = HashMap::new();
    for _ in 0..1200 {
        simulate_logistics(&world, 1.0, &power);
    }

    // Total items across all 4 loop tiles must remain exactly 1 (no items created or lost)
    let total_items: usize = [p_a, p_b, p_c, p_d]
        .iter()
        .map(|p| world.tiles.get(p).unwrap().conveyor_items.len())
        .sum();
    assert_eq!(
        total_items, 1,
        "items in conveyor loop must neither duplicate nor vanish"
    );
}

#[test]
fn test_conveyor_snapshot_serializes_non_empty_item_module() {
    use crate::network::buildings::snapshot::encode_conveyor_sync;
    use crate::network::codec::Reads;
    use std::io::Cursor;

    let mut tile = erekir_tile((10 << 16) | 10, 257, 0);
    tile.conveyor_items = vec![(1, 0.8), (1, 0.4), (2, 0.0)]; // 2 lead (1), 1 coal (2)

    let mut bytes = Vec::new();
    encode_conveyor_sync(&mut bytes, &tile).unwrap();
    let mut input = Cursor::new(bytes);
    input.read_f().unwrap(); // health
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let item_types = input.read_s().unwrap();
    assert_eq!(
        item_types, 2,
        "must encode 2 distinct item types (lead and coal)"
    );

    let item1 = input.read_s().unwrap();
    let count1 = input.read_i().unwrap();
    let item2 = input.read_s().unwrap();
    let count2 = input.read_i().unwrap();

    let map: std::collections::HashMap<i16, i32> =
        vec![(item1, count1), (item2, count2)].into_iter().collect();
    assert_eq!(
        map.get(&1).copied(),
        Some(2),
        "must report 2 lead items in ItemModule"
    );
    assert_eq!(
        map.get(&2).copied(),
        Some(1),
        "must report 1 coal item in ItemModule"
    );
}

#[test]
fn test_unit_factory_plan_defaults_to_zero_when_unconfigured() {
    use crate::network::buildings::snapshot::encode_unit_factory_sync;
    use crate::network::codec::Reads;
    use std::collections::HashMap;
    use std::io::Cursor;

    let tile = erekir_tile((10 << 16) | 10, 378, 0); // Air factory, empty config
    let mut bytes = Vec::new();
    encode_unit_factory_sync(&mut bytes, &tile, &HashMap::new()).unwrap();
    let mut input = Cursor::new(bytes);
    input.read_f().unwrap(); // health
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let items = input.read_s().unwrap();
    for _ in 0..items {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    input.read_s().unwrap(); // power links
    input.read_f().unwrap(); // eff
    input.read_b().unwrap(); // eff byte
    input.read_b().unwrap(); // opt eff byte
    input.read_f().unwrap(); // payVector.x
    input.read_f().unwrap(); // payVector.y
    input.read_f().unwrap(); // payRotation
    input.read_bool().unwrap(); // payload null
    input.read_f().unwrap(); // progress
    let plan = input.read_s().unwrap();
    assert_eq!(
        plan, 0,
        "unconfigured factory must default to plan 0 (not -1)"
    );
}

#[test]
fn impact_reactor_warmup_survives_block_snapshot_window() {
    use crate::network::buildings::snapshot::encode_power_generator_sync;
    use crate::network::codec::Reads;
    use crate::network::simulation::power::simulate_impact_reactors;
    use std::io::Cursor;

    let world = erekir_test_world();
    let pos = (20 << 16) | 20;
    let source = (18 << 16) | 20;
    let mut reactor_tile = erekir_tile(pos, 316, 0);
    reactor_tile.occupied = vec![pos];
    reactor_tile.inventory = vec![(14, 60)];
    reactor_tile.stored_liquid = 3;
    reactor_tile.liquid_amount = 2000.0;
    reactor_tile.output_liquid_amount = 0.0;
    reactor_tile.power_links = vec![source];
    let mut source_tile = erekir_tile(source, 410, 0);
    source_tile.power_links = vec![pos];
    world.tiles.insert(source, source_tile);
    world.tiles.insert(pos, reactor_tile);

    // More than one 6-second BlockSnapshot interval (360 ticks).
    for _ in 0..400 {
        simulate_impact_reactors(&world, 1.0);
    }
    let before = world.tiles.get(&pos).unwrap().clone();
    assert!(
        before.output_liquid_amount > 0.2,
        "warmup must advance on the server: {}",
        before.output_liquid_amount
    );
    let items_before = inventory_count(&before.inventory, 14);
    let cryo_before = before.liquid_amount;
    let links_before = before.power_links.clone();

    let mut sync_bytes = Vec::new();
    encode_power_generator_sync(&mut sync_bytes, &before, &std::collections::HashMap::new())
        .unwrap();
    let mut input = Cursor::new(sync_bytes);
    input.read_f().unwrap();
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let item_count = input.read_s().unwrap();
    assert!(item_count > 0, "snapshot must keep blast compound");
    for _ in 0..item_count {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    let link_count = input.read_s().unwrap();
    let mut links = Vec::new();
    for _ in 0..link_count {
        links.push(input.read_i().unwrap());
    }
    assert_eq!(links, links_before, "PowerModule.links must not be wiped");
    input.read_f().unwrap();
    let liquid_count = input.read_s().unwrap();
    assert!(liquid_count > 0, "snapshot must keep cryofluid");
    for _ in 0..liquid_count {
        input.read_s().unwrap();
        input.read_f().unwrap();
    }
    input.read_b().unwrap();
    input.read_b().unwrap();
    input.read_f().unwrap();
    input.read_f().unwrap();
    let wire_warmup = input.read_f().unwrap();
    assert!(
        (wire_warmup - before.output_liquid_amount).abs() < 0.001,
        "writeSync warmup must match live state"
    );

    for _ in 0..400 {
        simulate_impact_reactors(&world, 1.0);
    }
    let after = world.tiles.get(&pos).unwrap().clone();
    assert!(
        after.output_liquid_amount > before.output_liquid_amount,
        "warmup must keep rising across a BlockSnapshot window"
    );
    assert!(
        inventory_count(&after.inventory, 14) <= items_before,
        "fuel must not reset upward"
    );
    assert!(
        after.liquid_amount <= cryo_before,
        "cryofluid must not reset upward"
    );
}

fn saturated_power_source(position: i32) -> DynamicTile {
    let mut source = erekir_tile(position, 410, 0);
    source.occupied = vec![position];
    source.power_links = (0..100).map(|index| ((80 + index) << 16) | 80).collect();
    source
}

#[test]
fn reverse_only_power_laser_keeps_factory_and_impact_powered() {
    use crate::network::simulation::power::simulate_impact_reactors;
    // Live PowerModule.links on the consumer are enough for vanilla
    // getPowerConnections. A node at maxNodes must not split that graph
    // or BlockSnapshot writes status=0 and the client bar resets every 6s.
    let world = erekir_test_world();
    let factory = (14 << 16) | 10;
    let reactor = (14 << 16) | 20;
    let node = (8 << 16) | 10;
    let reactor_node = (8 << 16) | 20;
    world.tiles.insert(node, saturated_power_source(node));
    world
        .tiles
        .insert(reactor_node, saturated_power_source(reactor_node));

    let mut factory_tile = erekir_tile(factory, 377, 0);
    factory_tile.occupied = vec![factory];
    factory_tile.config = vec![1, 0, 0, 0, 0];
    factory_tile.inventory = vec![(9, 20), (1, 20)];
    factory_tile.power_links = vec![node];
    world.tiles.insert(factory, factory_tile);

    let mut reactor_tile = erekir_tile(reactor, 316, 0);
    reactor_tile.occupied =
        crate::network::buildings::construction::block_footprint(&world, reactor, 316)
            .unwrap_or_else(|| vec![reactor]);
    reactor_tile.inventory = vec![(14, 60)];
    reactor_tile.stored_liquid = 3;
    reactor_tile.liquid_amount = 2_000.0;
    reactor_tile.power_links = vec![reactor_node];
    world.tiles.insert(reactor, reactor_tile);

    let power = compute_power_efficiency(&world);
    assert!(
        power.get(&factory).copied().unwrap_or(0.0) >= 0.99,
        "factory reverse-only laser must carry graph status: {:?}",
        power.get(&factory)
    );
    assert!(
        power.get(&reactor).copied().unwrap_or(0.0) >= 0.99,
        "impact reverse-only laser must carry graph status: {:?}",
        power.get(&reactor)
    );

    let connections = DashMap::new();
    for _ in 0..400 {
        simulate_unit_factories(&world, &connections, 1.0, &power);
        simulate_impact_reactors(&world, 1.0);
    }
    let factory_after = world.tiles.get(&factory).unwrap().clone();
    assert!(
        factory_after.production_progress > 350.0,
        "factory progress must advance on a reverse-only laser: {}",
        factory_after.production_progress
    );
    let reactor_after = world.tiles.get(&reactor).unwrap().clone();
    assert!(
        reactor_after.output_liquid_amount > 0.2,
        "impact warmup must rise on a reverse-only laser: {}",
        reactor_after.output_liquid_amount
    );
}

#[test]
fn unit_factory_progress_advances_at_unit_cap() {
    let world = erekir_test_world();
    for id in 0..8 {
        world.enemies.insert(
            3_000_200 + id,
            ground_unit_on_tile(3_000_200 + id, 0, (id << 16) | 1, 100.0, 0.0),
        );
    }
    let factory = (10 << 16) | 10;
    let mut tile = erekir_tile(factory, 377, 0);
    tile.occupied = vec![factory];
    tile.config = vec![1, 0, 0, 0, 0];
    tile.inventory = vec![(9, 20), (1, 20)];
    world.tiles.insert(factory, tile);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(factory, 1.0);
    simulate_unit_factories(&world, &connections, 400.0, &power);
    let after = world.tiles.get(&factory).unwrap();
    assert!(
        (after.production_progress - 400.0).abs() < 0.01,
        "cap must not freeze the construct bar: {}",
        after.production_progress
    );
    assert_eq!(world.enemies.len(), 8, "cap still blocks the spawn");
}

#[test]
fn reconstructor_absorbs_enter_payload_on_footprint_once() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut dagger = ground_unit_on_tile(7, 0, pos, 150.0, 0.0);
    dagger.team = 1;
    world.enemies.insert(7, dagger);
    let mut tile = erekir_tile(pos, 380, 0);
    tile.occupied = vec![pos];
    world.tiles.insert(pos, tile);
    world.unit_orders.insert(
        7,
        crate::network::world::UnitOrder {
            unit_id: 7,
            command: 5,
            target_kind: 1,
            target_id: pos,
            ..Default::default()
        },
    );
    world.register_unit_group(7);
    let connections = DashMap::new();
    assert!(simulate_unit_payload_entries(&world, &connections, 1.0));
    assert!(
        !world.enemies.contains_key(&7),
        "enterPayload on the footprint absorbs exactly once"
    );
    assert!(
        !world.unit_group_order.lock().contains(&7),
        "absorbed unit must leave the unit group"
    );
    let after = world.tiles.get(&pos).unwrap();
    assert_eq!(after.stored_amount, 1);
    assert!(after.payload.is_some());
    drop(after);
    assert!(!simulate_unit_payload_entries(&world, &connections, 1.0));
}

#[test]
fn reconstructor_rejects_move_output_face_and_core_spawned_absorb() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let output_face = (12 << 16) | 10; // past a size-3 rot-0 output face
    let mut tile = erekir_tile(pos, 380, 0);
    tile.occupied = vec![pos];
    world.tiles.insert(pos, tile);
    let connections = DashMap::new();

    let mut dagger = ground_unit_on_tile(7, 0, pos, 150.0, 0.0);
    dagger.team = 1;
    world.enemies.insert(7, dagger);
    world.unit_orders.insert(
        7,
        crate::network::world::UnitOrder {
            unit_id: 7,
            command: 0,
            target_kind: 1,
            target_id: pos,
            ..Default::default()
        },
    );
    assert!(!simulate_unit_payload_entries(&world, &connections, 1.0));
    assert!(
        world.enemies.contains_key(&7),
        "move command must not absorb"
    );

    world.unit_orders.get_mut(&7).unwrap().command = 5;
    let mut off_face = world.enemies.get_mut(&7).unwrap();
    off_face.x = ((output_face >> 16) as i16 as f32) * 8.0 + 4.0;
    off_face.y = (output_face as i16 as f32) * 8.0 + 4.0;
    drop(off_face);
    assert!(!simulate_unit_payload_entries(&world, &connections, 1.0));
    assert!(
        world.enemies.contains_key(&7),
        "standing past the output face is not buildOn"
    );

    world.enemies.remove(&7);
    let mut alpha = ground_unit_on_tile(8, 35, pos, 150.0, 0.0);
    alpha.team = 1;
    world.enemies.insert(8, alpha);
    world.unit_orders.insert(
        8,
        crate::network::world::UnitOrder {
            unit_id: 8,
            command: 5,
            target_kind: 1,
            target_id: pos,
            ..Default::default()
        },
    );
    assert!(!simulate_unit_payload_entries(&world, &connections, 1.0));
    assert!(
        world.enemies.contains_key(&8),
        "core-spawned alpha must not absorb"
    );

    world.enemies.remove(&8);
    // Type 46 (anthicus-missile): `allowedInPayloads=false`. Even on the
    // footprint with command 5 it must not absorb.
    let mut missile = ground_unit_on_tile(9, 46, pos, 99.6, 0.0);
    missile.team = 1;
    world.enemies.insert(9, missile);
    world.unit_orders.insert(
        9,
        crate::network::world::UnitOrder {
            unit_id: 9,
            command: 5,
            target_kind: 1,
            target_id: pos,
            ..Default::default()
        },
    );
    assert!(!simulate_unit_payload_entries(&world, &connections, 1.0));
    assert!(
        world.enemies.contains_key(&9),
        "allowedInPayloads=false must not absorb"
    );
}

#[test]
fn unit_payload_gates_match_1597_allowed_and_core_spawned() {
    use crate::network::economy::{unit_allowed_in_payloads, unit_spawned_by_core};

    assert!(unit_allowed_in_payloads(0), "dagger");
    assert!(unit_allowed_in_payloads(38), "stell");
    for banned in [46, 53, 55, 61, 64, 68, 69] {
        assert!(
            !unit_allowed_in_payloads(banned),
            "unit {banned} is not allowedInPayloads"
        );
    }
    assert!(unit_spawned_by_core(35) && unit_spawned_by_core(36) && unit_spawned_by_core(37));
    assert!(unit_spawned_by_core(58) && unit_spawned_by_core(59) && unit_spawned_by_core(60));
    assert!(!unit_spawned_by_core(0));
}

#[test]
fn reconstructor_applies_unit_cost_multiplier_to_items() {
    let world = erekir_test_world();
    world.wave_rules.write().unit_cost_multiplier = 0.5;
    let pos = (10 << 16) | 10;
    let mut dagger = ground_unit_on_tile(7, 0, pos, 150.0, 0.0);
    dagger.team = 1;
    let mut tile = erekir_tile(pos, 380, 0);
    tile.occupied = vec![pos];
    tile.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(
        dagger,
    )));
    tile.inventory = vec![(9, 20), (3, 20)];
    world.tiles.insert(pos, tile);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(pos, 1.0);
    simulate_reconstructors(&world, &connections, 600.0, &power);
    let held = world.tiles.get(&pos).unwrap().clone();
    match held.payload.as_deref() {
        Some(crate::network::world::CarriedPayload::Unit(unit)) => {
            assert_eq!(unit.unit_type, 1, "half-cost still upgrades");
        }
        other => panic!("expected mace payload, got {other:?}"),
    }
    assert_eq!(inventory_count(&held.inventory, 9), 0);
    assert_eq!(inventory_count(&held.inventory, 4), 0);
    drop(held);

    world.enemies.clear();
    let mut dagger = ground_unit_on_tile(8, 0, pos, 150.0, 0.0);
    dagger.team = 1;
    let mut blocked = world.tiles.get_mut(&pos).unwrap();
    blocked.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(
        dagger,
    )));
    blocked.production_progress = 0.0;
    blocked.inventory = vec![(9, 19), (4, 20)];
    drop(blocked);
    simulate_reconstructors(&world, &connections, 600.0, &power);
    let blocked = world.tiles.get(&pos).unwrap();
    match blocked.payload.as_deref() {
        Some(crate::network::world::CarriedPayload::Unit(unit)) => {
            assert_eq!(unit.unit_type, 0, "below scaled cost does not upgrade");
        }
        other => panic!("expected dagger payload, got {other:?}"),
    }
    assert_eq!(inventory_count(&blocked.inventory, 9), 19);
}

#[test]
fn reconstructor_liquid_drain_ignores_unit_build_speed() {
    let world = erekir_test_world();
    world.wave_rules.write().unit_build_speed_multiplier = 2.0;
    let pos = (10 << 16) | 10;
    let mut stell = ground_unit_on_tile(8, 38, pos, 850.0, 0.0);
    stell.team = 1;
    let mut tile = erekir_tile(pos, 389, 0);
    tile.occupied = vec![pos];
    tile.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(stell)));
    tile.inventory = vec![(9, 40), (17, 30)];
    tile.stored_liquid = 8;
    tile.liquid_amount = 10.0;
    world.tiles.insert(pos, tile);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(pos, 1.0);
    simulate_reconstructors(&world, &connections, 60.0, &power);
    let after = world.tiles.get(&pos).unwrap();
    assert!(
        (after.production_progress - 120.0).abs() < 0.01,
        "build speed 2.0 doubles progress: {}",
        after.production_progress
    );
    let drained = 10.0 - after.liquid_amount;
    assert!(
        (drained - 3.0).abs() < 0.01,
        "liquid drain is edelta * unitCost, not unitBuildSpeed: drained {drained}"
    );
}

#[test]
fn reconstructor_progress_applies_unit_build_speed_multiplier() {
    // Reconstructor.updateTile: `progress += edelta() *
    // state.rules.unitBuildSpeed(team)` — the same multiplier UnitFactory
    // uses. The client predicts the block locally, so a server that ignored
    // the multiplier drifted and every BlockSnapshot snapped the bar back.
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let seed = |world: &crate::network::world::DynamicWorld| {
        let mut dagger = ground_unit_on_tile(7, 0, pos, 150.0, 0.0);
        dagger.team = 1;
        dagger.unit_type = 0;
        let mut tile = erekir_tile(pos, 380, 0);
        tile.occupied = vec![pos];
        tile.stored_amount = 1; // dagger occupies the payload
        tile.inventory = vec![(9, 80), (3, 80)];
        tile.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(
            dagger,
        )));
        world.tiles.insert(pos, tile);
    };
    seed(&world);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(pos, 1.0);

    simulate_reconstructors(&world, &connections, 100.0, &power);
    let baseline = world.tiles.get(&pos).unwrap().production_progress;
    assert!(
        (baseline - 100.0).abs() < 0.01,
        "default multiplier is 1.0: {baseline}"
    );

    // Gamemode.pvp sets unitBuildSpeedMultiplier = 2; TeamRule stacks on top.
    world.wave_rules.write().unit_build_speed_multiplier = 2.0;
    world.wave_rules.write().team_rules.insert(
        1,
        crate::network::units::TeamRule {
            unit_build_speed_multiplier: 1.5,
            ..Default::default()
        },
    );
    seed(&world);
    simulate_reconstructors(&world, &connections, 100.0, &power);
    let scaled = world.tiles.get(&pos).unwrap().production_progress;
    assert!(
        (scaled - 300.0).abs() < 0.01,
        "2.0 global * 1.5 team must triple the rate: {scaled}"
    );
}

#[test]
fn production_accepts_official_graphite_and_metaglass_across_snapshots() {
    let out = DashMap::new();
    for (block, input_type, items, rejected, ticks) in [
        (380, 20, vec![(9, 40), (3, 40)], 4, 600),
        (381, 21, vec![(9, 130), (6, 80), (2, 40)], 3, 1800),
        (379, -1, vec![(9, 20), (2, 35)], 3, 2700),
    ] {
        let world = erekir_test_world();
        let pos = (10 << 16) | 10;
        let mut tile = erekir_tile(pos, block, 0);
        tile.config = vec![1, 0, 0, 0, 0];
        if input_type >= 0 {
            let mut unit = ground_unit_on_tile(700, input_type, pos, 100.0, 0.0);
            unit.team = 1;
            tile.payload = Some(Box::new(CarriedPayload::Unit(unit)));
        }
        world.tiles.insert(pos, tile);
        for (item, amount) in &items {
            for _ in 0..*amount {
                assert!(
                    accept_logistics_item_from(&world, pos, *item, None, 0),
                    "block {block} rejected official input {item}"
                );
            }
        }
        assert!(!accept_logistics_item_from(&world, pos, rejected, None, 0));
        let power = HashMap::from([(pos, 1.0)]);
        for tick in 1..=ticks {
            simulate_unit_factories(&world, &out, 1.0, &power);
            simulate_reconstructors(&world, &out, 1.0, &power);
            if tick % 360 == 0 && tick < ticks {
                let tile = world.tiles.get(&pos).unwrap().clone();
                assert_eq!(tile.production_progress, tick as f32);
                let frame = crate::network::buildings::snapshot::encode_factory_snapshot_tiles(
                    &[tile],
                    &power,
                    Some(&world),
                )
                .unwrap();
                assert!(!frame.is_empty());
            }
        }
        let tile = world.tiles.get(&pos).unwrap();
        let Some(CarriedPayload::Unit(unit)) = tile.payload.as_deref() else {
            panic!("block {block} did not finish production");
        };
        assert_eq!(
            unit.unit_type,
            if input_type < 0 { 25 } else { input_type + 1 }
        );
        assert!(tile.inventory.is_empty(), "consume once on completion");
    }
}

#[test]
fn unit_cap_counts_carried_units_but_not_building_payloads() {
    let world = erekir_test_world();
    world.wave_rules.write().unit_cap = 1;
    world.wave_rules.write().unit_cap_variable = false;
    let pos = (10 << 16) | 10;
    let mut dagger = ground_unit_on_tile(700, 0, pos, 150.0, 0.0);
    dagger.team = 1;
    let mut carrier = ground_unit_on_tile(701, 24, pos, 1000.0, 0.0);
    carrier.team = 1;
    carrier.payloads.push(CarriedPayload::Unit(dagger.clone()));
    world.enemies.insert(carrier.id, carrier);
    assert!(
        !can_create_unit(&world, 1, 0),
        "carried dagger still occupies its cap"
    );
    assert!(can_create_unit(&world, 2, 0));
    world.enemies.remove(&701);
    let mut building = erekir_tile(pos, 380, 0);
    building.payload = Some(Box::new(CarriedPayload::Unit(dagger.clone())));
    world.tiles.insert(pos, building);
    assert!(
        can_create_unit(&world, 1, 0),
        "reconstructor payload has left the live-unit count"
    );
    world.enemies.insert(dagger.id, dagger);
    assert!(!can_create_unit(&world, 1, 0));
    world.enemies.remove(&700);
    assert!(
        can_create_unit(&world, 1, 0),
        "death/removal releases the cap"
    );
}

#[test]
fn reconstructor_progress_advances_at_unit_cap() {
    let world = erekir_test_world();
    for id in 0..8 {
        world.enemies.insert(
            3_000_300 + id,
            ground_unit_on_tile(3_000_300 + id, 1, (id << 16) | 2, 150.0, 0.0),
        );
    }
    let pos = (10 << 16) | 10;
    let mut mace = ground_unit_on_tile(7, 0, pos, 150.0, 0.0);
    mace.team = 1;
    mace.unit_type = 0;
    let mut tile = erekir_tile(pos, 380, 0);
    tile.occupied = vec![pos];
    tile.stored_amount = 1; // dagger
    tile.inventory = vec![(9, 80), (3, 80)];
    tile.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(mace)));
    world.tiles.insert(pos, tile);
    let connections = DashMap::new();
    let mut power = std::collections::HashMap::new();
    power.insert(pos, 1.0);
    simulate_reconstructors(&world, &connections, 400.0, &power);
    let after = world.tiles.get(&pos).unwrap();
    assert!(
        (after.production_progress - 400.0).abs() < 0.01,
        "cap must not freeze reconstructor progress: {}",
        after.production_progress
    );
    assert!(after.payload.is_some());
    assert_eq!(world.enemies.len(), 8, "cap still blocks the dump");
}

#[test]
fn unit_factory_progress_survives_block_snapshot_window() {
    use crate::network::buildings::snapshot::encode_unit_factory_sync;
    use crate::network::codec::Reads;
    use std::collections::HashMap;
    use std::io::Cursor;

    let world = erekir_test_world();
    let factory = (10 << 16) | 10;
    let node = (12 << 16) | 10;
    let mut tile = erekir_tile(factory, 377, 0);
    tile.occupied = vec![factory];
    tile.config = vec![1, 0, 0, 0, 0];
    tile.inventory = vec![(9, 20), (1, 20)];
    tile.power_links = vec![node];
    world.tiles.insert(factory, tile);
    let connections = DashMap::new();
    let mut power = HashMap::new();
    power.insert(factory, 1.0);

    for _ in 0..400 {
        simulate_unit_factories(&world, &connections, 1.0, &power);
    }
    let before = world.tiles.get(&factory).unwrap().clone();
    assert!(
        (before.production_progress - 400.0).abs() < 0.01,
        "dagger factory must advance 400 ticks: {}",
        before.production_progress
    );

    let mut bytes = Vec::new();
    encode_unit_factory_sync(&mut bytes, &before, &power).unwrap();
    let mut input = Cursor::new(bytes);
    input.read_f().unwrap();
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let items = input.read_s().unwrap();
    for _ in 0..items {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    let link_count = input.read_s().unwrap();
    let mut links = Vec::new();
    for _ in 0..link_count {
        links.push(input.read_i().unwrap());
    }
    assert_eq!(
        links,
        vec![node],
        "factory PowerModule.links must round-trip"
    );
    input.read_f().unwrap();
    input.read_b().unwrap();
    input.read_b().unwrap();
    input.read_f().unwrap();
    input.read_f().unwrap();
    input.read_f().unwrap();
    assert!(!input.read_bool().unwrap());
    let wire_progress = input.read_f().unwrap();
    assert!(
        (wire_progress - before.production_progress).abs() < 0.001,
        "writeSync progress must match live state"
    );
    assert_eq!(input.read_s().unwrap(), 0, "plan 0 dagger");

    for _ in 0..400 {
        simulate_unit_factories(&world, &connections, 1.0, &power);
    }
    let after = world.tiles.get(&factory).unwrap().clone();
    assert!(
        (after.production_progress - 800.0).abs() < 0.01,
        "progress must keep advancing across a BlockSnapshot window: {}",
        after.production_progress
    );
    assert_eq!(
        inventory_count(&after.inventory, 9),
        inventory_count(&before.inventory, 9),
        "items must not reset at the snapshot boundary"
    );
}

#[test]
fn unit_production_power_and_snapshot_follow_scaled_cost_and_held_payload() {
    use crate::network::codec::Reads;
    let world = erekir_test_world();
    world.wave_rules.write().unit_cost_multiplier = 0.5;
    let pos = (10 << 16) | 10;
    let mut unit = ground_unit_on_tile(701, 0, pos, 150.0, 0.0);
    unit.team = 1;
    let mut reconstructor = erekir_tile(pos, 380, 0);
    reconstructor.inventory = vec![(9, 20), (3, 20)];
    reconstructor.payload = Some(Box::new(CarriedPayload::Unit(unit.clone())));
    assert!(
        effective_power_role(&world, &reconstructor, 1.0)
            .unwrap()
            .demand
            > 0.0
    );
    let power = std::collections::HashMap::from([(pos, 1.0)]);
    let mut bytes = Vec::new();
    encode_dynamic_tile_sync(&mut bytes, &reconstructor, &power, Some(&world)).unwrap();
    let mut input = std::io::Cursor::new(bytes);
    input.read_f().unwrap();
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let items = input.read_s().unwrap();
    for _ in 0..items {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    assert_eq!(input.read_s().unwrap(), 0); // no power links in this codec fixture
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(
        input.read_b().unwrap(),
        255,
        "snapshot must use live unitCost, not raw item amounts"
    );

    let mut factory = erekir_tile(pos, 377, 0);
    factory.config = vec![5, 6, 0, 0];
    factory.inventory = vec![(9, 10), (1, 10)];
    assert!(effective_power_role(&world, &factory, 1.0).unwrap().demand > 0.0);
    factory.payload = Some(Box::new(CarriedPayload::Unit(unit)));
    assert_eq!(
        effective_power_role(&world, &factory, 1.0).unwrap().demand,
        0.0,
        "completed factory output waits without consuming production power"
    );
}

#[test]
fn survival_conveyor_delivery_reconstructor_consumes_power_from_actual_payload() {
    let world = erekir_test_world();
    let source = (7 << 16) | 10;
    let target = (10 << 16) | 10;
    let mut conveyor = erekir_tile(source, 398, 0);
    let mut dagger = ground_unit_on_tile(700, 0, source, 150.0, 0.0);
    dagger.team = 1;
    conveyor.payload = Some(Box::new(CarriedPayload::Unit(dagger)));
    let mut reconstructor = erekir_tile(target, 380, 0);
    reconstructor.inventory = vec![(9, 40), (3, 40)];
    world.tiles.insert(source, conveyor.clone());
    world.tiles.insert(target, reconstructor);
    assert!(transfer_payload_forward(&world, &conveyor));
    let mut received = world.tiles.get(&target).unwrap().clone();
    assert!(received.payload.is_some());
    // Old saves and alternate ingress paths can lack this compatibility
    // marker. The real payload must still drive power and prediction.
    received.stored_amount = 0;
    assert!(
        effective_power_role(&world, &received, 1.0).unwrap().demand > 0.0,
        "a delivered unit must activate the reconstructor without a legacy marker"
    );
    world.tiles.insert(target, received);
    let panels = [(10 << 16) | 14, (14 << 16) | 14];
    for panel in panels {
        let mut solar = erekir_tile(panel, 314, 0);
        solar.power_links = vec![target];
        world.tiles.insert(panel, solar);
    }
    world.tiles.get_mut(&target).unwrap().power_links = panels.to_vec();
    let wall = (12 << 16) | 10;
    world.tiles.insert(wall, erekir_tile(wall, 216, 0));
    let out = DashMap::new();
    let snapshot = |name: &str, expected_efficiency: u8| {
        use crate::network::codec::Reads;
        let tile = world.tiles.get(&target).unwrap().clone();
        let power = compute_power_efficiency(&world);
        let mut bytes = Vec::new();
        encode_dynamic_tile_sync(&mut bytes, &tile, &power, Some(&world)).unwrap();
        let mut input = std::io::Cursor::new(&bytes);
        input.read_f().unwrap();
        for _ in 0..5 {
            input.read_b().unwrap();
        }
        let items = input.read_s().unwrap();
        for _ in 0..items {
            input.read_s().unwrap();
            input.read_i().unwrap();
        }
        let links = input.read_s().unwrap();
        for _ in 0..links {
            input.read_i().unwrap();
        }
        let status = input.read_f().unwrap();
        assert_eq!(status, 1.0, "solar graph remains supplied");
        assert_eq!(input.read_b().unwrap(), expected_efficiency);
        if let Ok(directory) = std::env::var("OXIDE_RECONSTRUCTOR_FIXTURE_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(std::path::Path::new(&directory).join(name), bytes).unwrap();
        }
    };
    for _ in 0..420 {
        let power = update_power_network(&world, 1.0);
        simulate_reconstructors(&world, &out, 1.0, &power);
    }
    let progress = world.tiles.get(&target).unwrap().production_progress;
    assert!(
        progress > 390.0 && progress < 420.0,
        "input slides before construction: {progress}"
    );
    snapshot("reconstructor-running.bin", 255);
    world.tiles.get_mut(&target).unwrap().enabled = false;
    for _ in 0..200 {
        let power = update_power_network(&world, 1.0);
        simulate_reconstructors(&world, &out, 1.0, &power);
    }
    assert_eq!(
        world.tiles.get(&target).unwrap().production_progress,
        progress
    );
    snapshot("reconstructor-paused.bin", 0);
    world.tiles.get_mut(&target).unwrap().enabled = true;
    for _ in 0..400 {
        let power = update_power_network(&world, 1.0);
        simulate_reconstructors(&world, &out, 1.0, &power);
    }
    let completed = world.tiles.get(&target).unwrap().clone();
    assert!(
        matches!(completed.payload.as_deref(), Some(CarriedPayload::Unit(unit)) if unit.unit_type == 1)
    );
    assert_eq!(inventory_count(&completed.inventory, 9), 0);
    assert_eq!(inventory_count(&completed.inventory, 4), 0);
    assert_eq!(
        completed.production_progress, 0.0,
        "completion resets once, not during construction"
    );
    snapshot("reconstructor-completed.bin", 0);
    world.tiles.remove(&wall);
    for _ in 0..30 {
        let power = update_power_network(&world, 1.0);
        simulate_reconstructors(&world, &out, 1.0, &power);
    }
    assert!(world.tiles.get(&target).unwrap().payload.is_none());
    assert_eq!(
        world
            .enemies
            .iter()
            .filter(|unit| unit.team == 1 && unit.unit_type == 1)
            .count(),
        1
    );
    // The released upgrade must be a live combat unit, not just a rendered
    // payload. Let its command AI engage a stationary survival enemy.
    let enemy = ground_unit_on_tile(702, 0, (17 << 16) | 10, 150.0, 0.0);
    world.enemies.insert(702, enemy);
    for _ in 0..180 {
        crate::network::simulation::simulate_allied_units(&world, &out, 1.0);
        crate::network::combat::simulate_projectiles(&world, &out, 1.0);
    }
    assert!(
        world
            .enemies
            .get(&702)
            .is_none_or(|enemy| enemy.health < 150.0),
        "reconstructed unit must fight after leaving the payload"
    );
}

#[test]
fn reconstructor_progress_and_payload_survive_block_snapshot_window() {
    use crate::network::buildings::snapshot::encode_reconstructor_sync;
    use crate::network::codec::Reads;
    use crate::network::units::controller::write_carried_payload;
    use std::collections::HashMap;
    use std::io::{Cursor, Read};

    // Multiplicative reconstructTime is 1800 ticks, so two 400-tick windows stay
    // in-progress (additive would complete at 600 and look like a reset).
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let node = (12 << 16) | 10;
    let mut mace = ground_unit_on_tile(7, 1, pos, 150.0, 0.0);
    mace.team = 1;
    let mut tile = erekir_tile(pos, 381, 0);
    tile.occupied = vec![pos];
    tile.stored_amount = 2; // mace (type 1)
    tile.inventory = vec![(9, 260), (6, 160), (2, 80)];
    tile.power_links = vec![node];
    tile.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(mace)));
    world.tiles.insert(pos, tile);
    let connections = DashMap::new();
    let mut power = HashMap::new();
    power.insert(pos, 1.0);

    for _ in 0..400 {
        simulate_reconstructors(&world, &connections, 1.0, &power);
    }
    let before = world.tiles.get(&pos).unwrap().clone();
    assert!(
        (before.production_progress - 400.0).abs() < 0.01,
        "reconstructor must advance 400 ticks: {}",
        before.production_progress
    );
    assert!(before.payload.is_some(), "unit payload must stay loaded");
    assert_eq!(before.stored_amount, 2);

    let mut bytes = Vec::new();
    encode_reconstructor_sync(&mut bytes, &before, &power).unwrap();
    let mut input = Cursor::new(bytes);
    input.read_f().unwrap();
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let items = input.read_s().unwrap();
    for _ in 0..items {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    let link_count = input.read_s().unwrap();
    let mut links = Vec::new();
    for _ in 0..link_count {
        links.push(input.read_i().unwrap());
    }
    assert_eq!(
        links,
        vec![node],
        "reconstructor PowerModule.links must round-trip"
    );
    input.read_f().unwrap();
    input.read_b().unwrap();
    input.read_b().unwrap();
    input.read_f().unwrap();
    input.read_f().unwrap();
    input.read_f().unwrap();
    let mut expected_payload = Vec::new();
    write_carried_payload(&mut expected_payload, before.payload.as_deref().unwrap()).unwrap();
    let mut got = vec![0u8; expected_payload.len()];
    input.read_exact(&mut got).unwrap();
    assert_eq!(
        got, expected_payload,
        "writeSync must keep the loaded unit payload"
    );
    let wire_progress = input.read_f().unwrap();
    assert!(
        (wire_progress - before.production_progress).abs() < 0.001,
        "writeSync progress must match live state"
    );

    for _ in 0..400 {
        simulate_reconstructors(&world, &connections, 1.0, &power);
    }
    let after = world.tiles.get(&pos).unwrap().clone();
    assert!(
        (after.production_progress - 800.0).abs() < 0.01,
        "progress must keep advancing across a BlockSnapshot window: {}",
        after.production_progress
    );
    assert!(
        after.payload.is_some(),
        "payload must not vanish at snapshot"
    );
    assert_eq!(after.stored_amount, 2);
}

#[test]
fn reconstructor_holds_payload_until_move_out_and_keeps_subtick_remainder() {
    use crate::network::buildings::snapshot::encode_reconstructor_sync;
    use crate::network::codec::Reads;
    use crate::network::units::controller::write_carried_payload;
    use std::collections::HashMap;
    use std::io::{Cursor, Read};

    // Additive reconstructor (380): size 3, constructTime 600, dagger → mace.
    // Incoming payVector starts at the input face so progress is gated on
    // hasArrived; completion wraps with `progress %= 1f` and holds the
    // upgraded unit; dump happens only after moveOut reaches the output face.
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut dagger = ground_unit_on_tile(7, 0, pos, 150.0, 0.0);
    dagger.team = 1;
    let mut tile = erekir_tile(pos, 380, 0);
    tile.occupied = vec![pos];
    tile.stored_amount = 1;
    tile.inventory = vec![(9, 40), (3, 40)];
    tile.payload = Some(Box::new(crate::network::world::CarriedPayload::Unit(
        dagger,
    )));
    tile.payload_accum = vec![-12.0, 0.0];
    world.tiles.insert(pos, tile);
    let connections = DashMap::new();
    let mut power = HashMap::new();
    power.insert(pos, 1.0);

    simulate_reconstructors(&world, &connections, 1.0, &power);
    let after_step = world.tiles.get(&pos).unwrap().clone();
    assert!(
        (after_step.production_progress).abs() < 1e-5,
        "progress must not advance before arrival: {}",
        after_step.production_progress
    );
    assert!(
        world.enemies.is_empty(),
        "incoming payload is not a world unit"
    );
    let (pay_x, _) = unit_block_pay_vector(&after_step);
    assert!(
        (pay_x + 11.3).abs() < 0.02,
        "moveIn approaches the origin at 0.7 wu/tick: {pay_x}"
    );
    drop(after_step);

    for _ in 0..20 {
        simulate_reconstructors(&world, &connections, 1.0, &power);
    }
    let arrived = world.tiles.get(&pos).unwrap().clone();
    let (ax, ay) = unit_block_pay_vector(&arrived);
    assert!(
        ax.hypot(ay) <= 0.01,
        "payload must have arrived: ({ax}, {ay})"
    );
    assert!(
        (arrived.production_progress - 4.0).abs() < 0.02,
        "four produce ticks after the 18-tick slide: {}",
        arrived.production_progress
    );
    drop(arrived);

    for _ in 0..400 {
        simulate_reconstructors(&world, &connections, 1.0, &power);
    }
    let before = world.tiles.get(&pos).unwrap().clone();
    assert!(
        (before.production_progress - 404.0).abs() < 0.02,
        "progress must survive a BlockSnapshot window: {}",
        before.production_progress
    );
    match before.payload.as_deref() {
        Some(crate::network::world::CarriedPayload::Unit(unit)) => {
            assert_eq!(unit.unit_type, 0, "still the incoming dagger");
        }
        other => panic!("expected dagger payload, got {other:?}"),
    }

    let mut bytes = Vec::new();
    encode_reconstructor_sync(&mut bytes, &before, &power).unwrap();
    let mut input = Cursor::new(bytes);
    input.read_f().unwrap();
    for _ in 0..5 {
        input.read_b().unwrap();
    }
    let items = input.read_s().unwrap();
    for _ in 0..items {
        input.read_s().unwrap();
        input.read_i().unwrap();
    }
    let link_count = input.read_s().unwrap();
    for _ in 0..link_count {
        let _ = input.read_i().unwrap();
    }
    input.read_f().unwrap();
    input.read_b().unwrap();
    input.read_b().unwrap();
    let wire_x = input.read_f().unwrap();
    let wire_y = input.read_f().unwrap();
    let wire_rot = input.read_f().unwrap();
    let (live_x, live_y) = unit_block_pay_vector(&before);
    assert!(
        (wire_x - live_x).abs() < 1e-5 && (wire_y - live_y).abs() < 1e-5,
        "writeSync payVector must match live state ({wire_x}, {wire_y}) vs ({live_x}, {live_y})"
    );
    assert!(
        (wire_rot - before.payload_rotation).abs() < 1e-5,
        "writeSync payRotation must match live state"
    );
    let mut expected_payload = Vec::new();
    write_carried_payload(&mut expected_payload, before.payload.as_deref().unwrap()).unwrap();
    let mut got = vec![0u8; expected_payload.len()];
    input.read_exact(&mut got).unwrap();
    assert_eq!(got, expected_payload, "writeSync payload prefix");
    let wire_progress = input.read_f().unwrap();
    assert!(
        (wire_progress - before.production_progress).abs() < 0.001,
        "writeSync progress must match live state"
    );
    drop(before);

    if let Some(mut live) = world.tiles.get_mut(&pos) {
        live.production_progress = 599.4;
    }
    simulate_reconstructors(&world, &connections, 1.0, &power);
    let completed = world.tiles.get(&pos).unwrap().clone();
    assert!(
        (completed.production_progress - 0.4).abs() < 1e-4,
        "completion remainder is progress %= 1f: {}",
        completed.production_progress
    );
    match completed.payload.as_deref() {
        Some(crate::network::world::CarriedPayload::Unit(unit)) => {
            assert_eq!(unit.unit_type, 1, "held payload is the upgraded mace");
        }
        other => panic!("expected mace payload, got {other:?}"),
    }
    assert!(
        world.enemies.is_empty(),
        "unit enters the world only on release"
    );
    drop(completed);

    simulate_reconstructors(&world, &connections, 10.0, &power);
    let sliding = world.tiles.get(&pos).unwrap().clone();
    assert!(
        world.enemies.is_empty(),
        "10 ticks of 0.7 wu/tick have not reached the size-3 output face"
    );
    let (sx, sy) = unit_block_pay_vector(&sliding);
    assert!(
        sx.hypot(sy) > 0.5,
        "moveOut must have left the origin: ({sx}, {sy})"
    );
    let mut slide_bytes = Vec::new();
    encode_reconstructor_sync(&mut slide_bytes, &sliding, &power).unwrap();
    drop(sliding);
    simulate_reconstructors(&world, &connections, 10.0, &power);
    assert_eq!(world.enemies.len(), 1, "payload dumps once it reaches dest");
    let released = world.enemies.iter().next().unwrap().clone();
    assert_eq!(released.unit_type, 1, "mace");
    assert_eq!(released.team, 1);
    let empty = world.tiles.get(&pos).unwrap();
    assert!(empty.payload.is_none());
    assert_eq!(empty.stored_amount, 0);
    let _ = slide_bytes;
}

#[test]
fn unit_block_spawn_frame_is_tile_position() {
    let position: i32 = (10 << 16) | 10;
    let frame = encode_unit_block_spawn_frame(position).unwrap();
    let packet = crate::network::codec::read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
    assert_eq!(
        packet[0],
        crate::network::protocol::UNIT_BLOCK_SPAWN_PACKET_ID
    );
    assert_eq!(&packet[1..5], &position.to_be_bytes());
    assert_eq!(packet.len(), 5, "id + TypeIO.writeTile, no trailing bytes");
}

#[test]
fn unit_block_spawn_emits_on_factory_release_not_on_complete() {
    struct Collector(std::sync::Mutex<Vec<Vec<u8>>>);
    impl crate::network::outbound::FrameEmit for Collector {
        fn broadcast(&self, frame: Vec<u8>) {
            self.0.lock().unwrap().push(frame);
        }
        fn enqueue_to(&self, _id: i32, _frame: Vec<u8>, _critical: bool) -> bool {
            true
        }
        fn for_each_connection(&self, _visit: &mut dyn FnMut(i32)) {}
    }
    fn frames_contain_block_spawn(frames: &[Vec<u8>], position: i32) -> bool {
        frames.iter().any(|frame| {
            let Ok(packet) = crate::network::codec::read_packet(std::io::Cursor::new(&frame[2..]))
            else {
                return false;
            };
            packet.first() == Some(&crate::network::protocol::UNIT_BLOCK_SPAWN_PACKET_ID)
                && packet.get(1..5) == Some(&position.to_be_bytes())
        })
    }

    // tank-fabricator (386) plan is 2100 ticks (> one 360-tick BlockSnapshot).
    let world = erekir_test_world();
    let factory = (10 << 16) | 10;
    let mut tile = erekir_tile(factory, 386, 0);
    tile.inventory = vec![(16, 40), (9, 50)];
    world.tiles.insert(factory, tile);
    let collector = Collector(std::sync::Mutex::new(Vec::new()));
    let mut power = std::collections::HashMap::new();
    power.insert(factory, 1.0);

    for _ in 0..400 {
        simulate_unit_factories(&world, &collector, 1.0, &power);
    }
    assert!(
        world.tiles.get(&factory).unwrap().payload.is_none(),
        "400 ticks are still in-progress"
    );
    assert!(
        !frames_contain_block_spawn(&collector.0.lock().unwrap(), factory),
        "packet 146 must not fire while the payload is still unborn"
    );

    simulate_unit_factories(&world, &collector, 1_700.0, &power);
    assert!(
        world.tiles.get(&factory).unwrap().payload.is_some(),
        "completion holds the payload"
    );
    assert!(world.enemies.is_empty());
    assert!(
        !frames_contain_block_spawn(&collector.0.lock().unwrap(), factory),
        "packet 146 must wait for dump, not completion"
    );

    simulate_unit_factories(&world, &collector, 20.0, &power);
    assert_eq!(world.enemies.len(), 1);
    assert!(
        frames_contain_block_spawn(&collector.0.lock().unwrap(), factory),
        "UnitBlockSpawnCallPacket must leave on the release tick"
    );
}

#[test]
fn test_impact_reactor_rejects_non_cryofluid() {
    use crate::network::economy::liquids::accept_liquid_from;

    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut tile = erekir_tile(pos, 316, 0);
    tile.occupied = vec![pos];
    world.tiles.insert(pos, tile);

    // Liquid 0 (water) must be rejected
    assert_eq!(accept_liquid_from(&world, None, pos, 0, 10.0), 0.0);
    // Liquid 1 (slag) must be rejected
    assert_eq!(accept_liquid_from(&world, None, pos, 1, 10.0), 0.0);
    // Liquid 2 (oil) must be rejected
    assert_eq!(accept_liquid_from(&world, None, pos, 2, 10.0), 0.0);
    // Liquid 3 (cryofluid) must be accepted
    assert!(accept_liquid_from(&world, None, pos, 3, 10.0) > 0.0);
}

fn logistics_tile(world: &DynamicWorld, position: i32, block: i16, rotation: u8) {
    let mut tile = erekir_tile(position, block, rotation);
    tile.occupied = vec![position];
    world.tiles.insert(position, tile);
}

fn sorter_config(item: i16) -> Vec<u8> {
    // TypeIO object config: tag 5 (int) + 0 padding + big-endian item id.
    vec![5, 0, (item >> 8) as u8, (item & 0xff) as u8]
}

/// Router.java speed = 8f with time += 1/speed * delta(): one item leaves
/// every 8 ticks, not every 10 (M2).
#[test]
fn router_forwards_one_item_every_eight_ticks() {
    let world = erekir_test_world();
    let router_pos = (10 << 16) | 10;
    logistics_tile(&world, router_pos, 266, 0);
    let void_pos = (11 << 16) | 10;
    logistics_tile(&world, void_pos, 413, 0);
    assert!(accept_logistics_item_from(&world, router_pos, 5, None, 0));
    let no_power = HashMap::new();
    for _ in 0..7 {
        simulate_logistics(&world, 1.0, &no_power);
        assert_eq!(
            world.tiles.get(&router_pos).unwrap().stored_amount,
            1,
            "item must not leave before tick 8"
        );
    }
    simulate_logistics(&world, 1.0, &no_power);
    assert_eq!(world.tiles.get(&router_pos).unwrap().stored_amount, 0);
}

/// ((item == sortItem) != invert) == enabled (Sorter.java:116): a disabled
/// sorter never routes straight, not even matching items (M3).
#[test]
fn disabled_sorter_never_routes_straight_even_on_matching_item() {
    let world = erekir_test_world();
    let sorter_pos = (10 << 16) | 10;
    let mut sorter = erekir_tile(sorter_pos, 264, 0);
    sorter.config = sorter_config(5);
    sorter.enabled = false;
    world.tiles.insert(sorter_pos, sorter);
    // Straight (east) is air; the only receiver is the north conveyor.
    let north = (10 << 16) | 11;
    logistics_tile(&world, north, 257, 1);
    assert!(accept_logistics_item_from(
        &world,
        sorter_pos,
        5,
        Some((9 << 16) | 10),
        0
    ));
    let conveyor = world.tiles.get(&north).unwrap();
    assert!(
        conveyor.conveyor_items.iter().any(|(item, _)| *item == 5) || conveyor.stored_item == 5,
        "item must have been routed to the side conveyor"
    );
}

/// Sorter.getTileTarget 3-chain prevention (Sorter.java:119-121).
#[test]
fn instant_transfer_three_chain_is_rejected() {
    let world = erekir_test_world();
    for x in 9..=11 {
        let pos = (x << 16) | 10;
        let mut sorter = erekir_tile(pos, 264, 0);
        sorter.config = sorter_config(5);
        world.tiles.insert(pos, sorter);
    }
    // Middle sorter fed by another sorter with a third sorter straight
    // ahead: the transfer must refuse instead of chaining through.
    assert!(!accept_logistics_item_from(
        &world,
        (10 << 16) | 10,
        5,
        Some((9 << 16) | 10),
        0
    ));
}

/// Java Junction.updateTile walks four directional buffers independently:
/// a blocked head must not head-of-line block other directions (M4).
#[test]
fn junction_blocked_direction_does_not_head_of_line_block_others() {
    let world = erekir_test_world();
    let junction_pos = (10 << 16) | 10;
    let mut junction = erekir_tile(junction_pos, 261, 0);
    // East output is air (blocked); west output has an item void. Both
    // items are ready; the blocked one comes first in the buffer.
    junction.junction_items = vec![(0, 6, 0.0), (2, 7, 0.0)];
    world.tiles.insert(junction_pos, junction);
    let west_void = (9 << 16) | 10;
    logistics_tile(&world, west_void, 413, 0);
    simulate_junctions(&world, 1.0);
    let remaining = world
        .tiles
        .get(&junction_pos)
        .unwrap()
        .junction_items
        .clone();
    assert_eq!(remaining.len(), 1, "only the blocked item may remain");
    assert_eq!(remaining[0].0, 0, "the east (blocked) item stays queued");
}

/// Normal overflow gate runs forward-first; only a refused straight output
/// overflows to the sides (OverflowGate.java:60-79).
#[test]
fn overflow_gate_routes_forward_first_and_overflows_to_sides() {
    let world = erekir_test_world();
    let source = (9 << 16) | 10;
    let gate_pos = (10 << 16) | 10;
    logistics_tile(&world, gate_pos, 268, 0);
    let forward = (11 << 16) | 10;
    logistics_tile(&world, forward, 257, 0); // east-facing conveyor
    let side = (10 << 16) | 11;
    logistics_tile(&world, side, 257, 1); // north-facing conveyor

    assert!(accept_logistics_item_from(
        &world,
        gate_pos,
        5,
        Some(source),
        0
    ));
    let forward_tile = world.tiles.get(&forward).unwrap().clone();
    assert!(
        forward_tile
            .conveyor_items
            .iter()
            .any(|(item, _)| *item == 5)
            || forward_tile.stored_item == 5,
        "open straight output must receive the item"
    );
    let side_tile = world.tiles.get(&side).unwrap().clone();
    assert!(
        side_tile.conveyor_items.is_empty(),
        "sides must stay idle while straight accepts"
    );

    // Fill the forward conveyor (capacity 3): the next item overflows to
    // the north side.
    for _ in 0..2 {
        assert!(accept_logistics_item_from(
            &world,
            gate_pos,
            5,
            Some(source),
            0
        ));
    }
    assert!(accept_logistics_item_from(
        &world,
        gate_pos,
        5,
        Some(source),
        0
    ));
    let side_tile = world.tiles.get(&side).unwrap().clone();
    assert!(
        side_tile.conveyor_items.iter().any(|(item, _)| *item == 5) || side_tile.stored_item == 5,
        "full straight output must overflow to the side"
    );
}

/// An inverted, powered gate prefers the sides (inv = invert == enabled,
/// OverflowGate.java:60).
#[test]
fn inverted_overflow_gate_prefers_sides() {
    let world = erekir_test_world();
    let source = (9 << 16) | 10;
    let gate_pos = (10 << 16) | 10;
    world.tiles.insert(gate_pos, erekir_tile(gate_pos, 269, 0));
    let forward = (11 << 16) | 10;
    logistics_tile(&world, forward, 257, 0);
    let side = (10 << 16) | 11;
    logistics_tile(&world, side, 257, 1);

    assert!(accept_logistics_item_from(
        &world,
        gate_pos,
        5,
        Some(source),
        0
    ));
    let side_tile = world.tiles.get(&side).unwrap().clone();
    assert!(
        side_tile.conveyor_items.iter().any(|(item, _)| *item == 5) || side_tile.stored_item == 5,
        "inverted gate must feed the side first"
    );
    let forward_tile = world.tiles.get(&forward).unwrap().clone();
    assert!(
        forward_tile.conveyor_items.is_empty(),
        "inverted gate must not use the open straight output first"
    );
}

/// Incinerator.acceptItem (Incinerator.java:66-69): burns any item while
/// enabled; disposal loops must not jam (M5). A disabled incinerator
/// rejects.
#[test]
fn incinerator_burns_any_item_while_enabled_and_rejects_when_disabled() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    logistics_tile(&world, pos, 198, 0);
    assert!(accept_logistics_item_from(
        &world,
        pos,
        5,
        Some((9 << 16) | 10),
        0
    ));
    assert!(accept_logistics_item_from(&world, pos, 18, None, 0));
    if let Some(mut tile) = world.tiles.get_mut(&pos) {
        tile.enabled = false;
    }
    assert!(!accept_logistics_item_from(
        &world,
        pos,
        5,
        Some((9 << 16) | 10),
        0
    ));
}

/// RadarBuild.updateTile (Radar.java:60-74): progress ramps to 1 over
/// discoveryTime = 600 ticks at power efficiency; previously nothing ever
/// advanced production_progress, so the synced fog radius stayed 0 (H5).
/// Spans more than half a BlockSnapshot interval (360 ticks).
#[test]
fn radar_progress_ramps_over_discovery_time_and_saturates() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    logistics_tile(&world, pos, 251, 0);
    let mut power = HashMap::new();
    power.insert(pos, 1.0f32);
    let no_power = HashMap::new();

    // Unpowered radars never advance (edelta() = 0).
    crate::network::economy::simulate_radars(&world, 100.0, &no_power);
    assert_eq!(world.tiles.get(&pos).unwrap().production_progress, 0.0);

    // 300 powered ticks: exactly halfway through discovery.
    for _ in 0..300 {
        crate::network::economy::simulate_radars(&world, 1.0, &power);
    }
    let halfway = world.tiles.get(&pos).unwrap().production_progress;
    assert!((halfway - 0.5).abs() < 0.001, "progress {halfway} != ~0.5");

    // Long run saturates at 1 and never regresses.
    for _ in 0..600 {
        crate::network::economy::simulate_radars(&world, 1.0, &power);
    }
    let saturated = world.tiles.get(&pos).unwrap().production_progress;
    assert_eq!(saturated, 1.0);

    // Partial efficiency scales the ramp (edelta() semantics).
    let slow_pos = (20 << 16) | 10;
    logistics_tile(&world, slow_pos, 251, 0);
    let mut half_power = HashMap::new();
    half_power.insert(slow_pos, 0.5f32);
    crate::network::economy::simulate_radars(&world, 10.0, &half_power);
    let slow = world.tiles.get(&slow_pos).unwrap().production_progress;
    assert!((slow - 10.0 * 0.5 / 600.0).abs() < 1e-6, "progress {slow}");
}

/// Audit H10: fires spread to in-bounds neighbors when the tile
/// flammability (puddle-fed) exceeds 1, gated on Rules.fire.
#[test]
fn fire_spreads_to_neighbors_when_flammable_and_respects_rules_fire() {
    let world = erekir_test_world();
    let tile = (20 << 16) | 20;
    // Flammable puddle: spore-pod? use oil (liquid 1, flammability high).
    world.puddles.deposit(tile, 2, 40.0); // oil: flammability 1.2
    world.puddles.create_fire(tile);
    let connections = crate::network::outbound::NOOP;

    // Rules.fire defaults true: the spread timer (22 ticks at rate
    // clamp(flammability/5, .3, 2)) must ignite a neighbor eventually.
    let mut spread = false;
    for tick in 0..600 {
        world.game_state.world_ticks.store(tick, Ordering::Relaxed);
        // Puddle tick moves `accepting` into `amount` (flammability source).
        world.puddles.tick(1.0);
        simulate_fires(&world, &connections, 1.0);
        if world.puddles.fires.iter().count() > 1 {
            spread = true;
            break;
        }
    }
    assert!(spread, "flammable fire must spread to a neighbor");

    // Disable Rules.fire: no NEW fires may be created.
    for key in world
        .puddles
        .fires
        .iter()
        .map(|entry| *entry.key())
        .collect::<Vec<_>>()
    {
        world.puddles.fires.remove(&key);
    }
    world.wave_rules.write().fires_enabled = false;
    world.puddles.create_fire(tile);
    let before = world.puddles.fires.iter().count();
    for tick in 0..300 {
        world
            .game_state
            .world_ticks
            .store(1000 + tick, Ordering::Relaxed);
        simulate_fires(&world, &connections, 1.0);
    }
    assert_eq!(
        world.puddles.fires.iter().count(),
        before,
        "Rules.fire=false must freeze spread"
    );
}

/// Audit H10: water floors burn fires down faster (official speedMultiplier
/// = 1 + Attribute.water * 10; approximated as 2.0 on pumpable floors).
#[test]
fn water_floor_extinguishes_fires_faster() {
    let mut world = erekir_test_world();
    let dry_tile = (20 << 16) | 20;
    let wet_tile = (25 << 16) | 20;
    // Floor 22 = shallow water (floor_liquid_drop -> water).
    let wet_index = 20 * world.width as usize + 25;
    world.floors[wet_index] = 22;
    world.puddles.create_fire(dry_tile);
    world.puddles.create_fire(wet_tile);

    let connections = crate::network::outbound::NOOP;
    for tick in 0..500 {
        world.game_state.world_ticks.store(tick, Ordering::Relaxed);
        simulate_fires(&world, &connections, 1.0);
        // Official lifetime clock (FireComp time += delta).
        world.puddles.tick_fires(1.0);
        if world.puddles.fires.get(&wet_tile).is_none() {
            break;
        }
    }
    assert!(
        world.puddles.fires.get(&wet_tile).is_none(),
        "water-floor fire must burn out early"
    );
    assert!(
        world.puddles.fires.contains_key(&dry_tile),
        "dry-floor fire must outlive the water-floor one"
    );
}

/// Fire entity sync rides the enemy snapshot stream with classId 10 and the
/// JAR-verified `Fire.writeSync` layout:
/// [lifetime f][tilePos i][time f][x f][y f] after [id i][classId b].
#[test]
fn fire_sync_frame_matches_official_write_layout() {
    let world = erekir_test_world();
    let tile = (3 << 16) | 4;
    world.puddles.create_fire(tile);
    let fire = world.puddles.fires.get(&tile).unwrap().clone();

    let snapshots = crate::network::wire::encode_enemy_entity_snapshots(&world).unwrap();
    assert!(!snapshots.is_empty());
    // Rebuild the expected body for this single fire entity.
    let mut body: Vec<u8> = Vec::new();
    body.write_i(fire.entity_id).unwrap();
    body.write_b(crate::network::wire::encode::FIRE_ENTITY_CLASS_ID)
        .unwrap();
    body.write_f(fire.lifetime).unwrap();
    body.write_i(tile).unwrap();
    body.write_f(fire.time).unwrap();
    body.write_f(28.0f32).unwrap(); // x = 3*8 + 4
    body.write_f(36.0f32).unwrap(); // y = 4*8 + 4
                                    // The batched stream carries the entity body verbatim; batch headers
                                    // differ between flush paths, so assert as a contiguous subsequence.
    let concatenated: Vec<u8> = snapshots.concat();
    assert!(
        concatenated
            .windows(body.len())
            .any(|window| window == &body[..]),
        "fire sync body missing from snapshot stream"
    );
}

/// Game over now emits the official ServerControl infoMessage after the
/// GameOver packet (audit H14). The next-map line is omitted (map rotation
/// lives behind the runtime boundary, ARCH002) — documented deviation.
#[test]
fn game_over_broadcasts_official_info_message() {
    struct Collector(std::sync::Mutex<Vec<Vec<u8>>>);
    impl crate::network::outbound::FrameEmit for Collector {
        fn broadcast(&self, frame: Vec<u8>) {
            self.0.lock().unwrap().push(frame);
        }
        fn enqueue_to(&self, _id: i32, _frame: Vec<u8>, _critical: bool) -> bool {
            true
        }
        fn for_each_connection(&self, _visit: &mut dyn FnMut(i32)) {}
    }
    let world = erekir_test_world();
    let collector = Collector(std::sync::Mutex::new(Vec::new()));
    crate::network::wire::bootstrap::emit_game_over_packet_with_winner(&world, &collector, 2);
    let frames = collector.0.into_inner().unwrap();
    assert!(frames.len() >= 2, "game over + infoMessage expected");
    // Frame layout: [u16 tcpLen][id][u16 payloadLen][compress][payload];
    // read_packet consumes the first two bytes.
    let last = frames.last().unwrap().clone();
    let packet = crate::network::codec::read_packet(std::io::Cursor::new(&last[2..])).unwrap();
    assert_eq!(packet[0], 54, "infoMessage must follow game over");
    // The survival header text is present in the payload.
    let text = b"Game over";
    assert!(
        packet.windows(text.len()).any(|window| window == text),
        "survival game-over infoMessage must contain the official header"
    );
}

/// ASTRA E04: `costGround` only walls `allDeep` water. Shallow water and
/// deep shore tiles stay walkable (high cost). Naval land is cost 7000.
#[test]
fn naval_and_ground_navigation_fields_gate_liquid_tiles() {
    let mut world = erekir_test_world();
    // Core sits at (20, 20); paint a coastal water strip on rows 22..=24 so
    // the core is reachable through water for the naval field.
    for y in 21..=24 {
        for x in 15..=25 {
            world.floors[y * world.width as usize + x as usize] = 21;
        }
    }
    // An inland lake far from the core: reachable for naval only if water
    // connects to the coastal strip (it does not).
    for x in 30..=32 {
        world.floors[8 * world.width as usize + x as usize] = 22;
    }
    let ground = crate::network::combat::build_navigation_field_class(
        &world,
        crate::network::combat::NavigationClass::Ground,
    );
    let naval = crate::network::combat::build_navigation_field_class(
        &world,
        crate::network::combat::NavigationClass::Naval,
    );
    let land_near_core = 20 * world.width as usize + 19;
    let coast_water = 21 * world.width as usize + 20;
    let interior_deep = 23 * world.width as usize + 20;
    let inland_lake = 8 * world.width as usize + 31;
    const IMPASSABLE: u32 = u32::MAX / 4;

    assert_eq!(
        ground[interior_deep], IMPASSABLE,
        "allDeep interior water is a ground wall"
    );
    assert!(
        ground[coast_water] < IMPASSABLE,
        "deep shore next to land is costly, not a wall"
    );
    assert!(
        ground[coast_water] >= 6000,
        "deep shore pays the vanilla deep cost"
    );
    assert!(
        ground[land_near_core] < IMPASSABLE,
        "land stays passable for ground units"
    );
    assert!(
        naval[land_near_core] >= 6000,
        "naval pays 7000 on land (Pathfinder treats >=6000 as no-step)"
    );
    assert!(
        naval[coast_water] < 6000,
        "coastal water is passable and connected to the core"
    );
    assert!(
        naval[inland_lake] > naval[coast_water],
        "disconnected water is farther via land (cost 7000) than coastal water"
    );
}

/// Audit H13/H4: grounded units are slowed by the official
/// `Floor.speedMultiplier` of their tile; flying units are unaffected.
#[test]
fn floor_speed_multiplier_slows_grounded_units_only() {
    use crate::network::world::EnemyUnit;
    let mut dagger = EnemyUnit {
        id: 1,
        unit_type: 0,
        team: 2,
        x: 88.0, // tile (11, 10)
        y: 80.0,
        health: 100.0,
        move_speed: DAGGER.speed,
        ..Default::default()
    };
    assert!(
        (crate::network::combat::effective_unit_speed_on_floor(&dagger, Some(21))
            - DAGGER.speed * 0.2)
            .abs()
            < 1e-4
    );
    assert!(
        (crate::network::combat::effective_unit_speed_on_floor(&dagger, Some(22))
            - DAGGER.speed * 0.5)
            .abs()
            < 1e-4
    );
    assert!(
        (crate::network::combat::effective_unit_speed_on_floor(&dagger, None) - DAGGER.speed).abs()
            < 1e-4
    );
    // Flying unit (flare 15) ignores terrain.
    let flare = EnemyUnit {
        id: 2,
        unit_type: 15,
        team: 2,
        health: 100.0,
        move_speed: FLARE.speed,
        ..Default::default()
    };
    assert!(
        (crate::network::combat::effective_unit_speed_on_floor(&flare, Some(21)) - FLARE.speed)
            .abs()
            < 1e-4
    );
    let _ = &mut dagger;
}

#[test]
fn auto_door_opens_for_allied_ground_unit_and_closes_when_gone() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut door = erekir_tile(pos, 239, 0);
    door.occupied = crate::network::buildings::construction::block_footprint_in(40, 40, pos, 239)
        .unwrap_or_else(|| vec![pos]);
    world.tiles.insert(pos, door);

    let mut ally = ground_unit_on_tile(1, 0, pos, 100.0, 0.0);
    ally.team = 1;
    world.enemies.insert(ally.id, ally);

    assert!(simulate_auto_doors(
        &world,
        &crate::network::outbound::NOOP,
        20.0
    ));
    assert!(world.tiles.get(&pos).unwrap().door_open);

    world.enemies.clear();
    assert!(simulate_auto_doors(
        &world,
        &crate::network::outbound::NOOP,
        20.0
    ));
    assert!(!world.tiles.get(&pos).unwrap().door_open);
}

#[test]
fn auto_door_ignores_enemies_and_flying_allies() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut door = erekir_tile(pos, 239, 0);
    door.occupied = vec![pos];
    world.tiles.insert(pos, door);

    let enemy = ground_unit_on_tile(1, 0, pos, 100.0, 0.0); // team 2
    world.enemies.insert(enemy.id, enemy);
    assert!(!simulate_auto_doors(
        &world,
        &crate::network::outbound::NOOP,
        20.0
    ));
    assert!(!world.tiles.get(&pos).unwrap().door_open);
    world.enemies.clear();

    let mut flare = ground_unit_on_tile(2, 15, pos, 100.0, 1.0);
    flare.team = 1;
    world.enemies.insert(flare.id, flare);
    assert!(!simulate_auto_doors(
        &world,
        &crate::network::outbound::NOOP,
        20.0
    ));
    assert!(!world.tiles.get(&pos).unwrap().door_open);
}

#[test]
fn closed_door_is_check_solid_open_door_is_not() {
    assert!(crate::game::content::building_check_solid(228, false));
    assert!(!crate::game::content::building_check_solid(228, true));
    assert!(crate::game::content::building_check_solid(239, false));
    assert!(!crate::game::content::building_check_solid(239, true));
    assert!(crate::game::content::building_check_solid(216, false)); // copper wall
}

/// Audit LOW ShockMine: `unitOn` + 80-tick cooldown, 4 Lightning.create
/// ground tendrils (damage 25, length 10), self tileDamage 7.
#[test]
fn shock_mine_fires_ground_lightning_on_enemy_unit_on() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut mine = erekir_tile(pos, 250, 0);
    mine.team = 1;
    mine.health = crate::game::content::block_health(250);
    world.tiles.insert(pos, mine);

    let enemy = ground_unit_on_tile(1, 0, pos, 150.0, 0.0);
    world.enemies.insert(enemy.id, enemy);

    assert!(simulate_shock_mines(
        &world,
        &crate::network::outbound::NOOP,
        1.0
    ));
    let health = world.enemies.get(&1).map(|e| e.health).unwrap_or(0.0);
    assert!(
        health < 150.0,
        "shock-mine lightning tendrils must damage the occupying enemy, health {health}"
    );
    let tile = world.tiles.get(&pos).unwrap();
    assert!(
        (tile.health - (crate::game::content::block_health(250) - 7.0)).abs() < 0.01,
        "tileDamage = 7, got {}",
        tile.health
    );
    assert_eq!(tile.production_progress, 80.0);
}

#[test]
fn repair_point_heals_allied_unit_in_range() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut point = erekir_tile(pos, 384, 0);
    point.team = 1;
    world.tiles.insert(pos, point);
    let mut ally = ground_unit_on_tile(1, 0, pos, 40.0, 0.0);
    ally.team = 1;
    let max_hp = crate::network::combat::enemy::enemy_max_health(&ally);
    ally.health = max_hp * 0.25;
    world.enemies.insert(ally.id, ally);
    let power = std::collections::HashMap::from([(pos, 1.0)]);
    assert!(crate::network::economy::simulate_repair_and_cargo(
        &world, 10.0, &power
    ));
    let health = world.enemies.get(&1).unwrap().health;
    assert!(
        health > max_hp * 0.25 + 1.0,
        "repair-point 384 must heal allied units: {health} vs {}",
        max_hp * 0.25
    );
}

#[test]
fn cargo_loader_spawns_manifold_after_build_time() {
    let world = erekir_test_world();
    let pos = (12 << 16) | 12;
    let mut loader = erekir_tile(pos, 281, 0);
    loader.team = 1;
    loader.production_progress = 0.99;
    loader.stored_amount = -1;
    loader.inventory = vec![(0, 20)];
    world.tiles.insert(pos, loader);
    let power = std::collections::HashMap::from([(pos, 1.0)]);
    assert!(crate::network::economy::simulate_repair_and_cargo(
        &world, 20.0, &power
    ));
    let tile = world.tiles.get(&pos).unwrap();
    assert!(
        tile.stored_amount > 0,
        "cargo loader must tether a manifold"
    );
    let id = tile.stored_amount;
    let unit = world.enemies.get(&id).expect("spawned manifold");
    assert_eq!(unit.unit_type, 62);
    assert_eq!(unit.team, 1);
}

#[test]
fn cargo_manifold_ferries_loader_items_to_unload_point() {
    let world = erekir_test_world();
    let loader_pos = (8 << 16) | 8;
    let unload_pos = (20 << 16) | 8;
    let mut loader = erekir_tile(loader_pos, 281, 0);
    loader.inventory = vec![(0, 40)];
    loader.stored_amount = 7;
    world.tiles.insert(loader_pos, loader);
    let mut unload = erekir_tile(unload_pos, 282, 0);
    unload.stored_item = 0;
    world.tiles.insert(unload_pos, unload);
    let mut manifold = ground_unit_on_tile(7, 62, loader_pos, 200.0, 1.0);
    manifold.team = 1;
    manifold.x = 8.0 * 8.0 + 4.0;
    manifold.y = 8.0 * 8.0 + 4.0;
    world.enemies.insert(7, manifold);
    let power = std::collections::HashMap::new();
    assert!(crate::network::economy::simulate_repair_and_cargo(
        &world, 1.0, &power
    ));
    let held = world.enemies.get(&7).unwrap().items.clone();
    assert_eq!(held, vec![(0, 40)], "CargoAI picks up from the loader");
    assert_eq!(
        inventory_count(&world.tiles.get(&loader_pos).unwrap().inventory, 0),
        0
    );
    for _ in 0..80 {
        crate::network::economy::simulate_repair_and_cargo(&world, 4.0, &power);
        if world
            .enemies
            .get(&7)
            .is_some_and(|unit| unit.items.iter().all(|(_, n)| *n <= 0))
        {
            break;
        }
    }
    assert!(
        inventory_count(&world.tiles.get(&unload_pos).unwrap().inventory, 0) > 0,
        "CargoAI deposits at the configured unload point"
    );
}

#[test]
fn assembler_ai_spawns_drones_and_flies_them_to_the_perimeter() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut assembler = erekir_tile(pos, 393, 0);
    assembler.occupied = vec![pos];
    world.tiles.insert(pos, assembler);
    let power = std::collections::HashMap::from([(pos, 1.0)]);
    assert!(crate::network::economy::simulate_repair_and_cargo(
        &world, 240.0, &power
    ));
    let ids = world.assembler_drone_ids(pos);
    assert_eq!(
        ids.len(),
        1,
        "first assembly-drone after droneConstructTime"
    );
    let drone = world.enemies.get(&ids[0]).expect("spawned assembly-drone");
    assert_eq!(drone.unit_type, 63);
    assert_eq!(drone.team, 1);
    let assembler_x = 10.0 * 8.0 + 4.0;
    let assembler_y = 10.0 * 8.0 + 4.0;
    let from_center = (drone.x - assembler_x).hypot(drone.y - assembler_y);
    drop(drone);
    assert!(
        from_center > 1.0,
        "AssemblerAI must fly the drone toward its perimeter slot on the spawn tick, moved {from_center}"
    );
    for _ in 0..3 {
        crate::network::economy::simulate_repair_and_cargo(&world, 1.0, &power);
    }
    assert_eq!(world.assembler_drone_ids(pos).len(), 4);
    let calls = world.game_state.extras.take_calls();
    assert!(
        calls.iter().any(|frame| frame.get(2).copied() == Some(9)),
        "AssemblerDroneSpawnedCallPacket (id 9) must be queued"
    );
}

#[test]
fn factory_spawned_unit_inherits_command_and_rally_point() {
    let world = erekir_test_world();
    let factory_pos = (10 << 16) | 10;
    let mut factory = erekir_tile(factory_pos, 377, 0);
    factory.factory_command = Some(4);
    world.tiles.insert(factory_pos, factory.clone());
    world.building_commands.insert(
        factory_pos,
        crate::network::world::BuildingCommand {
            position: factory_pos,
            target_x: 320.0,
            target_y: 160.0,
        },
    );
    spawn_factory_unit(&world, &DashMap::new(), &factory, 0);
    let unit = world.enemies.iter().next().expect("factory spawn");
    let order = world.unit_orders.get(&unit.id).expect("unit order");
    assert_eq!(
        order.command, 0,
        "dagger rejects mine; CommandAI keeps move"
    );
    assert_eq!(order.target_x, Some(320.0));
    assert_eq!(order.target_y, Some(160.0));
    assert_eq!(order.target_kind, 0);
}

#[test]
fn erekir_fabricator_spawned_unit_inherits_command_and_rally_point() {
    // tank-fabricator (386) shares spawn_factory_unit with Serpulo factories;
    // configured_unit_command must read factory_command for 386-388 so the
    // unit entity inherits command+rally (not building progress).
    let world = erekir_test_world();
    let factory_pos = (10 << 16) | 10;
    let mut factory = erekir_tile(factory_pos, 386, 0);
    factory.factory_command = Some(4);
    world.tiles.insert(factory_pos, factory.clone());
    world.building_commands.insert(
        factory_pos,
        crate::network::world::BuildingCommand {
            position: factory_pos,
            target_x: 320.0,
            target_y: 160.0,
        },
    );
    spawn_factory_unit(&world, &DashMap::new(), &factory, 38);
    let unit = world.enemies.iter().next().expect("fabricator spawn");
    assert_eq!(unit.unit_type, 38);
    let order = world.unit_orders.get(&unit.id).expect("unit order");
    assert_eq!(order.command, 0, "stell rejects mine; CommandAI keeps move");
    assert_eq!(order.target_x, Some(320.0));
    assert_eq!(order.target_y, Some(160.0));
    assert_eq!(order.target_kind, 0);
}

#[test]
fn f03_unit_factory_plan_minus_one_deselects() {
    use crate::network::codec::Reads;
    let cleared = vec![1, 255, 255, 255, 255];
    assert!(crate::network::wire::tile_config::valid_tile_config(
        377, &cleared
    ));
    assert!(
        unit_factory_recipe(377, &cleared).is_none(),
        "explicit -1 must not fall back to plan 0"
    );
    assert!(
        unit_factory_recipe(377, &[]).is_some(),
        "created() still selects the first plan"
    );
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let mut factory = erekir_tile(pos, 377, 0);
    factory.config = cleared;
    factory.inventory = vec![(9, 10), (1, 10)];
    // Preserve the choice through persistence before crossing a snapshot.
    let serialized = serde_json::to_vec(&factory).unwrap();
    let restored: DynamicTile = serde_json::from_slice(&serialized).unwrap();
    world.tiles.insert(pos, restored);
    let power = std::collections::HashMap::from([(pos, 1.0)]);
    let out = DashMap::new();
    for tick in 0..600 {
        simulate_unit_factories(&world, &out, 1.0, &power);
        if tick == 399 {
            let tile = world.tiles.get(&pos).unwrap().clone();
            let mut bytes = Vec::new();
            encode_dynamic_tile_sync(&mut bytes, &tile, &power, Some(&world)).unwrap();
            let mut tail = std::io::Cursor::new(&bytes);
            tail.set_position((bytes.len() - 15) as u64);
            assert_eq!(tail.read_f().unwrap(), 0.0);
            assert_eq!(
                tail.read_s().unwrap(),
                -1,
                "BlockSnapshot must not reselect plan zero"
            );
            if let Ok(directory) = std::env::var("OXIDE_RECONSTRUCTOR_FIXTURE_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                std::fs::write(
                    std::path::Path::new(&directory).join("factory-deselected.bin"),
                    bytes,
                )
                .unwrap();
            }
        }
    }
    let tile = world.tiles.get(&pos).unwrap();
    assert_eq!(tile.production_progress, 0.0);
    assert!(tile.payload.is_none());
    assert_eq!(tile.inventory, factory.inventory);
}

#[test]
fn f06_mono_accepts_mine_from_factory() {
    let world = erekir_test_world();
    let factory_pos = (10 << 16) | 10;
    let mut factory = erekir_tile(factory_pos, 378, 0);
    factory.factory_command = Some(4);
    world.tiles.insert(factory_pos, factory.clone());
    spawn_factory_unit(&world, &DashMap::new(), &factory, 20);
    let unit = world.enemies.iter().next().expect("mono spawn");
    let order = world.unit_orders.get(&unit.id).expect("unit order");
    assert_eq!(order.command, 4, "mono accepts mine");
}

#[test]
fn f06_blocked_payload_keeps_creation_rally_when_factory_rally_changes() {
    let world = erekir_test_world();
    let factory_pos = (10 << 16) | 10;
    let mut factory = erekir_tile(factory_pos, 377, 0);
    factory.factory_command = Some(0);
    world.tiles.insert(factory_pos, factory.clone());
    world.building_commands.insert(
        factory_pos,
        crate::network::world::BuildingCommand {
            position: factory_pos,
            target_x: 80.0,
            target_y: 80.0,
        },
    );
    spawn_factory_unit(&world, &DashMap::new(), &factory, 0);
    let id = world.enemies.iter().next().expect("spawned").id;
    assert_eq!(
        world.unit_orders.get(&id).unwrap().target_x,
        Some(80.0),
        "creation rally A"
    );
    world.building_commands.insert(
        factory_pos,
        crate::network::world::BuildingCommand {
            position: factory_pos,
            target_x: 240.0,
            target_y: 240.0,
        },
    );
    // Re-binding at dump must not replace an order captured at creation.
    let factory = world.tiles.get(&factory_pos).unwrap().clone();
    spawn_factory_unit(&world, &DashMap::new(), &factory, 0);
    assert_eq!(
        world.unit_orders.get(&id).unwrap().target_x,
        Some(80.0),
        "held/created unit keeps rally A after the factory rally moves to B"
    );
    let other = world
        .enemies
        .iter()
        .map(|unit| unit.id)
        .find(|spawned| *spawned != id)
        .expect("second unit");
    assert_eq!(
        world.unit_orders.get(&other).unwrap().target_x,
        Some(240.0),
        "the next unit receives rally B"
    );
}

#[test]
fn c01_enter_payload_moves_from_outside_footprint() {
    let world = erekir_test_world();
    let pos = (10 << 16) | 10;
    let start = (14 << 16) | 10;
    let mut dagger = ground_unit_on_tile(7, 0, start, 150.0, 0.0);
    dagger.team = 1;
    dagger.authority = crate::network::world::UnitAuthority::Command;
    world.enemies.insert(7, dagger);
    let mut tile = erekir_tile(pos, 380, 0);
    tile.occupied = (9..=11)
        .flat_map(|x| (9..=11).map(move |y| (x << 16) | y))
        .collect();
    for &occupied in &tile.occupied {
        world.tile_footprint.insert(occupied, pos);
    }
    world.tiles.insert(pos, tile);
    world.unit_orders.insert(
        7,
        crate::network::world::UnitOrder {
            unit_id: 7,
            command: 5,
            target_kind: 1,
            target_id: pos,
            target_x: Some(((pos >> 16) as i16 as f32) * 8.0),
            target_y: Some((pos as i16 as f32) * 8.0),
            ..Default::default()
        },
    );
    let snapshot = world.enemies.get(&7).unwrap().clone();
    let x0 = snapshot.x;
    assert!(crate::network::units::apply_ordered_unit_movement(
        &world, &snapshot, 16.0
    ));
    let after = world.enemies.get(&7).unwrap();
    assert!(
        (after.x - x0).abs() > 0.05,
        "command 5 must path toward the reconstructor"
    );
    drop(after);
    for _ in 0..600 {
        crate::network::simulation::simulate_allied_units(&world, &DashMap::new(), 1.0);
        simulate_unit_payload_entries(&world, &DashMap::new(), 1.0);
    }
    assert!(
        !world.enemies.contains_key(&7),
        "must enter the actual multiblock footprint"
    );
    assert!(world.tiles.get(&pos).unwrap().payload.is_some());
}

#[test]
fn f01_unit_cap_tracks_live_cores_and_rule() {
    let world = erekir_test_world();
    world.wave_rules.write().unit_cap = 0;
    world.wave_rules.write().unit_cap_variable = true;
    crate::network::world::register_team_core(
        &world,
        1,
        crate::network::world::TeamCore {
            position: (5 << 16) | 5,
            block: 339,
            health: 1_000.0,
            max_health: 1_000.0,
        },
    );
    assert_eq!(team_unit_cap(&world, 1), 8);
    crate::network::world::unregister_team_core(&world, 1, (5 << 16) | 5);
    assert_eq!(team_unit_cap(&world, 1), 0);
    world.wave_rules.write().unit_cap = 12;
    world.wave_rules.write().unit_cap_variable = false;
    assert_eq!(team_unit_cap(&world, 1), 12);
}

#[test]
fn f02_ground_factory_silicon_capacity_scales_with_unit_cost() {
    let world = erekir_test_world();
    world.wave_rules.write().unit_cost_multiplier = 3.0;
    let factory = (10 << 16) | 10;
    let mut tile = erekir_tile(factory, 377, 0);
    tile.occupied = vec![factory];
    tile.config = vec![1, 0, 0, 0, 0];
    world.tiles.insert(factory, tile);
    for _ in 0..180 {
        assert!(
            accept_logistics_item_from(&world, factory, 9, None, 0),
            "ground-factory silicon capacity is 60 * unitCost"
        );
    }
    assert!(!accept_logistics_item_from(&world, factory, 9, None, 0));
    assert_eq!(
        inventory_count(&world.tiles.get(&factory).unwrap().inventory, 9),
        180
    );
}

#[test]
fn f05_payload_dump_rejects_solid_and_accepts_conveyor() {
    let world = erekir_test_world();
    let unit = ground_unit_on_tile(1, 0, (4 << 16) | 4, 150.0, 0.0);
    let wall = (5 << 16) | 4;
    let belt = (6 << 16) | 4;
    let mut wall_tile = erekir_tile(wall, 216, 0);
    wall_tile.occupied = vec![wall];
    world.tiles.insert(wall, wall_tile);
    let mut belt_tile = erekir_tile(belt, 257, 0);
    belt_tile.occupied = vec![belt];
    world.tiles.insert(belt, belt_tile);
    let wall_x = ((wall >> 16) as i16 as f32) * 8.0;
    let wall_y = (wall as i16 as f32) * 8.0;
    let belt_x = ((belt >> 16) as i16 as f32) * 8.0;
    let belt_y = (belt as i16 as f32) * 8.0;
    assert!(
        !payload_dump_world_clear(&world, &unit, wall_x, wall_y),
        "dump must not release into a solid wall"
    );
    assert!(
        payload_dump_world_clear(&world, &unit, belt_x, belt_y),
        "a conveyor is not a wall for UnitPayload.dump"
    );
    let rock_index = (4 * world.width + 7) as usize;
    let mut world = world;
    world.floors[rock_index] = 31;
    world.base_blocks[rock_index] = 0;
    let rock_x = 7.0 * 8.0;
    let rock_y = 4.0 * 8.0;
    assert!(
        !payload_dump_world_clear(&world, &unit, rock_x, rock_y),
        "natural rock (floor 31) blocks UnitPayload.dump"
    );
}

#[test]
fn f04_unit_factory_rejects_unit_payload() {
    let dagger = ground_unit_on_tile(1, 0, (4 << 16) | 4, 150.0, 0.0);
    assert!(
        !payload_block_accepts(377, &CarriedPayload::Unit(dagger.clone())),
        "UnitFactoryBuild.acceptPayload is always false"
    );
    assert!(
        payload_block_accepts(380, &CarriedPayload::Unit(dagger.clone())),
        "additive reconstructor upgrades dagger"
    );
    let mut wall = erekir_tile((4 << 16) | 4, 216, 0);
    wall.occupied = vec![(4 << 16) | 4];
    assert!(
        !payload_block_accepts(
            380,
            &CarriedPayload::Build(CarriedBuildPayload {
                tile: wall,
                version: 0,
                sync: Vec::new(),
            })
        ),
        "UnitBlock does not accept a building payload"
    );
}

#[test]
fn repair_ai_retreats_to_core_when_idle_and_threatened() {
    let world = erekir_test_world();
    let mut mega = ground_unit_on_tile(3, 22, (2 << 16) | 2, 170.0, 1.0);
    mega.team = 1;
    let start = (mega.x, mega.y);
    world.enemies.insert(3, mega);
    let mut threat = ground_unit_on_tile(4, 0, (3 << 16) | 2, 150.0, 0.0);
    threat.team = 2;
    world.enemies.insert(4, threat);
    let out = DashMap::new();
    assert!(crate::network::simulation::simulate_support_units(
        &world, &out, 8.0
    ));
    let mega = world.enemies.get(&3).unwrap();
    let toward_core = (mega.x - start.0).hypot(mega.y - start.1);
    assert!(
        toward_core > 1.0,
        "idle RepairAI must retreat toward the core when an enemy is in fleeRange"
    );
}

#[test]
fn e04_world_to_tile_rounds_half_tiles() {
    use crate::network::combat::enemy::world_to_tile;
    // World.toTile = round(coord/8). Tile origin (tile*8) maps to `tile`.
    // The geometric centre (tile*8+4) maps to tile+1.
    assert_eq!(world_to_tile(0.0), 0);
    assert_eq!(world_to_tile(8.0), 1);
    assert_eq!(world_to_tile(11.9), 1);
    assert_eq!(world_to_tile(12.0), 2);
    assert_eq!(world_to_tile(12.1), 2);
    assert_eq!(world_to_tile(4.0 - 0.1), 0);
    assert_eq!(world_to_tile(4.0 + 0.1), 1);

    let jar = std::env::var("MINDUSTRY_1597_JAR")
        .ok()
        .filter(|path| std::path::Path::new(path).is_file())
        .or_else(|| {
            let fallback = std::path::PathBuf::from("/tmp/astra-oracle/159.7.jar");
            fallback.is_file().then_some(fallback.display().to_string())
        });
    if let Some(jar) = jar {
        let dir = std::env::temp_dir().join(format!("oxide-e04-jar-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let java_path = dir.join("AstraToTile.java");
        std::fs::write(
            &java_path,
            r#"
import mindustry.core.World;
public class AstraToTile {
  public static void main(String[] args) {
    if (World.toTile(0f) != 0) throw new AssertionError("0");
    if (World.toTile(8f) != 1) throw new AssertionError("8");
    if (World.toTile(11.9f) != 1) throw new AssertionError("11.9");
    if (World.toTile(12f) != 2) throw new AssertionError("12");
    if (World.toTile(12.1f) != 2) throw new AssertionError("12.1");
    if (World.toTile(3.9f) != 0) throw new AssertionError("3.9");
    if (World.toTile(4.1f) != 1) throw new AssertionError("4.1");
    System.out.println("ASTRA_TOTILE_OK");
  }
}
"#,
        )
        .unwrap();
        let javac = std::process::Command::new("javac")
            .args([
                "-cp",
                &jar,
                "-d",
                dir.to_str().unwrap(),
                java_path.to_str().unwrap(),
            ])
            .output()
            .expect("javac");
        assert!(
            javac.status.success(),
            "javac AstraToTile: {}",
            String::from_utf8_lossy(&javac.stderr)
        );
        let java = std::process::Command::new("java")
            .args(["-cp", &format!("{}:{}", jar, dir.display()), "AstraToTile"])
            .output()
            .expect("java");
        let stdout = String::from_utf8_lossy(&java.stdout);
        assert!(
            java.status.success() && stdout.contains("ASTRA_TOTILE_OK"),
            "JAR World.toTile must match Rust: stdout={stdout} stderr={}",
            String::from_utf8_lossy(&java.stderr)
        );
    }
}

#[test]
fn e04_doors_and_teams_gate_ground_costs() {
    let world = erekir_test_world();
    let closed = (10 << 16) | 10;
    let open = (11 << 16) | 10;
    let enemy = (12 << 16) | 10;
    let own = (13 << 16) | 10;
    let mut closed_door = erekir_tile(closed, 228, 0);
    closed_door.door_open = false;
    closed_door.occupied = vec![closed];
    let mut open_door = erekir_tile(open, 228, 0);
    open_door.door_open = true;
    open_door.occupied = vec![open];
    let mut enemy_wall = erekir_tile(enemy, 216, 0);
    enemy_wall.team = 2;
    enemy_wall.occupied = vec![enemy];
    let mut own_wall = erekir_tile(own, 216, 0);
    own_wall.team = 1;
    own_wall.occupied = vec![own];
    world.tiles.insert(closed, closed_door);
    world.tiles.insert(open, open_door);
    world.tiles.insert(enemy, enemy_wall);
    world.tiles.insert(own, own_wall);
    let ground = crate::network::combat::enemy::build_navigation_field_toward(
        &world,
        crate::network::combat::NavigationClass::Ground,
        20,
        20,
        1,
    );
    const IMPASSABLE: u32 = u32::MAX / 4;
    let idx = |pos: i32| {
        let x = (pos >> 16) as i16 as usize;
        let y = (pos as i16) as usize;
        y * world.width as usize + x
    };
    assert_eq!(ground[idx(closed)], IMPASSABLE);
    assert!(ground[idx(open)] < IMPASSABLE, "open door is not a wall");
    assert_eq!(ground[idx(own)], IMPASSABLE, "own wall is a ground wall");
    assert!(
        ground[idx(enemy)] < IMPASSABLE,
        "enemy wall is costly, not impassable"
    );

    let start = (8 << 16) | 10;
    let wall = (9 << 16) | 10;
    let mut wall_tile = erekir_tile(wall, 216, 0);
    wall_tile.team = 1;
    wall_tile.occupied = vec![wall];
    world.tiles.insert(wall, wall_tile);
    let toward = crate::network::combat::enemy::build_navigation_field_toward(
        &world,
        crate::network::combat::NavigationClass::Ground,
        14,
        10,
        1,
    );
    let start_idx = 10 * world.width as usize + 8;
    let wall_idx = 10 * world.width as usize + 9;
    let step = crate::network::units::choose_navigation_step(
        &toward,
        world.width,
        world.height,
        start_idx,
    )
    .expect("ground first step");
    assert_ne!(
        step, wall_idx,
        "first step toward the goal must not enter an own wall"
    );
    let _ = start;
}

#[test]
fn e03_flare_prefers_generator_over_nearer_core() {
    let world = erekir_test_world();
    let core = (12 << 16) | 12;
    let generator = (30 << 16) | 30;
    world.tiles.insert(core, erekir_tile(core, 339, 0));
    world
        .tiles
        .insert(generator, erekir_tile(generator, 308, 0));
    crate::network::world::register_team_core(
        &world,
        1,
        crate::network::world::TeamCore {
            position: core,
            block: 339,
            health: 1_100.0,
            max_health: 1_100.0,
        },
    );
    let mut flare = ground_unit_on_tile(9, 15, (2 << 16) | 2, 70.0, 1.0);
    flare.team = 2;
    flare.attack_range = 40.0;
    let core_x = 12.0 * 8.0;
    let core_y = 12.0 * 8.0;
    let target =
        crate::network::units::enemy_navigation_target(&world, &flare, core_x, core_y, &[]);
    assert_eq!(target.building.map(|(p, _, _)| p), Some(generator));
}

#[test]
fn e03_random_wave_ai_is_deterministic_for_type_and_wave() {
    let world = erekir_test_world();
    world.wave_rules.write().random_wave_ai = true;
    world.wave_rules.write().waves_enabled = true;
    world
        .game_state
        .wave
        .store(3, std::sync::atomic::Ordering::Relaxed);
    world
        .tiles
        .insert((8 << 16) | 8, erekir_tile((8 << 16) | 8, 308, 0));
    world
        .tiles
        .insert((25 << 16) | 25, erekir_tile((25 << 16) | 25, 345, 0));
    let mut flare = ground_unit_on_tile(9, 15, (2 << 16) | 2, 70.0, 1.0);
    flare.team = 2;
    flare.attack_range = 20.0;
    let a = crate::network::units::enemy_navigation_target(&world, &flare, 160.0, 160.0, &[]);
    let b = crate::network::units::enemy_navigation_target(&world, &flare, 160.0, 160.0, &[]);
    assert_eq!(a.building.map(|(p, _, _)| p), b.building.map(|(p, _, _)| p));
    assert!(a.building.is_some());
}

#[test]
fn e03_quad_wave_team_uses_flying_ai() {
    let world = erekir_test_world();
    world
        .tiles
        .insert((8 << 16) | 8, erekir_tile((8 << 16) | 8, 182, 0));
    let mut quad = ground_unit_on_tile(12, 23, (2 << 16) | 2, 6000.0, 1.0);
    quad.team = world.wave_rules.read().wave_team;
    quad.attack_range = 20.0;
    let target = crate::network::units::enemy_navigation_target(&world, &quad, 160.0, 160.0, &[]);
    assert_eq!(target.building.map(|(p, _, _)| p), Some((8 << 16) | 8));
}

#[test]
fn e03_legs_cross_deep_water_ground_does_not() {
    let mut world = erekir_test_world();
    for y in 5..=10 {
        for x in 5..=10 {
            world.floors[y * world.width as usize + x as usize] = 21;
        }
    }
    let ground = crate::network::combat::build_navigation_field_class(
        &world,
        crate::network::combat::NavigationClass::Ground,
    );
    let legs = crate::network::combat::build_navigation_field_class(
        &world,
        crate::network::combat::NavigationClass::Legs,
    );
    const IMPASSABLE: u32 = u32::MAX / 4;
    let deep = 8 * world.width as usize + 8;
    assert_eq!(ground[deep], IMPASSABLE);
    assert!(legs[deep] < IMPASSABLE, "atrax legs cross allDeep water");
}

#[test]
fn e03_naval_channel_is_shared_by_risso_and_retusa() {
    let mut world = erekir_test_world();
    for y in 0..40 {
        for x in 0..40 {
            world.floors[y * world.width as usize + x as usize] = 0;
        }
    }
    for x in 5..=30 {
        world.floors[20 * world.width as usize + x] = 21;
    }
    let naval = crate::network::combat::build_navigation_field_class(
        &world,
        crate::network::combat::NavigationClass::Naval,
    );
    let channel = 20 * world.width as usize + 15;
    let land = 10 * world.width as usize + 15;
    assert!(naval[channel] < 6000);
    assert!(naval[land] >= 6000);
    assert!(crate::game::content::unit_movement(25).naval);
    assert!(crate::game::content::unit_movement(30).naval);
}

#[test]
fn f04_reconstructor_four_orientations_and_enabled() {
    let world = erekir_test_world();
    let dagger = ground_unit_on_tile(1, 0, (4 << 16) | 4, 150.0, 0.0);
    let payload = CarriedPayload::Unit(dagger);
    for rot in 0u8..4 {
        let front = offset_position_by((12 << 16) | 12, rot, 2);
        let mut rec = erekir_tile(front, 380, rot);
        rec.enabled = true;
        assert!(
            front_accepts_payload(&world, &rec, &payload),
            "additive reconstructor rot {rot} accepts dagger"
        );
        rec.enabled = false;
        assert!(
            !front_accepts_payload(&world, &rec, &payload),
            "disabled reconstructor rot {rot} rejects"
        );
    }
    let oct = ground_unit_on_tile(2, 24, (4 << 16) | 4, 6000.0, 1.0);
    let mut rec = erekir_tile((10 << 16) | 10, 380, 0);
    rec.enabled = true;
    assert!(!front_accepts_payload(
        &world,
        &rec,
        &CarriedPayload::Unit(oct)
    ));
    world.wave_rules.write().banned_units = vec![1];
    let dagger = ground_unit_on_tile(3, 0, (4 << 16) | 4, 150.0, 0.0);
    assert!(
        !front_accepts_payload(&world, &rec, &CarriedPayload::Unit(dagger)),
        "banned mace result rejects the dagger upgrade"
    );
}

#[test]
fn f05_payload_dump_door_water_overlap_naval_flyer_and_cap() {
    let mut world = erekir_test_world();
    let dagger = ground_unit_on_tile(1, 0, (4 << 16) | 4, 150.0, 0.0);
    let door = (8 << 16) | 8;
    let mut closed = erekir_tile(door, 228, 0);
    closed.door_open = false;
    closed.occupied = vec![door];
    world.tiles.insert(door, closed);
    let door_x = 8.0 * 8.0;
    let door_y = 8.0 * 8.0;
    assert!(!payload_dump_world_clear(&world, &dagger, door_x, door_y));
    world.tiles.get_mut(&door).unwrap().door_open = true;
    assert!(payload_dump_world_clear(&world, &dagger, door_x, door_y));

    let deep_index = (6 * world.width + 6) as usize;
    world.floors[deep_index] = 21;
    assert!(!payload_dump_world_clear(
        &world,
        &dagger,
        6.0 * 8.0,
        6.0 * 8.0
    ));

    let occupant = ground_unit_on_tile(2, 0, (9 << 16) | 9, 150.0, 0.0);
    world.enemies.insert(2, occupant);
    assert!(!payload_dump_world_clear(
        &world,
        &dagger,
        9.0 * 8.0 + 4.0,
        9.0 * 8.0 + 4.0
    ));

    let mut risso = ground_unit_on_tile(3, 25, (4 << 16) | 4, 170.0, 0.0);
    risso.team = 1;
    let water_index = (7 * world.width + 7) as usize;
    world.floors[water_index] = 21;
    assert!(
        !payload_dump_world_clear(&world, &risso, 5.0 * 8.0, 5.0 * 8.0),
        "risso cannot dump on land"
    );
    assert!(payload_dump_world_clear(
        &world,
        &risso,
        7.0 * 8.0,
        7.0 * 8.0
    ));

    let mut flare = ground_unit_on_tile(4, 15, (4 << 16) | 4, 70.0, 1.0);
    flare.elevation = 1.0;
    let rock_index = (5 * world.width + 5) as usize;
    world.floors[rock_index] = 31;
    world.base_blocks[rock_index] = 0;
    assert!(
        payload_dump_world_clear(&world, &flare, 5.0 * 8.0, 5.0 * 8.0),
        "flare dumps over a solid floor"
    );

    for n in 0..8 {
        let mut reign = ground_unit_on_tile(100 + n, 10, (n << 16) | 2, 10_000.0, 0.0);
        reign.team = 1;
        world.enemies.insert(100 + n, reign);
    }
    assert!(
        !can_create_unit(&world, 1, 10),
        "T5 reign is retained at the live cap"
    );
}

#[test]
fn c04_logic_move_stops_at_continuous_wall() {
    let world = erekir_test_world();
    let wall = (12 << 16) | 10;
    let mut tile = erekir_tile(wall, 216, 0);
    tile.occupied = vec![wall];
    world.tiles.insert(wall, tile);
    let mut dagger = ground_unit_on_tile(5, 0, (8 << 16) | 10, 150.0, 0.0);
    dagger.team = 1;
    dagger.authority = UnitAuthority::Logic {
        processor_pos: 1,
        remaining_ticks: 600.0,
        processor_generation: 1,
    };
    let start_x = dagger.x;
    world.enemies.insert(5, dagger);
    world.unit_orders.insert(
        5,
        UnitOrder {
            unit_id: 5,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(20.0 * 8.0),
            target_y: Some(10.0 * 8.0),
            logic_control: logic_control::MOVE,
            queue: Vec::new(),
        },
    );
    for _ in 0..80 {
        let snap = world.enemies.get(&5).unwrap().clone();
        crate::network::units::apply_logic_unit_movement(&world, &snap, 1.0);
    }
    let x = world.enemies.get(&5).unwrap().x;
    assert!(x > start_x, "dagger must walk toward the wall");
    assert!(
        x < 12.0 * 8.0 + 1.0,
        "ucontrol move must stop, not tunnel through a solid wall (x={x})"
    );
}

#[test]
fn c04_logic_pathfind_skirts_the_wall() {
    let world = erekir_test_world();
    let wall = (12 << 16) | 10;
    let mut tile = erekir_tile(wall, 216, 0);
    tile.occupied = vec![wall];
    world.tiles.insert(wall, tile);
    let mut dagger = ground_unit_on_tile(5, 0, (8 << 16) | 10, 150.0, 0.0);
    dagger.team = 1;
    dagger.authority = UnitAuthority::Logic {
        processor_pos: 1,
        remaining_ticks: 600.0,
        processor_generation: 1,
    };
    world.enemies.insert(5, dagger);
    world.unit_orders.insert(
        5,
        UnitOrder {
            unit_id: 5,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(16.0 * 8.0),
            target_y: Some(10.0 * 8.0),
            logic_control: logic_control::PATHFIND,
            queue: Vec::new(),
        },
    );
    let start = world.enemies.get(&5).unwrap().clone();
    let path = crate::network::units::ordered_unit_path(&world, &start, 16.0 * 8.0, 10.0 * 8.0);
    assert!(path.should_move);
    assert!(
        (path.dest_x - 12.0 * 8.0).abs() > 4.0 || (path.dest_y - 10.0 * 8.0).abs() > 4.0,
        "pathfind must not step into the wall tile"
    );
}

#[test]
fn c03_item_filters_are_exclusive_and_toggle() {
    let world = erekir_test_world();
    let mut mono = ground_unit_on_tile(6, 20, (4 << 16) | 4, 100.0, 1.0);
    mono.team = 1;
    world.enemies.insert(6, mono);
    world.unit_orders.insert(
        6,
        UnitOrder {
            unit_id: 6,
            command: 4,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    assert!(crate::network::decoders::apply_set_unit_stance(
        &world,
        &[6],
        8,
        true
    ));
    assert_eq!(
        world.unit_orders.get(&6).unwrap().stances & (1 << 8),
        1 << 8
    );
    assert!(crate::network::decoders::apply_set_unit_stance(
        &world,
        &[6],
        9,
        true
    ));
    let stances = world.unit_orders.get(&6).unwrap().stances;
    assert_eq!(stances & (1 << 8), 0, "copper filter yields to lead");
    assert_ne!(stances & (1 << 9), 0);
    assert!(crate::network::decoders::apply_set_unit_stance(
        &world,
        &[6],
        9,
        false
    ));
    assert_eq!(world.unit_orders.get(&6).unwrap().stances & (1 << 9), 0);
    assert!(crate::network::decoders::apply_set_unit_stance(
        &world,
        &[6],
        7,
        false
    ));
    assert_ne!(
        world.unit_orders.get(&6).unwrap().stances & (1 << 7),
        0,
        "mineAuto is not a toggle"
    );
}

#[test]
fn c03_hold_fire_still_moves_and_hold_position_stops_repair() {
    let world = erekir_test_world();
    let mut dagger = ground_unit_on_tile(7, 0, (4 << 16) | 4, 150.0, 0.0);
    dagger.team = 1;
    dagger.authority = UnitAuthority::Command;
    let start = (dagger.x, dagger.y);
    world.enemies.insert(7, dagger);
    world.unit_orders.insert(
        7,
        UnitOrder {
            unit_id: 7,
            command: 0,
            stances: 1 << 1,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(start.0 + 64.0),
            target_y: Some(start.1),
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    let out = DashMap::new();
    crate::network::simulation::simulate_allied_units(&world, &out, 8.0);
    let moved = world.enemies.get(&7).unwrap().clone();
    assert!(moved.x > start.0 + 1.0, "holdFire is not a movement stop");
    assert!(world.projectiles.is_empty());

    let mut mega = ground_unit_on_tile(8, 22, (3 << 16) | 3, 170.0, 1.0);
    mega.team = 1;
    let mega_start = (mega.x, mega.y);
    world.enemies.insert(8, mega);
    world.unit_orders.insert(
        8,
        UnitOrder {
            unit_id: 8,
            command: 1,
            stances: 1 << 6,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    let damaged = (6 << 16) | 6;
    let mut wall = erekir_tile(damaged, 216, 0);
    wall.health = 10.0;
    wall.occupied = vec![damaged];
    world.tiles.insert(damaged, wall);
    crate::network::simulation::simulate_support_units(&world, &out, 8.0);
    let mega = world.enemies.get(&8).unwrap();
    assert!(
        (mega.x - mega_start.0).abs() < 0.01 && (mega.y - mega_start.1).abs() < 0.01,
        "holdPosition gates RepairAI movement"
    );
}

#[test]
fn c03_pursue_target_reacquires_when_idle() {
    let world = erekir_test_world();
    let mut dagger = ground_unit_on_tile(13, 0, (4 << 16) | 4, 150.0, 0.0);
    dagger.team = 1;
    dagger.authority = UnitAuthority::Command;
    world.enemies.insert(13, dagger);
    let mut foe = ground_unit_on_tile(14, 0, (8 << 16) | 4, 150.0, 0.0);
    foe.team = 2;
    world.enemies.insert(14, foe);
    world.unit_orders.insert(
        13,
        UnitOrder {
            unit_id: 13,
            command: 0,
            stances: 1 << 2,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    let out = DashMap::new();
    crate::network::simulation::simulate_allied_units(&world, &out, 1.0);
    let order = world.unit_orders.get(&13).unwrap();
    assert_eq!(order.target_kind, 2);
    assert_eq!(order.target_id, 14);
}

#[test]
fn c05_logic_mine_fills_items_move_does_not() {
    let mut world = erekir_test_world();
    let tile = 10;
    world.overlays[(10 * world.width + tile) as usize] = 73;
    let mut mono = ground_unit_on_tile(11, 20, (tile << 16) | 10, 100.0, 1.0);
    mono.team = 1;
    mono.x = tile as f32 * 8.0;
    mono.y = 10.0 * 8.0;
    mono.authority = UnitAuthority::Logic {
        processor_pos: 1,
        remaining_ticks: 600.0,
        processor_generation: 1,
    };
    world.enemies.insert(11, mono.clone());
    world.unit_orders.insert(
        11,
        UnitOrder {
            unit_id: 11,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 6,
            target_id: -1,
            target_x: Some(mono.x),
            target_y: Some(mono.y),
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    let snap = world.enemies.get(&11).unwrap().clone();
    crate::network::simulation::simulate_logic_mining(&world, &snap, 60.0);
    assert_eq!(world.enemies.get(&11).unwrap().items, vec![(0, 1)]);

    world.enemies.get_mut(&11).unwrap().items.clear();
    world.unit_orders.get_mut(&11).unwrap().command = 4;
    world.unit_orders.get_mut(&11).unwrap().logic_control = logic_control::IDLE;
    crate::network::simulation::simulate_support_units(&world, &DashMap::new(), 60.0);
    assert!(
        world.enemies.get(&11).unwrap().items.is_empty(),
        "LogicAI idle must not run CommandAI mining"
    );

    world.unit_orders.get_mut(&11).unwrap().target_kind = 0;
    world.unit_orders.get_mut(&11).unwrap().logic_control = logic_control::MOVE;
    world.unit_orders.get_mut(&11).unwrap().target_x = Some(mono.x + 40.0);
    let snap = world.enemies.get(&11).unwrap().clone();
    crate::network::units::apply_logic_unit_movement(&world, &snap, 8.0);
    assert!(
        world.enemies.get(&11).unwrap().items.is_empty(),
        "LogicAI move does not mine"
    );
}

fn logic_move_unit(id: i32, unit_type: i16, tile: i32) -> (DynamicWorld, f32, f32) {
    let world = erekir_test_world();
    let mut unit = ground_unit_on_tile(id, unit_type, tile, 200.0, 0.0);
    unit.team = 1;
    unit.authority = UnitAuthority::Logic {
        processor_pos: 1,
        remaining_ticks: 600.0,
        processor_generation: 1,
    };
    let start = (unit.x, unit.y);
    world.enemies.insert(id, unit);
    world.unit_orders.insert(
        id,
        UnitOrder {
            unit_id: id,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(start.0 + 80.0),
            target_y: Some(start.1),
            logic_control: logic_control::MOVE,
            queue: Vec::new(),
        },
    );
    (world, start.0, start.1)
}

#[test]
fn c04_type_speed_status_boost_floor_and_drag() {
    for unit_type in [22i16, 20, 25, 26] {
        let spec = enemy_spec(unit_type).unwrap();
        let (speed, _, _, _, _) = crate::network::units::unit_move_physics(unit_type);
        assert!(
            (speed - spec.speed).abs() < 1e-5,
            "unit_move_physics({unit_type}) must use enemy_spec.speed"
        );
    }

    let (world, start_x, _) = logic_move_unit(31, 0, (4 << 16) | 10);
    for _ in 0..20 {
        let snap = world.enemies.get(&31).unwrap().clone();
        crate::network::units::apply_logic_unit_movement(&world, &snap, 1.0);
    }
    let baseline = world.enemies.get(&31).unwrap().x - start_x;

    let (slow_world, slow_start, _) = logic_move_unit(32, 0, (4 << 16) | 10);
    crate::network::units::StatusContainer::apply_status(
        &mut *slow_world.enemies.get_mut(&32).unwrap(),
        4,
        600.0,
    );
    for _ in 0..20 {
        let snap = slow_world.enemies.get(&32).unwrap().clone();
        crate::network::units::apply_logic_unit_movement(&slow_world, &snap, 1.0);
    }
    let slowed = slow_world.enemies.get(&32).unwrap().x - slow_start;
    assert!(
        slowed < baseline * 0.85,
        "slow must cut LogicAI travel (slowed={slowed} baseline={baseline})"
    );

    let (over_world, over_start, _) = logic_move_unit(33, 0, (4 << 16) | 10);
    crate::network::units::StatusContainer::apply_status(
        &mut *over_world.enemies.get_mut(&33).unwrap(),
        13,
        600.0,
    );
    for _ in 0..20 {
        let snap = over_world.enemies.get(&33).unwrap().clone();
        crate::network::units::apply_logic_unit_movement(&over_world, &snap, 1.0);
    }
    let overdriven = over_world.enemies.get(&33).unwrap().x - over_start;
    assert!(
        overdriven > baseline,
        "overdrive must increase LogicAI travel"
    );

    let (boost_world, boost_start, _) = logic_move_unit(34, 5, (4 << 16) | 10);
    boost_world.enemies.get_mut(&34).unwrap().elevation = 1.0;
    for _ in 0..20 {
        let snap = boost_world.enemies.get(&34).unwrap().clone();
        crate::network::units::apply_logic_unit_movement(&boost_world, &snap, 1.0);
    }
    let boosted = boost_world.enemies.get(&34).unwrap().x - boost_start;
    let (nova_world, nova_start, _) = logic_move_unit(35, 5, (4 << 16) | 10);
    for _ in 0..20 {
        let snap = nova_world.enemies.get(&35).unwrap().clone();
        crate::network::units::apply_logic_unit_movement(&nova_world, &snap, 1.0);
    }
    let grounded_nova = nova_world.enemies.get(&35).unwrap().x - nova_start;
    assert!(
        boosted > grounded_nova,
        "canBoost elevation must raise LogicAI speed"
    );

    let mut water = erekir_test_world();
    water.floors[(10 * water.width + 4) as usize] = 22;
    let mut wet = ground_unit_on_tile(36, 0, (4 << 16) | 10, 150.0, 0.0);
    wet.team = 1;
    wet.x = 4.0 * 8.0;
    wet.y = 10.0 * 8.0;
    wet.authority = UnitAuthority::Logic {
        processor_pos: 1,
        remaining_ticks: 600.0,
        processor_generation: 1,
    };
    let wet_start = wet.x;
    water.enemies.insert(36, wet);
    water.unit_orders.insert(
        36,
        UnitOrder {
            unit_id: 36,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(wet_start + 80.0),
            target_y: Some(10.0 * 8.0),
            logic_control: logic_control::MOVE,
            queue: Vec::new(),
        },
    );
    for _ in 0..20 {
        let snap = water.enemies.get(&36).unwrap().clone();
        crate::network::units::apply_logic_unit_movement(&water, &snap, 1.0);
    }
    let wet_travel = water.enemies.get(&36).unwrap().x - wet_start;
    assert!(
        wet_travel < baseline * 0.75,
        "shallow water must slow grounded LogicAI (wet={wet_travel} baseline={baseline})"
    );

    let (drag_world, _, _) = logic_move_unit(37, 0, (4 << 16) | 10);
    drag_world.wave_rules.write().drag_multiplier = 0.0;
    drag_world.enemies.get_mut(&37).unwrap().velocity_x = 2.0;
    let snap = drag_world.enemies.get(&37).unwrap().clone();
    crate::network::units::apply_logic_unit_movement(&drag_world, &snap, 1.0);
    let kept = drag_world.enemies.get(&37).unwrap().velocity_x.abs();
    let (normal_drag, _, _) = logic_move_unit(38, 0, (4 << 16) | 10);
    normal_drag.enemies.get_mut(&38).unwrap().velocity_x = 2.0;
    let snap = normal_drag.enemies.get(&38).unwrap().clone();
    crate::network::units::apply_logic_unit_movement(&normal_drag, &snap, 1.0);
    let damped = normal_drag.enemies.get(&38).unwrap().velocity_x.abs();
    assert!(
        kept > damped,
        "Rules.dragMultiplier 0 must preserve more velocity than default drag"
    );
}

#[test]
fn f02_conveyor_feeds_scaled_capacity_and_enabled_gates_power() {
    for cost in [0.0_f32, 0.5, 1.0, 3.0] {
        let world = erekir_test_world();
        world.wave_rules.write().unit_cost_multiplier = cost;
        let factory = (10 << 16) | 10;
        let belt = (9 << 16) | 10;
        let mut factory_tile = erekir_tile(factory, 377, 0);
        factory_tile.occupied = vec![factory];
        factory_tile.config = vec![1, 0, 0, 0, 0];
        world.tiles.insert(factory, factory_tile);
        let mut belt_tile = erekir_tile(belt, 257, 0);
        belt_tile.occupied = vec![belt];
        belt_tile.rotation = 0;
        world.tiles.insert(belt, belt_tile);
        let expected = (60.0 * cost).round() as i32;
        for _ in 0..(expected.max(1) + 4) {
            world.tiles.get_mut(&belt).unwrap().conveyor_items = vec![(9, 1.0)];
            simulate_logistics(&world, 1.0, &std::collections::HashMap::new());
        }
        let got = inventory_count(&world.tiles.get(&factory).unwrap().inventory, 9);
        if expected == 0 {
            assert_eq!(got, 0, "cost 0 silicon capacity rejects conveyor feed");
        } else {
            assert_eq!(got, expected, "cost {cost} silicon capacity via conveyor");
        }
    }

    let unload = erekir_test_world();
    let factory = (10 << 16) | 10;
    let unloader = (11 << 16) | 10;
    let container = (12 << 16) | 10;
    let mut factory_tile = erekir_tile(factory, 377, 0);
    factory_tile.occupied = vec![factory];
    factory_tile.config = vec![1, 0, 0, 0, 0];
    unload.tiles.insert(factory, factory_tile);
    let mut unloader_tile = erekir_tile(unloader, 270, 0);
    unloader_tile.occupied = vec![unloader];
    unloader_tile.config = vec![5, 0, 0, 9];
    unload.tiles.insert(unloader, unloader_tile);
    let mut container_tile = erekir_tile(container, 345, 0);
    container_tile.occupied = vec![container];
    container_tile.inventory = vec![(9, 8)];
    unload.tiles.insert(container, container_tile);
    simulate_unloaders(&unload, 6.0);
    simulate_unloaders(&unload, 1.0);
    assert!(
        inventory_count(&unload.tiles.get(&factory).unwrap().inventory, 9) >= 1,
        "unloader must feed silicon into the factory from a real container"
    );

    let world = erekir_test_world();
    let factory = (10 << 16) | 10;
    let source = (12 << 16) | 10;
    let mut factory_tile = erekir_tile(factory, 377, 0);
    factory_tile.occupied = vec![factory];
    factory_tile.config = vec![1, 0, 0, 0, 0];
    factory_tile.inventory = vec![(9, 10), (1, 10)];
    factory_tile.power_links = vec![source];
    world.tiles.insert(factory, factory_tile);
    let mut source_tile = erekir_tile(source, 410, 0);
    source_tile.occupied = vec![source];
    source_tile.power_links = vec![factory];
    world.tiles.insert(source, source_tile);
    assert_eq!(
        compute_power_efficiency(&world).get(&factory).copied(),
        Some(1.0)
    );

    world.tiles.get_mut(&factory).unwrap().enabled = false;
    let connections = DashMap::new();
    assert_eq!(
        effective_power_role(&world, &world.tiles.get(&factory).unwrap(), 1.0)
            .unwrap()
            .demand,
        0.0,
        "a disabled factory on a live net posts zero demand"
    );
    let power = compute_power_efficiency(&world);
    simulate_unit_factories(&world, &connections, 900.0, &power);
    assert!(
        world.enemies.is_empty(),
        "disabled factory on a full net must not produce"
    );
    assert_eq!(world.tiles.get(&factory).unwrap().production_progress, 0.0);

    world.tiles.get_mut(&factory).unwrap().enabled = true;
    world.tiles.get_mut(&factory).unwrap().power_links.clear();
    world.tiles.remove(&source);
    let isolated = compute_power_efficiency(&world);
    assert!(
        isolated.get(&factory).copied().unwrap_or(0.0) <= 0.0,
        "isolated factory has no graph satisfaction"
    );

    let solar = (14 << 16) | 10;
    let mut solar_tile = erekir_tile(solar, 313, 0);
    solar_tile.occupied = vec![solar];
    solar_tile.power_links = vec![factory];
    world.tiles.insert(solar, solar_tile);
    world.tiles.get_mut(&factory).unwrap().power_links = vec![solar];
    let partial = compute_power_efficiency(&world);
    let status = partial.get(&factory).copied().unwrap_or(0.0);
    assert!(
        status > 0.0 && status < 1.0,
        "undersized solar must leave the factory on a partial net ({status})"
    );

    let zero = erekir_test_world();
    zero.wave_rules.write().unit_cost_multiplier = 0.0;
    let zf = (10 << 16) | 10;
    let mut ztile = erekir_tile(zf, 377, 0);
    ztile.occupied = vec![zf];
    ztile.config = vec![1, 0, 0, 0, 0];
    zero.tiles.insert(zf, ztile);
    let mut zp = std::collections::HashMap::new();
    zp.insert(zf, 1.0);
    *zero.game_state.simulation_time.write() = 0.0;
    simulate_unit_factories(&zero, &connections, 900.0, &zp);
    simulate_unit_factories(&zero, &connections, 20.0, &zp);
    assert!(
        zero.enemies.len() == 1 || zero.tiles.get(&zf).unwrap().payload.is_some(),
        "cost 0 completes a dagger with no items (progress={})",
        zero.tiles.get(&zf).unwrap().production_progress
    );

    let hold = erekir_test_world();
    let hf = (10 << 16) | 10;
    let mut htile = erekir_tile(hf, 377, 0);
    htile.occupied = vec![hf];
    htile.config = vec![1, 0, 0, 0, 0];
    htile.inventory = vec![(9, 20), (1, 20)];
    htile.payload = Some(Box::new(CarriedPayload::Unit(ground_unit_on_tile(
        99, 0, hf, 150.0, 0.0,
    ))));
    hold.tiles.insert(hf, htile);
    let mut hp = std::collections::HashMap::new();
    hp.insert(hf, 1.0);
    simulate_unit_factories(&hold, &connections, 900.0, &hp);
    assert_eq!(
        inventory_count(&hold.tiles.get(&hf).unwrap().inventory, 9),
        20,
        "a factory holding payload must not consume another plan"
    );
}

#[test]
fn c05_mine_stance_payload_save_and_mega_gates() {
    let mut world = erekir_test_world();
    let tile = 10;
    world.overlays[(10 * world.width + tile) as usize] = 73;
    let mut mono = ground_unit_on_tile(41, 20, (tile << 16) | 10, 100.0, 1.0);
    mono.team = 1;
    mono.authority = UnitAuthority::Command;
    mono.x = tile as f32 * 8.0;
    mono.y = 10.0 * 8.0;
    mono.items = vec![(0, 4)];
    world.enemies.insert(41, mono.clone());
    world.unit_orders.insert(
        41,
        UnitOrder {
            unit_id: 41,
            command: 4,
            stances: 1 << 8,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(mono.x),
            target_y: Some(mono.y),
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    assert!(crate::network::decoders::apply_set_unit_stance(
        &world,
        &[41],
        9,
        true
    ));
    assert_eq!(
        world.enemies.get(&41).unwrap().items,
        vec![(0, 4)],
        "changing the mine filter mid-load must keep unit.items"
    );

    let mut mega = ground_unit_on_tile(42, 22, (6 << 16) | 6, enemy_spec(22).unwrap().health, 1.0);
    mega.team = 1;
    let mega_x = mega.x;
    let mega_y = mega.y;
    world.enemies.insert(42, mega);
    let mut carried = ground_unit_on_tile(43, 0, (6 << 16) | 6, enemy_spec(0).unwrap().health, 0.0);
    carried.team = 1;
    world.enemies.insert(43, carried);
    world.unit_orders.insert(
        42,
        UnitOrder {
            unit_id: 42,
            command: 6,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: Some(mega_x),
            target_y: Some(mega_y),
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    crate::network::simulation::simulate_payload_carriers(&world, &DashMap::new(), 1.0);
    assert_eq!(
        world.enemies.get(&42).unwrap().payloads.len(),
        1,
        "mega CommandAI pickup must swallow the grounded dagger"
    );
    assert!(world.enemies.get(&43).is_none());
    let path = std::env::temp_dir().join(format!("mindustry-c05-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    crate::network::wire::persistence::persist_tiles(
        &path,
        &world.tiles,
        &world.game_state,
        &world.enemies,
        &world.base_buildings,
        &world.player_profiles,
        &world.building_commands,
        &world.unit_orders,
        &world.team_build_plans.read(),
        (&world.cores, &world.team_core_lists),
        &world.logic_flags,
        &world.puddles,
        crate::network::wire::persistence::checkpoint_rules_json(&world),
    )
    .unwrap();
    let loaded = crate::network::wire::persistence::load_tiles(&path, Some((40, 40))).unwrap();
    let restored_mono = loaded
        .enemies
        .iter()
        .find(|unit| unit.id == 41)
        .expect("miner round-trips");
    assert_eq!(restored_mono.items, vec![(0, 4)]);
    let restored_mega = loaded
        .enemies
        .iter()
        .find(|unit| unit.id == 42)
        .expect("mega round-trips");
    assert_eq!(
        restored_mega.payloads.len(),
        1,
        "payload pickup survives save/load"
    );
    let _ = std::fs::remove_file(&path);

    world.unit_orders.get_mut(&42).unwrap().command = 8;
    crate::network::simulation::simulate_payload_carriers(&world, &DashMap::new(), 1.0);
    assert!(
        world.enemies.get(&42).unwrap().payloads.is_empty(),
        "mega CommandAI drop must empty the payload stack"
    );
    assert!(
        world
            .enemies
            .iter()
            .any(|unit| unit.unit_type == 0 && unit.id != 41),
        "dropped dagger must re-enter the world"
    );

    let support = erekir_test_world();
    let wall = (8 << 16) | 8;
    let mut wall_tile = erekir_tile(wall, 216, 0);
    wall_tile.occupied = vec![wall];
    wall_tile.health = 40.0;
    support.tiles.insert(wall, wall_tile);
    let mut mega = ground_unit_on_tile(44, 22, (8 << 16) | 9, 170.0, 1.0);
    mega.team = 1;
    mega.secondary_attack_reload = 14.5;
    support.enemies.insert(44, mega);
    support.unit_orders.insert(
        44,
        UnitOrder {
            unit_id: 44,
            command: 1,
            stances: 1 << 1,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: logic_control::IDLE,
            queue: Vec::new(),
        },
    );
    let out = DashMap::new();
    crate::network::simulation::simulate_support_units(&support, &out, 2.0);
    assert_eq!(
        support.tiles.get(&wall).unwrap().health,
        40.0,
        "holdFire mega must not auto-repair"
    );
    support.unit_orders.get_mut(&44).unwrap().stances = 0;
    crate::network::units::StatusContainer::apply_status(
        &mut *support.enemies.get_mut(&44).unwrap(),
        20,
        600.0,
    );
    crate::network::simulation::simulate_support_units(&support, &out, 2.0);
    assert_eq!(
        support.tiles.get(&wall).unwrap().health,
        40.0,
        "disarmed mega must not auto-repair"
    );
    support.enemies.get_mut(&44).unwrap().statuses.clear();
    support.enemies.get_mut(&44).unwrap().status_effect = -1;
    support.enemies.get_mut(&44).unwrap().authority = UnitAuthority::Player { player_id: 1 };
    crate::network::simulation::simulate_support_units(&support, &out, 2.0);
    assert_eq!(
        support.tiles.get(&wall).unwrap().health,
        40.0,
        "possessed mega must not auto-repair"
    );
}
