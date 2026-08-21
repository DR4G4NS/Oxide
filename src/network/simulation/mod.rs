//! World simulation loop.
//!
//! Extracted from `listener.rs` (P2 listener split): the `spawn_world_simulation`
//! task that owns the authoritative tick loop (TPS pacing, logic processors,
//! waves/enemies, pvp damage, collisions) plus the simulation delta helpers.

use crate::network::buildings::config as building_config;
use crate::network::buildings::construction::{
    block_footprint, consume_requirements, dynamic_at, effective_block,
    encode_begin_place_for_unit, simulate_breaks, simulate_constructions,
};
use crate::network::buildings::placement as building_placement;
use crate::network::buildings::plans::{rebuild_plan, AssistTarget};
use crate::network::buildings::reactor;
use crate::network::buildings::sandbox::SandboxSystem;
use crate::network::buildings::snapshot::{
    angle_between, build_payload_version, dynamic_tile_health, encode_payload_build_sync,
    is_pickup_payload_supported, valid_logic_config,
};
use crate::network::combat::apply_allied_splash_damage;
use crate::network::combat::apply_enemy_direct_damage;
use crate::network::combat::apply_enemy_splash_damage;
use crate::network::combat::apply_incoming_unit_damage;
use crate::network::combat::damage_player;
use crate::network::combat::damaged_allied_building_on_ray;
use crate::network::combat::enemy::{
    apply_enemy_support_abilities, base_building_at, base_building_tombstone, damage_building,
    enemy_circle_radius, enemy_max_health, hostile_unit_count, move_enemy_in_attack_orbit,
    navigation_index, spawn_wave,
};
use crate::network::combat::kill_enemy;
use crate::network::combat::naval_weapon_volleys;
use crate::network::combat::point_hits_segment;
use crate::network::combat::point_segment_distance;
use crate::network::combat::projectile_direct_heal_percent;
use crate::network::combat::resolve_manual_aim;
use crate::network::combat::retusa_mine_shots_between;
use crate::network::combat::simulate_base_menders;
use crate::network::combat::simulate_base_turrets;
use crate::network::combat::simulate_player_combat;
use crate::network::combat::simulate_projectiles;
use crate::network::combat::simulate_turrets;
use crate::network::combat::spawn_allied_unit_projectile;
use crate::network::combat::spawn_enemy_horizon_bomb;
use crate::network::combat::spawn_enemy_projectile;
use crate::network::combat::spawn_navanax_lasers;
use crate::network::combat::unit_combat::{
    boost_properties, collect_allied_weapon_fire, collect_manual_weapon_fire,
    collision_position_passable, damaged_allied_building_target, effective_unit_build_speed,
    effective_unit_damage_multiplier, effective_unit_reload_delta, effective_unit_speed,
    invalidate_navigation_for_block, scaled_projectile_volley, spawn_allied_weapon_fire,
    spawn_weapon_fire_for_team, unit_can_shoot, unit_collision_layer, unit_hit_size,
    AlliedWeaponFire,
};
use crate::network::combat::ARKYID_SAP;
use crate::network::combat::ECLIPSE_FLAK;
use crate::network::combat::ECLIPSE_LASER;
use crate::network::combat::{
    enemy_projectile_volley, ANTUMBRA_CANNON, ANTUMBRA_MISSILE, ARKYID_ARTILLERY, RETUSA_BOLT,
    RETUSA_MINE, SCEPTER_BOLT, SCEPTER_MOUNT,
};
use crate::network::economy::building_heal_suppressed;
use crate::network::economy::building_time_scale;
use crate::network::economy::compute_power_efficiency;
use crate::network::economy::default_unit_command;
use crate::network::economy::dump_factory_output;
use crate::network::economy::has_requirements;
use crate::network::economy::liquid_capacity;
use crate::network::economy::payload::{
    carried_payload_requirements, choose_payload_router_rotation, decode_constructor_recipe,
    drop_carried_build, dump_deconstructor_items, encode_payload_dropped_frame,
    encode_picked_build_payload_frame, encode_picked_unit_payload_frame,
    insert_into_payload_conveyor, offset_position_by, payload_block_accepts, payload_block_limit,
    payload_capacity, payload_conveyor_move_time, payload_fits_limit, payload_used,
    refresh_build_payload_sync, transfer_payload_forward, valid_payload_mass_driver_link,
};
use crate::network::economy::power_role;
use crate::network::economy::projectile_position;
use crate::network::economy::simulate_base_drills;
use crate::network::economy::simulate_base_factories;
use crate::network::economy::simulate_erekir_assemblers;
use crate::network::economy::simulate_erekir_crafters;
use crate::network::economy::simulate_erekir_drills;
use crate::network::economy::simulate_erekir_ducts;
use crate::network::economy::simulate_erekir_turrets;
use crate::network::economy::simulate_factories;
use crate::network::economy::simulate_force_projectors;
use crate::network::economy::simulate_heat_network;
use crate::network::economy::simulate_liquid_factories;
use crate::network::economy::simulate_liquids;
use crate::network::economy::simulate_logistics;
use crate::network::economy::simulate_menders;
use crate::network::economy::simulate_navanax_suppression;
use crate::network::economy::simulate_oct_force_fields;
use crate::network::economy::simulate_overdrives;
use crate::network::economy::simulate_reconstructors;
use crate::network::economy::simulate_regen_projectors;
use crate::network::economy::simulate_separators;
use crate::network::economy::simulate_shock_mines;
use crate::network::economy::simulate_shockwave_towers;
use crate::network::economy::simulate_tecta_shield_arcs;
use crate::network::economy::simulate_unit_factories;
use crate::network::economy::simulate_unit_payload_entries;
use crate::network::economy::spec::{
    accept_logistics_item_from, angle_near, generator_fuel, inventory_add, inventory_count,
    inventory_remove, inventory_total, mass_driver_state, move_toward_angle, offset_position,
    storage_capacity, valid_mass_driver_link,
};
use crate::network::economy::update_power_network;
use crate::network::outbound::broadcast;
use crate::network::protocol::*;
use crate::network::runtime::save_slot_path;
use crate::network::units::controller::{controlling_player_for_unit, unit_is_player_controlled};
use crate::network::units::mining::{
    enemy_navigation_target, heal_building_for_team, heal_buildings_in_radius,
    heal_nearest_building, heal_nearest_building_flat, move_repair_unit, move_unit_toward,
    nearest_mineable_ore, unit_avoidance_requests,
};
use crate::network::units::unit_orders::{
    advance_unit_order, apply_ordered_unit_movement, boost_should_land_near_target,
    builder_unit_hit_size, clear_logic_build_order, ordered_opposing_building,
    place_logic_building, route_unit_movement, unit_build_range, unit_has_stance,
    unit_logic_building, unit_logic_firing, unit_mining,
};
use crate::network::units::{unit_bound_to_logic, ANTUMBRA, CRAWLER, HORIZON, MONO, RETUSA};
use crate::network::wire::auth::player_team;
use crate::network::wire::bootstrap::emit_game_over_packet_with_winner;
use crate::network::wire::client_snapshot::raw_mine_result;
use crate::network::wire::encode::{
    coalesce_build_health, encode_block_snapshot, encode_block_snapshots,
    encode_build_destroyed_frame, encode_build_health_update_frame, encode_unit_spawn_payload,
    frame_generated_packet, take_coalesced_build_health,
};
use crate::network::wire::persistence::{
    encode_construct_finish_for_unit, snapshot_persisted_world, PersistJob, PersistenceWorker,
};
use crate::network::wire::tile_config::broadcast_placement_power_configs;
use crate::network::wire::transfer::nearest_opposing_unit;
use crate::network::wire::{BLOCK_SNAPSHOT_INTERVAL, SLOW_CONSUMER_DROP_LIMIT};
use crate::network::world::{
    core_world, core_world_for_team, BaseBuildingState, CarriedBuildPayload, CarriedPayload,
    DynamicTile, DynamicWorld, EnemyUnit, PendingConnection, PlayerCombatState, Projectile,
    TeamCore, UnitAuthority, UnitOrder, WorldStore,
};
use crate::state::game_state::GameMode;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::network::wire::{host_map, persist_tiles, HEALTH_SYNC_INTERVAL};
pub fn simulation_delta_for_tps(tps: u32) -> f32 {
    60.0 / tps.max(1) as f32
}

/// SOL-005: authoritative wall-clock delta (game ticks) for one world-loop
/// step, mirroring the official `Logic.update`:
/// `delta = Core.graphics.getDeltaTime() * 60f` with arc capping the raw
/// frame delta at 1 s, so a single update advances the game by AT MOST 60
/// ticks. `MissedTickBehavior::Skip` only drops interval *wakeups*; the
/// elapsed span between two processed steps is still accounted for here, so
/// under load the game clock catches up with wall time instead of drifting
/// (the official server slows down but never loses accumulated game time to
/// a skipped interval). Steady state is unchanged: at the configured TPS the
/// elapsed span equals `1 / tps`, so this returns exactly
/// `simulation_delta_for_tps(tps)`.
///
/// Round 74d/74g/74h develop diagnostics: one ordered block per interval with
/// the real-time data needed to debug hard multiplayer issues — windowed
/// tick stats, wave/game state, build plans/pending backlog with details,
/// power graph health, re-host events, save latency, per-connection
/// RTT/queue/drops and player info. Run with `--develop`.
fn develop_dump(
    world: &DynamicWorld,
    connections: &DashMap<i32, PendingConnection>,
    dump_index: &mut u64,
    last_ticks: &mut u64,
    last_sum_us: &mut u64,
    window_secs: f64,
    reason: &str,
) {
    *dump_index += 1;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let ticks = world.game_state.world_ticks.load(Ordering::Relaxed);
    let sum_us = world.game_state.world_tick_us_sum.load(Ordering::Relaxed);
    let window_ticks = ticks.saturating_sub(*last_ticks).max(1);
    let window_avg_us = sum_us.saturating_sub(*last_sum_us) / window_ticks;
    *last_ticks = ticks;
    *last_sum_us = sum_us;

    let mut connection_lines: Vec<String> = connections
        .iter()
        .map(|entry| {
            let connection = entry.value();
            let name = connection
                .player_name
                .read()
                .clone()
                .unwrap_or_else(|| "?".into());
            format!(
                "#{} {} rtt={}ms silent={}ms queue={} drops={} crit={} udp={}",
                *entry.key(),
                name,
                connection.last_keepalive_rtt_ms.load(Ordering::Relaxed),
                now_ms.saturating_sub(connection.last_packet_epoch_ms.load(Ordering::Relaxed)),
                connection.outbound_queued.load(Ordering::Relaxed),
                connection.outbound_drops.load(Ordering::Relaxed),
                connection.critical_drops.load(Ordering::Relaxed),
                connection.udp_endpoint.read().is_some(),
            )
        })
        .collect();
    connection_lines.sort();
    let power_tiles = world
        .tiles
        .iter()
        .filter(|tile| crate::network::economy::power_role(tile.block).is_some())
        .count();
    let unlinked_nodes = world
        .tiles
        .iter()
        .filter(|tile| {
            crate::network::buildings::power::is_power_node(tile.block)
                && tile.power_links.is_empty()
        })
        .count();
    let pending_build_lines: Vec<String> = world
        .pending_builds
        .iter()
        .take(8)
        .map(|build| {
            let (x, y) = (build.position >> 16, build.position as i16 as i32);
            format!(
                "({x},{y})b{}#{}/{}ms",
                build.block,
                build.rotation,
                build.last_seen.elapsed().as_millis()
            )
        })
        .collect();
    let pending_break_lines: Vec<String> = world
        .pending_breaks
        .iter()
        .take(8)
        .map(|pending| {
            let (x, y) = (pending.position >> 16, pending.position as i16 as i32);
            format!(
                "({x},{y})b{}/{}ms",
                pending.block,
                pending.last_seen.elapsed().as_millis()
            )
        })
        .collect();
    let pending_ages: Vec<u128> = world
        .pending_builds
        .iter()
        .map(|build| build.last_seen.elapsed().as_millis())
        .collect();
    let oldest_build_ms = pending_ages.iter().copied().max().unwrap_or(0);
    let mode = *world.game_state.mode.read();
    let (battery_stored, battery_capacity) =
        world
            .tiles
            .iter()
            .fold((0.0f32, 0.0f32), |(stored, capacity), tile| {
                if let Some(role) = crate::network::economy::power_role(tile.block) {
                    if role.battery_capacity > 0.0 {
                        (stored + tile.power_stored, capacity + role.battery_capacity)
                    } else {
                        (stored, capacity)
                    }
                } else {
                    (stored, capacity)
                }
            });
    // Power component satisfaction histogram (O(V^2) graph walk, ~1-2 ms
    // debug with ~70 power tiles — only inside the dump cadence).
    let efficiency = crate::network::economy::compute_power_efficiency(world);
    let mut components_satisfied = 0u32;
    let mut components_partial = 0u32;
    for (_, status) in efficiency {
        if status >= 0.999 {
            components_satisfied += 1;
        } else if status > 0.0 {
            components_partial += 1;
        }
    }
    let mut enemy_histogram: Vec<String> = Vec::new();
    {
        let mut counts: std::collections::BTreeMap<i16, u32> = std::collections::BTreeMap::new();
        for enemy in world.enemies.iter() {
            *counts.entry(enemy.unit_type).or_default() += 1;
        }
        for (unit_type, count) in counts {
            enemy_histogram.push(format!("u{unit_type}x{count}"));
        }
    }
    let player_lines: Vec<String> = world
        .player_sessions
        .iter()
        .take(8)
        .map(|session| {
            format!(
                "{}@({:.0},{:.0})unit{}",
                session.name, session.x, session.y, session.unit_id
            )
        })
        .collect();
    let mono_cache = world.mono_mining_targets.len();
    let core_health = world
        .cores
        .get(&1)
        .map(|core| core.health)
        .unwrap_or(world.core_max_health);
    let core_items: i32 = world.game_state.core_items.read().iter().sum();
    let stats = world.game_state.game_stats.read();
    let ups = window_ticks as f64 / window_secs.max(0.001);
    let rss_mb = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|text| {
            let resident_pages: u64 = text.split_whitespace().nth(1)?.parse().ok()?;
            Some(resident_pages * 4096 / (1024 * 1024))
        })
        .unwrap_or(0);
    info!(
        target: "develop",
        "═══ develop dump #{} [{}] ═══",
        dump_index,
        reason
    );
    info!(
        target: "develop",
        "world     mode={:?} game_over={} paused={} strict={} ticks={} ups={:.1} tick_last_us={} tick_avg_us(win)={} tick_max_us={} save_build_us={} host_events={} rss={}mb",
        mode,
        world.game_state.game_over.load(Ordering::Relaxed),
        world.game_state.is_paused.load(Ordering::Relaxed),
        world.game_state.strict_mode.load(Ordering::Relaxed),
        ticks,
        ups,
        world.game_state.world_tick_us.load(Ordering::Relaxed),
        window_avg_us,
        world.game_state.world_tick_max_us.load(Ordering::Relaxed),
        world.game_state.save_build_us.load(Ordering::Relaxed),
        crate::network::wire::HOST_MAP_EVENTS.load(Ordering::Relaxed),
        rss_mb,
    );
    info!(
        target: "develop",
        "wave      wave={} wave_time={:.0} sim_time={:.0} core_hp={:.0} core_items={} built={} deconstructed={} enemies_killed={}",
        world.game_state.wave.load(Ordering::Relaxed),
        *world.game_state.wave_time.read(),
        *world.game_state.simulation_time.read(),
        core_health,
        core_items,
        stats.buildings_built,
        stats.buildings_deconstructed,
        stats.enemy_units_destroyed,
    );
    info!(
        target: "develop",
        "entities  tiles={} enemies={} players={} puddles={} projectiles={} logic={} mono_targets={}",
        world.tiles.len(),
        world.enemies.len(),
        world.players.len(),
        world.puddles.puddles.len(),
        world.projectiles.len(),
        world.logic_executors.len(),
        mono_cache,
    );
    if !enemy_histogram.is_empty() {
        info!(
            target: "develop",
            "enemies   {}",
            enemy_histogram.join(" ")
        );
    }
    if !player_lines.is_empty() {
        info!(
            target: "develop",
            "players   {}",
            player_lines.join(" | ")
        );
    }
    info!(
        target: "develop",
        "builds    pending={} oldest_ms={} breaks={} plans={}",
        world.pending_builds.len(),
        oldest_build_ms,
        world.pending_breaks.len(),
        world
            .team_build_plans
            .read()
            .teams
            .iter()
            .map(|team| team.plans.len())
            .sum::<usize>(),
    );
    if !pending_build_lines.is_empty() {
        info!(target: "develop", "builds>   {}", pending_build_lines.join(" "));
    }
    if !pending_break_lines.is_empty() {
        info!(target: "develop", "breaks>   {}", pending_break_lines.join(" "));
    }
    let plan_lines: Vec<String> = world
        .team_build_plans
        .read()
        .teams
        .iter()
        .flat_map(|team| {
            team.plans
                .iter()
                .map(|plan| format!("({},{})b{}#{}", plan.x, plan.y, plan.block, plan.rotation))
        })
        .take(24)
        .collect();
    if !plan_lines.is_empty() {
        let total_plans: usize = world
            .team_build_plans
            .read()
            .teams
            .iter()
            .map(|team| team.plans.len())
            .sum();
        info!(
            target: "develop",
            "plans     {} (showing {}/{})",
            plan_lines.join(" "),
            plan_lines.len().min(24),
            total_plans,
        );
    }
    info!(
        target: "develop",
        "power     tiles={} unlinked_nodes_sources={} battery={:.0}/{:.0} components_satisfied={} components_partial={}",
        power_tiles,
        unlinked_nodes,
        battery_stored,
        battery_capacity,
        components_satisfied,
        components_partial,
    );
    if !connection_lines.is_empty() {
        info!(
            target: "develop",
            "conns     {}",
            connection_lines.join(" | ")
        );
    }
}

pub fn simulation_delta_from_elapsed(elapsed: f64) -> f32 {
    if !elapsed.is_finite() || elapsed <= 0.0 {
        return 0.0;
    }
    ((elapsed * 60.0) as f32).min(60.0)
}

pub fn update_pvp_auto_pause(world: &DynamicWorld) {
    let state = &world.game_state;
    let is_pvp = *state.mode.read() == GameMode::Pvp;
    if !is_pvp || state.game_over.load(Ordering::Relaxed) {
        if state.pvp_auto_paused.swap(false, Ordering::Relaxed) && !is_pvp {
            state.is_paused.store(false, Ordering::Relaxed);
        }
        return;
    }
    if !state.pvp_auto_pause.load(Ordering::Relaxed) {
        state.pvp_auto_paused.store(false, Ordering::Relaxed);
        return;
    }
    let mut teams_with_players = HashSet::new();
    for entry in world.players.iter() {
        let team = entry.value().team;
        if team != 0 {
            teams_with_players.insert(team);
        }
    }
    let waiting = teams_with_players.len() < 2;
    let paused = state.is_paused.load(Ordering::Relaxed);
    if waiting != paused {
        if waiting {
            state.pvp_auto_paused.store(true, Ordering::Relaxed);
            state.is_paused.store(true, Ordering::Relaxed);
        } else if state.pvp_auto_paused.load(Ordering::Relaxed) {
            state.is_paused.store(false, Ordering::Relaxed);
            state.pvp_auto_paused.store(false, Ordering::Relaxed);
        }
    }
}

pub(crate) mod logic;
pub(crate) use logic::{empty_logic_program, logic_config_hash};
pub use logic::{
    simulate_logic, simulate_logic_build, simulate_logic_control_leases, simulate_logic_fire,
    simulate_logic_mining,
};
pub(crate) mod waves;
pub use waves::{
    simulate_aegires_energy_fields, simulate_enemy_point_defense, simulate_enemy_statuses,
    simulate_waves_and_enemies,
};
pub(crate) mod units;
pub(crate) use units::{
    mono_target_item, simulate_controlled_navanax_lasers, simulate_controlled_repair_beam,
};
pub use units::{
    simulate_all_unit_statuses, simulate_allied_oxynoe_repair, simulate_allied_units,
    simulate_assist_units, simulate_builder_units, simulate_controlled_unit_weapons,
    simulate_mono_mining, simulate_pvp_player_damage, simulate_support_units,
    simulate_unit_collisions, simulate_unit_elevation,
};
pub(crate) mod payloads;
pub use payloads::{
    simulate_payload_carriers, simulate_payload_constructors, simulate_payload_conveyors,
    simulate_payload_deconstructors, simulate_payload_loaders, simulate_payload_mass_drivers,
};
pub(crate) mod power;
pub use power::{
    simulate_generators, simulate_impact_reactors, simulate_mass_drivers,
    simulate_reactors_with_network,
};

pub fn spawn_world_simulation(
    store: WorldStore,
    connections: Arc<DashMap<i32, PendingConnection>>,
    tps: u32,
    admin: crate::state::administration::Administration,
) {
    // P0-8: the periodic autosave runs on a dedicated I/O worker; the tick
    // only builds the (consistent) snapshot under the world lock.
    let persistence_worker = PersistenceWorker::spawn();
    tokio::spawn(async move {
        let tps = tps.max(1);
        let mut interval =
            tokio::time::interval(std::time::Duration::from_micros(1_000_000 / u64::from(tps)));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut dirty = false;
        let mut last_save = std::time::Instant::now();
        let mut last_block_snapshot = std::time::Instant::now();
        let mut last_health_sync = std::time::Instant::now();
        let mut pending_health: std::collections::HashMap<i32, f32> =
            std::collections::HashMap::new();
        // P2: Administration lifecycle hooks — periodic autosave slots and
        // game-over -> round wait -> next map rotation.
        let mut last_autosave = std::time::Instant::now();
        // Round 74: power-node self-heal sweep (stale-link pruning +
        // autolink top-up), one node per sweep tick to keep the graph cost
        // tiny. Heals the split-graph cases where a placement path missed
        // the autolink or a deconstruction left dangling links (drills
        // losing power, sandbox sources never connecting).
        let mut last_relink_sweep = std::time::Instant::now();
        let mut relink_sweep_index = 0usize;
        // Round 74d develop diagnostics dump cadence.
        let mut last_develop_dump = std::time::Instant::now();
        let mut develop_dump_index = 0u64;
        let mut develop_last_ticks = 0u64;
        let mut develop_last_sum_us = 0u64;
        // Round 74h: last-seen outbound drop totals per connection, so a
        // NEW drop can trigger a hot-capture dump.
        let mut develop_last_drops: std::collections::HashMap<i32, u64> =
            std::collections::HashMap::new();
        let mut game_over_since: Option<f32> = None;
        // SOL-005: the world loop runs at the configured TPS (official 60).
        // The authoritative delta is derived from the wall-clock span since
        // the last PROCESSED step (simulation_delta_from_elapsed): steady
        // state equals 60/tps game ticks per step (10 TPS keeps the legacy
        // 100 ms / delta 6 behavior, 60 TPS matches the Java server), and
        // under load the clock catches up with wall time like Logic.update
        // instead of losing the skipped intervals to MissedTickBehavior::Skip.
        let mut last_step = std::time::Instant::now();
        loop {
            interval.tick().await;
            // Reload the shared world each tick so a `host` hot-swap takes
            // effect immediately and the previous world stops simulating.
            let world = store.load();
            // Rules.pvpAutoPause: pause while waiting for players of both
            // teams, resume as soon as a second team joins. Runs before the
            // is_active gate so the paused world can still be evaluated.
            update_pvp_auto_pause(&world);
            if !world.game_state.is_active() {
                // Pause/hot-swap freezes game time: reset the wall-clock
                // baseline so a resume never bursts stale elapsed time.
                last_step = std::time::Instant::now();
                continue;
            }
            let now = std::time::Instant::now();
            let delta = simulation_delta_from_elapsed(now.duration_since(last_step).as_secs_f64());
            last_step = now;
            // P2: scheduler lag — when the loop cannot keep up with the
            // target TPS the delta caps at 60 ticks; report the backlog.
            let iteration = std::time::Instant::now();
            let _guard = world.persistence_lock.lock();
            // Round 74d: rebuild the tile footprint index once per tick
            // (O(tiles)) so dynamic_at/effective_block stay O(1) in the
            // hot logistics path. The index covers every occupied position
            // of every live building.
            world.tile_footprint.clear();
            for tile in world.tiles.iter() {
                if tile.block == 0 {
                    continue;
                }
                for position in &tile.occupied {
                    world.tile_footprint.insert(*position, tile.position);
                }
            }
            dirty |= world.persistence_dirty.swap(false, Ordering::Relaxed);
            *world.game_state.simulation_time.write() += delta;
            dirty |= SandboxSystem::tick(&world, delta, |world, position, item, source| {
                accept_logistics_item_from(world, position, item, source, 0)
            });
            dirty |= simulate_generators(&world, delta);
            dirty |= simulate_reactors_with_network(&world, &connections, delta);
            let power = update_power_network(&world, delta);
            // Round 74f: 3 nodes per 100 ms instead of 1 per 250 ms — with
            // ~20 nodes the full self-heal cycle used to take 5 s ("los
            // nodos tardan mucho en linkearse"); now <1 s.
            if last_relink_sweep.elapsed() >= std::time::Duration::from_millis(100) {
                last_relink_sweep = std::time::Instant::now();
                let relink_candidates: Vec<i32> = world
                    .tiles
                    .iter()
                    .filter(|tile| crate::network::buildings::power::is_power_node(tile.block))
                    .map(|tile| *tile.key())
                    .collect();
                for _ in 0..3 {
                    if relink_candidates.is_empty() {
                        break;
                    }
                    relink_sweep_index %= relink_candidates.len();
                    let node_position = relink_candidates[relink_sweep_index];
                    if crate::network::buildings::power::relink_power_node(&world, node_position) {
                        // Round 74f: push the updated node config to every
                        // client (TileConfig broadcast) — the client only
                        // merges power graphs through the PowerNode config
                        // handler, so live autolink changes must be sent
                        // explicitly or the new link stays invisible and
                        // unpowered client-side.
                        if let Some(config) = world
                            .tiles
                            .get(&node_position)
                            .map(|tile| tile.config.clone())
                        {
                            let player_id = world
                                .player_sessions
                                .iter()
                                .next()
                                .map(|session| session.id)
                                .unwrap_or(1);
                            if let Ok(frame) = crate::network::wire::encode_tile_config_broadcast(
                                player_id,
                                node_position,
                                &config,
                            ) {
                                broadcast(&connections, frame);
                            }
                        }
                    }
                    relink_sweep_index += 1;
                }
            }
            // P0-03 / P1-B1: LogicAI lease + processor validity — the Rust
            // counterpart of the timeout block at the top of
            // `LogicAI.updateMovement` (LogicAI.java:59-64). Runs before the
            // unit movement pass so an invalid/ expired lease never moves.
            dirty |= simulate_logic_control_leases(&world, delta);
            dirty |= simulate_all_unit_statuses(&world, &connections, delta);
            // P1-06: Java `Logic.updateEntities` runs `Groups.unit.update()`
            // before `Groups.build.update()`. Allied movement therefore uses
            // last tick's `ucontrol` / CommandAI orders; this tick's logic
            // processors assign the next tick's destination.
            dirty |= simulate_allied_units(&world, &connections, delta);
            dirty |= simulate_logic(&world, &connections, delta);
            dirty |= simulate_overdrives(&world, delta, &power);
            dirty |= simulate_force_projectors(&world, delta, &power);
            dirty |= simulate_liquids(&world, delta, &power);
            // P1: authoritative puddles (evaporation/spread/reactions) —
            // classId 13 writeSync is emitted separately; this tick is the
            // amount/spread pass. P1-14 then applies status/fire.
            dirty |= world.puddles.tick(delta);
            let (puddle_fx, puddle_health, puddle_destroyed) =
                crate::network::economy::simulate_puddle_tile_effects(&world, delta);
            dirty |= puddle_fx;
            for (position, health) in puddle_health {
                coalesce_build_health(&mut pending_health, position, health);
            }
            for position in puddle_destroyed {
                if let Ok(frame) = encode_build_destroyed_frame(position) {
                    broadcast(&connections, frame);
                }
            }
            dirty |= simulate_logistics(&world, delta, &power);
            dirty |= simulate_base_drills(&world, delta);
            dirty |= simulate_base_factories(&world, delta);
            dirty |= simulate_base_turrets(&world, &connections, delta);
            dirty |= simulate_base_menders(&world, delta);
            dirty |= simulate_payload_conveyors(&world, &connections, delta);
            dirty |= simulate_payload_mass_drivers(&world, delta, &power);
            dirty |= simulate_payload_loaders(&world, delta, &power);
            dirty |= simulate_payload_deconstructors(&world, delta, &power);
            dirty |= simulate_payload_constructors(&world, delta, &power);
            dirty |= simulate_mass_drivers(&world, delta, &power);
            dirty |= simulate_factories(&world, delta, &power);
            dirty |= simulate_separators(&world, delta, &power);
            dirty |= simulate_liquid_factories(&world, delta, &power);
            // Erekir simulation (round 26): ducts, heat network, beam drills
            // and Erekir turrets.
            dirty |= simulate_erekir_ducts(&world, delta, &power);
            dirty |= simulate_heat_network(&world, delta, &power);
            dirty |= simulate_erekir_drills(&world, delta, &power);
            dirty |= simulate_erekir_turrets(&world, &connections, delta, &power);
            dirty |= simulate_erekir_assemblers(&world, &connections, delta, &power);
            dirty |= simulate_erekir_crafters(&world, delta, &power);
            dirty |= simulate_unit_factories(&world, &connections, delta, &power);
            dirty |= simulate_unit_payload_entries(&world, &connections, delta);
            dirty |= simulate_reconstructors(&world, &connections, delta, &power);
            dirty |= simulate_menders(&world, &connections, delta, &power);
            dirty |= simulate_regen_projectors(&world, &connections, delta, &power);
            dirty |= simulate_turrets(&world, &connections, delta, &power);
            dirty |= simulate_shockwave_towers(&world, delta, &power);
            dirty |= simulate_controlled_unit_weapons(&world, &connections, delta);
            let (enemy_dirty, destroyed_buildings, health_updates) =
                simulate_waves_and_enemies(&world, &connections, delta);
            dirty |= enemy_dirty;
            // P1-B2: Java runs `Groups.bullet.update/collide` after all unit
            // updates; bullet-applied statuses must not affect movement/weapons
            // until the following tick's StatusComp pass.
            dirty |= simulate_pvp_player_damage(&world, &connections);
            dirty |= simulate_projectiles(&world, &connections, delta);
            dirty |= simulate_player_combat(&world, &connections, delta);
            dirty |= simulate_shock_mines(&world, &connections, delta);
            dirty |= simulate_unit_elevation(&world, delta);
            dirty |= simulate_payload_carriers(&world, &connections, delta);
            dirty |= simulate_assist_units(&world, delta);
            dirty |= simulate_builder_units(&world, &connections, delta);
            dirty |= simulate_constructions(&world, &connections, delta);
            dirty |= simulate_breaks(&world, &connections, delta);
            dirty |= simulate_support_units(&world, &connections, delta);
            dirty |= simulate_unit_collisions(&world);
            if !health_updates.is_empty() {
                for (position, health) in health_updates {
                    coalesce_build_health(&mut pending_health, position, health);
                }
            }
            if last_health_sync.elapsed() >= HEALTH_SYNC_INTERVAL {
                let updates = take_coalesced_build_health(&mut pending_health);
                if !updates.is_empty() {
                    if let Ok(frame) = encode_build_health_update_frame(&updates) {
                        broadcast(&connections, frame);
                    }
                }
                last_health_sync = std::time::Instant::now();
            }
            for position in destroyed_buildings {
                if let Ok(frame) = encode_build_destroyed_frame(position) {
                    broadcast(&connections, frame);
                }
            }
            if last_block_snapshot.elapsed() >= BLOCK_SNAPSHOT_INTERVAL {
                if let Ok(frames) = encode_block_snapshots(&world, &power) {
                    for frame in frames {
                        broadcast(&connections, frame);
                    }
                }
                last_block_snapshot = std::time::Instant::now();
            }
            // P0-9: slow-consumer teardown. A connection whose bounded
            // outbound queue has dropped more frames than it can ever drain
            // is removed from the registry; dropping the sender makes the
            // connection task's `outbound.recv()` return None and the
            // standard teardown path (session removal, player broadcast)
            // runs. Critical drops (kicks/restreams) teardown much sooner.
            let slow: Vec<i32> = connections
                .iter()
                .filter(|entry| {
                    entry.value().outbound_drops.load(Ordering::Relaxed) >= SLOW_CONSUMER_DROP_LIMIT
                        || entry.value().critical_drops.load(Ordering::Relaxed) >= 16
                })
                .map(|entry| *entry.key())
                .collect();
            for id in slow {
                if let Some(connection) = connections.get(&id) {
                    warn!(
                        "Disconnecting slow consumer {} ({} dropped frames, {} critical)",
                        connection.ip,
                        connection.outbound_drops.load(Ordering::Relaxed),
                        connection.critical_drops.load(Ordering::Relaxed)
                    );
                }
                connections.remove(&id);
            }
            if dirty && last_save.elapsed() >= std::time::Duration::from_secs(1) {
                // P0-8: build the snapshot on the tick (consistent under the
                // world lock) and hand the I/O to the worker thread. If the
                // worker is gone (should not happen), fall back to a
                // synchronous durable save so state is never lost silently.
                let save_build_start = std::time::Instant::now();
                let saved = snapshot_persisted_world(
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
                );
                world.game_state.save_build_us.store(
                    save_build_start.elapsed().as_micros() as u64,
                    Ordering::Relaxed,
                );
                let submitted = persistence_worker.submit(PersistJob {
                    path: world.save_path.clone(),
                    world: saved,
                });
                if !submitted {
                    if let Err(err) = persist_tiles(
                        &world.save_path,
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
                    ) {
                        warn!("Could not persist simulated world state: {}", err);
                    }
                }
                dirty = false;
                last_save = std::time::Instant::now();
            }
            // P2: periodic autosave into a named slot (official
            // `Config.autosaveSpacing` lifecycle). The main save keeps the
            // live state; the autosave slot is a durable rotation point.
            if admin.autosave_due(last_autosave.elapsed().as_secs_f64()) {
                if let Ok(path) = save_slot_path(&world.save_path, "autosave1") {
                    let saved = snapshot_persisted_world(
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
                    );
                    if persistence_worker.submit(PersistJob {
                        path: path.clone(),
                        world: saved,
                    }) {
                        debug!("Autosave written to {}", path.display());
                    }
                }
                last_autosave = std::time::Instant::now();
            }
            // P2: game-over lifecycle — wait `round_wait_ticks` (official
            // `Config.roundExtraTime` = 12 s) and then advance to the next
            // map in the rotation (official ServerControl.gameOverListener
            // -> nextMap). The rotation list is the Administration model.
            if world.game_state.game_over.load(Ordering::Relaxed)
                && *world.game_state.mode.read() != GameMode::Sandbox
            {
                let tick = *world.game_state.simulation_time.read();
                if game_over_since.is_none() {
                    game_over_since = Some(tick);
                    info!(
                        "Game over: next map in {} game ticks",
                        admin.round_wait_ticks()
                    );
                }
                let waiting = tick - game_over_since.unwrap_or(tick);
                if waiting >= admin.round_wait_ticks() as f32 {
                    let mode = format!("{:?}", *world.game_state.mode.read()).to_lowercase();
                    match admin.advance_map() {
                        Some(next) => {
                            info!("Rotating to next map '{}'", next);
                            if let Err(err) =
                                host_map(&store, &connections, &next, &mode, Some(&admin))
                            {
                                warn!("Could not rotate to map '{}': {}", next, err);
                            }
                            // The new world resets the game-over state; the
                            // loop reloads it from the store next tick.
                            game_over_since = None;
                        }
                        None => {
                            debug!("No map rotation configured; game stays over");
                        }
                    }
                }
            } else {
                game_over_since = None;
            }
            // P2: record world-loop metrics (duration, max, tick count and
            // aggregate dropped outbound frames).
            world.game_state.world_ticks.fetch_add(1, Ordering::Relaxed);
            let elapsed_us = iteration.elapsed().as_micros() as u64;
            world
                .game_state
                .world_tick_us
                .store(elapsed_us, Ordering::Relaxed);
            world
                .game_state
                .world_tick_us_sum
                .fetch_add(elapsed_us, Ordering::Relaxed);
            world
                .game_state
                .world_tick_max_us
                .fetch_max(elapsed_us, Ordering::Relaxed);
            let dropped: u64 = connections
                .iter()
                .map(|connection| connection.value().outbound_drops.load(Ordering::Relaxed))
                .sum();
            world
                .game_state
                .dropped_frames_total
                .store(dropped, Ordering::Relaxed);
            // Round 74d/74h: develop diagnostics dump — periodic by interval,
            // plus AUTOMATIC hot captures on irregularities (tick stalls,
            // ping spikes, client silence, new outbound drops, stuck builds)
            // so the state is captured in the moment instead of at the next
            // interval boundary. Auto dumps are rate-limited to one per 2 s.
            if world.game_state.develop_mode.load(Ordering::Relaxed) {
                let interval = world
                    .game_state
                    .develop_interval_ms
                    .load(Ordering::Relaxed)
                    .max(100);
                let since_dump = last_develop_dump.elapsed();
                let periodic_due = since_dump >= std::time::Duration::from_millis(interval);
                let auto_allowed = since_dump >= std::time::Duration::from_millis(2000);
                let mut auto_reason: Option<String> = None;
                let tick_us = world.game_state.world_tick_us.load(Ordering::Relaxed);
                if tick_us > 50_000 {
                    auto_reason = Some(format!("tick_stall_{}ms", tick_us / 1000));
                }
                if auto_reason.is_none() {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis() as u64)
                        .unwrap_or(0);
                    for entry in connections.iter() {
                        let connection = entry.value();
                        let rtt = connection.last_keepalive_rtt_ms.load(Ordering::Relaxed);
                        if rtt > 60 {
                            auto_reason =
                                Some(format!("rtt_spike_conn#{}_{}ms", *entry.key(), rtt));
                            break;
                        }
                        let silent = now_ms.saturating_sub(
                            connection.last_packet_epoch_ms.load(Ordering::Relaxed),
                        );
                        if silent > 10_000 {
                            auto_reason = Some(format!("client_silent_conn#{}", *entry.key()));
                            break;
                        }
                        let drops = connection.outbound_drops.load(Ordering::Relaxed);
                        if drops > develop_last_drops.get(entry.key()).copied().unwrap_or(0) {
                            auto_reason = Some(format!("drops_conn#{}", *entry.key()));
                            break;
                        }
                    }
                }
                if auto_reason.is_none() {
                    if let Some(age_ms) = world
                        .pending_builds
                        .iter()
                        .map(|build| build.last_seen.elapsed().as_millis())
                        .max()
                    {
                        if age_ms > 10_000 {
                            auto_reason = Some(format!("build_stuck_{}ms", age_ms));
                        }
                    }
                }
                if periodic_due || (auto_reason.is_some() && auto_allowed) {
                    let window_secs = since_dump.as_secs_f64();
                    last_develop_dump = std::time::Instant::now();
                    for entry in connections.iter() {
                        develop_last_drops.insert(
                            *entry.key(),
                            entry.value().outbound_drops.load(Ordering::Relaxed),
                        );
                    }
                    let reason = if periodic_due {
                        "interval".to_string()
                    } else {
                        auto_reason.unwrap_or_else(|| "auto".into())
                    };
                    develop_dump(
                        &world,
                        &connections,
                        &mut develop_dump_index,
                        &mut develop_last_ticks,
                        &mut develop_last_sum_us,
                        window_secs,
                        &reason,
                    );
                }
            }
        }
    });
}

#[derive(Clone, Copy)]
enum AegiresFieldTarget {
    Unit(i32, bool),
    Player(i32),
    Building(i32, bool),
    Core,
}
