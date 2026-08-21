//! Unit content tables, enemy AI helpers and wave spawning.
//!
//! Sources: official v158.1 UnitTypes/Blocks content and the wave logic
//! of the Serpulo campaign.

use crate::network::combat::enemy::hostile_unit_count;
use crate::network::world::{
    core_world, ControlledUnit, DynamicWorld, EnemyUnit, SessionPlayer, UnitAuthority, UnitOrder,
    UnitOrderTarget,
};
use std::sync::atomic::Ordering;

/// Resolves a console `spawn` unit argument: a lowercase Mindustry unit name
/// or a raw numeric content id (0..=34, the unit range used by enemy_spec).
pub(crate) mod unit_orders;
pub(crate) use unit_orders::{
    advance_unit_order, apply_ordered_unit_movement, boost_should_land_near_target,
    builder_unit_hit_size, clear_logic_build_order, order_target_building_exists,
    ordered_opposing_building, ordered_unit_path, ordered_unit_step, place_logic_building,
    prune_invalid_queued_targets, route_unit_movement, unit_build_range, unit_build_speed,
    unit_construction_work, unit_has_place_plan, unit_has_stance, unit_logic_building,
    unit_logic_firing, unit_mining, unit_plan_world, unit_within_build_range, BUILDER_RANGE,
};
pub(crate) mod mining;
pub(crate) use mining::{
    build_mineable_ore_index, cell_ring, choose_mining_ore, choose_navigation_step,
    choose_navigation_step_with, enemy_navigation_target, heal_building, heal_building_for_team,
    heal_buildings_in_radius, heal_nearest_building, heal_nearest_building_by,
    heal_nearest_building_flat, move_repair_unit, move_unit_toward, navigation_tile_avoided,
    nearest_mineable_ore, unit_avoidance_requests, UnitAvoidanceRequest, ORE_CELL_TILES,
};
pub(crate) mod controller;
pub(crate) use controller::{
    apply_controller_snapshot, controlling_player_for_building, controlling_player_for_unit,
    controlling_session_for_building, controlling_session_for_unit, finalize_controller_after_load,
    read_unit_controller, roundtrip_controller_save, unit_is_player_controlled,
    unit_player_controllable, unit_save_revision, unit_uses_command_ai, valid_item_stack,
    write_carried_payload, write_unit_controller_sync, write_unit_payload, ControllerSnapshot,
};
mod spawn;
pub(crate) use spawn::EnemySpec;
pub(crate) use spawn::{
    enemy_spec, nearest_enemy_spawn, parse_unit_type, spawn_enemy_units, spawn_unit_world,
    ANTUMBRA, ATRAX, CRAWLER, DAGGER, FLARE, FORTRESS, HORIZON, MACE, MONO, NOVA, PULSAR, QUASAR,
    REIGN, RETUSA, RISSO, SCEPTER, SPIROCT, VELA, ZENITH,
};
/// Official `SpawnGroup.getSpawned(wave)` (SpawnGroup.java v158.1):
/// `min(unitAmount + (int)(((wave - begin) / spacing) / unitScaling), max)`,
/// where `unitScaling == Integer.MAX_VALUE` (`never`) contributes 0 so the
/// amount stays at `unitAmount`. No fixed 40 cap: the per-group `max` rules.
mod status;
pub(crate) use status::{
    floor_id_under_unit, immune_to_status, reapply_floor_status, status_multipliers_composite,
    tick_unit_statuses_with_floor, unit_is_grounded, unit_receives_floor_status, StatusContainer,
};

/// `UnitOrder` that exists without an active target (factory/wave default
/// setups, exhausted queues) does NOT count.
mod rules;
pub(crate) use rules::{
    arc_json_to_strict, initial_official_wave_groups, map_spawn_group_amount, map_wave_spawns,
    parse_loadout, parse_spawn_group, parse_wave_rules, parse_wave_rules_report,
    spawn_group_amount, status_effect_id_by_name, wave_spawn, wave_spawn_with_effect,
    MapSpawnGroup, SpawnGroupParse, TeamRule, WaveRules, DEFAULT_INITIAL_WAVE_SPACING,
    DEFAULT_WAVE_SPACING,
};

/// Official `CommandAI.hasCommand()` (158.1: `targetPos != null`) evaluated
/// against the persisted order. A command is active only for RTS target
/// kinds (0 = position, 1 = building, 2 = unit) whose target datum is
/// populated; kinds 6-9 are logic bindings and never RTS commands. A
mod orders;
pub(crate) use orders::{
    acquire_logic_control, block_is_logic_processor, clear_on_first_logic_takeover,
    clear_order_active_target, clear_transient_logic_orders, default_unit_authority,
    fresh_command_order, processor_lease_valid, queue_unit_target, refresh_logic_control,
    release_logic_control, reset_unit_authority, set_order_active_target, targets_equal,
    unit_bound_to_logic, unit_command_ai_reachable, unit_has_active_rts_command,
    unit_is_logic_controllable, unit_order_has_active_rts_target, LOGIC_CONTROL_TIMEOUT_TICKS,
    MAX_COMMAND_QUEUE_SIZE,
};
mod control;
pub(crate) use control::{
    acquire_command_control, apply_logic_unit_movement, detach_unit_control,
    integrate_unit_velocity, logic_accelerate_toward, logic_movement_snapshot,
    release_command_control, switch_player_unit, unit_move_physics, unit_possessed_by,
    LogicMovementSnapshot,
};

/// Official `UnitType` movement constants for LogicAI `moveAt`/`moveTo`.
#[cfg(test)]
mod status_tests {
    use super::*;
    use crate::game::status::ActiveStatus;
    use crate::network::world::EnemyUnit;

    fn unit() -> EnemyUnit {
        EnemyUnit {
            id: 1,
            unit_type: 0,
            entity_class: 0,
            team: 2,
            x: 0.0,
            y: 0.0,
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
            status_agg: Default::default(),
        }
    }

    #[test]
    fn status_entry_collection_stacks_and_composes() {
        // P1: the StatusEntry collection stacks multiple statuses with
        // official apply semantics; the legacy view mirrors the first entry.
        let mut u = unit();
        StatusContainer::apply_status(&mut u, 5, 60.0); // fast
        assert_eq!(u.statuses, vec![ActiveStatus::simple(5, 60.0)]);
        assert_eq!(u.status_effect, 5);
        assert_eq!(u.status_duration, 60.0);
        // A second status stacks; the legacy view keeps the first entry.
        StatusContainer::apply_status(&mut u, 13, 300.0); // overdrive
        assert_eq!(u.statuses.len(), 2);
        assert_eq!(u.status_effect, 5, "legacy view mirrors the first entry");
        // Reapplying refreshes the duration in place (no duplicates).
        StatusContainer::apply_status(&mut u, 5, 999.0);
        assert_eq!(u.statuses.iter().filter(|e| e.effect == 5).count(), 1);
        assert_eq!(u.statuses[0].time, 999.0);
        // M9: a shorter re-application keeps the LONGER duration
        StatusContainer::apply_status(&mut u, 5, 30.0);
        assert_eq!(u.statuses[0].time, 999.0);
        StatusContainer::apply_status(&mut u, 13, 10.0);
        assert_eq!(
            u.statuses.iter().find(|e| e.effect == 13).unwrap().time,
            300.0,
            "overdrive keeps its longer duration"
        );
        // Composite: fast (health 1, speed 1.6, damage 1) x overdrive
        // (0.95, 1.15, damage 1.4, reload 1.0). P0-08: 1.4 is damage, not reload.
        let (health, speed, damage) = u.status_multipliers_composite();
        assert!((health - 0.95).abs() < 1e-6);
        assert!((speed - 1.6 * 1.15).abs() < 1e-6);
        assert!((damage - 1.4).abs() < 1e-6);
        assert!((u.status_aggregate().reload - 1.0).abs() < 1e-6);
        StatusContainer::apply_status(&mut u, -1, 0.0);
        assert_eq!(u.statuses.len(), 2);
    }

    #[test]
    fn status_tick_expires_entries_and_resyncs_legacy_view() {
        let mut u = unit();
        StatusContainer::apply_status(&mut u, 1, 30.0); // burning, expires
        StatusContainer::apply_status(&mut u, 16, f32::MAX); // boss, permanent
        assert!(StatusContainer::tick_statuses(&mut u, 20.0));
        assert_eq!(
            u.statuses,
            vec![
                crate::game::status::ActiveStatus::simple(1, 10.0),
                crate::game::status::ActiveStatus::simple(16, f32::MAX)
            ]
        );
        assert_eq!(u.status_effect, 1);
        assert_eq!(u.status_duration, 10.0);
        assert!(StatusContainer::tick_statuses(&mut u, 10.0));
        // Burning expired; boss remains; legacy view moves to the boss.
        assert_eq!(u.statuses, vec![ActiveStatus::simple(16, f32::MAX)]);
        assert_eq!(u.status_effect, 16);
        assert_eq!(u.status_duration, f32::MAX);
        // Permanent entries never expire.
        assert!(!StatusContainer::tick_statuses(&mut u, 1_000_000.0));
        assert_eq!(u.statuses, vec![ActiveStatus::simple(16, f32::MAX)]);
        // clear resets everything.
        StatusContainer::clear_statuses(&mut u);
        assert!(u.statuses.is_empty());
        assert_eq!(u.status_effect, -1);
        assert_eq!(u.status_duration, 0.0);
    }

    fn set_floor(world: &mut DynamicWorld, tile: i32, floor: i16) {
        let index = (tile as i16 as i32) * world.width + (tile >> 16) as i16 as i32;
        world.floors[index as usize] = floor;
    }

    fn floor_test_world(mud_tile: i32) -> DynamicWorld {
        let mut world = authority_tests::authority_world();
        set_floor(&mut world, mud_tile, 42);
        world
    }

    fn unit_on_tile(tile: i32, unit_type: i16, elevation: f32) -> EnemyUnit {
        let mut u = unit();
        u.unit_type = unit_type;
        u.elevation = elevation;
        u.x = ((tile >> 16) as i16 as f32) * 8.0;
        u.y = (tile as i16 as f32) * 8.0;
        u
    }

    #[test]
    fn floor_status_applies_for_grounded_non_hovering_unit() {
        let tile = (5 << 16) | 5;
        let world = floor_test_world(tile);
        let mut u = unit_on_tile(tile, 0, 0.0);
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert_eq!(u.status_effect, 7); // muddy
        assert!((u.status_duration - 29.0).abs() < 1e-4);
    }

    #[test]
    fn floor_status_skips_hovering_unit_on_same_floor() {
        let tile = (5 << 16) | 5;
        let world = floor_test_world(tile);
        let mut u = unit_on_tile(tile, 11, 0.0); // atrax hovers
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert!(u.statuses.is_empty());
        assert_eq!(u.status_effect, -1);
    }

    #[test]
    fn floor_status_extends_same_effect_to_floor_duration_then_ticks() {
        let tile = (5 << 16) | 5;
        let world = floor_test_world(tile);
        let mut u = unit_on_tile(tile, 0, 0.0);
        StatusContainer::apply_status(&mut u, 7, 10.0);
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert!((u.status_duration - 29.0).abs() < 1e-4);
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert!((u.status_duration - 29.0).abs() < 1e-4, "stable on mud");
    }

    #[test]
    fn floor_status_opposite_triggers_transition_from_floor() {
        let tile = (6 << 16) | 5;
        let mut world = authority_tests::authority_world();
        set_floor(&mut world, tile, 22); // wet
        let mut u = unit_on_tile(tile, 0, 0.0);
        StatusContainer::apply_status(&mut u, 1, 5.0); // burning
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert_eq!(u.status_effect, 6); // wet replaced burning
        assert!((u.status_duration - 89.0).abs() < 1e-4);
    }

    #[test]
    fn floor_status_decays_after_leaving_floor_and_reapplies_on_return() {
        let mud = (5 << 16) | 5;
        let stone = (10 << 16) | 5;
        let world = floor_test_world(mud);
        let mut u = unit_on_tile(mud, 0, 0.0);
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert!((u.status_duration - 29.0).abs() < 1e-4);
        u.x = ((stone >> 16) as i16 as f32) * 8.0;
        u.y = (stone as i16 as f32) * 8.0;
        tick_unit_statuses_with_floor(&mut u, &world, 5.0);
        assert!((u.status_duration - 24.0).abs() < 1e-4);
        u.x = ((mud >> 16) as i16 as f32) * 8.0;
        u.y = (mud as i16 as f32) * 8.0;
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert!((u.status_duration - 29.0).abs() < 1e-4);
    }

    #[test]
    fn floor_status_is_noop_for_immune_unit() {
        let tile = (7 << 16) | 5;
        let mut world = authority_tests::authority_world();
        set_floor(&mut world, tile, 30); // slag
        let mut u = unit_on_tile(tile, 40, 0.0); // precept: immune to melting
        tick_unit_statuses_with_floor(&mut u, &world, 1.0);
        assert!(u.statuses.is_empty());
    }

    #[test]
    fn floor_status_honors_fractional_delta_after_reapply() {
        let tile = (5 << 16) | 5;
        let world = floor_test_world(tile);
        let mut u = unit_on_tile(tile, 0, 0.0);
        tick_unit_statuses_with_floor(&mut u, &world, 0.5);
        assert!((u.status_duration - 29.5).abs() < 1e-4);
    }
}

#[cfg(test)]
mod authority_tests {
    use super::*;
    use crate::network::world::{ControlledUnit, SessionPlayer, UnitOrder};

    pub(super) fn authority_world() -> DynamicWorld {
        let state = crate::state::game_state::GameState::new();
        state.start_hosting(
            "authority-test".into(),
            crate::state::game_state::GameMode::Survival,
        );
        DynamicWorld {
            game_state: state,
            width: 40,
            height: 40,
            sharded_unit_cap: 8,
            core_position: (20 << 16) | 20,
            core_max_health: 6_000.0,
            cores: dashmap::DashMap::new(),
            team_core_lists: dashmap::DashMap::new(),
            base_blocks: vec![0; 40 * 40],
            base_centers: vec![false; 40 * 40],
            tile_data: Vec::new(),
            base_building_templates: Vec::new(),
            base_buildings: dashmap::DashMap::new(),
            floors: vec![0; 40 * 40],
            overlays: vec![0; 40 * 40],
            enemy_spawns: Vec::new(),
            enemies: dashmap::DashMap::new(),
            players: dashmap::DashMap::new(),
            player_sessions: dashmap::DashMap::new(),
            player_profiles: dashmap::DashMap::new(),
            building_commands: dashmap::DashMap::new(),
            unit_orders: dashmap::DashMap::new(),
            next_player_unit_id: std::sync::atomic::AtomicI32::new(2_500_000),
            next_enemy_id: std::sync::atomic::AtomicI32::new(3_000_100),
            unit_group_order: parking_lot::Mutex::new(Vec::new()),
            projectiles: dashmap::DashMap::new(),
            next_projectile_id: std::sync::atomic::AtomicI32::new(4_000_000),
            overdrive_boosts: dashmap::DashMap::new(),
            heal_suppression: dashmap::DashMap::new(),
            force_fields: dashmap::DashMap::new(),
            tiles: dashmap::DashMap::new(),
            pending_builds: dashmap::DashMap::new(),
            pending_breaks: dashmap::DashMap::new(),
            mineable_ore: std::sync::OnceLock::new(),
            mono_mining_targets: dashmap::DashMap::new(),
            tile_footprint: dashmap::DashMap::new(),
            navigation_revision: std::sync::atomic::AtomicU64::new(0),
            ground_navigation: parking_lot::Mutex::new(None),
            leg_navigation: parking_lot::Mutex::new(None),
            save_path: std::env::temp_dir().join("authority-functional-test.json"),
            network_template: std::sync::Arc::new(Vec::new()),
            persistence_dirty: std::sync::atomic::AtomicBool::new(false),
            persistence_lock: parking_lot::Mutex::new(()),
            logic_flags: dashmap::DashMap::new(),
            logic_executors: dashmap::DashMap::new(),
            logic_display_commands: dashmap::DashMap::new(),
            base_drill_progress: dashmap::DashMap::new(),
            base_factory_progress: dashmap::DashMap::new(),
            base_turret_progress: dashmap::DashMap::new(),
            base_mender_progress: dashmap::DashMap::new(),
            team_build_plans: parking_lot::RwLock::new(crate::engine::typeio::TeamBlocks::default()),
            wave_rules: parking_lot::RwLock::new(WaveRules::default()),
            votekick_target: parking_lot::RwLock::new(None),
            votekick_votes: std::sync::atomic::AtomicI32::new(0),
            votekick_voters: dashmap::DashMap::new(),
            votekick_cooldowns: dashmap::DashMap::new(),
            puddles: crate::network::buildings::puddles::PuddleSystem::new(),
        }
    }

    pub(super) fn unit(id: i32, team: u8, authority: UnitAuthority) -> EnemyUnit {
        EnemyUnit {
            id,
            unit_type: 0,
            entity_class: 0,
            team,
            x: 0.0,
            y: 0.0,
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
            authority,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: Default::default(),
        }
    }

    pub(super) fn session(id: i32) -> SessionPlayer {
        SessionPlayer {
            id,
            controlled_unit: ControlledUnit::Core,
            unit_id: 2_500_000 + id,
            uuid: format!("authority-uuid-{id}"),
            name: "authority".into(),
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

    pub(super) fn default_order(id: i32) -> UnitOrder {
        UnitOrder {
            unit_id: id,
            command: 0,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: 0,
            queue: Vec::new(),
        }
    }

    fn authority_of(world: &DynamicWorld, id: i32) -> UnitAuthority {
        world.enemies.get(&id).unwrap().authority
    }

    #[test]
    fn new_unit_gets_default_authority() {
        let world = authority_world();
        // Fresh spawns (console/wave, team 2) start at DefaultAi.
        assert_eq!(spawn_enemy_units(&world, 0, 2, Some(10), Some(10)), 2);
        for entry in world.enemies.iter() {
            assert_eq!(entry.value().authority, UnitAuthority::DefaultAi);
        }
        // `type.createController` for a player-commandable team (survival
        // default team 1) is CommandAI — Command authority with no active
        // target.
        let ally = unit(3_000_050, 1, UnitAuthority::DefaultAi);
        assert_eq!(
            default_unit_authority(&world, &ally),
            UnitAuthority::Command
        );
        // Non-default AI teams (wave AI) fall back to DefaultAi unless rtsAi.
        let wave = unit(3_000_051, 2, UnitAuthority::DefaultAi);
        assert_eq!(
            default_unit_authority(&world, &wave),
            UnitAuthority::DefaultAi
        );
        world.wave_rules.write().team_rules.insert(
            2,
            TeamRule {
                rts_ai: true,
                ..Default::default()
            },
        );
        world.wave_rules.write().waves_enabled = true;
        assert_eq!(
            default_unit_authority(&world, &wave),
            UnitAuthority::Command
        );
    }

    #[test]
    fn rts_ai_pvp_and_build_ai_pvp_gate() {
        let world = authority_world();
        world.wave_rules.write().waves_enabled = true;
        world.wave_rules.write().team_rules.insert(
            2,
            TeamRule {
                rts_ai: false,
                build_ai: true,
                ..Default::default()
            },
        );
        // Survival: wave team without rtsAi stays on DefaultAi.
        let wave = unit(3_000_052, 2, UnitAuthority::DefaultAi);
        assert_eq!(
            default_unit_authority(&world, &wave),
            UnitAuthority::DefaultAi
        );
        // PvP: isAI() is false — all player-controllable units get CommandAI.
        *world.game_state.mode.write() = crate::state::game_state::GameMode::Pvp;
        assert_eq!(
            default_unit_authority(&world, &wave),
            UnitAuthority::Command
        );
        // BaseBuilderAI gate: buildAi is suppressed in PvP.
        let rules = world.wave_rules.read();
        assert!(rules.team_build_ai_enabled(2, false));
        assert!(!rules.team_build_ai_enabled(2, true));
    }

    #[test]
    fn unit_without_active_rts_target_is_logic_controllable() {
        let world = authority_world();
        world
            .enemies
            .insert(3_000_100, unit(3_000_100, 1, UnitAuthority::Command));
        // No order at all.
        assert!(unit_is_logic_controllable(&world, 3_000_100));
        // Default (empty) order, as factory/wave spawns create.
        world
            .unit_orders
            .insert(3_000_100, default_order(3_000_100));
        assert!(unit_is_logic_controllable(&world, 3_000_100));
        // Exhausted attack order: kind 1 with no target id.
        let mut order = default_order(3_000_100);
        order.target_kind = 1;
        world.unit_orders.insert(3_000_100, order);
        assert!(unit_is_logic_controllable(&world, 3_000_100));
        // Logic authority stays controllable (LogicAI keeps renewing).
        if let Some(mut live) = world.enemies.get_mut(&3_000_100) {
            live.authority = UnitAuthority::Logic {
                processor_pos: 5,
                remaining_ticks: 60.0,
                processor_generation: 0,
            };
        }
        assert!(unit_is_logic_controllable(&world, 3_000_100));
    }

    #[test]
    fn unit_with_active_rts_target_is_not_logic_controllable() {
        let world = authority_world();
        world
            .enemies
            .insert(3_000_200, unit(3_000_200, 1, UnitAuthority::Command));
        for (kind, id, x) in [(0_u8, -1_i32, Some(96.0_f32)), (1, 42, None), (2, 7, None)] {
            let mut order = default_order(3_000_200);
            order.target_kind = kind;
            order.target_id = id;
            order.target_x = x;
            world.unit_orders.insert(3_000_200, order);
            assert!(
                !unit_is_logic_controllable(&world, 3_000_200),
                "kind {kind} with a live target must block logic control"
            );
            assert!(!acquire_logic_control(&world, 3_000_200, 5, 60.0));
            assert_eq!(authority_of(&world, 3_000_200), UnitAuthority::Command);
        }
        // Player authority always blocks logic control (Player controller).
        world.unit_orders.remove(&3_000_200);
        if let Some(mut live) = world.enemies.get_mut(&3_000_200) {
            live.authority = UnitAuthority::Player { player_id: 11 };
        }
        assert!(!unit_is_logic_controllable(&world, 3_000_200));
    }

    #[test]
    fn reset_returns_to_default_authority() {
        let world = authority_world();
        world.enemies.insert(
            3_000_300,
            unit(3_000_300, 1, UnitAuthority::Player { player_id: 3 }),
        );
        reset_unit_authority(&world, 3_000_300);
        assert_eq!(authority_of(&world, 3_000_300), UnitAuthority::Command);
        world.enemies.insert(
            3_000_301,
            unit(
                3_000_301,
                2,
                UnitAuthority::Logic {
                    processor_pos: 9,
                    remaining_ticks: 12.0,
                    processor_generation: 0,
                },
            ),
        );
        reset_unit_authority(&world, 3_000_301);
        assert_eq!(authority_of(&world, 3_000_301), UnitAuthority::DefaultAi);
        // Resetting an unknown unit is a no-op.
        reset_unit_authority(&world, 999);
    }

    #[test]
    fn default_order_existence_does_not_force_command_authority() {
        let world = authority_world();
        // A wave unit with the default order factory/wave spawns install:
        // the order exists, but nothing commands the unit.
        world
            .enemies
            .insert(3_000_400, unit(3_000_400, 2, UnitAuthority::DefaultAi));
        world
            .unit_orders
            .insert(3_000_400, default_order(3_000_400));
        assert!(world.unit_orders.contains_key(&3_000_400));
        assert_eq!(authority_of(&world, 3_000_400), UnitAuthority::DefaultAi);
        assert!(unit_is_logic_controllable(&world, 3_000_400));
        assert_eq!(unit_possessed_by(&world, 3_000_400), None);
        // A queued but inactive order is not a command either.
        let mut queued = default_order(3_000_400);
        queued.queue.push(crate::network::world::UnitOrderTarget {
            kind: 0,
            id: -1,
            x: 8.0,
            y: 8.0,
        });
        world.unit_orders.insert(3_000_400, queued);
        assert_eq!(authority_of(&world, 3_000_400), UnitAuthority::DefaultAi);
    }

    #[test]
    fn unit_dying_under_logic_authority_leaves_no_residue() {
        let world = authority_world();
        let mut logic_unit = unit(3_000_500, 1, UnitAuthority::DefaultAi);
        world.enemies.insert(3_000_500, logic_unit.clone());
        let mut order = default_order(3_000_500);
        order.target_kind = 6; // logic mine binding
        world.unit_orders.insert(3_000_500, order);
        assert!(acquire_logic_control(&world, 3_000_500, 21, 60.0));
        logic_unit.authority = UnitAuthority::Logic {
            processor_pos: 21,
            remaining_ticks: 60.0,
            processor_generation: 0,
        };
        // Death: the unit entry and its control associations disappear.
        world.enemies.remove(&3_000_500);
        detach_unit_control(&world, 3_000_500);
        assert!(!world.enemies.contains_key(&3_000_500));
        assert!(!world.unit_orders.contains_key(&3_000_500));
        // Queries on the dead unit answer safely instead of panicking.
        assert!(!unit_is_logic_controllable(&world, 3_000_500));
        assert_eq!(unit_possessed_by(&world, 3_000_500), None);
    }

    #[test]
    fn unit_dying_under_player_authority_releases_possession() {
        let world = authority_world();
        world.enemies.insert(
            3_000_600,
            unit(3_000_600, 1, UnitAuthority::Player { player_id: 4 }),
        );
        let mut possessing = session(4);
        possessing.controlled_unit = ControlledUnit::Standard(3_000_600);
        world.player_sessions.insert(possessing.unit_id, possessing);
        // Death (kill_enemy path): the order dies with the unit and the
        // session returns to its core avatar.
        world.enemies.remove(&3_000_600);
        detach_unit_control(&world, 3_000_600);
        assert!(!world.enemies.contains_key(&3_000_600));
        assert!(!world.unit_orders.contains_key(&3_000_600));
        let session = world
            .player_sessions
            .get(&2_500_004)
            .expect("session survives the unit");
        assert_eq!(session.controlled_unit, ControlledUnit::Core);
        assert_eq!(unit_possessed_by(&world, 3_000_600), None);
    }

    #[test]
    fn target_removed_but_order_remains_resets_authority() {
        let world = authority_world();
        // Team 5 in survival is not player-commandable, so its default
        // authority is DefaultAi and a reset is observable.
        world
            .enemies
            .insert(3_000_700, unit(3_000_700, 5, UnitAuthority::Command));
        let mut order = default_order(3_000_700);
        order.target_kind = 2; // attack-unit target that no longer exists
        order.target_id = 3_000_999;
        world.unit_orders.insert(3_000_700, order);
        assert!(!unit_is_logic_controllable(&world, 3_000_700));
        // The dead-target path advances the order: with an empty queue the
        // target is cleared but the UnitOrder itself REMAINS.
        crate::network::units::unit_orders::advance_unit_order(&world, 3_000_700);
        let order = world
            .unit_orders
            .get(&3_000_700)
            .expect("order survives target loss");
        assert_eq!(order.target_kind, 0);
        assert_eq!(order.target_x, None);
        drop(order);
        assert_eq!(authority_of(&world, 3_000_700), UnitAuthority::DefaultAi);
        assert!(unit_is_logic_controllable(&world, 3_000_700));
    }

    #[test]
    fn team_change_leaves_no_inconsistent_authority() {
        let world = authority_world();
        // A possessed team-1 unit changes team (e.g. campaign conversion):
        // resetting its controller yields the new team's default, never a
        // stale Player marker.
        let mut converted = unit(3_000_800, 1, UnitAuthority::Player { player_id: 6 });
        world.enemies.insert(3_000_800, converted.clone());
        converted.team = 5;
        if let Some(mut live) = world.enemies.get_mut(&3_000_800) {
            live.team = 5;
        }
        reset_unit_authority(&world, 3_000_800);
        assert_eq!(authority_of(&world, 3_000_800), UnitAuthority::DefaultAi);
        assert!(unit_is_logic_controllable(&world, 3_000_800));
        // Same for a Command-authority unit switching to a wave team.
        world
            .enemies
            .insert(3_000_801, unit(3_000_801, 1, UnitAuthority::Command));
        if let Some(mut live) = world.enemies.get_mut(&3_000_801) {
            live.team = 2;
        }
        reset_unit_authority(&world, 3_000_801);
        assert_eq!(authority_of(&world, 3_000_801), UnitAuthority::DefaultAi);
    }
}

/// P0-04: RTS CommandAI order semantics — queue cap, dedup, first-target
/// promotion, invalidation, progression and their persistence. Java sources:
/// CommandAI.java (commandQueue 493-503, maxCommandQueueSize 19, queue
/// invalidation 136-139, finishPath 412-486, hasCommand 549-551) and
/// InputHandler.java (commandUnits 308-407).
#[cfg(test)]
mod command_queue_tests {
    use super::authority_tests::{authority_world, default_order, unit};
    use super::*;
    use crate::network::decoders::{apply_command_units_for_team, CommandUnitsRequest};
    use crate::network::units::unit_orders::{advance_unit_order, apply_ordered_unit_movement};
    use crate::network::world::{DynamicTile, PersistedWorld, UnitOrderTarget};

    const ACTOR: u8 = 1;

    fn queued_request(unit_id: i32, x: f32, y: f32) -> CommandUnitsRequest {
        CommandUnitsRequest {
            unit_ids: vec![unit_id],
            build_target: -1,
            unit_target_type: 0,
            unit_target_id: -1,
            pos_x: x,
            pos_y: y,
            queue_command: true,
            final_batch: true,
        }
    }

    fn position(x: f32, y: f32) -> UnitOrderTarget {
        UnitOrderTarget {
            kind: 0,
            id: -1,
            x,
            y,
        }
    }

    fn order_of(world: &DynamicWorld, id: i32) -> UnitOrder {
        world.unit_orders.get(&id).unwrap().clone()
    }

    fn authority_of(world: &DynamicWorld, id: i32) -> UnitAuthority {
        world.enemies.get(&id).unwrap().authority
    }

    /// Tests 1+2 — `maxCommandQueueSize = 50` (CommandAI.java:19): fifty
    /// unique queued targets fit, the fifty-first is rejected.
    #[test]
    fn command_queue_accepts_fifty_and_rejects_the_fifty_first() {
        assert_eq!(MAX_COMMAND_QUEUE_SIZE, 50);
        let world = authority_world();
        let id = 3_001_001;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut order = default_order(id);
        set_order_active_target(&mut order, position(0.0, 0.0));
        world.unit_orders.insert(id, order);
        for i in 1..=50_i32 {
            assert!(queue_unit_target(
                &mut world.unit_orders.get_mut(&id).unwrap(),
                position(i as f32 * 8.0, 16.0)
            ));
        }
        // The 51st UNIQUE target is rejected (cap), queue stays at 50.
        assert!(!queue_unit_target(
            &mut world.unit_orders.get_mut(&id).unwrap(),
            position(1_000.0, 1_000.0)
        ));
        assert_eq!(order_of(&world, id).queue.len(), 50);
    }

    /// Test 3 — `!commandQueue.contains(location)` (CommandAI.java:500).
    #[test]
    fn duplicate_queued_target_is_not_appended() {
        let world = authority_world();
        let id = 3_001_002;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut order = default_order(id);
        set_order_active_target(&mut order, position(0.0, 0.0));
        world.unit_orders.insert(id, order);
        let mut live = world.unit_orders.get_mut(&id).unwrap();
        assert!(queue_unit_target(&mut live, position(64.0, 64.0)));
        assert!(!queue_unit_target(&mut live, position(64.0, 64.0)));
        assert_eq!(live.queue.len(), 1);
    }

    /// Test 4 — `commandQueue(Position)`: with no active target the first
    /// queued command is consumed as the ACTIVE command, never queued
    /// (CommandAI.java:494-499), and activates Command authority.
    #[test]
    fn first_queued_command_is_promoted_to_active() {
        let world = authority_world();
        let id = 3_001_003;
        // Realistic state: a stop stance cleared the target but left the order.
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::DefaultAi));
        world.unit_orders.insert(id, default_order(id));
        assert!(apply_command_units_for_team(
            &world,
            ACTOR,
            &queued_request(id, 200.0, 300.0)
        ));
        let order = order_of(&world, id);
        assert_eq!((order.target_kind, order.target_id), (0, -1));
        assert_eq!((order.target_x, order.target_y), (Some(200.0), Some(300.0)));
        assert!(order.queue.is_empty());
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
    }

    /// InputHandler.java:334-337 — commanding implicitly switches the unit
    /// back to move for every non-payload command (`switchToMove`,
    /// UnitCommand.java: false only for enterPayload..loopPayload, ids 5-9).
    #[test]
    fn commanding_switches_non_payload_commands_back_to_move() {
        let world = authority_world();
        let id = 3_001_004;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut repairing = default_order(id);
        repairing.command = 1; // repair
        world.unit_orders.insert(id, repairing);
        assert!(apply_command_units_for_team(
            &world,
            ACTOR,
            &queued_request(id, 90.0, 90.0)
        ));
        assert_eq!(order_of(&world, id).command, 0);
        // Payload commands keep their command byte.
        let mut loader = default_order(id);
        loader.command = 6; // loadUnits: switchToMove = false
        world.unit_orders.insert(id, loader);
        assert!(apply_command_units_for_team(
            &world,
            ACTOR,
            &queued_request(id, 95.0, 95.0)
        ));
        assert_eq!(order_of(&world, id).command, 6);
    }

    /// InputHandler.java:346-354 — a direct (non-queued) command clears the
    /// queue and sets the active target while stances survive.
    #[test]
    fn direct_command_clears_queue_and_keeps_stances() {
        let world = authority_world();
        let id = 3_001_005;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut order = default_order(id);
        order.stances = 1 << 1;
        set_order_active_target(&mut order, position(10.0, 10.0));
        order.queue = vec![position(20.0, 20.0), position(30.0, 30.0)];
        world.unit_orders.insert(id, order);
        let mut direct = queued_request(id, 400.0, 500.0);
        direct.queue_command = false;
        assert!(apply_command_units_for_team(&world, ACTOR, &direct));
        let order = order_of(&world, id);
        assert_eq!((order.target_x, order.target_y), (Some(400.0), Some(500.0)));
        assert!(order.queue.is_empty());
        assert_eq!(order.stances, 1 << 1);
    }

    /// Test 5 — `finishPath` pops queue entries in order
    /// (CommandAI.java:465-471); FIFO.
    #[test]
    fn finishing_active_target_advances_to_next_in_order() {
        let world = authority_world();
        let id = 3_001_006;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut order = default_order(id);
        set_order_active_target(&mut order, position(8.0, 0.0)); // within stop distance
        order.queue = vec![position(64.0, 0.0), position(128.0, 0.0)];
        world.unit_orders.insert(id, order);
        let snapshot = world.enemies.get(&id).unwrap().clone();
        assert!(apply_ordered_unit_movement(&world, &snapshot, 1.0));
        let order = order_of(&world, id);
        assert_eq!(order.target_x, Some(64.0));
        assert_eq!(order.queue.len(), 1);
        assert_eq!(order.queue[0].x, 128.0);
        // Patrol (stance bit 3) re-queues the previous targetPos as a plain
        // position (CommandAI.java:473-475).
        let mut order = default_order(id);
        order.stances = 1 << 3;
        set_order_active_target(&mut order, position(8.0, 0.0));
        order.queue = vec![position(64.0, 0.0)];
        world.unit_orders.insert(id, order);
        advance_unit_order(&world, id);
        let order = order_of(&world, id);
        assert_eq!(order.target_x, Some(64.0));
        assert_eq!(order.queue.len(), 1);
        assert_eq!(
            (order.queue[0].kind, order.queue[0].id, order.queue[0].x),
            (0, -1, 8.0)
        );
    }

    /// Test 6 — the last target finishing clears the command: hasCommand()
    /// false, Command authority released, logic-controllable again
    /// (CommandAI.java:391-396 + 462-463).
    #[test]
    fn finishing_final_target_restores_logic_controllable() {
        let world = authority_world();
        let id = 3_001_007;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut order = default_order(id);
        set_order_active_target(&mut order, position(8.0, 0.0));
        world.unit_orders.insert(id, order);
        assert!(!unit_is_logic_controllable(&world, id));
        let snapshot = world.enemies.get(&id).unwrap().clone();
        assert!(apply_ordered_unit_movement(&world, &snapshot, 1.0));
        let order = order_of(&world, id);
        assert_eq!((order.target_kind, order.target_id), (0, -1));
        assert_eq!((order.target_x, order.target_y), (None, None));
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
        assert!(unit_is_logic_controllable(&world, id));
    }

    /// Test 7 — an active RTS command blocks logic control at P0-03's
    /// refresh gate; exhausting the order un-blocks it.
    #[test]
    fn active_rts_command_blocks_ucontrol_refresh() {
        let world = authority_world();
        let id = 3_001_008;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut order = default_order(id);
        set_order_active_target(&mut order, position(500.0, 500.0));
        order.queue = vec![position(600.0, 600.0)];
        world.unit_orders.insert(id, order);
        assert!(!refresh_logic_control(&world, Some(id), 21, ACTOR, false));
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
        // Exhaust: active + one queued entry.
        advance_unit_order(&world, id);
        assert!(!refresh_logic_control(&world, Some(id), 21, ACTOR, false));
        advance_unit_order(&world, id);
        assert!(refresh_logic_control(&world, Some(id), 21, ACTOR, false));
    }

    /// Test 8 — a queued unit target that dies is pruned from the queue
    /// (CommandAI.java:136-139).
    #[test]
    fn dead_queued_unit_is_pruned_from_queue() {
        let world = authority_world();
        let id = 3_001_009;
        let victim = 3_001_050;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        world
            .enemies
            .insert(victim, unit(victim, 2, UnitAuthority::DefaultAi));
        let mut order = default_order(id);
        set_order_active_target(&mut order, position(500.0, 500.0));
        order.queue = vec![
            UnitOrderTarget {
                kind: 2,
                id: victim,
                x: 100.0,
                y: 100.0,
            },
            position(640.0, 640.0),
        ];
        world.unit_orders.insert(id, order);
        world.enemies.remove(&victim);
        let snapshot = world.enemies.get(&id).unwrap().clone();
        assert!(apply_ordered_unit_movement(&world, &snapshot, 1.0));
        let queue = order_of(&world, id).queue;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].kind, 0);
    }

    /// Test 9 — a queued building target whose building was destroyed is
    /// pruned from the queue (CommandAI.java:136-139).
    #[test]
    fn removed_queued_building_is_pruned_from_queue() {
        let world = authority_world();
        let id = 3_001_010;
        let wall_position = (30 << 16) | 30;
        let wall = DynamicTile {
            position: wall_position,
            block: 216,
            team: 2,
            ..Default::default()
        };
        world.tiles.insert(wall_position, wall);
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let mut order = default_order(id);
        set_order_active_target(&mut order, position(500.0, 500.0));
        order.queue = vec![
            UnitOrderTarget {
                kind: 1,
                id: wall_position,
                x: 240.0,
                y: 240.0,
            },
            position(640.0, 640.0),
        ];
        world.unit_orders.insert(id, order);
        // Still standing: nothing pruned.
        let snapshot = world.enemies.get(&id).unwrap().clone();
        assert!(apply_ordered_unit_movement(&world, &snapshot, 1.0));
        assert_eq!(order_of(&world, id).queue.len(), 2);
        // Destroyed: pruned, the position entry survives.
        world.tiles.remove(&wall_position);
        assert!(apply_ordered_unit_movement(&world, &snapshot, 1.0));
        let queue = order_of(&world, id).queue;
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].kind, 0);
    }

    /// Test 10 — Vec2 equality is by exact coordinates, not instance
    /// (Arc Vec2.java:611-618), so distinct instances with equal coords are
    /// queue duplicates.
    #[test]
    fn distinct_vec2_instances_with_equal_coords_are_duplicates() {
        let a = position(123.5, -7.25);
        let b = position(123.5, -7.25);
        assert_ne!(a.id, b.id + 1); // sanity: separate values
        assert!(targets_equal(&a, &b));
        // A coordinate difference is NOT a duplicate (exact bits, like
        // Float.floatToIntBits — a delta too small to change the f32 bits
        // would compare equal in Java too).
        assert!(!targets_equal(&a, &position(123.75, -7.25)));
        // Cross-kind never matches (class check in Vec2.equals / identity
        // for entities).
        let building = UnitOrderTarget {
            kind: 1,
            id: 42,
            x: 123.5,
            y: -7.25,
        };
        assert!(!targets_equal(&a, &building));
        let unit_target = UnitOrderTarget {
            kind: 2,
            id: 42,
            x: 123.5,
            y: -7.25,
        };
        assert!(!targets_equal(&building, &unit_target));
        // Same id, same kind: the same entity — duplicate.
        assert!(targets_equal(
            &UnitOrderTarget {
                kind: 1,
                id: 42,
                x: 0.0,
                y: 0.0
            },
            &UnitOrderTarget {
                kind: 1,
                id: 42,
                x: 9.0,
                y: 9.0
            }
        ));
    }

    /// Test 11 — the same queued CommandUnits RPC replayed within one tick:
    /// the first promotes to active, the second queues once, any further
    /// replay never grows the queue (CommandAI.commandQueue dedup).
    #[test]
    fn identical_rpc_repeated_in_one_tick_does_not_duplicate() {
        let world = authority_world();
        let id = 3_001_011;
        world
            .enemies
            .insert(id, unit(id, ACTOR, UnitAuthority::Command));
        let request = queued_request(id, 320.0, 480.0);
        assert!(apply_command_units_for_team(&world, ACTOR, &request));
        let order = order_of(&world, id);
        assert_eq!((order.target_x, order.target_y), (Some(320.0), Some(480.0)));
        assert!(order.queue.is_empty());
        assert!(apply_command_units_for_team(&world, ACTOR, &request));
        assert_eq!(order_of(&world, id).queue.len(), 1);
        assert!(!apply_command_units_for_team(&world, ACTOR, &request));
        assert_eq!(order_of(&world, id).queue.len(), 1);
    }

    /// Test 12 — the JSON checkpoint round-trip preserves the command byte,
    /// stance bitset, active target and the full queue in order.
    #[test]
    fn checkpoint_roundtrip_preserves_queue_and_stances() {
        let mut order = default_order(3_001_012);
        order.command = 2;
        order.stances = (1 << 1) | (1 << 3);
        set_order_active_target(&mut order, position(88.0, 96.0));
        order.queue = (0..50_i32)
            .map(|i| UnitOrderTarget {
                kind: 0,
                id: -1,
                x: i as f32 * 8.0,
                y: 16.0,
            })
            .collect();
        order.queue.push(UnitOrderTarget {
            kind: 2,
            id: 77,
            x: 1.0,
            y: 2.0,
        });
        let saved = PersistedWorld {
            version: 14,
            map_name: "command-queue-roundtrip".into(),
            tiles: Vec::new(),
            core_items: Vec::new(),
            wave: 1,
            wave_time: 180.0,
            core_health: 6_000.0,
            enemies: Vec::new(),
            base_building_health: Vec::new(),
            players: Vec::new(),
            building_commands: Vec::new(),
            unit_orders: vec![order],
            team_build_plans: Default::default(),
            team_cores: Vec::new(),
            team_items: Vec::new(),
            simulation_time: 0.0,
            logic_flags: Vec::new(),
            game_stats: Default::default(),
            puddles: Vec::new(),
        };
        let json = serde_json::to_string(&saved).expect("checkpoint serializes");
        let loaded: PersistedWorld = serde_json::from_str(&json).expect("checkpoint parses");
        let order = &loaded.unit_orders[0];
        assert_eq!(order.command, 2);
        assert_eq!(order.stances, (1 << 1) | (1 << 3));
        assert_eq!(
            (order.target_kind, order.target_x, order.target_y),
            (0, Some(88.0), Some(96.0))
        );
        assert_eq!(order.queue.len(), 51);
        assert_eq!((order.queue[0].x, order.queue[49].x), (0.0, 392.0));
        assert_eq!((order.queue[50].kind, order.queue[50].id), (2, 77));
    }
}

/// P1-A1: same-tick unit-update → logic ordering for CommandAI targets.
/// Java materializes `targetPos` during `CommandAI.updateUnit` before
/// `LogicBlock.updateTile`; Logic's `checkLogicAI` gate therefore never
/// sees the attackTarget-only transient (ParCommandTiming158).
#[cfg(test)]
mod command_timing_tests {
    use super::authority_tests::{authority_world, default_order, unit};
    use super::*;
    use crate::logic::{compile, ExecutorState, WorldView};
    use crate::network::units::unit_orders::apply_ordered_unit_movement;
    use crate::network::world::{UnitAuthority, UnitOrderTarget};
    use dashmap::DashMap;
    use std::sync::Arc;

    const UNIT: i32 = 3_000_100;
    const FOE: i32 = 3_000_101;
    const PROC: i32 = (7 << 16) | 7;
    const WALL: i32 = (8 << 16) | 8;

    fn timing_world() -> Arc<DynamicWorld> {
        let world = Arc::new(authority_world());
        world
            .enemies
            .insert(UNIT, unit(UNIT, 1, UnitAuthority::Command));
        world
            .enemies
            .insert(FOE, unit(FOE, 6, UnitAuthority::DefaultAi));
        world.unit_orders.insert(UNIT, default_order(UNIT));
        let mut proc = crate::network::world::DynamicTile {
            position: PROC,
            block: 431,
            team: 1,
            ..Default::default()
        };
        crate::network::world::stamp_new_building(&world, &mut proc);
        world.tiles.insert(PROC, proc);
        world
    }

    fn issue_building(world: &DynamicWorld) {
        let wall = crate::network::world::DynamicTile {
            position: WALL,
            block: 22,
            team: 6,
            ..Default::default()
        };
        world.tiles.insert(WALL, wall);
        if let Some(mut order) = world.unit_orders.get_mut(&UNIT) {
            set_order_active_target(
                &mut order,
                UnitOrderTarget {
                    kind: 1,
                    id: WALL,
                    x: 68.0,
                    y: 68.0,
                },
            );
        }
        acquire_command_control(world, UNIT);
    }

    fn unit_tick(world: &DynamicWorld) {
        let snapshot = world.enemies.get(&UNIT).unwrap().clone();
        apply_ordered_unit_movement(world, &snapshot, 1.0);
    }

    fn ucontrol_tick(world: &DynamicWorld) -> bool {
        let program = compile("ucontrol move 50 50\nstop").unwrap();
        let mut state = ExecutorState::new(program, Vec::new());
        state.bound_unit = Some(UNIT);
        let connections = DashMap::new();
        let view = WorldView {
            world,
            processor_pos: PROC,
            out: &connections,
        };
        crate::network::simulation::simulate_logic_control_leases(world, 1.0);
        state.run_tick(Some(&view), 1);
        matches!(
            world.enemies.get(&UNIT).map(|unit| unit.authority),
            Some(UnitAuthority::Logic { .. })
        )
    }

    #[test]
    fn p1a1_logic_gate_sees_post_unit_update_not_transient() {
        let world = timing_world();
        issue_building(&world);
        // Rust materializes immediately — active before the unit tick.
        assert!(
            unit_has_active_rts_command(&world, UNIT),
            "Rust eager slot is active right after commandTarget"
        );
        assert!(
            !unit_is_logic_controllable(&world, UNIT),
            "logic gate would block ucontrol if it ran before the unit tick"
        );

        unit_tick(&world);
        assert!(
            unit_has_active_rts_command(&world, UNIT),
            "post-update: Java targetPos materialized, hasCommand true"
        );
        assert!(!unit_is_logic_controllable(&world, UNIT));

        assert!(
            !ucontrol_tick(&world),
            "same-tick logic phase: ucontrol blocked while RTS command active"
        );
    }

    #[test]
    fn p1a1_invalid_target_clears_before_logic_and_ucontrol_succeeds() {
        let world = timing_world();
        issue_building(&world);
        world.tiles.remove(&WALL);
        unit_tick(&world);
        assert!(!unit_has_active_rts_command(&world, UNIT));
        assert!(unit_is_logic_controllable(&world, UNIT));
        assert!(ucontrol_tick(&world));
    }
}

/// P1-B1: LogicAI ucontrol move tick ordering (ParLogicMoveTiming158).
#[cfg(test)]
mod logic_move_timing_tests {
    use super::authority_tests::{authority_world, default_order, unit};
    use super::*;
    use crate::logic::{compile, ExecutorState, WorldView};
    use crate::network::simulation::simulate_logic_control_leases;
    use crate::network::world::{logic_control, UnitAuthority};
    use dashmap::DashMap;
    use std::sync::Arc;

    const UNIT: i32 = 3_000_300;
    const PROC: i32 = (7 << 16) | 7;

    fn timing_world() -> Arc<DynamicWorld> {
        let world = Arc::new(authority_world());
        let mut flare = unit(UNIT, 1, UnitAuthority::Command);
        flare.unit_type = FLARE.unit_type;
        flare.move_speed = FLARE.speed;
        flare.x = 80.0;
        flare.y = 80.0;
        flare.elevation = 1.0;
        world.enemies.insert(UNIT, flare);
        world.unit_orders.insert(UNIT, default_order(UNIT));
        let mut proc = crate::network::world::DynamicTile {
            position: PROC,
            block: 431,
            team: 1,
            ..Default::default()
        };
        crate::network::world::stamp_new_building(&world, &mut proc);
        world.tiles.insert(PROC, proc);
        world
    }

    fn unit_tick(world: &DynamicWorld) {
        simulate_logic_control_leases(world, 1.0);
        let snapshot = world.enemies.get(&UNIT).unwrap().clone();
        apply_logic_unit_movement(world, &snapshot, 1.0);
    }

    fn ucontrol(world: &DynamicWorld, src: &str) {
        let program = compile(&format!("{src}\nstop")).unwrap();
        let mut state = ExecutorState::new(program, Vec::new());
        state.bound_unit = Some(UNIT);
        let connections = DashMap::new();
        let view = WorldView {
            world,
            processor_pos: PROC,
            out: &connections,
        };
        state.run_tick(Some(&view), 1);
    }

    fn acquire(world: &DynamicWorld) {
        release_logic_control(world, UNIT);
        ucontrol(world, "ucontrol flag 0");
        assert!(matches!(
            world.enemies.get(&UNIT).map(|u| u.authority),
            Some(UnitAuthority::Logic { .. })
        ));
    }

    #[test]
    fn p1b1_flying_first_position_change_is_end_n_plus_2() {
        let world = timing_world();
        acquire(&world);
        if let Some(mut order) = world.unit_orders.get_mut(&UNIT) {
            order.logic_control = logic_control::STOP;
        }
        unit_tick(&world); // n_minus_1 setup
        unit_tick(&world); // tick N unit
        ucontrol(&world, "ucontrol move 200 80");
        let (_, _, _, _, control, _, _, _, _) = logic_movement_snapshot(&world, UNIT).unwrap();
        assert_eq!(control, "move");
        unit_tick(&world); // N+1: velocity only
        let (x, y, vel_x, ..) = logic_movement_snapshot(&world, UNIT).unwrap();
        assert!((x - 80.0).abs() < 0.01, "position unchanged on N+1");
        assert!((y - 80.0).abs() < 0.01);
        assert!(vel_x > 0.0, "velocity set on N+1");
        unit_tick(&world); // N+2: position integrates
        let (x2, ..) = logic_movement_snapshot(&world, UNIT).unwrap();
        assert!(x2 > x, "first position delta on N+2");
    }

    #[test]
    fn p1b1_grounded_matches_flying_tick_boundary() {
        let world = timing_world();
        if let Some(mut unit) = world.enemies.get_mut(&UNIT) {
            unit.unit_type = DAGGER.unit_type;
            unit.move_speed = DAGGER.speed;
            unit.elevation = 0.0;
        }
        acquire(&world);
        if let Some(mut order) = world.unit_orders.get_mut(&UNIT) {
            order.logic_control = logic_control::STOP;
        }
        unit_tick(&world);
        unit_tick(&world);
        ucontrol(&world, "ucontrol move 200 80");
        unit_tick(&world);
        let (x, vel_x, ..) = logic_movement_snapshot(&world, UNIT).unwrap();
        assert!((x - 80.0).abs() < 0.01);
        assert!(vel_x > 0.0);
    }

    #[test]
    fn p1b1_stop_to_move_and_move_to_stop_boundaries() {
        let world = timing_world();
        if let Some(mut unit) = world.enemies.get_mut(&UNIT) {
            unit.unit_type = DAGGER.unit_type;
            unit.move_speed = DAGGER.speed;
            unit.elevation = 0.0;
        }
        acquire(&world);
        ucontrol(&world, "ucontrol stop");
        unit_tick(&world);
        ucontrol(&world, "ucontrol move 200 80");
        let (_, _, _, _, control, ..) = logic_movement_snapshot(&world, UNIT).unwrap();
        assert_eq!(control, "move");
        unit_tick(&world);
        ucontrol(&world, "ucontrol stop");
        let (_, _, _, _, control, move_x, move_y, ..) =
            logic_movement_snapshot(&world, UNIT).unwrap();
        assert_eq!(control, "stop");
        assert_eq!(move_x, Some(0.0));
        assert_eq!(move_y, Some(0.0));
    }

    #[test]
    fn p1b1_processor_destroyed_releases_on_next_unit_tick() {
        let world = timing_world();
        if let Some(mut unit) = world.enemies.get_mut(&UNIT) {
            unit.unit_type = DAGGER.unit_type;
            unit.move_speed = DAGGER.speed;
            unit.elevation = 0.0;
        }
        acquire(&world);
        if let Some(mut order) = world.unit_orders.get_mut(&UNIT) {
            order.logic_control = logic_control::STOP;
        }
        unit_tick(&world);
        unit_tick(&world);
        ucontrol(&world, "ucontrol move 200 80");
        world.tiles.remove(&PROC);
        let (_, _, _, _, _, _, _, is_logic, proc_valid) =
            logic_movement_snapshot(&world, UNIT).unwrap();
        assert!(is_logic);
        assert!(!proc_valid);
        unit_tick(&world);
        let (_, _, _, _, control, _, _, is_logic, _) =
            logic_movement_snapshot(&world, UNIT).unwrap();
        assert!(!is_logic);
        assert_eq!(control, "none");
    }
}

/// P0-05: player ↔ unit possession lifecycle. Java sources:
/// PlayerComp.java (unit setter 281-319: lastCommand save 290-292, restore
/// 294-300, controller swap 303-305; team propagation 266-271; update tick
/// 224-225), UnitComp.java (controller 455-458, resetController 465-467),
/// UnitType.java (controller prov 281: CommandAI iff player-controllable
/// and not a non-rtsAi AI team; create defaultCommand 558-560) and
/// InputHandler.java (unitControl gate 770-817: possessionAllowed 775,
/// same-team AI unit 783).
///
/// The verified 158.1 observable: possession REPLACES the controller
/// reference, so the CommandAI OBJECT — commandQueue, targetPos,
/// attackTarget, stances — is destroyed; only its `command` enum survives,
/// saved on the player (PlayerComp.lastCommand, "command the unit had
/// before it was controlled", PlayerComp.java:48-49) and restored through
/// `ai.command(lastCommand)` when the unit's fresh default controller is a
/// CommandAI again. Nothing else round-trips.
#[cfg(test)]
mod possession_tests {
    use super::authority_tests::{authority_world, default_order, session, unit};
    use super::*;
    use crate::network::decoders::apply_command_units_for_team;
    use crate::network::wire::{apply_unit_control, respawn_session_player, unit_control_allowed};
    use crate::network::world::{ControlledUnit, EnemyUnit, PlayerCombatState, UnitOrderTarget};

    const ACTOR: u8 = 1;

    fn combat(session: &crate::network::world::SessionPlayer, team: u8) -> PlayerCombatState {
        PlayerCombatState {
            uuid: session.uuid.clone(),
            player_id: session.id,
            unit_id: session.unit_id,
            x: 0.0,
            y: 0.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team,
        }
    }

    /// A team-1 mono (type 20): player-controllable, default command 4
    /// (mine) — `default_unit_command` mirrors CommandAI.init/UnitType.
    /// create defaults (UnitType.java:558-560, CommandAI.java:99-104).
    fn mono(id: i32) -> EnemyUnit {
        let mut unit = unit(id, ACTOR, UnitAuthority::Command);
        unit.unit_type = 20;
        unit
    }

    fn mining_order(id: i32) -> crate::network::world::UnitOrder {
        let mut order = default_order(id);
        order.command = 4; // mine
        order.stances = 1 << 1;
        set_order_active_target(
            &mut order,
            UnitOrderTarget {
                kind: 0,
                id: -1,
                x: 96.0,
                y: 96.0,
            },
        );
        order.queue = vec![
            UnitOrderTarget {
                kind: 0,
                id: -1,
                x: 128.0,
                y: 128.0,
            },
            UnitOrderTarget {
                kind: 0,
                id: -1,
                x: 160.0,
                y: 160.0,
            },
        ];
        order
    }

    fn authority_of(world: &DynamicWorld, id: i32) -> UnitAuthority {
        world.enemies.get(&id).unwrap().authority
    }

    fn order_of(world: &DynamicWorld, id: i32) -> crate::network::world::UnitOrder {
        world.unit_orders.get(&id).unwrap().clone()
    }

    /// Test 1 — possessing a valid same-team AI unit passes the official
    /// gate, takes Player authority, saves the pre-possession command into
    /// `last_command` (PlayerComp.java:290-292) and destroys the CommandAI
    /// object state (queue, active target, stances — the controller
    /// reference is replaced, UnitComp.java:455-458).
    #[test]
    fn possess_valid_unit_saves_command_and_drops_command_ai_state() {
        let world = authority_world();
        let admin = crate::state::administration::Administration::new();
        let mut player = session(7);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let id = 3_002_001;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));

        assert!(unit_control_allowed(&world, &admin, &player, 2, id));
        assert_eq!(
            apply_unit_control(&world, &mut player, 2, id),
            Some(ControlledUnit::Core)
        );
        assert_eq!(player.controlled_unit, ControlledUnit::Standard(id));
        assert_eq!(
            authority_of(&world, id),
            UnitAuthority::Player {
                player_id: player.id
            }
        );
        // The save (PlayerComp.java:290-292): UnitCommand id only.
        assert_eq!(player.last_command, Some(4));
        // The dropped CommandAI object: no queue, no active target, no
        // stances. The command byte is dead state until the release restore.
        let order = order_of(&world, id);
        assert!(order.queue.is_empty());
        assert_eq!(
            (order.target_kind, order.target_id, order.target_x),
            (0, -1, None)
        );
        assert_eq!(order.stances, 0);
        // Possession never changes the unit's team (InputHandler.java:783
        // admits only same-team units; PlayerComp.java:304 is a no-op).
        assert_eq!(world.enemies.get(&id).unwrap().team, ACTOR);
        // A possessed unit is not logic-controllable (Player controller).
        assert!(!unit_is_logic_controllable(&world, id));
    }

    #[test]
    fn player_last_command_starts_empty_and_is_independent_per_runtime() {
        let world = authority_world();
        let mut first = session(70);
        let mut second = session(71);
        assert_eq!(first.last_command, None);
        assert_eq!(second.last_command, None);
        world.players.insert(first.unit_id, combat(&first, ACTOR));
        world.players.insert(second.unit_id, combat(&second, ACTOR));

        let mine = 3_002_006;
        let repair = 3_002_007;
        world.enemies.insert(mine, mono(mine));
        world.enemies.insert(repair, mono(repair));
        world.unit_orders.insert(mine, mining_order(mine));
        let mut repair_order = default_order(repair);
        repair_order.command = 1;
        world.unit_orders.insert(repair, repair_order);

        apply_unit_control(&world, &mut first, 2, mine).unwrap();
        apply_unit_control(&world, &mut second, 2, repair).unwrap();

        assert_eq!(first.last_command, Some(4));
        assert_eq!(second.last_command, Some(1));
    }

    #[test]
    fn reconnect_resets_player_last_command() {
        let mut connected = session(72);
        connected.last_command = Some(4);

        // A reconnect creates a new Player runtime. `lastCommand` is
        // `@NoSync` in Java and is not sourced from PlayerCombatState.
        let reconnected = session(72);
        assert_eq!(reconnected.last_command, None);
    }

    /// Test 2 — switching possession A→B un-controls A
    /// (`this.unit.resetController()`, PlayerComp.java:296) and restores
    /// its saved command (PlayerComp.java:298-300).
    #[test]
    fn switching_units_resets_previous_unit_and_restores_its_command() {
        let world = authority_world();
        let mut player = session(8);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let a = 3_002_010;
        let b = 3_002_011;
        world.enemies.insert(a, mono(a));
        world.enemies.insert(b, mono(b));
        world.unit_orders.insert(a, mining_order(a));
        world.unit_orders.insert(b, mining_order(b));

        apply_unit_control(&world, &mut player, 2, a).unwrap();
        apply_unit_control(&world, &mut player, 2, b).unwrap();
        assert_eq!(player.controlled_unit, ControlledUnit::Standard(b));
        assert_eq!(
            authority_of(&world, b),
            UnitAuthority::Player {
                player_id: player.id
            }
        );
        // A: back on its team default controller with the saved command...
        assert_eq!(authority_of(&world, a), UnitAuthority::Command);
        assert_eq!(order_of(&world, a).command, 4);
        // ...and none of the destroyed queue/target state came back.
        let order = order_of(&world, a);
        assert!(order.queue.is_empty());
        assert_eq!(order.target_x, None);
        assert_eq!(order.stances, 0);
    }

    #[test]
    fn same_type_switch_saves_incoming_before_restoring_old() {
        let world = authority_world();
        let mut player = session(80);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let a = 3_002_012;
        let b = 3_002_013;
        for id in [a, b] {
            let mut poly = mono(id);
            poly.unit_type = 21;
            world.enemies.insert(id, poly);
        }
        world.unit_orders.insert(a, default_order(a)); // move
        let mut rebuild = default_order(b);
        rebuild.command = 2;
        world.unit_orders.insert(b, rebuild);

        apply_unit_control(&world, &mut player, 2, a).unwrap();
        apply_unit_control(&world, &mut player, 2, b).unwrap();
        assert_eq!(player.last_command, Some(2));
        assert_eq!(authority_of(&world, a), UnitAuthority::Command);
        assert_eq!(order_of(&world, a).command, 2);

        apply_unit_control(&world, &mut player, 2, a).unwrap();
        assert_eq!(player.last_command, Some(2));
        assert_eq!(authority_of(&world, b), UnitAuthority::Command);
        assert_eq!(order_of(&world, b).command, 2);
        assert_eq!(
            authority_of(&world, a),
            UnitAuthority::Player {
                player_id: player.id
            }
        );
    }

    /// Test 3 — the restore applies ONLY when the released unit's default
    /// controller is a CommandAI (PlayerComp.java:298:
    /// `this.unit.controller() instanceof CommandAI`). A wave-team unit
    /// (DefaultAi default, UnitType.java:281) keeps its command byte as-is.
    #[test]
    fn last_command_restored_only_when_default_controller_is_command_ai() {
        let world = authority_world();
        let mut player = session(21);
        // CommandAI default (team 1 survival): restore writes the byte.
        let id = 3_002_020;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));
        switch_player_unit(&world, &mut player, Some(id));
        // While possessed the order byte may drift (dead state); the
        // restore must still write the SAVED value.
        world.unit_orders.get_mut(&id).unwrap().command = 0;
        switch_player_unit(&world, &mut player, None);
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
        assert_eq!(order_of(&world, id).command, 4);

        // DefaultAi default (team 2 wave): no restore, byte untouched, and
        // a possessed unit that never had an order keeps having none.
        let wave = 3_002_021;
        let mut wave_unit = unit(wave, 2, UnitAuthority::DefaultAi);
        wave_unit.unit_type = 20;
        world.enemies.insert(wave, wave_unit);
        world.players.insert(player.unit_id, combat(&player, 2));
        switch_player_unit(&world, &mut player, Some(wave));
        assert_eq!(player.last_command, Some(4));
        switch_player_unit(&world, &mut player, None);
        assert_eq!(authority_of(&world, wave), UnitAuthority::DefaultAi);
        assert!(world.unit_orders.get(&wave).is_none());
    }

    /// Test 3b — a possessed unit without an order entry (console/wave
    /// spawn of a commandable type) still gets the spawn-default command
    /// saved and materializes an order on release: Java's CommandAI exists
    /// from birth with the init default (CommandAI.java:99-104).
    #[test]
    fn possession_without_order_saves_spawn_default_and_materializes_on_release() {
        let world = authority_world();
        let mut player = session(22);
        let id = 3_002_022;
        world.enemies.insert(id, mono(id));
        switch_player_unit(&world, &mut player, Some(id));
        assert_eq!(player.last_command, Some(4));
        switch_player_unit(&world, &mut player, None);
        let order = order_of(&world, id);
        assert_eq!(order.command, 4);
        assert!(order.queue.is_empty());
        assert_eq!(order.target_x, None);
    }

    /// Test 4 — `possessionAllowed=false` rejects the takeover at the
    /// official gate (InputHandler.java:775) and nothing changes.
    #[test]
    fn possession_allowed_false_rejects_takeover() {
        let world = authority_world();
        let admin = crate::state::administration::Administration::new();
        let player = session(9);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let id = 3_002_030;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));

        world.wave_rules.write().possession_allowed = false;
        assert!(!unit_control_allowed(&world, &admin, &player, 2, id));
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
        // The packet path applies only after the gate passes
        // (session.rs UNIT_CONTROL: `if valid { apply_unit_control }`), so
        // a rejected request leaves every observable untouched.
        assert_eq!(player.controlled_unit, ControlledUnit::Core);
        assert_eq!(player.last_command, None);
        assert_eq!(order_of(&world, id).queue.len(), 2);
        // ...and the gate reopens when the rule does.
        world.wave_rules.write().possession_allowed = true;
        assert!(unit_control_allowed(&world, &admin, &player, 2, id));
    }

    /// Test 5 — a disconnecting player releases the possessed unit with the
    /// command restore — the exact sequence of the TCP teardown
    /// (`switch_player_unit(..., None)` after session removal).
    #[test]
    fn disconnect_releases_possession_and_restores_command() {
        let world = authority_world();
        let mut player = session(10);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let id = 3_002_040;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));
        apply_unit_control(&world, &mut player, 2, id).unwrap();
        world.player_sessions.insert(player.unit_id, player.clone());

        // TCP teardown block (listener.rs disconnect path).
        let (_, mut session) = world.player_sessions.remove(&player.unit_id).unwrap();
        switch_player_unit(&world, &mut session, None);
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
        assert_eq!(order_of(&world, id).command, 4);
        assert!(order_of(&world, id).queue.is_empty());
        assert_eq!(unit_possessed_by(&world, id), None);
    }

    /// Test 5b — respawning at the core (BuildingControlSelect / death
    /// respawn) releases the possessed unit the same way.
    #[test]
    fn respawn_at_core_releases_possession() {
        let world = authority_world();
        let mut player = session(11);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let id = 3_002_041;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));
        apply_unit_control(&world, &mut player, 2, id).unwrap();
        assert_eq!(player.last_command, Some(4));

        assert!(respawn_session_player(&mut player, &world).is_some());
        assert_eq!(player.controlled_unit, ControlledUnit::Core);
        // Respawn keeps the same Player runtime, so the slot is retained.
        assert_eq!(player.last_command, Some(4));
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
        assert_eq!(order_of(&world, id).command, 4);
        assert_eq!(unit_possessed_by(&world, id), None);
    }

    /// Test 6 — the possessed unit dying (kill_enemy path) removes the
    /// unit, its order AND the possession in one stroke; the session falls
    /// back to its core avatar while its runtime-owned last_command remains.
    #[test]
    fn possessed_unit_death_releases_session_and_clears_state() {
        let world = authority_world();
        let mut player = session(12);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let id = 3_002_050;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));
        apply_unit_control(&world, &mut player, 2, id).unwrap();
        world.player_sessions.insert(player.unit_id, player.clone());
        assert_eq!(player.last_command, Some(4));

        // kill_enemy tail (combat.rs): entry removal + detach_unit_control.
        world.enemies.remove(&id);
        detach_unit_control(&world, id);
        assert!(!world.enemies.contains_key(&id));
        assert!(!world.unit_orders.contains_key(&id));
        let session = world.player_sessions.get(&player.unit_id).unwrap();
        assert_eq!(session.controlled_unit, ControlledUnit::Core);
        drop(session);
        assert_eq!(unit_possessed_by(&world, id), None);
    }

    /// Test 7 — a team change mid-possession propagates to the possessed
    /// unit (`PlayerComp.team` -> `unit.team(team)`, PlayerComp.java:266-271,
    /// re-asserted every tick at 224-225) but possession itself survives;
    /// on release the unit keeps the new team and its default controller is
    /// no longer a CommandAI, so no command restore happens.
    #[test]
    fn player_team_change_mid_possession_follows_player_and_survives() {
        let world = authority_world();
        let mut player = session(13);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let id = 3_002_060;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));
        apply_unit_control(&world, &mut player, 2, id).unwrap();

        // The switchTeam admin path (session.rs): combat team update plus
        // the PlayerComp.team propagation to the possessed unit.
        world.players.get_mut(&player.unit_id).unwrap().team = 5;
        if let ControlledUnit::Standard(controlled_id) = player.controlled_unit {
            world.enemies.get_mut(&controlled_id).unwrap().team = 5;
        }
        // Possession survives the team change.
        assert_eq!(
            authority_of(&world, id),
            UnitAuthority::Player {
                player_id: player.id
            }
        );
        assert_eq!(unit_possessed_by(&world, id), Some(player.id));
        // Release: team-5 default controller is not CommandAI
        // (UnitType.java:281 — isAI team without rtsAi), so the unit keeps
        // the new team and no restore runs.
        switch_player_unit(&world, &mut player, None);
        assert_eq!(world.enemies.get(&id).unwrap().team, 5);
        assert_eq!(authority_of(&world, id), UnitAuthority::DefaultAi);
    }

    /// Test 8 — A→B→A fast double switch leaves no stale Player authority.
    #[test]
    fn fast_double_switch_back_leaves_no_stale_authority() {
        let world = authority_world();
        let mut player = session(14);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let a = 3_002_070;
        let b = 3_002_071;
        world.enemies.insert(a, mono(a));
        world.enemies.insert(b, mono(b));
        world.unit_orders.insert(a, mining_order(a));
        let mut repair = default_order(b);
        repair.command = 1; // repair — distinct from A's mine
        world.unit_orders.insert(b, repair);

        apply_unit_control(&world, &mut player, 2, a).unwrap();
        apply_unit_control(&world, &mut player, 2, b).unwrap();
        // B carries its own pre-possession command.
        assert_eq!(player.last_command, Some(1));
        // A rejects B's unsupported repair command and keeps its fresh mono
        // default (mine).
        assert_eq!(order_of(&world, a).command, 4);
        assert_eq!(authority_of(&world, a), UnitAuthority::Command);

        // Back to A: incoming A saves mine first, so B is restored to mine.
        apply_unit_control(&world, &mut player, 2, a).unwrap();
        assert_eq!(player.controlled_unit, ControlledUnit::Standard(a));
        assert_eq!(
            authority_of(&world, a),
            UnitAuthority::Player {
                player_id: player.id
            }
        );
        assert_eq!(authority_of(&world, b), UnitAuthority::Command);
        assert_eq!(order_of(&world, b).command, 4);
        // The re-possess saved A's fresh/default command before resetting B.
        assert_eq!(player.last_command, Some(4));
        assert_eq!(unit_possessed_by(&world, b), None);
    }

    /// Test 9 — possessing a unit with an ACTIVE RTS queue: the queue does
    /// NOT survive the round-trip. Java destroys the CommandAI object on
    /// possess (UnitComp.java:455-458 keeps no old-controller reference)
    /// and `resetController` creates a fresh one (465-467); only the
    /// command enum returns via lastCommand. Command packets during
    /// possession are no-ops (`controller() instanceof CommandAI`,
    /// InputHandler.java:333).
    #[test]
    fn possession_with_active_rts_queue_destroys_queue_restores_command_only() {
        let world = authority_world();
        let mut player = session(15);
        world.players.insert(player.unit_id, combat(&player, ACTOR));
        let id = 3_002_080;
        world.enemies.insert(id, mono(id));
        world.unit_orders.insert(id, mining_order(id));
        assert!(unit_has_active_rts_command(&world, id));

        apply_unit_control(&world, &mut player, 2, id).unwrap();
        // During possession: the whole CommandAI object state is gone.
        assert!(!unit_has_active_rts_command(&world, id));
        assert!(order_of(&world, id).queue.is_empty());

        // Commands cannot touch a possessed unit (InputHandler.java:333).
        let request = crate::network::decoders::CommandUnitsRequest {
            unit_ids: vec![id],
            build_target: -1,
            unit_target_type: 0,
            unit_target_id: -1,
            pos_x: 400.0,
            pos_y: 500.0,
            queue_command: true,
            final_batch: true,
        };
        assert!(!apply_command_units_for_team(&world, ACTOR, &request));
        assert!(!unit_has_active_rts_command(&world, id));
        assert!(order_of(&world, id).queue.is_empty());

        // After release: the command enum is back, the queue is not.
        switch_player_unit(&world, &mut player, None);
        assert_eq!(order_of(&world, id).command, 4);
        assert!(!unit_has_active_rts_command(&world, id));
        assert!(order_of(&world, id).queue.is_empty());
        assert_eq!(order_of(&world, id).stances, 0);
        assert_eq!(authority_of(&world, id), UnitAuthority::Command);
        // The fresh CommandAI has no command active: logic control is
        // admissible again (CommandAI.isLogicControllable = !hasCommand).
        assert!(unit_is_logic_controllable(&world, id));
    }
}

/// P2-B1: vanilla unit AI/pathfinding breadth — inventory validation and
/// differential navigation/targeting tests on small deterministic worlds.
#[cfg(test)]
mod p2b1_unit_ai_breadth_tests {
    use super::*;
    use crate::game::content::{unit_inventory, UnitInventoryEntry};
    use crate::network::economy::payload::payload_capacity;
    use crate::network::units::mining::{enemy_navigation_target, unit_avoidance_requests};
    use crate::network::world::{DynamicWorld, EnemyUnit, UnitAuthority, UnitOrder};
    use crate::state::game_state::{GameMode, GameState};
    use dashmap::DashMap;
    use parking_lot::RwLock;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
    use std::sync::Arc;

    fn nav_world(width: i32, height: i32, core_x: i32, core_y: i32) -> DynamicWorld {
        let state = GameState::new();
        state.start_hosting("p2b1-nav".into(), GameMode::Survival);
        let cells = (width * height).max(0) as usize;
        DynamicWorld {
            game_state: state,
            width,
            height,
            sharded_unit_cap: 8,
            core_position: (core_x << 16) | core_y,
            core_max_health: 6_000.0,
            cores: DashMap::new(),
            team_core_lists: DashMap::new(),
            base_blocks: vec![0; cells],
            base_centers: vec![false; cells],
            tile_data: vec![0; cells],
            base_building_templates: Vec::new(),
            base_buildings: DashMap::new(),
            floors: vec![0; cells],
            overlays: vec![0; cells],
            enemy_spawns: Vec::new(),
            enemies: DashMap::new(),
            players: DashMap::new(),
            player_sessions: DashMap::new(),
            player_profiles: DashMap::new(),
            building_commands: DashMap::new(),
            unit_orders: DashMap::new(),
            next_player_unit_id: AtomicI32::new(2_500_000),
            next_enemy_id: AtomicI32::new(1),
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
            save_path: std::env::temp_dir().join("p2b1-nav-test.json"),
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
            team_build_plans: RwLock::new(crate::engine::typeio::TeamBlocks::default()),
            wave_rules: RwLock::new(WaveRules::default()),
            votekick_target: RwLock::new(None),
            votekick_votes: AtomicI32::new(0),
            votekick_voters: DashMap::new(),
            votekick_cooldowns: DashMap::new(),
            puddles: crate::network::buildings::puddles::PuddleSystem::new(),
        }
    }

    fn wave_enemy(id: i32, unit_type: i16, tile_x: i32, tile_y: i32) -> EnemyUnit {
        let spec = enemy_spec(unit_type).expect("test uses supported units");
        EnemyUnit {
            id,
            unit_type,
            entity_class: spec.entity_class,
            team: 2,
            x: (tile_x as f32 + 0.5) * 8.0,
            y: (tile_y as f32 + 0.5) * 8.0,
            rotation: 0.0,
            health: spec.health,
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

    #[test]
    fn p2b1_unit_inventory_tsv_is_well_formed() {
        const SPAWNABLE: &[&str] = &["YES", "CHILD", "NO", "INTERNAL"];
        const CONTROLLERS: &[&str] = &[
            "GroundAI",
            "CommandAI",
            "MissileAI",
            "NeoplasmAI",
            "InternalAI",
        ];
        const MOVEMENT: &[&str] = &["ground", "flying", "legs", "naval", "hover", "missile"];
        const TARGETS: &[&str] = &[
            "core_buildings",
            "core_direct",
            "nearest_building",
            "command",
            "none",
        ];
        const WEAPONS: &[&str] = &["FULL", "PARTIAL", "REPAIR", "PD", "NONE"];
        const PAYLOAD: &[&str] = &["YES", "NO"];
        const MINE_BUILD: &[&str] = &["mine", "build", "repair", "none"];
        const PATH: &[&str] = &[
            "flow_field",
            "flow_field_legs",
            "direct",
            "command_astar",
            "none",
        ];
        const STATUS: &[&str] = &["FULL", "PARTIAL", "REJECTED", "INTERNAL"];
        let mut rows = 0usize;
        for line in include_str!("../../game/unit_inventory.tsv").lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let cols: Vec<_> = line.split('\t').collect();
            assert_eq!(cols.len(), 11, "inventory row must have 11 columns: {line}");
            assert!(SPAWNABLE.contains(&cols[2]), "bad spawnable: {}", cols[2]);
            assert!(
                CONTROLLERS.contains(&cols[3]),
                "bad controller: {}",
                cols[3]
            );
            assert!(MOVEMENT.contains(&cols[4]), "bad movement: {}", cols[4]);
            assert!(TARGETS.contains(&cols[5]), "bad target: {}", cols[5]);
            assert!(WEAPONS.contains(&cols[6]), "bad weapons: {}", cols[6]);
            assert!(PAYLOAD.contains(&cols[7]), "bad payload: {}", cols[7]);
            assert!(MINE_BUILD.contains(&cols[8]), "bad mine_build: {}", cols[8]);
            assert!(PATH.contains(&cols[9]), "bad path_cost: {}", cols[9]);
            assert!(STATUS.contains(&cols[10]), "bad rust_status: {}", cols[10]);
            rows += 1;
        }
        assert_eq!(rows, crate::game::unit_types::UNIT_COUNT);
    }

    #[test]
    fn p2b1_inventory_covers_all_registry_ids() {
        for (id, name) in crate::game::unit_types::UNIT_NAMES {
            let entry = unit_inventory(*id).expect("missing inventory row for {name}");
            assert_eq!(entry.id, *id);
            assert_eq!(entry.name, *name);
        }
    }

    #[test]
    fn p2b1_inventory_status_matches_enemy_spec() {
        for (id, name) in crate::game::unit_types::UNIT_NAMES {
            let entry = unit_inventory(*id).unwrap();
            let has_spec = enemy_spec(*id).is_some();
            match entry.rust_status {
                "FULL" | "PARTIAL" => assert!(
                    has_spec,
                    "{name} ({id}) marked {} but has no enemy_spec",
                    entry.rust_status
                ),
                "REJECTED" => assert!(
                    !has_spec,
                    "{name} ({id}) marked REJECTED but has enemy_spec"
                ),
                "INTERNAL" => assert_eq!(*id, 68, "only build-tower is INTERNAL"),
                other => panic!("unknown rust_status '{other}' for {name}"),
            }
            assert_eq!(
                entry.simulation_supported(),
                matches!(entry.rust_status, "FULL" | "PARTIAL")
            );
            assert_eq!(entry.strict_rejected(), entry.rust_status == "REJECTED");
        }
    }

    #[test]
    fn p2b1_strict_spawn_rejects_unsupported_vanilla_units() {
        for (id, name) in crate::game::unit_types::UNIT_NAMES {
            let entry = unit_inventory(*id).unwrap();
            if entry.rust_status != "REJECTED" {
                continue;
            }
            let rules = format!(r#"{{"spawns":[{{"type":"{name}"}}]}}"#);
            let (parsed, diagnostics) = parse_wave_rules_report(&rules);
            assert!(
                parsed.spawn_groups.is_empty(),
                "{name} must not enter spawn table"
            );
            assert_eq!(diagnostics.len(), 1, "{name} must produce one diagnostic");
            assert!(
                diagnostics[0].contains(name),
                "diagnostic must name the unit: {}",
                diagnostics[0]
            );
            assert!(
                diagnostics[0].contains(&id.to_string()),
                "diagnostic must include id: {}",
                diagnostics[0]
            );
        }
    }

    #[test]
    fn p2b1_flying_navigation_targets_core_directly() {
        let world = nav_world(5, 5, 0, 0);
        let core_x = 4.0;
        let core_y = 4.0;
        let enemy = wave_enemy(1, FLARE.unit_type, 4, 4);
        let avoidance = unit_avoidance_requests(&world);
        let nav = enemy_navigation_target(&world, &enemy, core_x, core_y, &avoidance);
        assert!(nav.building.is_none());
        assert!(
            (nav.movement.0 - core_x).abs() < 0.01 && (nav.movement.1 - core_y).abs() < 0.01,
            "flying units fly direct to core, got {:?}",
            nav.movement
        );
    }

    #[test]
    fn p2b1_grounded_flow_field_steps_toward_core() {
        let world = nav_world(5, 5, 0, 2);
        let core_x = 4.0;
        let core_y = 20.0;
        let enemy = wave_enemy(1, DAGGER.unit_type, 4, 2);
        let avoidance = unit_avoidance_requests(&world);
        let nav = enemy_navigation_target(&world, &enemy, core_x, core_y, &avoidance);
        assert!(
            nav.movement.0 < enemy.x,
            "ground unit east of core must step west toward lower cost, got {:?}",
            nav.movement
        );
    }

    #[test]
    fn p2b1_legs_navigation_differs_from_ground_on_solid_base_wall() {
        // 3x1 strip: core (0,0) — solid base wall (1,0) — enemy (2,0).
        // Ground cannot traverse the wall; legs treat it as costly but passable.
        let mut world = nav_world(3, 1, 0, 0);
        world.base_blocks[1] = 4;
        let core_x = 4.0;
        let core_y = 4.0;
        let avoidance = unit_avoidance_requests(&world);
        let ground = wave_enemy(1, DAGGER.unit_type, 2, 0);
        let legs = wave_enemy(2, ATRAX.unit_type, 2, 0);
        let ground_nav = enemy_navigation_target(&world, &ground, core_x, core_y, &avoidance);
        let legs_nav = enemy_navigation_target(&world, &legs, core_x, core_y, &avoidance);
        assert!(
            (ground_nav.movement.0 - ground.x).abs() < 0.01,
            "ground unit blocked by solid base wall, got {:?}",
            ground_nav.movement
        );
        assert!(
            legs_nav.movement.0 < legs.x,
            "leg unit must step toward the wall/core through costly terrain, got {:?}",
            legs_nav.movement
        );
    }

    #[test]
    fn p2b1_naval_unit_uses_ground_flow_field() {
        let world = nav_world(5, 5, 0, 2);
        let core_x = 4.0;
        let core_y = 20.0;
        let enemy = wave_enemy(1, RISSO.unit_type, 4, 2);
        let avoidance = unit_avoidance_requests(&world);
        let nav = enemy_navigation_target(&world, &enemy, core_x, core_y, &avoidance);
        assert!(
            nav.movement.0 < enemy.x,
            "naval wave AI uses the same flow-field step as ground, got {:?}",
            nav.movement
        );
    }

    #[test]
    fn p2b1_horizon_targets_nearest_player_building() {
        let world = nav_world(7, 5, 0, 2);
        let core_x = 4.0;
        let core_y = 20.0;
        let wall_pos = (3 << 16) | 2;
        world.tiles.insert(
            wall_pos,
            crate::network::world::DynamicTile {
                position: wall_pos,
                block: 216,
                rotation: 0,
                team: 1,
                occupied: vec![wall_pos],
                health: 320.0,
                ..Default::default()
            },
        );
        world
            .navigation_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let enemy = wave_enemy(1, HORIZON.unit_type, 6, 2);
        let avoidance = unit_avoidance_requests(&world);
        let nav = enemy_navigation_target(&world, &enemy, core_x, core_y, &avoidance);
        assert!(
            nav.building.is_some(),
            "horizon must prefer a nearby player building over the core"
        );
    }

    #[test]
    fn p2b1_command_ai_movement_differs_from_wave_ai() {
        use crate::network::simulation::simulate_allied_units;
        use crate::network::units::unit_orders::apply_ordered_unit_movement;

        let world = nav_world(40, 40, 20, 20);
        let connections = DashMap::new();
        // Wave-team dagger ignores queued RTS orders.
        let wave_unit = wave_enemy(1, DAGGER.unit_type, 5, 5);
        world.enemies.insert(1, wave_unit.clone());
        world.unit_orders.insert(
            1,
            UnitOrder {
                unit_id: 1,
                command: 0,
                stances: 0,
                payload_cooldown: 0.0,
                target_kind: 0,
                target_id: -1,
                target_x: Some(200.0),
                target_y: Some(200.0),
                logic_control: 0,
                queue: Vec::new(),
            },
        );
        simulate_allied_units(&world, &connections, 1.0);
        let wave_x = world.enemies.get(&1).unwrap().x;
        assert!(
            (wave_x - wave_unit.x).abs() < 0.001,
            "wave-team unit must ignore CommandAI queue"
        );

        // Player-team dagger under CommandAI moves toward the order.
        let mut cmd_unit = wave_enemy(2, DAGGER.unit_type, 0, 0);
        cmd_unit.team = 1;
        cmd_unit.authority = UnitAuthority::Command;
        cmd_unit.x = 0.0;
        cmd_unit.y = 0.0;
        cmd_unit.move_speed = 1.0;
        world.enemies.insert(2, cmd_unit.clone());
        world.unit_orders.insert(
            2,
            UnitOrder {
                unit_id: 2,
                command: 0,
                stances: 0,
                payload_cooldown: 0.0,
                target_kind: 0,
                target_id: -1,
                target_x: Some(200.0),
                target_y: Some(0.0),
                logic_control: 0,
                queue: Vec::new(),
            },
        );
        let snapshot = world.enemies.get(&2).unwrap().clone();
        assert!(apply_ordered_unit_movement(&world, &snapshot, 1.0));
        assert!(
            world.enemies.get(&2).unwrap().x > cmd_unit.x,
            "CommandAI unit must move toward its order target"
        );
    }

    #[test]
    fn p2b1_mono_default_command_is_mine() {
        assert_eq!(
            crate::network::economy::default_unit_command(MONO.unit_type),
            4
        );
        let entry = unit_inventory(MONO.unit_type).unwrap();
        assert_eq!(entry.mine_build, "mine");
        assert_eq!(entry.default_controller, "CommandAI");
    }

    #[test]
    fn p2b1_payload_carrier_has_nonzero_capacity() {
        for (unit_type, expected) in [(22, 256.0), (23, 576.0), (24, 1_936.0)] {
            assert!(
                (payload_capacity(unit_type) - expected).abs() < 0.001,
                "unit {unit_type} payload capacity"
            );
            let entry = unit_inventory(unit_type).unwrap();
            assert_eq!(entry.payload, "YES");
        }
        assert_eq!(payload_capacity(DAGGER.unit_type), 0.0);
    }

    #[test]
    fn ordered_unit_path_returns_pathfind_result_fields() {
        let world = authority_tests::authority_world();
        let unit = EnemyUnit {
            id: 1,
            unit_type: 15, // flare flying
            elevation: 0.09,
            x: 80.0,
            y: 80.0,
            team: 1,
            health: 100.0,
            ..Default::default()
        };
        let result = ordered_unit_path(&world, &unit, 200.0, 200.0);
        assert!(result.should_move);
        assert!(!result.unreachable);
        assert!(result.next.is_none());
    }
}
