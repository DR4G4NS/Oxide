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
            status_agg: None,
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
    assert!((titan.0 - 60.0).abs() < 0.001 && (titan.1 - 390.0).abs() < 0.001);
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
    assert!((breach_beryllium.damage - 85.0).abs() < 0.001);
    assert!((breach_beryllium.speed - 7.5).abs() < 0.001);
    assert!(breach_beryllium.pierce);
    let titan_thorium = erekir_turret_ammo_spec(370, 7).unwrap();
    assert_eq!(titan_thorium.bullet_id, 172);
    assert!((titan_thorium.damage - 350.0).abs() < 0.001);
    assert!((titan_thorium.splash_damage - 350.0).abs() < 0.001);
    assert!((titan_thorium.splash_radius - 65.0).abs() < 0.001);
    let disperse_tungsten = erekir_turret_ammo_spec(371, 17).unwrap();
    assert_eq!(disperse_tungsten.multiplier as i32, 3); // ammoMultiplier 3
    let afflict_weapon = erekir_power_turret_weapon(372).unwrap();
    assert_eq!(afflict_weapon.bullet_id, 181);
    assert!((afflict_weapon.damage - 180.0).abs() < 0.001);
    let lustre_weapon = erekir_power_turret_weapon(373).unwrap();
    assert_eq!(lustre_weapon.bullet_id, 183);
    let malign_weapon = erekir_power_turret_weapon(376).unwrap();
    assert_eq!(malign_weapon.bullet_id, 196);
    // sublimate liquid ammo.
    let sublimate_ozone = erekir_liquid_turret_ammo(369, 7).unwrap();
    assert_eq!(sublimate_ozone.bullet_id, 170);
    assert!((sublimate_ozone.damage - 60.0).abs() < 0.001);
    let sublimate_cyanogen = erekir_liquid_turret_ammo(369, 10).unwrap();
    assert_eq!(sublimate_cyanogen.bullet_id, 171);
    assert!((sublimate_cyanogen.damage - 130.0).abs() < 0.001);
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
            status_agg: None,
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
        .all(|projectile| (projectile.damage - 41.0).abs() < 0.001));
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
        enemy_spawns: Vec::new(),
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: std::sync::atomic::AtomicI32::new(2_500_000),
        next_enemy_id: std::sync::atomic::AtomicI32::new(3_000_100),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
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
        tile_footprint: DashMap::new(),
        navigation_revision: std::sync::atomic::AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
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
        wave_rules: parking_lot::RwLock::new(crate::network::units::WaveRules::default()),
        votekick_target: parking_lot::RwLock::new(None),
        votekick_votes: std::sync::atomic::AtomicI32::new(0),
        votekick_voters: dashmap::DashMap::new(),
        votekick_cooldowns: dashmap::DashMap::new(),
        puddles: crate::network::buildings::puddles::PuddleSystem::new(),
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
        status_agg: None,
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
fn assembler_tier_zero_builds_without_module_and_consumes_real_inventory() {
    // SOL-010: tank-assembler (393) tier 0 (vanquish 41) builds WITHOUT any
    // adjacent UnitAssemblerModule, and the item cost is deducted from the
    // REAL team core inventory (the old code deducted on a cloned Vec, so
    // the actual store never changed — UnitAssembler.java:315-390 has
    // currentTier = 0 and the base plan needs no module).
    let world = erekir_test_world();
    let assembler = (10 << 16) | 10;
    let mut asm = erekir_tile(assembler, 393, 0);
    asm.occupied = vec![assembler];
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
    assert_eq!(items[16], 0, "beryllium consumed from the REAL inventory");
    assert_eq!(items[9], 0, "silicon consumed from the REAL inventory");
    assert_eq!(world.enemies.len(), 1);
    let unit = world.enemies.iter().next().unwrap();
    assert_eq!(unit.unit_type, 41, "tier 0 vanquish without any module");
    assert_eq!(unit.team, 1);
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
    {
        let mut items = crate::network::economy::items_for_team_mut(&world, 1);
        items[18] = 60; // oxide
        items[10] = 40; // phase fabric
    }
    let mut power = std::collections::HashMap::new();
    power.insert(assembler, 1.0);
    let connections = DashMap::new();
    assert!(simulate_erekir_assemblers(
        &world,
        &connections,
        60.0 * 180.0 + 1.0,
        &power
    ));
    let unit = world.enemies.iter().next().unwrap();
    assert_eq!(unit.unit_type, 42, "tier 1 conquer (id 42, not 44)");
    let items = crate::network::economy::items_for_team(&world, 1);
    assert_eq!(items[18], 0, "oxide consumed from the REAL inventory");
    assert_eq!(items[10], 0, "phase consumed from the REAL inventory");
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
    // SOL-010: official Conduit semantics — ConduitBuild.updateTile moves
    // liquid FORWARD only (Conduit.java:144-151) with the pressure flow of
    // BuildingComp.moveLiquid (BuildingComp.java:944-958), and acceptLiquid
    // rejects liquid pushed into the conduit's front (Conduit.java:129-133).
    // A3: the whole step runs once per second (`timer(timerFlow, 1)`), so a
    // 60-tick batch fires exactly one discrete moveLiquidForward step.
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
    // One tick (1/60 s) must NOT fire the 1 Hz gate: no flow at all.
    simulate_liquids(&world, 1.0, &power);
    assert_eq!(
        world.tiles.get(&conduit).unwrap().liquid_amount,
        10.0,
        "the 1 Hz timer gate keeps the step for a single tick"
    );
    // 60 ticks = one official second -> one discrete pressure step.
    simulate_liquids(&world, 59.0, &power);
    assert_eq!(
        world.tiles.get(&conduit).unwrap().liquid_amount,
        0.0,
        "conduit empties into its front on the 1 Hz step"
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
    simulate_liquids(&world, 60.0, &power);
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
    // A3: moveLiquidForward is ONE DISCRETE STEP PER SECOND
    // (Conduit$ConduitBuild.updateTile JAR offsets 42-75:
    // `if(liquids.currentAmount() > 1e-4 && timer(timerFlow, 1))`), so a
    // single tick must not leak at all and every 60-tick batch applies the
    // official `leakAmount = currentAmount / 1.5` step
    // (Building.moveLiquidForward JAR offsets 59-93).
    fn amount_after_steps(block: i16, one_second_steps: u32) -> f32 {
        let world = erekir_test_world();
        let position = (10 << 16) | 10;
        let mut conduit = erekir_tile(position, block, 0);
        conduit.occupied = vec![position];
        conduit.stored_liquid = 0;
        conduit.liquid_amount = 15.0;
        world.tiles.insert(position, conduit);
        for _ in 0..one_second_steps {
            simulate_liquids(&world, 60.0, &std::collections::HashMap::new());
        }
        let remaining = world.tiles.get(&position).unwrap().liquid_amount;
        remaining
    }

    fn amount_after_ticks(block: i16, batch_ticks: f32) -> f32 {
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

    // One tick (1/60 s) and 59 ticks are below the 1 Hz gate: no leak yet.
    assert!((amount_after_ticks(286, 1.0) - 15.0).abs() < 0.001);
    assert!((amount_after_ticks(286, 59.0) - 15.0).abs() < 0.001);
    // After one second: 15 - 15/1.5 = 5.
    assert!((amount_after_steps(286, 1) - 5.0).abs() < 0.001); // conduit leaks
    assert!((amount_after_steps(287, 1) - 5.0).abs() < 0.001); // pulse leaks
    assert!((amount_after_steps(288, 1) - 15.0).abs() < 0.001); // plated is sealed
    assert!((amount_after_steps(296, 1) - 5.0).abs() < 0.001); // reinforced opts in
                                                               // After a second step: 5 - 5/1.5 = 3.3333.
    assert!((amount_after_steps(286, 2) - (5.0 - 5.0 / 1.5)).abs() < 0.001);
    // A single 120-tick BATCH is one laggy frame: the official Interval
    // fires once and drops the excess (times[id] = Time.time), so it leaks
    // exactly once.
    assert!((amount_after_ticks(286, 120.0) - 5.0).abs() < 0.001);
    // The leak deposits a puddle on the front tile (P1: authoritative
    // PuddleSystem), so the leak is observable server-side.
    let world = erekir_test_world();
    let position = (10 << 16) | 10;
    let mut conduit = erekir_tile(position, 286, 0);
    conduit.occupied = vec![position];
    conduit.stored_liquid = 0;
    conduit.liquid_amount = 15.0;
    world.tiles.insert(position, conduit);
    simulate_liquids(&world, 60.0, &std::collections::HashMap::new());
    let front = (11 << 16) | 10;
    let puddle = world.puddles.puddles.get(&front).unwrap();
    assert!(
        (puddle.accepting - 10.0).abs() < 0.001,
        "one second of leaking deposits currentAmount/1.5 = 10 into the puddle"
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
fn pump_conduit_tank_chain_conserves_water_over_300_ticks() {
    // Differential vs Java 158.1 formulas (Pump.updateTile + Conduit 1 Hz
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
        "tank should hold the bulk after five 1 Hz conduit steps"
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
        (pa - 5.0).abs() < 0.001 && (pb - 5.0).abs() < 0.001,
        "each poly (0.5 speed) advances only its own plan: {pa} / {pb}"
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
        (world.tiles.get(&site).unwrap().production_progress - 5.5).abs() < 0.001,
        "rebuild 0.5 + assist 5.0"
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
