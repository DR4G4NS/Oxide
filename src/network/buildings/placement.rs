//! Shared post-placement lifecycle.
//!
//! Every construction path (players, builder units, logic and future plugins)
//! must call this service after inserting the tile and before broadcasting or
//! persisting it. This is the Rust equivalent of `Building.created/placed`.

use crate::network::buildings::power;
use crate::network::world::DynamicWorld;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlacementChanges {
    pub configured_power_links: bool,
    pub auto_linked_power: bool,
    /// Canonical Point2[] configs for every power node whose links changed.
    /// The caller must forward these as TileConfig packets after publishing
    /// the placed building; block snapshots alone do not reflow client graphs.
    pub power_node_configs: Vec<(i32, Vec<u8>)>,
}

pub fn after_placement(
    world: &DynamicWorld,
    position: i32,
    initial_config: &[u8],
) -> PlacementChanges {
    let before: HashMap<i32, (Vec<i32>, Vec<u8>)> = world
        .tiles
        .iter()
        .filter(|tile| power::is_power_node(tile.block))
        .map(|tile| {
            (
                tile.position,
                (tile.power_links.clone(), tile.config.clone()),
            )
        })
        .collect();
    let is_node = world
        .tiles
        .get(&position)
        .is_some_and(|tile| power::is_power_node(tile.block));
    let configured_power_links = is_node
        && power::valid_configuration(initial_config)
        && power::apply_configuration(world, position, initial_config);
    let auto_linked_power = power::autolink_after_placement(world, position);
    // Round 74g: instant autolink for machines placed near nodes (the user
    // expects the link the moment the build completes, not on the next
    // sweep pass).
    let nearby_linked = power::relink_nearby_nodes(world, position);
    let mut affected: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| power::is_power_node(tile.block))
        .filter_map(|tile| {
            let changed = before
                .get(&tile.position)
                .is_none_or(|(links, config)| *links != tile.power_links || *config != tile.config);
            changed.then_some(tile.position)
        })
        .collect();
    affected.sort_unstable();
    affected.dedup();
    // Reverse links can change another node without that node being the
    // relink source. Canonicalize every affected node before exposing events.
    for node in &affected {
        power::sync_node_config_with_links(world, *node);
    }
    let power_node_configs = affected
        .into_iter()
        .filter_map(|node| {
            world
                .tiles
                .get(&node)
                .map(|tile| (node, tile.config.clone()))
        })
        .collect();
    PlacementChanges {
        configured_power_links,
        auto_linked_power: auto_linked_power || nearby_linked,
        power_node_configs,
    }
}

fn footprint(world: &DynamicWorld, origin: i32, block: i16) -> Option<Vec<i32>> {
    let x = (origin >> 16) as i16 as i32;
    let y = origin as i16 as i32;
    let size = i32::from(crate::game::content::block_size(block));
    let offset = -(size - 1) / 2;
    let mut positions = Vec::with_capacity((size * size) as usize);
    for dy in 0..size {
        for dx in 0..size {
            let px = x + offset + dx;
            let py = y + offset + dy;
            if px < 0 || py < 0 || px >= world.width() || py >= world.height() {
                return None;
            }
            positions.push((px << 16) | (py as u16 as i32));
        }
    }
    Some(positions)
}

fn cell_occupied(world: &DynamicWorld, position: i32) -> bool {
    world.tiles.contains_key(&position)
        || world.tile_footprint.contains_key(&position)
        || world.base_buildings.contains_key(&position)
}

fn building_origin(world: &DynamicWorld, position: i32) -> Option<i32> {
    world
        .tiles
        .get(&position)
        .map(|tile| tile.position)
        .or_else(|| world.tile_footprint.get(&position).map(|origin| *origin))
}

fn occupied_cells(origin: i32, tile: &crate::network::world::DynamicTile) -> Vec<i32> {
    if tile.occupied.is_empty() {
        vec![origin]
    } else {
        tile.occupied.clone()
    }
}

/// P1-D1: shared teardown for every path that retires a live building.
/// When `remove_tile` is true the origin is erased from `tiles` and its
/// footprint cells are cleared; when false the tile may be tombstoned in
/// place (combat destruction).
fn retire_building(
    world: &DynamicWorld,
    origin: i32,
    tile: &crate::network::world::DynamicTile,
    remove_tile: bool,
) {
    let cells = occupied_cells(origin, tile);
    power::disconnect_building(world, origin);
    world.building_commands.remove(&origin);
    world.logic_executors.remove(&origin);
    if remove_tile {
        if let Some(live) = world.tiles.get(&origin) {
            crate::network::world::note_building_generation(live.generation);
        }
        world.tiles.remove(&origin);
        for cell in &tile.occupied {
            world.tile_footprint.remove(cell);
        }
    }
    world
        .navigation_revision
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    world
        .persistence_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
    power::relink_after_insulated_removed(world, origin, tile.block, &cells);
}

/// P1-D1: permanent removal (break, deconstruct, same-tile replacement).
/// Power links (both directions), logic executors and footprint are cleared.
pub fn remove_building_from_world(
    world: &DynamicWorld,
    position: i32,
) -> Option<crate::network::world::DynamicTile> {
    let origin = building_origin(world, position)?;
    let tile = world.tiles.get(&origin)?.clone();
    if tile.block == 0 {
        return None;
    }
    retire_building(world, origin, &tile, true);
    Some(tile)
}

/// P1-D1: combat/destruct path — disconnect side effects but leave the tile
/// slot for a tombstone insert by the caller.
pub fn teardown_building_in_place(world: &DynamicWorld, position: i32) -> bool {
    let Some(origin) = building_origin(world, position) else {
        return false;
    };
    let Some(tile) = world.tiles.get(&origin).map(|tile| tile.clone()) else {
        return false;
    };
    if tile.block == 0 {
        return false;
    }
    retire_building(world, origin, &tile, false);
    true
}

/// P0-09: take a live building out of the world. Power links (both
/// directions) are dropped; the returned tile never keeps stale lasers.
pub fn detach_building_from_world(
    world: &DynamicWorld,
    position: i32,
) -> Option<crate::network::world::DynamicTile> {
    let mut carried = remove_building_from_world(world, position)?;
    carried.power_links.clear();
    Some(carried)
}

/// P0-09: materialize a carried building at `position`. New power links come
/// only from `after_placement`; previous lasers are never restored.
pub fn attach_building_to_world(
    world: &DynamicWorld,
    mut tile: crate::network::world::DynamicTile,
    position: i32,
) -> Option<PlacementChanges> {
    let occupied = footprint(world, position, tile.block)?;
    if occupied.iter().any(|cell| cell_occupied(world, *cell)) {
        return None;
    }
    tile.position = position;
    tile.occupied = occupied.clone();
    tile.power_links.clear();
    // Payload drop keeps the carried Building instance. A brand-new
    // building (generation 0, e.g. a map tile that never ran through
    // construction) stays 0; live construction stamps before insert.
    crate::network::world::note_building_generation(tile.generation);
    let initial_config = tile.config.clone();
    world.tiles.insert(position, tile);
    for cell in occupied {
        world.tile_footprint.insert(cell, position);
    }
    let changes = after_placement(world, position, &initial_config);
    world
        .navigation_revision
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    world
        .persistence_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
    Some(changes)
}

/// P1-07: `Building.changeTeam` — drop old power edges, then rebuild
/// proximity on the new team so graphs split/join immediately.
pub fn change_building_team(world: &DynamicWorld, position: i32, team: u8) -> bool {
    let Some(origin) = building_origin(world, position) else {
        return false;
    };
    let Some(current) = world.tiles.get(&origin).map(|tile| tile.team) else {
        return false;
    };
    if current == team {
        return false;
    }
    power::disconnect_building(world, origin);
    if let Some(mut tile) = world.tiles.get_mut(&origin) {
        tile.team = team;
        tile.power_links.clear();
    }
    after_placement(world, origin, &[]);
    world
        .persistence_dirty
        .store(true, std::sync::atomic::Ordering::Relaxed);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::economy::payload::{
        apply_request_build_payload, apply_request_drop_payload, apply_request_unit_payload,
        payload_capacity,
    };
    use crate::network::world::{
        CarriedBuildPayload, CarriedPayload, ControlledUnit, DynamicTile, DynamicWorld, EnemyUnit,
        PlayerCombatState, SessionPlayer, UnitAuthority,
    };
    use dashmap::DashMap;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
    use std::sync::Arc;

    fn test_world() -> DynamicWorld {
        let width = 80i32;
        let height = 80i32;
        let state = crate::state::game_state::GameState::new();
        state.start_hosting(
            "payload-rpc-test".into(),
            crate::state::game_state::GameMode::Survival,
        );
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
            floors: Vec::new(),
            overlays: Vec::new(),
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
            save_path: std::env::temp_dir().join("payload-rpc-test.json"),
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

    fn pos(x: i32, y: i32) -> i32 {
        (x << 16) | (y & 0xffff)
    }

    fn insert_building(world: &DynamicWorld, x: i32, y: i32, block: i16, team: u8) -> i32 {
        let origin = pos(x, y);
        let occupied = footprint(world, origin, block).expect("footprint");
        let tile = DynamicTile {
            position: origin,
            block,
            team,
            occupied: occupied.clone(),
            health: 1.0,
            ..DynamicTile::default()
        };
        world.tiles.insert(origin, tile);
        for cell in occupied {
            world.tile_footprint.insert(cell, origin);
        }
        origin
    }

    fn mega(id: i32, x: f32, y: f32) -> EnemyUnit {
        EnemyUnit {
            id,
            unit_type: 22,
            entity_class: 0,
            team: 1,
            x,
            y,
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
            authority: UnitAuthority::Player { player_id: 1 },
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        }
    }

    fn dagger(id: i32, x: f32, y: f32, elevation: f32, authority: UnitAuthority) -> EnemyUnit {
        let mut unit = mega(id, x, y);
        unit.unit_type = 0;
        unit.elevation = elevation;
        unit.authority = authority;
        unit
    }

    fn player_for(carrier_id: i32) -> SessionPlayer {
        SessionPlayer {
            id: 1,
            controlled_unit: ControlledUnit::Standard(carrier_id),
            unit_id: 2_500_001,
            uuid: "payload-player".into(),
            name: "payload".into(),
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
            last_shot: std::time::Instant::now() - std::time::Duration::from_secs(1),
            admin: false,
            chat_rate: crate::network::wire::ChatRateLimiter::new(),
        }
    }

    fn give_wall(world: &DynamicWorld, carrier_id: i32, tile: DynamicTile) {
        if let Some(mut live) = world.enemies.get_mut(&carrier_id) {
            live.payloads
                .push(CarriedPayload::Build(CarriedBuildPayload {
                    version: 0,
                    tile,
                    sync: Vec::new(),
                }));
        }
    }

    #[test]
    fn detach_clears_stale_power_links_on_1x1_and_multiblock() {
        let world = test_world();
        let battery = insert_building(&world, 10, 10, 306, 1);
        let node = insert_building(&world, 12, 10, 302, 1);
        let large = insert_building(&world, 20, 20, 217, 1);
        if let Some(mut tile) = world.tiles.get_mut(&battery) {
            tile.power_links.push(node);
        }
        if let Some(mut tile) = world.tiles.get_mut(&node) {
            tile.power_links.push(battery);
        }
        if let Some(mut tile) = world.tiles.get_mut(&large) {
            tile.power_links.push(node);
        }
        let carried = detach_building_from_world(&world, battery).unwrap();
        assert!(carried.power_links.is_empty());
        assert!(!world.tiles.contains_key(&battery));
        assert_eq!(
            world.tiles.get(&node).unwrap().power_links,
            Vec::<i32>::new()
        );

        let carried_large = detach_building_from_world(&world, large).unwrap();
        assert!(carried_large.power_links.is_empty());
        assert!(!world.tile_footprint.contains_key(&pos(21, 21)));

        let placed = attach_building_to_world(&world, carried, pos(40, 40)).unwrap();
        let _ = placed;
        assert!(world
            .tiles
            .get(&pos(40, 40))
            .unwrap()
            .power_links
            .is_empty());
    }

    #[test]
    fn pickup_in_range_and_reject_out_of_range() {
        let world = test_world();
        let origin = insert_building(&world, 5, 5, 216, 1);
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        let player = player_for(10);
        assert!(!apply_request_build_payload(&world, &player, origin).is_empty());
        assert!(!world.tiles.contains_key(&origin));
        assert_eq!(world.enemies.get(&10).unwrap().payloads.len(), 1);

        let far = insert_building(&world, 60, 60, 216, 1);
        world.enemies.insert(11, mega(11, 40.0, 40.0));
        let other = player_for(11);
        assert!(apply_request_build_payload(&world, &other, far).is_empty());
        assert!(world.tiles.contains_key(&far));
    }

    #[test]
    fn hidden_block_whole_pickup_is_rejected() {
        let world = test_world();
        let origin = insert_building(&world, 5, 5, 1, 1); // spawn, BuildVisibility.hidden
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        assert!(apply_request_build_payload(&world, &player_for(10), origin).is_empty());
        assert!(world.tiles.contains_key(&origin));
    }

    #[test]
    fn internal_payload_is_extracted_without_taking_the_building() {
        let world = test_world();
        let origin = insert_building(&world, 5, 5, 398, 1);
        let inner = DynamicTile {
            position: 0,
            block: 216,
            team: 1,
            occupied: vec![0],
            health: 1.0,
            ..DynamicTile::default()
        };
        if let Some(mut tile) = world.tiles.get_mut(&origin) {
            tile.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
                version: 0,
                tile: inner,
                sync: Vec::new(),
            })));
        }
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        assert!(!apply_request_build_payload(&world, &player_for(10), origin).is_empty());
        assert!(world.tiles.contains_key(&origin));
        assert!(world.tiles.get(&origin).unwrap().payload.is_none());
        assert_eq!(world.enemies.get(&10).unwrap().payloads.len(), 1);
    }

    #[test]
    fn grounded_ai_unit_pickup_and_flying_rejected() {
        let world = test_world();
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        world
            .enemies
            .insert(20, dagger(20, 40.0, 40.0, 0.0, UnitAuthority::DefaultAi));
        assert!(!apply_request_unit_payload(&world, &player_for(10), 20).is_empty());
        assert!(!world.enemies.contains_key(&20));

        world
            .enemies
            .insert(21, dagger(21, 40.0, 40.0, 1.0, UnitAuthority::DefaultAi));
        assert!(apply_request_unit_payload(&world, &player_for(10), 21).is_empty());
        assert!(world.enemies.contains_key(&21));
    }

    #[test]
    fn drop_within_four_tiles_keeps_position_far_drop_is_clamped() {
        let world = test_world();
        world.enemies.insert(10, mega(10, 80.0, 80.0));
        let wall = DynamicTile {
            position: 0,
            block: 216,
            team: 1,
            occupied: vec![0],
            health: 1.0,
            ..DynamicTile::default()
        };
        give_wall(&world, 10, wall.clone());
        assert!(!apply_request_drop_payload(&world, &player_for(10), 88.0, 80.0).is_empty());
        assert!(world.tiles.contains_key(&pos(11, 10)));
        assert!(world.enemies.get(&10).unwrap().payloads.is_empty());

        give_wall(&world, 10, wall);
        assert!(!apply_request_drop_payload(&world, &player_for(10), 240.0, 80.0).is_empty());
        // 20 tiles requested, clamp to 4 tiles: (80+32, 80) -> tile (14, 10)
        assert!(world.tiles.contains_key(&pos(14, 10)));
        assert!(!world.tiles.contains_key(&pos(30, 10)));
    }

    #[test]
    fn two_players_cannot_duplicate_the_same_building() {
        let world = test_world();
        let origin = insert_building(&world, 5, 5, 216, 1);
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        world.enemies.insert(11, mega(11, 40.0, 40.0));
        let first = apply_request_build_payload(&world, &player_for(10), origin);
        let second = apply_request_build_payload(&world, &player_for(11), origin);
        assert!(!first.is_empty());
        assert!(second.is_empty());
        assert_eq!(world.enemies.get(&10).unwrap().payloads.len(), 1);
        assert!(world.enemies.get(&11).unwrap().payloads.is_empty());
        assert!(!world.tiles.contains_key(&origin));
    }

    #[test]
    fn vanished_building_and_capacity_boundary_and_blocked_drop() {
        let world = test_world();
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        assert!(apply_request_build_payload(&world, &player_for(10), pos(5, 5)).is_empty());

        let exact = insert_building(&world, 5, 5, 217, 1); // 2x2 wall, area 256 == mega cap
        assert_eq!(payload_capacity(22), 256.0);
        assert!(!apply_request_build_payload(&world, &player_for(10), exact).is_empty());

        let extra = insert_building(&world, 8, 5, 216, 1);
        assert!(apply_request_build_payload(&world, &player_for(10), extra).is_empty());
        assert!(world.tiles.contains_key(&extra));

        // Blocked drop: target cell already occupied.
        world.enemies.insert(12, mega(12, 80.0, 80.0));
        let wall = DynamicTile {
            position: 0,
            block: 216,
            team: 1,
            occupied: vec![0],
            health: 1.0,
            ..DynamicTile::default()
        };
        give_wall(&world, 12, wall);
        insert_building(&world, 10, 10, 216, 1);
        assert!(apply_request_drop_payload(&world, &player_for(12), 80.0, 80.0).is_empty());
        assert_eq!(world.enemies.get(&12).unwrap().payloads.len(), 1);
    }

    #[test]
    fn dead_player_cannot_drop() {
        let world = test_world();
        world.enemies.insert(10, mega(10, 80.0, 80.0));
        give_wall(
            &world,
            10,
            DynamicTile {
                position: 0,
                block: 216,
                team: 1,
                occupied: vec![0],
                health: 1.0,
                ..DynamicTile::default()
            },
        );
        let player = player_for(10);
        world.players.insert(
            player.unit_id,
            PlayerCombatState {
                uuid: player.uuid.clone(),
                player_id: player.id,
                unit_id: player.unit_id,
                x: 80.0,
                y: 80.0,
                health: 0.0,
                shield: 0.0,
                status_effect: -1,
                status_duration: 0.0,
                statuses: Vec::new(),
                dead: true,
                respawn_timer: 0.0,
                team: 1,
            },
        );
        assert!(apply_request_drop_payload(&world, &player, 80.0, 80.0).is_empty());
        assert_eq!(world.enemies.get(&10).unwrap().payloads.len(), 1);
    }

    #[test]
    fn placement_and_removal_update_neighbor_power_links() {
        let world = test_world();
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        power::normalize_power_links(&world);
        assert!(
            world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "placing a battery next to a node autolinks"
        );

        detach_building_from_world(&world, battery);
        assert!(
            !world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "removing the battery drops the neighbor laser"
        );
    }

    #[test]
    fn power_node_autolink_respects_los_team_capacity_and_restores_after_insulated_removed() {
        // P1-12: autolink path of PowerNode.getPotentialLinks / getNodeLinks
        // 158.1 — range, insulated Bresenham, maxNodes, team, restore.
        let world = test_world();

        // 1. Valid link in range with clear line of sight.
        let node = insert_building(&world, 10, 10, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 14, 10, 306, 1);
        after_placement(&world, battery, &[]);
        assert!(
            world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "in-range clear LOS must autolink"
        );
        detach_building_from_world(&world, battery);
        detach_building_from_world(&world, node);

        // 2. Plastanium wall on the Bresenham ray blocks autolink.
        // 3. Removing it restores the laser on this same update.
        let node = insert_building(&world, 10, 20, 302, 1);
        after_placement(&world, node, &[]);
        let wall = insert_building(&world, 12, 20, 220, 1);
        let battery = insert_building(&world, 14, 20, 306, 1);
        after_placement(&world, battery, &[]);
        assert!(
            world.tiles.get(&node).unwrap().power_links.is_empty(),
            "insulated wall must block autolink"
        );
        detach_building_from_world(&world, wall);
        assert!(
            world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "removing insulated must restore autolink this tick"
        );
        detach_building_from_world(&world, battery);
        detach_building_from_world(&world, node);

        // Size-2 plastanium wall: the ray hits a non-origin footprint cell.
        let node = insert_building(&world, 10, 50, 302, 1);
        after_placement(&world, node, &[]);
        let wall = insert_building(&world, 12, 49, 221, 1);
        let battery = insert_building(&world, 16, 50, 306, 1);
        after_placement(&world, battery, &[]);
        assert!(
            world.tiles.get(&node).unwrap().power_links.is_empty(),
            "large insulated footprint must block the ray"
        );
        detach_building_from_world(&world, wall);
        assert!(
            world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "removing a large insulated wall restores autolink"
        );
        detach_building_from_world(&world, battery);
        detach_building_from_world(&world, node);

        // 5. Team mismatch does not link.
        let node = insert_building(&world, 10, 30, 302, 1);
        after_placement(&world, node, &[]);
        let enemy = insert_building(&world, 14, 30, 306, 2);
        after_placement(&world, enemy, &[]);
        assert!(
            world.tiles.get(&node).unwrap().power_links.is_empty(),
            "other-team buildings must not autolink"
        );
        detach_building_from_world(&world, enemy);
        detach_building_from_world(&world, node);

        // 4. maxNodes (PowerNode = 10) is a hard cap.
        let node = insert_building(&world, 40, 40, 302, 1);
        after_placement(&world, node, &[]);
        let offsets = [
            (3, 0),
            (-3, 0),
            (0, 3),
            (0, -3),
            (3, 3),
            (-3, -3),
            (3, -3),
            (-3, 3),
            (4, 2),
            (-4, 2),
            (4, -2),
        ];
        for (dx, dy) in offsets {
            let battery = insert_building(&world, 40 + dx, 40 + dy, 306, 1);
            after_placement(&world, battery, &[]);
        }
        assert_eq!(
            world.tiles.get(&node).unwrap().power_links.len(),
            10,
            "autolink must not exceed maxNodes"
        );

        // 6. Differential geometry vs Java Point2.pack: node (2,2) + battery (6,2).
        let java_node = insert_building(&world, 2, 2, 302, 1);
        after_placement(&world, java_node, &[]);
        let java_battery = insert_building(&world, 6, 2, 306, 1);
        after_placement(&world, java_battery, &[]);
        let mut rust_links = world.tiles.get(&java_node).unwrap().power_links.clone();
        rust_links.sort_unstable();
        assert_eq!(
            rust_links,
            vec![(6 << 16) | 2],
            "clear-LOS packed link set must match Java Point2.pack(6, 2)"
        );
    }

    #[test]
    fn change_team_splits_and_rejoins_power_graphs() {
        let world = test_world();
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        power::normalize_power_links(&world);
        assert!(world
            .tiles
            .get(&node)
            .unwrap()
            .power_links
            .contains(&battery));

        assert!(change_building_team(&world, battery, 2));
        assert_eq!(world.tiles.get(&battery).unwrap().team, 2);
        assert!(
            !world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "team change disconnects the old graph"
        );

        assert!(change_building_team(&world, battery, 1));
        power::normalize_power_links(&world);
        assert!(
            world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "returning to the original team autolinks again"
        );
    }

    #[test]
    fn multiblock_removal_clears_every_footprint_cell() {
        let world = test_world();
        let origin = insert_building(&world, 10, 10, 217, 1); // 2x2 wall
        let occupied = world.tiles.get(&origin).unwrap().occupied.clone();
        assert_eq!(occupied.len(), 4);
        for cell in &occupied {
            assert!(world.tile_footprint.contains_key(cell));
        }
        assert!(detach_building_from_world(&world, origin).is_some());
        assert!(!world.tiles.contains_key(&origin));
        for cell in occupied {
            assert!(
                !world.tile_footprint.contains_key(&cell),
                "footprint cell {cell} must vanish"
            );
        }
    }

    #[test]
    fn pickup_clears_power_links_and_drop_rebuilds_them() {
        let world = test_world();
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        assert!(!apply_request_build_payload(&world, &player_for(10), battery).is_empty());
        assert!(!world.tiles.contains_key(&battery));
        assert!(!world
            .tiles
            .get(&node)
            .unwrap()
            .power_links
            .contains(&battery));

        world.enemies.get_mut(&10).unwrap().x = 80.0;
        world.enemies.get_mut(&10).unwrap().y = 80.0;
        assert!(!apply_request_drop_payload(&world, &player_for(10), 80.0, 80.0).is_empty());
        let dropped = world
            .tiles
            .iter()
            .find(|tile| tile.block == 306)
            .map(|tile| tile.position);
        assert!(dropped.is_some());
    }

    #[test]
    fn attach_after_detach_reconstructs_power_like_a_load() {
        let world = test_world();
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        let carried = detach_building_from_world(&world, battery).unwrap();
        assert!(carried.power_links.is_empty());
        let dropped_at = pos(7, 5);
        assert!(attach_building_to_world(&world, carried, dropped_at).is_some());
        power::normalize_power_links(&world);
        assert!(
            world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&dropped_at)
                || world
                    .tiles
                    .get(&dropped_at)
                    .unwrap()
                    .power_links
                    .contains(&node),
            "load-style reconstruction restores a valid laser"
        );
    }

    #[test]
    fn remove_building_clears_stale_reverse_links() {
        let world = test_world();
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        power::normalize_power_links(&world);
        assert!(world
            .tiles
            .get(&node)
            .unwrap()
            .power_links
            .contains(&battery));

        remove_building_from_world(&world, battery);
        assert!(
            !world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "forward link must vanish"
        );
        for tile in world.tiles.iter() {
            assert!(
                !tile.power_links.contains(&battery),
                "no stale reverse link to removed building at {}",
                tile.position
            );
        }
    }

    #[test]
    fn teardown_in_place_clears_logic_executors_and_leaves_tombstone() {
        use crate::logic::compile;
        use crate::network::combat::enemy::damage_building;

        let world = test_world();
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        let program = compile("stop").expect("compile");
        world
            .logic_executors
            .insert(battery, crate::logic::ExecutorState::new(program, vec![]));
        assert!(world.logic_executors.contains_key(&battery));

        let (destroyed, _) = damage_building(&world, battery, 10_000.0).unwrap();
        assert!(destroyed);
        assert!(
            !world.logic_executors.contains_key(&battery),
            "combat teardown must drop logic executors"
        );
        assert!(!world
            .tiles
            .get(&node)
            .unwrap()
            .power_links
            .contains(&battery));
        assert_eq!(
            world.tiles.get(&battery).unwrap().block,
            0,
            "tombstone slot"
        );
        assert!(world
            .persistence_dirty
            .load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn same_tile_replacement_disconnects_old_power() {
        let world = test_world();
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        power::normalize_power_links(&world);

        remove_building_from_world(&world, battery);
        let replacement = insert_building(&world, 8, 5, 216, 1);
        after_placement(&world, replacement, &[]);
        assert!(
            !world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "same-tile replacement must not keep lasers to the old building"
        );
    }

    #[test]
    fn pickup_during_active_power_load_clears_graph() {
        let world = test_world();
        world.enemies.insert(10, mega(10, 40.0, 40.0));
        let node = insert_building(&world, 5, 5, 302, 1);
        after_placement(&world, node, &[]);
        let battery = insert_building(&world, 8, 5, 306, 1);
        after_placement(&world, battery, &[]);
        if let Some(mut tile) = world.tiles.get_mut(&battery) {
            tile.power_stored = 500.0;
        }
        assert!(!apply_request_build_payload(&world, &player_for(10), battery).is_empty());
        assert!(
            !world
                .tiles
                .get(&node)
                .unwrap()
                .power_links
                .contains(&battery),
            "pickup under load must drop neighbor lasers immediately"
        );
    }

    #[test]
    fn remove_building_clears_multiblock_footprint() {
        let world = test_world();
        let origin = insert_building(&world, 10, 10, 217, 1);
        let occupied = world.tiles.get(&origin).unwrap().occupied.clone();
        remove_building_from_world(&world, origin);
        assert!(!world.tiles.contains_key(&origin));
        for cell in occupied {
            assert!(!world.tile_footprint.contains_key(&cell));
        }
    }
}
