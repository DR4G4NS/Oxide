//! Client-to-server packet decoders.
//!
//! Extracted from `listener.rs` (P2 listener split): every `decode_*`
//! entry point that turns a client payload into the authoritative request
//! struct consumed by the session/authority layer. The listener re-exports
//! these so existing callers and integration tests keep working unchanged.

use crate::network::buildings::construction::dynamic_at;
use crate::network::codec::Reads;
use crate::network::combat::enemy::base_building_at;
use crate::network::economy::default_unit_command;
use crate::network::economy::payload::decode_constructor_recipe;
use crate::network::protocol::*;
use crate::network::wire::frame_generated_packet;
use crate::network::world::{
    BuildingCommand, DynamicWorld, PendingConnection, SessionPlayer, UnitOrder, UnitOrderTarget,
};
use std::io::{Error, ErrorKind};

#[derive(Debug, Clone)]
pub struct ClientSnapshot {
    pub snapshot_id: i32,
    pub unit_id: i32,
    pub dead: bool,
    pub x: f32,
    pub y: f32,
    pub mouse_x: f32,
    pub mouse_y: f32,
    pub rotation: f32,
    pub boosting: bool,
    pub shooting: bool,
    /// Official ClientSnapshot `isBuilding` → `unit.updateBuilding`.
    pub building: bool,
    pub plans: Vec<BuildPlan>,
    pub mining_position: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub breaking: bool,
    pub position: i32,
    pub block: i16,
    pub rotation: u8,
    pub config: Vec<u8>,
}

pub fn decode_client_snapshot(payload: &[u8]) -> std::io::Result<ClientSnapshot> {
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    let snapshot_id = input.read_i()?;
    let unit_id = input.read_i()?;
    let dead = input.read_bool()?;
    let x = input.read_f()?;
    let y = input.read_f()?;
    let mouse_x = input.read_f()?;
    let mouse_y = input.read_f()?;
    let rotation = input.read_f()?;
    let _base_rotation = input.read_f()?;
    let _velocity_x = input.read_f()?;
    let _velocity_y = input.read_f()?;
    let mining_tile = input.read_i()?;
    let boosting = input.read_bool()?;
    let shooting = input.read_bool()?;
    let _chatting = input.read_bool()?;
    let building = input.read_bool()?;
    let _selected_block = input.read_s()?;
    let _selected_rotation = input.read_i()?;
    let plan_count = input.read_i()?;
    if !(-1..=20).contains(&plan_count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid build plan count",
        ));
    }
    let mut plans = Vec::with_capacity(plan_count.max(0) as usize);
    for _ in 0..plan_count.max(0) {
        let breaking = input.read_b()? == 1;
        let position = input.read_i()?;
        if breaking {
            plans.push(BuildPlan {
                breaking,
                position,
                block: -1,
                rotation: 0,
                config: vec![0],
            });
        } else {
            let block = input.read_s()?;
            let rotation = input.read_b()?;
            let _has_config = input.read_b()?;
            let config = read_typeio_object_raw(&mut input)?;
            plans.push(BuildPlan {
                breaking,
                position,
                block,
                rotation,
                config,
            });
        }
    }
    let view_x = input.read_f()?;
    let view_y = input.read_f()?;
    let view_width = input.read_f()?;
    let view_height = input.read_f()?;

    if [
        x,
        y,
        mouse_x,
        mouse_y,
        rotation,
        view_x,
        view_y,
        view_width,
        view_height,
    ]
    .iter()
    .any(|value| !value.is_finite())
    {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "non-finite client snapshot",
        ));
    }
    Ok(ClientSnapshot {
        snapshot_id,
        unit_id,
        dead,
        x,
        y,
        mouse_x,
        mouse_y,
        rotation,
        boosting,
        shooting,
        building,
        plans,
        mining_position: (mining_tile != -1).then_some(mining_tile),
    })
}

pub fn read_typeio_object_raw(input: &mut std::io::Cursor<&[u8]>) -> std::io::Result<Vec<u8>> {
    let start = input.position() as usize;
    skip_typeio_object(input, 0)?;
    let end = input.position() as usize;
    Ok(input.get_ref()[start..end].to_vec())
}

pub fn decode_tile_config(payload: &[u8]) -> std::io::Result<Option<(i32, Vec<u8>)>> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let position = input.read_i()?;
    let config = read_typeio_object_raw(&mut input)?;
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in TileConfig packet",
        ));
    }
    Ok(Some((position, config)))
}

pub fn decode_command_building(payload: &[u8]) -> std::io::Result<(Vec<i32>, f32, f32)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let count = input.read_s()?;
    if !(0..=200).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid CommandBuilding building count",
        ));
    }
    let mut buildings = Vec::with_capacity(count as usize);
    for _ in 0..count {
        buildings.push(input.read_i()?);
    }
    let target_x = input.read_f()?;
    let target_y = input.read_f()?;
    if input.position() != payload.len() as u64 || !target_x.is_finite() || !target_y.is_finite() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid CommandBuilding payload",
        ));
    }
    Ok((buildings, target_x, target_y))
}

/// Applies CommandBuilding using the legacy survival/attack team (team 1).
///
/// Network callers must use [`apply_command_building_for_team`] so a PvP
/// actor cannot issue commands to another team's factories. Keeping this
/// wrapper avoids changing the unit-test and save-replay call sites that model
/// the non-PvP default team.
pub fn apply_command_building(
    world: &DynamicWorld,
    buildings: &[i32],
    target_x: f32,
    target_y: f32,
) -> bool {
    apply_command_building_for_team(world, 1, buildings, target_x, target_y)
}

/// Mirrors InputHandler.commandBuilding: only commandable buildings belonging
/// to the acting player's team may be changed. `dynamic_at` resolves a
/// footprint tile to its origin before the command is stored, matching the
/// Java `world.build(pos)` lookup.
pub fn apply_command_building_for_team(
    world: &DynamicWorld,
    actor_team: u8,
    buildings: &[i32],
    target_x: f32,
    target_y: f32,
) -> bool {
    let mut changed = false;
    for requested in buildings {
        let Some(tile) = dynamic_at(world, *requested)
            .filter(|tile| tile.team == actor_team && matches!(tile.block, 377..=383))
        else {
            continue;
        };
        let command = BuildingCommand {
            position: tile.position,
            target_x,
            target_y,
        };
        let same = world
            .building_commands
            .get(&tile.position)
            .is_some_and(|old| old.target_x == target_x && old.target_y == target_y);
        if !same {
            world.building_commands.insert(tile.position, command);
            changed = true;
        }
    }
    changed
}

pub fn encode_command_building_frame(
    player: &SessionPlayer,
    buildings: &[i32],
    target_x: f32,
    target_y: f32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(14 + buildings.len() * 4);
    payload.write_i(player.id)?;
    payload.write_s(i16::try_from(buildings.len()).map_err(|_| {
        Error::new(
            ErrorKind::InvalidInput,
            "too many CommandBuilding positions",
        )
    })?)?;
    for position in buildings {
        payload.write_i(*position)?;
    }
    payload.write_f(target_x)?;
    payload.write_f(target_y)?;
    frame_generated_packet(COMMAND_BUILDING_PACKET_ID, &payload, false)
}

#[derive(Debug, PartialEq)]
pub struct CommandUnitsRequest {
    pub unit_ids: Vec<i32>,
    pub build_target: i32,
    pub unit_target_type: u8,
    pub unit_target_id: i32,
    pub pos_x: f32,
    pub pos_y: f32,
    pub queue_command: bool,
    pub final_batch: bool,
}

pub fn decode_command_units(payload: &[u8]) -> std::io::Result<CommandUnitsRequest> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let count = input.read_s()?;
    if !(0..=200).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid CommandUnits unit count",
        ));
    }
    let mut unit_ids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        unit_ids.push(input.read_i()?);
    }
    let build_target = input.read_i()?;
    let unit_target_type = input.read_b()?;
    let unit_target_id = input.read_i()?;
    let pos_x = input.read_f()?;
    let pos_y = input.read_f()?;
    let queue_command = match input.read_b()? {
        0 => false,
        1 => true,
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid CommandUnits queue flag",
            ))
        }
    };
    let final_batch = match input.read_b()? {
        0 => false,
        1 => true,
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid CommandUnits final-batch flag",
            ))
        }
    };
    if input.position() != payload.len() as u64
        || unit_target_type > 2
        || !pos_x.is_finite()
        || !pos_y.is_finite()
    {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid CommandUnits payload",
        ));
    }
    Ok(CommandUnitsRequest {
        unit_ids,
        build_target,
        unit_target_type,
        unit_target_id,
        pos_x,
        pos_y,
        queue_command,
        final_batch,
    })
}

pub fn command_target(world: &DynamicWorld, request: &CommandUnitsRequest) -> UnitOrderTarget {
    if request.build_target != -1 {
        if let Some(tile) = dynamic_at(world, request.build_target) {
            return UnitOrderTarget {
                kind: 1,
                id: tile.position,
                x: (tile.position >> 16) as i16 as f32 * 8.0,
                y: tile.position as i16 as f32 * 8.0,
            };
        }
        if let Some(building) = base_building_at(world, request.build_target) {
            return UnitOrderTarget {
                kind: 1,
                id: building.position,
                x: (building.position >> 16) as i16 as f32 * 8.0,
                y: building.position as i16 as f32 * 8.0,
            };
        }
    }
    if request.unit_target_type == 2 {
        if let Some(unit) = world.enemies.get(&request.unit_target_id) {
            return UnitOrderTarget {
                kind: 2,
                id: unit.id,
                x: unit.x,
                y: unit.y,
            };
        }
    }
    UnitOrderTarget {
        kind: 0,
        id: -1,
        x: request.pos_x,
        y: request.pos_y,
    }
}

/// Applies CommandUnits for the legacy survival/attack team (team 1).
/// Network callers must use [`apply_command_units_for_team`] with the
/// authenticated player's combat-state team.
pub fn apply_command_units(world: &DynamicWorld, request: &CommandUnitsRequest) -> bool {
    apply_command_units_for_team(world, 1, request)
}

/// Mirrors InputHandler.commandUnits' ownership gate (`unit.team ==
/// player.team()`). The packet contains only unit IDs; the actor team is
/// supplied by the authenticated session and is never accepted from the
/// request payload.
///
/// Queued orders go through [`crate::network::units::queue_unit_target`]
/// (the `CommandAI.commandQueue` port: first-target promotion, cap 50,
/// queue dedup). Direct orders mirror InputHandler.java:346-354 exactly:
/// `ai.commandQueue.clear()` then `commandTarget`/`commandPosition` — the
/// queue is dropped, the target becomes ACTIVE and stances survive.
pub fn apply_command_units_for_team(
    world: &DynamicWorld,
    actor_team: u8,
    request: &CommandUnitsRequest,
) -> bool {
    use crate::network::units::{
        acquire_command_control, queue_unit_target, set_order_active_target,
        unit_command_ai_reachable, unit_order_has_active_rts_target,
    };

    let target = command_target(world, request);
    let mut changed = false;
    for unit_id in &request.unit_ids {
        let Some(unit) = world
            .enemies
            .get(unit_id)
            .filter(|unit| unit.team == actor_team)
        else {
            continue;
        };
        // P0-05: InputHandler gates every command on
        // `unit.controller() instanceof CommandAI` (InputHandler.java:333)
        // — a POSSESSED unit (Player controller) or a logic-bound unit
        // (LogicAI) is skipped entirely. The authority field is the
        // controller model.
        if !unit_command_ai_reachable(world, &unit) {
            continue;
        }
        let current_command = world
            .unit_orders
            .get(unit_id)
            .map(|order| order.command)
            .unwrap_or_else(|| default_unit_command(unit.unit_type));
        drop(unit);
        // InputHandler.java:334-337: commanding a unit implicitly orders it
        // to move ("if(ai.command == null || ai.command.switchToMove)
        // ai.command(moveCommand)"). `switchToMove` is false only for the
        // payload command family (UnitCommand.java: loadAll: enterPayload,
        // loadUnits, loadBlocks, unloadPayload, loopPayload — ids 5-9).
        let command = if matches!(current_command, 5..=9) {
            current_command
        } else {
            0
        };
        let became_active;
        {
            let mut order = world
                .unit_orders
                .entry(*unit_id)
                .or_insert_with(|| UnitOrder {
                    unit_id: *unit_id,
                    command,
                    stances: 0,
                    payload_cooldown: 0.0,
                    target_kind: 0,
                    target_id: -1,
                    target_x: None,
                    target_y: None,
                    logic_control: 0,
                    queue: Vec::new(),
                });
            changed |= order.command != command;
            order.command = command;
            if request.queue_command {
                // CommandAI.commandQueue(Position) — CommandAI.java:493-503.
                let had_active = unit_order_has_active_rts_target(&order);
                changed |= queue_unit_target(&mut order, target.clone());
                // Only a PROMOTED first target activates Command authority;
                // entries that merely sit in the queue never do.
                became_active = !had_active;
            } else {
                order.queue.clear();
                set_order_active_target(&mut order, target.clone());
                became_active = true;
                changed = true;
            }
        }
        if became_active {
            // P0-01: an ACTIVE RTS target gives the order Command authority
            // (`commandUnits` acts on CommandAI controllers only); logic
            // kinds can never reach this branch (command_target yields
            // kinds 0-2).
            acquire_command_control(world, *unit_id);
        }
    }
    changed
}

pub fn encode_command_units_frame(
    player: &SessionPlayer,
    client_payload: &[u8],
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4 + client_payload.len());
    payload.write_i(player.id)?;
    payload.extend_from_slice(client_payload);
    frame_generated_packet(COMMAND_UNITS_PACKET_ID, &payload, false)
}

pub fn decode_set_unit_command(payload: &[u8]) -> std::io::Result<(Vec<i32>, u8)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let count = input.read_s()?;
    if !(0..=200).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid SetUnitCommand unit count",
        ));
    }
    let mut unit_ids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        unit_ids.push(input.read_i()?);
    }
    let command = input.read_b()?;
    if input.position() != payload.len() as u64 || command > 9 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid SetUnitCommand command",
        ));
    }
    Ok((unit_ids, command))
}

/// P0-3: encodes `TraceInfoCallPacket` (134) for the `AdminAction.trace`
/// path: `TypeIO.writeEntity(player)` (i id) + `TypeIO.writeTraceInfo`:
/// writeString ip + writeString uuid + writeString locale + b modded +
/// b mobile + i timesJoined + i timesKicked + writeStrings(ips, 12) +
/// writeStrings(names, 12).
pub fn encode_trace_info_frame(
    session: &SessionPlayer,
    connection: &PendingConnection,
    admin: &crate::state::administration::Administration,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let mut payload = Vec::new();
    payload.write_i(session.id)?;
    let current_ip = connection.ip.to_string();
    payload.write_typeio_string(Some(&current_ip))?;
    payload.write_typeio_string(Some(&session.uuid))?;
    // M7: honest TraceInfo fields — the port tracks no locale/modded/mobile
    // or join/kick history, so empty/zero values are reported instead of
    // fabricating "en"/1/0. The IP history contains only IPs actually seen
    // for this player (the current connection), never the server's ban list.
    payload.write_typeio_string(Some(""))?; // locale (not tracked)
    payload.write_b(0)?; // modded (not tracked)
    payload.write_b(0)?; // mobile (not tracked)
    payload.write_i(0)?; // timesJoined (not tracked)
    payload.write_i(0)?; // timesKicked (not tracked)
    let ips: Vec<String> = if admin
        .connected_players_list()
        .iter()
        .any(|p| p.uuid == session.uuid)
    {
        vec![current_ip.clone()]
    } else {
        Vec::new()
    };
    let names: Vec<String> = vec![session.name.clone()];
    let ip_count = ips.len().min(12) as u8;
    payload.write_b(ip_count)?;
    for ip in ips.iter().take(ip_count as usize) {
        payload.write_typeio_string(Some(ip))?;
    }
    let name_count = names.len().min(12) as u8;
    payload.write_b(name_count)?;
    for name in names.iter().take(name_count as usize) {
        payload.write_typeio_string(Some(name))?;
    }
    frame_generated_packet(TRACE_INFO_PACKET_ID, &payload, false)
}

/// Validated TypeIO object carried by an AdminRequest `readObjectSafe`
/// payload (tags from `TypeIO.readObject`, desktop 158.1 offsets 0-1005).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminRequestParams {
    /// tag 0 (null object) — the vanilla client sends this for kick/ban/
    /// trace/wave.
    None,
    /// tag 20 + team ordinal (`Team.all[ub]`).
    Team(u8),
    /// Any other well-formed single object (parsed and consumed, then
    /// ignored by the action handlers like the JAR).
    Other,
}

/// P0-3: decodes `AdminRequestCallPacket.handled` (158.1):
/// `TypeIO.readEntity` (i player id) + `readAction` (b ordinal) + the raw
/// remaining `readObjectSafe` bytes (Team for switchTeam: `b 20 + b team`).
/// The action ordinal is validated against `AdminAction.all` (kick, ban,
/// trace, wave, switchTeam).
/// M7: the params MUST parse as exactly one complete TypeIO object —
/// `TypeIO.readObjectSafe` throws on malformed objects (unknown tags,
/// truncation, oversized arrays) and the official server closes the
/// connection; the port returns an error instead of acting on garbage.
pub fn decode_admin_request(payload: &[u8]) -> std::io::Result<(i32, u8, AdminRequestParams)> {
    use crate::network::codec::Reads;
    use std::io::Read as _;
    let mut input = std::io::Cursor::new(payload);
    let player_id = input.read_i()?;
    let action = input.read_b()?;
    if action > 4 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("AdminRequest action ordinal {action} out of range"),
        ));
    }
    let start = input.position() as usize;
    let _ = std::io::Read::read_to_end(&mut input, &mut Vec::new())?;
    let params = &payload[start..];
    let mut object_input = std::io::Cursor::new(params);
    skip_typeio_object(&mut object_input, 0)?;
    if object_input.position() != params.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "AdminRequest params must be exactly one TypeIO object",
        ));
    }
    let kind = match params.first() {
        Some(0) => AdminRequestParams::None,
        Some(20) if params.len() == 2 => AdminRequestParams::Team(params[1]),
        Some(_) => AdminRequestParams::Other,
        None => AdminRequestParams::None,
    };
    // Official switchTeam only acts when the object is a Team.
    if action == 4 && !matches!(kind, AdminRequestParams::Team(_)) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "AdminRequest switchTeam requires a Team param",
        ));
    }
    Ok((player_id, action, kind))
}

/// P0-3: decodes `ClientLogicData*CallPacket.handled`:
/// `TypeIO.readString` channel + `readObjectSafe` value bytes. The value is
/// consumed verbatim; vanilla 158.1 has no registered channel handlers (the
/// Java hook `logicClientDataHandlers` is a mod extension), so the packet is
/// a validated no-op on the official server too.
pub fn decode_client_logic_data(payload: &[u8]) -> std::io::Result<(String, Vec<u8>)> {
    use crate::network::codec::Reads;
    use std::io::Read as _;
    let mut input = std::io::Cursor::new(payload);
    let channel = input
        .read_typeio_string()?
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "null channel in ClientLogicData"))?;
    let mut value = Vec::new();
    std::io::Read::read_to_end(&mut input, &mut value)?;
    Ok((channel, value))
}

/// P0-3: decodes `RequestBuildPayloadCallPacket.handled`:
/// `TypeIO.readBuilding` = i packed position.
pub fn decode_request_build_payload(payload: &[u8]) -> std::io::Result<i32> {
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    let position = input.read_i()?;
    Ok(position)
}

/// P0-3: decodes `RequestDropPayloadCallPacket.handled`: f x + f y.
pub fn decode_request_drop_payload(payload: &[u8]) -> std::io::Result<(f32, f32)> {
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    let x = input.read_f()?;
    let y = input.read_f()?;
    if !x.is_finite() || !y.is_finite() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "non-finite coordinates in RequestDropPayload",
        ));
    }
    Ok((x, y))
}

/// P0-3: decodes `RequestUnitPayloadCallPacket.handled`:
/// `TypeIO.readUnit` = b type + i id (type 2 = standard unit).
pub fn decode_request_unit_payload(payload: &[u8]) -> std::io::Result<(u8, i32)> {
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    let unit_type = input.read_b()?;
    let id = input.read_i()?;
    Ok((unit_type, id))
}

/// P0-3: decodes `SetPlayerTeamEditorCallPacket.handled`: `TypeIO.readTeam`
/// = b team id.
pub fn decode_set_player_team_editor(payload: &[u8]) -> std::io::Result<u8> {
    use crate::network::codec::Reads;
    let mut input = std::io::Cursor::new(payload);
    let team = input.read_b()?;
    Ok(team)
}

/// P0-3: decodes `ServerPacket*CallPacket.handled`: two TypeIO strings
/// (type + contents). `ServerBinaryPacket*` uses a string + raw bytes.
pub fn decode_server_packet(payload: &[u8], binary: bool) -> std::io::Result<(String, Vec<u8>)> {
    use crate::network::codec::Reads;
    use std::io::Read as _;
    let mut input = std::io::Cursor::new(payload);
    let packet_type = input
        .read_typeio_string()?
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "null type in server packet"))?;
    let contents = if binary {
        let len = input.read_us()?;
        let mut buf = vec![0; len as usize];
        std::io::Read::read_exact(&mut input, &mut buf)?;
        buf
    } else {
        input
            .read_typeio_string()?
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "null contents in server packet"))?
            .into_bytes()
    };
    Ok((packet_type, contents))
}

pub fn unit_allows_command(unit_type: i16, command: u8) -> bool {
    match unit_type {
        5 | 8 => matches!(command, 0 | 2 | 3 | 5),
        6 | 7 => matches!(command, 0 | 2 | 3 | 4 | 5),
        20 => matches!(command, 0 | 4 | 5),
        21 => matches!(command, 0..=5),
        22 => matches!(command, 0..=9),
        23 => matches!(command, 0 | 1 | 2 | 3 | 5..=9),
        24 => matches!(command, 0 | 2 | 3 | 5..=9),
        0..=34 => matches!(command, 0 | 5),
        _ => false,
    }
}

pub fn command_resets_target(command: u8) -> bool {
    matches!(command, 1..=4)
}

/// Applies SetUnitCommand for the legacy team 1.
pub fn apply_set_unit_command(world: &DynamicWorld, unit_ids: &[i32], command: u8) -> bool {
    apply_set_unit_command_for_team(world, 1, unit_ids, command)
}

/// Mirrors InputHandler.setUnitCommand's team and commandability checks. The
/// request cannot choose an actor team; callers on the network derive it from
/// the authenticated player's combat state.
pub fn apply_set_unit_command_for_team(
    world: &DynamicWorld,
    actor_team: u8,
    unit_ids: &[i32],
    command: u8,
) -> bool {
    let mut changed = false;
    for unit_id in unit_ids {
        let Some(unit) = world
            .enemies
            .get(unit_id)
            .filter(|unit| unit.team == actor_team && unit_allows_command(unit.unit_type, command))
        else {
            continue;
        };
        // P0-05: setUnitCommand also gates on
        // `unit.controller() instanceof CommandAI` (InputHandler.java:422) —
        // possessed and logic-bound units are skipped.
        if !crate::network::units::unit_command_ai_reachable(world, &unit) {
            continue;
        }
        let current = world
            .unit_orders
            .get(unit_id)
            .map(|order| order.command)
            .unwrap_or_else(|| default_unit_command(unit.unit_type));
        let unit_type = unit.unit_type;
        drop(unit);
        if current == command {
            continue;
        }
        let mut reset_target = false;
        if let Some(mut order) = world.unit_orders.get_mut(unit_id) {
            if command_resets_target(command) || command_resets_target(current) {
                // setUnitCommand's target reset (InputHandler.java:423-428):
                // `ai.targetPos = null; ai.attackTarget = null` plus the
                // queue clear that follows a command switch in the official
                // controller churn.
                crate::network::units::clear_order_active_target(&mut order);
                order.queue.clear();
                reset_target = true;
            }
            order.command = command;
            order.stances &= (0..30_u8)
                .filter(|stance| unit_allows_stance(unit_type, command, *stance))
                .fold(0_u32, |mask, stance| mask | (1_u32 << stance));
        } else {
            world.unit_orders.insert(
                *unit_id,
                UnitOrder {
                    unit_id: *unit_id,
                    command,
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
        }
        if reset_target {
            // P0-01: the active RTS target is gone; Command authority
            // returns to the team default.
            crate::network::units::release_command_control(world, *unit_id);
        }
        changed = true;
    }
    changed
}

pub fn encode_set_unit_command_frame(
    player: &SessionPlayer,
    client_payload: &[u8],
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4 + client_payload.len());
    payload.write_i(player.id)?;
    payload.extend_from_slice(client_payload);
    frame_generated_packet(SET_UNIT_COMMAND_PACKET_ID, &payload, false)
}

pub fn decode_set_unit_stance(payload: &[u8]) -> std::io::Result<(Vec<i32>, u8, bool)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let count = input.read_s()?;
    if !(0..=200).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid SetUnitStance unit count",
        ));
    }
    let mut unit_ids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        unit_ids.push(input.read_i()?);
    }
    let stance = input.read_b()?;
    let enable = match input.read_b()? {
        0 => false,
        1 => true,
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid SetUnitStance enable flag",
            ))
        }
    };
    if input.position() != payload.len() as u64 || stance > 29 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid SetUnitStance payload",
        ));
    }
    Ok((unit_ids, stance, enable))
}

pub fn unit_allows_stance(unit_type: i16, command: u8, stance: u8) -> bool {
    if stance == 0 {
        return true;
    }
    if command == 4 {
        return (7..=29).contains(&stance);
    }
    if stance == 6 {
        return matches!(command, 1..=3);
    }
    if stance == 3 && matches!(command, 1..=3) {
        return false;
    }
    if stance == 5 && matches!(command, 1..=3 | 5) {
        return false;
    }
    match unit_type {
        5..=8 => (1..=5).contains(&stance),
        15..=19 | 21..=23 => (1..=3).contains(&stance),
        20 | 24 => stance == 3,
        0..=14 | 25..=34 => (1..=4).contains(&stance),
        _ => false,
    }
}

/// Applies SetUnitStance for the legacy team 1.
pub fn apply_set_unit_stance(
    world: &DynamicWorld,
    unit_ids: &[i32],
    stance: u8,
    enable: bool,
) -> bool {
    apply_set_unit_stance_for_team(world, 1, unit_ids, stance, enable)
}

/// Mirrors InputHandler.setUnitStance's team gate and stance capability check.
/// A team-5 actor can only alter team-5 AI orders; a mixed packet simply
/// leaves foreign units untouched, as the Java loop does.
pub fn apply_set_unit_stance_for_team(
    world: &DynamicWorld,
    actor_team: u8,
    unit_ids: &[i32],
    stance: u8,
    enable: bool,
) -> bool {
    let mut changed = false;
    for unit_id in unit_ids {
        let Some(unit) = world
            .enemies
            .get(unit_id)
            .filter(|unit| unit.team == actor_team)
        else {
            continue;
        };
        // P0-05: setUnitStance gates on
        // `unit.controller() instanceof CommandAI` (InputHandler.java:456) —
        // possessed and logic-bound units are skipped.
        if !crate::network::units::unit_command_ai_reachable(world, &unit) {
            continue;
        }
        let command = world
            .unit_orders
            .get(unit_id)
            .map(|order| order.command)
            .unwrap_or_else(|| default_unit_command(unit.unit_type));
        if !unit_allows_stance(unit.unit_type, command, stance) {
            continue;
        }
        drop(unit);
        if stance == 0 {
            if let Some(mut order) = world.unit_orders.get_mut(unit_id) {
                // UnitStance.stop cancels orders: `ai.clearCommands()`
                // (InputHandler.java:457-458, CommandAI.java:177-181).
                crate::network::units::clear_order_active_target(&mut order);
                order.queue.clear();
            }
            if let Some(mut unit) = world.enemies.get_mut(unit_id) {
                unit.velocity_x = 0.0;
                unit.velocity_y = 0.0;
            }
            // P0-01: stop clears the command (`clearCommands`); Command
            // authority returns to the team default.
            crate::network::units::release_command_control(world, *unit_id);
            changed = true;
            continue;
        }
        let mut order = world.unit_orders.entry(*unit_id).or_insert(UnitOrder {
            unit_id: *unit_id,
            command,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: None,
            target_y: None,
            logic_control: 0,
            queue: Vec::new(),
        });
        let before = order.stances;
        if stance == 7 {
            order.stances &= !(((1_u32 << 30) - 1) & !((1_u32 << 7) - 1));
            order.stances |= 1_u32 << 7;
            if let Some(mut unit) = world.enemies.get_mut(unit_id) {
                if unit.secondary_attack_reload <= 0.0 {
                    unit.tertiary_attack_reload = 0.0;
                }
            }
        } else if stance >= 8 {
            order.stances &= !(((1_u32 << 30) - 1) & !((1_u32 << 7) - 1));
            order.stances |= 1_u32 << stance;
            if let Some(mut unit) = world.enemies.get_mut(unit_id) {
                unit.tertiary_attack_reload = f32::from(stance - 8 + 1);
            }
        } else if enable {
            order.stances |= 1_u32 << stance;
        } else {
            order.stances &= !(1_u32 << stance);
        }
        changed |= before != order.stances;
    }
    changed
}

pub fn encode_set_unit_stance_frame(
    player: &SessionPlayer,
    client_payload: &[u8],
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4 + client_payload.len());
    payload.write_i(player.id)?;
    payload.extend_from_slice(client_payload);
    frame_generated_packet(SET_UNIT_STANCE_PACKET_ID, &payload, false)
}

pub fn decode_rotate_block(payload: &[u8]) -> std::io::Result<(i32, bool)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let position = input.read_i()?;
    let direction = match input.read_b()? {
        0 => false,
        1 => true,
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid RotateBlock direction",
            ))
        }
    };
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in RotateBlock packet",
        ));
    }
    Ok((position, direction))
}

pub fn decode_drop_item(payload: &[u8]) -> std::io::Result<f32> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let angle = input.read_f()?;
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in DropItem packet",
        ));
    }
    Ok(angle)
}

pub fn decode_delete_plans(payload: &[u8]) -> std::io::Result<Vec<i32>> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let count = input.read_s()?;
    if !(0..=200).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid DeletePlans count",
        ));
    }
    let mut positions = Vec::with_capacity(count as usize);
    for _ in 0..count {
        positions.push(input.read_i()?);
    }
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in DeletePlans packet",
        ));
    }
    Ok(positions)
}

pub fn decode_ping_location(payload: &[u8]) -> std::io::Result<(f32, f32, Option<String>)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let x = input.read_f()?;
    let y = input.read_f()?;
    let text = input.read_typeio_string()?;
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in PingLocation packet",
        ));
    }
    Ok((x, y, text))
}

pub fn decode_unit_control(payload: &[u8]) -> std::io::Result<(u8, i32)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let control_type = input.read_b()?;
    if control_type > 2 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid UnitControl type",
        ));
    }
    let id = input.read_i()?;
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in UnitControl packet",
        ));
    }
    Ok((control_type, id))
}

pub fn decode_building_control_select(payload: &[u8]) -> std::io::Result<i32> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let position = input.read_i()?;
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in BuildingControlSelect packet",
        ));
    }
    Ok(position)
}

/// Decodes a ClientPlanSnapshot: `i groupId` + `TypeIO.writeClientPlans`
/// (s count + per plan: us x, us y, us blockId, [b rotation if the block
/// rotates], TypeIO config object). Returns the group id and the RAW plan
/// bytes (after the group id) so the server can forward them verbatim as
/// ClientPlanSnapshotReceived (whose plan section is byte-identical).
pub fn decode_client_plan_snapshot(payload: &[u8]) -> std::io::Result<(i32, Vec<u8>)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let group_id = input.read_i()?;
    let plan_start = input.position() as usize;
    // B7: official TypeIO.readClientPlans throws "Too many plans" only
    // beyond 1000 (`s` count, JAR bytecode offsets 0-16); the old cap of
    // 100 disconnected clients during large plan bursts.
    let amount = input.read_s()?;
    if !(0..=1000).contains(&amount) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid ClientPlanSnapshot amount",
        ));
    }
    for _ in 0..amount {
        let _x = input.read_us()?;
        let _y = input.read_us()?;
        let block = input.read_us()?;
        if crate::game::content::block_placement(block as i16).rotate {
            input.read_b()?;
        }
        read_typeio_object_raw(&mut input)?;
    }
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "trailing bytes in ClientPlanSnapshot packet",
        ));
    }
    let plans_raw = payload[plan_start..].to_vec();
    Ok((group_id, plans_raw))
}

/// Builds the S2C forward for DeletePlans: `i player_id + s count + i pos[]`.
pub fn encode_delete_plans_forward(player_id: i32, positions: &[i32]) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4 + 2 + positions.len() * 4);
    payload.write_i(player_id)?;
    payload.write_s(i16::try_from(positions.len()).unwrap_or(0))?;
    for position in positions {
        payload.write_i(*position)?;
    }
    frame_generated_packet(DELETE_PLANS_PACKET_ID, &payload, false)
}

/// Builds the S2C forward for PingLocation: `i player_id + f x + f y +
/// typeio_str text`.
pub fn encode_ping_location_forward(
    player_id: i32,
    x: f32,
    y: f32,
    text: Option<&str>,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4 + 8 + 4 + text.map_or(0, |s| s.len()));
    payload.write_i(player_id)?;
    payload.write_f(x)?;
    payload.write_f(y)?;
    payload.write_typeio_string(text)?;
    frame_generated_packet(PING_LOCATION_PACKET_ID, &payload, false)
}

/// Builds the S2C forward for UnitControl: `i player_id + b type + i id`.
pub fn encode_unit_control_forward(
    player_id: i32,
    control_type: u8,
    id: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4 + 1 + 4);
    payload.write_i(player_id)?;
    payload.write_b(control_type)?;
    payload.write_i(id)?;
    frame_generated_packet(UNIT_CONTROL_PACKET_ID, &payload, false)
}

/// Builds the S2C forward for ClientPlanSnapshotReceived:
/// `i player_id + i groupId + plan bytes` (verbatim from the sender).
pub fn encode_client_plan_snapshot_received(
    player_id: i32,
    group_id: i32,
    plans_raw: &[u8],
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4 + 4 + plans_raw.len());
    payload.write_i(player_id)?;
    payload.write_i(group_id)?;
    payload.extend_from_slice(plans_raw);
    frame_generated_packet(CLIENT_PLAN_SNAPSHOT_RECEIVED_PACKET_ID, &payload, false)
}

pub fn decode_building_reference(payload: &[u8], packet: &str) -> std::io::Result<i32> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let position = input.read_i()?;
    if input.position() != payload.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("trailing bytes in {packet} packet"),
        ));
    }
    Ok(position)
}

pub fn decode_request_item(payload: &[u8]) -> std::io::Result<(i32, i16, i32)> {
    use crate::network::codec::Reads;

    let mut input = std::io::Cursor::new(payload);
    let position = input.read_i()?;
    let item = input.read_s()?;
    let amount = input.read_i()?;
    if input.position() != payload.len() as u64
        || !(0..22).contains(&item)
        || !(1..=10_000).contains(&amount)
    {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid RequestItem packet",
        ));
    }
    Ok((position, item, amount))
}

pub fn decode_unit_command_config(config: &[u8]) -> Option<Option<u8>> {
    match config {
        [0] => Some(None),
        [23, high, low] => {
            let command = i16::from_be_bytes([*high, *low]);
            (0..=9)
                .contains(&command)
                .then(|| Some(u8::try_from(command).unwrap()))
        }
        _ => None,
    }
}

pub fn skip_typeio_object(input: &mut std::io::Cursor<&[u8]>, depth: usize) -> std::io::Result<()> {
    use crate::network::codec::Reads;
    if depth > 4 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "TypeIO object nesting is too deep",
        ));
    }
    let tag = input.read_b()?;
    let fixed = match tag {
        0 => 0,
        1 | 3 | 12 | 17 => 4,
        2 | 7 | 11 | 19 => 8,
        5 | 9 => 3,
        10 | 15 | 20 => 1,
        13 | 23 => 2,
        4 => {
            if input.read_b()? != 0 {
                let length = input.read_us()? as usize;
                skip_bytes(input, length)?;
            }
            return Ok(());
        }
        6 => {
            let length = input.read_s()?;
            if !(0..=1000).contains(&length) {
                return Err(Error::new(ErrorKind::InvalidData, "invalid IntSeq length"));
            }
            skip_bytes(input, length as usize * 4)?;
            return Ok(());
        }
        8 => {
            let length = input.read_b()? as usize;
            skip_bytes(input, length * 4)?;
            return Ok(());
        }
        14 => {
            let length = input.read_i()?;
            if !(0..=40_000).contains(&length) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid byte array length",
                ));
            }
            skip_bytes(input, length as usize)?;
            return Ok(());
        }
        16 => {
            let length = input.read_i()?;
            if !(0..=1000).contains(&length) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid bool array length",
                ));
            }
            skip_bytes(input, length as usize)?;
            return Ok(());
        }
        18 => {
            let length = input.read_s()?;
            if !(0..=1000).contains(&length) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid Vec2 array length",
                ));
            }
            skip_bytes(input, length as usize * 8)?;
            return Ok(());
        }
        21 => {
            let length = input.read_s()?;
            if !(0..=1000).contains(&length) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid int array length",
                ));
            }
            skip_bytes(input, length as usize * 4)?;
            return Ok(());
        }
        22 => {
            let length = input.read_i()?;
            if !(0..=1000).contains(&length) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid object array length",
                ));
            }
            for _ in 0..length {
                skip_typeio_object(input, depth + 1)?;
            }
            return Ok(());
        }
        _ => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "unknown TypeIO object tag",
            ))
        }
    };
    skip_bytes(input, fixed)
}

fn skip_bytes(input: &mut std::io::Cursor<&[u8]>, amount: usize) -> std::io::Result<()> {
    let next = (input.position() as usize)
        .checked_add(amount)
        .filter(|next| *next <= input.get_ref().len())
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated TypeIO object"))?;
    input.set_position(next as u64);
    Ok(())
}
