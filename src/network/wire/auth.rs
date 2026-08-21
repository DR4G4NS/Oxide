//! Action-policy gates and unit-control admission. The listener adapter
//! re-exports these through crate::network::listener::*.

use crate::network::protocol::KICK_PACKET_ID;
use crate::network::protocol::SEND_MESSAGE_PACKET_ID;
use crate::network::wire::encode::frame_generated_packet;
use crate::network::wire::persistence::encode_typeio_string;
use crate::network::world::{DynamicWorld, SessionPlayer};

/// Official `Administration.allowAction` gate for a live player action
/// (P0-4): every player-triggered mutation passes through the registered
/// action filters with the authenticated actor identity (SOL-002). Server
/// actions (null player in Java) are always allowed.
pub(crate) fn actor_action_allowed(
    admin: &crate::state::administration::Administration,
    player: &SessionPlayer,
    action_type: crate::state::administration::ActionType,
    tile: Option<i32>,
    block: Option<i16>,
    unit: Option<i32>,
) -> bool {
    let mut action = crate::state::administration::PlayerAction::new(
        player.uuid.clone(),
        player.admin,
        action_type,
    );
    if let Some(tile) = tile {
        action.tile = Some(tile);
    }
    if let Some(block) = block {
        action.block = Some(block);
    }
    if let Some(unit) = unit {
        action.unit_id = Some(unit);
    }
    admin.allow_action(&action)
}

/// The acting player's team from their combat state (fallback: 1). PvP
/// ownership checks compare against this instead of the hardcoded team 1
/// (SOL-002): players may only configure/rotate/destroy their OWN team's
/// buildings.
pub(crate) fn player_team(world: &DynamicWorld, player: &SessionPlayer) -> u8 {
    // The live unit is authoritative during normal play. During a hot-host
    // swap (or the tiny respawn hand-off window), use the UUID-keyed profile
    // before falling back to the legacy non-PvP team 1. Never infer the team
    // from a packet field: the session identity is the only actor authority.
    world
        .players
        .get(&player.unit_id)
        .map(|combat| combat.team)
        .or_else(|| {
            world
                .player_profiles
                .get(&player.uuid)
                .map(|profile| profile.team)
        })
        .unwrap_or(1)
}

fn kick_message_frame(message: &str) -> std::io::Result<Vec<u8>> {
    let payload = encode_typeio_string(message)?;
    frame_generated_packet(KICK_PACKET_ID, &payload, false)
}

/// M11: after a rejected action, applies the official anti-spam aftermath
/// (`Administration.lambda$new$1` + `NetConnection.kick`): a 30 s kick
/// (frame + `handleKicked(uuid, ip, 30000)` + persist, returned as the
/// frame to write) or the 120 s warning message. Returns the kick frame for
/// the session to deliver and close the connection; None when only a
/// warning (already sent) or nothing.
pub(crate) fn rejected_action_aftermath(
    admin: &crate::state::administration::Administration,
    player: &SessionPlayer,
    out: &dyn crate::network::outbound::FrameEmit,
) -> Option<Vec<u8>> {
    if let Some(until) = admin.take_pending_kick(&player.uuid) {
        let ip = out.connection_ip(player.id - 1_000_000);
        let duration = until.saturating_duration_since(std::time::Instant::now());
        admin.handle_kicked(&player.uuid, &ip, duration);
        return kick_message_frame("You are interacting with too many blocks.").ok();
    }
    if admin.take_pending_warning(&player.uuid) {
        if let Ok(payload) =
            encode_typeio_string("[scarlet]You are interacting with blocks too quickly.")
        {
            if let Ok(frame) = frame_generated_packet(SEND_MESSAGE_PACKET_ID, &payload, false) {
                out.enqueue_to(player.id - 1_000_000, frame, false);
            }
        }
    }
    None
}

/// Gate + anti-spam aftermath for session RPCs. `Ok(true)` = allowed;
/// `Ok(false)` = denied (warning sent or nothing); `Err(frame)` = denied
/// AND the official 30 s anti-spam kick fired — the caller must write the
/// frame and terminate the session.
pub(crate) fn session_action_allowed(
    admin: &crate::state::administration::Administration,
    player: &SessionPlayer,
    out: &dyn crate::network::outbound::FrameEmit,
    action_type: crate::state::administration::ActionType,
    tile: Option<i32>,
    block: Option<i16>,
    unit: Option<i32>,
) -> Result<bool, Vec<u8>> {
    if actor_action_allowed(admin, player, action_type, tile, block, unit) {
        return Ok(true);
    }
    match rejected_action_aftermath(admin, player, out) {
        Some(frame) => Err(frame),
        None => Ok(false),
    }
}

/// Variant with the plan/unit/building lists populated (removePlanned,
/// commandUnits/commandBuilding contexts).
pub(crate) fn session_action_allowed_full(
    admin: &crate::state::administration::Administration,
    player: &SessionPlayer,
    out: &dyn crate::network::outbound::FrameEmit,
    action_type: crate::state::administration::ActionType,
    plans: &[i32],
    unit_ids: &[i32],
    building_positions: &[i32],
) -> Result<bool, Vec<u8>> {
    let mut action = crate::state::administration::PlayerAction::new(
        player.uuid.clone(),
        player.admin,
        action_type,
    );
    action.plans = plans.to_vec();
    action.unit_ids = unit_ids.to_vec();
    action.building_positions = building_positions.to_vec();
    if admin.allow_action(&action) {
        return Ok(true);
    }
    match rejected_action_aftermath(admin, player, out) {
        Some(frame) => Err(frame),
        None => Ok(false),
    }
}
