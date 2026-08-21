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
    hostile_unit_count, move_enemy_in_attack_orbit, spawn_wave,
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
                            let dealt = apply_incoming_unit_damage(&target, 40.0, 1.0);
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
    if !world.game_state.is_active()
        || world.game_state.game_over.load(Ordering::Relaxed)
        || matches!(
            *world.game_state.mode.read(),
            GameMode::Sandbox | GameMode::Pvp
        )
    {
        return (false, Vec::new(), Vec::new());
    }

    // Logic.runWave() only runs when Rules.waves and Rules.waveTimer permit
    // automatic waves. Rules.waitEnemies freezes the countdown while any
    // wave-team unit remains alive; waveSending is retained for the manual
    // play-button path (which this headless listener does not synthesize).
    let should_spawn =
        world.wave_rules.read().waves_enabled && world.wave_rules.read().wave_timer && {
            if world.wave_rules.read().wait_enemies && hostile_unit_count(world) > 0 {
                false
            } else {
                let mut time = world.game_state.wave_time.write();
                *time = (*time - delta_ticks).max(0.0);
                *time <= 0.0
            }
        };
    if should_spawn {
        spawn_wave(world);
        // Official Logic.runWave(): `wavetime = rules.waveSpacing`.
        *world.game_state.wave_time.write() = world.wave_rules.read().wave_spacing;
        if world.wave_rules.read().win_wave > 0
            && world.game_state.wave.load(Ordering::Relaxed)
                >= world.wave_rules.read().win_wave as u32
            && matches!(
                *world.game_state.mode.read(),
                GameMode::Survival | GameMode::Attack
            )
        {
            world.game_state.game_over.store(true, Ordering::Relaxed);
            emit_game_over_packet_with_winner(world, out, world.wave_rules.read().default_team);
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
    // Snapshot before acquiring DashMap shard write guards; iterating the map
    // from path selection while holding `iter_mut()` can deadlock on a shard.
    let avoidance = unit_avoidance_requests(world);
    for mut enemy in world.enemies.iter_mut() {
        if enemy.team != world.wave_rules.read().wave_team {
            continue;
        }
        let navigation = enemy_navigation_target(world, &enemy, target_x, target_y, &avoidance);
        let building_target = navigation.building;
        let movement_target = navigation.movement;
        let (aim_x, aim_y) = building_target
            .map(|(_, x, y)| (x, y))
            .unwrap_or((target_x, target_y));
        let dx = aim_x - enemy.x;
        let dy = aim_y - enemy.y;
        let distance = dx.hypot(dy);
        let reload_delta = effective_unit_reload_delta(&enemy, delta_ticks);
        let damage_multiplier = effective_unit_damage_multiplier(&enemy);
        if distance <= enemy.attack_range {
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
            enemy.attack_reload += reload_delta;
            if enemy.unit_type == ANTUMBRA.unit_type {
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                while enemy.attack_reload >= 20.0 {
                    enemy.attack_reload -= 20.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ANTUMBRA_MISSILE, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.secondary_attack_reload >= 35.0 {
                    enemy.secondary_attack_reload -= 35.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ANTUMBRA_MISSILE, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.tertiary_attack_reload >= 12.0 {
                    enemy.tertiary_attack_reload -= 12.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ANTUMBRA_CANNON, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
            } else if enemy.unit_type == 34 {
                while enemy.attack_reload >= 65.0 {
                    enemy.attack_reload -= 65.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(
                            enemy_projectile_volley(34).unwrap(),
                            damage_multiplier,
                        ),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
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
                while enemy.attack_reload >= 22.0 {
                    enemy.attack_reload -= 22.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(RETUSA_BOLT, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                let previous = enemy.tertiary_attack_reload.max(0.0);
                let current = previous + reload_delta;
                let mine_shots = retusa_mine_shots_between(previous, current);
                enemy.tertiary_attack_reload = current;
                enemy.secondary_attack_reload = current % 90.0;
                for shot_index in 0..mine_shots {
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
                        u8::try_from(shot_index).unwrap_or(u8::MAX),
                    );
                }
            } else if enemy.unit_type == 3 {
                // Scepter: scepter-weapon (45, 3-shot burst) + 2 scepter-mounts
                // (12 / 15). Shot delay of the burst is not modeled.
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                while enemy.attack_reload >= 45.0 {
                    enemy.attack_reload -= 45.0;
                    for shot_index in 0..SCEPTER_BOLT.shots {
                        spawn_enemy_projectile(
                            world,
                            out,
                            enemy.id,
                            building_target.map(|target| target.0),
                            building_target.is_none(),
                            scaled_projectile_volley(SCEPTER_BOLT, damage_multiplier),
                            enemy.x,
                            enemy.y,
                            aim_x,
                            aim_y,
                            shot_index,
                        );
                    }
                }
                while enemy.secondary_attack_reload >= 12.0 {
                    enemy.secondary_attack_reload -= 12.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(SCEPTER_MOUNT, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.tertiary_attack_reload >= 15.0 {
                    enemy.tertiary_attack_reload -= 15.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(SCEPTER_MOUNT, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
            } else if enemy.unit_type == 13 {
                // Arkyid: large-purple-mount (45) + 3 spiroct-weapons
                // (9 / 14 / 22); the third sap timer uses quaternary_attack_reload.
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                enemy.quaternary_attack_reload += reload_delta;
                while enemy.attack_reload >= 45.0 {
                    enemy.attack_reload -= 45.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ARKYID_ARTILLERY, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.secondary_attack_reload >= 9.0 {
                    enemy.secondary_attack_reload -= 9.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ARKYID_SAP, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.tertiary_attack_reload >= 14.0 {
                    enemy.tertiary_attack_reload -= 14.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ARKYID_SAP, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.quaternary_attack_reload >= 22.0 {
                    enemy.quaternary_attack_reload -= 22.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ARKYID_SAP, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
            } else if enemy.unit_type == 19 {
                // Eclipse: large-laser-mount (45) + 2 large-artillery (9 / 12).
                enemy.secondary_attack_reload += reload_delta;
                enemy.tertiary_attack_reload += reload_delta;
                while enemy.attack_reload >= 45.0 {
                    enemy.attack_reload -= 45.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ECLIPSE_LASER, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.secondary_attack_reload >= 9.0 {
                    enemy.secondary_attack_reload -= 9.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ECLIPSE_FLAK, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
                while enemy.tertiary_attack_reload >= 12.0 {
                    enemy.tertiary_attack_reload -= 12.0;
                    spawn_enemy_projectile(
                        world,
                        out,
                        enemy.id,
                        building_target.map(|target| target.0),
                        building_target.is_none(),
                        scaled_projectile_volley(ECLIPSE_FLAK, damage_multiplier),
                        enemy.x,
                        enemy.y,
                        aim_x,
                        aim_y,
                        0,
                    );
                }
            } else if let Some(((primary_reload, primary), (secondary_reload, secondary))) =
                naval_weapon_volleys(enemy.unit_type)
            {
                enemy.secondary_attack_reload += reload_delta;
                while enemy.attack_reload >= primary_reload {
                    enemy.attack_reload -= primary_reload;
                    for shot_index in 0..primary.shots {
                        spawn_enemy_projectile(
                            world,
                            out,
                            enemy.id,
                            building_target.map(|target| target.0),
                            building_target.is_none(),
                            scaled_projectile_volley(primary, damage_multiplier),
                            enemy.x,
                            enemy.y,
                            aim_x,
                            aim_y,
                            shot_index,
                        );
                    }
                }
                while enemy.secondary_attack_reload >= secondary_reload {
                    enemy.secondary_attack_reload -= secondary_reload;
                    for shot_index in 0..secondary.shots {
                        spawn_enemy_projectile(
                            world,
                            out,
                            enemy.id,
                            building_target.map(|target| target.0),
                            building_target.is_none(),
                            scaled_projectile_volley(secondary, damage_multiplier),
                            enemy.x,
                            enemy.y,
                            aim_x,
                            aim_y,
                            shot_index,
                        );
                    }
                }
            } else {
                while enemy.attack_reload >= enemy.attack_reload_time {
                    enemy.attack_reload -= enemy.attack_reload_time;
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
                        for shot_index in 0..volley.shots {
                            spawn_enemy_projectile(
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
                                shot_index,
                            );
                        }
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
                let speed = effective_unit_speed(&enemy);
                let step = (speed * delta_ticks).min(move_distance);
                enemy.velocity_x = move_dx / move_distance * speed;
                enemy.velocity_y = move_dy / move_distance * speed;
                enemy.x += move_dx / move_distance * step;
                enemy.y += move_dy / move_distance * step;
                enemy.rotation = move_dy.atan2(move_dx).to_degrees();
            }
        }
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
