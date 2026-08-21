//! Combat simulation: turrets, projectiles, pierce/splash/rail damage,
//! EMP effects, player combat and enemy kills.
//!
//! **P2-A1 bullet inventory:** every server-authoritative bullet id and
//! whether direct/splash/pierce/status/frag/continuous/lightning/heal/point
//! defense/spawnUnit/sticky is **IMPLEMENTED** or a **DOCUMENTED DEVIATION**
//! lives in `src/game/bullet_inventory.tsv` with prose in
//! `tools/BULLET_INVENTORY.md`.

use crate::network::buildings::construction::{dynamic_at, effective_building_team};
use crate::network::buildings::snapshot::dynamic_tile_health;
use crate::network::codec::Writes;
use crate::network::economy::spec::{
    inventory_remove, is_supported_item_turret, liquid_turret_weapon, power_turret_weapon,
    turret_ammo, turret_can_target, turret_shots, TurretAmmo,
};
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::simulation::{simulate_enemy_point_defense, simulate_enemy_statuses};
use crate::network::units::controller::controlling_session_for_building;
use crate::network::units::mining::heal_building_for_team;
use crate::network::units::*;
use crate::network::wire::auth::player_team;
use crate::network::wire::bootstrap::emit_game_over_packet_with_winner;
use crate::network::wire::encode::{
    encode_build_destroyed_frame, encode_build_health_update_frame, encode_enemy_entity_snapshots,
    encode_initial_entity_snapshot, frame_generated_packet,
};
use crate::network::world::*;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::io::Error;
use std::sync::atomic::Ordering;
use tracing::{debug, info, warn};

/// Default ammo for a turret block: the first item entry in the official
/// ammo table (used by prebuilt map turrets that have no simulated
/// conveyor-fed stock).
pub(crate) mod unit_combat;
pub(crate) use unit_combat::{
    boost_properties, collect_allied_weapon_fire, collect_manual_weapon_fire,
    collision_position_passable, damaged_allied_building_target, drain_weapon_timer,
    effective_unit_build_speed, effective_unit_damage_multiplier, effective_unit_reload_delta,
    effective_unit_speed, generic_unit_weapon_volley, invalidate_navigation_for_block,
    scaled_projectile_volley, set_unit_weapon_timer, spawn_allied_weapon_fire,
    spawn_weapon_fire_for_team, unit_can_shoot, unit_collision_layer, unit_hit_size,
    unit_weapon_timer, AlliedWeaponFire,
};
pub(crate) mod enemy;
pub(crate) use enemy::{
    apply_enemy_support_abilities, base_building_at, base_building_tombstone,
    build_navigation_field, cancel_transient_world_actions, damage_building,
    dynamic_building_tombstone, enemy_circle_radius, enemy_max_health, hostile_unit_count,
    move_enemy_in_attack_orbit, navigation_field, navigation_index, nearest_player_building,
    reregister_team_core, restore_base_buildings, spawn_wave, tile_is_leg_solid,
    SupportRepairTarget,
};
mod turrets;
pub(crate) use turrets::{
    controlled_building_weapon_input, damaged_allied_building_on_ray, default_turret_ammo,
    projectile_direct_heal_percent, projectile_splash_heal_percent, resolve_manual_aim,
    simulate_base_menders, simulate_base_turrets, simulate_turrets, ControlledWeaponInput,
    ManualAim,
};
mod projectiles;
pub(crate) use projectiles::{
    encode_create_bullet_payload, encode_projectile_replay_payload, enemy_projectile_volley,
    naval_weapon_volleys, projectile_armor_multiplier, projectile_maximum_travel,
    retusa_mine_shots_between, sap_strength, simulate_projectiles, spawn_allied_unit_projectile,
    spawn_continuous_projectile, spawn_continuous_projectile_for_team, spawn_enemy_horizon_bomb,
    spawn_enemy_projectile, spawn_navanax_lasers, spawn_projectile, spawn_projectile_for_team,
    spawn_unit_projectile_for_team, unit_weapon_beam_length, volley_shot_delay,
    EnemyProjectileVolley, AEGIRES_PD, ANTUMBRA_CANNON, ANTUMBRA_MISSILE, ARKYID_ARTILLERY,
    ARKYID_SAP, ATRAX_SLAG, BRYDE_ARTILLERY, BRYDE_MISSILES, CORVUS_LASER, ECLIPSE_FLAK,
    ECLIPSE_LASER, FLARE_BOLT, MEGA_HEAL_A, MEGA_HEAL_B, MINKE_ARTILLERY, MINKE_GUN, OMURA_RAIL,
    POLY_MISSILE, QUAD_BOMB, RETUSA_BOLT, RETUSA_MINE, RISSO_GUN, RISSO_MISSILE, SCEPTER_BOLT,
    SCEPTER_MOUNT, SEI_CANNON, SEI_LAUNCHER, SPIROCT_SAP, SPIROCT_SAP_MOUNT, TOXOPID_CANNON,
    TOXOPID_SHRAPNEL, VELA_BEAM,
};
mod damage;
pub(crate) use damage::{
    apply_allied_pierce_damage, apply_allied_pierce_damage_for_team, apply_allied_splash_damage,
    apply_allied_splash_damage_for_team, apply_emp_bullet_effects, apply_enemy_direct_damage,
    apply_enemy_pierce_building_damage, apply_enemy_pierce_player_damage, apply_enemy_rail_damage,
    apply_enemy_shared_pierce_damage, apply_enemy_splash_damage, apply_incoming_unit_damage,
    apply_quad_bomb_heal, apply_quad_bomb_heal_for_team, apply_splash_building_heal_for_team,
    apply_unit_armor, building_exists, damage_player, damage_team_core, enemy_armor,
    heal_team_core, kill_enemy, nearest_player_building_in_range, point_hits_segment,
    point_segment_distance, pvp_elimination_winner, simulate_player_combat, spawn_cyerce_fragments,
    spawn_reign_fragments, spawn_toxopid_fragments, unit_effective_armor, unit_immune_to_status,
    AlliedPierceTarget, EnemyPierceTarget, EnemyRailTarget, EMP_HEAL_PERCENT, EMP_POWER_DAMAGE_SCL,
    EMP_TIME_DURATION, EMP_TIME_INCREASE,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::units::StatusContainer;

    fn test_world() -> (DynamicWorld, DashMap<i32, PendingConnection>) {
        let state = crate::state::game_state::GameState::new();
        state.start_hosting(
            "combat-test".into(),
            crate::state::game_state::GameMode::Survival,
        );
        let world = DynamicWorld {
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
            save_path: std::env::temp_dir().join("combat-functional-test.json"),
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
        };
        (world, DashMap::new())
    }

    fn enemy_unit(id: i32) -> EnemyUnit {
        EnemyUnit {
            id,
            unit_type: 0,
            entity_class: 0,
            team: 2,
            x: 50.0,
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
            status_agg: None,
        }
    }

    fn default_team_rule() -> TeamRule {
        TeamRule {
            protect_cores: true,
            check_placement: true,
            cheat: false,
            fill_items: false,
            infinite_resources: false,
            prebuild_ai: false,
            build_ai: false,
            build_ai_tier: 1.0,
            rts_ai: false,
            rts_min_squad: 4,
            rts_max_squad: 50,
            rts_min_weight: 1.2,
            unit_factory_activation_delay: 0.0,
            unit_build_speed_multiplier: 1.0,
            unit_damage_multiplier: 1.0,
            unit_mine_speed_multiplier: 1.0,
            unit_cost_multiplier: 1.0,
            unit_health_multiplier: 1.0,
            block_health_multiplier: 1.0,
            block_damage_multiplier: 1.0,
            build_speed_multiplier: 1.0,
            extra_core_build_radius: 0.0,
        }
    }

    #[test]
    fn immunity_table_matches_desktop_jar_158() {
        // A5: unit_immune_to_status verified against the JAR bytecode
        // (UnitTypes$2/$9/$12/$41/$42/$43 constructors).
        assert!(unit_immune_to_status(1, 1)); // mace: burning
        assert!(!unit_immune_to_status(1, 8)); // mace: no melting
        assert!(unit_immune_to_status(8, 1)); // vela: burning
        assert!(!unit_immune_to_status(8, 8)); // vela: no melting
        assert!(unit_immune_to_status(11, 1)); // atrax: burning
        assert!(unit_immune_to_status(11, 8)); // atrax: melting
        assert!(unit_immune_to_status(40, 1)); // precept: burning
        assert!(unit_immune_to_status(40, 8)); // precept: melting
        assert!(unit_immune_to_status(41, 1)); // vanquish: burning
        assert!(unit_immune_to_status(41, 8)); // vanquish: melting
        assert!(unit_immune_to_status(42, 1)); // conquer: burning
        assert!(unit_immune_to_status(42, 8)); // conquer: melting
                                               // Round-73 A5 corrections: navanax and naval units are NOT immune.
        assert!(!unit_immune_to_status(34, 1), "navanax can burn");
        assert!(!unit_immune_to_status(34, 8), "navanax can melt");
        for naval in 25..=29 {
            assert!(!unit_immune_to_status(naval, 1), "naval {naval} can burn");
            assert!(!unit_immune_to_status(naval, 8), "naval {naval} can melt");
        }
        // No other unit has immunities in 158.1.
        for unit_type in [0, 2, 3, 5, 10, 30, 50, 60, 68] {
            assert!(!unit_immune_to_status(unit_type, 1));
            assert!(!unit_immune_to_status(unit_type, 8));
        }
    }

    #[test]
    fn naval_and_navanax_units_receive_burning_from_incendiary_hits() {
        // A5: without the (wrong) hard immunity, an incendiary projectile
        // applies burning to navanax (34) and naval units (25-29).
        let (world, connections) = test_world();
        for (id, unit_type) in [(1, 34), (2, 25)] {
            let mut unit = enemy_unit(id);
            unit.unit_type = unit_type;
            world.enemies.insert(id, unit);
        }
        assert!(apply_allied_pierce_damage(
            &world,
            &connections,
            0.0,
            0.0,
            100.0,
            0.0,
            5.0,
            8,
            false,
            1.0,
            1, // burning
            300.0,
        ));
        for id in [1, 2] {
            let unit = world.enemies.get(&id).unwrap();
            assert!(
                unit.statuses
                    .iter()
                    .any(|entry| entry.effect == 1 && entry.time == 300.0),
                "unit {id} received burning through the collection"
            );
        }
    }

    #[test]
    fn incendiary_projectile_burning_persists_with_stacked_statuses_and_dots() {
        // A6: a unit already carrying overdrive (13) hit by an incendiary
        // projectile keeps BOTH statuses in the StatusEntry collection, the
        // legacy view mirrors the first entry, and the burning DoT applies
        // on the next status tick (it must not be lost to a legacy-field
        // overwrite).
        let (world, connections) = test_world();
        let id = 1;
        world.enemies.insert(id, enemy_unit(id));
        {
            let mut unit = world.enemies.get_mut(&id).unwrap();
            StatusContainer::apply_status(&mut *unit, 13, 300.0); // overdrive
        }
        assert!(apply_allied_pierce_damage(
            &world,
            &connections,
            0.0,
            0.0,
            100.0,
            0.0,
            10.0,
            4,
            false,
            1.0,
            1, // burning
            600.0,
        ));
        let unit = world.enemies.get(&id).unwrap();
        assert_eq!(
            unit.statuses
                .iter()
                .filter(|entry| entry.effect == 1)
                .count(),
            1,
            "burning present exactly once"
        );
        assert!(
            unit.statuses
                .iter()
                .any(|entry| entry.effect == 1 && entry.time == 600.0),
            "burning keeps the applied duration"
        );
        assert!(
            unit.statuses
                .iter()
                .any(|entry| entry.effect == 13 && entry.time == 300.0),
            "overdrive persists alongside burning"
        );
        // Legacy view mirrors the first collection entry.
        assert_eq!(unit.status_effect, unit.statuses[0].effect);
        assert_eq!(unit.status_duration, unit.statuses[0].time);
        drop(unit);
        // The DoT applies while burning is present in the collection.
        let health_before = world.enemies.get(&id).unwrap().health;
        assert!(crate::network::simulation::simulate_enemy_statuses(
            &world,
            &connections,
            60.0
        ));
        let after = world.enemies.get(&id).unwrap();
        assert!(
            after.health < health_before,
            "burning DoT applied: {} -> {}",
            health_before,
            after.health
        );
        assert!(
            after
                .statuses
                .iter()
                .any(|entry| entry.effect == 1 && entry.time > 0.0),
            "burning persists after the tick"
        );
        assert!(
            after
                .statuses
                .iter()
                .any(|entry| entry.effect == 13 && entry.time > 0.0),
            "overdrive persists after the tick"
        );
    }

    #[test]
    fn unit_projectile_damage_scales_with_attacker_team_rule() {
        // A7: unit-fired bullets deal `base * TeamRule.unitDamageMultiplier`
        // (BulletType.damageMultiplier JAR offsets 0-61, Rules.unitDamage
        // offsets 0-16); building-fired bullets keep the base damage because
        // the official blockDamage(team) path is not modeled.
        let (world, connections) = test_world();
        let mut rule = default_team_rule();
        rule.unit_damage_multiplier = 1.5;
        world.wave_rules.write().team_rules.insert(2, rule);
        let target = (10 << 16) | 10;
        let max_health = crate::game::content::block_health(218);
        let building = crate::network::world::DynamicTile {
            position: target,
            block: 218, // a team-1 building
            team: 1,
            health: max_health,
            occupied: vec![target],
            ..Default::default()
        };
        world.tiles.insert(target, building);

        let spawn =
            |world: &DynamicWorld, bullet_id: i16, source_position: Option<i32>, damage: f32| {
                let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
                world.projectiles.insert(
                    projectile_id,
                    Projectile {
                        target_id: 0,
                        shooter_id: 7,
                        team: 2,
                        bullet_id,
                        damage,
                        splash_damage: 0.0,
                        splash_radius: 0.0,
                        status_effect: -1,
                        status_duration: 0.0,
                        pierce_units: 0,
                        pierce_buildings: 0,
                        spawn_reign_frags: false,
                        homing_range: 0.0,
                        enemy_target_position: Some(target),
                        enemy_target_core: false,
                        apply_direct_on_impact: true,
                        armor_multiplier: 1.0,
                        remaining_ticks: 1.0,
                        total_ticks: 1.0,
                        source_x: 0.0,
                        source_y: 0.0,
                        target_x: 80.0,
                        target_y: 80.0,
                        lifetime_scale: 1.0,
                        source_position,
                        damage_interval: None,
                        damage_timer: 0.0,
                    },
                );
            };

        // Unit-fired beam (source_position None): 10 * 1.5 = 15.
        spawn(&world, 61, None, 10.0);
        simulate_projectiles(&world, &connections, 1.0);
        let health_after_unit = world.tiles.get(&target).unwrap().health;
        assert!(
            (health_after_unit - (max_health - 15.0)).abs() < 0.001,
            "unit-fired damage scaled by team rule, got {health_after_unit}"
        );

        // Building-fired bullet (source_position Some): base damage, no rule.
        world.tiles.get_mut(&target).unwrap().health = max_health;
        spawn(&world, 61, Some(target), 10.0);
        simulate_projectiles(&world, &connections, 1.0);
        let health_after_building = world.tiles.get(&target).unwrap().health;
        assert!(
            (health_after_building - (max_health - 10.0)).abs() < 0.001,
            "building-fired damage is NOT scaled by unitDamage, got {health_after_building}"
        );

        // Default rules (multiplier 1.0): unit-fired damage stays base.
        world.tiles.get_mut(&target).unwrap().health = max_health;
        world.wave_rules.write().team_rules.clear();
        spawn(&world, 61, None, 10.0);
        simulate_projectiles(&world, &connections, 1.0);
        let health_default = world.tiles.get(&target).unwrap().health;
        assert!(
            (health_default - (max_health - 10.0)).abs() < 0.001,
            "default team rule leaves unit damage unchanged, got {health_default}"
        );
    }

    #[test]
    fn p201_pierce_cap_stops_after_the_nth_target_and_missing_owner_is_safe() {
        let (world, connections) = test_world();
        for (id, x) in [(1, 10.0), (2, 20.0), (3, 30.0)] {
            let mut unit = enemy_unit(id);
            unit.x = x;
            unit.y = 0.0;
            unit.health = 100.0;
            world.enemies.insert(id, unit);
        }
        assert!(apply_allied_pierce_damage(
            &world,
            &connections,
            0.0,
            0.0,
            40.0,
            0.0,
            10.0,
            2,
            false,
            1.0,
            -1,
            0.0,
        ));
        let hit = world
            .enemies
            .iter()
            .filter(|unit| unit.health < 100.0)
            .count();
        assert_eq!(hit, 2, "pierce cap 2 hits two units");
        let unhit = world
            .enemies
            .iter()
            .filter(|unit| (unit.health - 100.0).abs() < 0.001)
            .count();
        assert_eq!(unhit, 1);

        world.projectiles.insert(
            99,
            Projectile {
                target_id: 1,
                shooter_id: 9_999_999,
                team: 1,
                bullet_id: 1,
                damage: 5.0,
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
                remaining_ticks: 0.0,
                total_ticks: 1.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 10.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(
            world.enemies.get(&1).unwrap().health > 0.0,
            "missing owner must not panic or wipe the target"
        );
    }

    #[test]
    fn p201_spawn_unit_launchers_are_terminal_projectiles_not_missile_entities() {
        // Vanilla 158.1 anthicus/quell/disrupt/scathe weapons set spawnUnit.
        // The headless model keeps a projectile with the missile's terminal
        // splash instead of inserting MissileUnitType entities (46/53/55/64).
        let (world, connections) = test_world();
        world.enemies.insert(1, enemy_unit(1));
        world.projectiles.insert(
            7,
            Projectile {
                target_id: 1,
                shooter_id: -1,
                team: 1,
                bullet_id: 92,
                damage: 0.75,
                splash_damage: 140.0,
                splash_radius: 25.0,
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
                remaining_ticks: 0.0,
                total_ticks: 1.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 50.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(world.projectiles.is_empty());
        assert!(
            world
                .enemies
                .iter()
                .all(|unit| unit.unit_type != 46 && unit.unit_type != 53 && unit.unit_type != 55),
            "no missile unit was spawned"
        );
        let target_gone_or_hurt = world.enemies.get(&1).is_none_or(|unit| unit.health < 100.0);
        assert!(
            target_gone_or_hurt,
            "terminal splash still damages the target"
        );
    }

    #[test]
    fn p106_status_changes_speed_and_reload_on_the_next_unit_read() {
        use crate::game::status::{STATUS_ELECTRIFIED, STATUS_SLOW};
        use crate::network::combat::{effective_unit_reload_delta, effective_unit_speed};

        let (world, _connections) = test_world();
        let mut unit = enemy_unit(1);
        unit.team = 1;
        unit.move_speed = 1.0;
        world.enemies.insert(1, unit);

        let n_minus_1 = effective_unit_speed(&world.enemies.get(&1).unwrap());
        assert!((n_minus_1 - 1.0).abs() < 1e-5);

        {
            let mut live = world.enemies.get_mut(&1).unwrap();
            StatusContainer::apply_status(&mut *live, STATUS_SLOW, 30.0);
            StatusContainer::apply_status(&mut *live, STATUS_ELECTRIFIED, 30.0);
        }
        let end_n = effective_unit_speed(&world.enemies.get(&1).unwrap());
        let reload_n = effective_unit_reload_delta(&world.enemies.get(&1).unwrap(), 1.0);
        assert!((end_n - 0.4 * 0.7).abs() < 1e-5, "slow*electrified speed");
        assert!((reload_n - 0.6).abs() < 1e-5, "electrified reload");

        {
            let mut live = world.enemies.get_mut(&1).unwrap();
            StatusContainer::tick_statuses(&mut *live, 30.0);
        }
        let end_n1 = effective_unit_speed(&world.enemies.get(&1).unwrap());
        let reload_n1 = effective_unit_reload_delta(&world.enemies.get(&1).unwrap(), 1.0);
        assert!((end_n1 - 1.0).abs() < 1e-5, "expired status restores speed");
        assert!(
            (reload_n1 - 1.0).abs() < 1e-5,
            "expired status restores reload"
        );
    }

    #[test]
    fn p201_direct_projectile_despawns_without_hit_when_lifetime_expires() {
        let (world, connections) = test_world();
        world.enemies.insert(1, enemy_unit(1));
        world.projectiles.insert(
            50,
            Projectile {
                target_id: 999,
                shooter_id: -1,
                team: 1,
                bullet_id: 6,
                damage: 9.0,
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
                total_ticks: 60.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 200.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(
            world.projectiles.is_empty(),
            "missed bolt is removed at expiry"
        );
        assert!(
            (world.enemies.get(&1).unwrap().health - 100.0).abs() < 0.001,
            "no damage when the target id does not resolve"
        );
    }

    #[test]
    fn p201_direct_projectile_skips_damage_when_target_removed() {
        let (world, connections) = test_world();
        world.projectiles.insert(
            51,
            Projectile {
                target_id: 42,
                shooter_id: -1,
                team: 1,
                bullet_id: 6,
                damage: 50.0,
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
                remaining_ticks: 0.0,
                total_ticks: 10.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 50.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(world.projectiles.is_empty());
        assert!(world.enemies.is_empty());
    }

    #[test]
    fn p201_continuous_beam_differential_documents_collapsed_laser() {
        // Official vela-weapon (18): 35 dmg every damageInterval 5 ticks for
        // lifetime 160 along length 180 → up to 32 ticks of damage (~1120 total).
        // The server collapses this to one 35-dmg pierce impact at expiry.
        assert_eq!(unit_weapon_beam_length(18), Some(180.0));
        let vela = enemy_projectile_volley(8).unwrap();
        assert_eq!(vela.bullet_id, 18);
        assert_eq!(vela.direct_damage, 35.0);
        assert_eq!(vela.lifetime, 160.0);
        let official_interval_hits = (vela.lifetime / 5.0).floor();
        let official_total = official_interval_hits * vela.direct_damage;
        assert!(
            official_total > vela.direct_damage * 10.0,
            "collapsed beam is intentionally much lower than interval sum"
        );

        let (world, connections) = test_world();
        for (id, x) in [(1, 10.0), (2, 30.0), (3, 50.0)] {
            let mut unit = enemy_unit(id);
            unit.x = x;
            unit.team = 2;
            world.enemies.insert(id, unit);
        }
        world.projectiles.insert(
            52,
            Projectile {
                target_id: -1,
                shooter_id: 0,
                team: 1,
                bullet_id: 18,
                damage: 35.0,
                splash_damage: 0.0,
                splash_radius: 0.0,
                status_effect: -1,
                status_duration: 0.0,
                pierce_units: u8::MAX,
                pierce_buildings: 0,
                spawn_reign_frags: false,
                homing_range: 0.0,
                enemy_target_position: None,
                enemy_target_core: false,
                apply_direct_on_impact: false,
                armor_multiplier: 1.0,
                remaining_ticks: 0.0,
                total_ticks: 160.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 180.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        let damaged = world
            .enemies
            .iter()
            .filter(|unit| unit.health < 100.0)
            .count();
        assert_eq!(damaged, 3, "collapsed pierce beam still hits the line");
        assert!(
            world.enemies.iter().all(|unit| unit.health >= 65.0),
            "one 35-dmg tick, not interval stacking"
        );
    }

    #[test]
    fn p201_point_defense_partially_damages_then_removes_weak_projectiles() {
        use crate::network::simulation::simulate_enemy_point_defense;

        let (world, _connections) = test_world();
        let mut defender = enemy_unit(10);
        defender.unit_type = 31; // oxynoe PD, 17 dmg, range 100
        defender.team = 1;
        defender.x = 0.0;
        defender.y = 0.0;
        defender.secondary_attack_reload = 9.0;
        world.enemies.insert(10, defender);
        world.projectiles.insert(
            60,
            Projectile {
                target_id: -1,
                shooter_id: -1,
                team: 2,
                bullet_id: 6,
                damage: 10.0,
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
                remaining_ticks: 30.0,
                total_ticks: 30.0,
                source_x: 50.0,
                source_y: 0.0,
                target_x: 0.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        assert!(simulate_enemy_point_defense(&world, 9.0));
        assert!(
            world.projectiles.is_empty(),
            "10 dmg bolt removed by 17 dmg PD"
        );

        {
            let mut defender = world.enemies.get_mut(&10).unwrap();
            defender.secondary_attack_reload = 0.0;
        }
        world.projectiles.insert(
            61,
            Projectile {
                target_id: -1,
                shooter_id: -1,
                team: 2,
                bullet_id: 8,
                damage: 100.0,
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
                remaining_ticks: 30.0,
                total_ticks: 30.0,
                source_x: 50.0,
                source_y: 0.0,
                target_x: 0.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        assert!(simulate_enemy_point_defense(&world, 9.0));
        let remaining = world.projectiles.get(&61).unwrap();
        assert!(
            (remaining.damage - 83.0).abs() < 0.001,
            "one 17-dmg PD hit leaves 83, got {}",
            remaining.damage
        );
    }

    #[test]
    fn p201_reign_and_cyerce_frags_spawn_on_parent_expiry() {
        let (world, connections) = test_world();
        world.enemies.insert(1, enemy_unit(1));
        for (id, bullet_id, spawn_frags) in [(70, 12, true), (71, 56, false)] {
            world.projectiles.insert(
                id,
                Projectile {
                    target_id: 1,
                    shooter_id: 1,
                    team: 1,
                    bullet_id,
                    damage: 10.0,
                    splash_damage: 0.0,
                    splash_radius: 0.0,
                    status_effect: -1,
                    status_duration: 0.0,
                    pierce_units: 0,
                    pierce_buildings: 0,
                    spawn_reign_frags: spawn_frags,
                    homing_range: 0.0,
                    enemy_target_position: None,
                    enemy_target_core: false,
                    apply_direct_on_impact: true,
                    armor_multiplier: 1.0,
                    remaining_ticks: 0.0,
                    total_ticks: 1.0,
                    source_x: 0.0,
                    source_y: 0.0,
                    target_x: 40.0,
                    target_y: 0.0,
                    lifetime_scale: 1.0,
                    source_position: None,
                    damage_interval: None,
                    damage_timer: 0.0,
                },
            );
        }
        simulate_projectiles(&world, &connections, 1.0);
        assert_eq!(
            world
                .projectiles
                .iter()
                .filter(|p| p.bullet_id == 13)
                .count(),
            3,
            "reign spawns three frag bolts"
        );
        assert_eq!(
            world
                .projectiles
                .iter()
                .filter(|p| p.bullet_id == 57)
                .count(),
            7,
            "cyerce spawns seven frag bolts"
        );
    }

    #[test]
    fn p201_bullet_inventory_tsv_is_well_formed() {
        let mut rows = 0usize;
        for line in include_str!("../../game/bullet_inventory.tsv").lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let cols: Vec<_> = line.split('\t').collect();
            assert_eq!(cols.len(), 13, "inventory row must have 13 columns: {line}");
            for cell in &cols[2..] {
                assert!(
                    matches!(*cell, "IMPLEMENTED" | "DEVIATION" | "—"),
                    "invalid classification '{cell}' in row {rows}"
                );
            }
            rows += 1;
        }
        assert!(
            rows >= 80,
            "inventory should cover all authoritative families"
        );
    }

    #[test]
    fn p202_wave_ai_ignores_command_queue_orders() {
        let (world, connections) = test_world();
        let mut unit = enemy_unit(8);
        unit.team = world.wave_rules.read().wave_team;
        unit.x = 40.0;
        unit.y = 40.0;
        unit.move_speed = 1.0;
        world.enemies.insert(8, unit);
        world.unit_orders.insert(
            8,
            crate::network::world::UnitOrder {
                unit_id: 8,
                command: 0,
                stances: 0,
                payload_cooldown: 0.0,
                target_kind: 0,
                target_id: -1,
                target_x: Some(400.0),
                target_y: Some(40.0),
                logic_control: 0,
                queue: Vec::new(),
            },
        );
        crate::network::simulation::simulate_allied_units(&world, &connections, 1.0);
        assert!(
            (world.enemies.get(&8).unwrap().x - 40.0).abs() < 0.001,
            "wave-team units are not CommandAI and ignore the queue"
        );
    }
}
