//! Payload domain. The listener adapter re-exports these through
//! crate::network::listener::*.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::placement as building_placement;
use crate::network::buildings::snapshot::{
    build_payload_version, encode_payload_build_sync, is_core_block, is_pickup_payload_supported,
};
use crate::network::combat::unit_combat::unit_hit_size;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::network::world::*;
use dashmap::DashMap;

use crate::network::buildings::construction::dynamic_at;
use crate::network::economy::spec::{
    accept_logistics_item_from, configured_link, inventory_remove, offset_position,
    storage_capacity,
};
use crate::network::wire::frame_generated_packet;

pub(crate) fn payload_capacity(unit_type: i16) -> f32 {
    match unit_type {
        22 => 256.0,
        23 => 576.0,
        24 => 1_936.0,
        _ => 0.0,
    }
}

pub(crate) fn payload_used(unit: &EnemyUnit) -> f32 {
    unit.payloads
        .iter()
        .map(|payload| match payload {
            CarriedPayload::Unit(unit) => unit_hit_size(unit.unit_type).powi(2),
            CarriedPayload::Build(build) => {
                f32::from(crate::game::content::block_size(build.tile.block) * 8).powi(2)
            }
        })
        .sum()
}

pub(crate) fn payload_fits_limit(payload: &CarriedPayload, limit: f32) -> bool {
    match payload {
        CarriedPayload::Unit(unit) => unit_hit_size(unit.unit_type) / 8.0 <= limit + 0.001,
        CarriedPayload::Build(build) => {
            f32::from(crate::game::content::block_size(build.tile.block)) <= limit + 0.001
        }
    }
}

pub(crate) fn payload_block_limit(block: i16) -> Option<f32> {
    match block {
        398..=401 => Some(3.0),
        402 => Some(2.5),
        403 => Some(4.0),
        404 | 405 => Some(4.0),
        408 | 409 => Some(3.0),
        _ => None,
    }
}

pub(crate) fn payload_block_accepts(block: i16, payload: &CarriedPayload) -> bool {
    match block {
        404 | 405 => match payload {
            CarriedPayload::Unit(unit) => {
                crate::game::content::unit_requirements(unit.unit_type).is_some()
            }
            CarriedPayload::Build(build) => {
                !crate::game::content::block_requirements(build.tile.block).is_empty()
            }
        },
        408 | 409 => matches!(payload, CarriedPayload::Build(build)
            if storage_capacity(build.tile.block).is_some()
                || liquid_capacity(build.tile.block).is_some_and(|capacity| capacity >= 10.0)
                || power_role(build.tile.block).is_some_and(|role| role.battery_capacity > 0.0)),
        _ => payload_block_limit(block).is_some_and(|limit| payload_fits_limit(payload, limit)),
    }
}

pub(crate) fn insert_into_payload_conveyor(
    world: &DynamicWorld,
    carrier: &EnemyUnit,
    payload: CarriedPayload,
) -> bool {
    let position =
        (((carrier.x / 8.0).floor() as i32) << 16) | ((carrier.y / 8.0).floor() as i32 & 0xffff);
    let Some(tile) = dynamic_at(world, position) else {
        return false;
    };
    let Some(limit) = payload_block_limit(tile.block) else {
        return false;
    };
    if tile.team != carrier.team
        || tile.payload.is_some()
        || !payload_fits_limit(&payload, limit)
        || !payload_block_accepts(tile.block, &payload)
    {
        return false;
    }
    let Some(mut live) = world.tiles.get_mut(&tile.position) else {
        return false;
    };
    live.payload = Some(Box::new(payload));
    live.payload_progress = 0.0;
    live.payload_rotation = carrier.rotation;
    if matches!(live.block, 399 | 401) {
        live.stored_amount = i32::from(live.rotation) + 1;
    }
    true
}

pub(crate) fn payload_conveyor_move_time(block: i16) -> Option<f32> {
    match block {
        398 | 399 => Some(45.0),
        400 | 401 => Some(35.0),
        _ => None,
    }
}

pub(crate) fn offset_position_by(position: i32, rotation: u8, amount: i32) -> i32 {
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

pub(crate) fn choose_payload_router_rotation(
    world: &DynamicWorld,
    router: &DynamicTile,
    payload: &CarriedPayload,
) -> u8 {
    let sorted = (router.config.len() == 4 && router.config[0] == 5).then(|| {
        (
            router.config[1],
            i16::from_be_bytes([router.config[2], router.config[3]]),
        )
    });
    let matches = sorted.is_some_and(|(kind, id)| match payload {
        CarriedPayload::Unit(unit) => kind == 6 && unit.unit_type == id,
        CarriedPayload::Build(build) => kind == 1 && build.tile.block == id,
    });
    let receive_direction =
        u8::try_from(router.stored_amount.saturating_sub(1)).unwrap_or(router.rotation) % 4;
    if matches {
        return receive_direction;
    }
    for offset in 1..=4 {
        let rotation = (router.rotation + offset) % 4;
        if sorted.is_some() && rotation == receive_direction {
            continue;
        }
        let position = offset_position_by(router.position, rotation, 3);
        let viable = dynamic_at(world, position).is_some_and(|next| {
            matches!(next.block, 398..=401)
                && next.team == router.team
                && next.payload.is_none()
                && payload_fits_limit(payload, 3.0)
        });
        if viable {
            return rotation;
        }
    }
    router.rotation
}

pub(crate) fn valid_payload_mass_driver_link(
    world: &DynamicWorld,
    driver: &DynamicTile,
) -> Option<i32> {
    let target = configured_link(driver, world.width, world.height)?;
    let other = dynamic_at(world, target)?;
    let x = (driver.position >> 16) as i16 as f32;
    let y = driver.position as i16 as f32;
    let tx = (other.position >> 16) as i16 as f32;
    let ty = other.position as i16 as f32;
    let range = if driver.block == 402 { 87.5 } else { 262.5 };
    (other.position != driver.position
        && other.block == driver.block
        && other.team == driver.team
        && (tx - x).hypot(ty - y) <= range)
        .then_some(other.position)
}

pub(crate) fn refresh_build_payload_sync(
    world: &DynamicWorld,
    power: &std::collections::HashMap<i32, f32>,
    payload: &mut CarriedBuildPayload,
) -> bool {
    let mut sync = Vec::new();
    if encode_payload_build_sync(&mut sync, &payload.tile, power, world).is_err() {
        return false;
    }
    payload.version = build_payload_version(payload.tile.block);
    payload.sync = sync;
    true
}

pub(crate) fn transfer_payload_forward(world: &DynamicWorld, source: &DynamicTile) -> bool {
    let target_position = offset_position_by(source.position, source.rotation, 3);
    let Some(target) = world
        .tiles
        .get(&target_position)
        .map(|tile| tile.clone())
        .or_else(|| dynamic_at(world, target_position))
    else {
        return false;
    };
    let Some(limit) = payload_block_limit(target.block) else {
        return false;
    };
    if target.team != source.team || target.payload.is_some() {
        return false;
    }
    let Some(payload) = source.payload.as_deref() else {
        return false;
    };
    if !payload_fits_limit(payload, limit) || !payload_block_accepts(target.block, payload) {
        return false;
    }
    let payload = world
        .tiles
        .get_mut(&source.position)
        .and_then(|mut tile| tile.payload.take());
    let Some(payload) = payload else {
        return false;
    };
    if let Some(mut receiver) = world.tiles.get_mut(&target.position) {
        receiver.payload = Some(payload);
        receiver.payload_progress = 0.0;
        receiver.payload_rotation = f32::from(source.rotation) * 90.0;
        receiver.production_progress = 0.0;
    }
    if let Some(mut sender) = world.tiles.get_mut(&source.position) {
        sender.payload_progress = 0.0;
        sender.production_progress = 0.0;
    }
    true
}

pub(crate) fn carried_payload_requirements(
    payload: &CarriedPayload,
) -> Option<(f32, Vec<(i16, i32)>)> {
    match payload {
        CarriedPayload::Unit(unit) => crate::game::content::unit_requirements(unit.unit_type)
            .map(|(time, requirements)| (time, requirements.to_vec())),
        CarriedPayload::Build(build) => {
            let requirements = crate::game::content::block_requirements(build.tile.block);
            (!requirements.is_empty()).then(|| {
                (
                    crate::game::content::block_build_time(build.tile.block),
                    requirements
                        .iter()
                        .filter_map(|(item, amount)| {
                            i16::try_from(*item).ok().map(|item| (item, *amount))
                        })
                        .collect(),
                )
            })
        }
    }
}

pub(crate) const SMALL_CONSTRUCTOR_RECIPES: &[i16] = &[236, 238, 241, 243, 300, 317, 347];
pub(crate) const LARGE_CONSTRUCTOR_RECIPES: &[i16] = &[
    182, 184, 188, 194, 199, 200, 201, 202, 204, 206, 208, 210, 212, 213, 214, 232, 233, 248, 249,
    252, 253, 254, 271, 281, 285, 291, 301, 307, 311, 314, 315, 316, 318, 319, 320, 321, 322, 327,
    328, 331, 332, 334, 336, 337, 346, 348, 360, 361, 362, 363, 364, 365, 366, 367, 368, 369, 370,
    371, 372, 373, 374, 377, 378, 379, 380, 386, 387, 388, 389, 390, 391, 398, 399, 400, 401, 402,
    404, 406, 408, 409, 426, 427, 433, 436, 440,
];
pub(crate) const NEW_LARGE_CODEC_RECIPES: &[i16] = &[
    194, 199, 200, 201, 202, 204, 206, 208, 210, 212, 213, 214, 252, 281, 301, 311, 315, 316, 318,
    319, 320, 321, 322, 327, 328, 331, 332, 334, 336, 337, 367, 368, 369, 370, 371, 372, 373, 374,
    386, 387, 388, 389, 390, 391, 426, 427, 433, 436, 440,
];

pub(crate) fn decode_constructor_recipe(block: i16, config: &[u8]) -> Option<i16> {
    let recipe = match config {
        [5, 1, high, low] => i16::from_be_bytes([*high, *low]),
        _ => return None,
    };
    match block {
        406 if SMALL_CONSTRUCTOR_RECIPES.contains(&recipe) => Some(recipe),
        407 if LARGE_CONSTRUCTOR_RECIPES.contains(&recipe) => Some(recipe),
        _ => None,
    }
}

pub(crate) fn constructor_item_capacity(block: i16, config: &[u8], item: i16) -> i32 {
    decode_constructor_recipe(block, config)
        .and_then(|recipe| {
            crate::game::content::block_requirements(recipe)
                .iter()
                .find(|(accepted, _)| i16::try_from(*accepted).ok() == Some(item))
                .map(|(_, amount)| amount.saturating_mul(2))
        })
        .unwrap_or(0)
}

pub(crate) fn dump_deconstructor_items(world: &DynamicWorld, key: i32, amount: usize) -> bool {
    let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
        return false;
    };
    let mut changed = false;
    for _ in 0..amount {
        let Some((item, _)) = world
            .tiles
            .get(&key)
            .and_then(|tile| tile.inventory.first().copied())
        else {
            break;
        };
        let mut outputs = Vec::new();
        for occupied in &snapshot.occupied {
            for rotation in 0..4 {
                let target = offset_position(*occupied, rotation);
                if !snapshot.occupied.contains(&target) && !outputs.contains(&target) {
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
    changed
}

pub(crate) fn encode_picked_unit_payload_frame(
    carrier_id: i32,
    target_id: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(10);
    payload.write_b(2)?;
    payload.write_i(carrier_id)?;
    payload.write_b(2)?;
    payload.write_i(target_id)?;
    frame_generated_packet(PICKED_UNIT_PAYLOAD_PACKET_ID, &payload, false)
}

pub(crate) fn encode_picked_build_payload_frame(
    carrier_id: i32,
    position: i32,
    on_ground: bool,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(10);
    payload.write_b(2)?;
    payload.write_i(carrier_id)?;
    payload.write_i(position)?;
    payload.write_bool(on_ground)?;
    frame_generated_packet(PICKED_BUILD_PAYLOAD_PACKET_ID, &payload, false)
}

pub(crate) fn drop_carried_build(
    world: &DynamicWorld,
    carrier: &EnemyUnit,
    payload: CarriedBuildPayload,
) -> Option<building_placement::PlacementChanges> {
    drop_carried_build_at(world, carrier.x, carrier.y, payload)
}

pub(crate) fn drop_carried_build_at(
    world: &DynamicWorld,
    x: f32,
    y: f32,
    payload: CarriedBuildPayload,
) -> Option<building_placement::PlacementChanges> {
    let size = crate::game::content::block_size(payload.tile.block);
    let offset = if size.is_multiple_of(2) { 4.0 } else { 0.0 };
    let tile_x = ((x - offset) / 8.0).floor() as i32;
    let tile_y = ((y - offset) / 8.0).floor() as i32;
    let position = (tile_x << 16) | (tile_y & 0xffff);
    building_placement::attach_building_to_world(world, payload.tile, position)
}

pub(crate) fn payload_carrier(world: &DynamicWorld, player: &SessionPlayer) -> Option<EnemyUnit> {
    let id = player
        .controlled_unit
        .standard_id()
        .unwrap_or(player.unit_id);
    world.enemies.get(&id).map(|unit| unit.clone())
}

pub(crate) fn building_center(position: i32, block: i16) -> (f32, f32) {
    let size = f32::from(crate::game::content::block_size(block));
    let offset = if size % 2.0 == 0.0 { 4.0 } else { 0.0 };
    (
        (position >> 16) as i16 as f32 * 8.0 + offset,
        (position as i16 as f32) * 8.0 + offset,
    )
}

pub(crate) fn building_can_pickup(block: i16) -> bool {
    if is_core_block(block) {
        return false;
    }
    // Storage, radar, logic/message/switch/memory — `Building.canPickup()` is
    // false in 158.1. Hidden/non-snapshot blocks are also rejected here.
    if matches!(block, 251 | 345..=348 | 429..=435 | 441..=444) {
        return false;
    }
    is_pickup_payload_supported(block)
}

pub(crate) fn player_is_dead(world: &DynamicWorld, player: &SessionPlayer) -> bool {
    world
        .players
        .get(&player.unit_id)
        .is_some_and(|state| state.dead)
        || world
            .player_profiles
            .get(&player.uuid)
            .is_some_and(|state| state.dead)
}

/// P0-10: `InputHandler.requestBuildPayload`. Returns frames to broadcast.
pub(crate) fn apply_request_build_payload(
    world: &DynamicWorld,
    player: &SessionPlayer,
    position: i32,
) -> Vec<Vec<u8>> {
    let Some(carrier) = payload_carrier(world, player) else {
        return Vec::new();
    };
    if payload_capacity(carrier.unit_type) <= 0.0 {
        return Vec::new();
    }
    let Some(tile) = dynamic_at(world, position) else {
        return Vec::new();
    };
    let (bx, by) = building_center(tile.position, tile.block);
    let size = f32::from(crate::game::content::block_size(tile.block));
    let range = 8.0 * size * 1.2 + 40.0;
    if (carrier.x - bx).hypot(carrier.y - by) > range {
        return Vec::new();
    }
    // `Teams.canInteract`: same team or derelict.
    if tile.team != 0 && tile.team != carrier.team {
        return Vec::new();
    }
    if let Some(inner) = tile.payload.clone() {
        if payload_used(&carrier) + payload_used_of(&inner)
            <= payload_capacity(carrier.unit_type) + 0.001
        {
            if let Some(mut live) = world.tiles.get_mut(&tile.position) {
                live.payload = None;
            }
            if let Some(mut live) = world.enemies.get_mut(&carrier.id) {
                live.payloads.push(*inner);
            }
            return encode_picked_build_payload_frame(carrier.id, tile.position, false)
                .ok()
                .into_iter()
                .collect();
        }
    }
    // Whole-building pickup: visible, `canPickup()`, same team, capacity.
    if !building_can_pickup(tile.block) || tile.team != carrier.team {
        return Vec::new();
    }
    let area = f32::from(crate::game::content::block_size(tile.block) * 8).powi(2);
    if area > payload_capacity(carrier.unit_type) - payload_used(&carrier) + 0.001 {
        return Vec::new();
    }
    let Some(detached) = building_placement::detach_building_from_world(world, tile.position)
    else {
        return Vec::new();
    };
    let power = crate::network::economy::compute_power_efficiency(world);
    let mut sync = Vec::new();
    if encode_payload_build_sync(&mut sync, &detached, &power, world).is_err() {
        let _ = building_placement::attach_building_to_world(world, detached, tile.position);
        return Vec::new();
    }
    if let Some(mut live) = world.enemies.get_mut(&carrier.id) {
        live.payloads
            .push(CarriedPayload::Build(CarriedBuildPayload {
                version: build_payload_version(detached.block),
                tile: detached.clone(),
                sync,
            }));
    }
    encode_picked_build_payload_frame(carrier.id, detached.position, true)
        .ok()
        .into_iter()
        .collect()
}

pub(crate) fn payload_used_of(payload: &CarriedPayload) -> f32 {
    match payload {
        CarriedPayload::Unit(unit) => unit_hit_size(unit.unit_type).powi(2),
        CarriedPayload::Build(build) => {
            f32::from(crate::game::content::block_size(build.tile.block) * 8).powi(2)
        }
    }
}

/// P0-10: `InputHandler.requestUnitPayload`. No admin gate in 158.1.
pub(crate) fn apply_request_unit_payload(
    world: &DynamicWorld,
    player: &SessionPlayer,
    target_id: i32,
) -> Vec<Vec<u8>> {
    let Some(carrier) = payload_carrier(world, player) else {
        return Vec::new();
    };
    if payload_capacity(carrier.unit_type) <= 0.0 {
        return Vec::new();
    }
    let Some(target) = world.enemies.get(&target_id).map(|unit| unit.clone()) else {
        return Vec::new();
    };
    if matches!(
        target.authority,
        crate::network::world::UnitAuthority::Player { .. }
    ) || target.elevation > 0.001
        || target.team != carrier.team
        || target.id == carrier.id
    {
        return Vec::new();
    }
    let range = unit_hit_size(carrier.unit_type) * 2.0 + unit_hit_size(target.unit_type) * 2.0;
    if (carrier.x - target.x).hypot(carrier.y - target.y) > range {
        return Vec::new();
    }
    let area = unit_hit_size(target.unit_type).powi(2);
    if payload_used(&carrier) + area > payload_capacity(carrier.unit_type) + 0.001 {
        return Vec::new();
    }
    let Some((_, taken)) = world.enemies.remove(&target_id) else {
        return Vec::new();
    };
    world.unregister_unit_group(target_id);
    crate::network::units::detach_unit_control(world, target_id);
    if let Some(mut live) = world.enemies.get_mut(&carrier.id) {
        live.payloads.push(CarriedPayload::Unit(taken));
    }
    encode_picked_unit_payload_frame(carrier.id, target_id)
        .ok()
        .into_iter()
        .collect()
}

pub(crate) fn payload_unit_drop_jitter() -> (f32, f32) {
    // desktop 158.1 PayloadUnit.dropUnit: Tmp.v1.rnd(Mathf.random(2f)).
    let angle = rand::random::<f32>() * std::f32::consts::TAU;
    let len = rand::random::<f32>() * 2.0;
    (angle.cos() * len, angle.sin() * len)
}

/// P0-10: `InputHandler.requestDropPayload` — clamp to 4 tiles, never reject
/// solely for distance.
pub(crate) fn apply_request_drop_payload(
    world: &DynamicWorld,
    player: &SessionPlayer,
    x: f32,
    y: f32,
) -> Vec<Vec<u8>> {
    if player_is_dead(world, player) {
        return Vec::new();
    }
    let Some(carrier) = payload_carrier(world, player) else {
        return Vec::new();
    };
    if carrier.payloads.is_empty() {
        return Vec::new();
    }
    let mut dx = x - carrier.x;
    let mut dy = y - carrier.y;
    let limit = 32.0;
    let len = dx.hypot(dy);
    if len > limit && len > 0.0 {
        dx *= limit / len;
        dy *= limit / len;
    }
    let cx = carrier.x + dx;
    let cy = carrier.y + dy;
    let Some(payload) = world
        .enemies
        .get(&carrier.id)
        .and_then(|unit| unit.payloads.last().cloned())
    else {
        return Vec::new();
    };
    let dropped = if insert_into_payload_conveyor(world, &carrier, payload.clone()) {
        true
    } else {
        match payload {
            CarriedPayload::Unit(mut unit) => {
                unit.id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
                let (jx, jy) = payload_unit_drop_jitter();
                unit.x = cx + jx;
                unit.y = cy + jy;
                unit.authority = crate::network::units::default_unit_authority(world, &unit);
                world.register_unit_group(unit.id);
                world.enemies.insert(unit.id, unit.clone());
                world.unit_orders.insert(
                    unit.id,
                    crate::network::world::UnitOrder {
                        unit_id: unit.id,
                        command: crate::network::economy::default_unit_command(unit.unit_type),
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
            CarriedPayload::Build(build) => drop_carried_build_at(world, cx, cy, build).is_some(),
        }
    };
    if !dropped {
        return Vec::new();
    }
    if let Some(mut live) = world.enemies.get_mut(&carrier.id) {
        live.payloads.pop();
    }
    encode_payload_dropped_frame(carrier.id, cx, cy)
        .ok()
        .into_iter()
        .collect()
}

pub(crate) fn encode_payload_dropped_frame(
    unit_id: i32,
    x: f32,
    y: f32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(13);
    payload.write_b(2)?;
    payload.write_i(unit_id)?;
    payload.write_f(x)?;
    payload.write_f(y)?;
    frame_generated_packet(PAYLOAD_DROPPED_PACKET_ID, &payload, false)
}
