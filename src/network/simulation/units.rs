//! Unit simulation: statuses, collisions, allied/controlled units,
//! assist/builder/support/mining/nav helpers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::construction::{
    consume_requirements, dynamic_at, effective_block, encode_begin_place_for_unit,
};
use crate::network::buildings::plans::{rebuild_plan, AssistTarget};
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::dynamic_tile_health;
use crate::network::buildings::snapshot::*;
use crate::network::combat::enemy::{base_building_at, damage_building, navigation_index};
use crate::network::combat::unit_combat::{
    boost_properties, collect_allied_weapon_fire, collect_manual_weapon_fire,
    collision_position_passable, damaged_allied_building_target, effective_unit_build_speed,
    effective_unit_damage_multiplier, effective_unit_reload_delta, invalidate_navigation_for_block,
    scaled_projectile_volley, spawn_allied_weapon_fire, spawn_weapon_fire_for_team, unit_can_shoot,
    unit_collision_layer, AlliedWeaponFire,
};
use crate::network::combat::*;
use crate::network::economy::has_requirements;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::simulation::{
    simulate_enemy_statuses, simulate_logic_build, simulate_logic_fire, simulate_logic_mining,
};
use crate::network::units::controller::{controlling_player_for_unit, unit_is_player_controlled};
use crate::network::units::mining::{
    heal_building_for_team, heal_buildings_in_radius, heal_nearest_building,
    heal_nearest_building_flat, move_repair_unit, move_unit_toward, nearest_mineable_ore,
};
use crate::network::units::unit_orders::{
    advance_unit_order, apply_ordered_unit_movement, boost_should_land_near_target,
    builder_unit_hit_size, ordered_opposing_building, route_unit_movement, unit_build_range,
    unit_has_stance, unit_logic_building, unit_logic_firing, unit_mining,
};
use crate::network::units::*;
use crate::network::wire::auth::player_team;
use crate::network::wire::client_snapshot::raw_mine_result;
use crate::network::wire::encode::{
    encode_build_destroyed_frame, encode_build_health_update_frame, frame_generated_packet,
};
use crate::network::wire::persistence::encode_construct_finish_for_unit;
use crate::network::wire::tile_config::broadcast_placement_power_configs;
use crate::network::wire::transfer::nearest_opposing_unit;
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;
use tracing::{debug, error, info, warn};

use super::*;

pub fn simulate_all_unit_statuses(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    simulate_enemy_statuses(world, out, delta_ticks)
}

/// Emits `GameOverCallPacket` (ID 48) to every connected client, mirroring
/// the official broadcast `Call.gameOver(event.winner)` fired by
/// `Logic.checkGameState` when the last player core is destroyed
/// (`Control.java` GameOverEvent listener). The winner is
/// `state.rules.waveTeam` — in survival/attack the enemy team (crux, id 2) —
/// so the 158.1 client computes `state.won = player.team() == winner` and
/// shows the defeat/restart dialog. Also flags the world dirty for
/// persistence, like the official server marks state and the console
/// `gameover` command already does.
/// Broadcasts the `GameOverCallPacket` (ID 48) with the given winner team.
/// Official TypeIO.writeTeam serializes `Team` as one `b teamId`; survival/
/// attack use the waveTeam (crux = 2) when the player core is destroyed,
/// while PvP uses the last team standing (its own core id).
pub fn simulate_pvp_player_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
) -> bool {
    if *world.game_state.mode.read() != GameMode::Pvp {
        return false;
    }
    let projectiles: Vec<(i32, Projectile)> = world
        .projectiles
        .iter()
        .map(|entry| (*entry.key(), entry.value().clone()))
        .collect();
    if projectiles.is_empty() {
        return false;
    }
    let players: Vec<PlayerCombatState> = world
        .players
        .iter()
        .map(|entry| entry.value().clone())
        .collect();
    // Snapshot the registered cores (one per team) so no DashMap shard guard
    // is held while the damage helpers below mutate the same maps.
    let cores: Vec<(u8, TeamCore)> = crate::network::world::registered_core_teams(world)
        .into_iter()
        .flat_map(|team| {
            crate::network::world::team_core_snapshot(world, team)
                .into_iter()
                .map(move |core| (team, core))
        })
        .collect();
    let mut changed = false;
    for (projectile_id, projectile) in projectiles {
        if projectile.team == 0 || projectile.team == world.wave_rules.read().wave_team {
            continue;
        }
        // PvP core destruction: a projectile of team A crossing an enemy
        // team's core footprint damages that team's core (official
        // BulletType.collidesTeam vs CoreBlock buildings). The first core
        // crossed consumes a non-beam projectile, like a player hit.
        let core_hit = cores.iter().find(|(team, core)| {
            *team != 0 && *team != projectile.team && core.health > 0.0 && {
                let core_x = (core.position >> 16) as i16 as f32 * 8.0;
                let core_y = core.position as i16 as f32 * 8.0;
                let radius =
                    f32::from(crate::game::content::block_size(core.block)) * 8.0 / 2.0 + 4.0;
                let (distance, _) = point_segment_distance(
                    core_x,
                    core_y,
                    projectile.source_x,
                    projectile.source_y,
                    projectile.target_x,
                    projectile.target_y,
                );
                distance <= radius
            }
        });
        if let Some((team, _)) = core_hit {
            let _ = crate::network::combat::damage_team_core(world, out, *team, projectile.damage);
            changed = true;
            if projectile.damage_interval.is_none() {
                world.projectiles.remove(&projectile_id);
            }
            continue;
        }
        let Some(victim) = players
            .iter()
            .filter(|player| !player.dead && player.team != 0 && player.team != projectile.team)
            .find(|player| {
                let (distance, _) = point_segment_distance(
                    player.x,
                    player.y,
                    projectile.source_x,
                    projectile.source_y,
                    projectile.target_x,
                    projectile.target_y,
                );
                distance <= 8.0
            })
        else {
            continue;
        };
        changed |= damage_player(
            world,
            out,
            victim.unit_id,
            projectile.damage,
            projectile.status_effect,
            projectile.status_duration,
        );
        if projectile.damage_interval.is_none() {
            world.projectiles.remove(&projectile_id);
        }
    }
    changed
}

/// Rules.pvpAutoPause (158.1 default true): while fewer than two teams have a
/// connected player the game is paused waiting for players; as soon as a
/// second team joins it resumes. Matches `NetServer.isWaitingForPlayers` +
/// the auto-pause state machine in `NetServer.update`. A manual `pause` from
/// the console is never overridden by the auto-resume (it clears the
/// auto-paused marker), and after game over the pause is released.
pub fn simulate_unit_collisions(world: &DynamicWorld) -> bool {
    const RADIUS_SCALE: f32 = 0.6;
    const SEPARATION_SCALE: f32 = 1.25;

    let mut units: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| crate::game::content::unit_movement(unit.unit_type).physics)
        .map(|unit| unit.clone())
        .collect();
    units.sort_unstable_by_key(|unit| unit.id);
    let mut offsets: HashMap<i32, (f32, f32)> = HashMap::new();
    for left_index in 0..units.len() {
        for right_index in left_index + 1..units.len() {
            let left = &units[left_index];
            let right = &units[right_index];
            if unit_collision_layer(left) != unit_collision_layer(right) {
                continue;
            }
            let left_movement = crate::game::content::unit_movement(left.unit_type);
            let right_movement = crate::game::content::unit_movement(right.unit_type);
            let left_radius = left_movement.hit_size * RADIUS_SCALE;
            let right_radius = right_movement.hit_size * RADIUS_SCALE;
            let required = left_radius + right_radius;
            let dx = left.x - right.x;
            let dy = left.y - right.y;
            let distance = dx.hypot(dy);
            if distance >= required {
                continue;
            }
            let (direction_x, direction_y) = if distance > 0.0001 {
                (dx / distance, dy / distance)
            } else {
                let hash = (left.id as u32)
                    .wrapping_mul(0x9e37_79b9)
                    .wrapping_add(right.id as u32);
                let angle = (hash % 360) as f32 * std::f32::consts::PI / 180.0;
                (angle.cos(), angle.sin())
            };
            let overlap = (required - distance) / SEPARATION_SCALE;
            let left_mass = left_movement.hit_size * left_movement.hit_size;
            let right_mass = right_movement.hit_size * right_movement.hit_size;
            let mass = (left_mass + right_mass).max(f32::EPSILON);
            let left_share = right_mass / mass;
            let right_share = left_mass / mass;
            let left_offset = offsets.entry(left.id).or_default();
            left_offset.0 += direction_x * overlap * left_share;
            left_offset.1 += direction_y * overlap * left_share;
            let right_offset = offsets.entry(right.id).or_default();
            right_offset.0 -= direction_x * overlap * right_share;
            right_offset.1 -= direction_y * overlap * right_share;
        }
    }
    let players: Vec<_> = world
        .players
        .iter()
        .filter(|player| !player.dead)
        .map(|player| player.clone())
        .collect();
    let mut player_offsets: HashMap<i32, (f32, f32)> = HashMap::new();
    for player in &players {
        for unit in &units {
            if unit_collision_layer(unit) != 0 {
                continue;
            }
            let movement = crate::game::content::unit_movement(unit.unit_type);
            let required = (8.0 + movement.hit_size) * RADIUS_SCALE;
            let dx = player.x - unit.x;
            let dy = player.y - unit.y;
            let distance = dx.hypot(dy);
            if distance >= required {
                continue;
            }
            let (direction_x, direction_y) = if distance > 0.0001 {
                (dx / distance, dy / distance)
            } else {
                let hash = (player.unit_id as u32)
                    .wrapping_mul(0x9e37_79b9)
                    .wrapping_add(unit.id as u32);
                let angle = (hash % 360) as f32 * std::f32::consts::PI / 180.0;
                (angle.cos(), angle.sin())
            };
            let overlap = (required - distance) / SEPARATION_SCALE;
            let player_mass = 64.0;
            let unit_mass = movement.hit_size * movement.hit_size;
            let mass = player_mass + unit_mass;
            let player_offset = player_offsets.entry(player.unit_id).or_default();
            player_offset.0 += direction_x * overlap * unit_mass / mass;
            player_offset.1 += direction_y * overlap * unit_mass / mass;
            let unit_offset = offsets.entry(unit.id).or_default();
            unit_offset.0 -= direction_x * overlap * player_mass / mass;
            unit_offset.1 -= direction_y * overlap * player_mass / mass;
        }
    }
    let mut changed = false;
    for player in players {
        let Some((dx, dy)) = player_offsets.get(&player.unit_id).copied() else {
            continue;
        };
        let tile_x = ((player.x + dx) / 8.0).floor() as i32;
        let tile_y = ((player.y + dy) / 8.0).floor() as i32;
        let passable = navigation_index(world, tile_x, tile_y).is_some_and(|index| {
            let position = (tile_x << 16) | (tile_y as u16 as i32);
            let block = dynamic_at(world, position)
                .map(|tile| tile.block)
                .or_else(|| base_building_at(world, position).map(|building| building.block))
                .unwrap_or(world.base_blocks[index]);
            !crate::game::content::block_navigation(block).solid
        });
        if passable {
            if let Some(mut live) = world.players.get_mut(&player.unit_id) {
                live.x += dx;
                live.y += dy;
                changed = true;
            }
        }
    }
    for unit in units {
        let Some((dx, dy)) = offsets.get(&unit.id).copied() else {
            continue;
        };
        let target_x = unit.x + dx;
        let target_y = unit.y + dy;
        if collision_position_passable(world, &unit, target_x, target_y) {
            if let Some(mut live) = world.enemies.get_mut(&unit.id) {
                live.x = target_x;
                live.y = target_y;
                changed = true;
            }
        }
    }
    changed
}

pub fn simulate_allied_oxynoe_repair(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    snapshot: &EnemyUnit,
    delta_ticks: f32,
) -> bool {
    if snapshot.unit_type != 31 {
        return false;
    }
    let Some((position, target_x, target_y)) =
        damaged_allied_building_target(world, snapshot.team, snapshot.x, snapshot.y, 61.2)
    else {
        return false;
    };
    let shots = {
        let Some(mut unit) = world.enemies.get_mut(&snapshot.id) else {
            return false;
        };
        unit.attack_reload += effective_unit_reload_delta(&unit, delta_ticks);
        let shots = (unit.attack_reload / 5.0).floor() as usize;
        unit.attack_reload %= 5.0;
        shots
    };
    let volley = enemy_projectile_volley(31).unwrap();
    for shot in 0..shots {
        spawn_allied_unit_projectile(
            world,
            out,
            0,
            -1,
            Some(position),
            volley,
            snapshot.x,
            snapshot.y,
            target_x,
            target_y,
            u8::try_from(shot).unwrap_or(u8::MAX),
        );
    }
    true
}

pub fn simulate_allied_units(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let allied_ids: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.team == 1 && !unit_is_player_controlled(world, unit.id))
        .map(|unit| unit.id)
        .collect();
    let mut changed = false;
    for allied_id in allied_ids {
        let Some(snapshot) = world.enemies.get(&allied_id).map(|unit| unit.clone()) else {
            continue;
        };
        let ramming = unit_has_stance(world, allied_id, 4);
        let boosting = unit_has_stance(world, allied_id, 5) && snapshot.elevation > 0.0;
        if crate::network::units::unit_bound_to_logic(world, allied_id) {
            if crate::network::units::apply_logic_unit_movement(world, &snapshot, delta_ticks)
                && !ramming
            {
                changed = true;
            }
            if unit_mining(world, allied_id) {
                simulate_logic_mining(world, &snapshot, delta_ticks);
                changed = true;
                continue;
            }
            if unit_logic_firing(world, allied_id) {
                simulate_logic_fire(world, out, &snapshot, delta_ticks);
                changed = true;
                continue;
            }
            if unit_logic_building(world, allied_id) {
                simulate_logic_build(world, out, &snapshot, delta_ticks);
                changed = true;
                continue;
            }
            continue;
        }
        if apply_ordered_unit_movement(world, &snapshot, delta_ticks) && !ramming {
            changed = true;
            continue;
        }
        if unit_mining(world, allied_id) {
            simulate_logic_mining(world, &snapshot, delta_ticks);
            changed = true;
            continue;
        }
        if unit_logic_firing(world, allied_id) {
            simulate_logic_fire(world, out, &snapshot, delta_ticks);
            changed = true;
            continue;
        }
        if unit_logic_building(world, allied_id) {
            simulate_logic_build(world, out, &snapshot, delta_ticks);
            changed = true;
            continue;
        }
        let command = world
            .unit_orders
            .get(&allied_id)
            .map(|order| order.command)
            .unwrap_or_else(|| default_unit_command(snapshot.unit_type));
        if unit_has_stance(world, allied_id, 1) {
            if let Some(mut ally) = world.enemies.get_mut(&allied_id) {
                ally.velocity_x = 0.0;
                ally.velocity_y = 0.0;
            }
            changed = true;
            continue;
        }
        if snapshot.unit_type == MONO.unit_type && command == 4 {
            continue;
        }
        if simulate_allied_oxynoe_repair(world, out, &snapshot, delta_ticks) {
            changed = true;
            continue;
        }
        if matches!(command, 1..=4) {
            continue;
        }
        if let Some((building_position, target_x, target_y)) =
            ordered_opposing_building(world, &snapshot)
        {
            let mut attack = false;
            let mut authoritative_fire = None;
            if let Some(mut ally) = world.enemies.get_mut(&allied_id) {
                let distance = (target_x - ally.x).hypot(target_y - ally.y);
                if distance <= ally.attack_range && !boosting {
                    if !ramming {
                        ally.velocity_x = 0.0;
                        ally.velocity_y = 0.0;
                    }
                    authoritative_fire =
                        collect_allied_weapon_fire(&mut ally, delta_ticks, distance);
                    if authoritative_fire.is_none() {
                        ally.attack_reload += effective_unit_reload_delta(&ally, delta_ticks);
                        if unit_can_shoot(&ally)
                            && ally.attack_damage > 0.0
                            && ally.attack_reload >= ally.attack_reload_time
                        {
                            ally.attack_reload %= ally.attack_reload_time.max(0.0001);
                            attack = true;
                        }
                    }
                }
            }
            if let Some(fires) = authoritative_fire {
                spawn_allied_weapon_fire(
                    world,
                    out,
                    &fires,
                    snapshot.id,
                    -1,
                    Some(building_position),
                    snapshot.x,
                    snapshot.y,
                    target_x,
                    target_y,
                );
            } else if attack {
                if let Some(volley) = enemy_projectile_volley(snapshot.unit_type) {
                    let volley = scaled_projectile_volley(
                        volley,
                        effective_unit_damage_multiplier(&snapshot),
                    );
                    for shot_index in 0..volley.shots {
                        spawn_allied_unit_projectile(
                            world,
                            out,
                            snapshot.id,
                            -1,
                            Some(building_position),
                            volley,
                            snapshot.x,
                            snapshot.y,
                            target_x,
                            target_y,
                            shot_index,
                        );
                    }
                } else if let Some((destroyed, health)) = damage_building(
                    world,
                    building_position,
                    snapshot.attack_damage * effective_unit_damage_multiplier(&snapshot),
                ) {
                    if destroyed {
                        advance_unit_order(world, allied_id);
                        if let Ok(frame) = encode_build_destroyed_frame(building_position) {
                            out.broadcast(frame);
                        }
                    } else if let Ok(frame) =
                        encode_build_health_update_frame(&[(building_position, health)])
                    {
                        out.broadcast(frame);
                    }
                }
            }
            changed = true;
            continue;
        }
        let ordered_target = world.unit_orders.get(&allied_id).and_then(|order| {
            (order.target_kind == 2)
                .then_some(order.target_id)
                .and_then(|id| {
                    world
                        .enemies
                        .get(&id)
                        .filter(|target| target.team != snapshot.team)
                        .map(|target| (target.id, target.x, target.y))
                })
        });
        let Some((target_id, target_x, target_y)) = ordered_target
            .or_else(|| nearest_opposing_unit(world, snapshot.team, snapshot.x, snapshot.y))
        else {
            if let Some(mut ally) = world.enemies.get_mut(&allied_id) {
                ally.velocity_x = 0.0;
                ally.velocity_y = 0.0;
            }
            continue;
        };
        let dx = target_x - snapshot.x;
        let dy = target_y - snapshot.y;
        let distance = dx.hypot(dy);
        let routed = route_unit_movement(
            world,
            &snapshot,
            target_x,
            target_y,
            delta_ticks,
            if ramming { 1.0 } else { snapshot.attack_range },
        );
        let mut attack = false;
        let mut authoritative_fire = None;
        if let Some(mut ally) = world.enemies.get_mut(&allied_id) {
            if distance <= ally.attack_range && ally.attack_damage > 0.0 && !boosting {
                if !ramming {
                    ally.velocity_x = 0.0;
                    ally.velocity_y = 0.0;
                }
                authoritative_fire = collect_allied_weapon_fire(&mut ally, delta_ticks, distance);
                if authoritative_fire.is_none() {
                    ally.attack_reload += effective_unit_reload_delta(&ally, delta_ticks);
                    if unit_can_shoot(&ally) && ally.attack_reload >= ally.attack_reload_time {
                        ally.attack_reload %= ally.attack_reload_time.max(0.0001);
                        attack = true;
                    }
                }
            }
            if distance > 0.001 && (ramming || distance > ally.attack_range) {
                if let Some((x, y, velocity_x, velocity_y, rotation)) = routed {
                    ally.x = x;
                    ally.y = y;
                    ally.velocity_x = velocity_x;
                    ally.velocity_y = velocity_y;
                    ally.rotation = rotation;
                } else {
                    ally.velocity_x = 0.0;
                    ally.velocity_y = 0.0;
                }
            }
        }
        if let Some(fires) = authoritative_fire {
            spawn_allied_weapon_fire(
                world,
                out,
                &fires,
                snapshot.id,
                target_id,
                None,
                snapshot.x,
                snapshot.y,
                target_x,
                target_y,
            );
        } else if attack {
            if let Some(volley) = enemy_projectile_volley(snapshot.unit_type) {
                let volley =
                    scaled_projectile_volley(volley, effective_unit_damage_multiplier(&snapshot));
                for shot_index in 0..volley.shots {
                    spawn_allied_unit_projectile(
                        world,
                        out,
                        snapshot.id,
                        target_id,
                        None,
                        volley,
                        snapshot.x,
                        snapshot.y,
                        target_x,
                        target_y,
                        shot_index,
                    );
                }
            } else {
                let dead = if let Some(mut target) = world.enemies.get_mut(&target_id) {
                    let damage = apply_incoming_unit_damage(
                        &target,
                        snapshot.attack_damage * effective_unit_damage_multiplier(&snapshot),
                        1.0,
                    );
                    let absorbed = target.shield.min(damage);
                    target.shield -= absorbed;
                    target.health -= damage - absorbed;
                    target.health <= 0.0
                } else {
                    false
                };
                if dead {
                    kill_enemy(world, out, target_id);
                }
            }
        }

        changed = true;
    }
    changed
}

/// Tick-driven weapons for units possessed through `UnitControl`.
///
/// Client snapshots only carry aim/trigger state; cadence, mount timers,
/// damage and projectile ownership stay server-authoritative. The regular AI
/// loop excludes these ids, so this is the single firing owner while a player
/// possesses a unit.
pub fn simulate_controlled_unit_weapons(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let controllers: Vec<_> = world
        .player_sessions
        .iter()
        .filter_map(|session| {
            let unit_id = session.controlled_unit.standard_id()?;
            Some((unit_id, session.value().clone()))
        })
        .collect();
    let mut changed = false;
    for (unit_id, controller) in controllers {
        let Some(snapshot) = world.enemies.get(&unit_id).map(|unit| unit.clone()) else {
            continue;
        };
        if snapshot.health <= 0.0 || player_team(world, &controller) != snapshot.team {
            continue;
        }
        changed |= simulate_controlled_navanax_lasers(world, out, &snapshot, delta_ticks);
        // Stop a stuck manual trigger if snapshots cease without a clean
        // disconnect. Autonomous mounts above remain active, like Java.
        if !controller.shooting
            || controller.last_shot.elapsed() > std::time::Duration::from_millis(500)
        {
            continue;
        }
        let Some(aim) = resolve_manual_aim(
            world,
            snapshot.team,
            snapshot.x,
            snapshot.y,
            controller.mouse_x,
            controller.mouse_y,
            snapshot.attack_range,
            |_| true,
        ) else {
            continue;
        };
        if let Some(healed) =
            simulate_controlled_repair_beam(world, out, &snapshot, aim.x, aim.y, delta_ticks)
        {
            changed |= healed;
            continue;
        }
        let fires = if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
            collect_manual_weapon_fire(&mut unit, delta_ticks, aim.distance)
        } else {
            None
        };
        let Some(fires) = fires else {
            continue;
        };
        if !fires.is_empty() {
            let can_heal_building = fires.iter().any(|fire| {
                matches!(
                    fire,
                    AlliedWeaponFire::Projectile(volley)
                        if projectile_direct_heal_percent(volley.bullet_id).is_some()
                )
            });
            let heal_target = can_heal_building
                .then(|| {
                    damaged_allied_building_on_ray(
                        world,
                        snapshot.team,
                        snapshot.x,
                        snapshot.y,
                        aim.x,
                        aim.y,
                    )
                })
                .flatten()
                .filter(|(_, target_x, target_y)| {
                    let allied_distance = (*target_x - snapshot.x).hypot(*target_y - snapshot.y);
                    world.enemies.get(&aim.target_id).is_none_or(|enemy| {
                        allied_distance
                            <= (enemy.x - snapshot.x).hypot(enemy.y - snapshot.y) + 0.001
                    })
                });
            let (target_id, target_position, target_x, target_y) =
                if let Some((position, target_x, target_y)) = heal_target {
                    (-1, Some(position), target_x, target_y)
                } else {
                    (aim.target_id, None, aim.x, aim.y)
                };
            spawn_weapon_fire_for_team(
                world,
                out,
                &fires,
                snapshot.id,
                target_id,
                target_position,
                snapshot.x,
                snapshot.y,
                target_x,
                target_y,
                snapshot.team,
            );
        }
        changed = true;
    }
    changed
}

pub(crate) fn simulate_controlled_navanax_lasers(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    unit: &EnemyUnit,
    delta_ticks: f32,
) -> bool {
    if unit.unit_type != 34 {
        return false;
    }
    let target = world
        .enemies
        .iter()
        .filter(|target| target.team != unit.team)
        .filter_map(|target| {
            let distance = (target.x - unit.x).hypot(target.y - unit.y);
            (distance <= 90.0).then_some((distance, target.id, target.x, target.y))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0));
    let Some((_, target_id, target_x, target_y)) = target else {
        return false;
    };
    let (shots, multiplier) = if let Some(mut live) = world.enemies.get_mut(&unit.id) {
        live.secondary_attack_reload += effective_unit_reload_delta(&live, delta_ticks);
        let shots = (live.secondary_attack_reload / 170.0).floor() as usize;
        live.secondary_attack_reload %= 170.0;
        (shots, effective_unit_damage_multiplier(&live))
    } else {
        return false;
    };
    for _ in 0..shots {
        spawn_navanax_lasers(
            world, out, unit.team, multiplier, unit.id, target_id, None, false, unit.x, unit.y,
            target_x, target_y,
        );
    }
    shots > 0
}

/// The three Erekir core units deliberately override RepairBeamWeapon's
/// autonomous defaults (`autoTarget=false`, `controllable=true`). These are
/// manual tools, not BulletTypes: snap the aim ray to the first damaged allied
/// building and apply their per-tick flat + percentage repair exactly once.
pub(crate) fn simulate_controlled_repair_beam(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    unit: &EnemyUnit,
    aim_x: f32,
    aim_y: f32,
    delta_ticks: f32,
) -> Option<bool> {
    let (flat_per_tick, percent_per_tick) = match unit.unit_type {
        58 => (3.1, 0.06), // evoke
        59 => (3.3, 0.06), // incite
        // Emanate has two mirrored 1.8 + 0.03% mounts.
        60 => (3.6, 0.06),
        _ => return None,
    };
    let mut seen = std::collections::HashSet::new();
    let mut candidates: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == unit.team && seen.insert(tile.position))
        .filter_map(|tile| {
            let maximum = crate::game::content::block_health(tile.block);
            if dynamic_tile_health(&tile) >= maximum - 0.0001 {
                return None;
            }
            let x = (tile.position >> 16) as i16 as f32 * 8.0;
            let y = tile.position as i16 as f32 * 8.0;
            point_hits_segment(x, y, unit.x, unit.y, aim_x, aim_y, 7.0)
                .map(|progress| (progress, tile.position, maximum))
        })
        .collect();
    candidates.extend(world.base_buildings.iter().filter_map(|building| {
        if building.team != unit.team
            || !seen.insert(building.position)
            || building.health >= crate::game::content::block_health(building.block) - 0.0001
        {
            return None;
        }
        let maximum = crate::game::content::block_health(building.block);
        let x = (building.position >> 16) as i16 as f32 * 8.0;
        let y = building.position as i16 as f32 * 8.0;
        point_hits_segment(x, y, unit.x, unit.y, aim_x, aim_y, 7.0)
            .map(|progress| (progress, building.position, maximum))
    }));
    let Some((_, position, maximum)) = candidates
        .into_iter()
        .min_by(|left, right| left.0.total_cmp(&right.0))
    else {
        return Some(false);
    };
    let amount = (flat_per_tick + maximum * percent_per_tick / 100.0) * delta_ticks.max(0.0);
    let Some(health) = heal_building_for_team(world, position, unit.team, 0.0, amount) else {
        return Some(false);
    };
    if let Ok(frame) = encode_build_health_update_frame(&[(position, health)]) {
        out.broadcast(frame);
    }
    Some(true)
}

pub fn simulate_unit_elevation(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let ids: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.team == 1 && boost_properties(unit.unit_type).is_some())
        .map(|unit| unit.id)
        .collect();
    let mut changed = false;
    for id in ids {
        let Some(snapshot) = world.enemies.get(&id).map(|unit| unit.clone()) else {
            continue;
        };
        let controlled_boosting = controlling_player_for_unit(world, id).and_then(|player_id| {
            world
                .player_sessions
                .iter()
                .find(|session| session.id == player_id)
                .map(|session| session.boosting)
        });
        let target = f32::from(controlled_boosting.unwrap_or_else(|| {
            unit_has_stance(world, id, 5) && !boost_should_land_near_target(world, &snapshot)
        }));
        let Some(mut unit) = world.enemies.get_mut(&id) else {
            continue;
        };
        let (_, rise, descent) = boost_properties(unit.unit_type).unwrap();
        let rate = if target > unit.elevation {
            rise
        } else {
            descent
        };
        let before = unit.elevation;
        if unit.elevation < target {
            unit.elevation = (unit.elevation + rate * delta_ticks.max(0.0)).min(target);
        } else {
            unit.elevation = (unit.elevation - rate * delta_ticks.max(0.0)).max(target);
        }
        changed |= (unit.elevation - before).abs() > f32::EPSILON;
    }
    changed
}

pub fn simulate_assist_units(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let assistants: Vec<_> = world
        .enemies
        .iter()
        .filter_map(|unit| {
            let speed = effective_unit_build_speed(&unit)?;
            let command = world
                .unit_orders
                .get(&unit.id)
                .map(|order| order.command)
                .unwrap_or_else(|| default_unit_command(unit.unit_type));
            (unit.team == 1
                && command == 3
                && unit.update_building
                && !unit_is_player_controlled(world, unit.id)
                && !unit_bound_to_logic(world, unit.id))
            .then(|| (unit.clone(), speed))
        })
        .collect();
    let primary_rebuilders: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| {
            unit.team == 1
                && !unit_is_player_controlled(world, unit.id)
                && !unit_bound_to_logic(world, unit.id)
                && world
                    .unit_orders
                    .get(&unit.id)
                    .map(|order| order.command)
                    .unwrap_or_else(|| default_unit_command(unit.unit_type))
                    == 2
        })
        .map(|unit| unit.id)
        .collect();
    let mut changed = false;
    for (assistant, build_speed) in assistants {
        let now = std::time::Instant::now();
        let mut candidates: Vec<_> = world
            .pending_builds
            .iter()
            .filter(|build| {
                now.duration_since(build.last_seen) <= std::time::Duration::from_millis(300)
            })
            .map(|build| {
                let x = (build.position >> 16) as i16 as f32 * 8.0;
                let y = build.position as i16 as f32 * 8.0;
                (
                    (x - assistant.x).hypot(y - assistant.y),
                    AssistTarget::Pending(build.position),
                    x,
                    y,
                )
            })
            .collect();
        if !primary_rebuilders.is_empty() {
            candidates.extend(world.tiles.iter().filter_map(|tile| {
                (tile.production_progress > 0.0 && rebuild_plan(&tile).is_some()).then(|| {
                    let x = (tile.position >> 16) as i16 as f32 * 8.0;
                    let y = tile.position as i16 as f32 * 8.0;
                    (
                        (x - assistant.x).hypot(y - assistant.y),
                        AssistTarget::Rebuild(tile.position),
                        x,
                        y,
                    )
                })
            }));
        }
        let Some((distance, target, target_x, target_y)) = candidates
            .into_iter()
            .min_by(|left, right| left.0.total_cmp(&right.0))
        else {
            continue;
        };
        if distance > 1_500.0 {
            continue;
        }
        let range = unit_build_range(assistant.unit_type)
            - builder_unit_hit_size(assistant.unit_type).unwrap_or(0.0) * 2.0;
        if distance > range {
            if !unit_has_stance(world, assistant.id, 6) {
                changed |= move_unit_toward(world, assistant.id, target_x, target_y, delta_ticks);
            }
            continue;
        }
        if let Some(mut unit) = world.enemies.get_mut(&assistant.id) {
            unit.velocity_x = 0.0;
            unit.velocity_y = 0.0;
            unit.rotation = (target_y - unit.y).atan2(target_x - unit.x).to_degrees();
        }
        let work = build_speed * delta_ticks.max(0.0);
        match target {
            AssistTarget::Pending(position) => {
                if let Some(mut unit) = world.enemies.get_mut(&assistant.id) {
                    if !unit
                        .build_plans
                        .iter()
                        .any(|plan| plan.position == position)
                    {
                        if let Some(build) = world.pending_builds.get(&position) {
                            unit.build_plans.insert(
                                0,
                                crate::network::world::UnitBuildPlan {
                                    breaking: false,
                                    position,
                                    block: build.block,
                                    rotation: build.rotation,
                                    config: build.config.clone(),
                                },
                            );
                        }
                    }
                }
                if let Some(mut build) = world.pending_builds.get_mut(&position) {
                    build.assist_progress += work;
                    changed = true;
                }
            }
            AssistTarget::Rebuild(position) => {
                if let Some(mut unit) = world.enemies.get_mut(&assistant.id) {
                    if !unit
                        .build_plans
                        .iter()
                        .any(|plan| plan.position == position)
                    {
                        if let Some((block, rotation, _, config)) = world
                            .tiles
                            .get(&position)
                            .and_then(|tile| rebuild_plan(&tile))
                        {
                            unit.build_plans.insert(
                                0,
                                crate::network::world::UnitBuildPlan {
                                    breaking: false,
                                    position,
                                    block,
                                    rotation,
                                    config,
                                },
                            );
                        }
                    }
                }
                if let Some(mut tile) = world.tiles.get_mut(&position) {
                    if tile.production_progress > 0.0 && rebuild_plan(&tile).is_some() {
                        tile.production_progress += work;
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

pub fn simulate_builder_units(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let builders: Vec<_> = world
        .enemies
        .iter()
        .filter_map(|unit| {
            let speed = effective_unit_build_speed(&unit)?;
            let command = world
                .unit_orders
                .get(&unit.id)
                .map(|order| order.command)
                .unwrap_or_else(|| default_unit_command(unit.unit_type));
            (unit.team == 1
                && command == 2
                && unit.update_building
                && !unit_is_player_controlled(world, unit.id)
                && !unit_bound_to_logic(world, unit.id))
            .then(|| (unit.clone(), speed))
        })
        .collect();
    let mut changed = false;
    // P0-5: Rules.enemyCoreBuildRadius(team) (default 400): AI builders of
    // the player team never build inside the radius of an enemy (wave team)
    // core (official `anyEnemyCoresWithin` gates in AI/Logic). The radius is
    // the builder team's protected radius (0 when the team does not protect
    // cores); snapshot the enemy core positions once per tick.
    let rules = world.wave_rules.read();
    let enemy_core_radius = rules.enemy_core_radius_for(1);
    let wave_team = rules.wave_team;
    drop(rules);
    let enemy_cores: Vec<(f32, f32)> = world
        .team_core_lists
        .iter()
        .filter(|entry| *entry.key() == wave_team)
        .flat_map(|entry| {
            entry
                .value()
                .iter()
                .map(|core| {
                    (
                        (core.position >> 16) as i16 as f32 * 8.0,
                        core.position as i16 as f32 * 8.0,
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for (builder, build_speed) in builders {
        if let Some(mut unit) = world.enemies.get_mut(&builder.id) {
            unit.build_plans.retain(|plan| {
                if plan.breaking {
                    return world
                        .tiles
                        .get(&plan.position)
                        .is_some_and(|tile| tile.block != 0)
                        || world.base_buildings.contains_key(&plan.position);
                }
                world.pending_builds.contains_key(&plan.position)
                    || world
                        .tiles
                        .get(&plan.position)
                        .is_some_and(|tile| rebuild_plan(&tile).is_some())
            });
        }
        let planned: Vec<i32> = world
            .enemies
            .get(&builder.id)
            .map(|unit| {
                unit.build_plans
                    .iter()
                    .filter(|plan| !plan.breaking)
                    .map(|plan| plan.position)
                    .collect()
            })
            .unwrap_or_default();
        let target = world
            .tiles
            .iter()
            .filter_map(|tile| {
                if !planned.is_empty() && !planned.contains(&tile.position) {
                    return None;
                }
                let (block, _, _, _) = rebuild_plan(&tile)?;
                has_requirements(&world.game_state, block).then(|| {
                    let x = (tile.position >> 16) as i16 as f32 * 8.0;
                    let y = tile.position as i16 as f32 * 8.0;
                    ((x - builder.x).hypot(y - builder.y), tile.position, x, y)
                })
            })
            .filter(|(_, _, x, y)| {
                enemy_cores
                    .iter()
                    .all(|(cx, cy)| (x - cx).hypot(y - cy) >= enemy_core_radius)
            })
            .min_by(|left, right| left.0.total_cmp(&right.0));
        let Some((distance, position, target_x, target_y)) = target else {
            continue;
        };
        if distance > unit_build_range(builder.unit_type) {
            if !unit_has_stance(world, builder.id, 6) {
                changed |= move_unit_toward(world, builder.id, target_x, target_y, delta_ticks);
            }
            continue;
        }
        if let Some(mut unit) = world.enemies.get_mut(&builder.id) {
            unit.velocity_x = 0.0;
            unit.velocity_y = 0.0;
            unit.rotation = (target_y - unit.y).atan2(target_x - unit.x).to_degrees();
            if unit.build_plans.is_empty() {
                if let Some((block, rotation, _, config)) = world
                    .tiles
                    .get(&position)
                    .and_then(|tile| rebuild_plan(&tile))
                {
                    unit.build_plans.push(crate::network::world::UnitBuildPlan {
                        breaking: false,
                        position,
                        block,
                        rotation,
                        config,
                    });
                }
            }
        }
        let completed = if let Some(mut tile) = world.tiles.get_mut(&position) {
            let Some((block, rotation, team, config)) = rebuild_plan(&tile) else {
                continue;
            };
            let starting = tile.production_progress <= f32::EPSILON;
            tile.production_progress += build_speed * delta_ticks.max(0.0);
            changed = true;
            if starting {
                if let Ok(payload) = encode_begin_place_for_unit(
                    builder.id, position, block, rotation, team, &config,
                ) {
                    if let Ok(frame) =
                        frame_generated_packet(BEGIN_PLACE_PACKET_ID, &payload, false)
                    {
                        out.broadcast(frame);
                    }
                }
            }
            (tile.production_progress >= crate::game::content::block_build_time(block))
                .then(|| (block, rotation, team, config, tile.occupied.clone()))
        } else {
            None
        };
        let Some((block, rotation, team, config, occupied)) = completed else {
            continue;
        };
        if !consume_requirements(&world.game_state, 1, block) {
            continue;
        }
        if let Some(template) = world
            .base_building_templates
            .iter()
            .find(|template| {
                template.position == position && template.block == block && template.team == team
            })
            // Powered rebuilds must stay in the dynamic authoritative registry:
            // placement lifecycle and the server PowerGraph both consume it.
            .filter(|_| power_role(block).is_none())
        {
            world.tiles.remove(&position);
            world.base_buildings.insert(
                position,
                BaseBuildingState {
                    health: crate::game::content::block_health(block),
                    inventory: Vec::new(),
                    ..template.clone()
                },
            );
        } else {
            world.tiles.insert(
                position,
                DynamicTile {
                    position,
                    block,
                    rotation,
                    team,
                    config: config.clone(),
                    enabled: true,
                    message: None,
                    occupied,
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
                    health: crate::game::content::block_health(block),
                    door_open: false,
                    shield: 0.0,
                    light_color: -1_900_545,
                    memory: crate::network::economy::memory_capacity(block)
                        .map(|capacity| vec![0.0; capacity])
                        .unwrap_or_default(),
                    duct_rec_dir: 0,
                    unloader_offset: 0,
                    conveyor_items: Vec::new(),
                    factory_command: None,
                    stack_state: 0,
                    stack_link: -1,
                    stack_cooldown: 0.0,
                    generation: 0,
                },
            );
        }
        let placement_changes = building_placement::after_placement(world, position, &config);
        let final_config = world
            .tiles
            .get(&position)
            .map(|tile| tile.config.clone())
            .unwrap_or(config);
        invalidate_navigation_for_block(world, block);
        if let Ok(payload) = encode_construct_finish_for_unit(
            builder.id,
            position,
            block,
            rotation,
            team,
            &final_config,
        ) {
            if let Ok(frame) = frame_generated_packet(CONSTRUCT_FINISH_PACKET_ID, &payload, false) {
                out.broadcast(frame);
            }
        }
        let actor_id = world
            .player_sessions
            .iter()
            .next()
            .map(|session| session.id)
            .unwrap_or(1);
        let _ = broadcast_placement_power_configs(out, actor_id, &placement_changes);
    }
    changed
}

pub fn simulate_support_units(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let ids: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| {
            unit.team == 1
                && matches!(unit.unit_type, 5 | 20 | 21 | 22 | 30 | 32)
                && !unit_is_player_controlled(world, unit.id)
        })
        .map(|unit| unit.id)
        .collect();
    let simulation_time = *world.game_state.simulation_time.read();
    let crossed = |period: f32| {
        (simulation_time / period).floor()
            > ((simulation_time - delta_ticks.max(0.0)).max(0.0) / period).floor()
    };
    let mut changed = false;
    for id in ids {
        let Some(snapshot) = world.enemies.get(&id).map(|unit| unit.clone()) else {
            continue;
        };
        let command = world
            .unit_orders
            .get(&id)
            .map(|order| order.command)
            .unwrap_or_else(|| default_unit_command(snapshot.unit_type));
        if command == 1 {
            let repair_range = match snapshot.unit_type {
                21 => 45.0,
                22 => 170.0,
                _ => 0.0,
            };
            if repair_range > 0.0 && !unit_has_stance(world, id, 6) {
                changed |=
                    move_repair_unit(world, id, snapshot.x, snapshot.y, repair_range, delta_ticks);
            }
        }
        match snapshot.unit_type {
            20 if world
                .unit_orders
                .get(&id)
                .is_none_or(|order| order.command == 4) =>
            {
                changed |= simulate_mono_mining(world, out, id, delta_ticks);
            }
            5 if crossed(240.0) => {
                changed |= heal_buildings_in_radius(world, out, snapshot.x, snapshot.y, 60.0, 10.0);
            }
            21 if crossed(480.0) => {
                changed |= heal_buildings_in_radius(world, out, snapshot.x, snapshot.y, 50.0, 5.0);
            }
            22 => {
                if let Some(mut unit) = world.enemies.get_mut(&id) {
                    unit.secondary_attack_reload += delta_ticks;
                    if unit.secondary_attack_reload >= 15.0 {
                        unit.secondary_attack_reload %= 15.0;
                        drop(unit);
                        changed |=
                            heal_nearest_building(world, out, snapshot.x, snapshot.y, 182.0, 5.5);
                    }
                }
            }
            30 => {
                changed |= heal_nearest_building_flat(
                    world,
                    out,
                    snapshot.x,
                    snapshot.y,
                    120.0,
                    0.75 * delta_ticks.max(0.0),
                );
            }
            32 => {
                changed |= heal_nearest_building_flat(
                    world,
                    out,
                    snapshot.x,
                    snapshot.y,
                    130.0,
                    0.7 * delta_ticks.max(0.0),
                );
            }
            _ => {}
        }
    }
    changed
}

pub fn simulate_mono_mining(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    unit_id: i32,
    delta_ticks: f32,
) -> bool {
    let Some(snapshot) = world.enemies.get(&unit_id).map(|unit| unit.clone()) else {
        world.mono_mining_targets.remove(&unit_id);
        return false;
    };
    let carried_amount = snapshot.secondary_attack_reload.max(0.0).round() as i32;
    let carried_item = snapshot.tertiary_attack_reload.round() as i16 - 1;
    let team = snapshot.team;
    let (core_x, core_y) = core_world_for_team(world, team);
    if carried_amount >= 30 {
        // Depositing: drop the cached target so the next mining pass
        // re-acquires ore with the reset item filter.
        world.mono_mining_targets.remove(&unit_id);
        let distance = (core_x - snapshot.x).hypot(core_y - snapshot.y);
        if distance <= 50.0 {
            // Official MinerAI deposit (bytecode 158.1): when within
            // `unit.type.range` (50 u) of the core, hand the whole stack
            // over and emit Call.transferItemTo so the client animates the
            // items flying from the unit into the core. The legacy port
            // deposited silently at 30 u, which made the drop invisible and
            // the mono hover over the core.
            let accepted = crate::network::core_inventory::deposit_core_items(
                world,
                team,
                carried_item,
                carried_amount,
            );
            if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
                unit.attack_reload = 0.0;
                unit.secondary_attack_reload = 0.0;
                unit.tertiary_attack_reload = 0.0;
                unit.velocity_x = 0.0;
                unit.velocity_y = 0.0;
            }
            // MinerAI always clears the carried stack on arrival. It emits
            // TransferItemTo only if the core can accept at least one item;
            // CoreBuild.handleStack then stores at most the remaining space.
            if accepted > 0 {
                if let Ok(frame) = crate::network::wire::encode_transfer_item_to_frame(
                    unit_id,
                    carried_item.max(0),
                    carried_amount,
                    snapshot.x,
                    snapshot.y,
                    crate::network::world::core_position_for_team(world, team),
                ) {
                    out.broadcast(frame);
                }
            }
            return true;
        }
        return move_unit_toward(world, unit_id, core_x, core_y, delta_ticks);
    }
    // Round 74 fix: the nearest-ore search used to run for every mining unit
    // every tick; with ore far away or absent it took seconds per tick and
    // the 158.1 client disconnected after 20 s without snapshots. Now the
    // target is cached in `world.mono_mining_targets` and only re-validated
    // with O(1) lookups; a fresh scan runs when the cached tile is built
    // over or the item no longer matches, and failed scans back off 60 ticks.
    let desired_item = if carried_item >= 0 {
        Some(carried_item)
    } else {
        mono_target_item(world, team)
    };
    let Some(desired_item) = desired_item else {
        world.mono_mining_targets.insert(unit_id, (0, 60.0));
        return false;
    };
    let mut target: Option<(i32, i16, u8, f32, f32)> = None;
    if let Some((position, cooldown)) = world.mono_mining_targets.get(&unit_id).map(|entry| *entry)
    {
        if position != 0 {
            if let Some((item, hardness)) = raw_mine_result(world, position) {
                if hardness <= 1 && item == desired_item && effective_block(world, position) == 0 {
                    target = Some((
                        position,
                        item,
                        hardness,
                        (position >> 16) as i16 as f32 * 8.0,
                        position as i16 as f32 * 8.0,
                    ));
                }
            }
        } else if cooldown > 0.0 {
            world
                .mono_mining_targets
                .insert(unit_id, (0, cooldown - delta_ticks.max(0.0)));
            return false;
        }
    }
    if target.is_none() {
        // Official MinerAI searches from the closest core, and Mono's
        // default mineItems list filtered by mineTier=1 is copper/lead.
        match nearest_mineable_ore(world, core_x, core_y, desired_item) {
            Some((position, item, hardness, target_x, target_y)) => {
                world.mono_mining_targets.insert(unit_id, (position, 0.0));
                target = Some((position, item, hardness, target_x, target_y));
            }
            None => {
                world.mono_mining_targets.insert(unit_id, (0, 60.0));
                return false;
            }
        }
    }
    let (_position, item, hardness, target_x, target_y) = target.unwrap();
    let distance = (target_x - snapshot.x).hypot(target_y - snapshot.y);
    // Official mono.mineRange = 70 u: the mono flies to the ore and only
    // mines (beam on, mineTile set in the snapshot) once it is in range.
    if distance > 70.0 {
        return move_unit_toward(world, unit_id, target_x, target_y, delta_ticks);
    }
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.velocity_x = 0.0;
        unit.velocity_y = 0.0;
        unit.attack_reload += delta_ticks.max(0.0) * 2.5;
        if unit.attack_reload >= 50.0 + f32::from(hardness) * 15.0 {
            unit.attack_reload = 0.0;
            unit.secondary_attack_reload = (carried_amount + 1) as f32;
            unit.tertiary_attack_reload = f32::from(item + 1);
        }
        unit.rotation = (target_y - unit.y).atan2(target_x - unit.x).to_degrees();
    }
    true
}

/// Vanilla Mono target selection: from its allowed mine items, choose the
/// least-stocked item that exists on this map. Mono mineTier=1 leaves copper
/// and lead; floor sand and scrap are not in `UnitType.mineItems`.
pub(crate) fn mono_target_item(world: &DynamicWorld, team: u8) -> Option<i16> {
    let stored = crate::network::economy::items_for_team(world, team);
    let capacity = crate::network::core_inventory::core_item_capacity(world, team);
    let incinerates = world.wave_rules.read().core_incinerates;
    [0i16, 1i16]
        .into_iter()
        .filter(|item| {
            world.mineable_ore.get().map_or_else(
                || {
                    (0..world.width * world.height).any(|index| {
                        let x = index % world.width;
                        let y = index / world.width;
                        raw_mine_result(world, (x << 16) | y)
                            .is_some_and(|(found, hardness)| found == *item && hardness <= 1)
                    })
                },
                |index| index.per_item.get(*item as usize).copied().unwrap_or(0) > 0,
            )
        })
        .filter(|item| incinerates || stored.get(*item as usize).copied().unwrap_or(0) < capacity)
        .min_by_key(|item| stored.get(*item as usize).copied().unwrap_or(0))
}
