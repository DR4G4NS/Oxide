//! Block rotation / tile-config / factory-command wire application. The
//! listener adapter re-exports these through crate::network::listener::*.

use crate::network::world::{DynamicTile, DynamicWorld, SessionPlayer};

use crate::network::buildings::construction::dynamic_at;
use crate::network::buildings::{
    config as building_config, placement as building_placement, power as power_nodes,
};
use crate::network::decoders::decode_unit_command_config;
use crate::network::economy::payload::decode_constructor_recipe;
use crate::network::protocol::*;
use crate::network::wire::auth::player_team;
use crate::network::wire::encode::frame_generated_packet;

pub(crate) fn apply_rotate_block(
    player: &SessionPlayer,
    world: &DynamicWorld,
    requested_position: i32,
    direction: bool,
) -> Option<i32> {
    let tile = dynamic_at(world, requested_position)?;
    // Official InputHandler.rotateBlock: the server gate is team ownership
    // (Units.canInteract) + allowAction; there is NO server-side range check
    // (the client enforces its interact range). The legacy port added a
    // BUILD_RANGE gate that wrongly rejected legal rotations beyond it.
    if tile.block == 0
        || tile.team != player_team(world, player)
        || world
            .players
            .get(&player.unit_id)
            .is_some_and(|combat| combat.dead)
    {
        return None;
    }
    let mut live = world.tiles.get_mut(&tile.position)?;
    live.rotation = if direction {
        (live.rotation + 1) % 4
    } else {
        (live.rotation + 3) % 4
    };
    Some(tile.position)
}

pub(crate) fn encode_rotate_block_frame(
    player: &SessionPlayer,
    position: i32,
    direction: bool,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(9);
    payload.write_i(player.id)?;
    payload.write_i(position)?;
    payload.write_bool(direction)?;
    frame_generated_packet(ROTATE_BLOCK_PACKET_ID, &payload, false)
}

pub(crate) fn valid_tile_config(block: i16, config: &[u8]) -> bool {
    match block {
        // Unit factories accept either a plan index or the UnitType content itself.
        377..=379 => {
            unit_factory_plan(block, config).is_some()
                || decode_unit_command_config(config).is_some()
        }
        // Reconstructors are configured with a UnitCommand or null.
        380..=383 => {
            config == [0]
                || matches!(
                    config,
                    [23, high, low] if (0..32).contains(&i16::from_be_bytes([*high, *low]))
                )
        }
        399 | 401 => {
            config == [0]
                || matches!(config, [5, 1, high, low]
                    if (0..418).contains(&i16::from_be_bytes([*high, *low])))
                || matches!(config, [5, 6, high, low]
                    if (0..35).contains(&i16::from_be_bytes([*high, *low])))
        }
        406 | 407 => config == [0] || decode_constructor_recipe(block, config).is_some(),
        // Sorter, Inverted Sorter and Unloader accept an Item or null.
        264 | 265 | 270 | 274 | 278 | 280 | 282 => {
            config == [0]
                || matches!(
                    config,
                    [5, 0, high, low]
                        if (0..22).contains(&i16::from_be_bytes([*high, *low]))
                )
        }
        // Power nodes accept a TypeIO Integer toggle (tag 1, absolute packed
        // position), a single Point2 (tag 7) or a Point2[] link set (tag 8).
        // The official client sends tag 1 when the player taps a link target
        // (PowerNodeBuild.onConfigureBuildTapped -> configure(other.pos()),
        // PowerNode.java:419-428) and tag 8 when auto-linking or clearing
        // (PowerNode.java:429-438); TypeIO tags verified against
        // TypeIO.writeObject (TypeIO.java:45-80).
        302..=304 | 410 => power_nodes::valid_configuration(config),
        // Item/liquid bridges and Mass Driver accept an absolute integer or relative Point2.
        262 | 263 | 271 | 293 | 294 | 402 | 403 => {
            matches!(config, [1, _, _, _, _] | [7, _, _, _, _, _, _, _, _])
        }
        // SwitchBlock is configured with a TypeIO Boolean (tapped to toggle).
        430 => matches!(config, [1, 0] | [1, 1]),
        // Message blocks accept a TypeIO String (tag 4 + u16 length + utf8).
        429 | 441 => {
            matches!(config, [4, high, low, rest @ ..]
                if rest.len() == i16::from_be_bytes([*high, *low]) as usize)
        }
        // Logic processors accept a null config or the compressed program
        // container produced by LogicBlock.compress (zlib stream).
        431..=433 | 442 => building_config::valid_logic_object(config),
        // Sandbox source selection is a nullable typed Content object.
        412 => building_config::selected_item(config).is_some(),
        414 => building_config::selected_liquid(config).is_some(),
        _ => false,
    }
}

pub(crate) fn unit_factory_plan(block: i16, config: &[u8]) -> Option<(i16, i16)> {
    let units: &[i16] = match block {
        377 => &[0, 10, 5], // Dagger, Crawler, Nova
        378 => &[15, 20],   // Flare, Mono
        379 => &[25, 30],   // Risso, Retusa
        _ => return None,
    };
    let plan_config = config
        .iter()
        .position(|byte| *byte == FACTORY_COMMAND_MARKER)
        .map_or(config, |index| &config[..index]);
    let index = match plan_config {
        [1, a, b, c, d] => i32::from_be_bytes([*a, *b, *c, *d]),
        // TypeIO Content: tag, ContentType.unit, signed content ID.
        [5, 6, high, low] => {
            let unit = i16::from_be_bytes([*high, *low]);
            return units
                .iter()
                .position(|candidate| *candidate == unit)
                .and_then(|index| i16::try_from(index).ok())
                .map(|index| (index, unit));
        }
        _ => return None,
    };
    usize::try_from(index)
        .ok()
        .and_then(|index| units.get(index).copied().map(|unit| (index as i16, unit)))
}

pub(crate) fn configured_unit_command(tile: &DynamicTile) -> Option<u8> {
    if matches!(tile.block, 377..=379) {
        // P0-10: typed field is authoritative; the legacy `[254, command]`
        // suffix inside config is only read for tiles that predate the
        // separation (loaded from old checkpoints before migration).
        tile.factory_command.or_else(|| {
            tile.config
                .windows(2)
                .find_map(|bytes| (bytes[0] == FACTORY_COMMAND_MARKER).then_some(bytes[1]))
                .filter(|command| *command <= 9)
        })
    } else if matches!(tile.block, 380..=383) {
        decode_unit_command_config(&tile.config).flatten()
    } else {
        None
    }
}

pub(crate) fn apply_tile_config(
    player: &SessionPlayer,
    world: &DynamicWorld,
    requested_position: i32,
    config: &[u8],
) -> bool {
    let Some(tile) = dynamic_at(world, requested_position) else {
        return false;
    };
    // Official InputHandler.tileConfig: the server gate is team ownership
    // (Units.canInteract) + allowAction + valid config; there is NO
    // server-side range check (the client enforces its interact range).
    if tile.team != player_team(world, player)
        || !valid_tile_config(tile.block, config)
        || world
            .players
            .get(&player.unit_id)
            .is_some_and(|combat| combat.dead)
    {
        return false;
    }
    let power_node = power_nodes::is_power_node(tile.block);
    let Some(mut live) = world.tiles.get_mut(&tile.position) else {
        return false;
    };
    if matches!(live.block, 377..=379) {
        if let Some(command) = decode_unit_command_config(config) {
            // P0-10: the factory command lives in the typed field, never in
            // `config`; the legacy marker suffix (if any) is dropped here so
            // the next checkpoint serializes a clean TypeIO object.
            let previous = configured_unit_command(&live);
            if previous == command {
                return false;
            }
            let marker = live
                .config
                .iter()
                .position(|byte| *byte == FACTORY_COMMAND_MARKER);
            if let Some(index) = marker {
                live.config.truncate(index);
            }
            live.factory_command = command;
            return true;
        }
    }
    // A power-node toggle (Integer/Point2) is stateful: the SAME bytes toggle
    // the link back OFF, so the config-equality no-op check cannot apply
    // (PowerNode.java:60-75 Integer handler). Point2[] replacements are
    // idempotent and keep the check.
    let power_toggle = power_node && matches!(config, [1, ..] | [7, ..]);
    if !power_toggle && live.config == config {
        return false;
    }
    if matches!(live.block, 406 | 407) {
        live.production_progress = 0.0;
    }
    if live.block == 430 {
        if let [1, value] = config {
            live.enabled = *value != 0;
        }
    }
    if matches!(live.block, 429 | 441) {
        if let [4, high, low, rest @ ..] = config {
            let len = i16::from_be_bytes([*high, *low]) as usize;
            if rest.len() == len {
                live.message = Some(String::from_utf8_lossy(rest).into_owned());
            }
        }
    }
    let factory_command = configured_unit_command(&live);
    live.config.clear();
    live.config.extend_from_slice(config);
    if matches!(live.block, 377..=379) {
        // P0-10: the command is typed metadata; `config` stays a pure
        // TypeIO object so serializers never emit the private marker.
        live.factory_command = factory_command;
    }
    drop(live);
    if power_node {
        // The domain service performs both sides without retaining a tile
        // guard, so packet handling cannot reintroduce the old DashMap lock
        // inversion while adding/removing reverse links.
        power_nodes::apply_configuration(world, tile.position, config);
    }
    true
}

pub(crate) fn encode_tile_config_frame(
    player: &SessionPlayer,
    position: i32,
    config: &[u8],
) -> std::io::Result<Vec<u8>> {
    encode_tile_config_broadcast(player.id, position, config)
}

/// Server-initiated TileConfig (round 74f): the 158.1 client only
/// activates/merges power-node graphs through the PowerNode config handler
/// (Building.add never runs for update=false nodes), so autolink/sweep
/// changes must be pushed as a TileConfig broadcast — snapshots alone leave
/// the client-side node at "+0/s" and its linked machines unpowered.
pub(crate) fn encode_tile_config_broadcast(
    player_id: i32,
    position: i32,
    config: &[u8],
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(8 + config.len());
    payload.write_i(player_id)?; // server-to-client packets include the configuring player.
    payload.write_i(position)?;
    payload.extend_from_slice(config);
    frame_generated_packet(TILE_CONFIG_PACKET_ID, &payload, false)
}

/// Publishes the canonical configs produced by the shared placement lifecycle.
/// PowerNode links written only through BlockSnapshot do not cause the 158.1
/// client to merge/reflow its local PowerGraph.
pub(crate) fn broadcast_placement_power_configs(
    out: &dyn crate::network::outbound::FrameEmit,
    actor_id: i32,
    changes: &building_placement::PlacementChanges,
) -> std::io::Result<()> {
    for (position, config) in &changes.power_node_configs {
        out.broadcast(encode_tile_config_broadcast(actor_id, *position, config)?);
    }
    Ok(())
}
