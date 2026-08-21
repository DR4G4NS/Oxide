//! ClientSnapshot apply + bounded (non-strict/strict) movement and mining
//! update. The listener adapter re-exports these through
//! crate::network::listener::*.

use crate::network::decoders::ClientSnapshot;
use crate::network::world::{ControlledUnit, DynamicWorld, PlayerCombatState, SessionPlayer};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::network::buildings::construction::{dynamic_at, effective_block};
use crate::network::units::controller::valid_item_stack;
use crate::network::wire::transfer::broadcast_player_snapshot;
use crate::network::wire::transfer::encode_transfer_item_to_frame;
use crate::network::world::{core_position_for_team, core_world_for_team};

/// M3: official NetServer.clientSnapshot (desktop 158.1 bytecode offsets
/// 645-895) — in non-strict mode the server accepts the (float-sanitized)
/// client position; in strict mode the movement is limited to
/// `min(elapsedMs, 1500)/1000 * 60 * unit.speed() * 1.1` world units
/// (alpha `UnitType.speed` = 3.0, not tiles), and when the client is
/// further than `correctDist` (112 units, `Mathf.within(...,112)` at
/// offsets 868-893) a SetPositionCallPacket (110) correction is returned.
/// Boosting is forced off for units that cannot boost (alpha: flying=false,
/// canBoost=false; bytecode offsets 214-249).
/// Returns the authoritative position to send via SetPosition, if any.
pub(crate) fn apply_client_snapshot(
    player: &mut SessionPlayer,
    snapshot: &ClientSnapshot,
    strict: bool,
    elapsed_ms: u64,
) -> Option<(f32, f32)> {
    const ALPHA_SPEED: f32 = 3.0; // UnitTypes.alpha speed, world units/tick
    apply_client_snapshot_with_speed(player, snapshot, strict, elapsed_ms, ALPHA_SPEED, false)
}

pub(crate) fn apply_client_snapshot_with_speed(
    player: &mut SessionPlayer,
    snapshot: &ClientSnapshot,
    strict: bool,
    elapsed_ms: u64,
    unit_speed: f32,
    can_boost: bool,
) -> Option<(f32, f32)> {
    const CORRECT_DIST: f32 = 112.0;
    player.last_snapshot = snapshot.snapshot_id;
    if snapshot.dead {
        return None;
    }
    let dx = snapshot.x - player.x;
    let dy = snapshot.y - player.y;
    let distance = dx.hypot(dy);
    let mut corrected = None;
    if strict {
        // Official cap: `elapsed = Math.min(timeSinceMillis, 1500)`.
        let elapsed = (elapsed_ms as f32 / 1000.0).min(1.5);
        let max_move = elapsed * 60.0 * unit_speed.max(0.0) * 1.1;
        if distance <= max_move {
            player.x = snapshot.x;
            player.y = snapshot.y;
        } else if distance > 0.0 {
            let scale = max_move / distance;
            player.x += dx * scale;
            player.y += dy * scale;
        }
        // `!Mathf.within(x, y, unit.x, unit.y, correctDist)` ->
        // `Call.setPosition(con, unit.x, unit.y)`.
        if distance * distance >= CORRECT_DIST * CORRECT_DIST {
            corrected = Some((player.x, player.y));
        }
    } else {
        player.x = snapshot.x;
        player.y = snapshot.y;
    }
    player.mouse_x = snapshot.mouse_x;
    player.mouse_y = snapshot.mouse_y;
    player.rotation = snapshot.rotation.rem_euclid(360.0);
    // Official: `if (!dead && (!type.flying || !type.canBoost)) boosting = 0`
    // — unconditional in clientSnapshot; alpha cannot boost (bytecode
    // offsets 214-249).
    player.boosting = !snapshot.dead && can_boost && snapshot.boosting;
    player.shooting = snapshot.shooting;
    corrected
}

/// Applies a ClientSnapshot to the unit currently exposed by Player.writeSync.
/// Standard possessed units update their authoritative EnemyUnit transform;
/// block proxy units accept aim/shoot state but remain fixed to their building.
pub(crate) fn apply_controlled_client_snapshot(
    player: &mut SessionPlayer,
    snapshot: &ClientSnapshot,
    world: &DynamicWorld,
    strict: bool,
    elapsed_ms: u64,
) -> Option<(f32, f32)> {
    match player.controlled_unit {
        ControlledUnit::Core => apply_client_snapshot(player, snapshot, strict, elapsed_ms),
        ControlledUnit::Standard(unit_id) => {
            // For possessed units this timestamp is the manual-input lease;
            // unlike Alpha, their reload is tick-driven in simulation.rs.
            player.last_shot = std::time::Instant::now();
            let unit = world.enemies.get(&unit_id)?.clone();
            let can_boost = matches!(unit.unit_type, 5..=8);
            let correction = apply_client_snapshot_with_speed(
                player,
                snapshot,
                strict,
                elapsed_ms,
                unit.move_speed,
                can_boost,
            );
            if !snapshot.dead {
                if let Some(mut controlled) = world.enemies.get_mut(&unit_id) {
                    controlled.x = player.x;
                    controlled.y = player.y;
                    controlled.rotation = player.rotation;
                    controlled.velocity_x = 0.0;
                    controlled.velocity_y = 0.0;
                }
            }
            correction
        }
        ControlledUnit::Building(position) => {
            player.last_shot = std::time::Instant::now();
            player.last_snapshot = snapshot.snapshot_id;
            player.mouse_x = snapshot.mouse_x;
            player.mouse_y = snapshot.mouse_y;
            player.rotation = snapshot.rotation.rem_euclid(360.0);
            player.boosting = false;
            player.shooting = !snapshot.dead && snapshot.shooting;
            if let Some(tile) = dynamic_at(world, position) {
                player.x = (tile.position >> 16) as i16 as f32 * 8.0;
                player.y = tile.position as i16 as f32 * 8.0;
            }
            None
        }
    }
}

pub(crate) fn update_mining(
    player: &mut SessionPlayer,
    snapshot: &ClientSnapshot,
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
) -> std::io::Result<()> {
    const ALPHA_MINE_SPEED: f32 = 6.5;
    const ALPHA_MINE_RANGE: f32 = 70.0;
    const ALPHA_ITEM_CAPACITY: i32 = 30;
    const CORE_TRANSFER_RANGE: f32 = 220.0;

    let now = std::time::Instant::now();
    let elapsed = now
        .duration_since(player.mining_updated)
        .as_secs_f32()
        .min(0.25);
    player.mining_updated = now;
    let Some(position) = snapshot.mining_position else {
        player.mining_position = None;
        player.mining_progress = 0.0;
        return Ok(());
    };
    if !snapshot.plans.is_empty() {
        return Ok(());
    }
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    if x < 0
        || y < 0
        || x >= world.width
        || y >= world.height
        || (x as f32 * 8.0 - player.x).hypot(y as f32 * 8.0 - player.y) > ALPHA_MINE_RANGE
    {
        player.mining_position = None;
        player.mining_progress = 0.0;
        return Ok(());
    }
    let Some((item, hardness)) = mine_result(world, position) else {
        player.mining_position = None;
        player.mining_progress = 0.0;
        return Ok(());
    };
    // The player mines into THEIR OWN team's core (official per-team
    // `TeamData.items`; survival/attack is team 1).
    let player_team = world
        .players
        .get(&player.unit_id)
        .map(|combat| combat.team)
        .unwrap_or(1);
    let (core_x, core_y) = core_world_for_team(world, player_team);
    let within_core = (player.x - core_x).hypot(player.y - core_y) <= CORE_TRANSFER_RANGE;

    // The official server offloads a player's current stack to a nearby core.
    // Without this transition, mining lead once permanently prevented selecting
    // copper (and vice versa) while standing in the core's transfer radius.
    if within_core && player.carried_amount > 0 && player.carried_item != item {
        let (stack_item, stack_amount) =
            valid_item_stack(player.carried_item, player.carried_amount);
        if stack_amount > 0 {
            crate::network::core_inventory::deposit_core_items(
                world,
                player_team,
                stack_item,
                stack_amount,
            );
        }
        player.carried_item = -1;
        player.carried_amount = 0;
    }
    if player.carried_amount >= ALPHA_ITEM_CAPACITY
        || (player.carried_amount > 0 && player.carried_item != item)
    {
        return Ok(());
    }
    if player.mining_position != Some(position) {
        player.mining_position = Some(position);
        player.mining_progress = 0.0;
    }
    // Official MinerComp: mine speed scales with
    // `state.rules.unitMineSpeedMultiplier` times the player team's
    // TeamRule.unitMineSpeedMultiplier (Rules.java:378/394).
    let mine_speed = world
        .wave_rules
        .read()
        .unit_mine_speed_multiplier
        .max(0.0001)
        * world
            .wave_rules
            .read()
            .team_rule(player_team)
            .unit_mine_speed_multiplier
            .max(0.0001);
    player.mining_progress += elapsed * 60.0 * ALPHA_MINE_SPEED * mine_speed;
    let required = 50.0 + hardness as f32 * 15.0;
    if player.mining_progress < required {
        return Ok(());
    }
    player.mining_progress = 0.0;

    if within_core {
        // The official client shows the mined item flying into the core via
        // Call.transferItemTo; a silent deposit makes the item animation
        // disappear. Emit the RPC and refresh the unit snapshot so the item
        // briefly appears on the unit before the transfer.
        let _guard = world.persistence_lock.lock();
        crate::network::core_inventory::deposit_core_items(world, player_team, item, 1);
        // Round 74e: the world loop persists dirty state every <=1 s
        // through the async PersistenceWorker — a synchronous
        // serde+fsync save here ran INSIDE the tick under
        // persistence_lock and stalled the world (tick_max up to
        // ~99 ms on the user's machine during builds).
        world.persistence_dirty.store(true, Ordering::Relaxed);
        drop(_guard);
        let (mine_x, mine_y) = (
            (position >> 16) as i16 as f32 * 8.0,
            position as i16 as f32 * 8.0,
        );
        if let Ok(frame) = encode_transfer_item_to_frame(
            player.unit_id,
            item,
            1,
            mine_x,
            mine_y,
            core_position_for_team(world, player_team),
        ) {
            out.broadcast(frame);
        }
        broadcast_player_snapshot(player, world, out)?;
    } else {
        player.carried_item = item;
        player.carried_amount += 1;
        // Let the item sprite sit on the unit for a moment (the client draws
        // unit.item until it is cleared by a later snapshot).
        broadcast_player_snapshot(player, world, out)?;
    }
    Ok(())
}

pub(crate) fn mine_result(world: &DynamicWorld, position: i32) -> Option<(i16, u8)> {
    if effective_block(world, position) != 0 {
        return None;
    }
    raw_mine_result(world, position)
}

pub(crate) fn raw_mine_result(world: &DynamicWorld, position: i32) -> Option<(i16, u8)> {
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    if x < 0 || y < 0 || x >= world.width || y >= world.height {
        return None;
    }
    let index = (y * world.width + x) as usize;
    match world.overlays[index] {
        167 => Some((0, 1)),       // copper
        168 => Some((1, 1)),       // lead
        169 => Some((8, 0)),       // scrap
        170 => Some((5, 2)),       // coal
        171 => Some((6, 3)),       // titanium
        172 | 175 => Some((7, 4)), // thorium
        173 => Some((16, 3)),      // beryllium
        174 => Some((17, 5)),      // tungsten
        _ => match world.floors[index] {
            39 | 40 => Some((4, 0)), // sand / darksand
            _ => None,
        },
    }
}
