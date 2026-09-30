//! Official 160.5 network protocol constants (packet ids).
//!
//! Verified against official Mindustry v8 Build 160.5 desktop JAR bytecode
//! and the Call.registerPackets generated registry.

pub(crate) const REGISTER_UDP: u8 = 3;
pub(crate) const REGISTER_TCP: u8 = 4;
pub(crate) const KEEP_ALIVE: u8 = 2;
pub(crate) const DISCOVER_HOST: u8 = 1;
pub(crate) const FRAMEWORK_PACKET_LEN: usize = 6;

/// Packet ID from the generated registry bundled with desktop build 160.5.
pub const CONNECT_CONFIRM_PACKET_ID: u8 = 34;

pub(crate) const ADMIN_REQUEST_PACKET_ID: u8 = 7;
pub(crate) const CLIENT_LOGIC_DATA_RELIABLE_PACKET_ID: u8 = 23;
pub(crate) const CLIENT_LOGIC_DATA_UNRELIABLE_PACKET_ID: u8 = 24;
pub(crate) const REQUEST_BUILD_PAYLOAD_PACKET_ID: u8 = 96;
pub(crate) const REQUEST_DROP_PAYLOAD_PACKET_ID: u8 = 98;
pub(crate) const REQUEST_UNIT_PAYLOAD_PACKET_ID: u8 = 100;
pub(crate) const REQUEST_ASSETS_PACKET_ID: u8 = 94;
pub(crate) const REQUEST_WORLD_PACKET_ID: u8 = 101;
pub(crate) const SERVER_BINARY_PACKET_RELIABLE_PACKET_ID: u8 = 108;
pub(crate) const SERVER_BINARY_PACKET_UNRELIABLE_PACKET_ID: u8 = 109;
pub(crate) const SERVER_PACKET_RELIABLE_PACKET_ID: u8 = 110;
pub(crate) const SERVER_PACKET_UNRELIABLE_PACKET_ID: u8 = 111;
pub(crate) const SET_PLAYER_TEAM_EDITOR_PACKET_ID: u8 = 124;
pub(crate) const TRACE_INFO_PACKET_ID: u8 = 149;
pub(crate) const SET_POSITION_PACKET_ID: u8 = 125;
pub(crate) const REMOVE_QUEUE_BLOCK_PACKET_ID: u8 = 91;
pub(crate) const CLIENT_SNAPSHOT_PACKET_ID: u8 = 29;
pub(crate) const COMMAND_BUILDING_PACKET_ID: u8 = 30;
pub(crate) const COMMAND_UNITS_PACKET_ID: u8 = 31;
pub(crate) const PING_PACKET_ID: u8 = 84;
pub(crate) const PING_RESPONSE_PACKET_ID: u8 = 86;
pub(crate) const PLAYER_DISCONNECT_PACKET_ID: u8 = 88;
pub(crate) const PLAYER_SPAWN_PACKET_ID: u8 = 89;
pub(crate) const SEND_CHAT_PACKET_ID: u8 = 105;
pub(crate) const SEND_MESSAGE_PACKET_ID: u8 = 106;
pub(crate) const DEBUG_STATUS_CLIENT_PACKET_ID: u8 = 40;
pub(crate) const DEBUG_STATUS_CLIENT_UNRELIABLE_PACKET_ID: u8 = 41;
pub(crate) const MENU_CHOOSE_PACKET_ID: u8 = 79;
pub(crate) const REQUEST_DEBUG_STATUS_PACKET_ID: u8 = 97;
pub(crate) const TEXT_INPUT_RESULT_PACKET_ID: u8 = 146;
pub(crate) const TILE_TAP_PACKET_ID: u8 = 148;
pub(crate) const SEND_MESSAGE_2_PACKET_ID: u8 = 107;
pub(crate) const SET_UNIT_COMMAND_PACKET_ID: u8 = 136;
pub(crate) const SET_UNIT_STANCE_PACKET_ID: u8 = 137;
pub(crate) const STATE_SNAPSHOT_PACKET_ID: u8 = 141;
pub(crate) const ENTITY_SNAPSHOT_PACKET_ID: u8 = 49;
pub(crate) const CREATE_BULLET_PACKET_ID: u8 = 37;
pub(crate) const UNIT_DEATH_PACKET_ID: u8 = 159;
pub(crate) const UNIT_DESPAWN_PACKET_ID: u8 = 160;
pub(crate) const UNIT_CLEAR_PACKET_ID: u8 = 157;
pub(crate) const UNIT_BLOCK_SPAWN_PACKET_ID: u8 = 154;
pub(crate) const UNIT_BUILDING_CONTROL_SELECT_PACKET_ID: u8 = 155;
pub(crate) const UNIT_SPAWN_PACKET_ID: u8 = 165;
pub(crate) const UNIT_ENTERED_PAYLOAD_PACKET_ID: u8 = 162;
pub(crate) const PICKED_UNIT_PAYLOAD_PACKET_ID: u8 = 83;
pub(crate) const PICKED_BUILD_PAYLOAD_PACKET_ID: u8 = 82;
pub(crate) const PAYLOAD_DROPPED_PACKET_ID: u8 = 81;
pub(crate) const CONSTRUCT_FINISH_PACKET_ID: u8 = 35;
pub(crate) const ASSEMBLER_DRONE_SPAWNED_PACKET_ID: u8 = 9;
pub(crate) const AUTO_DOOR_TOGGLE_PACKET_ID: u8 = 11;
pub(crate) const SET_RULES_PACKET_ID: u8 = 127;
pub(crate) const SET_FLAG_PACKET_ID: u8 = 113;
pub(crate) const SET_FLOOR_PACKET_ID: u8 = 114;
pub(crate) const SET_ITEM_PACKET_ID: u8 = 117;
pub(crate) const SET_ITEMS_PACKET_ID: u8 = 118;
pub(crate) const SET_LIQUID_PACKET_ID: u8 = 119;
pub(crate) const SET_LIQUIDS_PACKET_ID: u8 = 120;
pub(crate) const SET_MAP_AREA_PACKET_ID: u8 = 121;
pub(crate) const SET_OBJECTIVES_PACKET_ID: u8 = 122;
pub(crate) const SET_OVERLAY_PACKET_ID: u8 = 123;
pub(crate) const SET_TEAM_PACKET_ID: u8 = 128;
pub(crate) const SET_TEAMS_PACKET_ID: u8 = 129;
pub(crate) const SET_TILE_PACKET_ID: u8 = 130;
pub(crate) const SYNC_VARIABLE_PACKET_ID: u8 = 142;
pub(crate) const LOGIC_EXPLOSION_PACKET_ID: u8 = 74;
pub(crate) const CLEAR_ITEMS_PACKET_ID: u8 = 18;
pub(crate) const CLEAR_LIQUIDS_PACKET_ID: u8 = 19;
pub(crate) const CLEAR_OBJECTIVES_PACKET_ID: u8 = 20;
pub(crate) const COMPLETE_OBJECTIVE_PACKET_ID: u8 = 32;
pub(crate) const CREATE_WEATHER_PACKET_ID: u8 = 39;
pub(crate) const RESEARCHED_PACKET_ID: u8 = 102;
pub(crate) const SECTOR_CAPTURE_PACKET_ID: u8 = 104;
pub(crate) const UPDATE_GAME_OVER_PACKET_ID: u8 = 167;
pub(crate) const BEGIN_PLACE_PACKET_ID: u8 = 13;
pub(crate) const BEGIN_BREAK_PACKET_ID: u8 = 12;
pub(crate) const BLOCK_SNAPSHOT_PACKET_ID: u8 = 14;
pub(crate) const BUILD_DESTROYED_PACKET_ID: u8 = 15;
pub(crate) const BUILD_HEALTH_UPDATE_PACKET_ID: u8 = 16;
pub(crate) const DECONSTRUCT_FINISH_PACKET_ID: u8 = 42;
pub(crate) const REMOVE_TILE_PACKET_ID: u8 = 92;
pub(crate) const REQUEST_BLOCK_SNAPSHOT_PACKET_ID: u8 = 95;
pub(crate) const REQUEST_ITEM_PACKET_ID: u8 = 99;
pub(crate) const ROTATE_BLOCK_PACKET_ID: u8 = 103;
pub(crate) const TAKE_ITEMS_PACKET_ID: u8 = 143;
pub(crate) const TILE_CONFIG_PACKET_ID: u8 = 147;
pub(crate) const TRANSFER_INVENTORY_PACKET_ID: u8 = 150;
pub(crate) const TRANSFER_ITEM_TO_PACKET_ID: u8 = 152;
pub(crate) const KICK_PACKET_ID: u8 = 65;
pub(crate) const KICK_2_PACKET_ID: u8 = 66;
pub(crate) const GAME_OVER_PACKET_ID: u8 = 54;
pub(crate) const WORLD_DATA_BEGIN_PACKET_ID: u8 = 172;
pub(crate) const BUILDING_CONTROL_SELECT_PACKET_ID: u8 = 17;
pub(crate) const CLIENT_PLAN_SNAPSHOT_PACKET_ID: u8 = 27;
pub(crate) const CLIENT_PLAN_SNAPSHOT_RECEIVED_PACKET_ID: u8 = 28;
pub(crate) const DELETE_PLANS_PACKET_ID: u8 = 43;
pub(crate) const DROP_ITEM_PACKET_ID: u8 = 45;
pub(crate) const PING_LOCATION_PACKET_ID: u8 = 85;
pub(crate) const UNIT_CONTROL_PACKET_ID: u8 = 158;
pub(crate) const PLAYER_CLASS_ID: u8 = 12;
pub(crate) const ALPHA_CLASS_ID: u8 = 0;
pub(crate) const ALPHA_CONTENT_ID: i16 = 35;
/// `UnitEntityLegacyBeta` / `UnitEntityLegacyGamma` classIds (compat/160.5/entities.json).
pub(crate) const BETA_CLASS_ID: u8 = 30;
pub(crate) const GAMMA_CLASS_ID: u8 = 31;
pub(crate) const BETA_CONTENT_ID: i16 = 36;
pub(crate) const GAMMA_CONTENT_ID: i16 = 37;
/// `PayloadUnit` classId — Erekir core ships (evoke/incite/emanate).
pub(crate) const PAYLOAD_UNIT_CLASS_ID: u8 = 5;
pub(crate) const EVOKE_CONTENT_ID: i16 = 58;
pub(crate) const INCITE_CONTENT_ID: i16 = 59;
pub(crate) const EMANATE_CONTENT_ID: i16 = 60;

/// Player-core `writeSync` class + content ids. Alpha/beta/gamma share
/// `UnitEntity.writeSync`; Erekir core ships are `PayloadUnit` (class 5)
/// and insert an empty payload count after the weapon mounts.
pub(crate) fn core_unit_sync_ids(content_id: i16) -> (u8, i16) {
    match content_id {
        36 => (BETA_CLASS_ID, BETA_CONTENT_ID),
        37 => (GAMMA_CLASS_ID, GAMMA_CONTENT_ID),
        58 => (PAYLOAD_UNIT_CLASS_ID, EVOKE_CONTENT_ID),
        59 => (PAYLOAD_UNIT_CLASS_ID, INCITE_CONTENT_ID),
        60 => (PAYLOAD_UNIT_CLASS_ID, EMANATE_CONTENT_ID),
        _ => (ALPHA_CLASS_ID, ALPHA_CONTENT_ID),
    }
}
pub(crate) const SPAWN_X: i16 = 40;
pub(crate) const SPAWN_Y: i16 = 100;
pub(crate) const MAP_WIDTH: i32 = 300;
pub(crate) const MAP_HEIGHT: i32 = 300;
pub(crate) const BUILD_RANGE: f32 = 220.0;
pub(crate) const FACTORY_COMMAND_MARKER: u8 = 254;

/// Packets annotated `unreliable = true` in desktop 160.5 that this server
/// emits or accepts.
pub(crate) fn packet_unreliable(packet_id: u8) -> bool {
    matches!(
        packet_id,
        ENTITY_SNAPSHOT_PACKET_ID
            | STATE_SNAPSHOT_PACKET_ID
            | BLOCK_SNAPSHOT_PACKET_ID
            | CREATE_BULLET_PACKET_ID
            | UNIT_SPAWN_PACKET_ID
            | TAKE_ITEMS_PACKET_ID
            | TRANSFER_ITEM_TO_PACKET_ID
            | CLIENT_SNAPSHOT_PACKET_ID
            | CLIENT_PLAN_SNAPSHOT_PACKET_ID
            | REQUEST_BLOCK_SNAPSHOT_PACKET_ID
            | TILE_TAP_PACKET_ID
            | DEBUG_STATUS_CLIENT_UNRELIABLE_PACKET_ID
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_unit_sync_ids_match_legacy_classes() {
        assert_eq!(core_unit_sync_ids(35), (ALPHA_CLASS_ID, ALPHA_CONTENT_ID));
        assert_eq!(core_unit_sync_ids(36), (BETA_CLASS_ID, BETA_CONTENT_ID));
        assert_eq!(core_unit_sync_ids(37), (GAMMA_CLASS_ID, GAMMA_CONTENT_ID));
        assert_eq!(
            core_unit_sync_ids(58),
            (PAYLOAD_UNIT_CLASS_ID, EVOKE_CONTENT_ID)
        );
        assert_eq!(
            core_unit_sync_ids(59),
            (PAYLOAD_UNIT_CLASS_ID, INCITE_CONTENT_ID)
        );
        assert_eq!(
            core_unit_sync_ids(60),
            (PAYLOAD_UNIT_CLASS_ID, EMANATE_CONTENT_ID)
        );
    }
}
