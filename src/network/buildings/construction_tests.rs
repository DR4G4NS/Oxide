//! Block limit regression tests (Build 159.7 live building count).

use crate::network::buildings::construction::{
    block_footprint_in, finish_pending_build, live_team_building_count,
};
use crate::network::outbound::NOOP;
use crate::network::world::{
    stamp_new_building, BaseBuildingState, ControlledUnit, DynamicTile, DynamicWorld, PendingBuild,
    SessionPlayer,
};
use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
use std::sync::Arc;

fn block_limit_world() -> DynamicWorld {
    let width = 64i32;
    let height = 64i32;
    let state = crate::state::game_state::GameState::new();
    DynamicWorld {
        game_state: state,
        width,
        height,
        sharded_unit_cap: 8,
        core_position: 0,
        core_max_health: 0.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: vec![0i16; (width * height) as usize],
        base_centers: vec![false; (width * height) as usize],
        tile_data: Vec::new(),
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: vec![0i16; (width * height) as usize],
        overlays: vec![0i16; (width * height) as usize],
        enemy_spawns: Vec::new(),
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_000),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
        projectiles: DashMap::new(),
        next_projectile_id: AtomicI32::new(4_000_000),
        overdrive_boosts: DashMap::new(),
        heal_suppression: DashMap::new(),
        force_fields: DashMap::new(),
        tiles: DashMap::new(),
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::new(),
        mono_mining_targets: DashMap::new(),
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir().join("block-limit-test.json"),
        network_template: Arc::new(Vec::new()),
        persistence_dirty: AtomicBool::new(false),
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
        votekick_votes: AtomicI32::new(0),
        votekick_voters: DashMap::new(),
        votekick_cooldowns: DashMap::new(),
        puddles: crate::network::buildings::puddles::PuddleSystem::new(),
    }
}

fn pos(x: i16, y: i16) -> i32 {
    ((x as i32) << 16) | (y as i32 & 0xffff)
}

/// Independent Rust mirror of v159.7 `ApplicationTests.multiblock`.
///
/// Upstream creates a 3x3 core at an origin and observes that every occupied
/// tile resolves to the same `Building` object.  The Rust authority stores
/// that observable contract as an explicit footprint plus a generation-backed
/// [`BuildingIdentity`], so replacing a building at the origin cannot make a
/// stale footprint look live.
#[test]
fn upstream_application_multiblock_1597_origin_footprint_and_identity() {
    let world = block_limit_world();
    let origin = pos(20, 20);
    let occupied = block_footprint_in(world.width, world.height, origin, 339)
        .expect("core shard footprint must fit");
    assert_eq!(occupied.len(), 9, "core shard is a 3x3 multiblock");
    assert!(occupied.contains(&origin));

    let mut core = DynamicTile {
        position: origin,
        block: 339,
        team: 1,
        occupied: occupied.clone(),
        ..DynamicTile::default()
    };
    stamp_new_building(&world, &mut core);
    let first = core.identity();
    world.tiles.insert(origin, core);

    for position in occupied {
        let resolved = crate::network::buildings::construction::dynamic_at(&world, position)
            .expect("every footprint tile resolves to its origin");
        assert_eq!(resolved.identity(), first);
        assert_eq!(resolved.block, 339);
        assert_eq!(resolved.team, 1);
    }

    // A second building at the same origin has a distinct object identity,
    // matching Java's `tile.build == this` check rather than position-only
    // identity.
    let mut replacement = DynamicTile {
        position: origin,
        block: 339,
        team: 1,
        occupied: block_footprint_in(world.width, world.height, origin, 339).unwrap(),
        ..DynamicTile::default()
    };
    stamp_new_building(&world, &mut replacement);
    assert_ne!(replacement.identity(), first);
}

#[test]
fn block_limit_counts_prebuilt_base_buildings() {
    let world = block_limit_world();
    let block = 257;
    world.base_buildings.insert(
        pos(10, 10),
        BaseBuildingState {
            position: pos(10, 10),
            block,
            team: 1,
            health: 100.0,
            occupied: vec![pos(10, 10)],
            inventory: Vec::new(),
        },
    );
    assert_eq!(live_team_building_count(&world, 1, block), 1);
}

#[test]
fn block_limit_counts_dynamic_buildings() {
    let world = block_limit_world();
    let block = 257;
    world.tiles.insert(
        pos(5, 5),
        DynamicTile {
            position: pos(5, 5),
            block,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![pos(5, 5)],
            ..Default::default()
        },
    );
    assert_eq!(live_team_building_count(&world, 1, block), 1);
}

#[test]
fn block_limit_counts_base_plus_dynamic() {
    let world = block_limit_world();
    let block = 257;
    world.base_buildings.insert(
        pos(10, 10),
        BaseBuildingState {
            position: pos(10, 10),
            block,
            team: 1,
            health: 100.0,
            occupied: vec![pos(10, 10)],
            inventory: Vec::new(),
        },
    );
    world.tiles.insert(
        pos(20, 20),
        DynamicTile {
            position: pos(20, 20),
            block,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![pos(20, 20)],
            ..Default::default()
        },
    );
    assert_eq!(live_team_building_count(&world, 1, block), 2);
}

#[test]
fn block_limit_does_not_double_count_replaced_base_building() {
    let world = block_limit_world();
    let block = 257;
    let origin = pos(10, 10);
    world.base_buildings.insert(
        origin,
        BaseBuildingState {
            position: origin,
            block,
            team: 1,
            health: 100.0,
            occupied: vec![origin],
            inventory: Vec::new(),
        },
    );
    world.tiles.insert(
        origin,
        DynamicTile {
            position: origin,
            block,
            rotation: 0,
            team: 1,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![origin],
            ..Default::default()
        },
    );
    assert_eq!(live_team_building_count(&world, 1, block), 1);
}

#[test]
fn block_limit_zero_is_unlimited() {
    let world = block_limit_world();
    let rules = world.wave_rules.read();
    assert!(!rules.is_over_placement_limit(257, 999, 1));
}

#[test]
fn block_limit_at_capacity_rejects_third() {
    let world = block_limit_world();
    let block = 257;
    world.wave_rules.write().block_limits.insert(block, 2);
    for (x, y) in [(10, 10), (20, 20)] {
        world.base_buildings.insert(
            pos(x, y),
            BaseBuildingState {
                position: pos(x, y),
                block,
                team: 1,
                health: 100.0,
                occupied: vec![pos(x, y)],
                inventory: Vec::new(),
            },
        );
    }
    assert_eq!(live_team_building_count(&world, 1, block), 2);
    assert!(world.wave_rules.read().is_over_placement_limit(
        block,
        live_team_building_count(&world, 1, block),
        1
    ));
}

#[test]
fn block_limit_ai_team_bypass() {
    let world = block_limit_world();
    let block = 257;
    world.wave_rules.write().block_limits.insert(block, 1);
    world.base_buildings.insert(
        pos(10, 10),
        BaseBuildingState {
            position: pos(10, 10),
            block,
            team: 2,
            health: 100.0,
            occupied: vec![pos(10, 10)],
            inventory: Vec::new(),
        },
    );
    assert!(!world
        .wave_rules
        .read()
        .is_over_placement_limit(block, 99, 2));
}

#[test]
fn block_limit_editor_bypass() {
    let world = block_limit_world();
    let block = 257;
    world.wave_rules.write().block_limits.insert(block, 1);
    world.wave_rules.write().editor = true;
    assert!(!world
        .wave_rules
        .read()
        .is_over_placement_limit(block, 99, 1));
}

#[test]
fn block_limit_replacement_of_same_block_does_not_false_reject() {
    let world = block_limit_world();
    let block = 257;
    let origin = pos(10, 10);
    world.wave_rules.write().block_limits.insert(block, 1);
    world.base_buildings.insert(
        origin,
        BaseBuildingState {
            position: origin,
            block,
            team: 1,
            health: 100.0,
            occupied: vec![origin],
            inventory: Vec::new(),
        },
    );
    let mut count = live_team_building_count(&world, 1, block);
    assert_eq!(count, 1);
    count = count.saturating_sub(1);
    assert!(!world
        .wave_rules
        .read()
        .is_over_placement_limit(block, count, 1));
}

#[test]
fn block_limit_rechecked_at_construct_finish() {
    let world = block_limit_world();
    let block = 257;
    world.wave_rules.write().block_limits.insert(block, 2);
    for (x, y) in [(10, 10), (20, 20)] {
        world.base_buildings.insert(
            pos(x, y),
            BaseBuildingState {
                position: pos(x, y),
                block,
                team: 1,
                health: 100.0,
                occupied: vec![pos(x, y)],
                inventory: Vec::new(),
            },
        );
    }
    let origin = pos(30, 30);
    let pending = PendingBuild {
        position: origin,
        block,
        rotation: 0,
        config: Vec::new(),
        occupied: vec![origin],
        team: 1,
        builder: SessionPlayer {
            id: 1,
            controlled_unit: ControlledUnit::Core,
            unit_id: 2,
            uuid: "builder".into(),
            name: "builder".into(),
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
            // Architecture rule: tests must construct domain-owned types
            // (SessionPlayer::chat_rate is owned by network::wire, not listener).
            chat_rate: crate::network::wire::ChatRateLimiter::new(),
        },
        last_seen: std::time::Instant::now(),
        assist_progress: 0.0,
        remaining_ticks: 0.0,
        applied_assist: 0.0,
    };
    world.pending_builds.insert(origin, pending.clone());
    finish_pending_build(&world, &NOOP, pending).unwrap();
    assert!(
        world.pending_builds.contains_key(&origin),
        "construct finish must stall when block limit exceeded"
    );
    assert_eq!(live_team_building_count(&world, 1, block), 2);
}
