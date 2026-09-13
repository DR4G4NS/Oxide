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
    boost_properties, collect_allied_weapon_fire, collect_allied_weapon_fire_tick,
    collect_manual_weapon_fire, collision_position_passable, damaged_allied_building_target,
    drain_weapon_timer, effective_unit_build_speed, effective_unit_damage_multiplier,
    effective_unit_reload_delta, effective_unit_speed, effective_unit_speed_on_floor,
    generic_unit_weapon_volley, invalidate_navigation_for_block, scaled_projectile_volley,
    set_unit_weapon_timer, spawn_allied_weapon_fire, spawn_weapon_fire_for_team, unit_can_shoot,
    unit_collision_layer, unit_hit_size, unit_weapon_timer, AlliedWeaponFire,
};
pub(crate) mod enemy;
pub(crate) use enemy::{
    apply_enemy_support_abilities, base_building_at, base_building_tombstone,
    build_navigation_field_class, cancel_transient_world_actions, damage_building,
    dynamic_building_tombstone, enemy_circle_radius, enemy_max_health, floor_at, floor_at_tile,
    hostile_unit_count, move_enemy_in_attack_orbit, navigation_field, navigation_field_class,
    navigation_field_toward, navigation_index, nearest_player_building, overlay_at,
    overlay_at_tile, reregister_team_core, restore_base_buildings, simulate_drowning, spawn_wave,
    tile_is_leg_solid, world_to_tile, NavigationClass, SupportRepairTarget,
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
    naval_weapon_volleys, projectile_armor_multiplier, projectile_building_damage_multiplier,
    projectile_maximum_travel, retusa_mine_shots_between, sap_strength, simulate_projectiles,
    spawn_allied_unit_projectile, spawn_allied_unit_projectile_lateral,
    spawn_continuous_projectile, spawn_continuous_projectile_for_team, spawn_enemy_horizon_bomb,
    spawn_enemy_projectile, spawn_enemy_volley, spawn_navanax_lasers, spawn_projectile,
    spawn_projectile_for_team, spawn_unit_bullet_payload, spawn_unit_frag_carrier,
    spawn_unit_projectile_for_team, unit_weapon_beam_length, volley_mount_count,
    volley_mount_lateral, volley_shot_delay, volley_with_mount_offset, EnemyProjectileVolley,
    AEGIRES_PD, ANTUMBRA_CANNON, ANTUMBRA_MISSILE, ARKYID_ARTILLERY, ARKYID_SAP, ATRAX_SLAG,
    BRYDE_ARTILLERY, BRYDE_MISSILES, CORVUS_LASER, ECLIPSE_FLAK, ECLIPSE_LASER, FLARE_BOLT,
    MEGA_HEAL_A, MEGA_HEAL_B, MINKE_ARTILLERY, MINKE_GUN, OMURA_RAIL, POLY_MISSILE, QUAD_BOMB,
    RETUSA_BOLT, RETUSA_MINE, RISSO_GUN, RISSO_MISSILE, SCEPTER_BOLT, SCEPTER_MOUNT, SEI_CANNON,
    SEI_LAUNCHER, SPIROCT_SAP, SPIROCT_SAP_MOUNT, TOXOPID_CANNON, TOXOPID_SHRAPNEL, VELA_BEAM,
};
mod lightning;
pub(crate) use lightning::{
    lightning_spec, spawn_impact_lightning, DetRand, LightningSpec, LightningTarget,
    LIGHTNING_BULLET_AIR, LIGHTNING_BULLET_ALL, LIGHTNING_BULLET_GROUND,
};

mod damage;
pub(crate) use damage::{
    apply_allied_pierce_damage, apply_allied_pierce_damage_for_team, apply_allied_splash_damage,
    apply_allied_splash_damage_for_team, apply_emp_bullet_effects, apply_enemy_direct_damage,
    apply_enemy_pierce_building_damage, apply_enemy_pierce_player_damage, apply_enemy_rail_damage,
    apply_enemy_shared_pierce_damage, apply_enemy_splash_damage, apply_incoming_unit_damage,
    apply_incoming_unit_damage_in_world, apply_quad_bomb_heal, apply_quad_bomb_heal_for_team,
    apply_splash_building_heal_for_team, apply_unit_armor, building_exists, damage_player,
    damage_team_core, enemy_armor, heal_team_core, kill_enemy, nearest_player_building_in_range,
    point_hits_segment, point_segment_distance, pvp_elimination_winner, simulate_player_combat,
    spawn_cyerce_fragments, spawn_reign_fragments, spawn_toxopid_fragments, unit_effective_armor,
    unit_health_rule, unit_immune_to_status, AlliedPierceTarget, EnemyPierceTarget,
    EnemyRailTarget, EMP_HEAL_PERCENT, EMP_POWER_DAMAGE_SCL, EMP_TIME_DURATION, EMP_TIME_INCREASE,
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
            building_last_damage: DashMap::new(),
            repair_beam_strengths: DashMap::new(),
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
            missile_time: 0.0,
            status_agg: None,
            drown_progress: 0.0,
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
            unit_crash_damage_multiplier: 1.0,
        }
    }

    #[test]
    fn mirrored_command_weapons_use_initialized_reload_over_780_ticks() {
        // Raw TSV reloads are doubled by UnitType.init for mirrored mounts.
        // Count paired volleys, not individual bullets; alternation remains
        // a separate per-mount timing concern.
        for (unit_type, expected) in [
            (0, vec![(6, 30)]),
            (5, vec![(14, 16)]),
            (11, vec![(22, 43)]),
            (21, vec![(37, 13)]),
            (3, vec![(9, 58), (10, 8)]),
            (18, vec![(33, 30), (34, 32)]),
        ] {
            let mut unit = enemy_unit(1);
            unit.unit_type = unit_type;
            let mut counts = std::collections::BTreeMap::new();
            for _ in 0..780 {
                for fire in collect_allied_weapon_fire_tick(&mut unit, 1.0, 50.0).unwrap() {
                    if let AlliedWeaponFire::Projectile(volley) = fire {
                        *counts.entry(volley.bullet_id).or_insert(0) += 1;
                    }
                }
            }
            assert_eq!(
                counts.into_iter().collect::<Vec<_>>(),
                expected,
                "unit {unit_type}"
            );
        }
    }

    #[test]
    fn runtime_weapon_ticks_discard_overcharge_after_idle_or_large_delta() {
        for unit_type in [0, 3, 18] {
            let mut unit = enemy_unit(1);
            unit.unit_type = unit_type;
            unit_combat::accumulate_unit_weapon_timers(&mut unit, 10000.0);
            assert!(!collect_allied_weapon_fire_tick(&mut unit, 10000.0, 50.0)
                .unwrap()
                .is_empty());
            assert!(
                collect_allied_weapon_fire_tick(&mut unit, 1.0, 50.0)
                    .unwrap()
                    .is_empty(),
                "idle backlog for unit {unit_type}"
            );
        }
        for unit_type in [0, 3, 18, 34] {
            let mut unit = enemy_unit(1);
            unit.unit_type = unit_type;
            unit_combat::accumulate_unit_weapon_timers(&mut unit, 10000.0);
            assert!(!collect_manual_weapon_fire(&mut unit, 10000.0, 50.0)
                .unwrap()
                .is_empty());
            assert!(
                collect_manual_weapon_fire(&mut unit, 1.0, 50.0)
                    .unwrap()
                    .is_empty(),
                "manual backlog for unit {unit_type}"
            );
        }
    }

    #[test]
    fn serpulo_ballistic_hits_defenders_without_remote_core_damage() {
        let (world, out) = test_world();
        let mut defender = enemy_unit(1);
        defender.team = 1;
        world.enemies.insert(1, defender);
        let health = *world.game_state.core_health.read();
        spawn_enemy_projectile(
            &world,
            &out,
            999,
            None,
            true,
            enemy_projectile_volley(0).unwrap(),
            0.0,
            0.0,
            100.0,
            0.0,
            0.0,
            0,
        );
        for _ in 0..40 {
            simulate_projectiles(&world, &out, 1.0);
        }
        assert_eq!(world.enemies.get(&1).unwrap().health, 91.0);
        assert_eq!(*world.game_state.core_health.read(), health);
        assert!(world.projectiles.is_empty());
    }

    #[test]
    fn serpulo_ballistic_miss_does_not_damage_a_moved_target() {
        let (world, out) = test_world();
        world.enemies.insert(1, enemy_unit(1));
        spawn_allied_unit_projectile(
            &world,
            &out,
            999,
            1,
            None,
            enemy_projectile_volley(0).unwrap(),
            0.0,
            0.0,
            100.0,
            0.0,
            0,
        );
        simulate_projectiles(&world, &out, 5.0);
        world.enemies.get_mut(&1).unwrap().y = 40.0;
        for _ in 0..40 {
            simulate_projectiles(&world, &out, 1.0);
        }
        assert_eq!(world.enemies.get(&1).unwrap().health, 100.0);
    }

    #[test]
    fn serpulo_ballistic_pierce_hits_each_body_once() {
        let (world, out) = test_world();
        for (id, x) in [(1, 20.0), (2, 40.0)] {
            let mut unit = enemy_unit(id);
            unit.x = x;
            world.enemies.insert(id, unit);
        }
        let mut volley = enemy_projectile_volley(1).unwrap();
        volley.direct_damage = 10.0;
        volley.status_effect = -1;
        volley.pierce_units = 2;
        spawn_allied_unit_projectile(&world, &out, 999, 2, None, volley, 0.0, 0.0, 54.0, 0.0, 0);
        for _ in 0..60 {
            simulate_projectiles(&world, &out, 1.0);
        }
        assert_eq!(world.enemies.get(&1).unwrap().health, 90.0);
        assert_eq!(world.enemies.get(&2).unwrap().health, 90.0);
        assert!(world.projectiles.is_empty());
    }

    #[test]
    fn serpulo_ballistic_respects_each_bullet_air_ground_filter() {
        let (world, out) = test_world();
        let mut ground = enemy_unit(1);
        ground.x = 30.0;
        world.enemies.insert(1, ground);
        let mut air = enemy_unit(2);
        air.unit_type = 15;
        air.x = 60.0;
        world.enemies.insert(2, air);
        let mut volley = enemy_projectile_volley(0).unwrap();
        volley.bullet_id = 43; // Minke flak collidesAir=true, collidesGround=false.
        spawn_allied_unit_projectile(&world, &out, 999, 1, None, volley, 0.0, 0.0, 100.0, 0.0, 0);
        for _ in 0..50 {
            simulate_projectiles(&world, &out, 1.0);
        }
        assert_eq!(world.enemies.get(&1).unwrap().health, 100.0);
        assert_eq!(world.enemies.get(&2).unwrap().health, 91.0);
    }

    #[test]
    fn immunity_table_matches_java_source() {
        // unit_immune_to_status verified against UnitTypes.java +
        // NeoplasmUnitType.java (159.7 source).
        assert!(unit_immune_to_status(1, 1)); // mace: burning
        assert!(!unit_immune_to_status(1, 8)); // mace: no melting
        assert!(unit_immune_to_status(8, 1)); // vela: burning
        assert!(!unit_immune_to_status(8, 8)); // vela: no melting
        assert!(unit_immune_to_status(11, 1)); // atrax: burning
        assert!(unit_immune_to_status(11, 8)); // atrax: melting
        assert!(unit_immune_to_status(34, 1)); // navanax: burning
        assert!(!unit_immune_to_status(34, 8)); // navanax: no melting
        assert!(unit_immune_to_status(40, 1)); // precept: burning
        assert!(unit_immune_to_status(40, 8)); // precept: melting
        assert!(unit_immune_to_status(41, 1)); // vanquish: burning
        assert!(unit_immune_to_status(41, 8)); // vanquish: melting
        assert!(unit_immune_to_status(42, 1)); // conquer: burning
        assert!(unit_immune_to_status(42, 8)); // conquer: melting
        assert!(unit_immune_to_status(56, 1)); // renale (Neoplasm): burning
        assert!(unit_immune_to_status(56, 8)); // renale (Neoplasm): melting
        assert!(unit_immune_to_status(57, 1)); // latum (Neoplasm): burning
        assert!(unit_immune_to_status(57, 8)); // latum (Neoplasm): melting
                                               // Naval units are NOT immune: they only get wet from liquid and CAN burn.
        for naval in 25..=29 {
            assert!(!unit_immune_to_status(naval, 1), "naval {naval} can burn");
            assert!(!unit_immune_to_status(naval, 8), "naval {naval} can melt");
        }
        // No other unit has immunities.
        for unit_type in [0, 2, 3, 5, 10, 30, 49, 50, 60, 68] {
            assert!(!unit_immune_to_status(unit_type, 1));
            assert!(!unit_immune_to_status(unit_type, 8));
        }
    }

    #[test]
    fn naval_units_receive_burning_but_navanax_is_immune() {
        // UnitTypes.java: navanax (34) has `immunities.add(burning)` while
        // naval units (25-29) have no immunities and CAN burn. An
        // incendiary projectile applies burning only to the naval unit.
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
        let navanax = world.enemies.get(&1).unwrap();
        assert!(
            !navanax.statuses.iter().any(|entry| entry.effect == 1),
            "navanax must not receive burning (immunities)"
        );
        let naval = world.enemies.get(&2).unwrap();
        assert!(
            naval
                .statuses
                .iter()
                .any(|entry| entry.effect == 1 && entry.time == 300.0),
            "naval unit received burning through the collection"
        );
    }

    #[test]
    fn incendiary_projectile_burning_persists_with_stacked_statuses_and_dots() {
        // A6: a unit already carrying overdrive (13) hit by an incendiary
        // projectile keeps BOTH statuses in the StatusEntry collection, the
        // legacy view mirrors the first entry, and the burning DoT applies
        // on the next status tick (it must not be lost to a legacy-field
        // overwrite).
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
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
    fn unit_projectile_damage_is_frozen_at_fire() {
        // ASTRA R03: unitDamage is applied when the bullet is created. A live
        // rule change must not rewrite an already-emitted projectile.
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
        let mut rule = default_team_rule();
        rule.unit_damage_multiplier = 1.5;
        world.wave_rules.write().unit_damage_multiplier = 2.0;
        world.wave_rules.write().team_rules.insert(2, rule);
        let target = (10 << 16) | 10;
        let max_health = crate::game::content::block_health(218);
        let building = crate::network::world::DynamicTile {
            logic_control: None,
            position: target,
            block: 218,
            team: 1,
            health: max_health,
            occupied: vec![target],
            ..Default::default()
        };
        world.tiles.insert(target, building);

        let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
        world.projectiles.insert(
            projectile_id,
            Projectile {
                target_id: 0,
                shooter_id: 7,
                team: 2,
                bullet_id: 6,
                damage: 30.0,
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
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
                collided: Vec::new(),
            },
        );
        world.wave_rules.write().unit_damage_multiplier = 9.0;
        world
            .wave_rules
            .write()
            .team_rules
            .get_mut(&2)
            .unwrap()
            .unit_damage_multiplier = 9.0;
        simulate_projectiles(&world, &connections, 2.0);
        let health_after_unit = world.tiles.get(&target).unwrap().health;
        assert!(
            (health_after_unit - (max_health - 30.0)).abs() < 0.001,
            "frozen fire damage, got {health_after_unit} max {max_health}"
        );
    }

    #[test]
    fn p201_pierce_cap_stops_after_the_nth_target_and_missing_owner_is_safe() {
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
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
                homing_power: 0.0,
                homing_delay: -1.0,
                collides_air: true,
                collides_ground: true,
                heals: false,
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
                collided: Vec::new(),
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(
            world.enemies.get(&1).unwrap().health > 0.0,
            "missing owner must not panic or wipe the target"
        );
    }

    #[test]
    fn p201_spawn_unit_launchers_insert_missile_units_on_impact() {
        // Vanilla 158.1 anthicus/quell/disrupt launchers set spawnUnit
        // (BulletType.create): the payload joins the shooter's team as a
        // MissileUnitType entity and the launcher bullet itself never hits()
        // or splashes (create returns null). The headless model flies the
        // launcher projectile to its target point and inserts the unit there.
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
        world.enemies.insert(1, enemy_unit(1));
        world.projectiles.insert(
            7,
            Projectile {
                target_id: 1,
                shooter_id: -1,
                team: 1,
                bullet_id: 92,
                damage: 0.75,
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
                collided: Vec::new(),
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(world.projectiles.is_empty());
        // The anthicus-missile joined the shooter's team at the impact point.
        let missile = world
            .enemies
            .iter()
            .find(|unit| unit.unit_type == 46)
            .expect("spawnUnit payload must insert an anthicus-missile");
        assert_eq!(missile.team, 1);
        assert_eq!(missile.entity_class, 39, "MissileUnitType entity class");
        assert!((missile.x - 50.0).abs() < 1e-3 && missile.y.abs() < 1e-3);
        assert!((missile.health - 55.0).abs() < 1e-3, "spec health");
        assert!(missile.missile_time > 0.0, "TimedKillUnit countdown set");
        // No terminal splash on the launcher: only its 0.75 direct damage.
        let target_health = world.enemies.get(&1).unwrap().health;
        assert!((target_health - 99.25).abs() < 1e-3);
    }

    #[test]
    fn p201_spawn_unit_without_spec_or_payload_is_refused() {
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
        // A plain bullet has no spawnUnit payload: expiry inserts nothing.
        world.projectiles.insert(
            7,
            Projectile {
                target_id: -1,
                shooter_id: -1,
                team: 2,
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
                homing_power: 0.0,
                homing_delay: -1.0,
                collides_air: true,
                collides_ground: true,
                heals: false,
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
                collided: Vec::new(),
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(world.projectiles.is_empty());
        assert!(world.enemies.is_empty(), "no unit for unmapped bullets");
        // Unit types without an enemy_spec keep refusing insertion.
        assert!(spawn_unit_world(&world, 9999, 2, 0.0, 0.0, 0.0).is_none());
        assert!(world.enemies.is_empty());
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
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
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
                total_ticks: 60.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 200.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
                collided: Vec::new(),
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
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
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
                homing_power: 0.0,
                homing_delay: -1.0,
                collides_air: true,
                collides_ground: true,
                heals: false,
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
                collided: Vec::new(),
            },
        );
        simulate_projectiles(&world, &connections, 1.0);
        assert!(world.projectiles.is_empty());
        assert!(world.enemies.is_empty());
    }

    #[test]
    fn p201_continuous_beam_deals_interval_damage_while_the_beam_persists() {
        // Official vela-weapon (18): 35 dmg every damageInterval 5 ticks for
        // lifetime 160 along length 180 → up to 32 interval hits (~1120
        // total). The server now keeps the beam alive and applies the
        // interval damage every 5 ticks instead of one collapsed impact.
        assert_eq!(unit_weapon_beam_length(18), Some(180.0));
        let vela = enemy_projectile_volley(8).unwrap();
        assert_eq!(vela.bullet_id, 18);
        assert_eq!(vela.direct_damage, 35.0);
        assert_eq!(vela.lifetime, 160.0);

        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
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
                homing_power: 0.0,
                homing_delay: -1.0,
                collides_air: true,
                collides_ground: true,
                heals: false,
                enemy_target_position: None,
                enemy_target_core: false,
                apply_direct_on_impact: false,
                armor_multiplier: 1.0,
                remaining_ticks: 160.0,
                total_ticks: 160.0,
                source_x: 0.0,
                source_y: 0.0,
                target_x: 180.0,
                target_y: 0.0,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: Some(5.0),
                damage_timer: 0.0,
                collided: Vec::new(),
            },
        );
        // Ticks 1-4: inside the first damageInterval, no damage yet.
        for _ in 0..4 {
            simulate_projectiles(&world, &connections, 1.0);
        }
        assert!(
            world.enemies.iter().all(|unit| unit.health == 100.0),
            "no damage before the first damageInterval elapses"
        );
        // Ticks 5-15: three interval hits of 35 along the piercing line.
        for _ in 0..11 {
            simulate_projectiles(&world, &connections, 1.0);
        }
        let damaged = world
            .enemies
            .iter()
            .filter(|unit| unit.health < 100.0)
            .count();
        assert_eq!(damaged, 3, "interval damage reaches the whole beam line");
        assert!(
            world
                .enemies
                .iter()
                .all(|unit| (unit.health - 65.0).abs() < 0.001),
            "three 35-dmg interval ticks accumulated, got {:?}",
            world.enemies.iter().map(|u| u.health).collect::<Vec<_>>()
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
                homing_power: 0.0,
                homing_delay: -1.0,
                collides_air: true,
                collides_ground: true,
                heals: false,
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
                collided: Vec::new(),
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
                homing_power: 0.0,
                homing_delay: -1.0,
                collides_air: true,
                collides_ground: true,
                heals: false,
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
                collided: Vec::new(),
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
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
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
                    homing_power: 0.0,
                    homing_delay: -1.0,
                    collides_air: true,
                    collides_ground: true,
                    heals: false,
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
                    collided: Vec::new(),
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
        let (world, _) = test_world();
        let (connections, mut _rx) = recording_connections();
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
        world.wave_rules.write().waves_enabled = true;
        crate::network::simulation::simulate_allied_units(&world, &connections, 1.0);
        assert!(
            (world.enemies.get(&8).unwrap().x - 40.0).abs() < 0.001,
            "wave-team units are not CommandAI and ignore the queue"
        );
    }
    /// Captured CreateBullet payloads from a single-connection registry.
    use crate::network::codec::Reads;
    fn capture_create_bullet_payloads(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> Vec<i16> {
        let mut ids = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            // Frame layout: [u16 BE tcp_len][packet...]; the packet body may
            // be compressed, so decode it through the codec.
            let packet =
                crate::network::codec::read_packet(std::io::Cursor::new(&frame[2..])).unwrap();
            assert_eq!(packet[0], CREATE_BULLET_PACKET_ID, "CreateBullet frame");
            let mut cursor = std::io::Cursor::new(&packet[1..]);
            let bullet_id = cursor.read_s().unwrap();
            ids.push(bullet_id);
        }
        ids
    }

    fn recording_connections() -> (
        dashmap::DashMap<i32, crate::network::world::PendingConnection>,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        let (tx, rx) =
            tokio::sync::mpsc::channel(crate::network::wire::outbound::OUTBOUND_QUEUE_CAPACITY);
        let connections = dashmap::DashMap::new();
        connections.insert(
            1,
            crate::network::world::PendingConnection {
                ip: "127.0.0.1".parse().unwrap(),
                outbound: tx,
                udp_inbound: tokio::sync::mpsc::unbounded_channel().0,
                udp_endpoint: std::sync::Arc::new(parking_lot::RwLock::new(None)),
                udp_socket: None,
                player_name: std::sync::Arc::new(parking_lot::RwLock::new(Some(
                    "lightning".into(),
                ))),
                outbound_drops: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                critical_drops: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                last_keepalive_rtt_ms: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                last_packet_epoch_ms: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
                outbound_queued: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            },
        );
        (connections, rx)
    }

    fn lightning_test_projectile(_id: i32, bullet_id: i16, impact_x: f32) -> Projectile {
        Projectile {
            target_id: -1,
            shooter_id: -1,
            team: 1,
            bullet_id,
            damage: 0.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            homing_power: 0.0,
            collides_air: true,
            collides_ground: true,
            heals: false,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: false,
            armor_multiplier: 1.0,
            remaining_ticks: 1.0,
            total_ticks: 1.0,
            source_x: 0.0,
            source_y: 0.0,
            target_x: impact_x,
            target_y: 0.0,
            lifetime_scale: 1.0,
            source_position: None,
            damage_interval: None,
            damage_timer: 0.0,
            collided: Vec::new(),
            homing_delay: -1.0,
        }
    }

    fn insert_wall(
        world: &crate::network::world::DynamicWorld,
        position: i32,
        health: f32,
        block_override: Option<i16>,
    ) {
        insert_wall_at_team(world, position, block_override.unwrap_or(216), 1, health);
    }

    fn insert_wall_at_team(
        world: &crate::network::world::DynamicWorld,
        position: i32,
        block: i16,
        team: u8,
        health: f32,
    ) {
        let tile = crate::network::world::DynamicTile {
            logic_control: None,
            position,
            block,
            rotation: 0,
            team,
            config: Vec::new(),
            enabled: true,
            message: None,
            occupied: vec![position],
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
            payload_rotation: 90.0,
            payload_accum: Vec::new(),
            health,
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
            payload_inventory: Vec::new(),
        };
        world.tiles.insert(position, tile);
    }
    #[test]
    fn squads_below_rts_min_squad_have_no_shared_target() {
        // findTarget returns null for squads smaller than rtsMinSquad (4).
        let (world, _connections) = test_world();
        for (id, x) in [(1, 100.0), (2, 120.0)] {
            let mut e = enemy_unit(id);
            e.unit_type = 0;
            e.x = x;
            e.y = 100.0;
            world.enemies.insert(id, e);
        }
        insert_wall(
            &world,
            (30 << 16) | 12,
            crate::game::content::block_health(216),
            None,
        );

        let assignments = crate::network::simulation::waves::squad_target_assignments(&world);
        assert!(
            assignments.is_empty(),
            "a two-unit squad must not claim a shared target"
        );
    }

    #[test]
    fn squads_of_four_claim_distinct_nearest_targets() {
        // assignedTargets: the first squad takes the nearest building, the
        // second squad is pushed to a different one.
        let (world, _connections) = test_world();
        // Squad A around (100,100), squad B around (100,300).
        for (id, x, y) in [
            (1, 100.0, 100.0),
            (2, 120.0, 100.0),
            (3, 100.0, 120.0),
            (4, 120.0, 120.0),
        ] {
            let mut e = enemy_unit(id);
            e.x = x;
            e.y = y;
            world.enemies.insert(id, e);
        }
        for (id, x, y) in [
            (5, 100.0, 300.0),
            (6, 120.0, 300.0),
            (7, 100.0, 320.0),
            (8, 120.0, 320.0),
        ] {
            let mut e = enemy_unit(id);
            e.x = x;
            e.y = y;
            world.enemies.insert(id, e);
        }
        let wall_max = crate::game::content::block_health(216);
        insert_wall(&world, (20 << 16) | 12, wall_max, Some(1)); // near squad A
        insert_wall(&world, (20 << 16) | 40, wall_max, Some(2)); // near squad B

        let assignments = crate::network::simulation::waves::squad_target_assignments(&world);
        let a = assignments.get(&1).expect("squad A has a target");
        assert_eq!(assignments.get(&2), Some(a));
        assert_eq!(a.0, (20 << 16) | 12, "squad A claims its nearest wall");
        let b = assignments.get(&5).expect("squad B has a target");
        assert_eq!(b.0, (20 << 16) | 40, "squad B claims a different wall");
    }

    #[test]
    fn position_target_flow_skirts_a_concave_wall() {
        // Pathfinder PositionTarget: a U-shaped environment wall between the
        // unit and the goal must route the first step around the pocket, not
        // north into the dead-end (audit H13).
        let (mut world, _connections) = test_world();
        // Copper-wall as map solids (not team-1 buildings, so they are not
        // squad targets). Opening faces south; goal is north of the bar.
        for x in 8..=12 {
            world.base_blocks[(14 * 40 + x) as usize] = 216;
        }
        for y in 9..=14 {
            world.base_blocks[(y * 40 + 8) as usize] = 216;
            world.base_blocks[(y * 40 + 12) as usize] = 216;
        }
        world
            .navigation_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let mut enemy = enemy_unit(1);
        enemy.x = 10.0 * 8.0;
        enemy.y = 10.0 * 8.0;
        let goal_x = 10.0 * 8.0;
        let goal_y = 20.0 * 8.0;
        let (nx, ny) = crate::network::units::mining::first_flow_step_toward(
            &world,
            &enemy,
            goal_x,
            goal_y,
            &[],
        );
        let step_tx = (nx / 8.0).floor() as i32;
        assert!(
            ny < enemy.y + 0.1 || step_tx != 10,
            "must not walk north up the U's dead-end column, got ({nx},{ny})"
        );
    }

    #[test]
    fn squads_rush_an_exposed_core_first() {
        // RtsAI weights cores at -999999: any core beats a closer wall.
        let (world, _connections) = test_world();
        for (id, x, y) in [
            (1, 100.0, 100.0),
            (2, 120.0, 100.0),
            (3, 100.0, 120.0),
            (4, 120.0, 120.0),
        ] {
            let mut e = enemy_unit(id);
            e.x = x;
            e.y = y;
            world.enemies.insert(id, e);
        }
        let wall_max = crate::game::content::block_health(216);
        insert_wall(&world, (21 << 16) | 12, wall_max, Some(3)); // close wall
        insert_wall(&world, (60 << 16) | 12, wall_max, Some(339)); // far CORE

        let assignments = crate::network::simulation::waves::squad_target_assignments(&world);
        let target = assignments.get(&1).expect("squad has a target");
        assert_eq!(
            target.0,
            (60 << 16) | 12,
            "the core wins over the closer wall"
        );
    }

    #[test]
    fn damaged_own_building_pulls_a_small_squad_back() {
        // handleSquad defend branch: own recently damaged buildings pull even
        // sub-rtsMinSquad squads back to defend.
        let (world, _connections) = test_world();
        for (id, x, y) in [(1, 100.0, 100.0), (2, 120.0, 100.0)] {
            let mut e = enemy_unit(id);
            e.x = x;
            e.y = y;
            world.enemies.insert(id, e);
        }
        let wall_max = crate::game::content::block_health(216);
        // A wave-team (2) wall at half health, damaged this tick.
        insert_wall_at_team(&world, (30 << 16) | 12, 216, 2, wall_max / 2.0);
        crate::network::combat::enemy::record_damaged_building(&world, (30 << 16) | 12);

        // A stale-damaged wall (below max, but hurt outside the window)
        // must NOT qualify.
        insert_wall_at_team(&world, (40 << 16) | 12, 216, 2, wall_max / 2.0);

        let assignments = crate::network::simulation::waves::squad_target_assignments(&world);
        let shared = assignments.get(&1).expect("defending squad has a target");
        assert_eq!(assignments.get(&2), Some(shared));
        assert_eq!(shared.0, (30 << 16) | 12);
    }

    #[test]
    fn squads_share_one_target_and_loners_keep_their_own() {
        // RtsAI.assignSquads: units within the square query form a squad and
        // share one target; a lone unit never joins and stays unassigned.
        let (world, _connections) = test_world();
        let mut members = Vec::new();
        let spots = [
            (1, 100.0, 100.0),
            (2, 120.0, 100.0),
            (3, 100.0, 120.0),
            (4, 120.0, 120.0),
        ];
        for &(id, x, y) in &spots {
            let mut e = enemy_unit(id);
            e.unit_type = 0;
            e.x = x;
            e.y = y;
            world.enemies.insert(id, e);
            members.push((id, x, y));
        }
        let mut loner = enemy_unit(5);
        loner.unit_type = 1; // mace
        loner.x = 900.0;
        loner.y = 900.0;
        world.enemies.insert(5, loner);

        insert_wall(
            &world,
            (30 << 16) | 12,
            crate::game::content::block_health(216),
            None,
        );

        let assignments = crate::network::simulation::waves::squad_target_assignments(&world);
        let first = assignments.get(&1).expect("four-unit squad has a target");
        for &(id, _, _) in &members {
            assert_eq!(
                assignments.get(&id),
                Some(first),
                "every squad member shares one target"
            );
        }
        assert!(!assignments.contains_key(&5), "loner forms no squad");
    }
    #[test]
    fn corvus_beam_hits_once_while_vela_ticks_per_interval() {
        // JAR class probe: vela (18) is ContinuousLaserBulletType (35 dmg
        // every damageInterval); corvus (20) is LaserBulletType - one 560
        // impact, no interval ticking.
        use crate::network::combat::projectiles::is_continuous_laser;
        assert!(is_continuous_laser(18));
        assert!(!is_continuous_laser(20));
        let (world, connections) = test_world();
        let mut target = enemy_unit(1);
        target.health = 10_000.0;
        world.enemies.insert(1, target);

        // Corvus: a full 65-tick flight applies the 560 damage exactly once.
        let mut corvus = lightning_test_projectile(9, 20, 100.0);
        corvus.damage = 560.0;
        corvus.pierce_units = u8::MAX;
        corvus.remaining_ticks = 65.0;
        corvus.total_ticks = 65.0;
        world.projectiles.insert(9, corvus);
        simulate_projectiles(&world, &connections, 65.0);
        let after_corvus = world.enemies.get(&1).unwrap().health;
        assert!(
            (after_corvus - (10_000.0 - 560.0)).abs() < 0.01,
            "corvus applies exactly one 560 hit, got {after_corvus}"
        );
        world.projectiles.remove(&9);
    }
    #[test]
    fn timed_kill_missiles_self_expire_like_timedkillunit() {
        // TimedKillUnit.updateTile: time -= Time.delta, kill at zero. The
        // wire declares the countdown; wave simulation must honor it.
        let (world, connections) = test_world();
        let mut missile = enemy_unit(1);
        missile.unit_type = 46; // anthicus-missile
        missile.entity_class = 39; // TimedKillUnit
        missile.missile_time = 2.0;
        world.enemies.insert(1, missile);
        let mut survivor = enemy_unit(2);
        survivor.unit_type = 65; // scathe-missile-phase
        survivor.entity_class = 39;
        survivor.missile_time = 586.2;
        world.enemies.insert(2, survivor);

        crate::network::simulation::waves::simulate_waves_and_enemies(&world, &connections, 1.0);
        let aged = world.enemies.get(&1).unwrap();
        assert!((aged.missile_time - 1.0).abs() < 0.001, "countdown ages");
        drop(aged);

        crate::network::simulation::waves::simulate_waves_and_enemies(&world, &connections, 1.5);
        assert!(
            !world.enemies.contains_key(&1),
            "expired missile must self-destruct"
        );
        assert!(world.enemies.contains_key(&2));
    }

    #[test]
    fn homing_units_outrank_buildings_and_walls_catch_stray_missiles() {
        // Units.closestTarget: units are checked FIRST; buildings are only
        // consulted when no opposing unit is inside homingRange.
        let (world, connections) = test_world();
        let mut missile = lightning_test_projectile(21, 41, 100.0);
        missile.team = 2; // enemy missile hunting team-1 targets
        missile.homing_range = 50.0;
        // High turn rate: one tick rotates the stored impact point fully onto
        // the aim bearing, so the selected CANDIDATE is observable exactly.
        missile.homing_power = 100.0;
        missile.remaining_ticks = 5.0;
        missile.total_ticks = 10.0;
        missile.source_x = 0.0;
        missile.source_y = 0.0;
        missile.target_x = 100.0;
        missile.target_y = 0.0;
        world.projectiles.insert(21, missile);

        insert_wall(
            &world,
            (7 << 16) | 2, // tile origin (56, 16)
            crate::game::content::block_health(216),
            None,
        );

        // Mid-flight at (50, 0): a live player at ~22 units beats the wall
        // at ~17 even though the wall is nearer.
        let player = crate::network::world::PlayerCombatState {
            uuid: "p1".into(),
            player_id: 1,
            unit_id: 900,
            x: 60.0,
            y: 20.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        };
        world.players.insert(900, player);
        simulate_projectiles(&world, &connections, 1.0);
        // Full turn onto the player bearing atan2(20 - 0, 60 - 50) = ~63.4 deg,
        // impact point kept at radius 50 around the mid-flight position.
        let tracked = world.projectiles.get(&21).unwrap();
        let heading = ((tracked.target_y - 0.0).atan2(tracked.target_x - 50.0)).to_degrees();
        assert!(
            (heading - 20.0f32.atan2(10.0).to_degrees()).abs() < 0.5,
            "units always outrank buildings, heading was {heading}"
        );
        drop(tracked);

        // With the player gone the wall catches the lock (fresh missile so
        // the previous tick's rotation does not skew the geometry).
        world.players.remove(&900);
        let mut stray = lightning_test_projectile(22, 41, 100.0);
        stray.team = 2;
        stray.homing_range = 50.0;
        stray.homing_power = 100.0;
        stray.remaining_ticks = 5.0;
        stray.total_ticks = 10.0;
        stray.source_x = 0.0;
        stray.source_y = 0.0;
        stray.target_x = 100.0;
        stray.target_y = 0.0;
        world.projectiles.insert(22, stray);
        simulate_projectiles(&world, &connections, 1.0);
        let tracked = world.projectiles.get(&22).unwrap();
        let heading = ((tracked.target_y - 0.0).atan2(tracked.target_x - 50.0)).to_degrees();
        assert!(
            (heading - 16.0f32.atan2(6.0).to_degrees()).abs() < 0.5,
            "no units left: the building takes the lock, heading was {heading}"
        );
    }
    #[test]
    fn homing_projectiles_reaim_toward_a_live_target_inside_range() {
        // Official BulletType.updateHoming model: the missile tracks the
        // nearest opposing live unit inside homingRange instead of flying
        // ballistically to the fire-time point.
        let (world, connections) = test_world();
        let mut target = enemy_unit(1);
        target.x = 80.0;
        target.y = 30.0;
        world.enemies.insert(1, target);
        let mut missile = lightning_test_projectile(9, 41, 100.0);
        missile.target_id = 1;
        missile.homing_range = 50.0;
        missile.homing_power = 100.0;
        missile.remaining_ticks = 5.0;
        missile.total_ticks = 10.0;
        missile.source_x = 0.0;
        missile.source_y = 0.0;
        missile.target_x = 100.0;
        missile.target_y = 0.0;
        world.projectiles.insert(9, missile);
        simulate_projectiles(&world, &connections, 1.0);
        // Full turn onto the live target bearing atan2(30, 30) = 45 deg from
        // the mid-flight position (50, 0).
        let tracked = world.projectiles.get(&9).unwrap();
        let heading = ((tracked.target_y - 0.0).atan2(tracked.target_x - 50.0)).to_degrees();
        assert!(
            (heading - 45.0).abs() < 0.5,
            "missile must lock the live unit inside range, heading was {heading}"
        );
        drop(tracked);
        world.projectiles.remove(&9);

        // Retargeting: closestTarget runs every tick, so a nearer live unit
        // steals the lock from the fire-time target.
        let mut near = enemy_unit(3);
        near.x = 55.0;
        near.y = -25.0; // distinct bearing from the far unit
        world.enemies.insert(3, near);
        let mut stealer = lightning_test_projectile(11, 41, 100.0);
        stealer.homing_range = 50.0;
        stealer.homing_power = 100.0;
        stealer.remaining_ticks = 5.0;
        stealer.total_ticks = 10.0;
        stealer.source_x = 0.0;
        stealer.source_y = 0.0;
        stealer.target_x = 100.0;
        stealer.target_y = 0.0;
        world.projectiles.insert(11, stealer);
        simulate_projectiles(&world, &connections, 1.0);
        // The nearer unit steals the lock: heading rotates onto atan2(-25, 5)
        // instead of the far unit's +45 deg.
        let stolen = world.projectiles.get(&11).unwrap();
        let heading = ((stolen.target_y - 0.0).atan2(stolen.target_x - 50.0)).to_degrees();
        assert!(
            (heading - (-25.0f32).atan2(5.0).to_degrees()).abs() < 0.5,
            "nearer live unit must steal the lock, heading was {heading}"
        );
        drop(stolen);
        world.projectiles.remove(&11);

        // Ballistic fallback: outside homingRange the fire-time point wins.
        let mut far = enemy_unit(2);
        far.x = 300.0;
        far.y = 300.0;
        world.enemies.insert(2, far);
        // Remove the in-range units so nothing steals the lock.
        world.enemies.remove(&1);
        world.enemies.remove(&3);
        let mut ballistic = lightning_test_projectile(10, 41, 100.0);
        ballistic.target_id = 2;
        ballistic.homing_range = 50.0;
        ballistic.remaining_ticks = 5.0;
        ballistic.total_ticks = 10.0;
        world.projectiles.insert(10, ballistic);
        simulate_projectiles(&world, &connections, 1.0);
        let untracked = world.projectiles.get(&10).unwrap();
        assert_eq!(untracked.target_x, 100.0);
        assert_eq!(untracked.target_y, 0.0);
        drop(untracked);
        world.projectiles.remove(&10);
    }

    #[test]
    fn scepter_impact_spawns_authoritative_lightning_chains_deterministically() {
        // Scepter main bolt (10): lightning = 2 roots x length 6 → 3 segments each,
        // 20 dmg per segment, emitted as free wire id 1 (damageLightning).
        let run = || {
            let (world, _) = test_world();
            let (connections, mut rx) = recording_connections();
            let mut near = enemy_unit(1);
            near.x = 115.0;
            near.y = 0.0;
            world.enemies.insert(1, near);
            world
                .projectiles
                .insert(7, lightning_test_projectile(7, 10, 100.0));
            simulate_projectiles(&world, &connections, 1.0);
            let healths: Vec<f32> = world.enemies.iter().map(|u| u.health).collect();
            (healths, capture_create_bullet_payloads(&mut rx))
        };
        let (healths_a, frames_a) = run();
        let (healths_b, _frames_b) = run();

        assert_eq!(frames_a.len(), 6, "2 roots x 3 segments of CreateBullet");
        assert!(
            frames_a.iter().all(|id| *id == LIGHTNING_BULLET_ALL),
            "every chain segment uses the free all-target lightning id, got {:?}",
            frames_a
        );
        assert_eq!(healths_a, healths_b, "chains are deterministic per seed");
        assert!(
            healths_a[0] < 100.0,
            "the chained unit takes per-segment bolt damage, got {}",
            healths_a[0]
        );
    }

    #[test]
    fn pulsar_and_arkyid_lightning_use_the_free_wire_ids_with_java_chain_lengths() {
        // Pulsar heal-shotgun (15): LightningBulletType, 1 root, length
        // 8 + rand(7) → 4..=7 segments at 15 dmg.
        let (world, _) = test_world();
        let (connections, mut rx) = recording_connections();
        world
            .projectiles
            .insert(11, lightning_test_projectile(11, 15, 90.0));
        simulate_projectiles(&world, &connections, 1.0);
        let pulsar_frames = capture_create_bullet_payloads(&mut rx);
        assert!(
            (4..=7).contains(&pulsar_frames.len()),
            "pulsar chain walks length/2 segments, got {}",
            pulsar_frames.len()
        );
        assert!(
            pulsar_frames.iter().all(|id| *id == LIGHTNING_BULLET_ALL),
            "pulsar lightningType hits air and ground"
        );

        // Arkyid large-purple-mount (26): lightning = 3 roots x length 10 →
        // 5 segments each at the host bullet damage (12).
        let (world, _) = test_world();
        let (connections, mut rx) = recording_connections();
        world
            .projectiles
            .insert(12, lightning_test_projectile(12, 26, 90.0));
        simulate_projectiles(&world, &connections, 1.0);
        let arkyid_frames = capture_create_bullet_payloads(&mut rx);
        assert_eq!(arkyid_frames.len(), 15, "3 roots x 5 segments");
        assert!(
            arkyid_frames.iter().all(|id| *id == LIGHTNING_BULLET_ALL),
            "arkyid chains use the free all-target lightning id"
        );
    }

    #[test]
    fn ground_and_air_lightning_targets_filter_units_like_collides_flags() {
        // Official damageLightningGround (collidesAir = false) must skip a
        // flying unit; damageLightningAir (collidesGround = false) must skip
        // a grounded one.
        let spec_ground = LightningSpec {
            roots: 1,
            length: 6,
            length_rand: 0,
            damage: 20.0,
            target: LightningTarget::Ground,
        };
        let spec_air = LightningSpec {
            target: LightningTarget::Air,
            ..spec_ground
        };

        // Unit type 15 is flying in unit_movement.tsv; type 0 is grounded.
        let (world, _) = test_world();
        let (connections, mut rx) = recording_connections();
        let mut flyer = enemy_unit(1);
        flyer.unit_type = 15;
        flyer.x = 110.0;
        flyer.y = 0.0;
        world.enemies.insert(1, flyer);
        spawn_impact_lightning(
            &world,
            &connections,
            1,
            spec_ground,
            5,
            0.0,
            0.0,
            100.0,
            0.0,
        );
        assert_eq!(
            world.enemies.get(&1).unwrap().health,
            100.0,
            "ground-only chain never visits a flying unit"
        );
        let ground_frames = capture_create_bullet_payloads(&mut rx);
        assert!(!ground_frames.is_empty(), "segments are still emitted");
        assert!(
            ground_frames
                .iter()
                .all(|id| *id == LIGHTNING_BULLET_GROUND),
            "ground-only chains use free wire id 2"
        );

        let (world, _) = test_world();
        let (connections, mut rx) = recording_connections();
        let mut walker = enemy_unit(1);
        walker.unit_type = 0;
        walker.x = 110.0;
        walker.y = 0.0;
        world.enemies.insert(1, walker);
        spawn_impact_lightning(&world, &connections, 1, spec_air, 6, 0.0, 0.0, 100.0, 0.0);
        assert_eq!(
            world.enemies.get(&1).unwrap().health,
            100.0,
            "air-only chain never visits a grounded unit"
        );
        let air_frames = capture_create_bullet_payloads(&mut rx);
        assert!(
            air_frames.iter().all(|id| *id == LIGHTNING_BULLET_AIR),
            "air-only chains use free wire id 3"
        );
    }

    #[test]
    fn homing_turn_rate_caps_reaim_per_tick() {
        // Official updateHoming steering: the heading turns toward the aim at
        // most homingPower * Time.delta * 50 degrees per tick (poly missile:
        // 0.08 -> 4 deg/tick). It does NOT snap onto the target in one tick.
        let (world, connections) = test_world();
        let mut target = enemy_unit(1);
        target.x = 60.0;
        target.y = -40.0; // ~76 degrees off the current heading
        world.enemies.insert(1, target);
        let mut missile = lightning_test_projectile(9, 41, 100.0);
        missile.team = 1;
        missile.homing_range = 50.0;
        missile.homing_power = 0.08;
        missile.remaining_ticks = 5.0;
        missile.total_ticks = 10.0;
        missile.source_x = 0.0;
        missile.source_y = 0.0;
        missile.target_x = 100.0;
        missile.target_y = 0.0; // heading +x at (50, 0)
        world.projectiles.insert(9, missile);

        simulate_projectiles(&world, &connections, 1.0);
        // Decision geometry inside simulate_projectiles: bullet at (50, 0),
        // heading +x, aim ~76 degrees below -> one capped step of
        // 0.08 * 50 = 4 deg rotates the stored impact point around (50, 0):
        // (50, 0) + 50 * (cos -4deg, sin -4deg).
        let tracked = world.projectiles.get(&9).unwrap();
        let expected_x = 50.0 + 50.0 * (-4.0f32.to_radians()).cos();
        let expected_y = 50.0 * (-4.0f32.to_radians()).sin();
        assert!(
            (tracked.target_x - expected_x).abs() < 1.0e-3
                && (tracked.target_y - expected_y).abs() < 1.0e-3,
            "one tick must turn exactly the capped 4 deg toward the aim, got ({}, {})",
            tracked.target_x,
            tracked.target_y
        );
        drop(tracked);
        world.projectiles.remove(&9);
    }

    #[test]
    fn ground_only_homing_ignores_flying_units() {
        // Unit.checkTarget(collidesAir, collidesGround): a ground-only bolt
        // never locks a flying unit even when it is the only candidate.
        let (world, connections) = test_world();
        let mut flyer = enemy_unit(1);
        flyer.elevation = 1.0; // flying regardless of unit_movement()
        flyer.x = 60.0;
        flyer.y = 10.0;
        world.enemies.insert(1, flyer);
        let mut missile = lightning_test_projectile(11, 41, 100.0);
        missile.team = 1;
        missile.collides_air = false;
        missile.homing_range = 50.0;
        missile.homing_power = 0.0;
        missile.remaining_ticks = 5.0;
        missile.total_ticks = 10.0;
        missile.source_x = 0.0;
        missile.source_y = 0.0;
        missile.target_x = 200.0;
        missile.target_y = 0.0;
        world.projectiles.insert(11, missile);

        simulate_projectiles(&world, &connections, 1.0);
        let tracked = world.projectiles.get(&11).unwrap();
        assert_eq!(
            (tracked.target_x, tracked.target_y),
            (200.0, 0.0),
            "flyer must be filtered out of the homing scan"
        );
    }

    #[test]
    fn heal_bolt_homes_to_allied_unit_not_closer_enemy() {
        // heals() bolts pass bullet.team as closestTarget's team filter: they
        // scan ALLIED units only and never lock opposing units, no matter how
        // much closer the enemy is.
        let (world, connections) = test_world();
        let mut foe = enemy_unit(1);
        foe.x = 55.0;
        foe.y = 12.0; // closer than the ally, outside this tick's collision segment
        world.enemies.insert(1, foe);
        let ally = crate::network::world::PlayerCombatState {
            uuid: "ally".into(),
            player_id: 7,
            unit_id: 901,
            x: 70.0,
            y: 20.0,
            health: 40.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        };
        world.players.insert(901, ally);
        let mut bolt = lightning_test_projectile(13, 37, 100.0);
        bolt.team = 1;
        bolt.heals = true;
        bolt.homing_range = 50.0;
        bolt.homing_power = 0.08;
        bolt.remaining_ticks = 5.0;
        bolt.total_ticks = 10.0;
        bolt.source_x = 0.0;
        bolt.source_y = 0.0;
        bolt.target_x = 100.0;
        bolt.target_y = 0.0;
        world.projectiles.insert(13, bolt);

        simulate_projectiles(&world, &connections, 1.0);
        // Capped steering turned the stored impact point by exactly one
        // 4-degree step toward the ALLY (heading +x -> +4 deg): (50, 0) +
        // 50 * (cos 4deg, sin 4deg) -- never snapped onto either candidate.
        let tracked = world.projectiles.get(&13).unwrap();
        let expected_x = 50.0 + 50.0 * 4.0f32.to_radians().cos();
        let expected_y = 50.0 * 4.0f32.to_radians().sin();
        assert!(
            (tracked.target_x - expected_x).abs() < 1.0e-3
                && (tracked.target_y - expected_y).abs() < 1.0e-3,
            "heal bolt must steer toward the ally at the capped rate, got ({}, {})",
            tracked.target_x,
            tracked.target_y
        );
        drop(tracked);
    }
}
