//! Block snapshot codecs.
//!
//! Every `encode_*_sync` writeSync layout for the 158.1 baseline lives here.
//! The listener re-exports the public entry points
//! (`encode_dynamic_tile_sync`, `is_block_snapshot_supported`, ...)
//! so existing callers and integration tests keep working.

use crate::engine::typeio::*;
use crate::game::content as game_content;
use crate::network::buildings::config as building_config;
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::reactor;
use crate::network::decoders::BuildPlan;
use crate::network::economy::items_for_team;
use crate::network::economy::payload::{decode_constructor_recipe, valid_payload_mass_driver_link};
use crate::network::economy::spec::{
    configured_item, configured_link, factory_recipe, generator_fuel, inventory_count,
    liquid_turret_weapon, mass_driver_state, power_turret_weapon, storage_capacity,
    storage_linked_to_core, turret_ammo, turret_can_target, turret_max_ammo, valid_bridge_link,
    valid_mass_driver_link,
};
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::controller::{controlling_session_for_building, write_carried_payload};
use crate::network::wire::auth::player_team;
use crate::network::wire::encode::frame_generated_packet;
use crate::network::wire::tile_config::{configured_unit_command, unit_factory_plan};
use crate::network::world::*;
use dashmap::DashMap;
use std::io::Error;
use std::io::ErrorKind;
use std::sync::Arc;

pub fn encode_factory_snapshot_tiles(
    tiles: &[DynamicTile],
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let amount = i16::try_from(tiles.len())
        .map_err(|_| Error::new(ErrorKind::InvalidData, "too many block snapshots"))?;
    let mut data = Vec::new();
    for tile in tiles {
        data.write_i(tile.position)?;
        data.write_s(tile.block)?;
        encode_dynamic_tile_sync(&mut data, tile, power, world)?;
    }
    let data_len = i16::try_from(data.len())
        .map_err(|_| Error::new(ErrorKind::InvalidData, "block snapshot is too large"))?;
    let mut payload = Vec::with_capacity(data.len() + 4);
    payload.write_s(amount)?;
    payload.write_s(data_len)?; // TypeIO.writeBytes
    payload.extend_from_slice(&data);
    frame_generated_packet(BLOCK_SNAPSHOT_PACKET_ID, &payload, false)
}

/// Encodes a single building's writeSync layout. `pub` for integration tests.
pub fn encode_dynamic_tile_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    if tile.block == 331 {
        // Oil extractor (Fracker, 158.1): FrackerBuild has no writeSync
        // override (SolidPump/Pump chain), so the codec is the base layout
        // with items+power+liquids and NO progress/warmup tails — unlike
        // the GenericCrafter path. Verified against the JAR: the old codec
        // left 8 trailing bytes in VerifyProtocol158.
        encode_basic_modules_sync(output, tile, power, true, true, true)
    } else if generic_crafter_time(tile.block).is_some() {
        encode_generic_crafter_sync(output, tile, power)
    } else if mender_spec(tile.block).is_some() {
        encode_mender_sync(output, tile, power)
    } else if overdrive_spec(tile.block).is_some() {
        encode_overdrive_sync(output, tile, power)
    } else if tile.block == FORCE_PROJECTOR_BLOCK {
        encode_force_projector_sync(output, tile, power)
    } else if tile.block == REGEN_PROJECTOR_BLOCK {
        encode_regen_projector_sync(output, tile, power)
    } else if tile.block == SHOCKWAVE_TOWER_BLOCK {
        encode_shockwave_tower_sync(output, tile, power)
    } else if tile.block == SHOCK_MINE_BLOCK {
        encode_shock_mine_sync(output, tile)
    } else if matches!(tile.block, 377..=379 | 386..=388) {
        encode_unit_factory_sync(output, tile, power)
    } else if matches!(tile.block, 380..=383 | 389..=392) {
        encode_reconstructor_sync(output, tile, power)
    } else if matches!(tile.block, 398..=401) {
        encode_payload_conveyor_sync(output, tile)
    } else if matches!(tile.block, 402 | 403) {
        encode_payload_mass_driver_sync(output, tile, power, world)
    } else if matches!(tile.block, 404 | 405) {
        encode_payload_deconstructor_sync(output, tile, power)
    } else if matches!(tile.block, 406 | 407) {
        encode_payload_constructor_sync(output, tile, power)
    } else if matches!(tile.block, 408 | 409) {
        encode_payload_loader_sync(output, tile, power)
    } else if tile.block == 410 {
        encode_power_node_sync(output, tile, power, world)
    } else if tile.block == 411 {
        encode_simple_power_sync(output, tile, power)
    } else if tile.block == 412 {
        encode_item_source_sync(output, tile)
    } else if tile.block == 413 {
        encode_simple_wall_sync(output, tile)
    } else if tile.block == 414 {
        encode_liquid_source_sync(output, tile)
    } else if tile.block == 415 {
        encode_liquid_only_base_sync(output, tile)
    } else if tile.block == 418 {
        encode_heat_source_sync(output, tile)
    } else if matches!(tile.block, 317..=319) {
        encode_simple_power_sync(output, tile, power)
    } else if matches!(tile.block, 206..=208) {
        encode_simple_wall_sync(output, tile)
    } else if matches!(tile.block, 325..=328 | 330 | 335..=338) {
        encode_drill_sync(output, tile, power)
    } else if tile.block == 329 {
        // WaterExtractor (158.1): SolidPump/Fracker chain has no writeSync
        // override — the base layout with the block modules only (no
        // progress/warmup tail). Round 74d: added to the periodic batch so
        // the server's power status reaches the client (the client's
        // ConsumePower.efficiency == synced power.status; without a snapshot
        // the extractor stays at status 0 and never mines).
        encode_basic_modules_sync(output, tile, power, false, true, true)
    } else if matches!(tile.block, 331 | 333 | 334) {
        encode_wall_crafter_sync(output, tile, power)
    } else if matches!(tile.block, 193 | 194) {
        encode_separator_sync(output, tile, power)
    } else if tile.block == 198 {
        encode_power_liquid_base_sync(output, tile, power)
    } else if tile.block == 209 {
        encode_liquid_only_base_sync(output, tile)
    } else if matches!(tile.block, 228 | 229 | 239) {
        encode_door_sync(output, tile)
    } else if tile.block == 244 {
        encode_shield_wall_sync(output, tile, power)
    } else if tile.block == 251 {
        encode_radar_sync(output, tile, power)
    } else if matches!(tile.block, 257 | 258 | 260) {
        encode_conveyor_sync(output, tile)
    } else if matches!(tile.block, 259 | 279) {
        encode_stack_conveyor_sync(output, tile, power, world)
    } else if matches!(tile.block, 272 | 273) {
        encode_duct_sync(output, tile)
    } else if matches!(tile.block, 274 | 280) {
        encode_duct_router_sync(output, tile, power)
    } else if matches!(tile.block, 275 | 276) {
        encode_simple_items_sync(output, tile)
    } else if matches!(tile.block, 277 | 298) {
        encode_base_only_with_items_or_liquids_sync(output, tile, power)
    } else if tile.block == 278 {
        encode_directional_unloader_sync(output, tile)
    } else if tile.block == 282 {
        encode_unit_cargo_unload_point_sync(output, tile)
    } else if matches!(tile.block, 295..=297 | 299..=301) {
        encode_liquid_block_sync(output, tile, power)
    } else if matches!(tile.block, 302..=304) {
        encode_power_node_sync(output, tile, power, world)
    } else if tile.block == 305 {
        encode_simple_wall_sync(output, tile)
    } else if matches!(tile.block, 309..=312 | 315 | 316 | 320..=322) {
        encode_power_generator_sync(output, tile)
    } else if tile.block == 323 {
        encode_variable_reactor_sync(output, tile, power)
    } else if tile.block == 324 {
        encode_heater_generator_sync(output, tile, power)
    } else if matches!(tile.block, 339..=344) {
        encode_core_sync(output, tile, world)
    } else if matches!(tile.block, 356 | 359 | 384 | 385) {
        encode_turret_rotation_sync(output, tile, power, true, matches!(tile.block, 385))
    } else if matches!(tile.block, 393..=395) {
        encode_unit_assembler_sync(output, tile, power)
    } else if tile.block == 396 {
        encode_payload_block_base_sync(output, tile, power)
    } else if tile.block == 397 {
        encode_power_liquid_base_sync(output, tile, power)
    } else if tile.block == 419 {
        encode_light_sync(output, tile, power)
    } else if matches!(tile.block, 425 | 426) {
        encode_launch_pad_sync(output, tile, power)
    } else if tile.block == 427 {
        encode_campaign_pad_sync(output, tile, power)
    } else if tile.block == 428 {
        encode_accelerator_sync(output, tile, power)
    } else if matches!(tile.block, 429 | 441 | 444) {
        encode_message_sync(output, tile)
    } else if matches!(tile.block, 430 | 445) {
        encode_switch_sync(output, tile)
    } else if matches!(tile.block, 255 | 256) {
        encode_shield_sync(output, tile, power)
    } else if matches!(tile.block, 431..=433 | 442) {
        encode_logic_processor_sync(output, tile)
    } else if matches!(tile.block, 434 | 435 | 443) {
        encode_memory_sync(output, tile)
    } else if matches!(tile.block, 436..=438) {
        encode_logic_display_sync(output, tile)
    } else if matches!(tile.block, 439 | 440) {
        encode_canvas_sync(output, tile)
    } else if tile.block == 252 {
        encode_build_tower_sync(output, tile, power)
    } else if tile.block == 281 {
        encode_unit_cargo_loader_sync(output, tile, power)
    } else if tile.block == 271 {
        encode_mass_driver_sync(output, tile, power, world)
    } else if matches!(tile.block, 306 | 307) {
        encode_battery_sync(output, tile)
    } else if matches!(tile.block, 293 | 294) {
        encode_liquid_bridge_sync(output, tile, power, world)
    } else if matches!(tile.block, 262 | 263) {
        encode_item_bridge_sync(output, tile, power, world)
    } else if tile.block == 261 {
        encode_junction_sync(output, tile, world)
    } else if matches!(tile.block, 264..=270) {
        encode_item_logistics_sync(output, tile)
    } else if is_snapshot_item_turret(tile.block)
        || matches!(
            tile.block,
            353 | 354 | 355 | 360 | 366 | 369 | 372 | 373 | 376
        )
    {
        encode_turret_sync(output, tile, power, world)
    } else if is_simple_liquid_snapshot(tile.block) {
        encode_liquid_block_sync(output, tile, power)
    } else if storage_capacity(tile.block).is_some() {
        // A core-linked vault displays ITS OWN team's core inventory.
        let core_items = world
            .filter(|world| storage_linked_to_core(world, tile))
            .map(|world| items_for_team(world, tile.team));
        encode_storage_sync(output, tile, core_items.as_deref())
    } else if matches!(tile.block, 216..=227 | 230..=243) {
        encode_simple_wall_sync(output, tile)
    } else {
        encode_power_generator_sync(output, tile)
    }
}

pub fn generic_crafter_time(block: i16) -> Option<f32> {
    factory_recipe(block)
        .map(|recipe| recipe.craft_time)
        .or_else(|| liquid_factory_recipe(block).map(|recipe| recipe.craft_time))
        .or(match block {
            199 => Some(50.0),
            200 => Some(10.0),
            201 | 203 | 204 | 205 | 215 => Some(80.0),
            202 => Some(120.0),
            210 => Some(33.75),
            212 => Some(45.0),
            213 => Some(20.0),
            214 => Some(30.0),
            332 => Some(120.0),
            _ => None,
        })
}

/// Whether the server emits a BlockSnapshot codec for this block. `pub` for
/// integration tests.
pub fn is_block_snapshot_supported(block: i16) -> bool {
    generic_crafter_time(block).is_some()
        || mender_spec(block).is_some()
        || overdrive_spec(block).is_some()
        || block == FORCE_PROJECTOR_BLOCK
        || block == REGEN_PROJECTOR_BLOCK
        || block == SHOCKWAVE_TOWER_BLOCK
        || block == SHOCK_MINE_BLOCK
        || matches!(block, 377..=383 | 386..=392)
        || matches!(block, 398..=401)
        || matches!(block, 402 | 403)
        || matches!(block, 404 | 405)
        || matches!(block, 406 | 407)
        || matches!(block, 408 | 409)
        || matches!(block, 206..=208 | 317..=319)
        || matches!(block, 325..=331 | 333..=338)
        || matches!(block, 193 | 194 | 252 | 281 | 426 | 427 | 433 | 436 | 440)
        || storage_capacity(block).is_some()
        || block == 271
        || matches!(block, 306 | 307)
        || matches!(block, 309..=312 | 315 | 316 | 320..=324)
        || matches!(block, 293 | 294)
        || matches!(block, 262 | 263)
        || block == 261
        || matches!(block, 264..=270)
        || is_snapshot_item_turret(block)
        || matches!(block, 353 | 354 | 355 | 360 | 366 | 369 | 372 | 373 | 376)
        || is_simple_liquid_snapshot(block)
        || matches!(block, 308 | 313 | 314)
        || matches!(block, 198 | 209 | 216..=244 | 251 | 257..=260 | 272..=282 | 295..=305)
        || matches!(block, 255 | 256)
        // Official NetServer.writeBlockSnapshots iterates
        // `indexer.getFlagged(team, BlockFlag.synced)`: cores (339-344) are
        // NOT synced and never appear in the periodic 6s batch — they reach
        // the client through the world stream. The individual
        // RequestBlockSnapshot reply still covers them (see the handler).
        || matches!(block, 356 | 359 | 384 | 385 | 393..=397)
        || matches!(block, 410..=415 | 418)
        || matches!(block, 419 | 425 | 428..=432 | 434 | 435 | 437..=439 | 441..=445)
}

/// Official `BlockFlag.synced` membership for the periodic block-snapshot
/// batch: cores are excluded, everything `is_block_snapshot_supported`
/// accepts is included.
pub fn is_batch_snapshot_supported(block: i16) -> bool {
    is_block_snapshot_supported(block) && !matches!(block, 339..=344)
}

/// Core blocks (339-344): never in the periodic batch, but the individual
/// `RequestBlockSnapshot` reply answers them like the official
/// `NetServer.requestBlockSnapshot` (build.team == player.team()).
pub fn is_core_block(block: i16) -> bool {
    matches!(block, 339..=344)
}

pub fn encode_payload_mass_driver_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power
        .get(&tile.position)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(2)?; // power module
    output.write_s(0)?;
    output.write_f(efficiency)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    output.write_f(0.0)?; // payVector x
    output.write_f(0.0)?; // payVector y
    output.write_f(tile.payload_rotation)?;
    if let Some(payload) = tile.payload.as_deref() {
        write_carried_payload(output, payload)?;
    } else {
        output.write_bool(false)?;
    }
    let link = world
        .and_then(|world| valid_payload_mass_driver_link(world, tile))
        .unwrap_or(-1);
    output.write_i(link)?;
    output.write_f(if tile.payload_rotation == 0.0 {
        90.0
    } else {
        tile.payload_rotation
    })?;
    let state = u8::from(tile.payload.is_some() && link != -1) * 2;
    output.write_b(state)?;
    output.write_f(0.0)?; // reloadCounter
    output.write_f(tile.transport_progress)?; // charge
    output.write_bool(tile.payload.is_some())?;
    output.write_bool(tile.transport_progress > 0.0)?;
    Ok(())
}

pub fn encode_payload_loader_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power
        .get(&tile.position)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(15)?; // items(1)|power(2)|liquids(4)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?; // power links
    output.write_f(efficiency)?;
    write_liquid_module(output, tile)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    output.write_f(0.0)?; // payVector x
    output.write_f(0.0)?; // payVector y
    output.write_f(tile.payload_rotation)?;
    if let Some(payload) = tile.payload.as_deref() {
        write_carried_payload(output, payload)?;
    } else {
        output.write_bool(false)?;
    }
    output.write_bool(tile.production_progress >= 1.0)?; // exporting
    Ok(())
}

pub fn encode_payload_deconstructor_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power
        .get(&tile.position)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(11)?; // items(1)|power(2)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?;
    output.write_f(efficiency)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    output.write_f(0.0)?;
    output.write_f(0.0)?;
    output.write_f(tile.payload_rotation)?;
    output.write_bool(false)?; // PayloadBlock payload has moved into deconstructing.
    output.write_f(tile.payload_progress.clamp(0.0, 1.0))?;
    output.write_s(i16::try_from(tile.payload_accum.len()).unwrap_or(0))?;
    for value in &tile.payload_accum {
        output.write_f(*value)?;
    }
    if let Some(payload) = tile.payload.as_deref() {
        write_carried_payload(output, payload)?;
    } else {
        output.write_bool(false)?;
    }
    Ok(())
}

pub fn encode_payload_constructor_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power
        .get(&tile.position)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(11)?; // items(1)|power(2)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?; // power links
    output.write_f(efficiency)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    output.write_f(0.0)?; // payVector x
    output.write_f(0.0)?; // payVector y
    output.write_f(tile.payload_rotation)?;
    if let Some(payload) = tile.payload.as_deref() {
        write_carried_payload(output, payload)?;
    } else {
        output.write_bool(false)?;
    }
    output.write_f(tile.production_progress)?;
    output.write_s(decode_constructor_recipe(tile.block, &tile.config).unwrap_or(-1))?;
    Ok(())
}

pub fn encode_simple_power_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let status = power
        .get(&tile.position)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(2)?;
    output.write_s(0)?; // PowerModule links are reconstructed by the receiving world.
    output.write_f(status)?;
    output.write_b((status * 255.0) as u8)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_conveyor_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        true,
        false,
        false,
    )?;
    // ConveyorBuild.write: i32 len, then per item: s16 item id, byte x*127,
    // byte y*255-128. Each item carries its own position along the belt; the
    // client animates these offsets locally between snapshots. Old saves (or
    // routers that fell through) use the shared fields.
    // Sanitize before encoding as well as during simulation: a world streamed
    // immediately after loading an old checkpoint must never expose the
    // client's ConveyorBuild to non-finite/overlapping offsets.
    let items: Vec<(i16, f32)> = normalized_conveyor_items(tile)
        .into_iter()
        // Rust keeps the logical front at index 0 for FIFO/backpressure.
        // ConveyorBuild keeps the front at ids[len - 1], so translate the
        // order at the wire boundary or the client animates the rear item as
        // the head until every correction snapshot.
        .rev()
        .collect();
    output.write_i(i32::try_from(items.len()).unwrap_or(i32::MAX))?;
    for (item, progress) in items {
        output.write_s(item)?;
        output.write_b((0.0_f32 * 127.0) as i8 as u8)?;
        // Java's `(byte)(ys[i] * 255 - 128)` truncates toward zero.
        output.write_b((progress * 255.0 - 128.0) as i8 as u8)?;
    }
    Ok(())
}

pub fn encode_stack_conveyor_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    _world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // 158.1: both plastanium (259) and surge (279) are StackConveyor
    // subclasses; only surge (279) has a power module (Blocks$225).
    // Round 74: the official StackConveyorBuild ItemModule IS the item
    // stack (conveyor_items), not the generic inventory — serializing
    // `inventory` left the client-side plastanium belt empty and made its
    // reel/reset state look broken on every correction snapshot.
    let stack: Vec<(i16, i32)> = {
        let item = tile
            .conveyor_items
            .first()
            .map(|(item, _)| *item)
            .unwrap_or(-1);
        // Round 74: clamp to the official itemCapacity (10). Legacy saves
        // could hold 300+ items in one stretch (unbounded batch appends);
        // advertising more than the module capacity made the client draw an
        // enormous stack and its own machine jam.
        let amount = if item >= 0 {
            i32::try_from(tile.conveyor_items.len())
                .unwrap_or(i32::MAX)
                .min(10)
        } else {
            0
        };
        if item >= 0 && amount > 0 {
            vec![(item, amount)]
        } else {
            Vec::new()
        }
    };
    let has_power = tile.block == 279;
    encode_basic_modules_sync_with_items(
        output,
        tile,
        power,
        true,
        has_power,
        false,
        Some(&stack),
    )?;
    // P1: StackConveyorBuild.write = i32 link + f32 cooldown. Both are
    // RUNTIME state (the official machine reels from `link` and cools down
    // after a transfer); the legacy port wrote link derived from config and
    // cooldown 0. The client renders the crater from link and reels with
    // cooldown, so the authoritative values must be sent.
    output.write_i(tile.stack_link)?;
    output.write_f(tile.stack_cooldown)?;
    Ok(())
}

pub fn encode_wall_crafter_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    // WallCrafterBuild has no writeSync override; only the base Building layout
    // with the per-block modules (333: items+power, 334: items+power+liquids).
    encode_basic_modules_sync(output, tile, power, true, true, tile.block == 334)
}

pub fn encode_power_liquid_base_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    // Building.writeSync only (no subclass fields): Incinerator (198),
    // Unit Repair Tower (397).
    encode_basic_modules_sync(output, tile, power, false, true, true)
}

pub fn encode_liquid_only_base_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
) -> std::io::Result<()> {
    // ItemIncineratorBuild has no writeSync override; only the base layout.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        false,
        false,
        true,
    )
}

pub fn encode_item_source_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // ItemSourceBuild.write: Building base with an ItemModule, followed by
    // the selected output item ID. The module is transiently set to one only
    // around dump(), so an authoritative snapshot is normally empty.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        true,
        false,
        false,
    )?;
    output.write_s(
        building_config::selected_item(&tile.config)
            .flatten()
            .unwrap_or(-1),
    )?;
    Ok(())
}

pub fn encode_liquid_source_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // LiquidSourceBuild revision 1: base LiquidModule + selected source ID.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        false,
        false,
        true,
    )?;
    output.write_s(
        building_config::selected_liquid(&tile.config)
            .flatten()
            .unwrap_or(-1),
    )?;
    Ok(())
}

pub fn encode_heat_source_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // HeatProducerBuild extends GenericCrafterBuild: base, progress, warmup,
    // then current heat. The source has no inputs and converges immediately
    // to its configured 1000 heat output.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        true,
        false,
        false,
    )?;
    output.write_f(0.0)?;
    output.write_f(1.0)?;
    output.write_f(tile.mass_driver_rotation.max(0.0))?;
    Ok(())
}

pub fn encode_door_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // DoorBuild/AutoDoorBuild.write: bool open.
    encode_simple_wall_sync(output, tile)?;
    output.write_bool(tile.door_open)?;
    Ok(())
}

pub fn encode_shield_wall_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // ShieldWallBuild.write: f32 shield.
    encode_basic_modules_sync(output, tile, power, false, true, false)?;
    output.write_f(tile.shield.max(0.0))?;
    Ok(())
}

pub fn encode_radar_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // RadarBuild.write: f32 progress.
    encode_basic_modules_sync(output, tile, power, false, true, false)?;
    output.write_f(tile.production_progress.max(0.0))?;
    Ok(())
}

pub fn encode_duct_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // DuctBuild.write: byte recDir.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        true,
        false,
        false,
    )?;
    output.write_b(tile.duct_rec_dir % 4)?;
    Ok(())
}

pub fn encode_duct_router_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // DuctRouterBuild.write (StackRouterBuild inherits it): s16 sortItem.
    let has_power = tile.block == 280;
    encode_basic_modules_sync(output, tile, power, true, has_power, false)?;
    output.write_s(configured_item(&tile.config).unwrap_or(-1))?;
    Ok(())
}

pub fn encode_simple_items_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    // OverflowDuctBuild has no writeSync override; base with ItemModule only.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        true,
        false,
        false,
    )
}

pub fn encode_base_only_with_items_or_liquids_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    // DuctBridgeBuild (277 items / 298 liquids) has no writeSync override.
    let has_items = tile.block == 277;
    let has_liquids = tile.block == 298;
    encode_basic_modules_sync(output, tile, power, has_items, false, has_liquids)
}

pub fn encode_directional_unloader_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // DirectionalUnloaderBuild.write: s16 unloadItem, s16 offset.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        true,
        false,
        false,
    )?;
    output.write_s(configured_item(&tile.config).unwrap_or(-1))?;
    output.write_s(tile.unloader_offset)?;
    Ok(())
}

pub fn encode_unit_cargo_unload_point_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // UnitCargoUnloadPointBuild.write: s16 item, bool stale.
    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        true,
        false,
        false,
    )?;
    output.write_s(configured_item(&tile.config).unwrap_or(-1))?;
    output.write_bool(false)?; // stale is a transient client-side flag.
    Ok(())
}

pub fn power_node_links(
    tile: &DynamicTile,
    width: i32,
    height: i32,
    world: Option<&DynamicWorld>,
) -> Vec<i32> {
    // tile.power_links is the authoritative link list (map decode from the
    // PowerModule + SOL-010 config writes); it feeds PowerModule.write in the
    // snapshot exactly like the official `power.links` (PowerNodeBuild.config
    // derives from it, PowerNode.java:461-467).
    if !tile.power_links.is_empty() {
        return tile.power_links.clone();
    }
    // Fallback: decode ONLY the idempotent Point2[] config (tag 8) — used by
    // snapshot-parity fixtures that configure links purely through config
    // bytes. Tag 1 (Integer) and tag 7 (Point2) are STATEFUL toggles: after a
    // runtime toggle-off leaves power_links empty, decoding them would
    // re-advertise a removed link. Each decoded link is validated against
    // existence/team/range so an accepted-but-not-linked target (absent or
    // out-of-range) is never advertised. Links are relative to the node tile.
    let mut links = Vec::new();
    let push = |links: &mut Vec<i32>, dx: i32, dy: i32| {
        let x = (tile.position >> 16) as i16 as i32 + dx;
        let y = tile.position as i16 as i32 + dy;
        if (0..width).contains(&x) && (0..height).contains(&y) {
            links.push((x << 16) | (y as u16 as i32));
        }
    };
    if let [8, count, rest @ ..] = tile.config.as_slice() {
        for chunk in rest
            .as_chunks::<4>()
            .0
            .iter()
            .take((*count).min(100) as usize)
        {
            let packed = i32::from_be_bytes(*chunk);
            let abs = {
                let x = (tile.position >> 16) as i16 as i32 + (packed >> 16);
                let y = tile.position as i16 as i32 + (packed as i16 as i32);
                (x << 16) | (y as u16 as i32)
            };
            // Runtime validation: a link the plan rejected (absent target,
            // wrong team, out of range) must not be advertised. Isolated
            // snapshot fixtures (world = None) keep the raw decode.
            let valid =
                world.is_none_or(|world| power_nodes::link_valid_for_node(world, tile, abs));
            if valid {
                push(&mut links, packed >> 16, packed as i16 as i32);
            }
        }
    } else if let [7, rest @ ..] = tile.config.as_slice() {
        // A single Point2 is emitted by TypeIO for a one-link configuration
        // in older clients (tag 7 rather than the Point2[] tag 8).
        if rest.len() >= 8 {
            let dx = i32::from_be_bytes(rest[0..4].try_into().unwrap());
            let dy = i32::from_be_bytes(rest[4..8].try_into().unwrap());
            let x = (tile.position >> 16) as i16 as i32 + dx;
            let y = tile.position as i16 as i32 + dy;
            let abs = (x << 16) | (y as u16 as i32);
            if world.is_none_or(|world| power_nodes::link_valid_for_node(world, tile, abs)) {
                push(&mut links, dx, dy);
            }
        }
    }
    links
}

pub fn encode_power_node_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // PowerNodeBuild has no writeSync override; its state lives entirely in
    // the PowerModule (link list + graph status).
    let status = power
        .get(&tile.position)
        .copied()
        .unwrap_or(if tile.block == 410 { 1.0 } else { 0.0 })
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(2)?;
    let (width, height) =
        world.map_or((MAP_WIDTH, MAP_HEIGHT), |world| (world.width, world.height));
    let links = power_node_links(tile, width, height, world);
    output.write_s(i16::try_from(links.len()).unwrap_or(i16::MAX))?;
    for link in links {
        output.write_i(link)?;
    }
    output.write_f(status)?;
    output.write_b(255)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_variable_reactor_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // VariableReactorBuild.write: PowerGenerator (productionEfficiency,
    // generateTime) + f32 heat + f32 instability + f32 warmup.
    encode_basic_modules_sync(output, tile, power, false, true, true)?;
    output.write_f(0.0)?; // productionEfficiency
    output.write_f(0.0)?; // generateTime
    output.write_f(tile.output_liquid_amount.max(0.0))?; // heat
    output.write_f(0.0)?; // instability
    output.write_f(tile.transport_progress.clamp(0.0, 1.0))?; // warmup
    Ok(())
}

pub fn encode_heater_generator_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // HeaterGeneratorBuild.write: ConsumeGenerator (PowerGenerator fields) +
    // f32 heat.
    encode_basic_modules_sync(output, tile, power, true, true, true)?;
    output.write_f(0.0)?; // productionEfficiency
    output.write_f(0.0)?; // generateTime
    output.write_f(tile.output_liquid_amount.max(0.0))?; // heat
    Ok(())
}

pub fn encode_core_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // CoreBuild.write: base (ItemModule) + TypeIO.writeVecNullable(commandPos).
    // The core snapshot shows THIS CORE'S TEAM inventory (official
    // `CoreBuild.items` of the owning team; survival/attack == team 1).
    let core_items = world
        .map(|world| items_for_team(world, tile.team))
        .unwrap_or_default();
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(1)?; // ItemModule only
    let items: Vec<_> = core_items
        .iter()
        .enumerate()
        .filter_map(|(item, amount)| {
            (*amount > 0).then_some((i16::try_from(item).unwrap_or(-1), *amount))
        })
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_b(255)?;
    output.write_b(255)?;
    let command = world.and_then(|world| {
        world
            .building_commands
            .get(&tile.position)
            .map(|command| (command.target_x, command.target_y))
    });
    match command {
        Some((x, y)) => {
            output.write_f(x)?;
            output.write_f(y)?;
        }
        None => {
            output.write_f(f32::NAN)?;
            output.write_f(f32::NAN)?;
        }
    }
    Ok(())
}

pub fn encode_turret_rotation_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    has_power: bool,
    has_liquids: bool,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // TractorBeamBuild / PointDefenseBuild / RepairPointBuild write a single
    // f32 rotation after the base. Their read revisions (0/1) match.
    encode_basic_modules_sync(output, tile, power, false, has_power, has_liquids)?;
    // TractorBeamBuild / PointDefenseBuild / RepairPointBuild write their
    // building rotation (writeBase rotation byte * 90°); the client uses it
    // to aim the beam/repair laser.
    output.write_f(f32::from(tile.rotation) * 90.0)?;
    Ok(())
}

pub fn encode_unit_assembler_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // UnitAssemblerBuild.write: PayloadBlockBuild (payVector, payRotation,
    // payload) + f32 progress + byte unit count + int unit ids +
    // PayloadSeq (negated size) + TypeIO.writeVecNullable(commandPos).
    encode_basic_modules_sync(output, tile, power, true, true, true)?;
    output.write_f(0.0)?; // payVector.x
    output.write_f(0.0)?; // payVector.y
    output.write_f(tile.payload_rotation)?;
    output.write_bool(false)?; // no carried payload entity
    output.write_f(tile.production_progress.max(0.0))?; // progress
    output.write_b(0)?; // units.size
    output.write_s(0)?; // PayloadSeq: -size == 0 means empty in the new format
    output.write_f(f32::NAN)?; // commandPos x
    output.write_f(f32::NAN)?; // commandPos y
    Ok(())
}

pub fn encode_payload_block_base_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // UnitAssemblerModuleBuild inherits PayloadBlockBuild.write: base +
    // payVector.x/y + payRotation + Payload.write.
    encode_basic_modules_sync(output, tile, power, false, true, false)?;
    output.write_f(0.0)?;
    output.write_f(0.0)?;
    output.write_f(tile.payload_rotation)?;
    output.write_bool(false)?;
    Ok(())
}

pub fn encode_light_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // LightBuild.write: i32 color (Pal.accent.rgba() by default).
    encode_basic_modules_sync(output, tile, power, false, true, false)?;
    output.write_i(if tile.light_color == 0 {
        -1_900_545
    } else {
        tile.light_color
    })?;
    Ok(())
}

pub fn encode_launch_pad_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // LaunchPadBuild.write: f32 launchCounter.
    encode_basic_modules_sync(output, tile, power, true, true, tile.block == 426)?;
    output.write_f(tile.production_progress.max(0.0))?;
    Ok(())
}

pub fn encode_accelerator_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // AcceleratorBuild.write: f32 progress.
    encode_basic_modules_sync(output, tile, power, true, true, false)?;
    output.write_f(tile.production_progress.max(0.0))?;
    Ok(())
}

pub fn encode_message_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // MessageBuild.write / WorldMessageBuild.write: super + write.str(text).
    encode_simple_wall_sync(output, tile)?;
    output.write_utf(tile.message.as_deref().unwrap_or(""))?;
    Ok(())
}

pub fn encode_switch_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // SwitchBuild.write: the enabled byte in the base AND a bool after it
    // (SwitchBlock.java:93-97, writeBase uses the same `enabled` field).
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(u8::from(tile.enabled))?;
    output.write_b(8)?; // moduleBitmask: 1<<3 always-on
    output.write_b(255)?;
    output.write_b(255)?;
    output.write_bool(tile.enabled)?;
    Ok(())
}

/// BaseShieldBuild (shield-projector 255, large-shield-projector 256) does
/// NOT override Building.write, so its sync is exactly the base with a power
/// module (hasPower = true, consumePower(5f)). Without this codec the blocks
/// were invisible in the client.
pub fn encode_shield_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // BaseShieldBuild (255 shield-projector, 256 large-shield-projector)
    // overrides write(): base + PowerModule + efficiency bytes, then
    // `f smoothRadius` + `bool broken` (BaseShield.java write()).
    // Missing those 5 bytes makes the official client consume the next
    // snapshot's bytes as smoothRadius/broken, corrupting the batch
    // ("Block ID mismatch"/"Missing entity", snapshots dropped).
    let efficiency = power
        .get(&tile.position)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?; // version
    output.write_b(1)?; // enabled
                        // moduleBitmask(): power (2) | 1<<3 always-on old-consume bit (8).
    output.write_b(2 | 8)?;
    // PowerModule.write: s links.size, i*links, f status.
    output.write_s(0)?;
    output.write_f(efficiency)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    // BaseShieldBuild.write tail: smoothRadius converges to radius*efficiency.
    let radius = if tile.block == 256 { 400.0 } else { 200.0 };
    output.write_f(radius * efficiency)?;
    output.write_bool(false)?; // broken
    Ok(())
}

pub fn encode_memory_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // MemoryBuild.write: i32 memory.length + f64 per cell. The official
    // MemoryBuild initializes `memory = new double[memoryCapacity]` (64 for
    // memory-cell, 512 for memory-bank) and always writes the full capacity,
    // so a lazy-initialized/empty tile must still emit the full array (zeros
    // for unwritten cells) or the client builds a 0-length memory.
    encode_simple_wall_sync(output, tile)?;
    let capacity = crate::network::economy::memory_capacity(tile.block).unwrap_or(0);
    output.write_i(i32::try_from(capacity).unwrap_or(i32::MAX))?;
    for index in 0..capacity {
        let value = tile.memory.get(index).copied().unwrap_or(0.0);
        output.write_l(value.to_bits() as i64)?;
    }
    Ok(())
}

pub fn encode_canvas_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // CanvasBuild.write: i32 data.length + raw bytes.
    encode_simple_wall_sync(output, tile)?;
    output.write_i(0)?; // zero-length canvas; the client keeps its own buffer.
    Ok(())
}

pub fn encode_payload_conveyor_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(0)?;
    output.write_b(255)?;
    output.write_b(255)?;
    output.write_f(tile.payload_progress)?;
    output.write_f(tile.payload_rotation)?;
    if let Some(payload) = tile.payload.as_deref() {
        write_carried_payload(output, payload)?;
    } else {
        output.write_bool(false)?;
    }
    if matches!(tile.block, 399 | 401) {
        let sorted = (tile.config.len() == 4 && tile.config[0] == 5)
            .then(|| {
                (
                    tile.config[1],
                    i16::from_be_bytes([tile.config[2], tile.config[3]]),
                )
            })
            .filter(|(kind, id)| {
                (*kind == 1 && (0..418).contains(id)) || (*kind == 6 && (0..35).contains(id))
            });
        output.write_b(sorted.map_or(255, |(kind, _)| kind))?;
        output.write_s(sorted.map_or(-1, |(_, id)| id))?;
        output.write_b(
            u8::try_from(tile.stored_amount.saturating_sub(1)).unwrap_or(tile.rotation) % 4,
        )?;
    }
    Ok(())
}

pub fn is_pickup_payload_supported(block: i16) -> bool {
    is_block_snapshot_supported(block) || matches!(block, 216..=227 | 230..=238 | 240..=243)
}

pub fn build_payload_version(block: i16) -> u8 {
    match block {
        433 => 4,
        369 | 373 | 377..=383 | 386..=391 => 3,
        361..=365 | 367 | 368 | 370 | 371 | 374 => 2,
        193
        | 194
        | 311
        | 314..=316
        | 320..=322
        | 327
        | 328
        | 336
        | 337
        | 360
        | 366
        | 372
        | 399
        | 401..=403
        | 408
        | 409
        | 426
        | 427
        | 436 => 1,
        _ => 0,
    }
}

pub fn encode_payload_build_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: &DynamicWorld,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    if is_block_snapshot_supported(tile.block) {
        return encode_dynamic_tile_sync(output, tile, power, Some(world));
    }
    if matches!(tile.block, 216..=227 | 230..=238 | 240..=243) {
        return encode_simple_wall_sync(output, tile);
    }
    Err(Error::new(
        ErrorKind::InvalidInput,
        "unsupported BuildPayload block codec",
    ))
}

pub fn encode_simple_wall_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // Official moduleBitmask() always sets 1<<3 (8), even without modules.
    output.write_b(8)?;
    output.write_b(255)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_basic_modules_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    has_items: bool,
    has_power: bool,
    has_liquids: bool,
) -> std::io::Result<()> {
    encode_basic_modules_sync_with_items(
        output,
        tile,
        power,
        has_items,
        has_power,
        has_liquids,
        None,
    )
}

/// `encode_basic_modules_sync` with an explicit ItemModule source. Stack
/// conveyors (plastanium 259 / surge 279) keep their items in
/// `conveyor_items` (the official StackConveyorBuild items module IS the
/// stack), so their sync must serialize that queue instead of `inventory`.
pub fn encode_basic_modules_sync_with_items(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    has_items: bool,
    has_power: bool,
    has_liquids: bool,
    items_source: Option<&[(i16, i32)]>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = if has_power {
        power.get(&tile.position).copied().unwrap_or(0.0)
    } else {
        1.0
    }
    .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // Official moduleBitmask() always includes 1<<3 (old consume module).
    output.write_b(
        u8::from(has_items) | (u8::from(has_power) << 1) | (u8::from(has_liquids) << 2) | 8,
    )?;
    if has_items {
        let items: Vec<_> = items_source
            .map(|source| source.to_vec())
            .unwrap_or_else(|| {
                tile.inventory
                    .iter()
                    .copied()
                    .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
                    .collect()
            });
        output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
        for (item, amount) in items {
            output.write_s(item)?;
            output.write_i(amount)?;
        }
    }
    if has_power {
        output.write_s(0)?;
        output.write_f(efficiency)?;
    }
    if has_liquids {
        write_liquid_module(output, tile)?;
    }
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_drill_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let has_power = !matches!(tile.block, 325 | 326);
    encode_basic_modules_sync(output, tile, power, true, has_power, true)?;
    output.write_f(tile.production_progress.max(0.0))?;
    // DrillBuild.write warmup is not unit-interval: water boost drives it
    // toward liquidBoostIntensity (1.6, blast 1.8). Clamping to 1.0 made
    // every block snapshot snap the rotator/bar back down.
    output.write_f(tile.transport_progress.max(0.0))?;
    Ok(())
}

pub fn encode_separator_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    // SeparatorBuild.write: base (moduleBitmask includes 1<<3 always-on,
    // verified 0xf on the jar) + f progress + f warmup + i seed.
    encode_basic_modules_sync(output, tile, power, true, true, true)?;
    output.write_f(tile.production_progress.max(0.0))?;
    output.write_f(tile.transport_progress.clamp(0.0, 1.0))?;
    output.write_i(0)?; // seed (authoritative; deterministic per tile)
    Ok(())
}

pub fn encode_build_tower_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    encode_basic_modules_sync(output, tile, power, false, true, true)?;
    // BuildTurretBuild.write: rotation then TypeIO.writePlans (empty here).
    output.write_f(f32::from(tile.rotation) * 90.0)?;
    output.write_s(0)?; // empty TypeIO BuildPlan array
    Ok(())
}

pub fn encode_unit_cargo_loader_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    encode_basic_modules_sync(output, tile, power, true, true, true)?;
    output.write_i(-1)?; // no tethered cargo unit in a newly constructed payload
    Ok(())
}

pub fn encode_campaign_pad_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    if tile.block == 426 {
        encode_basic_modules_sync(output, tile, power, true, true, true)?;
        output.write_f(tile.production_progress.max(0.0))?;
    } else {
        encode_basic_modules_sync(output, tile, power, true, false, true)?;
        output.write_s(-1)?; // configured item
        output.write_i(0)?; // priority
        output.write_f(0.0)?; // cooldown
        output.write_s(-1)?; // arriving item
        output.write_f(0.0)?; // arriving timer
        output.write_f(0.0)?; // liquid removed
    }
    Ok(())
}

/// Validates a LogicBlock config: a zlib stream decoding to exactly
/// [byte version=1, int sourceLen, source, int linkCount, per link:
/// (u16 nameLen, name, s x, s y)] — the `LogicBlock.compress` output the
/// official client sends in TileConfig for micro/logic/hyper processors
/// (LogicBlock.java:139-164). `pub` so the snapshot parity integration test
/// can build and verify round trips.
pub fn valid_logic_config(config: &[u8]) -> bool {
    building_config::valid_logic_payload(config)
}

pub fn encode_logic_processor_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    encode_basic_modules_sync(
        output,
        tile,
        &std::collections::HashMap::new(),
        false,
        false,
        tile.block == 433,
    )?;
    // LogicBuild.write serializes `i compressed.length + b compressed`, where
    // compressed is the client's TileConfig container (zlib of
    // version+source+links). Pass it through verbatim so the program the
    // client configured is what every late joiner sees; fall back to the
    // official empty-program container when no code is set.
    let program: Vec<u8> = if let Some(payload) = building_config::logic_payload(&tile.config) {
        payload.to_vec()
    } else {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&[1, 0, 0, 0, 0, 0, 0, 0, 0])?;
        encoder.finish()?
    };
    output.write_i(i32::try_from(program.len()).unwrap_or(0))?;
    output.extend_from_slice(&program);
    output.write_i(0)?; // variables
    output.write_i(0)?; // legacy memory
                        // Official LogicBuild.write() (158.1): ONLY privileged
                        // processors serialize instructionsPerTick between the
                        // memory count and the tag string (`if(privileged)
                        // write.s(ipt)`). Hyper (433) is not privileged in the
                        // content, so it must NOT write the field — writing it
                        // left 2 trailing bytes that VerifyProtocol158 rejects.
                        // World processor (442) is privileged and writes its
                        // configured 8 instructions per tick.
    if tile.block == 442 {
        output.write_s(8)?; // world-processor instructionsPerTick (official)
    }
    output.write_b(0)?; // nullable tag string
    output.write_s(0)?; // icon tag
    output.write_s(0)?; // wait instruction count
    output.write_f(0.0)?; // accumulator
    Ok(())
}

pub fn encode_logic_display_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    encode_simple_wall_sync(output, tile)?;
    if matches!(tile.block, 436..=438) {
        output.write_bool(false)?; // no transform matrix
    } else {
        output.write_i(0)?; // empty canvas data; the client retains its zero-filled buffer
    }
    Ok(())
}

pub fn encode_unit_factory_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power
        .get(&tile.position)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(11)?; // items(1)|power(2)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?; // power links
    output.write_f(efficiency)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?; // optional efficiency

    // PayloadBlockBuild.
    output.write_f(0.0)?;
    output.write_f(0.0)?;
    output.write_f(f32::from(tile.rotation) * 90.0)?;
    output.write_bool(false)?; // no completed payload yet

    // UnitFactoryBuild revision 3.
    output.write_f(tile.production_progress.max(0.0))?;
    let plan = unit_factory_plan(tile.block, &tile.config)
        .map(|(plan, _)| plan)
        .or(if matches!(tile.block, 386..=388) {
            Some(0)
        } else {
            None
        })
        .unwrap_or(-1);
    output.write_s(plan)?;
    output.write_f(f32::NAN)?;
    output.write_f(f32::NAN)?;
    output.write_b(configured_unit_command(tile).unwrap_or(255))?;
    Ok(())
}

pub fn encode_reconstructor_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let (requirements, liquid_rate, has_liquid_module) =
        if let Some(recipe) = reconstructor_recipe(tile.block) {
            (
                recipe.items,
                recipe.liquid_rate,
                matches!(tile.block, 382 | 383 | 389..=392),
            )
        } else {
            match tile.block {
                389 => (&[(9, 40), (17, 30)][..], 3.0 / 60.0, true),
                390 => (&[(9, 60), (17, 40)][..], 3.0 / 60.0, true),
                391 => (&[(9, 50), (17, 40)][..], 3.0 / 60.0, true),
                392 => (&[(7, 80), (9, 100)][..], 10.0 / 60.0, true),
                _ => return Err(Error::new(ErrorKind::InvalidInput, "not a reconstructor")),
            }
        };
    let has_items = requirements
        .iter()
        .all(|(item, amount)| inventory_count(&tile.inventory, *item) >= *amount);
    let required_liquid = if matches!(tile.block, 389..=391) {
        5
    } else if tile.block == 392 {
        9
    } else {
        3
    };
    let has_liquid = liquid_rate <= 0.0
        || (tile.stored_liquid == required_liquid && tile.liquid_amount > 0.0001);
    let efficiency = power.get(&tile.position).copied().unwrap_or(0.0)
        * f32::from(tile.stored_amount > 0 && has_items && has_liquid);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // moduleBitmask() official: items(1) | power(2) | liquids(4) | 1<<3 (8).
    output.write_b(if has_liquid_module { 15 } else { 11 })?;
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?;
    output.write_f(efficiency)?;
    if has_liquid_module {
        write_liquid_module(output, tile)?;
    }
    // 158.1 ReconstructorBuild.write (verified via javap on desktop.jar):
    // base + modules + b eff*255 + b optEff*255, then PayloadBlockBuild
    // (f payVector.x, f payVector.y, f payRotation, Payload.write) and then
    // ReconstructorBuild (f progress, TypeIO.writeVecNullable(commandPos),
    // TypeIO.writeCommand(command)). writeVecNullable emits two floats (NaN
    // when null); writeCommand emits b 255 when null, else the command id.
    // The optionalEfficiency byte must be the real value, NOT the command id
    // (that slot previously leaked the command, so the client always decoded
    // command=null and a ~1% optionalEfficiency).
    output.write_b((efficiency.clamp(0.0, 1.0) * 255.0) as u8)?;
    output.write_b(255)?; // optionalEfficiency: nominal 1.0
    output.write_f(0.0)?; // payVector.x
    output.write_f(0.0)?; // payVector.y
    output.write_f(f32::from(tile.rotation) * 90.0)?; // payRotation
    output.write_bool(false)?; // Payload.write(null) -> b 0
    output.write_f(tile.production_progress.max(0.0))?; // progress
    output.write_f(f32::NAN)?; // commandPos.x (null)
    output.write_f(f32::NAN)?; // commandPos.y (null)
    output.write_b(configured_unit_command(tile).unwrap_or(255))?; // command
    Ok(())
}

pub fn is_simple_liquid_snapshot(block: i16) -> bool {
    matches!(block, 283..=292 | 300 | 301 | 329)
}

pub fn is_snapshot_item_turret(block: i16) -> bool {
    matches!(
        block,
        349 | 350 | 351 | 352 | 357 | 358 | 361..=365 | 367 | 368 | 370 | 371 | 374 | 375
    )
}

pub fn write_liquid_module(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let mut liquids = Vec::<(i16, f32)>::new();
    if tile.stored_liquid >= 0 && tile.liquid_amount > 0.0001 {
        liquids.push((tile.stored_liquid, tile.liquid_amount));
    }
    if tile.output_liquid_amount > 0.0001 {
        if let Some((_, amount)) = liquids.iter_mut().find(|(liquid, _)| *liquid == 3) {
            *amount += tile.output_liquid_amount;
        } else {
            liquids.push((3, tile.output_liquid_amount));
        }
    }
    output.write_s(i16::try_from(liquids.len()).unwrap_or(i16::MAX))?;
    for (liquid, amount) in liquids {
        output.write_s(liquid)?;
        output.write_f(amount)?;
    }
    Ok(())
}

/// Writes only the compatibility current-liquid state. Reactor heat/warmup
/// also lives in `output_liquid_amount` for old saves, but it is a subclass
/// field and must never be duplicated into LiquidModule as cryofluid.
pub fn write_primary_liquid_module(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let present = tile.stored_liquid >= 0 && tile.liquid_amount > 0.0001;
    output.write_s(i16::from(present))?;
    if present {
        output.write_s(tile.stored_liquid)?;
        output.write_f(tile.liquid_amount)?;
    }
    Ok(())
}

pub fn dynamic_tile_health(tile: &DynamicTile) -> f32 {
    let maximum = crate::game::content::block_health(tile.block);
    if tile.health.is_finite() && tile.health > 0.0 {
        tile.health.min(maximum)
    } else {
        maximum
    }
}

pub fn encode_mender_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power.get(&tile.position).copied().unwrap_or(0.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(11)?; // items(1)|power(2)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?; // no manually configured power-node links
    output.write_f(efficiency)?;
    output.write_b((efficiency.clamp(0.0, 1.0) * 255.0) as u8)?;
    let optional = mender_spec(tile.block)
        .is_some_and(|spec| inventory_count(&tile.inventory, spec.booster_item) > 0);
    output.write_b(if optional { 255 } else { 0 })?;
    output.write_f(tile.transport_progress.clamp(0.0, 1.0))?; // heat
    output.write_f(tile.output_liquid_amount.clamp(0.0, 1.0))?; // phaseHeat
    Ok(())
}

pub fn encode_overdrive_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let spec = overdrive_spec(tile.block)
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "not an overdrive projector"))?;
    let has_items = spec
        .required_items
        .iter()
        .all(|(item, amount)| inventory_count(&tile.inventory, *item) >= *amount);
    let efficiency =
        power.get(&tile.position).copied().unwrap_or(0.0) * f32::from(spec.has_boost || has_items);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(11)?; // items(1)|power(2)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?;
    output.write_f(efficiency)?;
    output.write_b((efficiency.clamp(0.0, 1.0) * 255.0) as u8)?;
    output.write_b(if !spec.has_boost || has_items { 255 } else { 0 })?;
    output.write_f(tile.transport_progress.clamp(0.0, 1.0))?;
    output.write_f(tile.output_liquid_amount.clamp(0.0, 1.0))?;
    Ok(())
}

pub fn encode_force_projector_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power.get(&tile.position).copied().unwrap_or(0.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // ForceProjector: consumeItem(phaseFabric).boost() + consumePower(4f);
    // NO LiquidModule (verified: jar bitmask=11). Tail is
    // ForceProjectorBuild.write: bool broken + f buildup + f radscl +
    // f warmup + f phaseHeat.
    output.write_b(11)?; // items(1)|power(2)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?;
    output.write_f(efficiency)?;
    output.write_b((efficiency.clamp(0.0, 1.0) * 255.0) as u8)?;
    output.write_b(if inventory_count(&tile.inventory, 11) > 0 {
        255
    } else {
        0
    })?;
    output.write_bool(force_broken(tile))?;
    output.write_f(tile.production_progress.max(0.0))?; // buildup
    output.write_f(tile.transport_progress.clamp(0.0, 1.0))?; // radscl
    output.write_f(tile.ammo_units.clamp(0.0, 1.0))?; // warmup
    output.write_f(tile.output_liquid_amount.clamp(0.0, 1.0))?; // phaseHeat
    Ok(())
}

pub fn encode_regen_projector_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let has_hydrogen = tile.stored_liquid == HYDROGEN_LIQUID && tile.liquid_amount > 0.0001;
    let efficiency = power.get(&tile.position).copied().unwrap_or(0.0) * f32::from(has_hydrogen);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // RegenProjector: consumeItem(phaseFabric).boost() + consumePower(1f) +
    // consumeLiquid(hydrogen) — hydrogen is a ConsumeLiquid, NOT a
    // LiquidModule (verified: jar bitmask=11, 19 bytes, no LiquidModule).
    output.write_b(11)?; // items(1)|power(2)|1<<3(8)
    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_s(0)?;
    output.write_f(efficiency)?;
    output.write_b((efficiency.clamp(0.0, 1.0) * 255.0) as u8)?;
    output.write_b(if inventory_count(&tile.inventory, 11) > 0 {
        255
    } else {
        0
    })?;
    Ok(())
}

pub fn encode_shockwave_tower_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let has_cyanogen = tile.stored_liquid == CYANOGEN_LIQUID && tile.liquid_amount > 0.0001;
    let efficiency = power.get(&tile.position).copied().unwrap_or(0.0) * f32::from(has_cyanogen);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(6)?; // PowerModule | LiquidModule
    output.write_s(0)?;
    output.write_f(efficiency)?;
    if has_cyanogen {
        output.write_s(1)?;
        output.write_s(CYANOGEN_LIQUID)?;
        output.write_f(tile.liquid_amount)?;
    } else {
        output.write_s(0)?;
    }
    output.write_b((efficiency.clamp(0.0, 1.0) * 255.0) as u8)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_shock_mine_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(0)?; // no modules
    output.write_b(255)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_battery_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let capacity = power_role(tile.block)
        .map(|role| role.battery_capacity)
        .unwrap_or(1.0)
        .max(0.0001);
    let status = (tile.power_stored / capacity).clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(2)?; // PowerModule
    output.write_s(0)?; // no manually configured power-node links
    output.write_f(status)?;
    output.write_b(255)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_liquid_block_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let has_power = power_role(tile.block).is_some();
    let efficiency = if has_power {
        power.get(&tile.position).copied().unwrap_or(0.0)
    } else {
        1.0
    }
    .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(4 | (u8::from(has_power) << 1))?; // LiquidModule, optionally power
    if has_power {
        output.write_s(0)?;
        output.write_f(efficiency)?;
    }
    write_liquid_module(output, tile)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_liquid_bridge_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let range = if tile.block == 293 { 4 } else { 12 };
    let (width, height) =
        world.map_or((MAP_WIDTH, MAP_HEIGHT), |world| (world.width, world.height));
    let link = configured_link(tile, width, height).unwrap_or(-1);
    let valid_link = world.and_then(|world| valid_bridge_link(world, tile, range));
    let efficiency = if tile.block == 294 {
        power.get(&tile.position).copied().unwrap_or(0.0)
    } else {
        1.0
    }
    .clamp(0.0, 1.0);
    let incoming: Vec<i32> = world
        .map(|world| {
            let mut incoming: Vec<_> = world
                .tiles
                .iter()
                .filter(|other| {
                    other.block == tile.block
                        && other.team == tile.team
                        && valid_bridge_link(world, other, range) == Some(tile.position)
                })
                .map(|other| other.position)
                .take(i8::MAX as usize)
                .collect();
            incoming.sort_unstable();
            incoming
        })
        .unwrap_or_default();

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(if tile.block == 294 { 6 } else { 4 })?;
    if tile.block == 294 {
        output.write_s(0)?;
        output.write_f(efficiency)?;
    }
    write_liquid_module(output, tile)?;
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    output.write_i(link)?;
    output.write_f(if valid_link.is_some() && efficiency > 0.0 {
        1.0
    } else {
        0.0
    })?;
    output.write_b(i8::try_from(incoming.len()).unwrap_or(i8::MAX) as u8)?;
    for source in incoming {
        output.write_i(source)?;
    }
    output.write_bool(false)?; // wasMoved/moved is transient and resets every update.
    Ok(())
}

pub fn encode_item_logistics_sync(output: &mut Vec<u8>, tile: &DynamicTile) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let has_items = matches!(tile.block, 266..=270);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(u8::from(has_items))?;
    if has_items {
        if matches!(tile.block, 266 | 267) && tile.stored_item >= 0 && tile.stored_amount > 0 {
            output.write_s(1)?;
            output.write_s(tile.stored_item)?;
            output.write_i(tile.stored_amount.min(1))?;
        } else {
            output.write_s(0)?;
        }
    }
    output.write_b(255)?;
    output.write_b(255)?;

    if matches!(tile.block, 264 | 265 | 270) {
        output.write_s(configured_item(&tile.config).unwrap_or(-1))?;
    }
    Ok(())
}

pub fn encode_junction_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(0)?; // JunctionBuild has no ItemModule; it owns four directional buffers.
    output.write_b(255)?;
    output.write_b(255)?;

    let simulation_time = world
        .map(|world| *world.game_state.simulation_time.read())
        .unwrap_or(0.0);
    for direction in 0..4u8 {
        let entries: Vec<_> = tile
            .junction_items
            .iter()
            .filter(|(stored_direction, item, remaining)| {
                *stored_direction == direction
                    && (0..22).contains(item)
                    && remaining.is_finite()
                    && (0.0..=26.0).contains(remaining)
            })
            .take(6)
            .collect();
        output.write_b(u8::try_from(entries.len()).unwrap_or(6))?;
        output.write_b(6)?;
        for slot in 0..6 {
            let packed = entries.get(slot).map_or(0, |(_, item, remaining)| {
                let insertion_time = simulation_time - (26.0 - *remaining);
                (i64::from(insertion_time.to_bits()) << 16) | i64::from(*item as u16)
            });
            output.write_l(packed)?;
        }
    }
    Ok(())
}

pub fn encode_item_bridge_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let range = if tile.block == 262 { 4 } else { 12 };
    let (width, height) =
        world.map_or((MAP_WIDTH, MAP_HEIGHT), |world| (world.width, world.height));
    let link = configured_link(tile, width, height).unwrap_or(-1);
    let valid_link = world.and_then(|world| valid_bridge_link(world, tile, range));
    let efficiency = if tile.block == 263 {
        power.get(&tile.position).copied().unwrap_or(0.0)
    } else {
        1.0
    }
    .clamp(0.0, 1.0);
    let incoming: Vec<i32> = world
        .map(|world| {
            let mut incoming: Vec<_> = world
                .tiles
                .iter()
                .filter(|other| {
                    other.block == tile.block
                        && other.team == tile.team
                        && valid_bridge_link(world, other, range) == Some(tile.position)
                })
                .map(|other| other.position)
                .take(i8::MAX as usize)
                .collect();
            incoming.sort_unstable();
            incoming
        })
        .unwrap_or_default();

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // 158.1 module bitmask: 262 (bridge-conveyor) is a BufferedItemBridge
    // (items only -> 1 | 1<<3 = 9); 263 (phase-conveyor) is a plain ItemBridge
    // (items + power -> 1 | 2 | 1<<3 = 11). The 1<<3 "old consume" bit is
    // always present in BuildingComp.moduleBitmask().
    output.write_b(if tile.block == 263 { 11 } else { 9 })?;
    if tile.stored_item >= 0 && tile.stored_amount > 0 {
        output.write_s(1)?;
        output.write_s(tile.stored_item)?;
        output.write_i(tile.stored_amount)?;
    } else {
        output.write_s(0)?;
    }
    if tile.block == 263 {
        // PowerModule.write: s links.size, i*links, f status.
        output.write_s(0)?;
        output.write_f(efficiency)?;
    }
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;
    output.write_i(link)?;
    output.write_f(if valid_link.is_some() && efficiency > 0.0 {
        1.0
    } else {
        0.0
    })?;
    output.write_b(i8::try_from(incoming.len()).unwrap_or(i8::MAX) as u8)?;
    for source in incoming {
        output.write_i(source)?;
    }
    output.write_bool(false)?;
    if tile.block == 262 {
        // BufferedItemBridgeBuild.write() appends its ItemBuffer after the
        // base ItemBridgeBuild.write(): b index, b capacity (14), then
        // capacity longs. Each long is a TimeItem: (time << 32) | (item << 16)
        // | (data & 0xffff) with data=-1 for ordinary accepts. Without these
        // bytes the official client reads the next block snapshot as its
        // buffer, corrupting the whole stream (ArrayIndexOutOfBounds in
        // ItemBuffer.accept).
        let now = world
            .map(|w| *w.game_state.simulation_time.read())
            .unwrap_or(0.0);
        let items: Vec<(i16, f32)> = if tile.conveyor_items.is_empty() {
            if tile.stored_amount > 0 && tile.stored_item >= 0 {
                vec![(tile.stored_item, 1.0)]
            } else {
                Vec::new()
            }
        } else {
            tile.conveyor_items.clone()
        };
        let index = items.len().min(14);
        output.write_b(index as u8)?;
        output.write_b(14)?;
        for slot in 0..14 {
            if let Some((item, _)) = items.get(slot) {
                let time_bits = now.to_bits() as u64;
                let long = (time_bits << 32) | ((*item as u16 as u64) << 16) | 0xFFFF;
                output.write_l(long as i64)?;
            } else {
                output.write_l(0)?;
            }
        }
    }
    Ok(())
}

pub fn encode_turret_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let item_turret = is_snapshot_item_turret(tile.block);
    let liquid_turret = matches!(tile.block, 353 | 360 | 369);
    // Official 158.1 module flags (verified against server-release.jar):
    // item turrets carry ItemModule + LiquidModule (coolant), power turrets
    // carry PowerModule (+ LiquidModule for lancer/arc/meltdown). Foreshadow
    // is an ItemTurret that also consumes power + coolant (bitmask 7).
    let has_items = item_turret;
    let has_power = (!item_turret && !liquid_turret) || tile.block == 364;
    let has_liquids = item_turret || liquid_turret || matches!(tile.block, 354 | 355 | 366 | 373);
    let efficiency = if item_turret {
        f32::from(tile.ammo_units > 0.0)
    } else if liquid_turret {
        f32::from(tile.liquid_amount > 0.0001)
    } else {
        power.get(&tile.position).copied().unwrap_or(0.0)
    }
    .clamp(0.0, 1.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // moduleBitmask(): items(1)|power(2)|liquids(4)|1<<3(8) — the official
    // BuildingComp.moduleBitmask() always sets 1<<3 ("old consume module");
    // the client skips it on read, but byte parity requires it.
    output.write_b(
        u8::from(has_items) | (u8::from(has_power) << 1) | (u8::from(has_liquids) << 2) | 8,
    )?;
    if has_items {
        output.write_s(0)?; // ItemTurret keeps ammunition in its own AmmoEntry queue.
    }
    if has_power {
        output.write_s(0)?;
        output.write_f(efficiency)?;
    }
    if has_liquids {
        write_liquid_module(output, tile)?;
    }
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?;

    let reload_counter = if tile.block == 366 {
        power_turret_weapon(tile.block)
            .map(|weapon| (weapon.reload - tile.production_progress).clamp(0.0, weapon.reload))
            .unwrap_or(0.0)
    } else {
        tile.production_progress.max(0.0)
    };
    output.write_f(reload_counter)?;
    output.write_f(turret_snapshot_rotation(tile, world))?;
    if item_turret {
        if tile.stored_item >= 0 && tile.ammo_units >= 1.0 {
            output.write_b(1)?;
            output.write_s(tile.stored_item)?;
            output.write_s(
                tile.ammo_units
                    .round()
                    .clamp(1.0, turret_max_ammo(tile.block)) as i16,
            )?;
        } else {
            output.write_b(0)?;
        }
    }
    if matches!(tile.block, 369 | 373) {
        output.write_f(0.0)?; // ContinuousTurretBuild.lastLength
    }
    Ok(())
}

pub fn turret_snapshot_rotation(tile: &DynamicTile, world: Option<&DynamicWorld>) -> f32 {
    let weapon = turret_ammo(tile.block, tile.stored_item)
        .or_else(|| power_turret_weapon(tile.block))
        .or_else(|| liquid_turret_weapon(tile.block, tile.stored_liquid));
    let Some((world, weapon)) = world.zip(weapon) else {
        return 90.0;
    };
    let x = (tile.position >> 16) as i16 as f32 * 8.0;
    let y = tile.position as i16 as f32 * 8.0;
    if let Some(controller) = controlling_session_for_building(world, tile.position) {
        if player_team(world, &controller) == tile.team
            && controller.mouse_x.is_finite()
            && controller.mouse_y.is_finite()
            && (controller.mouse_x - x).hypot(controller.mouse_y - y) > 0.001
        {
            return (controller.mouse_y - y)
                .atan2(controller.mouse_x - x)
                .to_degrees();
        }
    }
    world
        .enemies
        .iter()
        .filter(|enemy| enemy.team != tile.team && turret_can_target(tile.block, enemy.unit_type))
        .filter_map(|enemy| {
            let distance = (enemy.x - x).hypot(enemy.y - y);
            (distance <= weapon.range).then_some((distance, enemy.x, enemy.y))
        })
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map_or(90.0, |(_, target_x, target_y)| {
            (target_y - y).atan2(target_x - x).to_degrees()
        })
}

pub fn encode_generic_crafter_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let has_power = power_role(tile.block).is_some();
    let has_liquids = liquid_capacity(tile.block).is_some();
    // Official moduleBitmask() always sets 1<<3 (8).
    let module_bits = 1u8 | (u8::from(has_power) << 1) | (u8::from(has_liquids) << 2) | 8;
    let efficiency = if has_power {
        power.get(&tile.position).copied().unwrap_or(0.0)
    } else {
        1.0
    }
    .clamp(0.0, 1.0);

    output.write_f(dynamic_tile_health(tile))?; // readBase clamps this to the official block health.
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?; // current Building base serialization revision without fog flags.
    output.write_b(1)?; // enabled
    output.write_b(module_bits)?;

    let items: Vec<_> = tile
        .inventory
        .iter()
        .copied()
        .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
        .collect();
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }

    if has_power {
        output.write_s(0)?; // no manually configured power-node links
        output.write_f(efficiency)?;
    }
    if has_liquids {
        write_liquid_module(output, tile)?;
    }
    output.write_b((efficiency * 255.0) as u8)?;
    output.write_b(255)?; // optional efficiency

    let craft_time = generic_crafter_time(tile.block).unwrap_or(1.0).max(0.0001);
    let progress = (tile.production_progress / craft_time).clamp(0.0, 1.0);
    output.write_f(progress)?;
    output.write_f(if progress > 0.0 { efficiency } else { 0.0 })?;
    // The legacy cultivator (330) writes an extra f32 after warmup.
    if tile.block == 330 {
        output.write_f(0.0)?;
    }
    // HeatProducerBuild extends GenericCrafterBuild and appends f32 heat.
    if matches!(tile.block, 202 | 203 | 204 | 205 | 215) {
        output.write_f(tile.output_liquid_amount.max(0.0))?;
    }
    Ok(())
}

pub fn encode_power_generator_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let has_items = matches!(tile.block, 308 | 310..=312 | 315 | 316);
    let has_liquids = matches!(tile.block, 310 | 311 | 315 | 316 | 320..=322);
    // Official moduleBitmask() always sets 1<<3 (8).
    let module_bits = 2u8 | u8::from(has_items) | (u8::from(has_liquids) << 2) | 8;
    let production_efficiency = if tile.block == 308 {
        if tile.production_progress > 0.0 {
            generator_fuel(tile.stored_item)
                .map(|fuel| fuel.0)
                .unwrap_or(0.0)
        } else {
            0.0
        }
    } else if tile.block == 315 {
        (inventory_count(&tile.inventory, reactor::THORIUM_ITEM).max(0) as f32
            / reactor::ITEM_CAPACITY as f32)
            .clamp(0.0, 1.0)
    } else if matches!(tile.block, 309..=312 | 316 | 320..=322) {
        // These generators run at nominal production in the simulation;
        // the official writes their real efficiency (visuals on the client).
        1.0
    } else {
        1.0
    };
    let generate_time = if tile.block == 308 {
        generator_fuel(tile.stored_item)
            .map(|(_, duration_multiplier)| {
                tile.production_progress / (120.0 * duration_multiplier)
            })
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    } else {
        0.0
    };

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(module_bits)?;
    if has_items {
        let items: Vec<_> = tile
            .inventory
            .iter()
            .copied()
            .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
            .collect();
        output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
        for (item, amount) in items {
            output.write_s(item)?;
            output.write_i(amount)?;
        }
    }
    output.write_s(0)?; // PowerModule links
    output.write_f(1.0)?; // producer graph status
    if has_liquids {
        if matches!(tile.block, 315 | 316) {
            write_primary_liquid_module(output, tile)?;
        } else {
            write_liquid_module(output, tile)?;
        }
    }
    output.write_b(255)?;
    output.write_b(255)?;
    output.write_f(production_efficiency)?;
    output.write_f(generate_time)?;
    if matches!(tile.block, 315 | 316) {
        output.write_f(tile.output_liquid_amount.max(0.0))?; // heat/warmup
    }
    Ok(())
}

pub fn encode_storage_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    core_items: Option<&[i32]>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    output.write_b(1)?; // ItemModule
    let items: Vec<_> = if let Some(core_items) = core_items {
        core_items
            .iter()
            .enumerate()
            .filter_map(|(item, amount)| {
                (*amount > 0).then_some((i16::try_from(item).unwrap_or(-1), *amount))
            })
            .collect()
    } else {
        tile.inventory
            .iter()
            .copied()
            .filter(|(item, amount)| (0..22).contains(item) && *amount > 0)
            .collect()
    };
    output.write_s(i16::try_from(items.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in items {
        output.write_s(item)?;
        output.write_i(amount)?;
    }
    output.write_b(255)?;
    output.write_b(255)?;
    Ok(())
}

pub fn encode_mass_driver_sync(
    output: &mut Vec<u8>,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    let efficiency = power.get(&tile.position).copied().unwrap_or(0.0);
    output.write_f(dynamic_tile_health(tile))?;
    output.write_b(tile.rotation | 128)?;
    output.write_b(tile.team)?;
    output.write_b(3)?;
    output.write_b(1)?;
    // 271 (mass driver) HAS an ItemModule (capacity 120) and a power
    // module: moduleBitmask = items(1) | power(2) | 1<<3 = 11. The legacy
    // bitmask 10 made the client skip the item module and misread link /
    // rotation / state — the launcher sprite never showed and the driver
    // "disconnected" visually (round 74).
    output.write_b(11)?;
    output.write_s(i16::try_from(tile.inventory.len()).unwrap_or(i16::MAX))?;
    for (item, amount) in &tile.inventory {
        output.write_s(*item)?;
        output.write_i(*amount)?;
    }
    output.write_s(0)?;
    output.write_f(efficiency)?;
    output.write_b((efficiency.clamp(0.0, 1.0) * 255.0) as u8)?;
    output.write_b(255)?;

    let link = world
        .and_then(|world| valid_mass_driver_link(world, tile))
        .or_else(|| {
            let (width, height) =
                world.map_or((MAP_WIDTH, MAP_HEIGHT), |world| (world.width, world.height));
            configured_link(tile, width, height)
        })
        .unwrap_or(-1);
    let state = world.map_or_else(
        || {
            if !tile.mass_driver_waiting.is_empty() {
                1
            } else if link >= 0 {
                2
            } else {
                0
            }
        },
        |world| mass_driver_state(world, tile),
    );
    output.write_i(link)?;
    output.write_f(tile.mass_driver_rotation)?;
    output.write_b(state)?;
    Ok(())
}

pub fn angle_between(from: i32, to: i32) -> f32 {
    let fx = (from >> 16) as i16 as f32;
    let fy = from as i16 as f32;
    let tx = (to >> 16) as i16 as f32;
    let ty = to as i16 as f32;
    (ty - fy).atan2(tx - fx).to_degrees()
}

pub fn broadcast_plan_snapshot_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    excluded: i32,
    team: u8,
    frame: Vec<u8>,
) {
    let mut recipients: Vec<(i32, u8)> = Vec::new();
    out.for_each_connection(&mut |connection_id| {
        if connection_id == excluded {
            return;
        }
        let unit_id = 2_000_000 + connection_id;
        let recipient_team = world
            .players
            .get(&unit_id)
            .map(|combat| combat.team)
            .or_else(|| {
                world.player_sessions.get(&unit_id).map(|session| {
                    world
                        .players
                        .get(&session.unit_id)
                        .map(|combat| combat.team)
                        .unwrap_or(1)
                })
            })
            .unwrap_or(1);
        recipients.push((connection_id, recipient_team));
    });
    for (connection_id, recipient_team) in recipients {
        if recipient_team == team {
            out.enqueue_to(connection_id, frame.clone(), false);
        }
    }
}
