//! Erekir domain: ducts, heat network, beam drills, turrets, assemblers and
//! crafters. The economy facade re-exports through crate::network::economy::*.

use crate::network::buildings::snapshot::*;
use crate::network::economy::items_for_team;
use crate::network::economy::spec::{
    accept_logistics_item_from, inventory_add, inventory_count, inventory_remove, inventory_total,
    offset_position, turret_target_allowed,
};
use crate::network::units::*;
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

pub(crate) fn is_erekir_duct_block(block: i16) -> bool {
    matches!(block, 272..=278 | 280)
}

pub(crate) fn is_erekir_conveyor_block(block: i16) -> bool {
    matches!(block, 257 | 258 | 259 | 260 | 262 | 263 | 266 | 267 | 279)
}

pub(crate) fn duct_speed(block: i16) -> f32 {
    if block == 280 {
        6.0
    } else {
        4.0
    }
}

pub(crate) fn erekir_offset_by(position: i32, rotation: u8, amount: i32) -> i32 {
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    let (dx, dy) = match rotation % 4 {
        0 => (amount, 0),
        1 => (0, amount),
        2 => (-amount, 0),
        _ => (0, -amount),
    };
    ((x + dx) << 16) | ((y + dy) & 0xffff)
}

/// Direction from `source` to `position` for 1x1 blocks (the rotation index
/// whose offset lands on `source`).
pub(crate) fn relative_direction(source: i32, position: i32) -> Option<u8> {
    (0..4u8).find(|rotation| offset_position(position, *rotation) == source)
}

/// DuctBridge link search (DirectionBridge.findLink): first duct-bridge of
/// the same team within range 4 straight ahead.
pub(crate) fn duct_bridge_link(world: &DynamicWorld, tile: &DynamicTile) -> Option<i32> {
    if tile.block != 277 {
        return None;
    }
    let x = (tile.position >> 16) as i16 as i32;
    let y = tile.position as i16 as i32;
    let (dx, dy) = match tile.rotation % 4 {
        0 => (1, 0),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (0, -1),
    };
    for step in 1..=4 {
        let position = ((x + dx * step) << 16) | ((y + dy * step) as u16 as i32);
        if let Some(other) = world.tiles.get(&position) {
            if other.block == 277 && other.team == tile.team {
                return Some(position);
            }
        }
    }
    None
}

/// Whether a DuctBridge input side already has a linked bridge feeding it.
///
/// `DirectionBridgeBuild.occupied` is a four-slot runtime array.  Its slot is
/// written by the *source* bridge (`link.occupied[rotation] = this`) while
/// updating, so DynamicTile does not carry that transient field.  Reconstruct
/// the same occupied side from live links.  Snapshot the candidates before
/// asking `duct_bridge_link` to avoid nested DashMap iteration/get guards.
pub(crate) fn duct_bridge_input_occupied(
    world: &DynamicWorld,
    target: &DynamicTile,
    incoming_rotation: u8,
) -> bool {
    let candidates: Vec<DynamicTile> = world
        .tiles
        .iter()
        .filter(|candidate| {
            candidate.block == 277
                && candidate.team == target.team
                && candidate.rotation % 4 == incoming_rotation % 4
                && candidate.position != target.position
        })
        .map(|candidate| candidate.value().clone())
        .collect();
    candidates
        .iter()
        .any(|candidate| duct_bridge_link(world, candidate) == Some(target.position))
}

/// Official accept rules of the Erekir duct family (Duct.acceptItem,
/// DuctRouter.acceptItem, OverflowDuct.acceptItem, DuctBridge.acceptItem,
/// StackConveyor.acceptItem, StackRouter.acceptItem).
pub(crate) fn duct_accept_item(
    world: &DynamicWorld,
    position: i32,
    item: i16,
    source: i32,
) -> bool {
    let Some(snapshot) = world.tiles.get(&position).map(|tile| tile.value().clone()) else {
        return false;
    };
    let Some(rel) = relative_direction(source, position) else {
        return false;
    };
    let back = (snapshot.rotation + 2) % 4;
    match snapshot.block {
        // Duct.acceptItem: reject only when the source is our front side
        // (relativeTo(source.tile, tile) - rotation != 2, i.e. the source
        // must not be in front). Armored duct: any side (the official
        // armored rule also allows the front from a duct pointing at us;
        // approximated as accept-any).
        272 => snapshot.stored_amount == 0 && rel != snapshot.rotation,
        273 => snapshot.stored_amount == 0,
        // DuctRouter/OverflowDuct/UnderflowDuct: only from the back side
        // (the feed direction: relativeTo(source.tile, tile) == rotation).
        274..=276 => snapshot.stored_amount == 0 && rel == back,
        // DuctBridgeBuild.acceptItem: require an output link, reject the
        // output/front side, and allow only one bridge source per occupied
        // input side (itemCapacity = 4).
        277 => {
            duct_bridge_link(world, &snapshot).is_some()
                && rel != snapshot.rotation
                && !duct_bridge_input_occupied(world, &snapshot, (rel + 2) % 4)
                && inventory_total(&snapshot.inventory) < 4
        }
        // StackConveyorBuild.acceptItem (bytecode 158.1): capacity 10,
        // single item type, never fed by its own front, and only while the
        // machine is in stateLoad with cooldown <= recharge - 1 (the
        // official `cooldown <= recharge - 1f && state == stateLoad &&
        // front() != source`). Plastanium (259) and surge (279) share the
        // same StackConveyorBuild class and gates. stack_state/stack_cooldown
        // are persisted by the 259|279 machine in simulate_erekir_ducts.
        259 | 279 => {
            snapshot.stack_state == 1
                && snapshot.stack_cooldown <= 1.0
                && snapshot.conveyor_items.len() < 10
                && (snapshot.conveyor_items.is_empty() || snapshot.conveyor_items[0].0 == item)
                && source != offset_position(position, snapshot.rotation)
        }
        // StackRouter: only from the back (feed) side, single type, not
        // unloading, capacity 10.
        280 => {
            rel == back
                && snapshot.production_progress < duct_speed(280)
                && inventory_total(&snapshot.inventory) < 10
                && (inventory_total(&snapshot.inventory) == 0 || snapshot.inventory[0].0 == item)
        }
        _ => false,
    }
}

/// Stores one item into a duct-family tile (mirrors handleItem: ducts start
/// their progress at -1, bridges/conveyors append to their stack).
pub(crate) fn duct_store_item(world: &DynamicWorld, position: i32, item: i16) {
    if let Some(mut tile) = world.tiles.get_mut(&position) {
        match tile.block {
            272..=276 => {
                tile.stored_item = item;
                tile.stored_amount = 1;
                tile.transport_progress = -1.0;
            }
            277 | 280 => inventory_add(&mut tile.inventory, item, 1),
            259 | 279 => {
                // poofIn: the official StackConveyorBuild.handleItem sets
                // `link = tile.pos()` when the stack was empty (bytecode
                // 158.1), so the machine gate (link != -1 && cooldown <= 0)
                // can fire. Without it every 259/279 fed by a duct/conveyor
                // stagnates forever with link == -1 (P1-1 adversarial QA).
                let was_empty = tile.conveyor_items.is_empty();
                tile.conveyor_items.push((item, 0.0));
                if was_empty {
                    tile.stack_link = position;
                }
                tile.stored_item = item;
                tile.stored_amount = i32::try_from(tile.conveyor_items.len()).unwrap_or(i32::MAX);
            }
            _ => {}
        }
    }
}

/// Ammo acceptance for the Erekir item turrets (367/368/370/371/374/375).
/// Items arrive through the duct network; ammo_units counts bullet rounds
/// (one item = `ammoMultiplier` rounds, official ItemTurret ammo).
pub(crate) fn erekir_turret_accept_ammo(world: &DynamicWorld, position: i32, item: i16) -> bool {
    let Some(snapshot) = world.tiles.get(&position).map(|tile| tile.clone()) else {
        return false;
    };
    let Some(ammo) = erekir_turret_ammo_spec(snapshot.block, item) else {
        return false;
    };
    if (snapshot.ammo_units > 0.0 && snapshot.stored_item != item)
        || snapshot.ammo_units + ammo.multiplier > 30.0
    {
        return false;
    }
    if let Some(mut turret) = world.tiles.get_mut(&position) {
        turret.stored_item = item;
        turret.stored_amount += 1;
        turret.ammo_units += ammo.multiplier;
        return true;
    }
    false
}

/// Unified delivery funnel for Erekir systems: ducts and Erekir turrets are
/// handled with their own accept rules first; everything else falls back to
/// the Serpulo logistics funnel (accept_logistics_item_from).
pub(crate) fn deliver_item_to(world: &DynamicWorld, target: i32, item: i16, source: i32) -> bool {
    if world
        .tiles
        .get(&target)
        .is_some_and(|tile| is_erekir_duct_block(tile.block) || matches!(tile.block, 259 | 279))
    {
        // The stack conveyors (plastanium 259, surge 279) belong to the duct
        // family (duct_accept_item/duct_store_item implement them) but are
        // excluded from is_erekir_duct_block; route them explicitly so
        // ducts, unloaders and drills can deposit into a stack conveyor.
        return duct_accept_item(world, target, item, source) && {
            duct_store_item(world, target, item);
            true
        };
    }
    if world
        .tiles
        .get(&target)
        .is_some_and(|tile| matches!(tile.block, 367 | 368 | 370 | 371 | 374 | 375))
    {
        return erekir_turret_accept_ammo(world, target, item);
    }
    accept_logistics_item_from(world, target, item, Some(source), 0)
}

/// Front item of a conveyor-family tile ready for hand-off (progress >= 1),
/// or None. Mirrors the "keep at the far end and retry" state that
/// simulate_logistics leaves when the target did not accept.
pub(crate) fn ready_source_item(
    world: &DynamicWorld,
    source: i32,
    duct_position: i32,
) -> Option<i16> {
    let snapshot = world.tiles.get(&source).map(|tile| tile.value().clone())?;
    if !is_erekir_conveyor_block(snapshot.block)
        || offset_position(source, snapshot.rotation) != duct_position
    {
        return None;
    }
    if matches!(snapshot.block, 266 | 267) {
        return (snapshot.stored_amount > 0 && snapshot.transport_progress >= 1.0 - 0.01)
            .then_some(snapshot.stored_item);
    }
    snapshot
        .conveyor_items
        .first()
        .filter(|(_, progress)| *progress >= 1.0 - 0.01)
        .map(|(item, _)| *item)
}

pub(crate) fn remove_source_item(world: &DynamicWorld, source: i32, item: i16) {
    if let Some(mut tile) = world.tiles.get_mut(&source) {
        if matches!(tile.block, 266 | 267) {
            tile.stored_amount -= 1;
            if tile.stored_amount <= 0 {
                tile.stored_amount = 0;
                tile.stored_item = -1;
            }
        } else if tile
            .conveyor_items
            .first()
            .is_some_and(|(stored, _)| *stored == item)
        {
            tile.conveyor_items.remove(0);
            tile.stored_amount = i32::try_from(tile.conveyor_items.len()).unwrap_or(i32::MAX);
            tile.stored_item = tile
                .conveyor_items
                .first()
                .map(|(stored, _)| *stored)
                .unwrap_or(-1);
        }
    }
}

/// Candidate hand-off targets of a single-slot duct (272-276) following the
/// official target() selection order.
pub(crate) fn duct_output_targets(
    _world: &DynamicWorld,
    snapshot: &DynamicTile,
    item: i16,
) -> Vec<i32> {
    let front = offset_position(snapshot.position, snapshot.rotation);
    match snapshot.block {
        272 | 273 => vec![front],
        274 => {
            // DuctRouterBuild.target: round-robin over all neighbours except
            // the back; a configured sortItem forces the item forward when it
            // matches and sideways otherwise.
            let sort_item = configured_item_local(&snapshot.config);
            let dump = (snapshot.unloader_offset as i32).rem_euclid(4);
            let mut targets = Vec::new();
            for step in 0..4 {
                let rel = ((step + dump) % 4) as u8;
                if rel == (snapshot.rotation + 2) % 4 {
                    continue;
                }
                if let Some(sort) = sort_item {
                    if (item == sort) != (rel == snapshot.rotation) {
                        continue;
                    }
                }
                targets.push(offset_position(snapshot.position, rel));
            }
            targets
        }
        275 => {
            // OverflowDuct: front first, then the two sides round-robin.
            let front_ok = front;
            let mut targets = vec![front_ok];
            let cdump = (snapshot.unloader_offset as i32).rem_euclid(3);
            for step in 0..3 {
                let dir =
                    (snapshot.rotation as i32 + ((step + cdump + 1) % 3) - 1).rem_euclid(4) as u8;
                if dir == snapshot.rotation {
                    continue;
                }
                targets.push(offset_position(snapshot.position, dir));
            }
            targets
        }
        276 => {
            // UnderflowDuct: sides first (with cdump toggle), then front.
            let mut targets = Vec::new();
            for side in [1u8, 3u8] {
                let rel = (snapshot.rotation + side) % 4;
                targets.push(offset_position(snapshot.position, rel));
            }
            if snapshot.unloader_offset % 2 == 0 {
                targets.reverse();
            }
            targets.push(front);
            targets
        }
        _ => vec![],
    }
}

pub(crate) fn configured_item_local(config: &[u8]) -> Option<i16> {
    (config.len() == 4 && config[0] == 5 && config[1] == 0)
        .then(|| i16::from_be_bytes([config[2], config[3]]))
        .filter(|item| (0..22).contains(item))
}

/// Peek the item a duct-unloader (278) would pull from its back neighbour,
/// without removing it yet (DirectionalUnloader.updateTile: unloads one item
/// per `speed` tick from `back.items`; speed=1 -> 60/s).
pub(crate) fn peek_unloader_item(
    world: &DynamicWorld,
    back: i32,
    unload_item: Option<i16>,
    offset: i16,
) -> Option<i16> {
    let snapshot = world.tiles.get(&back).map(|tile| tile.value().clone())?;
    if is_erekir_conveyor_block(snapshot.block) {
        return None;
    }
    let mut candidates: Vec<i16> = Vec::new();
    for (item, amount) in &snapshot.inventory {
        if *amount > 0 {
            candidates.push(*item);
        }
    }
    if snapshot.stored_amount > 0 && snapshot.stored_item >= 0 {
        candidates.push(snapshot.stored_item);
    }
    if candidates.is_empty() {
        return None;
    }
    if let Some(item) = unload_item {
        return candidates.contains(&item).then_some(item);
    }
    let index = (offset as i32).rem_euclid(candidates.len() as i32) as usize;
    Some(candidates[index])
}

pub(crate) fn take_unloader_item(world: &DynamicWorld, back: i32, item: i16) -> bool {
    let Some(snapshot) = world.tiles.get(&back).map(|tile| tile.value().clone()) else {
        return false;
    };
    if snapshot.block == 408 {
        return false;
    }
    if inventory_count(&snapshot.inventory, item) > 0 {
        // Keep the mutable guard in a local expression and release it before
        // returning.  The caller may immediately write the destination tile,
        // which can share a DashMap shard with `back`.
        return world
            .tiles
            .get_mut(&back)
            .is_some_and(|mut tile| inventory_remove(&mut tile.inventory, item, 1));
    }
    if snapshot.stored_amount > 0 && snapshot.stored_item == item {
        if let Some(mut tile) = world.tiles.get_mut(&back) {
            tile.stored_amount -= 1;
            if tile.stored_amount <= 0 {
                tile.stored_item = -1;
            }
            return true;
        }
    }
    false
}

pub(crate) fn simulate_erekir_ducts(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    // The stack conveyors (plastanium 259, surge 279) belong to the duct
    // family (the branch below implements StackConveyorBuild) even though
    // is_erekir_duct_block excludes them.
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| is_erekir_duct_block(tile.block) || matches!(tile.block, 259 | 279))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;

    // Phase 1: advance progress and hand off ready items.
    for key in &keys {
        let Some(snapshot) = world.tiles.get(key).map(|tile| tile.value().clone()) else {
            continue;
        };
        let efficiency = power.get(key).copied().unwrap_or(1.0);
        match snapshot.block {
            272..=276 => {
                if snapshot.stored_amount <= 0 {
                    continue;
                }
                let speed = duct_speed(snapshot.block);
                let step =
                    delta_ticks * building_time_scale(world, *key) * efficiency * (2.0 / speed);
                let threshold = 1.0 - 1.0 / speed;
                if snapshot.transport_progress + step < threshold {
                    if let Some(mut tile) = world.tiles.get_mut(key) {
                        tile.transport_progress += step;
                    }
                    changed = true;
                    continue;
                }
                let item = snapshot.stored_item;
                let mut delivered = false;
                for target in duct_output_targets(world, &snapshot, item) {
                    if deliver_item_to(world, target, item, *key) {
                        delivered = true;
                        break;
                    }
                }
                if let Some(mut tile) = world.tiles.get_mut(key) {
                    if delivered {
                        tile.stored_amount = 0;
                        tile.stored_item = -1;
                        tile.transport_progress = 0.0;
                        if matches!(snapshot.block, 274..=276) {
                            // cdump alternation after a successful hand-off
                            // (OverflowDuct toggles 0/2; the router advances).
                            tile.unloader_offset = if snapshot.block == 274 {
                                tile.unloader_offset.wrapping_add(1)
                            } else if tile.unloader_offset == 0 {
                                2
                            } else {
                                0
                            };
                        }
                    } else {
                        // Keep the item at the far end; retry next tick.
                        tile.transport_progress = threshold - 0.0001;
                    }
                    changed = true;
                }
            }
            277 => {
                // DuctBridge: buffer transfer to the linked bridge.
                if snapshot.inventory.is_empty() {
                    continue;
                }
                let Some(link) = duct_bridge_link(world, &snapshot) else {
                    // No link: move the front item forward like a duct.
                    let item = snapshot.inventory[0].0;
                    if deliver_item_to(world, offset_position(*key, snapshot.rotation), item, *key)
                    {
                        if let Some(mut tile) = world.tiles.get_mut(key) {
                            inventory_remove(&mut tile.inventory, item, 1);
                        }
                        changed = true;
                    }
                    continue;
                };
                let mut progress = snapshot.production_progress + delta_ticks;
                let mut moved_any = false;
                loop {
                    if progress < duct_speed(277) {
                        break;
                    }
                    // Read the live front item each round; the snapshot
                    // inventory goes stale once items have been moved out.
                    let Some(item) = world
                        .tiles
                        .get(key)
                        .and_then(|tile| tile.inventory.first().map(|(item, _)| *item))
                    else {
                        break;
                    };
                    // DirectionBridgeBuild.updateTile calls link.handleItem
                    // directly, bypassing link.acceptItem.  In particular, do
                    // not apply the receiver's occupied-input guard here: the
                    // source bridge already owns that input slot.  Check only
                    // the receiver's live capacity before the hand-off.
                    if world
                        .tiles
                        .get(&link)
                        .is_none_or(|target| inventory_total(&target.inventory) >= 4)
                    {
                        break;
                    }
                    // Remove from the source bridge and DROP the guard before
                    // touching the link: holding a DashMap get_mut on `key`
                    // while duct_store_item takes get_mut on `link` deadlocks
                    // when both keys land in the same shard.
                    if let Some(mut tile) = world.tiles.get_mut(key) {
                        inventory_remove(&mut tile.inventory, item, 1);
                    }
                    duct_store_item(world, link, item);
                    progress -= duct_speed(277);
                    moved_any = true;
                }
                if moved_any {
                    if let Some(mut tile) = world.tiles.get_mut(key) {
                        tile.production_progress = progress % duct_speed(277);
                    }
                    changed = true;
                }
            }
            278 => {
                // Duct-unloader (DirectionalUnloader.updateTile): unloadTimer
                // accumulates edelta() (delta * timeScale) and unloads ONE
                // item whenever it reaches `speed`; ductUnloader speed = 4f
                // (Blocks.java v158.1) -> 60/4 = 15 items/s. Regression: the
                // port used a 1.0 threshold (one item per tick, 4x too fast).
                let mut timer =
                    snapshot.transport_progress + delta_ticks * building_time_scale(world, *key);
                let unload_item = configured_item_local(&snapshot.config);
                let mut offset = snapshot.unloader_offset;
                while timer >= duct_speed(278) {
                    timer -= duct_speed(278);
                    let front = offset_position(*key, snapshot.rotation);
                    let back = offset_position(*key, (snapshot.rotation + 2) % 4);
                    let front_exists = world.tiles.get(&front).is_some();
                    if !front_exists {
                        continue;
                    }
                    let Some(item) = peek_unloader_item(world, back, unload_item, offset) else {
                        continue;
                    };
                    if !take_unloader_item(world, back, item) {
                        continue;
                    }
                    if deliver_item_to(world, front, item, *key) {
                        if unload_item.is_none() {
                            offset = item.wrapping_add(1);
                        }
                    } else {
                        // The front rejected the item: put it back.
                        if let Some(mut tile) = world.tiles.get_mut(&back) {
                            if !tile.inventory.is_empty() {
                                inventory_add(&mut tile.inventory, item, 1);
                            } else {
                                tile.stored_item = item;
                                tile.stored_amount += 1;
                            }
                        }
                    }
                }
                if let Some(mut tile) = world.tiles.get_mut(key) {
                    tile.transport_progress = timer;
                    tile.unloader_offset = offset;
                }
                changed = true;
            }
            259 | 279 => {
                // P1: official StackConveyorBuild machine, verified against
                // desktop.jar 158.1 bytecode and the v158 source
                // (StackConveyor.java). Both plastanium (259) and surge (279)
                // are StackConveyor subclasses (Blocks$205 / Blocks$225):
                // recharge = 2f, itemCapacity = 10. Speeds: 259 = 4/60,
                // 279 = 5/60 per tick; baseEfficiency: 259 = 0, 279 = 1.
                // cooldown reels the link; stateMove/stateLoad transfer the
                // WHOLE stack to an idle linked front (front.link == -1,
                // same team) and set cooldown = recharge, front.cooldown = 1;
                // stateUnload is a BURST: `while(lastItem != null &&
                // moveForward(lastItem)) items.remove(lastItem, 1)` — every
                // item moves in one tick and cooldown is NOT reset (the
                // cadence comes from the conveyor behind).
                const STACK_RECHARGE: f32 = 2.0;
                let stack_speed = if snapshot.block == 279 {
                    5.0 / 60.0
                } else {
                    4.0 / 60.0
                };
                // Official updateTile: `eff = enabled ? efficiency +
                // baseEfficiency : 1f`. Reeling with plain efficiency runs
                // the cooldown at half cadence (P1-2 adversarial QA); with
                // efficiency 0 the official still reels at eff = 1 for 279,
                // which `efficiency + 1.0` reproduces (259: base 0).
                let step = delta_ticks
                    * building_time_scale(world, *key)
                    * (efficiency + if snapshot.block == 279 { 1.0 } else { 0.0 });
                let front = offset_position(*key, snapshot.rotation);
                let back = offset_position(*key, (snapshot.rotation + 2) % 4);
                // onProximityUpdate approximation (straight lines).
                let front_is_stack = world
                    .tiles
                    .get(&front)
                    .is_some_and(|tile| matches!(tile.block, 259 | 279));
                let back_is_stack = world
                    .tiles
                    .get(&back)
                    .is_some_and(|tile| matches!(tile.block, 259 | 279));
                let state = if !front_is_stack {
                    2 // stateUnload
                } else if !back_is_stack {
                    1 // stateLoad
                } else {
                    0 // stateMove
                };
                let mut items = snapshot.conveyor_items.clone();
                let mut stack_link = snapshot.stack_link;
                let mut stack_cooldown = snapshot.stack_cooldown;
                // Reel in the crater: cooldown decays with speed*eff*delta.
                if stack_cooldown > 0.0 {
                    stack_cooldown =
                        (stack_cooldown - stack_speed * step).clamp(0.0, STACK_RECHARGE);
                }
                // Empty stack has no link.
                if items.is_empty() {
                    stack_link = -1;
                }
                if stack_link != -1 && stack_cooldown <= 0.0 {
                    if state == 2 {
                        // stateUnload: official burst (bytecode 158.1):
                        // `while(lastItem != null && moveForward(lastItem))
                        // items.remove(lastItem, 1)` — every item of the
                        // CURRENT TYPE moves forward in the same tick, the
                        // cooldown is NOT reset (the cadence comes from the
                        // conveyor behind), and poofOut clears the link once
                        // that type is exhausted (with a mixed stack the
                        // remainder strands with link == -1, official
                        // quirk; homogeneous stacks — the only ones the
                        // accept gates allow — behave identically).
                        let burst_item = items.first().copied().map(|(item, _)| item);
                        if let Some(burst_item) = burst_item {
                            while items.first().is_some_and(|(item, _)| *item == burst_item) {
                                if !deliver_item_to(world, front, burst_item, *key) {
                                    break;
                                }
                                items.remove(0);
                            }
                            if !items.iter().any(|(item, _)| *item == burst_item) {
                                stack_link = -1; // poofOut
                            }
                        }
                    } else if state == 0 || items.len() >= 10 {
                        // Transfer the WHOLE stack to the front conveyor when
                        // it is idle (front.link == -1) and same team.
                        let front_state = world.tiles.get(&front).map(|tile| {
                            (
                                tile.block,
                                tile.team,
                                tile.stack_link,
                                tile.conveyor_items.clone(),
                            )
                        });
                        if let Some((block, team, front_link, front_items)) = front_state {
                            if matches!(block, 259 | 279)
                                && team == snapshot.team
                                && front_link == -1
                            {
                                // e.items.add(items); e.link = tile.pos();
                                // e.cooldown = 1; link = -1; items.clear();
                                // cooldown = recharge. Round 74: the official
                                // APPENDS unconditionally, but a stalled
                                // unload (e.g. a full turret at the line end)
                                // then reels batches in forever — the last
                                // stretch accumulated 300+ items and the
                                // client-side belt broke. Cap the batch at
                                // the front's remaining capacity (10) so the
                                // line backs up bounded like the official
                                // acceptItem gates intend.
                                let free = (10usize).saturating_sub(front_items.len());
                                if free > 0 {
                                    let take = items.len().min(free);
                                    let moving: Vec<(i16, f32)> = items.drain(..take).collect();
                                    if let Some(mut front_tile) = world.tiles.get_mut(&front) {
                                        for (stacked, progress) in &moving {
                                            front_tile.conveyor_items.push((*stacked, *progress));
                                        }
                                        front_tile.stack_link = *key;
                                        front_tile.stack_cooldown = 1.0;
                                        front_tile.stored_item =
                                            moving.first().map(|(item, _)| *item).unwrap_or(-1);
                                        front_tile.stored_amount =
                                            i32::try_from(front_tile.conveyor_items.len())
                                                .unwrap_or(i32::MAX);
                                    }
                                    stack_link = -1;
                                    stack_cooldown = STACK_RECHARGE;
                                }
                            }
                        }
                    }
                }
                // Preserve FIFO while blocked: keep the front item at index 0
                // with its logical position (local jam regression).
                if let Some(mut tile) = world.tiles.get_mut(key) {
                    tile.conveyor_items = items;
                    tile.stack_state = state;
                    tile.stack_link = stack_link;
                    tile.stack_cooldown = stack_cooldown;
                    tile.stored_item = tile
                        .conveyor_items
                        .first()
                        .map(|(item, _)| *item)
                        .unwrap_or(-1);
                    tile.stored_amount =
                        i32::try_from(tile.conveyor_items.len()).unwrap_or(i32::MAX);
                }
                changed = true;
            }
            280 => {
                // StackRouter: when full, charge the offload timer; then dump
                // the whole stack to round-robin targets.
                if inventory_total(&snapshot.inventory) < 10 {
                    continue;
                }
                let mut progress = snapshot.production_progress
                    + delta_ticks * building_time_scale(world, *key) * efficiency;
                if progress >= duct_speed(280) {
                    progress %= duct_speed(280);
                    let items: Vec<i16> = snapshot
                        .inventory
                        .iter()
                        .flat_map(|(item, amount)| vec![*item; *amount as usize])
                        .collect();
                    for item in items {
                        let dump = (snapshot.unloader_offset as i32).rem_euclid(4);
                        let mut done = false;
                        for step in 0..4 {
                            let rel = ((step + dump) % 4) as u8;
                            if rel == (snapshot.rotation + 2) % 4 {
                                continue;
                            }
                            if deliver_item_to(world, offset_position(*key, rel), item, *key) {
                                if let Some(mut tile) = world.tiles.get_mut(key) {
                                    inventory_remove(&mut tile.inventory, item, 1);
                                    tile.unloader_offset = tile.unloader_offset.wrapping_add(1);
                                }
                                done = true;
                                break;
                            }
                        }
                        if !done {
                            break;
                        }
                    }
                }
                if let Some(mut tile) = world.tiles.get_mut(key) {
                    tile.production_progress = progress;
                }
                changed = true;
            }
            _ => {}
        }
    }

    // Phase 2: pull ready items from conveyor-family neighbours into empty
    // duct slots. Conveyors that cannot deliver leave their front item at
    // progress 1-eps (simulate_logistics); ducts pick it up here.
    for key in &keys {
        let Some(snapshot) = world.tiles.get(key).map(|tile| tile.value().clone()) else {
            continue;
        };
        let has_slot = match snapshot.block {
            272..=276 => snapshot.stored_amount == 0,
            277 => {
                duct_bridge_link(world, &snapshot).is_some()
                    && inventory_total(&snapshot.inventory) < 4
            }
            279 => snapshot.conveyor_items.len() < 10,
            280 => {
                snapshot.production_progress < duct_speed(280)
                    && inventory_total(&snapshot.inventory) < 10
            }
            _ => false,
        };
        if !has_slot {
            continue;
        }
        for rotation in 0..4 {
            let source = offset_position(*key, rotation);
            let Some(item) = ready_source_item(world, source, *key) else {
                continue;
            };
            if !duct_accept_item(world, *key, item, source) {
                continue;
            }
            remove_source_item(world, source, item);
            duct_store_item(world, *key, item);
            changed = true;
            break;
        }
    }
    changed
}

// ===========================================================================
// EREKIR: HEAT NETWORK (201-208, 210, 212-215)
// ===========================================================================
// The official model has no HeatGraph class: heat is a per-building
// adjacency computation. `Building.calculateHeat` (BuildingComp.java) sums,
// over same-team HeatBlock neighbours that satisfy the facing rule, the
// neighbour's `heat() / neighbour.size * contactPoints` (heat-routers divide
// by 3 and face AWAY from their input). Heat producers
// (HeatProducerBuild.updateTile) approach `heatOutput * efficiency` at
// warmupRate 0.15/tick. Heat crafters (HeatCrafter) craft with efficiency
// `clamp(heat / heatRequirement, 0, maxEfficiency)`.
// Per-block values below come from Blocks.java v158.1; per-craft liquid
// amounts follow the round-20 calibration (per_craft = rate_per_tick *
// craft_time).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeatKind {
    Producer,
    Conductor,
    Consumer,
}

#[derive(Clone, Copy)]
pub(crate) struct HeatBlockSpec {
    pub(crate) block: i16,
    pub(crate) kind: HeatKind,
    pub(crate) size: u8,
    pub(crate) split: bool,
    pub(crate) heat_output: f32,
    pub(crate) heat_requirement: f32,
    pub(crate) craft_time: f32,
    pub(crate) item_inputs: &'static [(i16, i32)],
    /// (liquid id, amount per craft) — official consumeLiquid rate * craftTime.
    pub(crate) liquid_input: Option<(i16, f32)>,
    pub(crate) item_output: Option<(i16, i32)>,
    /// (liquid id, amount per craft) — official outputLiquid rate * craftTime.
    pub(crate) liquid_output: Option<(i16, f32)>,
    pub(crate) item_capacity: i32,
    pub(crate) power_demand: f32,
}

pub(crate) fn heat_block_spec(block: i16) -> Option<HeatBlockSpec> {
    let spec = match block {
        // atmospheric-concentrator: HeatCrafter, heatRequirement 24,
        // nitrogen 16/60 per tick, craftTime default 80, power 2.
        201 => HeatBlockSpec {
            block,
            kind: HeatKind::Consumer,
            size: 3,
            split: false,
            heat_output: 0.0,
            heat_requirement: 24.0,
            craft_time: 80.0,
            item_inputs: &[],
            liquid_input: None,
            item_output: None,
            liquid_output: Some((9, 16.0 / 60.0 * 80.0)),
            item_capacity: 0,
            power_demand: 2.0,
        },
        // oxidation-chamber: HeatProducer, heatOutput 5, beryllium -> oxide,
        // ozone 2/60 per tick, craftTime 120, power 0.5.
        202 => HeatBlockSpec {
            block,
            kind: HeatKind::Producer,
            size: 3,
            split: false,
            heat_output: 5.0,
            heat_requirement: 0.0,
            craft_time: 120.0,
            item_inputs: &[(16, 1)],
            liquid_input: Some((7, 2.0 / 60.0 * 120.0)),
            item_output: Some((18, 1)),
            liquid_output: None,
            item_capacity: 10,
            power_demand: 0.5,
        },
        // electric-heater: HeatProducer, heatOutput 3, power 100/60.
        203 => HeatBlockSpec {
            block,
            kind: HeatKind::Producer,
            size: 2,
            split: false,
            heat_output: 3.0,
            heat_requirement: 0.0,
            craft_time: 0.0,
            item_inputs: &[],
            liquid_input: None,
            item_output: None,
            liquid_output: None,
            item_capacity: 0,
            power_demand: 100.0 / 60.0,
        },
        // slag-heater: HeatProducer, heatOutput 8, slag 40/60 per tick.
        204 => HeatBlockSpec {
            block,
            kind: HeatKind::Producer,
            size: 3,
            split: false,
            heat_output: 8.0,
            heat_requirement: 0.0,
            craft_time: 80.0,
            item_inputs: &[],
            liquid_input: Some((1, 40.0 / 60.0 * 80.0)),
            item_output: None,
            liquid_output: None,
            item_capacity: 0,
            power_demand: 0.0,
        },
        // phase-heater: HeatProducer, heatOutput 15, one phase-fabric per
        // craft, craftTime 60*8 = 480.
        205 => HeatBlockSpec {
            block,
            kind: HeatKind::Producer,
            size: 2,
            split: false,
            heat_output: 15.0,
            heat_requirement: 0.0,
            craft_time: 480.0,
            item_inputs: &[(11, 1)],
            liquid_input: None,
            item_output: None,
            liquid_output: None,
            item_capacity: 10,
            power_demand: 0.0,
        },
        // heat-redirector / small-heat-redirector / heat-router: conductors.
        206 => HeatBlockSpec {
            block,
            kind: HeatKind::Conductor,
            size: 3,
            split: false,
            heat_output: 0.0,
            heat_requirement: 0.0,
            craft_time: 0.0,
            item_inputs: &[],
            liquid_input: None,
            item_output: None,
            liquid_output: None,
            item_capacity: 0,
            power_demand: 0.0,
        },
        207 => HeatBlockSpec {
            block,
            kind: HeatKind::Conductor,
            size: 2,
            split: false,
            heat_output: 0.0,
            heat_requirement: 0.0,
            craft_time: 0.0,
            item_inputs: &[],
            liquid_input: None,
            item_output: None,
            liquid_output: None,
            item_capacity: 0,
            power_demand: 0.0,
        },
        208 => HeatBlockSpec {
            block,
            kind: HeatKind::Conductor,
            size: 3,
            split: true,
            heat_output: 0.0,
            heat_requirement: 0.0,
            craft_time: 0.0,
            item_inputs: &[],
            liquid_input: None,
            item_output: None,
            liquid_output: None,
            item_capacity: 0,
            power_demand: 0.0,
        },
        // carbide-crucible: HeatCrafter, heatRequirement 40,
        // craftTime 60*2.25/4 = 33.75, tungsten 2 + graphite 3 -> carbide.
        210 => HeatBlockSpec {
            block,
            kind: HeatKind::Consumer,
            size: 3,
            split: false,
            heat_output: 0.0,
            heat_requirement: 40.0,
            craft_time: 33.75,
            item_inputs: &[(17, 2), (3, 3)],
            liquid_input: None,
            item_output: Some((19, 1)),
            liquid_output: None,
            item_capacity: 20,
            power_demand: 2.0,
        },
        // surge-crucible: HeatCrafter, heatRequirement 40, craftTime 45,
        // silicon 3 + slag 160/60 -> surge-alloy.
        212 => HeatBlockSpec {
            block,
            kind: HeatKind::Consumer,
            size: 3,
            split: false,
            heat_output: 0.0,
            heat_requirement: 40.0,
            craft_time: 45.0,
            item_inputs: &[(9, 3)],
            liquid_input: Some((1, 160.0 / 60.0 * 45.0)),
            item_output: Some((12, 1)),
            liquid_output: None,
            item_capacity: 20,
            power_demand: 1.5,
        },
        // cyanogen-synthesizer: HeatCrafter, heatRequirement 20, graphite +
        // arkycite 160/60 -> cyanogen 12/60, craftTime default 80.
        213 => HeatBlockSpec {
            block,
            kind: HeatKind::Consumer,
            size: 3,
            split: false,
            heat_output: 0.0,
            heat_requirement: 20.0,
            craft_time: 80.0,
            item_inputs: &[(3, 1)],
            liquid_input: Some((5, 160.0 / 60.0 * 80.0)),
            item_output: None,
            liquid_output: Some((10, 12.0 / 60.0 * 80.0)),
            item_capacity: 10,
            power_demand: 2.0,
        },
        // phase-synthesizer: HeatCrafter, heatRequirement 32, craftTime 30,
        // thorium 2 + sand 6 + ozone 8/60 -> phase-fabric, power 8.
        214 => HeatBlockSpec {
            block,
            kind: HeatKind::Consumer,
            size: 3,
            split: false,
            heat_output: 0.0,
            heat_requirement: 32.0,
            craft_time: 30.0,
            item_inputs: &[(7, 2), (4, 6)],
            liquid_input: Some((7, 8.0 / 60.0 * 30.0)),
            item_output: Some((11, 1)),
            liquid_output: None,
            item_capacity: 40,
            power_demand: 8.0,
        },
        // heat-reactor: HeatProducer (default heatOutput 10), thorium 3 +
        // nitrogen 1/60 -> fissile-matter, craftTime 600.
        215 => HeatBlockSpec {
            block,
            kind: HeatKind::Producer,
            size: 3,
            split: false,
            heat_output: 10.0,
            heat_requirement: 0.0,
            craft_time: 600.0,
            item_inputs: &[(7, 3)],
            liquid_input: Some((9, 1.0 / 60.0 * 600.0)),
            item_output: Some((20, 1)),
            liquid_output: None,
            item_capacity: 20,
            power_demand: 0.0,
        },
        // Sandbox heat-source: HeatProducer with heatOutput=1000 and
        // warmupRate=1000, so it reaches full output in one update.
        418 => HeatBlockSpec {
            block,
            kind: HeatKind::Producer,
            size: 1,
            split: false,
            heat_output: 1_000.0,
            heat_requirement: 0.0,
            craft_time: 0.0,
            item_inputs: &[],
            liquid_input: None,
            item_output: None,
            liquid_output: None,
            item_capacity: 0,
            power_demand: 0.0,
        },
        _ => return None,
    };
    Some(spec)
}

/// Received heat of one tile following Building.calculateHeat, with the
/// official cycle detection (`cameFrom`) and the recursive conductor update
/// (`cond.updateHeat()` inside the neighbour loop).
pub(crate) fn heat_value_at(
    world: &DynamicWorld,
    position: i32,
    producer_heat: &std::collections::HashMap<i32, f32>,
    memo: &mut std::collections::HashMap<i32, f32>,
    chain: &mut Vec<i32>,
) -> f32 {
    if let Some(value) = memo.get(&position) {
        return *value;
    }
    if let Some(value) = producer_heat.get(&position) {
        memo.insert(position, *value);
        return *value;
    }
    let Some(snapshot) = world.tiles.get(&position).map(|tile| tile.clone()) else {
        return 0.0;
    };
    let Some(spec) = heat_block_spec(snapshot.block) else {
        return 0.0;
    };
    let mut total = 0.0f32;
    for rel in 0..4u8 {
        let neighbor_position = offset_position(position, rel);
        let Some(neighbor) = world
            .tiles
            .iter()
            .find(|tile| {
                tile.position == neighbor_position || tile.occupied.contains(&neighbor_position)
            })
            .map(|tile| tile.clone())
        else {
            continue;
        };
        // Use the neighbour's base position (multi-tile heat blocks register
        // their key, not every occupied tile, in producer_heat/memo/chain).
        let neighbor_key = neighbor.position;
        if neighbor.team != snapshot.team {
            continue;
        }
        let Some(neighbor_spec) = heat_block_spec(neighbor.block) else {
            continue;
        };
        if !matches!(neighbor_spec.kind, HeatKind::Producer | HeatKind::Conductor) {
            continue; // consumers (HeatCrafter) do not implement HeatBlock.
        }
        let facing_ok = if neighbor_spec.split {
            rel != neighbor.rotation
        } else {
            (rel + 2) % 4 == neighbor.rotation
        };
        if !facing_ok {
            continue;
        }
        let neighbor_heat = if matches!(neighbor_spec.kind, HeatKind::Producer) {
            producer_heat.get(&neighbor_key).copied().unwrap_or(0.0)
        } else {
            if chain.contains(&neighbor_key) {
                continue; // cycle: ignore its heat (cameFrom check).
            }
            chain.push(neighbor_key);
            let value = heat_value_at(world, neighbor_key, producer_heat, memo, chain);
            chain.pop();
            value
        };
        let (x1, y1) = ((position >> 16) as i16 as i32, position as i16 as i32);
        let (x2, y2) = (
            (neighbor_position >> 16) as i16 as i32,
            neighbor_position as i16 as i32,
        );
        let diff = (x2 - x1).abs().min((y2 - y1).abs()) as f32;
        let contact =
            ((spec.size as f32 / 2.0 + neighbor_spec.size as f32 / 2.0) - diff).max(0.0) as i32;
        let contact = contact.min(spec.size.min(neighbor_spec.size) as i32);
        let mut add = neighbor_heat / f32::from(neighbor_spec.size) * contact as f32;
        if neighbor_spec.split {
            add /= 3.0;
        }
        total += add;
    }
    memo.insert(position, total);
    total
}

/// Public helper for turrets and logic: recompute the received heat of any
/// tile using the producer heats currently stored on their tiles.
pub(crate) fn erekir_heat_at(world: &DynamicWorld, position: i32) -> f32 {
    let mut producer_heat = std::collections::HashMap::new();
    for tile in world.tiles.iter() {
        if heat_block_spec(tile.block).is_some_and(|spec| matches!(spec.kind, HeatKind::Producer)) {
            producer_heat.insert(*tile.key(), tile.mass_driver_rotation);
        }
    }
    let mut memo = std::collections::HashMap::new();
    let mut chain = Vec::new();
    heat_value_at(world, position, &producer_heat, &mut memo, &mut chain)
}

pub(crate) fn heat_inputs_available(snapshot: &DynamicTile, spec: &HeatBlockSpec) -> bool {
    if !spec
        .item_inputs
        .iter()
        .all(|(item, amount)| inventory_count(&snapshot.inventory, *item) >= *amount)
    {
        return false;
    }
    if let Some((liquid, amount)) = spec.liquid_input {
        if snapshot.stored_liquid != liquid || snapshot.liquid_amount + 0.0001 < amount {
            return false;
        }
    }
    true
}

/// Applies one craft (consume inputs, produce outputs) to a heat block.
pub(crate) fn heat_apply_craft(world: &DynamicWorld, key: i32, spec: &HeatBlockSpec) -> bool {
    let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
        return false;
    };
    if !heat_inputs_available(&snapshot, spec) {
        return false;
    }
    let output_fits = spec.item_output.is_none_or(|(_, amount)| {
        inventory_total(&snapshot.inventory)
            - spec
                .item_inputs
                .iter()
                .map(|(_, amount)| *amount)
                .sum::<i32>()
            + amount
            <= spec.item_capacity
    }) && spec.liquid_output.is_none_or(|(_, amount)| {
        snapshot.liquid_amount + amount <= liquid_capacity(snapshot.block).unwrap_or(0.0)
    });
    if !output_fits {
        return false;
    }
    if let Some(mut tile) = world.tiles.get_mut(&key) {
        for (item, amount) in spec.item_inputs {
            let removed = inventory_remove(&mut tile.inventory, *item, *amount);
            debug_assert!(removed);
        }
        if let Some((_, amount)) = spec.liquid_input {
            tile.liquid_amount = (tile.liquid_amount - amount).max(0.0);
            if tile.liquid_amount <= 0.0001 {
                tile.liquid_amount = 0.0;
                tile.stored_liquid = -1;
            }
        }
        if let Some((item, amount)) = spec.item_output {
            inventory_add(&mut tile.inventory, item, amount);
        }
        if let Some((liquid, amount)) = spec.liquid_output {
            tile.stored_liquid = liquid;
            tile.liquid_amount = (tile.liquid_amount + amount)
                .min(liquid_capacity(snapshot.block).unwrap_or(f32::MAX));
        }
        true
    } else {
        false
    }
}

/// Official HeatGraph update (Erekir heat network): each tick:
/// - every heat PRODUCER advances its production (electric-heater outputs
///   `heatOutput * efficiency`; crafters output heat only while crafting),
///   stored on `DynamicTile.mass_driver_rotation` (heat field);
/// - every heat CONSUMER with enough received heat (`heat_value_at` via
///   adjacent conductors/producers) runs `heat_apply_craft` to produce its
///   output, consuming inputs.
///
/// The conductor chains propagate producer heat (heat_value_at walks
/// neighbours), matching HeatGraph semantics for the blocks the port models.
pub(crate) fn simulate_heat_network(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| heat_block_spec(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    // Pass 1: producers generate heat.
    let mut producer_heat = std::collections::HashMap::new();
    for key in &keys {
        let Some(snapshot) = world.tiles.get(key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(spec) = heat_block_spec(snapshot.block) else {
            continue;
        };
        let efficiency = power
            .get(key)
            .copied()
            .unwrap_or(if spec.power_demand <= 0.0 { 1.0 } else { 0.0 });
        if spec.kind != HeatKind::Producer {
            continue;
        }
        if spec.heat_output > 0.0 {
            // HeatProducerBuild.updateTile: heat approaches heatOutput *
            // efficiency at warmupRate (0.15) regardless of efficiency.
            let can_run = efficiency > 0.0
                && (spec.craft_time > 0.0 && spec.item_inputs.is_empty()
                    || heat_inputs_available(&snapshot, &spec));
            let target = if can_run {
                spec.heat_output * efficiency
            } else {
                0.0
            };
            if let Some(mut tile) = world.tiles.get_mut(key) {
                tile.mass_driver_rotation = if spec.block == 418 {
                    target
                } else {
                    let step = 0.15 * delta_ticks.max(0.0);
                    let current = tile.mass_driver_rotation;
                    if (target - current).abs() <= step {
                        target
                    } else if current < target {
                        current + step
                    } else {
                        current - step
                    }
                };
                tile.output_liquid_amount = tile.mass_driver_rotation;
                changed = true;
                producer_heat.insert(*key, tile.mass_driver_rotation);
            }
        } else {
            producer_heat.insert(*key, snapshot.mass_driver_rotation);
        }
    }
    // Pass 2: conductors receive/propagate heat; consumers craft when they
    // receive enough heat.
    let mut memo = std::collections::HashMap::new();
    for key in &keys {
        let Some(snapshot) = world.tiles.get(key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(spec) = heat_block_spec(snapshot.block) else {
            continue;
        };
        let received = heat_value_at(world, *key, &producer_heat, &mut memo, &mut Vec::new());
        if matches!(spec.kind, HeatKind::Conductor) {
            // Store the received heat on the conductor tile (mirrors
            // HeatBlock storage; used by erekir_heat_at and consumers).
            if let Some(mut tile) = world.tiles.get_mut(key) {
                if (tile.mass_driver_rotation - received).abs() > 0.0001 {
                    tile.mass_driver_rotation = received;
                    changed = true;
                }
            }
            continue;
        }
        if spec.kind != HeatKind::Consumer {
            continue;
        }
        let efficiency = power.get(key).copied().unwrap_or(0.0);
        if efficiency <= 0.0 {
            continue;
        }
        if received + 0.0001 < spec.heat_requirement {
            continue;
        }
        // Advance craft progress like GenericCrafter (edelta == 1.0/tick).
        let crafted = if let Some(mut tile) = world.tiles.get_mut(key) {
            tile.production_progress += delta_ticks * efficiency;
            if tile.production_progress >= spec.craft_time.max(0.0001) {
                tile.production_progress %= spec.craft_time.max(0.0001);
                true
            } else {
                false
            }
        } else {
            false
        };
        if crafted {
            changed |= heat_apply_craft(world, *key, &spec);
        }
    }
    changed
}

#[derive(Clone, Copy)]
pub(crate) struct ErekirDrillSpec {
    pub(crate) tier: u8,
    pub(crate) drill_time: f32,
    pub(crate) size: u8,
    pub(crate) range: u8,
    pub(crate) item_capacity: i32,
    pub(crate) booster_liquid: i16,
}

pub(crate) fn erekir_drill_spec(block: i16) -> Option<ErekirDrillSpec> {
    match block {
        335 => Some(ErekirDrillSpec {
            tier: 3,
            drill_time: 160.0,
            size: 2,
            range: 5,
            item_capacity: 10,
            booster_liquid: 8, // hydrogen
        }),
        336 => Some(ErekirDrillSpec {
            tier: 5,
            drill_time: 100.0,
            size: 3,
            range: 6,
            item_capacity: 20,
            booster_liquid: 9, // nitrogen
        }),
        _ => None,
    }
}

/// Wall-ore drop of the floor at `position` (v158.1 floor ids) with the
/// official item hardness used by the tier check.
pub(crate) fn wall_ore_drop(world: &DynamicWorld, position: i32) -> Option<(i16, u8)> {
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    if x < 0 || y < 0 || x >= world.width || y >= world.height {
        return None;
    }
    let index = (y * world.width + x) as usize;
    match world.floors[index] {
        175 | 176 => Some((7, 4)), // ore-crystal-thorium / ore-wall-thorium
        177 => Some((16, 3)),      // ore-wall-beryllium
        179 => Some((3, 1)),       // ore-wall-graphite
        180 => Some((17, 5)),      // ore-wall-tungsten
        _ => None,
    }
}

/// The mineable facing tiles of a beam drill: one lane per size, scanning
/// `range` tiles forward until the first solid tile (BeamDrill.updateFacing).
pub(crate) fn beam_drill_facing(
    world: &DynamicWorld,
    snapshot: &DynamicTile,
    spec: &ErekirDrillSpec,
) -> Vec<(i32, i16)> {
    let mut facing = Vec::new();
    let perpendicular = (snapshot.rotation + 1) % 4;
    for lane in 0..spec.size {
        let lane_offset = lane as i32 - (spec.size as i32 / 2);
        let origin = erekir_offset_by(snapshot.position, perpendicular, lane_offset);
        for step in 0..spec.range {
            let position = erekir_offset_by(origin, snapshot.rotation, i32::from(step));
            if let Some((item, hardness)) = wall_ore_drop(world, position) {
                if hardness <= spec.tier {
                    facing.push((position, item));
                }
                break;
            }
            // Stop at the first solid (non-floor) tile without a wall ore.
            let x = (position >> 16) as i16 as i32;
            let y = position as i16 as i32;
            if x < 0 || y < 0 || x >= world.width || y >= world.height {
                break;
            }
            if crate::game::content::block_navigation(world.floors[(y * world.width + x) as usize])
                .solid
            {
                break;
            }
        }
    }
    facing
}

pub(crate) fn simulate_erekir_drills(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| erekir_drill_spec(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(spec) = erekir_drill_spec(snapshot.block) else {
            continue;
        };
        let facing = beam_drill_facing(world, &snapshot, &spec);
        if facing.is_empty() {
            continue;
        }
        if inventory_total(&snapshot.inventory) >= spec.item_capacity {
            continue;
        }
        let power_eff = power.get(&key).copied().unwrap_or(0.0);
        if power_eff <= 0.0 {
            continue;
        }
        // optionalEfficiency: booster liquid present (hydrogen/nitrogen).
        let optional_efficiency = f32::from(
            snapshot.stored_liquid == spec.booster_liquid && snapshot.liquid_amount > 0.0001,
        );
        let multiplier = 1.0 + (2.5 - 1.0) * optional_efficiency; // optionalBoostIntensity 2.5
        let completed = if let Some(mut drill) = world.tiles.get_mut(&key) {
            drill.transport_progress +=
                delta_ticks * building_time_scale(world, key) * multiplier * power_eff;
            if drill.transport_progress >= spec.drill_time {
                drill.transport_progress %= spec.drill_time;
                true
            } else {
                false
            }
        } else {
            false
        };
        changed = true;
        if completed {
            let mut added = false;
            if let Some(mut drill) = world.tiles.get_mut(&key) {
                for (_, item) in &facing {
                    if inventory_total(&drill.inventory) >= spec.item_capacity {
                        break;
                    }
                    inventory_add(&mut drill.inventory, *item, 1);
                    added = true;
                }
            }
            if added {
                changed |= dump_erekir_drill(world, key);
            }
        }
    }
    changed
}

/// Dump one mined item to an adjacent duct first, then to any acceptor
/// (BeamDrillBuild.dump / DrillBuild.dump).
pub(crate) fn dump_erekir_drill(world: &DynamicWorld, key: i32) -> bool {
    let Some(drill) = world.tiles.get(&key).map(|tile| tile.clone()) else {
        return false;
    };
    if drill.inventory.is_empty() {
        return false;
    }
    let item = drill.inventory[0].0;
    for position in &drill.occupied {
        for rotation in 0..4 {
            let target = offset_position(*position, rotation);
            if drill.occupied.contains(&target) {
                continue;
            }
            if deliver_item_to(world, target, item, key) {
                if let Some(mut live) = world.tiles.get_mut(&key) {
                    inventory_remove(&mut live.inventory, item, 1);
                }
                return true;
            }
        }
    }
    false
}

// ===========================================================================
// EREKIR: TURRETS (367-376)
// ===========================================================================
// Reload/range/ammo from Blocks.java v158.1; bullet ids are the registered
// content order (verified with InspectBullets against desktop.jar 158.1:
// id = 113 + creation index; breach=163.., diffuse=167.., sublimate=170..,
// titan=172.., disperse=176.., afflict=181/182, lustre=183, scathe missile
// payloads=185/188/191, smite=193/194, malign=196).
// afflict (heatRequirement 20) and malign (heatRequirement 144) only fire
// with input heat (Turret.canConsume / updateEfficiencyMultiplier).

#[derive(Clone, Copy)]
pub(crate) struct ErekirTurretAmmo {
    pub(crate) multiplier: f32,
    pub(crate) bullet_id: i16,
    pub(crate) damage: f32,
    pub(crate) speed: f32,
    pub(crate) splash_damage: f32,
    pub(crate) splash_radius: f32,
    pub(crate) pierce: bool,
}

pub(crate) fn is_erekir_turret_block(block: i16) -> bool {
    matches!(block, 367..=376)
}

/// (item -> ammo) tables per turret, from Blocks.java ammo().
pub(crate) fn erekir_turret_ammo_spec(block: i16, item: i16) -> Option<ErekirTurretAmmo> {
    let ammo = match (block, item) {
        // breach: beryllium/tungsten/carbide, pierce 2, ammoMultiplier 1.
        (367, 16) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 163,
            damage: 85.0,
            speed: 7.5,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: true,
        },
        (367, 17) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 164,
            damage: 95.0,
            speed: 8.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: true,
        },
        (367, 19) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 165,
            damage: 325.0 / 0.75,
            speed: 12.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: true,
        },
        // diffuse: graphite/oxide/silicon.
        (368, 3) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 167,
            damage: 41.0,
            speed: 8.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        (368, 18) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 168,
            damage: 90.0,
            speed: 8.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        (368, 9) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 169,
            damage: 35.0,
            speed: 8.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        // titan: thorium/carbide/oxide artillery.
        (370, 7) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 172,
            damage: 350.0,
            speed: 2.5,
            splash_damage: 350.0,
            splash_radius: 65.0,
            pierce: false,
        },
        (370, 19) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 173,
            damage: 700.0,
            speed: 3.25,
            splash_damage: 700.0,
            splash_radius: 65.0,
            pierce: false,
        },
        (370, 18) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 175,
            damage: 300.0,
            speed: 2.5,
            splash_damage: 300.0,
            splash_radius: 65.0,
            pierce: false,
        },
        // disperse: tungsten/thorium/silicon/surge-alloy, ammoMultiplier 3.
        (371, 17) => ErekirTurretAmmo {
            multiplier: 3.0,
            bullet_id: 176,
            damage: 65.0,
            speed: 8.5,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        (371, 7) => ErekirTurretAmmo {
            multiplier: 3.0,
            bullet_id: 177,
            damage: 90.0,
            speed: 8.5,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        (371, 9) => ErekirTurretAmmo {
            multiplier: 3.0,
            bullet_id: 178,
            damage: 37.0,
            speed: 9.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        (371, 12) => ErekirTurretAmmo {
            multiplier: 3.0,
            bullet_id: 179,
            damage: 65.0,
            speed: 8.5,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        // scathe: carbide/phase-fabric/surge-alloy -> missile payloads.
        (374, 19) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 185,
            damage: 1_000.0,
            speed: 4.6,
            splash_damage: 1_000.0,
            splash_radius: 65.0,
            pierce: false,
        },
        (374, 11) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 188,
            damage: 320.0,
            speed: 4.6,
            splash_damage: 320.0,
            splash_radius: 120.0,
            pierce: false,
        },
        (374, 12) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 191,
            damage: 1_800.0,
            speed: 4.6,
            splash_damage: 1_800.0,
            splash_radius: 40.0,
            pierce: false,
        },
        // smite: surge-alloy, pierce 4 + lightning.
        (375, 12) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 193,
            damage: 250.0,
            speed: 7.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: true,
        },
        _ => return None,
    };
    Some(ammo)
}

/// Liquid ammo for sublimate (ContinuousLiquidTurret): ozone/cyanogen.
pub(crate) fn erekir_liquid_turret_ammo(block: i16, liquid: i16) -> Option<ErekirTurretAmmo> {
    let ammo = match (block, liquid) {
        (369, 7) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 170,
            damage: 60.0,
            speed: 3.5,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        (369, 10) => ErekirTurretAmmo {
            multiplier: 1.0,
            bullet_id: 171,
            damage: 130.0,
            speed: 3.5,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        _ => return None,
    };
    Some(ammo)
}

/// (reload, range, shots, ammo_per_shot, air_only, ground_only,
///  heat_requirement, power_demand) per Erekir turret (Blocks.java).
type ErekirTurretParams = (f32, f32, u8, f32, bool, bool, f32, f32);

pub(crate) fn erekir_turret_params(block: i16) -> Option<ErekirTurretParams> {
    let params = match block {
        367 => (40.0, 190.0, 1, 2.0, false, false, 0.0, 0.0), // breach
        368 => (30.0, 125.0, 15, 3.0, false, false, 0.0, 0.0), // diffuse: ShootSpread(15, 4deg)
        369 => (5.0, 130.0, 1, 1.0, false, false, 0.0, 0.0),  // sublimate (continuous)
        370 => (60.0, 390.0, 1, 4.0, false, true, 0.0, 0.0),  // titan
        371 => (9.0, 310.0, 4, 4.0, true, false, 0.0, 0.0),   // disperse (air only)
        372 => (50.0, 368.0, 1, 0.0, false, false, 20.0, 1.0), // afflict
        373 => (10.0, 250.0, 1, 0.0, false, false, 0.0, 1.0), // lustre (continuous laser)
        374 => (600.0, 1350.0, 1, 15.0, false, true, 0.0, 0.0), // scathe
        375 => (100.0, 300.0, 5, 2.0, false, false, 0.0, 0.0), // smite (5 barrels)
        376 => (3.5, 410.0, 1, 0.0, false, false, 144.0, 1.0), // malign
        _ => return None,
    };
    Some(params)
}

/// Power-turret shoot types (afflict/lustre/malign) from Blocks.java.
pub(crate) fn erekir_power_turret_weapon(block: i16) -> Option<ErekirTurretAmmo> {
    let ammo = match block {
        372 => ErekirTurretAmmo {
            multiplier: 0.0,
            bullet_id: 181,
            damage: 180.0,
            speed: 5.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        373 => ErekirTurretAmmo {
            multiplier: 0.0,
            bullet_id: 183,
            damage: 210.0,
            speed: 0.0,
            splash_damage: 0.0,
            splash_radius: 0.0,
            pierce: false,
        },
        376 => ErekirTurretAmmo {
            multiplier: 0.0,
            bullet_id: 196,
            damage: 70.0,
            speed: 8.0,
            splash_damage: 70.0,
            splash_radius: 12.0,
            pierce: false,
        },
        _ => return None,
    };
    Some(ammo)
}

/// Pull a ready ammo item from a conveyor/duct neighbour pointing at the
/// turret (conveyors and ducts cannot deliver to Erekir turrets through the
/// Serpulo funnel, so the turret picks the item up itself).
pub(crate) fn erekir_turret_pull_ammo(world: &DynamicWorld, position: i32) -> bool {
    for rotation in 0..4 {
        let source = offset_position(position, rotation);
        if let Some(item) = ready_source_item(world, source, position) {
            if erekir_turret_accept_ammo(world, position, item) {
                remove_source_item(world, source, item);
                return true;
            }
        }
        // A duct with a ready item also feeds the turret.
        // Snapshot then mutate: DashMap shard locks are not reentrant, so the
        // `tiles.get` guard must not overlap `erekir_turret_accept_ammo` /
        // `tiles.get_mut` (they can land on the same shard and deadlock on
        // 2-CPU runners even when a many-core desktop does not).
        let duct = world.tiles.get(&source).map(|t| {
            (
                t.block,
                t.stored_amount,
                t.transport_progress,
                t.rotation,
                t.stored_item,
            )
        });
        match duct {
            Some((source_block, stored_amount, transport_progress, source_rotation, item))
                if is_erekir_duct_block(source_block)
                    && stored_amount > 0
                    && transport_progress >= 1.0 - 1.0 / duct_speed(source_block) - 0.01
                    && offset_position(source, source_rotation) == position
                    && erekir_turret_accept_ammo(world, position, item) =>
            {
                if let Some(mut tile) = world.tiles.get_mut(&source) {
                    tile.stored_amount = 0;
                    tile.stored_item = -1;
                    tile.transport_progress = 0.0;
                }
                return true;
            }
            _ => {}
        }
    }
    false
}

pub(crate) fn simulate_erekir_turrets(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| is_erekir_turret_block(tile.block))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some((
            reload,
            range,
            shots,
            ammo_per_shot,
            air_only,
            ground_only,
            heat_requirement,
            power_demand,
        )) = erekir_turret_params(snapshot.block)
        else {
            continue;
        };
        // Heat-gated turrets: canConsume() = false without input heat.
        let heat = if heat_requirement > 0.0 {
            erekir_heat_at(world, key)
        } else {
            0.0
        };
        if heat_requirement > 0.0 && heat <= 0.0 {
            continue;
        }
        // Ammo selection: item / liquid / power.
        let liquid_ammo = erekir_liquid_turret_ammo(snapshot.block, snapshot.stored_liquid);
        let ammo = erekir_turret_ammo_spec(snapshot.block, snapshot.stored_item)
            .or_else(|| erekir_power_turret_weapon(snapshot.block))
            .or(liquid_ammo);
        let Some(ammo) = ammo else {
            continue;
        };
        let available = if liquid_ammo.is_some() {
            snapshot.liquid_amount
        } else {
            snapshot.ammo_units
        };
        if ammo_per_shot > 0.0 && available < ammo_per_shot {
            erekir_turret_pull_ammo(world, key);
            let refreshed = world.tiles.get(&key).map(|tile| tile.clone());
            if let Some(refreshed) = refreshed {
                let fresh_available = if liquid_ammo.is_some() {
                    refreshed.liquid_amount
                } else {
                    refreshed.ammo_units
                };
                if fresh_available < ammo_per_shot {
                    continue;
                }
            } else {
                continue;
            }
        }
        // Automatic targeting or the latest player-controlled BlockUnit aim.
        // Manual fire remains valid over empty ground; direct target_id is an
        // optimization while the projectile ray still resolves buildings and
        // PvP players authoritatively.
        let turret_x = (snapshot.position >> 16) as i16 as f32 * 8.0;
        let turret_y = snapshot.position as i16 as f32 * 8.0;
        let effective_range = if snapshot.block == 369 && snapshot.stored_liquid == 10 {
            range + 70.0 // cyanogen rangeChange
        } else {
            range
        };
        let valid_target =
            |enemy: &EnemyUnit| turret_target_allowed(enemy.unit_type, air_only, ground_only);
        let target = match controlled_building_weapon_input(
            world,
            key,
            snapshot.team,
            turret_x,
            turret_y,
            effective_range,
            valid_target,
        ) {
            ControlledWeaponInput::Idle => None,
            ControlledWeaponInput::Firing(aim) => Some((aim.target_id, aim.distance, aim.x, aim.y)),
            ControlledWeaponInput::Automatic => world
                .enemies
                .iter()
                .filter_map(|enemy| {
                    if enemy.team == snapshot.team || !valid_target(&enemy) {
                        return None;
                    }
                    let distance = (enemy.x - turret_x).hypot(enemy.y - turret_y);
                    (distance <= effective_range).then_some((enemy.id, distance, enemy.x, enemy.y))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1)),
        };
        let Some((target_id, distance, target_x, target_y)) = target else {
            continue;
        };
        let heat_eff = if heat_requirement > 0.0 {
            (heat / heat_requirement).min(3.0) // maxHeatEfficiency 3
        } else {
            1.0
        };
        let efficiency = if power_demand > 0.0 {
            power.get(&key).copied().unwrap_or(0.0) * heat_eff
        } else {
            1.0
        };
        if efficiency <= 0.0 {
            continue;
        }
        let ready = if let Some(mut turret) = world.tiles.get_mut(&key) {
            turret.production_progress +=
                delta_ticks * building_time_scale(world, key) * efficiency;
            if turret.production_progress >= reload {
                turret.production_progress %= reload;
                if liquid_ammo.is_some() {
                    turret.liquid_amount = (turret.liquid_amount - ammo_per_shot).max(0.0);
                    if turret.liquid_amount <= 0.0001 {
                        turret.liquid_amount = 0.0;
                        turret.stored_liquid = -1;
                    }
                } else if ammo_per_shot > 0.0 {
                    turret.ammo_units = (turret.ammo_units - ammo_per_shot).max(0.0);
                    turret.stored_amount =
                        (turret.ammo_units / ammo.multiplier.max(0.0001)).ceil() as i32;
                    if turret.ammo_units <= 0.0 {
                        turret.stored_item = -1;
                        turret.stored_amount = 0;
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        changed = true;
        if ready {
            for _ in 0..shots {
                let projectile_id = spawn_projectile_for_team(
                    world,
                    out,
                    Some(key),
                    target_id,
                    ammo.bullet_id,
                    turret_x,
                    turret_y,
                    target_x,
                    target_y,
                    // BulletType.damage is per projectile, not a volley total.
                    ammo.damage,
                    ammo.speed,
                    distance,
                    1.0,
                    snapshot.team,
                );
                if let Some(mut projectile) = world.projectiles.get_mut(&projectile_id) {
                    projectile.splash_damage = ammo.splash_damage;
                    projectile.splash_radius = ammo.splash_radius;
                    projectile.pierce_units = if ammo.pierce { 2 } else { 0 };
                    projectile.pierce_buildings = if ammo.pierce { 2 } else { 0u8 };
                }
            }
        }
    }
    changed
}

/// Official UnitAssembler (Blocks.java v158.1): assembles a large unit from a
/// plan (AssemblerUnitPlan) while adjacent UnitAssemblerModule(396) blocks
/// provide the tier. Plans (block -> tier 0 / tier 1):
///   393 tank-assembler: vanquish(41) 50s | conquer(42) 180s
///   394 ship-assembler: quell(52) 60s   | disrupt(54) 180s
///   395 mech-assembler: tecta(47) 70s   | collaris(48) 180s
/// The port does not model per-unit payload stock; the plan's payload
/// requirements are represented as item requirements drawn from the owning
/// team's core (materials equivalent), and the assembled unit spawns in front
/// of the assembler like a factory spawn.
pub(crate) fn simulate_erekir_assemblers(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 393..=395))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        // Official UnitAssemblerBuild (UnitAssembler.java:315-390): currentTier
        // starts at 0 and the base (tier 0) plan is buildable WITHOUT any
        // module; adjacent UnitAssemblerModules RAISE the effective plan tier
        // (checkTier/plan). plan() clamps to the last plan, so tier 2+ behaves
        // as tier 1 for the two-plan tank/ship/mech assemblers.
        let tier = assembler_tier(world, &snapshot).min(1);
        let Some((unit_type, build_time, item_reqs)) = assembler_plan(snapshot.block, tier) else {
            continue;
        };
        let efficiency = power.get(&key).copied().unwrap_or(0.0);
        if efficiency <= 0.0 {
            continue;
        }
        // Requirements drawn from the team's core.
        let team = snapshot.team;
        let items = crate::network::economy::items_for_team(world, team);
        let affordable = item_reqs
            .iter()
            .all(|(item, amount)| items.get(*item as usize).copied().unwrap_or(0) >= *amount);
        if !affordable {
            continue;
        }
        let completed = if let Some(mut asm) = world.tiles.get_mut(&key) {
            asm.production_progress += delta_ticks * efficiency;
            if asm.production_progress >= build_time {
                asm.production_progress %= build_time;
                true
            } else {
                false
            }
        } else {
            false
        };
        if completed {
            // Consume the item requirements from the REAL team core inventory
            // (items_for_team returns a clone; deducting on that copy never
            // reached the actual store — economy.rs SOL-010 mutation bug).
            {
                let mut items = crate::network::economy::items_for_team_mut(world, team);
                for (item, amount) in item_reqs {
                    if let Some(stored) = items.get_mut(*item as usize) {
                        *stored = stored.saturating_sub(*amount);
                    }
                }
            }
            // Spawn the assembled unit in front of the assembler.
            if let Some(tile) = world.tiles.get(&key).map(|t| t.clone()) {
                spawn_factory_unit(world, out, &tile, unit_type);
            }
            changed = true;
        }
    }
    changed
}

/// UnitAssembler plan table (Blocks.java v158.1). `tier` 0 is the default.
/// Payload requirements are mapped to equivalent item ids from the team core:
/// the plan's unit payloads map to their build cost materials (approximated).
/// (unit_type, build_time_ticks, item_requirements).
pub(crate) type AssemblerPlan = (i16, f32, &'static [(i16, i32)]);

pub(crate) fn assembler_plan(block: i16, tier: usize) -> Option<AssemblerPlan> {
    match (block, tier) {
        // tank-assembler: vanquish (41) 50s; conquer (42) 180s
        // (conquer=42, cleroi=44 per parse_unit_type/unit_weapons.tsv).
        (393, 0) => Some((41, 60.0 * 50.0, &[(16, 40), (9, 40)])), // beryllium, silicon
        (393, 1) => Some((42, 60.0 * 180.0, &[(18, 60), (10, 40)])), // oxide, phase
        // ship-assembler: quell (52) 60s; disrupt (54) 180s.
        (394, 0) => Some((52, 60.0 * 60.0, &[(16, 50), (3, 50)])), // beryllium, graphite
        (394, 1) => Some((54, 60.0 * 180.0, &[(18, 50), (10, 30)])),
        // mech-assembler: tecta (47) 70s; collaris (48) 180s.
        (395, 0) => Some((47, 60.0 * 70.0, &[(16, 50), (17, 40)])), // beryllium, tungsten
        (395, 1) => Some((48, 60.0 * 180.0, &[(18, 40), (10, 40)])),
        _ => None,
    }
}

/// Whether two tiles' footprints are adjacent (official module adjacency).
pub(crate) fn tiles_adjacent(a: &DynamicTile, b: &DynamicTile) -> bool {
    a.occupied.iter().any(|pa| {
        b.occupied.iter().any(|pb| {
            let ax = (*pa >> 16) as i16 as i32;
            let ay = *pa as i16 as i32;
            let bx = (*pb >> 16) as i16 as i32;
            let by = *pb as i16 as i32;
            (ax - bx).abs() + (ay - by).abs() == 1
        })
    })
}

/// UnitAssemblerModule tier (UnitAssemblerModule.java:21-24:
/// `public int tier = 1;` — vanilla has only the basic-assembler-module).
pub(crate) fn module_tier(block: i16) -> usize {
    if block == 396 {
        1
    } else {
        0
    }
}

/// Effective UnitAssembler plan tier, mirroring
/// UnitAssembler.UnitAssemblerBuild.checkTier() (UnitAssembler.java:315-390):
/// `currentTier` starts at 0 (the base plan needs NO module) and adjacent
/// modules raise it only when a module's tier equals the running max or
/// max + 1 (sorted ascending; a tier gap stops the chain, matching the
/// official loop). All vanilla modules are tier 1, so one adjacent module
/// yields tier 1.
pub(crate) fn assembler_tier(world: &DynamicWorld, assembler: &DynamicTile) -> usize {
    let mut module_tiers: Vec<usize> = world
        .tiles
        .iter()
        .filter(|tile| {
            let t = tile.value();
            t.block == 396 && t.team == assembler.team && tiles_adjacent(t, assembler)
        })
        .map(|tile| module_tier(tile.block))
        .collect();
    module_tiers.sort_unstable();
    let mut max = 0usize;
    for tier in module_tiers {
        if tier == max || tier == max + 1 {
            max = tier;
        } else {
            break;
        }
    }
    max
}

/// Erekir GenericCrafter / HeatCrafter / HeatProducer (Blocks.java v158.1):
///   199 silicon-arc-furnace: 1 graphite + 4 sand -> 4 silicon, craftTime 50s,
///      power 5, itemCapacity 30 (item -> item).
///   200 electrolyzer: consumes water 10/60 continuously, outputs ozone 4/60 +
///      hydrogen 6/60 continuously (official GenericCrafterBuild.updateTile
///      `outputLiquids` per tick), power 1. The port stores one liquid per
///      tile, so both outputs are delivered straight to adjacent acceptors.
///   201 atmospheric-concentrator: HeatCrafter, heatRequirement 24, power 2,
///      outputs nitrogen 16/60 continuously while heat >= 24.
///   202 oxidation-chamber: HeatProducer, consumes ozone 2/60 + 1 beryllium,
///      outputs 1 oxide per 120s craft, power 0.5.
pub(crate) fn simulate_erekir_crafters(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 199..=202))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let efficiency = power.get(&key).copied().unwrap_or(1.0);
        match snapshot.block {
            199 => {
                // silicon-arc-furnace: needs 1 graphite (3) + 4 sand (4) in
                // the local inventory and room for 4 silicon (9).
                if inventory_count(&snapshot.inventory, 3) < 1
                    || inventory_count(&snapshot.inventory, 4) < 4
                {
                    if inventory_count(&snapshot.inventory, 9) > 0 {
                        let _ = dump_factory_output(world, key, 9);
                    }
                    continue;
                }
                if inventory_total(&snapshot.inventory) - 5 + 4 > 30 {
                    continue;
                }
                if efficiency <= 0.0 {
                    continue;
                }
                let crafted = if let Some(mut factory) = world.tiles.get_mut(&key) {
                    factory.production_progress +=
                        delta_ticks * building_time_scale(world, key) * efficiency;
                    if factory.production_progress >= 50.0 {
                        factory.production_progress %= 50.0;
                        inventory_remove(&mut factory.inventory, 3, 1);
                        inventory_remove(&mut factory.inventory, 4, 4);
                        inventory_add(&mut factory.inventory, 9, 4);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if crafted {
                    let _ = dump_factory_output(world, key, 9);
                }
                changed = true;
            }
            200 => {
                // electrolyzer: needs water stored, then continuously turns
                // 10/60 water/tick into ozone (7) 4/60 + hydrogen (8) 6/60.
                if snapshot.stored_liquid != 0 || snapshot.liquid_amount < 0.001 {
                    continue;
                }
                if efficiency <= 0.0 {
                    continue;
                }
                let inc = delta_ticks * building_time_scale(world, key) * efficiency;
                let consumed = (10.0 / 60.0) * inc;
                let ozone_out = (4.0 / 60.0) * inc;
                let hydrogen_out = (6.0 / 60.0) * inc;
                if let Some(mut factory) = world.tiles.get_mut(&key) {
                    factory.liquid_amount = (factory.liquid_amount - consumed).max(0.0);
                    if factory.liquid_amount <= 0.0001 {
                        factory.liquid_amount = 0.0;
                        factory.stored_liquid = -1;
                    }
                }
                let mut remaining_ozone = ozone_out;
                let mut remaining_hydrogen = hydrogen_out;
                for rotation in 0..4 {
                    let target = offset_position(key, rotation);
                    if snapshot.occupied.contains(&target) {
                        continue;
                    }
                    if remaining_ozone > 0.0001 {
                        let accepted =
                            accept_liquid_from(world, Some(key), target, 7, remaining_ozone);
                        remaining_ozone -= accepted;
                    }
                    if remaining_hydrogen > 0.0001 {
                        let accepted =
                            accept_liquid_from(world, Some(key), target, 8, remaining_hydrogen);
                        remaining_hydrogen -= accepted;
                    }
                }
                changed = true;
            }
            201 => {
                // atmospheric-concentrator: HeatCrafter with heatRequirement
                // 24; efficiency = clamp(heat / 24, 0, 1) * power efficiency.
                let heat = erekir_heat_at(world, key);
                if heat < 24.0 {
                    continue;
                }
                let eff = (heat / 24.0).min(1.0) * efficiency;
                if eff <= 0.0 {
                    continue;
                }
                let inc = delta_ticks * building_time_scale(world, key) * eff;
                let nitrogen = (16.0 / 60.0) * inc;
                let mut remaining = nitrogen;
                for rotation in 0..4 {
                    let target = offset_position(key, rotation);
                    if snapshot.occupied.contains(&target) {
                        continue;
                    }
                    if remaining > 0.0001 {
                        let accepted = accept_liquid_from(world, Some(key), target, 9, remaining);
                        remaining -= accepted;
                    }
                }
                changed = true;
            }
            202 => {
                // oxidation-chamber: needs ozone (7) stored + 1 beryllium
                // (16) in inventory; per 120s craft emits 1 oxide (18).
                if snapshot.stored_liquid != 7 || snapshot.liquid_amount < 0.001 {
                    if inventory_count(&snapshot.inventory, 18) > 0 {
                        let _ = dump_factory_output(world, key, 18);
                    }
                    continue;
                }
                if inventory_count(&snapshot.inventory, 16) < 1 {
                    continue;
                }
                if inventory_total(&snapshot.inventory) - 1 + 1 > 10 {
                    continue;
                }
                if efficiency <= 0.0 {
                    continue;
                }
                let crafted = if let Some(mut factory) = world.tiles.get_mut(&key) {
                    factory.production_progress +=
                        delta_ticks * building_time_scale(world, key) * efficiency;
                    if factory.production_progress >= 120.0 {
                        factory.production_progress %= 120.0;
                        // Continuous ozone consumption 2/60 * 120s craft.
                        factory.liquid_amount =
                            (factory.liquid_amount - (2.0 / 60.0) * 120.0).max(0.0);
                        if factory.liquid_amount <= 0.0001 {
                            factory.liquid_amount = 0.0;
                            factory.stored_liquid = -1;
                        }
                        inventory_remove(&mut factory.inventory, 16, 1);
                        inventory_add(&mut factory.inventory, 18, 1);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if crafted {
                    let _ = dump_factory_output(world, key, 18);
                }
                changed = true;
            }
            _ => {}
        }
    }
    changed
}
