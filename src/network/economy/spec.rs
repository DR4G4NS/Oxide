//! Block spec tables: drills, transport, turrets, factories, logistics,
//! junctions/unloaders/reactors, bridges, mass drivers and inventory helpers.
//! The listener adapter re-exports these through crate::network::listener::*.

use crate::network::economy::*;
use crate::network::units::*;
use crate::network::world::*;

use super::{dynamic_at, frame_generated_packet};
use crate::network::wire::auth::player_team;

use std::collections::HashSet;
use std::sync::Arc;

use crate::network::buildings::construction::base_block;
use crate::network::buildings::reactor;
use crate::network::economy::payload::constructor_item_capacity;
use crate::network::simulation::power::simulate_reactors_with_network;
use crate::network::wire::raw_mine_result;
use dashmap::DashMap;

pub(crate) fn drill_parameters(block: i16) -> Option<(u8, f32, i32)> {
    match block {
        325 => Some((2, 600.0, 10)),
        326 => Some((3, 400.0, 10)),
        327 => Some((4, 280.0, 10)),
        328 => Some((5, 280.0, 20)),
        337 => Some((6, 720.0, 40)),
        338 => Some((7, 281.25, 60)),
        _ => None,
    }
}

/// Official mono mineRange (158.1 MonoUnitType): the mono only mines (and
/// shows its beam, via mineTile) when within this distance of the ore tile.
pub(crate) const MONO_MINE_RANGE: f32 = 70.0;

pub(crate) fn item_transport_speed(block: i16) -> Option<f32> {
    match block {
        // Blocks.java 158.1: conveyor 0.046, titanium-conveyor 0.0801,
        // armored-conveyor 0.08. The client animates at these rates between
        // 6 s block snapshots; a slower server speed makes every correction
        // teleport items backward.
        257 => Some(0.046),
        258 => Some(0.0801),
        260 => Some(0.08),
        262 => Some(1.0 / 74.0),
        263 => Some(0.5),
        // Router.java:14 speed = 8f with time += 1/speed * delta(): one
        // item forwarded every 8 ticks (distributor shares the constant).
        266 | 267 => Some(0.125),
        // 259 (plastanium-conveyor) is a StackConveyor in 158.1 and is
        // driven by the stack machine in simulate_erekir_ducts, not by the
        // generic per-item conveyor mover.
        _ => None,
    }
}

pub(crate) fn item_transport_capacity(block: i16) -> Option<i32> {
    match block {
        259 => Some(10),
        262 => Some(14),
        263 => Some(10),
        257 | 258 | 260 => Some(3),
        266 | 267 => Some(1),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TurretAmmo {
    pub(crate) multiplier: f32,
    pub(crate) bullet_id: i16,
    pub(crate) damage: f32,
    pub(crate) speed: f32,
    pub(crate) range: f32,
    pub(crate) reload: f32,
    pub(crate) ammo_per_shot: f32,
}

pub(crate) fn turret_ammo(block: i16, item: i16) -> Option<TurretAmmo> {
    let (multiplier, bullet_id, damage, speed, range, reload, ammo_per_shot) = match (block, item) {
        // duo
        (349, 0) => (2.0, 113, 9.0, 2.5, 160.0, 20.0, 1.0),
        (349, 3) => (4.0, 114, 18.0, 3.5, 176.0, 20.0, 1.0),
        (349, 9) => (5.0, 115, 12.0, 3.0, 160.0, 20.0, 1.0),
        // scatter: consumeAmmoOnce=true, so one ammo unit pays for both shots.
        (350, 8) => (5.0, 116, 72.0, 4.0, 220.0, 18.0, 1.0),
        (350, 1) => (4.0, 117, 87.0, 4.2, 220.0, 18.0, 1.0),
        (350, 2) => (5.0, 118, 96.0, 4.0, 220.0, 18.0, 1.0),
        // scorch
        (351, 5) => (3.0, 120, 17.0, 3.35, 60.0, 6.0, 1.0),
        (351, 15) => (10.0, 121, 30.0, 4.0, 60.0, 6.0, 1.0),
        // hail (direct + splash damage)
        (352, 3) => (2.0, 122, 53.0, 3.0, 235.0, 60.0, 1.0),
        (352, 9) => (3.0, 123, 53.0, 3.0, 235.0, 60.0, 1.0),
        (352, 15) => (4.0, 124, 70.0, 3.0, 235.0, 60.0, 1.0),
        // salvo, four shots per volley
        (358, 0) => (5.0, 135, 60.0, 2.5, 190.0, 29.0, 4.0),
        (358, 3) => (4.0, 136, 124.0, 3.5, 222.0, 29.0, 4.0),
        (358, 9) => (5.0, 138, 92.0, 3.0, 190.0, 29.0, 4.0),
        (358, 7) => (4.0, 139, 112.0, 4.0, 190.0, 29.0, 4.0),
        (358, 15) => (5.0, 137, 160.0, 3.2, 190.0, 29.0, 4.0),
        // swarmer, four missiles
        (357, 14) => (5.0, 132, 220.0, 3.7, 240.0, 34.285_713, 4.0),
        (357, 15) => (5.0, 133, 228.0, 3.7, 240.0, 34.285_713, 4.0),
        (357, 12) => (4.0, 134, 212.0, 3.7, 240.0, 34.285_713, 4.0),
        // fuse: consumeAmmoOnce=true, so one ammo unit pays for all three beams.
        (361, 6) => (4.0, 144, 198.0, 0.0, 90.0, 35.0, 1.0),
        (361, 7) => (5.0, 145, 315.0, 0.0, 90.0, 35.0, 1.0),
        // ripple: ammoPerShot=2 and consumeAmmoOnce=true for the four-shell volley.
        (362, 3) => (2.0, 146, 440.0, 3.0, 290.0, 120.0, 2.0),
        (362, 9) => (3.0, 147, 440.0, 3.0, 290.0, 120.0, 2.0),
        (362, 15) => (4.0, 148, 552.0, 3.0, 290.0, 120.0, 2.0),
        (362, 14) => (4.0, 149, 520.0, 2.0, 290.0, 120.0, 2.0),
        (362, 10) => (2.0, 150, 520.0, 3.4, 290.0, 120.0, 2.0),
        // cyclone
        (363, 2) => (2.0, 152, 51.0, 4.0, 200.0, 10.0, 1.0),
        (363, 14) => (5.0, 154, 53.0, 4.0, 200.0, 10.0, 1.0),
        (363, 10) => (4.0, 155, 45.5, 4.0, 200.0, 10.0, 1.0),
        (363, 12) => (5.0, 157, 88.0, 4.5, 200.0, 10.0, 1.0),
        // foreshadow
        (364, 12) => (1.0, 158, 1350.0, 0.0, 500.0, 200.0, 5.0),
        // spectre
        (365, 3) => (4.0, 159, 50.0, 7.5, 260.0, 7.0, 1.0),
        (365, 7) => (2.0, 160, 80.0, 8.0, 260.0, 7.0, 1.0),
        (365, 15) => (3.0, 161, 90.0, 7.0, 260.0, 7.0, 1.0),
        _ => return None,
    };
    Some(TurretAmmo {
        multiplier,
        bullet_id,
        damage,
        speed,
        range,
        reload,
        ammo_per_shot,
    })
}

pub(crate) fn is_supported_item_turret(block: i16) -> bool {
    matches!(
        block,
        349 | 350 | 351 | 352 | 357 | 358 | 361 | 362 | 363 | 364 | 365
    )
}

pub(crate) fn turret_max_ammo(block: i16) -> f32 {
    if block == 364 {
        40.0
    } else {
        30.0
    }
}

pub(crate) fn power_turret_weapon(block: i16) -> Option<TurretAmmo> {
    let (bullet_id, damage, range, reload) = match block {
        354 => (129, 140.0, 165.0, 80.0),
        355 => (130, 20.0, 90.0, 35.0),
        366 => (162, 78.0, 195.0, 90.0),
        _ => return None,
    };
    Some(TurretAmmo {
        multiplier: 0.0,
        bullet_id,
        damage,
        speed: 0.0,
        range,
        reload,
        ammo_per_shot: 0.0,
    })
}

pub(crate) fn liquid_turret_weapon(block: i16, liquid: i16) -> Option<TurretAmmo> {
    let (bullet_id, damage, speed, range, reload, ammo_per_shot) = match (block, liquid) {
        (353, 0) => (125, 0.0, 3.5, 110.0, 3.0, 1.0),
        (353, 1) => (126, 4.0, 3.5, 110.0, 3.0, 1.0),
        (353, 3) => (127, 0.0, 3.5, 110.0, 3.0, 1.0),
        (353, 2) => (128, 0.0, 3.5, 110.0, 3.0, 1.0),
        // LiquidTurret consumes once per volley: 1 / ammoMultiplier(.4) = 2.5.
        (360, 0) => (140, 0.4, 4.0, 190.0, 3.0, 2.5),
        (360, 1) => (141, 9.5, 4.0, 190.0, 3.0, 2.5),
        (360, 3) => (142, 0.4, 4.0, 190.0, 3.0, 2.5),
        (360, 2) => (143, 0.4, 4.0, 190.0, 3.0, 2.5),
        _ => return None,
    };
    Some(TurretAmmo {
        multiplier: 0.0,
        bullet_id,
        damage,
        speed,
        range,
        reload,
        ammo_per_shot,
    })
}

pub(crate) fn turret_shots(block: i16) -> u8 {
    match block {
        350 => 2,
        360 => 2,
        357 | 358 | 362 => 4,
        361 => 3,
        _ => 1,
    }
}

/// Official v158.1 `UnitType.flying` registry. Keep this shared by both
/// Serpulo and Erekir turret paths; `hovering` ground units (notably elude)
/// deliberately do not count as air targets.
pub(crate) fn unit_type_is_flying(unit_type: i16) -> bool {
    matches!(
        unit_type,
        15..=24 | 35..=37 | 46 | 50..=55 | 58..=60 | 62..=67
    )
}

pub(crate) fn turret_target_allowed(unit_type: i16, air_only: bool, ground_only: bool) -> bool {
    let is_air = unit_type_is_flying(unit_type);
    (!air_only || is_air) && (!ground_only || !is_air)
}

pub(crate) fn turret_can_target(block: i16, unit_type: i16) -> bool {
    match block {
        350 => turret_target_allowed(unit_type, true, false),
        351 | 352 | 354 | 355 | 362 => turret_target_allowed(unit_type, false, true),
        _ => true,
    }
}

pub(crate) fn dominant_drill_ore(
    world: &DynamicWorld,
    drill: &DynamicTile,
    tier: u8,
) -> Option<(i16, u8, usize)> {
    let mut counts = std::collections::BTreeMap::<(i16, u8), usize>::new();
    for position in &drill.occupied {
        if let Some((item, hardness)) = raw_mine_result(world, *position) {
            if hardness <= tier {
                *counts.entry((item, hardness)).or_default() += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by_key(|((item, _), count)| (*count, std::cmp::Reverse(*item)))
        .map(|((item, hardness), count)| (item, hardness, count))
}

pub(crate) fn dump_drill_item(world: &DynamicWorld, key: i32) -> bool {
    let Some(drill) = world.tiles.get(&key).map(|tile| tile.clone()) else {
        return false;
    };
    if drill.stored_amount <= 0 {
        return false;
    }
    let mut targets = Vec::new();
    for position in &drill.occupied {
        for rotation in 0..4 {
            let target = offset_position(*position, rotation);
            if !drill.occupied.contains(&target) && !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    if !targets
        .into_iter()
        .any(|target| accept_logistics_item_from(world, target, drill.stored_item, Some(key), 0))
    {
        return false;
    }
    if let Some(mut drill) = world.tiles.get_mut(&key) {
        drill.stored_amount -= 1;
        if drill.stored_amount == 0 {
            drill.stored_item = -1;
        }
    }
    true
}

pub(crate) fn accept_logistics_item(world: &DynamicWorld, position: i32, item: i16) -> bool {
    accept_logistics_item_from(world, position, item, None, 0)
}

pub(crate) fn accept_logistics_item_from(
    world: &DynamicWorld,
    position: i32,
    item: i16,
    source: Option<i32>,
    depth: u8,
) -> bool {
    if depth >= 8 {
        return false;
    }
    if matches!(base_block(world, position), 339..=344) {
        // Deposit into the CORE'S OWN team inventory (official
        // `CoreBuild.items` of that team). Cores are base buildings, so the
        // team comes from the registered per-team core map (fallback: 1).
        let team = crate::network::world::core_team_at_position(world, position).unwrap_or(1);
        return crate::network::core_inventory::deposit_core_items(world, team, item, 1) == 1;
    }
    let Some(target_key) = dynamic_at(world, position).map(|tile| tile.position) else {
        return false;
    };
    let Some(snapshot) = world.tiles.get(&target_key).map(|tile| tile.clone()) else {
        return false;
    };
    // ItemVoid.handleItem discards synchronously; its fake flow module is not
    // persistent building inventory.
    if snapshot.block == 413 {
        return snapshot.enabled;
    }
    // Incinerator.acceptItem (Incinerator.java:66-69): burns ANY item while
    // enabled. The official heat > 0.5f warmup gate is approximated as
    // immediately hot once powered (the burn is synchronous like ItemVoid).
    if snapshot.block == 198 {
        return snapshot.enabled;
    }
    // Stack conveyors (plastanium 259, surge 279) take items through the
    // official StackConveyorBuild.acceptItem gates (state/cooldown/capacity),
    // not the generic conveyor queue (round 74: a normal conveyor feeding a
    // plastanium belt bypassed the machine and corrupted its client state).
    if matches!(snapshot.block, 259 | 279) {
        let source_position = source.unwrap_or(position);
        return crate::network::economy::duct_accept_item(world, position, item, source_position)
            && {
                crate::network::economy::duct_store_item(world, position, item);
                true
            };
    }
    if snapshot.block == 408 {
        if inventory_total(&snapshot.inventory) >= 100 {
            return false;
        }
        if let Some(mut loader) = world.tiles.get_mut(&target_key) {
            inventory_add(&mut loader.inventory, item, 1);
            return true;
        }
        return false;
    }
    if let Some(capacity) = storage_capacity(snapshot.block) {
        if storage_linked_to_core(world, &snapshot) {
            // A core-linked vault routes into ITS OWN team's core inventory.
            return crate::network::core_inventory::deposit_core_items(
                world,
                snapshot.team,
                item,
                1,
            ) == 1;
        }
        if inventory_count(&snapshot.inventory, item) >= capacity {
            return false;
        }
        if let Some(mut storage) = world.tiles.get_mut(&target_key) {
            inventory_add(&mut storage.inventory, item, 1);
            return true;
        }
        return false;
    }
    if snapshot.block == 271 {
        if valid_mass_driver_link(world, &snapshot).is_none()
            || inventory_total(&snapshot.inventory) >= 120
        {
            return false;
        }
        if let Some(mut driver) = world.tiles.get_mut(&target_key) {
            inventory_add(&mut driver.inventory, item, 1);
            return true;
        }
        return false;
    }
    if snapshot.block == 261 {
        let Some(source) = source else {
            return false;
        };
        let Some(direction) =
            (0..4u8).find(|rotation| offset_position(source, *rotation) == snapshot.position)
        else {
            return false;
        };
        let output = offset_position(snapshot.position, direction);
        if dynamic_at(world, output).is_none_or(|tile| tile.team != snapshot.team)
            || snapshot
                .junction_items
                .iter()
                .filter(|(stored_direction, _, _)| *stored_direction == direction)
                .count()
                >= 6
        {
            return false;
        }
        if let Some(mut junction) = world.tiles.get_mut(&target_key) {
            junction.junction_items.push((direction, item, 26.0));
            return true;
        }
        return false;
    }
    if matches!(snapshot.block, 264 | 265 | 268 | 269) {
        let Some(source) = source else {
            return false;
        };
        return route_instant_item(world, &snapshot, source, item, depth + 1);
    }
    if is_plain_conveyor(snapshot.block) {
        return accept_plain_conveyor_item(world, target_key, item, source);
    }
    let Some(mut target) = world.tiles.get_mut(&target_key) else {
        return false;
    };
    if let Some(capacity) = item_transport_capacity(target.block) {
        // Conveyors and bridges queue items with individual positions (the
        // official ConveyorBuild keeps ids/xs/ys per item); routers keep the
        // legacy single-slot fields.
        if matches!(target.block, 266 | 267) {
            if target.stored_amount >= capacity
                || (target.stored_amount > 0 && target.stored_item != item)
            {
                return false;
            }
            target.stored_item = item;
            target.stored_amount += 1;
            return true;
        }
        let legacy_amount = if target.conveyor_items.is_empty() {
            target.stored_amount.max(0)
        } else {
            0
        };
        let legacy_item = target.stored_item;
        let legacy_progress = target.transport_progress;
        let front_item = target
            .conveyor_items
            .first()
            .map(|(stored, _)| *stored)
            .unwrap_or(legacy_item);
        if target.conveyor_items.len() + legacy_amount as usize >= capacity as usize
            || (legacy_amount > 0 && legacy_item != item)
            || (!target.conveyor_items.is_empty() && front_item != item)
        {
            return false;
        }
        if legacy_amount > 0 {
            // Migrate the legacy shared slot into the per-item list.
            target.conveyor_items.push((legacy_item, legacy_progress));
        }
        target.conveyor_items.push((item, 0.0));
        target.stored_item = item;
        target.stored_amount = i32::try_from(target.conveyor_items.len()).unwrap_or(i32::MAX);
        return true;
    }
    if let Some(ammo) = turret_ammo(target.block, item) {
        if (target.ammo_units > 0.0 && target.stored_item != item)
            || target.ammo_units + ammo.multiplier > turret_max_ammo(target.block)
        {
            return false;
        }
        target.stored_item = item;
        target.stored_amount += 1;
        target.ammo_units += ammo.multiplier;
        return true;
    }
    if let Some(spec) = mender_spec(target.block) {
        if item != spec.booster_item || inventory_total(&target.inventory) >= 10 {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if let Some(spec) = overdrive_spec(target.block) {
        if !spec
            .required_items
            .iter()
            .any(|(accepted, _)| *accepted == item)
            || inventory_total(&target.inventory) >= 10
        {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if target.block == FORCE_PROJECTOR_BLOCK {
        if item != 11 || inventory_total(&target.inventory) >= 10 {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if target.block == REGEN_PROJECTOR_BLOCK {
        if item != 11 || inventory_total(&target.inventory) >= 10 {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if target.block == 308 {
        if generator_fuel(item).is_none() || inventory_total(&target.inventory) >= 10 {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if target.block == 315 {
        if item != reactor::THORIUM_ITEM
            || inventory_count(&target.inventory, item) >= reactor::ITEM_CAPACITY
        {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if target.block == 316 {
        if item != 14 || inventory_total(&target.inventory) >= 10 {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if let Some(plan) = unit_factory_recipe(target.block, &target.config) {
        let cost_multiplier = world.wave_rules.read().unit_cost_multiplier.max(0.0)
            * world
                .wave_rules
                .read()
                .team_rule(target.team)
                .unit_cost_multiplier;
        let capacity = (unit_factory_item_capacity(target.block, item) as f32 * cost_multiplier)
            .round() as i32;
        if capacity == 0
            || !plan
                .requirements
                .iter()
                .any(|(accepted, _)| *accepted == item)
            || inventory_count(&target.inventory, item) >= capacity
        {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if let Some(recipe) = reconstructor_recipe(target.block) {
        let cost_multiplier = world.wave_rules.read().unit_cost_multiplier.max(0.0)
            * world
                .wave_rules
                .read()
                .team_rule(target.team)
                .unit_cost_multiplier;
        let capacity = (reconstructor_item_capacity(target.block, item) as f32
            * cost_multiplier.max(0.0))
        .round() as i32;
        if capacity == 0
            || !recipe.items.iter().any(|(accepted, _)| *accepted == item)
            || inventory_count(&target.inventory, item) >= capacity
        {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if matches!(target.block, 406 | 407) {
        let capacity = constructor_item_capacity(target.block, &target.config, item);
        if capacity == 0 || inventory_count(&target.inventory, item) >= capacity {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    if let Some(recipe) = factory_recipe(target.block) {
        if !recipe.inputs.iter().any(|(accepted, _)| *accepted == item)
            || inventory_total(&target.inventory) >= recipe.capacity
        {
            return false;
        }
        inventory_add(&mut target.inventory, item, 1);
        return true;
    }
    let Some(recipe) = liquid_factory_recipe(target.block) else {
        return false;
    };
    if !recipe
        .item_inputs
        .iter()
        .any(|(accepted, _)| *accepted == item)
        || inventory_total(&target.inventory) >= recipe.item_capacity
    {
        return false;
    }
    inventory_add(&mut target.inventory, item, 1);
    true
}

pub(crate) fn simulate_junctions(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == 261 && !tile.junction_items.is_empty())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        if let Some(mut junction) = world.tiles.get_mut(&key) {
            for (_, _, remaining) in &mut junction.junction_items {
                *remaining = (*remaining - delta_ticks).max(0.0);
            }
            changed = true;
        }
        // Java Junction.updateTile walks the four directional buffers
        // independently (Junction.java:55-76): a jammed output must not
        // head-of-line block items whose own output direction is free.
        loop {
            let snapshot = world
                .tiles
                .get(&key)
                .map(|junction| junction.junction_items.clone())
                .unwrap_or_default();
            let mut delivered = false;
            for direction in 0..4u8 {
                let Some((index, (_, item, _))) = snapshot
                    .iter()
                    .enumerate()
                    .find(|(_, (dir, _, remaining))| *dir == direction && *remaining <= 0.0)
                else {
                    continue;
                };
                let item = *item;
                let output = offset_position(key, direction);
                if !accept_logistics_item_from(world, output, item, Some(key), 0) {
                    // This direction's head is blocked; keep serving the
                    // remaining directions.
                    continue;
                }
                if let Some(mut junction) = world.tiles.get_mut(&key) {
                    if index < junction.junction_items.len()
                        && junction.junction_items[index].0 == direction
                        && junction.junction_items[index].1 == item
                    {
                        junction.junction_items.remove(index);
                        delivered = true;
                        changed = true;
                    }
                }
                break;
            }
            if !delivered {
                break;
            }
        }
    }
    changed
}

pub(crate) fn storage_capacity(block: i16) -> Option<i32> {
    match block {
        345 => Some(300),
        346 => Some(1_000),
        347 => Some(160),
        348 => Some(900),
        _ => None,
    }
}

pub(crate) fn storage_linked_to_core(world: &DynamicWorld, storage: &DynamicTile) -> bool {
    storage.occupied.iter().any(|occupied| {
        (0..4).any(|rotation| {
            matches!(
                base_block(world, offset_position(*occupied, rotation)),
                339..=344
            )
        })
    })
}

pub(crate) fn simulate_unloaders(world: &DynamicWorld, delta_ticks: f32) -> bool {
    const UNLOAD_TIME: f32 = 60.0 / 11.0;
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == 270)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let ready = if let Some(mut unloader) = world.tiles.get_mut(&key) {
            unloader.transport_progress += delta_ticks;
            if unloader.transport_progress >= UNLOAD_TIME {
                unloader.transport_progress %= UNLOAD_TIME;
                true
            } else {
                false
            }
        } else {
            false
        };
        if !ready {
            continue;
        }
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let neighbors: Vec<_> = (0..4)
            .map(|rotation| offset_position(key, rotation))
            .collect();
        let selected = configured_item(&snapshot.config);
        let start = selected.unwrap_or_else(|| (snapshot.stored_item + 1).rem_euclid(22));
        let candidates: Vec<_> = if selected.is_some() {
            vec![start]
        } else {
            (0..22)
                .map(|offset| (start + offset).rem_euclid(22))
                .collect()
        };
        let mut transferred = false;
        for item in candidates {
            for provider_position in &neighbors {
                let provider = dynamic_at(world, *provider_position);
                let provider_is_core = matches!(base_block(world, *provider_position), 339..=344);
                let available = if provider_is_core {
                    // The unloader pulls from ITS OWN team's core inventory.
                    items_for_team(world, snapshot.team)
                        .get(item as usize)
                        .is_some_and(|amount| *amount > 0)
                } else {
                    provider.as_ref().is_some_and(|tile| {
                        inventory_count(&tile.inventory, item) > 0
                            || (tile.stored_item == item && tile.stored_amount > 0)
                    })
                };
                if !available {
                    continue;
                }
                for receiver in &neighbors {
                    if receiver == provider_position
                        || (provider_is_core
                            && dynamic_at(world, *receiver)
                                .is_some_and(|tile| storage_linked_to_core(world, &tile)))
                    {
                        continue;
                    }
                    if !accept_logistics_item_from(world, *receiver, item, Some(key), 0) {
                        continue;
                    }
                    if provider_is_core {
                        if let Some(amount) =
                            items_for_team_mut(world, snapshot.team).get_mut(item as usize)
                        {
                            *amount -= 1;
                        }
                    } else if let Some(provider) = provider {
                        if let Some(mut tile) = world.tiles.get_mut(&provider.position) {
                            if !inventory_remove(&mut tile.inventory, item, 1)
                                && tile.stored_item == item
                                && tile.stored_amount > 0
                            {
                                tile.stored_amount -= 1;
                                if tile.stored_amount == 0 {
                                    tile.stored_item = -1;
                                }
                            }
                        }
                    }
                    if selected.is_none() {
                        if let Some(mut unloader) = world.tiles.get_mut(&key) {
                            unloader.stored_item = item;
                        }
                    }
                    transferred = true;
                    changed = true;
                    break;
                }
                if transferred {
                    break;
                }
            }
            if transferred {
                break;
            }
        }
        if !transferred {
            if let Some(mut unloader) = world.tiles.get_mut(&key) {
                unloader.transport_progress = UNLOAD_TIME;
            }
        }
    }
    changed
}

pub(crate) fn valid_mass_driver_link(world: &DynamicWorld, driver: &DynamicTile) -> Option<i32> {
    let target = configured_link(driver, world.width, world.height)?;
    let other = dynamic_at(world, target)?;
    let x = (driver.position >> 16) as i16 as f32;
    let y = driver.position as i16 as f32;
    let tx = (other.position >> 16) as i16 as f32;
    let ty = other.position as i16 as f32;
    (other.position != driver.position
        && other.block == 271
        && other.team == driver.team
        && (tx - x).hypot(ty - y) <= 55.0)
        .then_some(other.position)
}

pub(crate) fn mass_driver_state(world: &DynamicWorld, driver: &DynamicTile) -> u8 {
    // Official DriverState transitions (MassDriverBuild.updateTile, 158.1):
    // accepting (1) while the driver expects incoming items and has at least
    // minDistribute capacity; shooting (2) ONLY while a shot is firing
    // (rotation sweep + bolt flight); idle (0) otherwise. The legacy port
    // returned 2 whenever a link existed, so the client hid the launcher
    // sprite and never showed the idle/accepting animations.
    if !driver.mass_driver_waiting.is_empty() && 120 - inventory_total(&driver.inventory) >= 10 {
        1
    } else if valid_mass_driver_link(world, driver).is_some()
        && driver.production_progress > 100.0
        && !driver.inventory.is_empty()
    {
        // Recent shot: the cooldown (200 ticks) is still in its first half.
        2
    } else {
        0
    }
}

pub(crate) fn move_toward_angle(current: f32, target: f32, maximum: f32) -> f32 {
    let delta = (target - current + 180.0).rem_euclid(360.0) - 180.0;
    (current + delta.clamp(-maximum, maximum)).rem_euclid(360.0)
}

pub(crate) fn angle_near(current: f32, target: f32, tolerance: f32) -> bool {
    ((target - current + 180.0).rem_euclid(360.0) - 180.0).abs() <= tolerance
}

pub(crate) fn route_instant_item(
    world: &DynamicWorld,
    block: &DynamicTile,
    source: i32,
    item: i16,
    depth: u8,
) -> bool {
    let Some(forward) =
        (0..4u8).find(|rotation| offset_position(source, *rotation) == block.position)
    else {
        return false;
    };
    // instantTransfer family (Sorter.isSame): sorters 264/265 and overflow
    // gates 268/269 reject chained transfers through each other.
    let is_instant = |position: i32| -> bool {
        world
            .tiles
            .get(&position)
            .map(|tile| matches!(tile.block, 264 | 265 | 268 | 269))
            .unwrap_or(false)
    };
    let source_instant = is_instant(source);
    let straight = offset_position(block.position, forward);
    // Sorter.java getTileTarget probes a = nearby(mod(dir - 1, 4)) and
    // b = nearby(mod(dir + 1, 4)); the gate labels swap because its `from`
    // points back at the source ((from + 2) % 4 is straight).
    let side_ccw = offset_position(block.position, (forward + 3) % 4);
    let side_cw = offset_position(block.position, (forward + 1) % 4);
    let accept_target =
        |target: i32| accept_logistics_item_from(world, target, item, Some(block.position), depth);

    // Ordered candidate list; the first accepting probe delivers (each
    // accept attempt IS the deposit), mirroring the official
    // sole-side / both-side tie-break outcomes.
    let mut candidates: Vec<i32> = Vec::with_capacity(3);
    // Side-alternation flip bit: sorters use `dir`, gates use
    // `from = dir + 2` (Sorter.java:134 / OverflowGate.java:77).
    let mut flip_mask = 0u8;
    match block.block {
        264 | 265 => {
            let invert = block.block == 265;
            let selected = configured_item(&block.config);
            // ((item == sortItem) != invert) == enabled (Sorter.java:116):
            // a disabled sorter never routes straight.
            let goes_straight = ((selected == Some(item)) != invert) == block.enabled;
            if goes_straight {
                // Prevent 3-chains: source and straight target both
                // instantTransfer blocks (Sorter.java:119-121).
                if !(source_instant && is_instant(straight)) {
                    candidates.push(straight);
                }
            } else {
                flip_mask = 1u8 << forward;
                if block.rotation & flip_mask == 0 {
                    candidates.push(side_ccw);
                    candidates.push(side_cw);
                } else {
                    candidates.push(side_cw);
                    candidates.push(side_ccw);
                }
            }
        }
        268 | 269 => {
            let invert = block.block == 269;
            // inv = invert == enabled (OverflowGate.java:60): a normal
            // powered gate runs forward-first and overflows to the sides
            // only when straight refuses; inverted swaps the priority.
            let inv = invert == block.enabled;
            let straight_conflict = source_instant && is_instant(straight);
            let gate_mask = 1u8 << ((forward + 2) % 4);
            flip_mask = gate_mask;
            let sides_first = inv || straight_conflict;
            let bit_clear = block.rotation & gate_mask == 0;
            let (first_side, second_side) = if bit_clear {
                (side_cw, side_ccw)
            } else {
                (side_ccw, side_cw)
            };
            if sides_first {
                candidates.push(first_side);
                candidates.push(second_side);
                if !straight_conflict {
                    candidates.push(straight);
                }
            } else {
                candidates.push(straight);
                candidates.push(first_side);
                candidates.push(second_side);
            }
        }
        _ => return false,
    }
    for candidate in candidates {
        if candidate == source {
            continue;
        }
        if accept_target(candidate) {
            if flip_mask != 0 {
                if let Some(mut tile) = world.tiles.get_mut(&block.position) {
                    tile.rotation ^= flip_mask;
                }
            }
            return true;
        }
    }
    false
}

pub(crate) fn configured_item(config: &[u8]) -> Option<i16> {
    (config.len() == 4 && config[0] == 5 && config[1] == 0)
        .then(|| i16::from_be_bytes([config[2], config[3]]))
        .filter(|item| (0..22).contains(item))
}

pub(crate) fn generator_fuel(item: i16) -> Option<(f32, f32)> {
    match item {
        5 => Some((1.0, 1.0)),
        13 => Some((1.15, 1.0)),
        15 => Some((1.4, 3.0)),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FactoryRecipe {
    pub(crate) inputs: &'static [(i16, i32)],
    pub(crate) output: (i16, i32),
    pub(crate) craft_time: f32,
    pub(crate) capacity: i32,
}

/// Backwards-compatible test/domain entrypoint. Runtime callers use
/// `simulate_reactors_with_network` so overheat events can emit destruction
/// and radial-damage packets.
pub(crate) fn simulate_reactors(world: &DynamicWorld, delta_ticks: f32) -> bool {
    simulate_reactors_with_network(world, &DashMap::new(), delta_ticks)
}

pub(crate) fn factory_recipe(block: i16) -> Option<FactoryRecipe> {
    let recipe = match block {
        181 => FactoryRecipe {
            inputs: &[(5, 2)],
            output: (3, 1),
            craft_time: 90.0,
            capacity: 10,
        },
        183 => FactoryRecipe {
            inputs: &[(5, 1), (4, 2)],
            output: (9, 1),
            craft_time: 40.0,
            capacity: 10,
        },
        184 => FactoryRecipe {
            inputs: &[(5, 4), (4, 6), (15, 1)],
            output: (9, 8),
            craft_time: 90.0,
            capacity: 30,
        },
        185 => FactoryRecipe {
            inputs: &[(1, 1), (4, 1)],
            output: (2, 1),
            craft_time: 30.0,
            capacity: 10,
        },
        187 => FactoryRecipe {
            inputs: &[(7, 4), (4, 10)],
            output: (11, 1),
            craft_time: 120.0,
            capacity: 30,
        },
        188 => FactoryRecipe {
            inputs: &[(0, 3), (1, 4), (6, 2), (9, 3)],
            output: (12, 1),
            craft_time: 75.0,
            capacity: 20,
        },
        190 => FactoryRecipe {
            inputs: &[(5, 1), (1, 2), (4, 2)],
            output: (15, 1),
            craft_time: 80.0,
            capacity: 10,
        },
        191 => FactoryRecipe {
            inputs: &[(15, 1), (13, 1)],
            output: (14, 1),
            craft_time: 80.0,
            capacity: 10,
        },
        196 => FactoryRecipe {
            inputs: &[(8, 1)],
            output: (4, 1),
            craft_time: 40.0,
            capacity: 10,
        },
        _ => return None,
    };
    Some(recipe)
}

pub(crate) fn inventory_total(inventory: &[(i16, i32)]) -> i32 {
    inventory.iter().map(|(_, amount)| *amount).sum()
}

pub(crate) fn inventory_count(inventory: &[(i16, i32)], item: i16) -> i32 {
    inventory
        .iter()
        .find_map(|(stored, amount)| (*stored == item).then_some(*amount))
        .unwrap_or(0)
}

pub(crate) fn inventory_add(inventory: &mut Vec<(i16, i32)>, item: i16, amount: i32) {
    if let Some((_, stored)) = inventory.iter_mut().find(|(stored, _)| *stored == item) {
        *stored += amount;
    } else if amount > 0 {
        inventory.push((item, amount));
    }
}

pub(crate) fn inventory_remove(inventory: &mut Vec<(i16, i32)>, item: i16, amount: i32) -> bool {
    let Some(index) = inventory.iter().position(|(stored, _)| *stored == item) else {
        return false;
    };
    if inventory[index].1 < amount {
        return false;
    }
    inventory[index].1 -= amount;
    if inventory[index].1 == 0 {
        inventory.remove(index);
    }
    true
}

pub(crate) fn configured_link(tile: &DynamicTile, width: i32, height: i32) -> Option<i32> {
    match tile.config.as_slice() {
        [7, rest @ ..] if rest.len() >= 8 => {
            let dx = i32::from_be_bytes(rest[0..4].try_into().ok()?);
            let dy = i32::from_be_bytes(rest[4..8].try_into().ok()?);
            let x = (tile.position >> 16) as i16 as i32 + dx;
            let y = tile.position as i16 as i32 + dy;
            ((0..width).contains(&x) && (0..height).contains(&y))
                .then_some((x << 16) | (y as u16 as i32))
        }
        [1, rest @ ..] if rest.len() >= 4 => Some(i32::from_be_bytes(rest[0..4].try_into().ok()?)),
        _ => None,
    }
}

pub(crate) fn valid_bridge_link(
    world: &DynamicWorld,
    tile: &DynamicTile,
    range: i32,
) -> Option<i32> {
    if tile.block == 298 {
        // DirectionBridgeBuild.findLink (DirectionBridge.java:252-260): the
        // reinforced bridge has NO config - it auto-links to the FIRST same
        // block/team tile along its rotation, distance 1..=range.
        for i in 1..=range {
            // Sign-safe step: positions pack x/y as i16 halves.
            let dx = [1i32, -1, 0, 0][usize::from(tile.rotation)];
            let dy = [0i32, 0, 1, -1][usize::from(tile.rotation)];
            let px = ((tile.position >> 16) as i16 as i32) + dx * i;
            let py = (tile.position as i16 as i32) + dy * i;
            let target = (px << 16) | (py as u16 as i32);
            if let Some(other) = dynamic_at(world, target) {
                if other.block == tile.block && other.team == tile.team {
                    return Some(target);
                }
            }
        }
        return None;
    }
    let target = configured_link(tile, world.width, world.height)?;
    let tx = (target >> 16) as i16 as i32;
    let ty = target as i16 as i32;
    let x = (tile.position >> 16) as i16 as i32;
    let y = tile.position as i16 as i32;
    let distance = (tx - x).abs() + (ty - y).abs();
    if distance <= 1
        || distance > range
        || (tx != x && ty != y)
        || dynamic_at(world, target)
            .is_none_or(|other| other.block != tile.block || other.team != tile.team)
    {
        return None;
    }
    Some(target)
}

pub(crate) fn incoming_bridge_sources(
    world: &DynamicWorld,
    receiver: &DynamicTile,
) -> HashSet<i32> {
    let range = if receiver.block == 262 { 4 } else { 12 };
    world
        .tiles
        .iter()
        .filter(|source| {
            source.block == receiver.block
                && source.team == receiver.team
                && valid_bridge_link(world, source, range) == Some(receiver.position)
        })
        .map(|source| source.position)
        .collect()
}

pub(crate) fn offset_position(position: i32, rotation: u8) -> i32 {
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    let (dx, dy) = match rotation % 4 {
        0 => (1, 0),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (0, -1),
    };
    ((x + dx) << 16) | ((y + dy) as u16 as i32)
}

#[cfg(test)]
mod inventory_gate {
    #[test]
    fn turret_ammo_bullet_ids_are_inventoried() {
        use super::{liquid_turret_weapon, power_turret_weapon, turret_ammo};
        use crate::game::bullet_catalog::bullet_is_inventoried;
        for block in 349..=376 {
            for item in 0..22 {
                if let Some(ammo) = turret_ammo(block, item) {
                    assert!(
                        bullet_is_inventoried(ammo.bullet_id),
                        "turret {block} item {item} bullet {} missing from inventory (C2)",
                        ammo.bullet_id
                    );
                }
            }
            if let Some(ammo) = power_turret_weapon(block) {
                assert!(
                    bullet_is_inventoried(ammo.bullet_id),
                    "power turret {block} bullet {} missing from inventory (C2)",
                    ammo.bullet_id
                );
            }
            for liquid in 0..4 {
                if let Some(ammo) = liquid_turret_weapon(block, liquid) {
                    assert!(
                        bullet_is_inventoried(ammo.bullet_id),
                        "liquid turret {block} liquid {liquid} bullet {} missing (C2)",
                        ammo.bullet_id
                    );
                }
            }
        }
    }
}
