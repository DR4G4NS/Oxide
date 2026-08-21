//! Payload carrier simulation: conveyors, mass drivers, loaders, constructors,
//! deconstructors, carriers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::construction::dynamic_at;
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::*;
use crate::network::buildings::snapshot::{
    angle_between, build_payload_version, encode_payload_build_sync, is_pickup_payload_supported,
};
use crate::network::combat::enemy::base_building_tombstone;
use crate::network::combat::unit_combat::{effective_unit_speed, unit_hit_size};
use crate::network::combat::*;
use crate::network::economy::payload::{
    carried_payload_requirements, choose_payload_router_rotation, decode_constructor_recipe,
    drop_carried_build, dump_deconstructor_items, encode_payload_dropped_frame,
    encode_picked_build_payload_frame, encode_picked_unit_payload_frame,
    insert_into_payload_conveyor, offset_position_by, payload_block_accepts, payload_block_limit,
    payload_capacity, payload_conveyor_move_time, payload_fits_limit, payload_used,
    refresh_build_payload_sync, transfer_payload_forward, valid_payload_mass_driver_link,
};
use crate::network::economy::spec::{
    accept_logistics_item_from, inventory_add, inventory_count, inventory_remove, inventory_total,
    offset_position, storage_capacity,
};
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::controller::unit_is_player_controlled;
use crate::network::units::unit_orders::advance_unit_order;
use crate::network::units::*;
use crate::network::wire::encode::{encode_unit_spawn_payload, frame_generated_packet};
use crate::network::wire::tile_config::broadcast_placement_power_configs;
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;
use tracing::{debug, error, info, warn};

use super::*;

pub fn simulate_payload_conveyors(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| payload_conveyor_move_time(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(move_time) = payload_conveyor_move_time(snapshot.block) else {
            continue;
        };
        if snapshot.payload.is_none() {
            continue;
        }
        let ready = snapshot.payload_progress + delta_ticks.max(0.0) >= move_time;
        if !ready {
            if let Some(mut live) = world.tiles.get_mut(&key) {
                live.payload_progress += delta_ticks.max(0.0);
            }
            changed = true;
            continue;
        }
        let next_position = offset_position_by(snapshot.position, snapshot.rotation, 3);
        let next = dynamic_at(world, next_position);
        let can_transfer = next.as_ref().is_some_and(|next| {
            payload_block_limit(next.block).is_some()
                && next.team == snapshot.team
                && next.payload.is_none()
                && snapshot.payload.as_deref().is_some_and(|payload| {
                    payload_block_limit(next.block)
                        .is_some_and(|limit| payload_fits_limit(payload, limit))
                        && payload_block_accepts(next.block, payload)
                })
        });
        if can_transfer {
            let payload = world
                .tiles
                .get_mut(&key)
                .and_then(|mut tile| tile.payload.take());
            if let (Some(payload), Some(next)) = (payload, next) {
                let mut routed = next.clone();
                routed.stored_amount = i32::from((snapshot.rotation + 2) % 4) + 1;
                let target_rotation = matches!(routed.block, 399 | 401)
                    .then(|| choose_payload_router_rotation(world, &routed, &payload));
                if let Some(mut target) = world.tiles.get_mut(&next.position) {
                    if let Some(rotation) = target_rotation {
                        target.rotation = rotation;
                        target.stored_amount = i32::from((snapshot.rotation + 2) % 4) + 1;
                    }
                    target.payload = Some(payload);
                    target.payload_progress = 0.0;
                    target.payload_rotation = f32::from(snapshot.rotation) * 90.0;
                }
                if let Some(mut source) = world.tiles.get_mut(&key) {
                    source.payload_progress = 0.0;
                }
                changed = true;
            }
            continue;
        }
        if next.is_none() {
            let Some(CarriedPayload::Unit(mut unit)) = snapshot.payload.as_deref().cloned() else {
                continue;
            };
            unit.id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
            let x = (next_position >> 16) as i16 as f32 * 8.0;
            let y = next_position as i16 as f32 * 8.0;
            unit.x = x;
            unit.y = y;
            unit.rotation = f32::from(snapshot.rotation) * 90.0;
            unit.velocity_x = 0.0;
            unit.velocity_y = 0.0;
            // P0-01: a payload-dropped unit starts with a fresh controller.
            unit.authority = crate::network::units::default_unit_authority(world, &unit);
            world.register_unit_group(unit.id);
            world.enemies.insert(unit.id, unit.clone());
            world.unit_orders.insert(
                unit.id,
                UnitOrder {
                    unit_id: unit.id,
                    command: default_unit_command(unit.unit_type),
                    stances: 0,
                    payload_cooldown: 0.0,
                    target_kind: 0,
                    target_id: -1,
                    target_x: None,
                    target_y: None,
                    logic_control: 0,
                    queue: Vec::new(),
                },
            );
            world.game_state.game_stats.write().units_created += 1;
            if let Ok(payload) = encode_unit_spawn_payload(world, &unit) {
                if let Ok(frame) = frame_generated_packet(UNIT_SPAWN_PACKET_ID, &payload, false) {
                    out.broadcast(frame);
                }
            }
            if let Some(mut source) = world.tiles.get_mut(&key) {
                source.payload = None;
                source.payload_progress = 0.0;
            }
            changed = true;
        }
    }
    changed
}

pub fn simulate_payload_mass_drivers(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 402 | 403))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(source) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if source.payload.is_none() || power.get(&key).copied().unwrap_or(0.0) <= 0.0 {
            continue;
        }
        let Some(target_key) = valid_payload_mass_driver_link(world, &source) else {
            continue;
        };
        let Some(target) = world.tiles.get(&target_key).map(|tile| tile.clone()) else {
            continue;
        };
        if target.payload.is_some() || power.get(&target_key).copied().unwrap_or(0.0) <= 0.0 {
            continue;
        }
        let charge_time = if source.block == 402 { 220.0 } else { 230.0 };
        if source.transport_progress + delta_ticks.max(0.0) < charge_time {
            if let Some(mut live) = world.tiles.get_mut(&key) {
                live.transport_progress += delta_ticks.max(0.0);
                live.payload_rotation = angle_between(source.position, target.position);
            }
            changed = true;
            continue;
        }
        let payload = world
            .tiles
            .get_mut(&key)
            .and_then(|mut tile| tile.payload.take());
        if let Some(payload) = payload {
            if let Some(mut receiver) = world.tiles.get_mut(&target_key) {
                receiver.payload = Some(payload);
                receiver.payload_progress = 0.0;
                receiver.payload_rotation = angle_between(source.position, target.position);
                receiver.transport_progress = 0.0;
            }
            if let Some(mut sender) = world.tiles.get_mut(&key) {
                sender.transport_progress = 0.0;
            }
            changed = true;
        }
    }
    changed
}

pub fn simulate_payload_loaders(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 408 | 409))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(initial) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if initial.block == 409 {
            if let Some(mut live) = world.tiles.get_mut(&key) {
                live.output_liquid_amount = 0.0; // dynamic power production for this tick
            }
            for _ in 0..4 {
                let Some((item, _)) = world
                    .tiles
                    .get(&key)
                    .and_then(|tile| tile.inventory.first().copied())
                else {
                    break;
                };
                let mut outputs = Vec::new();
                for occupied in &initial.occupied {
                    for rotation in 0..4 {
                        let target = offset_position(*occupied, rotation);
                        if !initial.occupied.contains(&target) && !outputs.contains(&target) {
                            outputs.push(target);
                        }
                    }
                }
                if !outputs
                    .into_iter()
                    .any(|target| accept_logistics_item_from(world, target, item, Some(key), 0))
                {
                    break;
                }
                if let Some(mut live) = world.tiles.get_mut(&key) {
                    inventory_remove(&mut live.inventory, item, 1);
                }
                changed = true;
            }
        }
        let Some(mut snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if snapshot.payload.is_none() {
            continue;
        }
        if snapshot.production_progress >= 1.0 {
            changed |= transfer_payload_forward(world, &snapshot);
            continue;
        }
        if power.get(&key).copied().unwrap_or(0.0) <= 0.0 {
            continue;
        }
        snapshot.transport_progress += delta_ticks.max(0.0);
        let item_tick = snapshot.transport_progress >= 2.0;
        if item_tick {
            snapshot.transport_progress %= 2.0;
        }
        let Some(CarriedPayload::Build(build)) = snapshot.payload.as_deref_mut() else {
            snapshot.production_progress = 1.0;
            if let Some(mut live) = world.tiles.get_mut(&key) {
                live.production_progress = 1.0;
            }
            changed = true;
            continue;
        };
        let item_capacity = storage_capacity(build.tile.block);
        let inner_liquid_capacity = liquid_capacity(build.tile.block);
        let battery_capacity = power_role(build.tile.block)
            .map(|role| role.battery_capacity)
            .unwrap_or(0.0);
        let mut moved = 0;
        if snapshot.block == 408 {
            if item_tick {
                let attempted = item_capacity.is_some() && !snapshot.inventory.is_empty();
                while moved < 8 && !snapshot.inventory.is_empty() {
                    let Some(capacity) = item_capacity else {
                        break;
                    };
                    let (item, _) = snapshot.inventory[0];
                    if inventory_total(&build.tile.inventory) >= capacity {
                        break;
                    }
                    if !inventory_remove(&mut snapshot.inventory, item, 1) {
                        break;
                    }
                    inventory_add(&mut build.tile.inventory, item, 1);
                    moved += 1;
                }
                if attempted
                    && moved == 0
                    && item_capacity
                        .is_some_and(|capacity| inventory_total(&build.tile.inventory) >= capacity)
                {
                    snapshot.production_progress = 1.0;
                }
            }
            if snapshot.liquid_amount >= 0.001 {
                if let Some(capacity) = inner_liquid_capacity {
                    if build.tile.liquid_amount <= 0.2
                        || build.tile.stored_liquid == snapshot.stored_liquid
                    {
                        let flow = (40.0 * delta_ticks.max(0.0))
                            .min(snapshot.liquid_amount)
                            .min((capacity - build.tile.liquid_amount).max(0.0));
                        if flow > 0.0 {
                            build.tile.stored_liquid = snapshot.stored_liquid;
                            build.tile.liquid_amount += flow;
                            snapshot.liquid_amount -= flow;
                        } else if build.tile.liquid_amount >= capacity - 0.001 {
                            snapshot.production_progress = 1.0;
                        }
                    } else {
                        snapshot.production_progress = 1.0;
                    }
                }
            }
            if battery_capacity > 0.0 {
                let efficiency = power.get(&key).copied().unwrap_or(0.0);
                let available = (efficiency * 42.0 - 2.0).max(0.0) * delta_ticks.max(0.0);
                build.tile.power_stored =
                    (build.tile.power_stored + available).min(battery_capacity);
                if build.tile.power_stored >= battery_capacity - 0.001 {
                    snapshot.production_progress = 1.0;
                }
            }
        } else {
            if item_tick {
                while moved < 8
                    && inventory_total(&snapshot.inventory) < 100
                    && !build.tile.inventory.is_empty()
                {
                    let (item, _) = build.tile.inventory[0];
                    if !inventory_remove(&mut build.tile.inventory, item, 1) {
                        break;
                    }
                    inventory_add(&mut snapshot.inventory, item, 1);
                    moved += 1;
                }
            }
            if inner_liquid_capacity.is_some()
                && build.tile.liquid_amount >= 0.01
                && (snapshot.liquid_amount <= 0.2
                    || snapshot.stored_liquid == build.tile.stored_liquid)
            {
                let flow = (40.0 * delta_ticks.max(0.0))
                    .min(build.tile.liquid_amount)
                    .min((100.0 - snapshot.liquid_amount).max(0.0));
                if flow > 0.0 {
                    snapshot.stored_liquid = build.tile.stored_liquid;
                    snapshot.liquid_amount += flow;
                    build.tile.liquid_amount -= flow;
                }
            }
            if battery_capacity > 0.0 && build.tile.power_stored > 0.0 {
                let unloaded = (80.0 * delta_ticks.max(0.0)).min(build.tile.power_stored);
                build.tile.power_stored -= unloaded;
                snapshot.output_liquid_amount = unloaded / delta_ticks.max(1.0);
            }
            let items_empty = item_capacity.is_none() || build.tile.inventory.is_empty();
            let liquids_empty =
                inner_liquid_capacity.is_none() || build.tile.liquid_amount <= 0.011;
            let battery_empty = battery_capacity <= 0.0 || build.tile.power_stored <= 0.0001;
            if items_empty && liquids_empty && battery_empty {
                snapshot.production_progress = 1.0;
            }
        }
        if !refresh_build_payload_sync(world, power, build) {
            continue;
        }
        if let Some(mut live) = world.tiles.get_mut(&key) {
            live.inventory = snapshot.inventory;
            live.payload = snapshot.payload;
            live.transport_progress = snapshot.transport_progress;
            live.production_progress = snapshot.production_progress;
            live.stored_liquid = snapshot.stored_liquid;
            live.liquid_amount = snapshot.liquid_amount;
            live.output_liquid_amount = snapshot.output_liquid_amount;
        }
        changed = true;
    }
    changed
}

pub fn simulate_payload_constructors(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 406 | 407))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if snapshot.payload.is_some() {
            changed |= transfer_payload_forward(world, &snapshot);
            continue;
        }
        let Some(recipe) = decode_constructor_recipe(snapshot.block, &snapshot.config) else {
            continue;
        };
        let requirements = crate::game::content::block_requirements(recipe);
        if requirements.is_empty()
            || requirements.iter().any(|(item, amount)| {
                i16::try_from(*item)
                    .ok()
                    .is_none_or(|item| inventory_count(&snapshot.inventory, item) < *amount)
            })
        {
            continue;
        }
        let efficiency = power.get(&key).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        if efficiency <= 0.0 {
            continue;
        }
        let speed = if snapshot.block == 406 { 0.6 } else { 0.75 };
        let progress = snapshot.production_progress + speed * delta_ticks.max(0.0) * efficiency;
        let build_time = crate::game::content::block_build_time(recipe).max(0.0001);
        if progress < build_time {
            if let Some(mut live) = world.tiles.get_mut(&key) {
                live.production_progress = progress;
            }
            changed = true;
            continue;
        }

        let mut produced = base_building_tombstone(&BaseBuildingState {
            position: snapshot.position,
            block: recipe,
            team: snapshot.team,
            health: crate::game::content::block_health(recipe),
            occupied: vec![snapshot.position],
            inventory: Vec::new(),
        });
        produced.block = recipe;
        produced.team = snapshot.team;
        produced.rotation = snapshot.rotation;
        produced.health = crate::game::content::block_health(recipe);
        let mut sync = Vec::new();
        if encode_payload_build_sync(&mut sync, &produced, power, world).is_err() {
            continue;
        }
        let payload = CarriedPayload::Build(CarriedBuildPayload {
            version: build_payload_version(recipe),
            tile: produced,
            sync,
        });
        if let Some(mut live) = world.tiles.get_mut(&key) {
            for (item, amount) in requirements {
                if let Ok(item) = i16::try_from(*item) {
                    let removed = inventory_remove(&mut live.inventory, item, *amount);
                    debug_assert!(removed, "validated constructor inputs disappeared");
                }
            }
            live.payload = Some(Box::new(payload));
            live.payload_rotation = f32::from(live.rotation) * 90.0;
            live.production_progress = progress % 1.0;
        }
        changed = true;
    }
    changed
}

pub fn simulate_payload_deconstructors(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 404 | 405))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        changed |= dump_deconstructor_items(world, key, 4);
        let Some(mut snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(payload) = snapshot.payload.as_deref() else {
            if snapshot.payload_progress != 0.0 || !snapshot.payload_accum.is_empty() {
                if let Some(mut live) = world.tiles.get_mut(&key) {
                    live.payload_progress = 0.0;
                    live.payload_accum.clear();
                }
                changed = true;
            }
            continue;
        };
        let Some((build_time, requirements)) = carried_payload_requirements(payload) else {
            continue;
        };
        if power.get(&key).copied().unwrap_or(0.0) <= 0.0 {
            continue;
        }
        if snapshot.payload_accum.len() != requirements.len() {
            snapshot.payload_accum = vec![0.0; requirements.len()];
        }
        let capacity = if snapshot.block == 404 { 100 } else { 250 };
        let can_progress = inventory_total(&snapshot.inventory) <= capacity
            && snapshot.payload_accum.iter().all(|value| *value < 1.0);
        if can_progress {
            let shift = delta_ticks.max(0.0) * if snapshot.block == 404 { 3.0 } else { 6.0 }
                / build_time.max(0.0001);
            let real_shift = shift.min((1.0 - snapshot.payload_progress).max(0.0));
            snapshot.payload_progress = (snapshot.payload_progress + shift).min(1.0);
            for (accum, (_, amount)) in snapshot.payload_accum.iter_mut().zip(&requirements) {
                *accum += *amount as f32 * real_shift;
            }
        }
        for (index, (item, _)) in requirements.iter().enumerate() {
            let available = (snapshot.payload_accum[index] + 0.0001).floor() as i32;
            let taken = available.min(capacity - inventory_total(&snapshot.inventory));
            if taken > 0 {
                inventory_add(&mut snapshot.inventory, *item, taken);
                snapshot.payload_accum[index] -= taken as f32;
            }
        }
        if snapshot.payload_progress >= 1.0
            && snapshot.payload_accum.iter().all(|value| *value < 0.0002)
        {
            snapshot.payload = None;
            snapshot.payload_progress = 0.0;
            snapshot.payload_accum.clear();
        }
        if let Some(mut live) = world.tiles.get_mut(&key) {
            live.inventory = snapshot.inventory;
            live.payload = snapshot.payload;
            live.payload_progress = snapshot.payload_progress;
            live.payload_accum = snapshot.payload_accum;
            live.payload_rotation = 90.0;
        }
        changed = true;
    }
    changed
}

pub fn simulate_payload_carriers(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let carriers: Vec<_> = world
        .unit_orders
        .iter()
        .filter(|order| {
            matches!(order.command, 6..=9) && !unit_is_player_controlled(world, order.unit_id)
        })
        .map(|order| (order.unit_id, order.clone()))
        .collect();
    let mut changed = false;

    for (carrier_id, order) in carriers {
        if order.command == 9 && order.payload_cooldown > 0.0 {
            if let Some(mut live_order) = world.unit_orders.get_mut(&carrier_id) {
                live_order.payload_cooldown =
                    (live_order.payload_cooldown - delta_ticks.max(0.0)).max(0.0);
            }
        }
        let Some(carrier) = world.enemies.get(&carrier_id).map(|unit| unit.clone()) else {
            continue;
        };
        if payload_capacity(carrier.unit_type) <= 0.0 {
            continue;
        }
        let at_target = order
            .target_x
            .zip(order.target_y)
            .is_none_or(|(x, y)| (x - carrier.x).hypot(y - carrier.y) <= 10.0);
        let cooldown_ready = world
            .unit_orders
            .get(&carrier_id)
            .is_none_or(|order| order.payload_cooldown <= 0.0);
        if order.command == 8
            || (order.command == 9 && !carrier.payloads.is_empty() && at_target && cooldown_ready)
        {
            let Some(payload) = carrier.payloads.last().cloned() else {
                continue;
            };
            let mut placement_changes = None;
            let dropped = if insert_into_payload_conveyor(world, &carrier, payload.clone()) {
                true
            } else {
                match payload {
                    CarriedPayload::Unit(mut payload) => {
                        payload.id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
                        payload.x = carrier.x;
                        payload.y = carrier.y;
                        payload.velocity_x = 0.0;
                        payload.velocity_y = 0.0;
                        // P0-01: a payload-dropped unit starts with a fresh
                        // controller (the carried copy kept the old authority).
                        payload.authority =
                            crate::network::units::default_unit_authority(world, &payload);
                        world.register_unit_group(payload.id);
                        world.enemies.insert(payload.id, payload.clone());
                        world.unit_orders.insert(
                            payload.id,
                            UnitOrder {
                                unit_id: payload.id,
                                command: default_unit_command(payload.unit_type),
                                stances: 0,
                                payload_cooldown: 0.0,
                                target_kind: 0,
                                target_id: -1,
                                target_x: None,
                                target_y: None,
                                logic_control: 0,
                                queue: Vec::new(),
                            },
                        );
                        true
                    }
                    CarriedPayload::Build(build) => {
                        placement_changes = drop_carried_build(world, &carrier, build);
                        placement_changes.is_some()
                    }
                }
            };
            if !dropped {
                continue;
            }
            if let Some(mut live) = world.enemies.get_mut(&carrier_id) {
                live.payloads.pop();
            }
            if let Ok(frame) = encode_payload_dropped_frame(carrier_id, carrier.x, carrier.y) {
                out.broadcast(frame);
            }
            if let Some(changes) = placement_changes {
                let actor_id = world
                    .player_sessions
                    .iter()
                    .next()
                    .map(|session| session.id)
                    .unwrap_or(1);
                let _ = broadcast_placement_power_configs(out, actor_id, &changes);
            }
            if order.command == 9
                && world
                    .enemies
                    .get(&carrier_id)
                    .is_some_and(|unit| unit.payloads.is_empty())
            {
                if let Some(mut live_order) = world.unit_orders.get_mut(&carrier_id) {
                    live_order.payload_cooldown = 60.0;
                }
                advance_unit_order(world, carrier_id);
            }
            changed = true;
            continue;
        }

        if let (Some(target_x), Some(target_y)) = (order.target_x, order.target_y) {
            let distance = (target_x - carrier.x).hypot(target_y - carrier.y);
            let stop = if order.command == 7 { 1.0 } else { 10.0 };
            if distance > stop {
                if let Some(mut live) = world.enemies.get_mut(&carrier_id) {
                    let speed = effective_unit_speed(&live);
                    let step = (speed * delta_ticks.max(0.0)).min(distance - stop);
                    live.velocity_x = (target_x - carrier.x) / distance * speed;
                    live.velocity_y = (target_y - carrier.y) / distance * speed;
                    live.x += (target_x - carrier.x) / distance * step;
                    live.y += (target_y - carrier.y) / distance * step;
                    live.rotation = (target_y - carrier.y)
                        .atan2(target_x - carrier.x)
                        .to_degrees();
                }
                changed = true;
                continue;
            }
        }
        if order.command == 9 && !cooldown_ready {
            continue;
        }

        if order.command == 7 {
            let position = if order.target_kind == 1 {
                order.target_id
            } else {
                let x = (carrier.x / 8.0).floor() as i32;
                let y = (carrier.y / 8.0).floor() as i32;
                (x << 16) | (y & 0xffff)
            };
            let Some(tile) = dynamic_at(world, position) else {
                continue;
            };
            let block_area = f32::from(crate::game::content::block_size(tile.block) * 8).powi(2);
            if tile.team != carrier.team
                || !is_pickup_payload_supported(tile.block)
                || block_area > payload_capacity(carrier.unit_type) - payload_used(&carrier) + 0.001
            {
                continue;
            }
            let Some(tile) = building_placement::detach_building_from_world(world, tile.position)
            else {
                continue;
            };
            let power = compute_power_efficiency(world);
            let mut sync = Vec::new();
            if encode_payload_build_sync(&mut sync, &tile, &power, world).is_err() {
                let _ = building_placement::attach_building_to_world(world, tile, position);
                continue;
            }
            if let Some(mut live) = world.enemies.get_mut(&carrier_id) {
                live.payloads
                    .push(CarriedPayload::Build(CarriedBuildPayload {
                        version: build_payload_version(tile.block),
                        tile: tile.clone(),
                        sync,
                    }));
            }
            if let Ok(frame) = encode_picked_build_payload_frame(carrier_id, tile.position, true) {
                out.broadcast(frame);
            }
            changed = true;
            continue;
        }

        if !matches!(order.command, 6 | 9) {
            continue;
        }

        let capacity = payload_capacity(carrier.unit_type);
        let mut used = payload_used(&carrier);
        let pickup_range = unit_hit_size(carrier.unit_type) * 2.0;
        let mut targets: Vec<_> = world
            .enemies
            .iter()
            .filter(|target| {
                target.id != carrier_id && target.team == carrier.team && target.elevation <= 0.001
            })
            .filter_map(|target| {
                let distance = (target.x - carrier.x).hypot(target.y - carrier.y);
                (distance <= pickup_range
                    && distance
                        <= unit_hit_size(target.unit_type) + unit_hit_size(carrier.unit_type))
                .then_some((distance, target.id, unit_hit_size(target.unit_type).powi(2)))
            })
            .collect();
        targets.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut picked = false;
        for (_, target_id, area) in targets {
            if used + area > capacity + 0.001 {
                continue;
            }
            let Some(target) = world.enemies.remove(&target_id).map(|(_, unit)| unit) else {
                continue;
            };
            world.unregister_unit_group(target_id);
            crate::network::units::detach_unit_control(world, target_id);
            if let Some(mut live) = world.enemies.get_mut(&carrier_id) {
                live.payloads.push(CarriedPayload::Unit(target));
            }
            if let Ok(frame) = encode_picked_unit_payload_frame(carrier_id, target_id) {
                out.broadcast(frame);
            }
            used += area;
            picked = true;
            changed = true;
            if order.command == 6 {
                break;
            }
        }
        if order.command == 9 && picked {
            if let Some(mut live_order) = world.unit_orders.get_mut(&carrier_id) {
                live_order.payload_cooldown = 60.0;
            }
            advance_unit_order(world, carrier_id);
        }
    }
    changed
}
