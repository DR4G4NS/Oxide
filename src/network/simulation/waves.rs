//! Enemy/wave status + attack passes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::dynamic_tile_health;
use crate::network::buildings::snapshot::*;
use crate::network::combat::enemy::{
    apply_enemy_support_abilities, damage_building, enemy_circle_radius, enemy_max_health,
    hostile_unit_count, move_enemy_in_attack_orbit, spawn_wave, tether_follow_targets,
};
use crate::network::combat::unit_combat::{
    effective_unit_damage_multiplier, effective_unit_reload_delta, effective_unit_speed,
    scaled_projectile_volley,
};
use crate::network::combat::*;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::mining::{
    enemy_navigation_target, heal_building_for_team, unit_avoidance_requests,
};
use crate::network::units::*;
use crate::network::wire::bootstrap::emit_game_over_packet_with_winner;
use crate::network::wire::encode::encode_build_health_update_frame;
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;
use tracing::{debug, error, info, warn};

use super::*;

pub fn simulate_aegires_energy_fields(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let fields: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 33)
        .map(|unit| (unit.id, unit.team, unit.x, unit.y))
        .collect();
    let mut changed = false;
    for (source_id, team, x, y) in fields {
        let activations = {
            let Some(mut source) = world.enemies.get_mut(&source_id) else {
                continue;
            };
            source.tertiary_attack_reload += delta_ticks.max(0.0);
            let activations = (source.tertiary_attack_reload / 65.0).floor() as usize;
            source.tertiary_attack_reload %= 65.0;
            activations
        };
        for _ in 0..activations {
            let unit_targets = world.enemies.iter().filter_map(|unit| {
                if unit.id == source_id {
                    return None;
                }
                let distance = (unit.x - x).hypot(unit.y - y);
                let allied = unit.team == team;
                let maximum = enemy_max_health(&unit);
                (distance <= 180.0 && (!allied || unit.health < maximum))
                    .then_some((distance, AegiresFieldTarget::Unit(unit.id, allied)))
            });
            let player_targets = world.players.iter().filter_map(|player| {
                let distance = (player.x - x).hypot(player.y - y);
                (team != 1 && !player.dead && distance <= 180.0)
                    .then_some((distance, AegiresFieldTarget::Player(*player.key())))
            });
            let mut seen = HashSet::new();
            let dynamic_targets = world
                .tiles
                .iter()
                .filter_map(|tile| {
                    if tile.block == 0
                        || tile.position == world.core_position
                        || !seen.insert(tile.position)
                    {
                        return None;
                    }
                    let allied = tile.team == team;
                    let maximum = crate::game::content::block_health(tile.block);
                    let health = dynamic_tile_health(&tile);
                    let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
                    let target_y = tile.position as i16 as f32 * 8.0;
                    let distance = (target_x - x).hypot(target_y - y);
                    (distance <= 180.0
                        && (!allied || health < maximum)
                        && (!allied || !building_heal_suppressed(world, tile.position, tile.block)))
                    .then_some((
                        distance,
                        AegiresFieldTarget::Building(tile.position, allied),
                    ))
                })
                .collect::<Vec<_>>();
            let base_targets = world
                .base_buildings
                .iter()
                .filter_map(|building| {
                    if building.position == world.core_position || !seen.insert(building.position) {
                        return None;
                    }
                    let allied = building.team == team;
                    let maximum = crate::game::content::block_health(building.block);
                    let target_x = (building.position >> 16) as i16 as f32 * 8.0;
                    let target_y = building.position as i16 as f32 * 8.0;
                    let distance = (target_x - x).hypot(target_y - y);
                    (distance <= 180.0
                        && (!allied || building.health < maximum)
                        && (!allied
                            || !building_heal_suppressed(world, building.position, building.block)))
                    .then_some((
                        distance,
                        AegiresFieldTarget::Building(building.position, allied),
                    ))
                })
                .collect::<Vec<_>>();
            let core_target = (team != 1
                && (core_world(world).0 - x).hypot(core_world(world).1 - y) <= 180.0)
                .then_some((
                    (core_world(world).0 - x).hypot(core_world(world).1 - y),
                    AegiresFieldTarget::Core,
                ));
            let mut targets: Vec<_> = unit_targets
                .chain(player_targets)
                .chain(dynamic_targets)
                .chain(base_targets)
                .chain(core_target)
                .collect();
            targets.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
            for (_, target) in targets.into_iter().take(25) {
                changed |= match target {
                    AegiresFieldTarget::Unit(id, true) => {
                        if let Some(mut ally) = world.enemies.get_mut(&id) {
                            let maximum = enemy_max_health(&ally);
                            let multiplier = if ally.unit_type == 33 { 0.5 } else { 1.0 };
                            let previous = ally.health;
                            ally.health = (ally.health + maximum * 0.015 * multiplier).min(maximum);
                            ally.health > previous
                        } else {
                            false
                        }
                    }
                    AegiresFieldTarget::Unit(id, false) => {
                        if let Some(mut target) = world.enemies.get_mut(&id) {
                            let dealt =
                                apply_incoming_unit_damage_in_world(world, &target, 40.0, 1.0);
                            target.health = (target.health - dealt).max(0.0);
                            let emd_duration = target
                                .statuses
                                .iter()
                                .find(|entry| entry.effect == 10)
                                .map(|entry| entry.time.max(360.0))
                                .unwrap_or(360.0);
                            crate::network::units::StatusContainer::apply_status(
                                &mut *target,
                                10,
                                emd_duration,
                            );
                            let dead = target.health <= 0.0;
                            drop(target);
                            if dead {
                                kill_enemy(world, out, id);
                            }
                            true
                        } else {
                            false
                        }
                    }
                    AegiresFieldTarget::Player(id) => {
                        damage_player(world, out, id, 40.0, 10, 360.0)
                    }
                    AegiresFieldTarget::Building(position, true) => {
                        if let Some(health) =
                            heal_building_for_team(world, position, team, 1.5, 0.0)
                        {
                            if let Ok(frame) =
                                encode_build_health_update_frame(&[(position, health)])
                            {
                                out.broadcast(frame);
                            }
                            true
                        } else {
                            false
                        }
                    }
                    AegiresFieldTarget::Building(position, false) => {
                        apply_enemy_direct_damage(world, out, Some(position), false, 40.0)
                    }
                    AegiresFieldTarget::Core => {
                        apply_enemy_direct_damage(world, out, None, true, 40.0)
                    }
                };
            }
        }
    }
    changed
}

pub fn simulate_enemy_point_defense(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let defenders: Vec<_> = world
        .enemies
        .iter()
        .filter_map(|unit| {
            let (reload, range, damage, mounts) = match unit.unit_type {
                31 => (9.0, 100.0, 17.0, 1),
                33 => (4.0, 180.0, 30.0, 2),
                _ => return None,
            };
            Some((unit.id, reload, range, damage, mounts))
        })
        .collect();
    let mut changed = false;
    for (defender_id, reload, range, damage, mounts) in defenders {
        let Some(mut defender) = world.enemies.get_mut(&defender_id) else {
            continue;
        };
        defender.secondary_attack_reload += effective_unit_reload_delta(&defender, delta_ticks);
        let damage = damage * effective_unit_damage_multiplier(&defender);
        while defender.secondary_attack_reload >= reload {
            let mut fired = false;
            for _ in 0..mounts {
                let target = world
                    .projectiles
                    .iter()
                    .filter(|projectile| projectile.team != defender.team)
                    .filter_map(|projectile| {
                        let (x, y) = projectile_position(&projectile);
                        let distance = (x - defender.x).hypot(y - defender.y);
                        (distance <= range).then_some((distance, *projectile.key()))
                    })
                    .min_by(|left, right| left.0.total_cmp(&right.0))
                    .map(|(_, id)| id);
                let Some(target) = target else {
                    break;
                };
                let remove = world
                    .projectiles
                    .get_mut(&target)
                    .is_some_and(|mut projectile| {
                        if projectile.damage > damage {
                            projectile.damage -= damage;
                            false
                        } else {
                            true
                        }
                    });
                if remove {
                    world.projectiles.remove(&target);
                }
                fired = true;
                changed = true;
            }
            if !fired {
                break;
            }
            defender.secondary_attack_reload -= reload;
        }
    }
    changed
}

pub fn simulate_enemy_statuses(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let mut dead = Vec::new();
    let mut changed = false;
    for mut unit in world.enemies.iter_mut() {
        // P1: the StatusEntry collection is authoritative; burning (1)
        // damage is applied per entry for the elapsed part of this tick.
        let elapsed = delta_ticks.max(0.0);
        let _ = elapsed;
        changed |=
            crate::network::units::tick_unit_statuses_with_floor(&mut unit, world, delta_ticks);
        if unit.health <= 0.0 {
            dead.push(unit.id);
        }
    }
    for id in dead {
        kill_enemy(world, out, id);
    }
    changed
}

/// P1-B2: official `StatusComp.update` for every live unit before movement
/// and weapons (`Groups.unit.update` precedes `Groups.bullet.collide`).
pub fn simulate_waves_and_enemies(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> (bool, Vec<i32>, Vec<(i32, f32)>) {
    if !world.game_state.is_active() || world.game_state.game_over.load(Ordering::Relaxed) {
        return (false, Vec::new(), Vec::new());
    }

    // ASTRA W01: flags, not GameMode, decide the clock and the spawn. Sandbox
    // still simulates units; `waveTimer` is the only auto-countdown gate.
    // Manual `waves` / AdminRequest set wavetime to 0 and must fire one wave
    // when `waves` is on, even with `waveTimer=false`.
    let (waves_enabled, wave_timer, wait_enemies, wave_spacing, win_wave, can_game_over) = {
        let rules = world.wave_rules.read();
        (
            rules.waves_enabled,
            rules.wave_timer,
            rules.wait_enemies,
            rules.wave_spacing,
            rules.win_wave,
            rules.can_game_over,
        )
    };
    let hostiles = hostile_unit_count(world);
    let wave = world.game_state.wave.load(Ordering::Relaxed);
    let freeze_clock = hostiles > 0 && (wait_enemies || (win_wave > 0 && wave >= win_wave as u32));
    let should_spawn = waves_enabled && {
        if wave_timer {
            if freeze_clock {
                false
            } else {
                let mut time = world.game_state.wave_time.write();
                *time = (*time - delta_ticks).max(0.0);
                *time <= 0.0
            }
        } else {
            *world.game_state.wave_time.read() <= 0.0
        }
    };
    if should_spawn {
        spawn_wave(world, out);
        *world.game_state.wave_time.write() = wave_spacing;
    }

    // ASTRA W06: wave-count victory is inside canGameOver, requires waves,
    // waits until WaveSpawner.isSpawning() (121 ticks) is false, and uses
    // isEnemy() counts — not every unit on waveTeam.
    let attack_mode = world.wave_rules.read().attack_mode;
    let spawning = (*world.game_state.simulation_time.read() as u32)
        < world
            .game_state
            .extras
            .spawner_until
            .load(Ordering::Relaxed);
    if can_game_over
        && waves_enabled
        && !attack_mode
        && win_wave > 0
        && wave >= win_wave as u32
        && hostile_unit_count(world) == 0
        && !spawning
    {
        world.game_state.game_over.store(true, Ordering::Relaxed);
        emit_game_over_packet_with_winner(world, out, world.wave_rules.read().default_team);
    }

    // attackMode (Pvp/Attack): last-team-standing wins; a team loses when its
    // cores are all destroyed (core-destruction paths already fire game over).
    if attack_mode {
        let mut alive: Vec<u8> = Vec::new();
        if crate::network::world::core_health_for_team(world, 1) > 0.0
            || world.players.iter().any(|p| p.team == 1 && !p.dead)
        {
            alive.push(1);
        }
        let enemy_teams: std::collections::HashSet<u8> =
            world.enemies.iter().map(|e| e.team).collect();
        for team in enemy_teams {
            alive.push(team);
        }
        alive.sort_unstable();
        alive.dedup();
        if alive.len() == 1 && !world.game_state.game_over.load(Ordering::Relaxed) {
            world.game_state.game_over.store(true, Ordering::Relaxed);
            emit_game_over_packet_with_winner(world, out, alive[0]);
        }
    }

    let (target_x, target_y) = core_world_for_team(world, world.wave_rules.read().default_team);
    apply_enemy_support_abilities(world, out, delta_ticks);
    simulate_aegires_energy_fields(world, out, delta_ticks);
    simulate_navanax_suppression(world, delta_ticks);
    simulate_oct_force_fields(world, delta_ticks);
    simulate_tecta_shield_arcs(world, delta_ticks);
    simulate_enemy_point_defense(world, delta_ticks);
    let mut core_damage = 0.0;
    let mut exploded = Vec::new();
    let mut destroyed_buildings = HashSet::new();
    let mut health_updates = HashMap::new();
    // TimedKillUnit self-expiry (all MissileUnitType content):
    // `TimedKillUnit.updateTile` ages `time -= Time.delta` and kills the unit
    // at zero. Snapshot first, drop the guard, then kill outside any
    // iteration (DashMap DM rule).
    let mut expired_missiles = Vec::new();
    let mut aged_missiles = Vec::new();
    for entry in world.enemies.iter() {
        if entry.entity_class == 39 && entry.missile_time > 0.0 {
            let remaining = entry.missile_time - delta_ticks;
            if remaining <= 0.0 {
                expired_missiles.push(*entry.key());
            } else {
                aged_missiles.push((*entry.key(), remaining));
            }
        }
    }
    for id in &expired_missiles {
        crate::network::combat::kill_enemy(world, out, *id);
    }
    for (id, remaining) in &aged_missiles {
        if let Some(mut missile) = world.enemies.get_mut(id) {
            missile.missile_time = *remaining;
        }
    }
    // RtsAI.assignSquads/handleSquad (RtsAI.java): same-type units within
    // squadRadius (60 + hitSize*1.5) form a squad and share ONE target -
    // the team-1 building nearest the squad centroid. Snapshot first; no
    // guards held while probing tiles (DashMap DM rule).
    // RtsAI.assignSquads runs on a 60*2-tick timer, not every tick; cache
    // the assignments between recomputes (the O(units^2) flood fill is the
    // dominant per-tick cost otherwise).
    type SquadTargets = HashMap<i32, (i32, f32, f32)>;
    thread_local! {
        static SQUAD_CACHE: std::cell::RefCell<Option<(u64, SquadTargets)>> =
            const { std::cell::RefCell::new(None) };
    }
    let rts_ai = {
        let rules = world.wave_rules.read();
        rules.team_rule(rules.wave_team).rts_ai
    };
    let squad_targets = if rts_ai {
        SQUAD_CACHE.with(|cache| {
            let bucket = world
                .game_state
                .world_ticks
                .load(std::sync::atomic::Ordering::Relaxed)
                / 120;
            let mut cache = cache.borrow_mut();
            if let Some((cached_bucket, targets)) = cache.as_ref() {
                if *cached_bucket == bucket {
                    return targets.clone();
                }
            }
            let targets = squad_target_assignments(world);
            *cache = Some((bucket, targets.clone()));
            targets
        })
    } else {
        HashMap::new()
    };
    // Snapshot before acquiring DashMap shard write guards; iterating the map
    // from path selection while holding `iter_mut()` can deadlock on a shard.
    let avoidance = unit_avoidance_requests(world);
    // FlyingFollowAI / HugAI stand-in targets are precomputed read-only so
    // the write pass below never holds an enemies guard across an enemies
    // iteration (DashMap DM rule).
    let tether_targets = tether_follow_targets(world);
    // ASTRA E02: snapshot hostile bodies before `enemies.iter_mut()`. Include
    // factory units, not only PlayerCombatState avatars.
    let hostile_targets: Vec<(i32, u8, f32, f32, f32, f32, bool)> = {
        let possessing: HashSet<i32> = world
            .player_sessions
            .iter()
            .filter(|session| session.controlled_unit.standard_id().is_some())
            .map(|session| session.unit_id)
            .collect();
        let mut targets = Vec::new();
        for player in world.players.iter() {
            if player.dead || possessing.contains(&player.unit_id) {
                continue;
            }
            let (vx, vy) = world
                .player_sessions
                .get(&player.unit_id)
                .map(|session| (session.velocity_x, session.velocity_y))
                .unwrap_or((0.0, 0.0));
            targets.push((
                player.unit_id,
                player.team,
                player.x,
                player.y,
                vx,
                vy,
                true, // Core ships are airborne; possessed bodies are listed below.
            ));
        }
        for unit in world.enemies.iter() {
            let flying = unit.elevation >= 0.09
                || crate::game::content::unit_movement(unit.unit_type).flying;
            targets.push((
                unit.id,
                unit.team,
                unit.x,
                unit.y,
                unit.velocity_x,
                unit.velocity_y,
                flying,
            ));
        }
        targets
    };
    let wave_units: Vec<_> = world.enemies.iter().map(|unit| unit.clone()).collect();
    for mut enemy in wave_units {
        if matches!(
            enemy.authority,
            crate::network::world::UnitAuthority::Player { .. }
                | crate::network::world::UnitAuthority::Logic { .. }
        ) {
            continue;
        }
        if crate::network::units::controller::unit_is_player_controlled(world, enemy.id) {
            continue;
        }
        // CommandAI units (any non-AI team, or an AI team with rtsAi) belong
        // on the allied path, not GroundAI (ASTRA E01).
        if crate::network::units::unit_uses_command_ai(Some(world), &enemy) {
            continue;
        }
        let navigation = enemy_navigation_target(world, &enemy, target_x, target_y, &avoidance);
        // Squad members focus the shared squad target when their own path
        // has not surfaced a building yet (RtsAI.handleSquad).
        let building_target = navigation
            .building
            .or_else(|| squad_targets.get(&enemy.id).copied());
        // quell/disrupt (FlyingFollowAI) and renale/latum (HugAI) shadow the
        // nearest friendly large unit instead of pushing the core path alone;
        // attacks and abilities below stay untouched.
        let mut movement_target = navigation.movement;
        if let Some((_, bx, by)) = building_target {
            // Pathfinder PositionTarget (audit H13): walk the destination
            // field toward the squad/engaged building instead of the core
            // field, so concave walls do not stall the order. Close in, RtsAI
            // spreads the squad on a ring (audit H12 formation offsets).
            let range = enemy.attack_range.max(8.0);
            let dist = (bx - enemy.x).hypot(by - enemy.y);
            let slot = (enemy.id as u32 % 8) as f32 / 8.0 * std::f32::consts::TAU;
            let radius = 12.0 + crate::network::combat::unit_hit_size(enemy.unit_type) * 0.5;
            let form_x = bx + slot.cos() * radius;
            let form_y = by + slot.sin() * radius;
            movement_target = if enemy.unit_type == CRAWLER.unit_type || dist > range + 16.0 {
                crate::network::units::mining::first_flow_step_toward(
                    world, &enemy, bx, by, &avoidance,
                )
            } else {
                (form_x, form_y)
            };
        }
        if let Some((tether_x, tether_y)) = tether_targets.get(&enemy.id) {
            movement_target = (*tether_x, *tether_y);
        }
        // Audit H12: units in range outrank buildings for AIMING; melee
        // crawlers keep their kamikaze building rush.
        let unit_target = if enemy.unit_type != CRAWLER.unit_type {
            nearest_hostile_from_snapshot(
                &hostile_targets,
                enemy.id,
                enemy.x,
                enemy.y,
                enemy.team,
                enemy.attack_range,
                enemy.unit_type,
            )
        } else {
            None
        };
        let volley_speed = enemy_projectile_volley(enemy.unit_type)
            .map(|volley| volley.speed)
            .unwrap_or(3.0);
        let (aim_x, aim_y) = match unit_target {
            Some((_, px, py, vx, vy)) => {
                intercept_aim(enemy.x, enemy.y, px, py, vx, vy, volley_speed)
            }
            None => building_target
                .map(|(_, x, y)| (x, y))
                .unwrap_or((target_x, target_y)),
        };
        let dx = aim_x - enemy.x;
        let dy = aim_y - enemy.y;
        let distance = if let Some((_, px, py, _, _)) = unit_target {
            (px - enemy.x).hypot(py - enemy.y)
        } else {
            dx.hypot(dy)
        };
        let reload_delta = effective_unit_reload_delta(&enemy, delta_ticks);
        let damage_multiplier = effective_unit_damage_multiplier(&enemy);
        let shooting = distance <= enemy.attack_range && unit_can_shoot(&enemy);
        if !shooting {
            // An idle or disabled weapon becomes ready, but cannot bank volleys.
            crate::network::combat::unit_combat::accumulate_wave_weapon_timers(
                &mut enemy,
                delta_ticks,
            );
        } else if enemy.unit_type != CRAWLER.unit_type {
            enemy.attack_reload += reload_delta;
        }
        if shooting {
            if enemy.unit_type == CRAWLER.unit_type {
                if let Some((position, _, _)) = building_target {
                    // dashmap-guard: allow DM900 reason="damage_building operates on world.tiles, while this live guard belongs to world.enemies"
                    if let Some((destroyed, health)) =
                        damage_building(world, position, enemy.attack_damage * damage_multiplier)
                    {
                        if destroyed {
                            destroyed_buildings.insert(position);
                            health_updates.remove(&position);
                        } else {
                            health_updates.insert(position, health);
                        }
                    }
                } else {
                    core_damage += enemy.attack_damage * damage_multiplier;
                }
                exploded.push(enemy.id);
                continue;
            }
            if enemy.unit_type == ANTUMBRA.unit_type {
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                while enemy.attack_reload >= 40.0 {
                    enemy.attack_reload -= 40.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(ANTUMBRA_MISSILE, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.secondary_attack_reload >= 70.0 {
                    enemy.secondary_attack_reload -= 70.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(ANTUMBRA_MISSILE, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.tertiary_attack_reload >= 24.0 {
                    enemy.tertiary_attack_reload -= 24.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(ANTUMBRA_CANNON, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
            } else if enemy.unit_type == 34 {
                while enemy.attack_reload >= 130.0 {
                    enemy.attack_reload -= 130.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(
                            enemy_projectile_volley(34).unwrap(),
                            damage_multiplier,
                        ),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                enemy.secondary_attack_reload += reload_delta;
                while enemy.secondary_attack_reload >= 170.0 && distance <= 90.0 {
                    enemy.secondary_attack_reload -= 170.0;
                    spawn_navanax_lasers(
                        world,
                        out,
                        2,
                        effective_unit_damage_multiplier(&enemy),
                        enemy.id,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
            } else if enemy.unit_type == RETUSA.unit_type {
                while enemy.attack_reload >= 44.0 {
                    enemy.attack_reload -= 44.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(RETUSA_BOLT, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                let previous = enemy.tertiary_attack_reload.max(0.0);
                let current = previous + reload_delta;
                let mine_shots = retusa_mine_shots_between(previous, current);
                enemy.tertiary_attack_reload = current;
                enemy.secondary_attack_reload = current % 90.0;
                for _ in 0..mine_shots {
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(RETUSA_MINE, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        volley_mount_lateral(RETUSA_MINE, 0),
                        0,
                    );
                }
            } else if enemy.unit_type == 3 {
                // Scepter: scepter-weapon (45, 3-shot burst) + 2 scepter-mounts
                // (12 / 15). Shot delay of the burst is not modeled.
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                while enemy.attack_reload >= 90.0 {
                    enemy.attack_reload -= 90.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(SCEPTER_BOLT, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.secondary_attack_reload >= 24.0 {
                    enemy.secondary_attack_reload -= 24.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(SCEPTER_MOUNT, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.tertiary_attack_reload >= 30.0 {
                    enemy.tertiary_attack_reload -= 30.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(SCEPTER_MOUNT, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
            } else if enemy.unit_type == 13 {
                // Arkyid: large-purple-mount (45) + 3 spiroct-weapons
                // (9 / 14 / 22); the third sap timer uses quaternary_attack_reload.
                // Each sap timer fires its own mount (Weapon.x 4 / 9 / 14,
                // each mirrored), so the shared SAP const gets the per-group
                // lateral offset.
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                enemy.quaternary_attack_reload += reload_delta;
                while enemy.attack_reload >= 90.0 {
                    enemy.attack_reload -= 90.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(ARKYID_ARTILLERY, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.secondary_attack_reload >= 18.0 {
                    enemy.secondary_attack_reload -= 18.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(
                            volley_with_mount_offset(ARKYID_SAP, 4.0),
                            damage_multiplier,
                        ),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.tertiary_attack_reload >= 28.0 {
                    enemy.tertiary_attack_reload -= 28.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(
                            volley_with_mount_offset(ARKYID_SAP, 9.0),
                            damage_multiplier,
                        ),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.quaternary_attack_reload >= 44.0 {
                    enemy.quaternary_attack_reload -= 44.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(
                            volley_with_mount_offset(ARKYID_SAP, 14.0),
                            damage_multiplier,
                        ),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
            } else if enemy.unit_type == 19 {
                // Eclipse: large-laser-mount (45) + 2 large-artillery (9 / 12).
                // Each artillery timer fires its own mount (Weapon.x 11 / 20,
                // each mirrored), so the shared FLAK const gets the per-group
                // lateral offset.
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                while enemy.attack_reload >= 90.0 {
                    enemy.attack_reload -= 90.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(ECLIPSE_LASER, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.secondary_attack_reload >= 18.0 {
                    enemy.secondary_attack_reload -= 18.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(
                            volley_with_mount_offset(ECLIPSE_FLAK, 11.0),
                            damage_multiplier,
                        ),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.tertiary_attack_reload >= 24.0 {
                    enemy.tertiary_attack_reload -= 24.0;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(
                            volley_with_mount_offset(ECLIPSE_FLAK, 20.0),
                            damage_multiplier,
                        ),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
            } else if let Some(((primary_reload, primary), (secondary_reload, secondary))) =
                naval_weapon_volleys(enemy.unit_type)
            {
                enemy.secondary_attack_reload += reload_delta;
                while enemy.attack_reload >= primary_reload {
                    enemy.attack_reload -= primary_reload;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(primary, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
                while enemy.secondary_attack_reload >= secondary_reload {
                    enemy.secondary_attack_reload -= secondary_reload;
                    let locked_building = if unit_target.is_some() {
                        None
                    } else {
                        building_target.map(|target| target.0)
                    };
                    spawn_enemy_volley(
                        world,
                        out,
                        enemy.id,
                        locked_building,
                        building_target.is_none(),
                        scaled_projectile_volley(secondary, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                    );
                }
            } else {
                // Vanilla UnitType.init doubles Weapon.reload for the flipped
                // mirror copy, so a volley with N mounts fires both mounts
                // every declared*2 ticks. Units without a volley (direct
                // building damage fallback) keep the plain cadence.
                let generic_cycle = enemy.attack_reload_time
                    * f32::from(
                        enemy_projectile_volley(enemy.unit_type)
                            .map(volley_mount_count)
                            .unwrap_or(1),
                    );
                while enemy.attack_reload >= generic_cycle {
                    enemy.attack_reload -= generic_cycle;
                    if enemy.unit_type == HORIZON.unit_type {
                        for _ in 0..2 {
                            spawn_enemy_horizon_bomb(
                                world,
                                out,
                                enemy.id,
                                damage_multiplier,
                                enemy.x,
                                enemy.y,
                                aim_x,
                                aim_y,
                            );
                        }
                    } else if let Some(volley) = enemy_projectile_volley(enemy.unit_type) {
                        spawn_enemy_volley(
                            world,
                            out,
                            enemy.id,
                            building_target.map(|target| target.0),
                            building_target.is_none(),
                            scaled_projectile_volley(volley, damage_multiplier),
                            enemy.x,
                            enemy.y,
                            aim_x,
                            aim_y,
                        );
                    } else if let Some((position, _, _)) = building_target {
                        // dashmap-guard: allow DM900 reason="damage_building operates on world.tiles, while this live guard belongs to world.enemies"
                        if let Some((destroyed, health)) = damage_building(
                            world,
                            position,
                            enemy.attack_damage * damage_multiplier,
                        ) {
                            if destroyed {
                                destroyed_buildings.insert(position);
                                health_updates.remove(&position);
                            } else {
                                health_updates.insert(position, health);
                            }
                        }
                    } else {
                        core_damage += enemy.attack_damage * damage_multiplier;
                    }
                }
            }
            if let Some(radius) = enemy_circle_radius(enemy.unit_type) {
                move_enemy_in_attack_orbit(&mut enemy, aim_x, aim_y, radius, delta_ticks);
            } else {
                enemy.velocity_x = 0.0;
                enemy.velocity_y = 0.0;
            }
        } else {
            let move_dx = movement_target.0 - enemy.x;
            let move_dy = movement_target.1 - enemy.y;
            let move_distance = move_dx.hypot(move_dy);
            if move_distance > 0.001 {
                let speed =
                    effective_unit_speed_on_floor(&enemy, Some(floor_at(world, enemy.x, enemy.y)));
                if delta_ticks <= 2.0 {
                    let desired_x = move_dx / move_distance * speed;
                    let desired_y = move_dy / move_distance * speed;
                    let (_, accel, _, _, _) =
                        crate::network::units::unit_move_physics(enemy.unit_type);
                    let dvx = desired_x - enemy.velocity_x;
                    let dvy = desired_y - enemy.velocity_y;
                    let limit = accel * speed * delta_ticks;
                    let change = dvx.hypot(dvy).min(limit);
                    if change > 0.0 {
                        let scale = change / dvx.hypot(dvy).max(0.0001);
                        enemy.velocity_x += dvx * scale;
                        enemy.velocity_y += dvy * scale;
                    }
                    let unit_type = enemy.unit_type;
                    let drag_mul = {
                        let rules_drag = world.wave_rules.read().drag_multiplier;
                        let flying = crate::game::content::unit_movement(unit_type).flying;
                        let floor_drag = if flying {
                            1.0
                        } else {
                            crate::game::content::floor_drag_multiplier(
                                crate::network::combat::floor_at(world, enemy.x, enemy.y),
                            )
                        };
                        rules_drag * floor_drag
                    };
                    crate::network::units::integrate_unit_velocity_drag(
                        &mut enemy,
                        unit_type,
                        delta_ticks,
                        drag_mul,
                    );
                    enemy.rotation = move_dy.atan2(move_dx).to_degrees();
                } else {
                    let step = (speed * delta_ticks).min(move_distance);
                    enemy.velocity_x = move_dx / move_distance * speed;
                    enemy.velocity_y = move_dy / move_distance * speed;
                    enemy.x += move_dx / move_distance * step;
                    enemy.y += move_dy / move_distance * step;
                    enemy.rotation = move_dy.atan2(move_dx).to_degrees();
                }
            }
        }
        world.enemies.insert(enemy.id, enemy);
    }
    for id in exploded {
        world.enemies.remove(&id);
        world.unregister_unit_group(id);
        // P0-01: kamikaze deaths previously leaked the unit's order; control
        // associations die with the unit.
        crate::network::units::detach_unit_control(world, id);
    }
    if core_damage > 0.0 {
        // The wave enemy attacks the sharded core (team 1) in survival/attack.
        // Per-team core damage + game-over handling lives in combat.rs
        // (`damage_team_core`); it keeps GameState.core_health in sync.
        let destroyed = crate::network::combat::damage_team_core(world, out, 1, core_damage);
        if destroyed {
            info!("The sharded core was destroyed; game over");
        }
    }
    world
        .game_state
        .enemies_count
        .store(hostile_unit_count(world), Ordering::Relaxed);
    (
        true,
        destroyed_buildings.into_iter().collect(),
        health_updates.into_iter().collect(),
    )
}

/// RtsAI-style squad assignment: same-type enemies within
/// `60 + hitSize*1.5` of each other share one attack target - the team-1
/// building nearest the squad centroid. Deterministic: groups form in id
/// order and ties break on position.
/// Official GroundAI/FlyingAI unit priority (audit H12): when an opposing
/// PLAYER-CONTROLLED unit is inside attack range it takes aim precedence
/// Weapon.collidesAir / collidesGround for the attacker's primary mount.
/// Horizon (16) and quad (23) are ground bombers (ASTRA E02).
fn unit_weapon_air_ground(unit_type: i16) -> (bool, bool) {
    // UnitType.targetAir / targetGround from the 159.7 content dump.
    match unit_type {
        2 | 11 | 16 | 23 => (false, true),
        _ => (true, true),
    }
}

/// Snapshot-backed hostile lookup (ASTRA E02).
pub(crate) fn nearest_hostile_from_snapshot(
    targets: &[(i32, u8, f32, f32, f32, f32, bool)],
    self_id: i32,
    enemy_x: f32,
    enemy_y: f32,
    team: u8,
    range: f32,
    attacker_type: i16,
) -> Option<(i32, f32, f32, f32, f32)> {
    let (target_air, target_ground) = unit_weapon_air_ground(attacker_type);
    targets
        .iter()
        .filter(|&&(id, target_team, _, _, _, _, flying)| {
            id != self_id
                && target_team != team
                && ((flying && target_air) || (!flying && target_ground))
        })
        .filter_map(|&(id, _, x, y, vx, vy, _)| {
            let distance = (x - enemy_x).hypot(y - enemy_y);
            (distance <= range).then_some((distance, id, x, y, vx, vy))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, id, x, y, vx, vy)| (id, x, y, vx, vy))
}

/// over buildings (`Units.closestTarget` scans units before buildings).
/// Returns (player_id, x, y, velocity_x, velocity_y) of the closest target.
pub(crate) fn nearest_player_unit_in_range(
    world: &DynamicWorld,
    enemy_x: f32,
    enemy_y: f32,
    team: u8,
    range: f32,
) -> Option<(i32, f32, f32, f32, f32)> {
    world
        .players
        .iter()
        .filter(|player| !player.dead && player.team != team)
        .filter_map(|player| {
            let distance = (player.x - enemy_x).hypot(player.y - enemy_y);
            if distance > range {
                return None;
            }
            let (vx, vy) = world
                .player_sessions
                .get(player.key())
                .map(|session| (session.velocity_x, session.velocity_y))
                .unwrap_or((0.0, 0.0));
            Some((distance, *player.key(), player.x, player.y, vx, vy))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, id, x, y, vx, vy)| (id, x, y, vx, vy))
}

/// Official `Predict.intercept` approximation: lead time = distance /
/// bullet speed (clamped to 2 s), aimed at the target's future position.
pub(crate) fn intercept_aim(
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    velocity_x: f32,
    velocity_y: f32,
    bullet_speed: f32,
) -> (f32, f32) {
    let distance = (target_x - source_x).hypot(target_y - source_y);
    if bullet_speed <= 0.01 || distance <= 0.01 {
        return (target_x, target_y);
    }
    let lead = (distance / bullet_speed).clamp(0.0, 2.0);
    (target_x + velocity_x * lead, target_y + velocity_y * lead)
}

pub(crate) fn squad_target_assignments(world: &DynamicWorld) -> HashMap<i32, (i32, f32, f32)> {
    let wave_team = world.wave_rules.read().wave_team;
    // Members snapshot: id, unit_type, flag, x, y.
    let members: Vec<(i32, i16, f64, f32, f32)> = world
        .enemies
        .iter()
        .filter(|enemy| enemy.team == wave_team)
        .map(|enemy| (*enemy.key(), enemy.unit_type, enemy.flag, enemy.x, enemy.y))
        .collect();

    /// Block ids of every vanilla core (shard/foundation/nucleus/bastion/
    /// citadel/acropolis). RtsAI weights a core at -999999 so any squad
    /// defends/rushes an exposed core first.
    const CORE_BLOCKS: std::ops::RangeInclusive<i16> = 339..=344;

    // Buildings owned by each side: team-1 (players) are attack candidates,
    // wave-team ones feed the defend branch, exactly like RtsAI's indexer
    // getEnemy/getOwn split.
    struct Building {
        position: i32,
        block: i16,
        x: f32,
        y: f32,
        damaged: bool,
    }
    let mut player_buildings: Vec<Building> = Vec::new();
    let mut own_damaged: Vec<(i32, f32, f32)> = Vec::new();
    for tile in world.tiles.iter() {
        if tile.block == 0 || (tile.team != 1 && tile.team != wave_team) {
            continue;
        }
        let bx = (tile.position >> 16) as i16 as f32 * 8.0;
        let by = tile.position as i16 as f32 * 8.0;
        let damaged = tile.health < crate::game::content::block_health(tile.block);
        if tile.team == wave_team {
            if damaged {
                own_damaged.push((tile.position, bx, by));
            }
        } else {
            player_buildings.push(Building {
                position: tile.position,
                block: tile.block,
                x: bx,
                y: by,
                damaged,
            });
        }
    }
    for building in &world.base_buildings {
        if building.team != 1 && building.team != wave_team {
            continue;
        }
        let bx = (building.position >> 16) as i16 as f32 * 8.0;
        let by = building.position as i16 as f32 * 8.0;
        let damaged = building.health < crate::game::content::block_health(building.block);
        if building.team == wave_team {
            if damaged {
                own_damaged.push((building.position, bx, by));
            }
        } else {
            player_buildings.push(Building {
                position: building.position,
                block: building.block,
                x: bx,
                y: by,
                damaged,
            });
        }
    }

    let mut assignments = HashMap::new();
    let mut assigned: HashSet<i32> = HashSet::new();
    // Buildings already handed to an earlier squad (RtsAI.assignedTargets):
    // later squads pick different objectives instead of stacking.
    let mut claimed: HashSet<i32> = HashSet::new();
    let (min_squad, max_squad, min_weight) = {
        let rules = world.wave_rules.read();
        let rule = rules.team_rule(wave_team);
        (
            rule.rts_min_squad.max(2) as usize,
            rule.rts_max_squad.max(1) as usize,
            rule.rts_min_weight,
        )
    };

    for &(seed_id, _, seed_flag, ..) in &members {
        if assigned.contains(&seed_id) {
            continue;
        }
        // Flood fill exactly like assignSquads: each popped unit queries a
        // SQUARE of side rad = squadRadius + its own hitSize*1.5 centered on
        // itself; membership requires the seed's flag parity, NOT same type.
        let mut group: Vec<(i32, f32, f32)> = Vec::new();
        let mut queue = vec![seed_id];
        assigned.insert(seed_id);
        while let Some(id) = queue.pop() {
            let Some(&(_, utype, _, x, y)) = members.iter().find(|m| m.0 == id) else {
                continue;
            };
            group.push((id, x, y));
            let half =
                (60.0 + crate::network::combat::unit_combat::unit_hit_size(utype) * 1.5) / 2.0;
            for &(other_id, _, other_flag, other_x, other_y) in &members {
                if assigned.contains(&other_id)
                    || (other_flag == 0.0) != (seed_flag == 0.0)
                    || (other_x - x).abs() > half
                    || (other_y - y).abs() > half
                {
                    continue;
                }
                assigned.insert(other_id);
                queue.push(other_id);
            }
        }

        let count = group.len();
        if count < 2 {
            continue;
        }
        let centroid_x = group.iter().map(|(_, x, _)| x).sum::<f32>() / count as f32;
        let centroid_y = group.iter().map(|&(_, _, y)| y).sum::<f32>() / count as f32;
        let weight: f32 = group
            .iter()
            .map(|(id, _, _)| {
                let unit_type = members
                    .iter()
                    .find(|member| member.0 == *id)
                    .map(|member| member.1)
                    .unwrap_or(0);
                crate::network::units::enemy_spec(unit_type)
                    .map(|spec| spec.health)
                    .unwrap_or(0.0)
            })
            .sum();

        // handleSquad defend branch: only OWN recently damaged buildings pull
        // a squad back; cores always qualify, anything else needs rtsMinSquad
        // units or proximity. Without own damage, squads run the attack path.
        // Only buildings damaged inside the rolling BuildDamageEvent window
        // pull a squad back (damagedSet drains every assignSquads pass).
        let defend = own_damaged
            .iter()
            .filter(|(position, _, _)| {
                crate::network::combat::enemy::recently_damaged(world, *position)
            })
            .copied()
            .min_by_key(|&(position, bx, by)| {
                (
                    if CORE_BLOCKS.contains(&((position >> 16) as i16)) {
                        -999_999i64
                    } else {
                        ((bx - centroid_x).hypot(by - centroid_y)) as i64
                    },
                    position,
                )
            });
        let target: Option<(i32, f32, f32)> = match defend {
            Some((position, bx, by)) => Some((position, bx, by)),
            None => {
                // findTarget attack path: needs rtsMinSquad; rtsMinWeight
                // blocks under-strength squads unless they already hit
                // rtsMaxSquad (RtsAI.java handleSquad).
                if count < min_squad || (weight < min_weight && count < max_squad) {
                    None
                } else {
                    let mut candidates: Vec<&Building> = player_buildings
                        .iter()
                        .filter(|b| !claimed.contains(&b.position))
                        .collect();
                    // Core rush beats everything else.
                    let core_first = candidates.iter().any(|b| CORE_BLOCKS.contains(&b.block));
                    if core_first {
                        candidates.retain(|b| CORE_BLOCKS.contains(&b.block));
                    }
                    match candidates.into_iter().min_by(|l, r| {
                        let ld = (l.x - centroid_x).hypot(l.y - centroid_y);
                        let rd = (r.x - centroid_x).hypot(r.y - centroid_y);
                        ld.total_cmp(&rd).then_with(|| l.position.cmp(&r.position))
                    }) {
                        Some(best) => {
                            claimed.insert(best.position);
                            Some((best.position, best.x, best.y))
                        }
                        None => None,
                    }
                }
            }
        };
        if let Some((position, bx, by)) = target {
            for (id, _, _) in group {
                assignments.insert(id, (position, bx, by));
            }
        }
    }
    assignments
}

#[cfg(test)]
mod e02_target_air_tests {
    use super::nearest_hostile_from_snapshot;

    #[test]
    fn e02_fortress_skips_air_and_picks_ground() {
        let targets = [
            (1, 1u8, 0.0, 0.0, 0.0, 0.0, true),
            (2, 1u8, 10.0, 0.0, 0.0, 0.0, false),
        ];
        let hit = nearest_hostile_from_snapshot(&targets, 9, 0.0, 0.0, 2, 200.0, 2);
        assert_eq!(
            hit.map(|h| h.0),
            Some(2),
            "fortress targetAir=false must ignore a closer flyer"
        );
    }
}
