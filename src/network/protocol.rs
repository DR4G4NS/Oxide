//! Official 159.7 network protocol constants (packet ids).
//!
//! Verified against official Mindustry v8 Build 159.7 desktop JAR bytecode
//! and the Call.registerPackets generated registry.

pub(crate) const REGISTER_UDP: u8 = 3;
pub(crate) const REGISTER_TCP: u8 = 4;
pub(crate) const KEEP_ALIVE: u8 = 2;
pub(crate) const DISCOVER_HOST: u8 = 1;
pub(crate) const FRAMEWORK_PACKET_LEN: usize = 6;

/// Packet ID from the generated registry bundled with desktop build 159.7.
pub const CONNECT_CONFIRM_PACKET_ID: u8 = 33;

pub(crate) const ADMIN_REQUEST_PACKET_ID: u8 = 6;
pub(crate) const CLIENT_LOGIC_DATA_RELIABLE_PACKET_ID: u8 = 22;
pub(crate) const CLIENT_LOGIC_DATA_UNRELIABLE_PACKET_ID: u8 = 23;
pub(crate) const REQUEST_BUILD_PAYLOAD_PACKET_ID: u8 = 88;
pub(crate) const REQUEST_DROP_PAYLOAD_PACKET_ID: u8 = 90;
pub(crate) const REQUEST_UNIT_PAYLOAD_PACKET_ID: u8 = 92;
pub(crate) const REQUEST_ASSETS_PACKET_ID: u8 = 86;
pub(crate) const REQUEST_WORLD_PACKET_ID: u8 = 93;
pub(crate) const SERVER_BINARY_PACKET_RELIABLE_PACKET_ID: u8 = 100;
pub(crate) const SERVER_BINARY_PACKET_UNRELIABLE_PACKET_ID: u8 = 101;
pub(crate) const SERVER_PACKET_RELIABLE_PACKET_ID: u8 = 102;
pub(crate) const SERVER_PACKET_UNRELIABLE_PACKET_ID: u8 = 103;
pub(crate) const SET_PLAYER_TEAM_EDITOR_PACKET_ID: u8 = 116;
pub(crate) const TRACE_INFO_PACKET_ID: u8 = 141;
pub(crate) const SET_POSITION_PACKET_ID: u8 = 117;
pub(crate) const REMOVE_QUEUE_BLOCK_PACKET_ID: u8 = 83;
pub(crate) const CLIENT_SNAPSHOT_PACKET_ID: u8 = 28;
pub(crate) const COMMAND_BUILDING_PACKET_ID: u8 = 29;
pub(crate) const COMMAND_UNITS_PACKET_ID: u8 = 30;
pub(crate) const PING_PACKET_ID: u8 = 76;
pub(crate) const PING_RESPONSE_PACKET_ID: u8 = 78;
pub(crate) const PLAYER_DISCONNECT_PACKET_ID: u8 = 80;
pub(crate) const PLAYER_SPAWN_PACKET_ID: u8 = 81;
pub(crate) const SEND_CHAT_PACKET_ID: u8 = 97;
pub(crate) const SEND_MESSAGE_PACKET_ID: u8 = 98;
pub(crate) const DEBUG_STATUS_CLIENT_PACKET_ID: u8 = 39;
pub(crate) const DEBUG_STATUS_CLIENT_UNRELIABLE_PACKET_ID: u8 = 40;
pub(crate) const MENU_CHOOSE_PACKET_ID: u8 = 71;
pub(crate) const REQUEST_DEBUG_STATUS_PACKET_ID: u8 = 89;
pub(crate) const TEXT_INPUT_RESULT_PACKET_ID: u8 = 138;
pub(crate) const TILE_TAP_PACKET_ID: u8 = 140;
pub(crate) const SEND_MESSAGE_2_PACKET_ID: u8 = 99;
pub(crate) const SET_UNIT_COMMAND_PACKET_ID: u8 = 128;
pub(crate) const SET_UNIT_STANCE_PACKET_ID: u8 = 129;
pub(crate) const STATE_SNAPSHOT_PACKET_ID: u8 = 133;
pub(crate) const ENTITY_SNAPSHOT_PACKET_ID: u8 = 48;
pub(crate) const CREATE_BULLET_PACKET_ID: u8 = 36;
pub(crate) const UNIT_DEATH_PACKET_ID: u8 = 151;
pub(crate) const UNIT_DESPAWN_PACKET_ID: u8 = 152;
pub(crate) const UNIT_CLEAR_PACKET_ID: u8 = 149;
pub(crate) const UNIT_BUILDING_CONTROL_SELECT_PACKET_ID: u8 = 147;
pub(crate) const UNIT_SPAWN_PACKET_ID: u8 = 157;
pub(crate) const UNIT_ENTERED_PAYLOAD_PACKET_ID: u8 = 154;
pub(crate) const PICKED_UNIT_PAYLOAD_PACKET_ID: u8 = 75;
pub(crate) const PICKED_BUILD_PAYLOAD_PACKET_ID: u8 = 74;
pub(crate) const PAYLOAD_DROPPED_PACKET_ID: u8 = 73;
pub(crate) const CONSTRUCT_FINISH_PACKET_ID: u8 = 34;
pub(crate) const BEGIN_PLACE_PACKET_ID: u8 = 12;
pub(crate) const BEGIN_BREAK_PACKET_ID: u8 = 11;
pub(crate) const BLOCK_SNAPSHOT_PACKET_ID: u8 = 13;
pub(crate) const BUILD_DESTROYED_PACKET_ID: u8 = 14;
pub(crate) const BUILD_HEALTH_UPDATE_PACKET_ID: u8 = 15;
pub(crate) const DECONSTRUCT_FINISH_PACKET_ID: u8 = 41;
pub(crate) const REMOVE_TILE_PACKET_ID: u8 = 84;
pub(crate) const REQUEST_BLOCK_SNAPSHOT_PACKET_ID: u8 = 87;
pub(crate) const REQUEST_ITEM_PACKET_ID: u8 = 91;
pub(crate) const ROTATE_BLOCK_PACKET_ID: u8 = 95;
pub(crate) const TAKE_ITEMS_PACKET_ID: u8 = 135;
pub(crate) const TILE_CONFIG_PACKET_ID: u8 = 139;
pub(crate) const TRANSFER_INVENTORY_PACKET_ID: u8 = 142;
pub(crate) const TRANSFER_ITEM_TO_PACKET_ID: u8 = 144;
pub(crate) const KICK_PACKET_ID: u8 = 60;
pub(crate) const KICK_2_PACKET_ID: u8 = 61;
pub(crate) const GAME_OVER_PACKET_ID: u8 = 50;
pub(crate) const WORLD_DATA_BEGIN_PACKET_ID: u8 = 164;
pub(crate) const BUILDING_CONTROL_SELECT_PACKET_ID: u8 = 16;
pub(crate) const CLIENT_PLAN_SNAPSHOT_PACKET_ID: u8 = 26;
pub(crate) const CLIENT_PLAN_SNAPSHOT_RECEIVED_PACKET_ID: u8 = 27;
pub(crate) const DELETE_PLANS_PACKET_ID: u8 = 42;
pub(crate) const DROP_ITEM_PACKET_ID: u8 = 44;
pub(crate) const PING_LOCATION_PACKET_ID: u8 = 77;
pub(crate) const UNIT_CONTROL_PACKET_ID: u8 = 150;
pub(crate) const PLAYER_CLASS_ID: u8 = 12;
pub(crate) const ALPHA_CLASS_ID: u8 = 0;
pub(crate) const ALPHA_CONTENT_ID: i16 = 35;
pub(crate) const SPAWN_X: i16 = 40;
pub(crate) const SPAWN_Y: i16 = 100;
pub(crate) const MAP_WIDTH: i32 = 300;
pub(crate) const MAP_HEIGHT: i32 = 300;
pub(crate) const BUILD_RANGE: f32 = 220.0;
pub(crate) const FACTORY_COMMAND_MARKER: u8 = 254;

/// Packets annotated `unreliable = true` in desktop 159.7 that this server
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
