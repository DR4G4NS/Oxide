//! Unit movement physics, LogicAI movement, possession and control switch.
//! Units facade re-exports through crate::network::units::*.

use crate::network::wire::auth::player_team;
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

pub(crate) fn unit_move_physics(unit_type: i16) -> (f32, f32, f32, bool, f32) {
    // (speed, accel, drag, omni_movement, rotate_speed)
    match unit_type {
        15 => (FLARE.speed, 0.08, 0.04, false, 5.0),
        0 => (DAGGER.speed, 0.5, 0.3, true, 10.0),
        _ => (1.0, 0.5, 0.3, true, 10.0),
    }
}

pub(crate) fn unit_move_toward_angle(current: f32, target: f32, max_delta: f32) -> f32 {
    let mut delta = (target - current) % 360.0;
    if delta > 180.0 {
        delta -= 360.0;
    } else if delta < -180.0 {
        delta += 360.0;
    }
    if delta.abs() <= max_delta {
        target
    } else {
        current + delta.signum() * max_delta
    }
}

pub(crate) fn logic_move_at(
    unit: &mut EnemyUnit,
    target_vx: f32,
    target_vy: f32,
    delta_ticks: f32,
) {
    let (_, accel, _, _, _) = unit_move_physics(unit.unit_type);
    let target_len = target_vx.hypot(target_vy);
    let dvx = target_vx - unit.velocity_x;
    let dvy = target_vy - unit.velocity_y;
    let limit = accel * target_len * delta_ticks;
    let change_len = dvx.hypot(dvy).min(limit);
    if change_len > 0.0 {
        let scale = change_len / dvx.hypot(dvy).max(0.0001);
        unit.velocity_x += dvx * scale;
        unit.velocity_y += dvy * scale;
    }
}

/// UnitComp.rotateMove (UnitComp.java:146-152) for `!omniMovement` types.
pub(crate) fn logic_rotate_move(
    unit: &mut EnemyUnit,
    desired_vx: f32,
    desired_vy: f32,
    delta_ticks: f32,
) {
    let (_, _, _, _, rotate_speed) = unit_move_physics(unit.unit_type);
    let desired_len = desired_vx.hypot(desired_vy);
    if desired_len <= 0.0001 {
        return;
    }
    let rot_rad = unit.rotation.to_radians();
    logic_move_at(
        unit,
        rot_rad.cos() * desired_len,
        rot_rad.sin() * desired_len,
        delta_ticks,
    );
    let target_angle = desired_vy.atan2(desired_vx).to_degrees();
    unit.rotation = unit_move_toward_angle(unit.rotation, target_angle, rotate_speed * delta_ticks);
}

/// VelComp.update() (VelComp.java:21-33): integrate existing velocity,
/// zero blocked axes, apply drag. Runs before LogicAI.updateMovement.
pub(crate) fn integrate_unit_velocity(unit: &mut EnemyUnit, unit_type: i16, delta_ticks: f32) {
    let (_, _, drag, _, _) = unit_move_physics(unit_type);
    let px = unit.x;
    let py = unit.y;
    unit.x += unit.velocity_x * delta_ticks;
    unit.y += unit.velocity_y * delta_ticks;
    if (unit.x - px).abs() <= 0.0001 {
        unit.velocity_x = 0.0;
    }
    if (unit.y - py).abs() <= 0.0001 {
        unit.velocity_y = 0.0;
    }
    let scale = (1.0 - drag * delta_ticks).max(0.0);
    unit.velocity_x *= scale;
    unit.velocity_y *= scale;
}

/// LogicAI `case move` → `moveTo(dest, 1f, 30f)` + UnitComp.moveAt.
pub(crate) fn logic_accelerate_toward(
    unit: &mut EnemyUnit,
    target_x: f32,
    target_y: f32,
    delta_ticks: f32,
) {
    let (speed, _, _, omni, _) = unit_move_physics(unit.unit_type);
    let dx = target_x - unit.x;
    let dy = target_y - unit.y;
    let distance = dx.hypot(dy);
    if distance <= 0.001 {
        return;
    }
    let circle_length = 1.0_f32;
    let smooth = 30.0_f32;
    let length = if circle_length <= 0.001 {
        1.0
    } else {
        ((distance - circle_length) / smooth).clamp(-1.0, 1.0)
    };
    if length <= 0.0 {
        return;
    }
    let desired_x = dx / distance * speed * length;
    let desired_y = dy / distance * speed * length;
    if omni {
        logic_move_at(unit, desired_x, desired_y, delta_ticks);
    } else {
        logic_rotate_move(unit, desired_x, desired_y, delta_ticks);
    }
}

/// One LogicAI movement pass: VelComp integration then control-mode switch.
pub(crate) fn apply_logic_unit_movement(
    world: &DynamicWorld,
    snapshot: &EnemyUnit,
    delta_ticks: f32,
) -> bool {
    if !unit_bound_to_logic(world, snapshot.id) {
        return false;
    }
    let Some(order) = world
        .unit_orders
        .get(&snapshot.id)
        .map(|order| order.clone())
    else {
        return false;
    };
    let Some(mut unit) = world.enemies.get_mut(&snapshot.id) else {
        return false;
    };
    integrate_unit_velocity(&mut unit, snapshot.unit_type, delta_ticks);
    match order.logic_control {
        crate::network::world::logic_control::MOVE => {
            if let (Some(target_x), Some(target_y)) = (order.target_x, order.target_y) {
                logic_accelerate_toward(&mut unit, target_x, target_y, delta_ticks);
            }
        }
        crate::network::world::logic_control::PATHFIND => {
            if let (Some(target_x), Some(target_y)) = (order.target_x, order.target_y) {
                drop(unit);
                let snapshot = snapshot.clone();
                let result = crate::network::units::unit_orders::ordered_unit_path(
                    world, &snapshot, target_x, target_y,
                );
                if result.should_move {
                    if let Some(mut unit) = world.enemies.get_mut(&snapshot.id) {
                        logic_accelerate_toward(
                            &mut unit,
                            result.dest_x,
                            result.dest_y,
                            delta_ticks,
                        );
                    }
                }
                return true;
            }
        }
        crate::network::world::logic_control::STOP | crate::network::world::logic_control::IDLE => {
        }
        _ => {}
    }
    true
}

/// Snapshot fields comparable to ParLogicMoveTiming158.
pub(crate) type LogicMovementSnapshot = (
    f32,
    f32,
    f32,
    f32,
    &'static str,
    Option<f32>,
    Option<f32>,
    bool,
    bool,
);

pub(crate) fn logic_movement_snapshot(
    world: &DynamicWorld,
    unit_id: i32,
) -> Option<LogicMovementSnapshot> {
    let unit = world.enemies.get(&unit_id)?;
    let order = world.unit_orders.get(&unit_id);
    let is_logic = matches!(unit.authority, UnitAuthority::Logic { .. });
    let (control, move_x, move_y) = if is_logic {
        let control = match order.as_ref().map(|o| o.logic_control).unwrap_or(0) {
            crate::network::world::logic_control::STOP => "stop",
            crate::network::world::logic_control::MOVE => "move",
            _ => "idle",
        };
        let (mx, my) = match order.as_ref().map(|o| o.logic_control).unwrap_or(0) {
            crate::network::world::logic_control::MOVE => {
                let o = order.as_ref().unwrap();
                (o.target_x, o.target_y)
            }
            crate::network::world::logic_control::STOP => (Some(0.0), Some(0.0)),
            _ => (Some(0.0), Some(0.0)),
        };
        (control, mx, my)
    } else {
        ("none", None, None)
    };
    let processor_valid = match unit.authority {
        UnitAuthority::Logic {
            processor_pos,
            processor_generation,
            ..
        } => processor_lease_valid(world, processor_pos, processor_generation),
        _ => false,
    };
    Some((
        unit.x,
        unit.y,
        unit.velocity_x,
        unit.velocity_y,
        control,
        move_x,
        move_y,
        is_logic,
        processor_valid,
    ))
}

/// Performs the complete `PlayerComp.unit(newUnit)` transition as one
/// authoritative operation. The unusual 158.1 ordering is observable:
/// the incoming CommandAI's command is saved first, then that value is used
/// while restoring the old unit, and only then is the incoming controller
/// replaced by Player. Splitting A→B into release(A) + acquire(B) reverses
/// those first two steps and restores the wrong command to A.
///
/// `new_unit = None` mirrors `clearUnit()`. Queue, active target and stances
/// belong to the discarded CommandAI object and are never restored.
pub(crate) fn switch_player_unit(
    world: &DynamicWorld,
    player: &mut SessionPlayer,
    new_unit: Option<i32>,
) {
    let old_unit = player.controlled_unit.standard_id();
    if old_unit == new_unit {
        return;
    }

    // Step 1: inspect and save the INCOMING controller before touching A.
    // Authority, not team/type defaults, answers which controller object is
    // installed now (a LogicAI on a commandable unit is not a CommandAI).
    let incoming = new_unit.and_then(|id| world.enemies.get(&id).map(|unit| unit.clone()));
    if new_unit.is_some() && incoming.is_none() {
        return;
    }
    let incoming_was_command_ai = incoming
        .as_ref()
        .is_some_and(|unit| unit.authority == UnitAuthority::Command);
    if let Some(incoming) = incoming.as_ref().filter(|_| incoming_was_command_ai) {
        player.last_command = Some(
            world
                .unit_orders
                .get(&incoming.id)
                .map(|order| order.command)
                .unwrap_or_else(|| {
                    crate::network::economy::default_unit_command(incoming.unit_type)
                }),
        );
    }

    // Steps 2-3: reset A to a fresh default controller, then apply the
    // player slot saved from B. CommandAI.command rejects unsupported
    // commands, so the fresh controller keeps its type default in that case.
    if let Some(old_id) = old_unit {
        let old = world.enemies.get(&old_id).map(|unit| unit.clone());
        if let Some(old) = old {
            let default = default_unit_authority(world, &old);
            if let Some(mut live) = world.enemies.get_mut(&old_id) {
                live.authority = default;
            }
            if default == UnitAuthority::Command {
                let mut command = crate::network::economy::default_unit_command(old.unit_type);
                if let Some(saved) = player.last_command.filter(|saved| {
                    crate::network::decoders::unit_allows_command(old.unit_type, *saved)
                }) {
                    command = saved;
                }
                world
                    .unit_orders
                    .insert(old_id, fresh_command_order(old_id, command));
            } else {
                world.unit_orders.remove(&old_id);
            }
        }
    }

    // Step 4: Player.unit now points at B (or null).
    player.controlled_unit = new_unit.map_or(ControlledUnit::Core, ControlledUnit::Standard);

    let Some(incoming) = incoming else {
        return;
    };

    // Step 5: team adoption precedes controller replacement.
    let player_team = crate::network::wire::auth::player_team(world, player);
    if let Some(mut live) = world.enemies.get_mut(&incoming.id) {
        live.team = player_team;
    }

    // Step 6: replacing a CommandAI drops the whole old object, not just its
    // active command.
    if incoming_was_command_ai {
        if let Some(mut order) = world.unit_orders.get_mut(&incoming.id) {
            clear_order_active_target(&mut order);
            order.queue.clear();
            order.stances = 0;
            order.payload_cooldown = 0.0;
        }
    }
    if let Some(mut live) = world.enemies.get_mut(&incoming.id) {
        live.authority = UnitAuthority::Player {
            player_id: player.id,
        };
    }
}

/// Marks the unit as held by RTS orders (`CommandAI` with an active
/// target). Possession wins: a possessed unit keeps Player authority even
/// though a command packet may still overwrite its order, mirroring Java's
/// `commandUnits` acting only on `CommandAI` controllers.
pub(crate) fn acquire_command_control(world: &DynamicWorld, unit_id: i32) {
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        if !matches!(unit.authority, UnitAuthority::Player { .. }) {
            unit.authority = UnitAuthority::Command;
        }
    }
}

/// Drops Command authority once no active RTS target remains (exhausted
/// queue, SetUnitCommand target reset, stop stance). Player and Logic
/// authority are never clobbered by order churn.
pub(crate) fn release_command_control(world: &DynamicWorld, unit_id: i32) {
    // dashmap-guard: allow DM900 reason="default_unit_authority reads wave rules and game state only; it does not access world.enemies"
    let default = world
        .enemies
        .get(&unit_id)
        .filter(|unit| unit.authority == UnitAuthority::Command)
        .map(|unit| default_unit_authority(world, &unit));
    if let (Some(mut unit), Some(default)) = (world.enemies.get_mut(&unit_id), default) {
        unit.authority = default;
    }
}

/// Possession resolved from BOTH sources: the live session table (which
/// carries the authoritative player id for the wire) and the unit's Player
/// authority (which survives transient session races). The unit counts as
/// possessed when either source says so.
pub(crate) fn unit_possessed_by(world: &DynamicWorld, unit_id: i32) -> Option<i32> {
    crate::network::units::controller::controlling_player_for_unit(world, unit_id).or_else(|| {
        world
            .enemies
            .get(&unit_id)
            .and_then(|unit| match unit.authority {
                UnitAuthority::Player { player_id } => Some(player_id),
                _ => None,
            })
    })
}

/// Drops every control association of a unit that is leaving the world
/// (death, kamikaze, payload pickup/entry): its order is removed and a
/// possessing session is returned to its core avatar. The authority field
/// lives on the unit, so it disappears with the `enemies` entry itself.
pub(crate) fn detach_unit_control(world: &DynamicWorld, unit_id: i32) {
    world.unit_orders.remove(&unit_id);
    for mut session in world.player_sessions.iter_mut() {
        if session.controlled_unit.standard_id() == Some(unit_id) {
            session.controlled_unit = ControlledUnit::Core;
        }
    }
}
