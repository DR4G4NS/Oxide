//! Transport / logistics base simulation: conveyors, ducts, factories and
//! drills base maps. The economy facade re-exports these through
//! crate::network::economy::*.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::*;
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;

use super::*;

use crate::network::buildings::construction::dynamic_at;
use crate::network::economy::spec::{
    accept_logistics_item_from, dominant_drill_ore, drill_parameters, dump_drill_item,
    factory_recipe, incoming_bridge_sources, inventory_count, inventory_remove,
    item_transport_speed, offset_position, simulate_junctions, simulate_unloaders,
    valid_bridge_link,
};
use crate::network::economy::{items_for_team, items_for_team_mut};
pub(crate) fn is_plain_conveyor(block: i16) -> bool {
    matches!(block, 257 | 258 | 260)
}

pub(crate) fn conveyor_rotates(block: i16) -> bool {
    // Block.rotate on vanilla item transport that can feed a conveyor.
    matches!(
        block,
        257 | 258 | 260 | 261 | 264 | 265 | 266 | 267 | 268 | 269
    )
}

pub(crate) fn tile_relative_to(from: i32, to: i32) -> Option<u8> {
    let fx = (from >> 16) as i16 as i32;
    let fy = from as i16 as i32;
    let tx = (to >> 16) as i16 as i32;
    let ty = to as i16 as i32;
    if fx == tx && fy == ty - 1 {
        Some(1)
    } else if fx == tx && fy == ty + 1 {
        Some(3)
    } else if fx == tx - 1 && fy == ty {
        Some(0)
    } else if fx == tx + 1 && fy == ty {
        Some(2)
    } else {
        None
    }
}

pub(crate) fn conveyor_facing_edge(source: &DynamicTile, target: i32) -> Option<i32> {
    let occupied = if source.occupied.is_empty() {
        std::slice::from_ref(&source.position)
    } else {
        source.occupied.as_slice()
    };
    occupied
        .iter()
        .copied()
        .find(|&tile| tile_relative_to(tile, target).is_some())
}

/// `ConveyorBuild.acceptItem` reads the `minitem` cached by the last
/// `updateTile`, not the live rear offset. `handleItem` does not refresh it,
/// so a same-tick burst (StackConveyor `while (moveForward)`) still sees the
/// pre-insert value. Items parked at ys=0 are this-tick inserts.
pub(crate) fn conveyor_accept_minitem(items: &[(i16, f32)]) -> f32 {
    items
        .iter()
        .map(|(_, progress)| *progress)
        .filter(|progress| *progress > 1e-4)
        .fold(1.0, f32::min)
}

pub(crate) fn conveyor_side_insert_index(items: &[(i16, f32)]) -> usize {
    // ConveyorBuild.updateTile: mid starts at 0; walking Java's rear-first
    // array, any item with ys > 0.5 and i > 0 sets mid = i - 1.
    let len = items.len();
    let mut mid = 0usize;
    for java_index in (0..len).rev() {
        let rust_index = len - 1 - java_index;
        if items[rust_index].1 > 0.5 && java_index > 0 {
            mid = java_index - 1;
        }
    }
    len - mid
}

/// Official `ConveyorBuild.acceptItem` / `handleItem` (158.1). `clogHeat` is
/// visual/ambient only; the illegal-crossing gate is `minitem` plus the
/// rotating-source U-turn (`source.block.rotate && next == source`).
pub(crate) fn accept_plain_conveyor_item(
    world: &DynamicWorld,
    target_key: i32,
    item: i16,
    source: Option<i32>,
) -> bool {
    let Some(snapshot) = world.tiles.get(&target_key).map(|tile| tile.clone()) else {
        return false;
    };
    if !is_plain_conveyor(snapshot.block) {
        return false;
    }
    let mut items = if snapshot.conveyor_items.is_empty() {
        normalized_conveyor_items(&snapshot)
    } else {
        snapshot.conveyor_items.clone()
    };
    if items.len() >= CONVEYOR_CAPACITY {
        return false;
    }
    let minitem = conveyor_accept_minitem(&items);

    let mut side = false;
    if let Some(source_key) = source {
        let Some(source_tile) = dynamic_at(world, source_key)
            .or_else(|| world.tiles.get(&source_key).map(|tile| tile.clone()))
        else {
            return false;
        };
        let output = offset_position(snapshot.position, snapshot.rotation);
        if conveyor_rotates(source_tile.block)
            && dynamic_at(world, output).is_some_and(|next| next.position == source_tile.position)
        {
            return false;
        }
        let Some(facing) = conveyor_facing_edge(&source_tile, snapshot.position) else {
            return false;
        };
        let Some(rel) = tile_relative_to(facing, snapshot.position) else {
            return false;
        };
        let direction = (i32::from(rel) - i32::from(snapshot.rotation % 4)).unsigned_abs();
        let rear_ok = direction == 0 && minitem >= CONVEYOR_ITEM_SPACE;
        let side_ok = direction % 2 == 1 && minitem > 0.7;
        if !rear_ok && !side_ok {
            return false;
        }
        // ArmoredConveyorBuild.acceptItem: side inserts only from a Conveyor.
        if snapshot.block == 260
            && !is_plain_conveyor(source_tile.block)
            && rel != snapshot.rotation % 4
        {
            return false;
        }
        side = side_ok && !rear_ok;
    } else if minitem < CONVEYOR_ITEM_SPACE {
        return false;
    }

    if side {
        let index = conveyor_side_insert_index(&items).min(items.len());
        items.insert(index, (item, 0.5));
    } else {
        items.push((item, 0.0));
    }
    if let Some(mut conveyor) = world.tiles.get_mut(&target_key) {
        let front = items.first().copied();
        conveyor.conveyor_items = items;
        conveyor.stored_item = front.map(|(stored, _)| stored).unwrap_or(-1);
        conveyor.stored_amount = i32::try_from(conveyor.conveyor_items.len()).unwrap_or(i32::MAX);
        conveyor.transport_progress = front.map(|(_, progress)| progress).unwrap_or(0.0);
        true
    } else {
        false
    }
}

/// Return a protocol-safe, front-first conveyor queue.
///
/// Older checkpoints could contain progress values in the thousands because a
/// blocked belt advanced every item except its head indefinitely.  Project the
/// saved positions onto the official three-item, 0..=1 interval while keeping
/// FIFO order.  The backwards pass makes room for every trailing item; the
/// forwards pass caps the head and enforces `itemSpace` deterministically.
pub(crate) fn sanitize_conveyor_queue(items: &[(i16, f32)]) -> Vec<(i16, f32)> {
    let mut healed: Vec<_> = items
        .iter()
        .take(CONVEYOR_CAPACITY)
        .map(|(item, progress)| {
            let progress = if progress.is_finite() {
                progress.clamp(0.0, 1.0)
            } else {
                0.0
            };
            (*item, progress)
        })
        .collect();

    if healed.len() < 2 {
        return healed;
    }

    for index in (0..healed.len() - 1).rev() {
        healed[index].1 = healed[index]
            .1
            .max(healed[index + 1].1 + CONVEYOR_ITEM_SPACE);
    }
    healed[0].1 = healed[0].1.clamp(0.0, 1.0);
    for index in 1..healed.len() {
        let max_progress = (healed[index - 1].1 - CONVEYOR_ITEM_SPACE).max(0.0);
        healed[index].1 = healed[index].1.clamp(0.0, max_progress);
    }
    healed
}

/// Read either the current per-item representation or the legacy shared
/// fields, returning the same front-first representation used by Rust.  This
/// is also used by the snapshot codec so a freshly loaded legacy save cannot
/// send invalid offsets before its first simulation tick.
pub(crate) fn normalized_conveyor_items(tile: &DynamicTile) -> Vec<(i16, f32)> {
    if !tile.conveyor_items.is_empty() {
        return sanitize_conveyor_queue(&tile.conveyor_items);
    }
    if tile.stored_item < 0 || tile.stored_amount <= 0 {
        return Vec::new();
    }

    let amount = usize::try_from(tile.stored_amount)
        .unwrap_or(CONVEYOR_CAPACITY)
        .min(CONVEYOR_CAPACITY);
    let front = if tile.transport_progress.is_finite() {
        tile.transport_progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let legacy: Vec<_> = (0..amount)
        .map(|index| {
            (
                tile.stored_item,
                (front - index as f32 * CONVEYOR_ITEM_SPACE).max(0.0),
            )
        })
        .collect();
    sanitize_conveyor_queue(&legacy)
}

pub(crate) fn aligned_conveyor_next_max(world: &DynamicWorld, tile: &DynamicTile) -> f32 {
    let output = offset_position(tile.position, tile.rotation);
    let Some(next) = dynamic_at(world, output) else {
        return 1.0;
    };
    if !is_plain_conveyor(next.block)
        || next.team != tile.team
        || next.rotation % 4 != tile.rotation % 4
    {
        return 1.0;
    }

    // Official ConveyorBuild.updateTile:
    // nextMax = 1 - max(itemSpace - nextc.minitem, 0).
    let next_min = normalized_conveyor_items(&next)
        .last()
        .map(|(_, progress)| *progress)
        .unwrap_or(1.0);
    (1.0 - (CONVEYOR_ITEM_SPACE - next_min).max(0.0)).clamp(0.0, 1.0)
}

/// SOL-001: prebuilt map drills (base_buildings) produce their ore into the
/// owning team's core. The official map's conveyor network is not simulated,
/// so output feeds the core directly (documented approximation). Progress
/// uses the same official timing as dynamic drills (DrillBlock: delay =
/// drillTime + hardness*50, one item per delay ticks).
/// SOL-001: prebuilt map item factories (base_buildings with a
/// `factory_recipe`) craft from the owning team's core and deliver the
/// output to the core (documented approximation: the map's conveyor network
/// is not simulated). Timing matches official `craft_time` ticks.
pub(crate) fn simulate_base_factories(world: &DynamicWorld, delta_ticks: f32) -> bool {
    // Map entities are represented by tiles. Keep the base registry fallback
    // for old tests/saves, but never create a second inventory for a tile.
    let mut keys: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| factory_recipe(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    // Snapshot the base registry before consulting tiles; acquiring a second
    // DashMap shard from inside the base iterator can deadlock under load.
    let base_candidates: Vec<i32> = world
        .base_buildings
        .iter()
        .filter(|building| factory_recipe(building.block).is_some())
        .map(|building| *building.key())
        .collect();
    keys.extend(
        base_candidates
            .into_iter()
            .filter(|key| !world.tiles.contains_key(key)),
    );
    let mut changed = false;
    for key in keys {
        let tile_snapshot = world.tiles.get(&key).map(|tile| tile.clone());
        let (block, team, mut inventory) = if let Some(tile) = &tile_snapshot {
            (tile.block, tile.team, tile.inventory.clone())
        } else if let Some(building) = world.base_buildings.get(&key).map(|b| b.clone()) {
            (building.block, building.team, building.inventory)
        } else {
            continue;
        };
        let Some(recipe) = factory_recipe(block) else {
            continue;
        };
        let mut progress = *world
            .base_factory_progress
            .entry(key)
            .or_insert(0.0)
            .value();
        progress += delta_ticks;
        let mut crafted = 0;
        while progress >= recipe.craft_time {
            // Official consumption order is the building's ItemModule first,
            // then its team core. Work on a clone so a failed craft is atomic.
            let mut local = inventory.clone();
            let mut from_core = Vec::new();
            for (item, amount) in recipe.inputs {
                let owned = inventory_count(&local, *item).min(*amount);
                if owned > 0 {
                    let _ = inventory_remove(&mut local, *item, owned);
                }
                if owned < *amount {
                    from_core.push((*item, *amount - owned));
                }
            }
            let affordable = {
                let items = items_for_team(world, team);
                from_core.iter().all(|(item, amount)| {
                    items.get(*item as usize).copied().unwrap_or(0) >= *amount
                })
            };
            if !affordable {
                break;
            }
            if !from_core.is_empty() {
                let mut items = items_for_team_mut(world, team);
                for (item, amount) in from_core {
                    if let Some(stored) = items.get_mut(item as usize) {
                        *stored -= amount;
                    }
                }
            }
            if tile_snapshot.is_some() {
                if let Some(existing) = local.iter_mut().find(|(item, _)| *item == recipe.output.0)
                {
                    existing.1 += recipe.output.1;
                } else {
                    local.push((recipe.output.0, recipe.output.1));
                }
            } else {
                crate::network::core_inventory::deposit_core_items(
                    world,
                    team,
                    recipe.output.0,
                    recipe.output.1,
                );
            }
            inventory = local;
            progress -= recipe.craft_time;
            crafted += 1;
        }
        world.base_factory_progress.insert(key, progress);
        if let Some(tile) = tile_snapshot {
            if crafted > 0 {
                if let Some(mut live) = world.tiles.get_mut(&key) {
                    live.inventory = inventory;
                }
            }
            // A loaded tile is authoritative even when no craft completed.
            let _ = tile;
        } else if crafted > 0 {
            if let Some(mut live) = world.base_buildings.get_mut(&key) {
                live.inventory = inventory;
            }
        }
        changed |= crafted > 0;
    }
    changed
}

pub(crate) fn simulate_base_drills(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let keys: Vec<i32> = world
        .base_buildings
        .iter()
        .filter(|building| drill_parameters(building.block).is_some())
        .map(|building| *building.key())
        .collect();
    let mut changed = false;
    for key in keys {
        // Loaded MSAV buildings are registered in both the compatibility
        // base map and the authoritative DynamicTile registry. The dynamic
        // path below owns those drills; simulating this copy too duplicated
        // their production directly into the core.
        if world.tiles.contains_key(&key) {
            continue;
        }
        let Some(building) = world.base_buildings.get(&key).map(|b| b.clone()) else {
            continue;
        };
        let Some((tier, drill_time, _capacity)) = drill_parameters(building.block) else {
            continue;
        };
        let synthetic = DynamicTile {
            position: building.position,
            block: building.block,
            rotation: 0,
            config: Vec::new(),
            enabled: true,
            message: None,
            occupied: building.occupied.clone(),
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
            health: 1000.0,
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            team: building.team,
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
        };
        let Some((item, hardness, _ore_count)) = dominant_drill_ore(world, &synthetic, tier) else {
            continue;
        };
        let mut progress = *world.base_drill_progress.entry(key).or_insert(0.0).value();
        progress += delta_ticks;
        let delay = drill_time + hardness as f32 * 50.0;
        let mut produced = 0;
        while progress >= delay {
            progress -= delay;
            produced += 1;
        }
        world.base_drill_progress.insert(key, progress);
        if produced > 0 {
            crate::network::core_inventory::deposit_core_items(
                world,
                building.team,
                item,
                produced,
            );
            changed = true;
        }
    }
    changed
}

/// Vanilla Drill `liquidBoostIntensity` / water consume / `warmupSpeed`.
/// `progress += delta * oreCount * speed * warmup` with warmup approaching
/// `speed`, so a full water boost multiplies throughput by intensity²
/// (mechanical 1.6² = 2.56). timeDrilled/lastDrillSpeed are client-local.
pub(crate) fn drill_liquid_boost(block: i16) -> (f32, f32, f32) {
    match block {
        325 => (1.6, 0.05, 0.015),
        326 => (1.6, 3.5 / 60.0, 0.015),
        327 => (1.6, 0.08, 0.015),
        328 => (1.8, 0.1, 0.01),
        _ => (1.0, 0.0, 0.015),
    }
}

pub(crate) fn simulate_logistics(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world.tiles.iter().map(|tile| *tile.key()).collect();
    let mut changed = false;
    for key in &keys {
        let Some(tile) = world.tiles.get(key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some((tier, drill_time, capacity)) = drill_parameters(tile.block) else {
            continue;
        };
        let ore = dominant_drill_ore(world, &tile, tier);
        let efficiency = power
            .get(key)
            .copied()
            .unwrap_or(if matches!(tile.block, 325 | 326) {
                1.0
            } else {
                0.0
            })
            .clamp(0.0, 1.0);
        let (boost_intensity, water_rate, warmup_speed) = drill_liquid_boost(tile.block);
        let water_available =
            tile.stored_liquid == 0 && tile.liquid_amount > 0.0 && water_rate > 0.0;
        let optional_efficiency = if water_available { 1.0 } else { 0.0 };
        // DrillBuild.updateTile: speed = lerp(1, liquidBoostIntensity, optionalEfficiency) * efficiency.
        let speed = (1.0 + (boost_intensity - 1.0) * optional_efficiency) * efficiency;
        let operating = ore.is_some() && tile.stored_amount < capacity && efficiency > 0.0;
        if let Some(mut drill) = world.tiles.get_mut(key) {
            // Warmup approaches `speed` (not unit efficiency). Water boost
            // therefore drives warmup to 1.6/1.8; that value is the second
            // f32 in DrillBuild.write and lastDrillSpeed is derived from it.
            let target_warmup = if operating { speed } else { 0.0 };
            let warmup_step = warmup_speed * delta_ticks.max(0.0);
            let old_warmup = drill.transport_progress;
            drill.transport_progress = if old_warmup < target_warmup {
                (old_warmup + warmup_step).min(target_warmup)
            } else {
                (old_warmup - warmup_step).max(target_warmup)
            };
            changed |= (drill.transport_progress - old_warmup).abs() > f32::EPSILON;

            if operating {
                if water_available {
                    drill.liquid_amount =
                        (drill.liquid_amount - water_rate * delta_ticks.max(0.0)).max(0.0);
                    if drill.liquid_amount <= 0.0001 {
                        drill.liquid_amount = 0.0;
                        drill.stored_liquid = -1;
                    }
                }
                let (item, hardness, ore_count) = ore.expect("operating drill has ore");
                drill.production_progress += delta_ticks.max(0.0)
                    * building_time_scale(world, *key)
                    * ore_count as f32
                    * speed
                    * drill.transport_progress;
                let delay = drill_time + hardness as f32 * 50.0;
                if drill.production_progress >= delay {
                    drill.production_progress %= delay;
                    if drill.stored_amount == 0 || drill.stored_item == item {
                        drill.stored_item = item;
                        drill.stored_amount += 1;
                    }
                }
                changed = true;
            }
        }
        changed |= dump_drill_item(world, *key);
    }

    changed |= simulate_junctions(world, delta_ticks);
    changed |= simulate_unloaders(world, delta_ticks);

    for key in keys {
        let Some(tile) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(mut speed) = item_transport_speed(tile.block) else {
            continue;
        };
        if matches!(tile.block, 262 | 263) {
            let range = if tile.block == 262 { 4 } else { 12 };
            if valid_bridge_link(world, &tile, range).is_none() {
                // Unlinked bridge endpoints call Building.dump(), not the
                // bridge transport buffer. Vanilla dumpTime is five ticks.
                speed = 1.0 / 5.0;
            }
        }
        if is_plain_conveyor(tile.block) {
            let mut moved_items = normalized_conveyor_items(&tile);
            if moved_items.is_empty() {
                continue;
            }
            let raw_step = delta_ticks
                * building_time_scale(world, key)
                * speed
                * power.get(&key).copied().unwrap_or(1.0);
            let step = if raw_step.is_finite() {
                raw_step.max(0.0)
            } else {
                0.0
            };
            let next_max = aligned_conveyor_next_max(world, &tile);

            // Rust stores the FIFO head at index 0.  Move head-to-tail exactly
            // like official ConveyorBuild iterates len-1 down to zero: each
            // item is bounded by the updated item ahead and the head is bounded
            // by an aligned downstream conveyor's `minitem`.
            for index in 0..moved_items.len() {
                let max_progress = if index == 0 {
                    next_max
                } else {
                    (moved_items[index - 1].1 - CONVEYOR_ITEM_SPACE).max(0.0)
                };
                moved_items[index].1 = (moved_items[index].1 + step)
                    .min(max_progress)
                    .clamp(0.0, 1.0);
            }

            if moved_items
                .first()
                .is_some_and(|(_, progress)| *progress >= 1.0)
            {
                let item = moved_items[0].0;
                let output = offset_position(tile.position, tile.rotation);
                if accept_logistics_item_from(world, output, item, Some(tile.position), 0) {
                    moved_items.remove(0);
                }
            }

            let front = moved_items.first().copied();
            if let Some(mut conveyor) = world.tiles.get_mut(&key) {
                conveyor.conveyor_items = moved_items;
                conveyor.stored_item = front.map(|(item, _)| item).unwrap_or(-1);
                conveyor.stored_amount =
                    i32::try_from(conveyor.conveyor_items.len()).unwrap_or(i32::MAX);
                conveyor.transport_progress = front.map(|(_, progress)| progress).unwrap_or(0.0);
            }
            changed = true;
            continue;
        }
        // Per-item transport: conveyors 257/258/260 and bridges 262/263 keep
        // each item's own progress (conveyor_items), mirroring the official
        // ConveyorBuild `ids/xs/ys` arrays. The client animates those offsets
        // locally, so one shared progress made items jump in place. Routers
        // (266/267) keep the legacy single-slot behaviour.
        let is_router = matches!(tile.block, 266 | 267);
        let items_now: Vec<(i16, f32)> = if is_router {
            if tile.stored_amount > 0 {
                vec![(tile.stored_item, tile.transport_progress)]
            } else {
                Vec::new()
            }
        } else if !tile.conveyor_items.is_empty() {
            tile.conveyor_items.clone()
        } else if tile.stored_amount > 0 && tile.stored_item >= 0 {
            // Legacy save / seed: migrate the shared fields into per-item
            // positions, evenly spaced along the belt.
            let amount = tile.stored_amount.clamp(0, 14);
            (0..amount)
                .map(|i| {
                    let progress = (tile.transport_progress - i as f32 * 0.4).rem_euclid(1.0);
                    (tile.stored_item, progress)
                })
                .collect()
        } else {
            Vec::new()
        };
        if items_now.is_empty() {
            continue;
        }
        let efficiency = power.get(&key).copied().unwrap_or(1.0);
        let step = delta_ticks * building_time_scale(world, key) * speed * efficiency;
        // Advance every queued item; the first one that reaches the end is
        // delivered while the items queued behind it stay on the belt.
        let ready_index = items_now
            .iter()
            .position(|(_, progress)| progress + step >= 1.0);
        let mut moved_items: Vec<(i16, f32)> = Vec::with_capacity(items_now.len());
        let mut ready: Option<(i16, f32)> = None;
        for (index, (item, progress)) in items_now.into_iter().enumerate() {
            if Some(index) == ready_index {
                ready = Some((item, progress + step));
            } else {
                moved_items.push((item, progress + step));
            }
        }
        changed = true;
        if let Some((item, _)) = ready {
            // Deliver the front item, then keep the rest queued.
            let targets: Vec<(i32, Option<u8>)> = if matches!(tile.block, 262 | 263) {
                let range = if tile.block == 262 { 4 } else { 12 };
                if let Some(link) = valid_bridge_link(world, &tile, range) {
                    vec![(link, None)]
                } else {
                    let incoming = incoming_bridge_sources(world, &tile);
                    (0..4)
                        .map(|rotation| (offset_position(tile.position, rotation), Some(rotation)))
                        .filter(|(position, _)| !incoming.contains(position))
                        .collect()
                }
            } else if tile.block == 267 {
                let mut outputs = Vec::new();
                for occupied in &tile.occupied {
                    for rotation in 0..4 {
                        let target = offset_position(*occupied, rotation);
                        if !tile.occupied.contains(&target)
                            && !outputs
                                .iter()
                                .any(|(position, _): &(i32, Option<u8>)| *position == target)
                        {
                            outputs.push((target, Some(rotation)));
                        }
                    }
                }
                if !outputs.is_empty() {
                    let start = tile.rotation as usize % outputs.len();
                    outputs.rotate_left(start);
                }
                outputs
            } else if tile.block == 266 {
                (0..4)
                    .map(|step| (tile.rotation + step) % 4)
                    .map(|rotation| (offset_position(tile.position, rotation), Some(rotation)))
                    .collect()
            } else {
                vec![(
                    offset_position(tile.position, tile.rotation),
                    Some(tile.rotation),
                )]
            };
            let delivered = targets.into_iter().find(|(output, _)| {
                accept_logistics_item_from(world, *output, item, Some(tile.position), 0)
            });
            if let Some((_, rotation)) = delivered {
                if is_router {
                    if let Some(mut router) = world.tiles.get_mut(&key) {
                        router.stored_amount -= 1;
                        if let Some(rotation) = rotation {
                            router.rotation = (rotation + 1) % 4;
                        }
                        if router.stored_amount == 0 {
                            router.stored_item = -1;
                        }
                    }
                }
            } else {
                // Could not hand off: keep the front item at the far end and
                // let the rest advance behind it (next tick retries).  The
                // ready item was removed from `moved_items` above, so it must
                // be restored at index 0.  Appending it rotated a blocked
                // StackConveyor queue and allowed items behind the jam to
                // overtake its front.
                moved_items.insert(0, (item, 1.0 - f32::EPSILON));
            }
        }
        if is_router {
            if let Some(mut router) = world.tiles.get_mut(&key) {
                if !moved_items.is_empty() {
                    router.stored_item = moved_items[0].0;
                    router.stored_amount = 1;
                    router.transport_progress = moved_items[0].1;
                } else {
                    router.stored_item = -1;
                    router.stored_amount = 0;
                    router.transport_progress = 0.0;
                }
            }
        } else if let Some(mut conveyor) = world.tiles.get_mut(&key) {
            // Trim leading delivered items; the loop above stops at the first
            // ready item so moved_items holds everything still in transit.
            let front = moved_items.first().copied();
            conveyor.conveyor_items = moved_items;
            conveyor.stored_item = front.map(|(item, _)| item).unwrap_or(-1);
            conveyor.stored_amount =
                i32::try_from(conveyor.conveyor_items.len()).unwrap_or(i32::MAX);
            conveyor.transport_progress = front.map(|(_, p)| p).unwrap_or(0.0);
        }
    }
    changed
}
