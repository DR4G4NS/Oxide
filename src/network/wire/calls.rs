//! Official 159.7 generated Call packet bodies (audit M13–M16).
//! Layouts come from `compat/159.7/rpc.json` (TypeIO field order).

use crate::network::codec::Writes;
use crate::network::protocol::*;
use crate::network::wire::encode::frame_generated_packet;

pub(crate) fn encode_set_flag_frame(flag: &str, add: bool) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_typeio_string(Some(flag))?;
    payload.write_bool(add)?;
    frame_generated_packet(SET_FLAG_PACKET_ID, &payload, false)
}

pub(crate) fn encode_sync_variable_frame(
    building: i32,
    variable: i32,
    numeric: f64,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(building)?;
    payload.write_i(variable)?;
    // TypeIO.writeObject number: tag 4 (double) + f64, matching LVar numbers.
    payload.write_b(4)?;
    payload.write_d(numeric)?;
    frame_generated_packet(SYNC_VARIABLE_PACKET_ID, &payload, false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_logic_explosion_frame(
    team: u8,
    x: f32,
    y: f32,
    radius: f32,
    damage: f32,
    air: bool,
    ground: bool,
    pierce: bool,
    effect: bool,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_b(team)?;
    payload.write_f(x)?;
    payload.write_f(y)?;
    payload.write_f(radius)?;
    payload.write_f(damage)?;
    payload.write_bool(air)?;
    payload.write_bool(ground)?;
    payload.write_bool(pierce)?;
    payload.write_bool(effect)?;
    frame_generated_packet(LOGIC_EXPLOSION_PACKET_ID, &payload, false)
}

pub(crate) fn encode_set_map_area_frame(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(x)?;
    payload.write_i(y)?;
    payload.write_i(w)?;
    payload.write_i(h)?;
    frame_generated_packet(SET_MAP_AREA_PACKET_ID, &payload, false)
}

pub(crate) fn encode_set_item_frame(
    build: i32,
    item: i16,
    amount: i32,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(build)?;
    payload.write_s(item)?;
    payload.write_i(amount)?;
    frame_generated_packet(SET_ITEM_PACKET_ID, &payload, false)
}

pub(crate) fn encode_set_liquid_frame(
    build: i32,
    liquid: i16,
    amount: f32,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(build)?;
    payload.write_s(liquid)?;
    payload.write_f(amount)?;
    frame_generated_packet(SET_LIQUID_PACKET_ID, &payload, false)
}

pub(crate) fn encode_clear_items_frame(build: i32) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(build)?;
    frame_generated_packet(CLEAR_ITEMS_PACKET_ID, &payload, false)
}

pub(crate) fn encode_clear_liquids_frame(build: i32) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(build)?;
    frame_generated_packet(CLEAR_LIQUIDS_PACKET_ID, &payload, false)
}

pub(crate) fn encode_set_floor_frame(
    tile: i32,
    floor: i16,
    overlay: i16,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(tile)?;
    payload.write_s(floor)?;
    payload.write_s(overlay)?;
    frame_generated_packet(SET_FLOOR_PACKET_ID, &payload, false)
}

pub(crate) fn encode_set_overlay_frame(tile: i32, overlay: i16) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(tile)?;
    payload.write_s(overlay)?;
    frame_generated_packet(SET_OVERLAY_PACKET_ID, &payload, false)
}

pub(crate) fn encode_set_team_frame(build: i32, team: u8) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(build)?;
    payload.write_b(team)?;
    frame_generated_packet(SET_TEAM_PACKET_ID, &payload, false)
}

pub(crate) fn encode_set_tile_frame(
    tile: i32,
    block: i16,
    team: u8,
    rotation: i32,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(tile)?;
    payload.write_s(block)?;
    payload.write_b(team)?;
    payload.write_i(rotation)?;
    frame_generated_packet(SET_TILE_PACKET_ID, &payload, false)
}

pub(crate) fn encode_create_weather_frame(
    weather: i16,
    intensity: f32,
    duration: f32,
    wind_x: f32,
    wind_y: f32,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_s(weather)?;
    payload.write_f(intensity)?;
    payload.write_f(duration)?;
    payload.write_f(wind_x)?;
    payload.write_f(wind_y)?;
    frame_generated_packet(CREATE_WEATHER_PACKET_ID, &payload, false)
}

pub(crate) fn encode_complete_objective_frame(index: i32) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_i(index)?;
    frame_generated_packet(COMPLETE_OBJECTIVE_PACKET_ID, &payload, false)
}

pub(crate) fn encode_clear_objectives_frame() -> std::io::Result<Vec<u8>> {
    frame_generated_packet(CLEAR_OBJECTIVES_PACKET_ID, &[], false)
}

pub(crate) fn encode_sector_capture_frame() -> std::io::Result<Vec<u8>> {
    frame_generated_packet(SECTOR_CAPTURE_PACKET_ID, &[], false)
}

pub(crate) fn encode_update_game_over_frame(winner: u8) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_b(winner)?;
    frame_generated_packet(UPDATE_GAME_OVER_PACKET_ID, &payload, false)
}

pub(crate) fn encode_researched_frame(content_id: i16) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    payload.write_s(content_id)?;
    frame_generated_packet(RESEARCHED_PACKET_ID, &payload, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::codec::read_packet;

    #[test]
    fn set_flag_and_logic_explosion_carry_official_ids() {
        let flag = encode_set_flag_frame("open", true).unwrap();
        let body = read_packet(std::io::Cursor::new(&flag[2..])).unwrap();
        assert_eq!(body[0], SET_FLAG_PACKET_ID);
        let boom = encode_logic_explosion_frame(1, 8.0, 16.0, 24.0, 40.0, true, true, false, false)
            .unwrap();
        let body = read_packet(std::io::Cursor::new(&boom[2..])).unwrap();
        assert_eq!(body[0], LOGIC_EXPLOSION_PACKET_ID);
        assert_eq!(
            encode_set_map_area_frame(1, 2, 3, 4).unwrap()[2],
            SET_MAP_AREA_PACKET_ID
        );
    }
}
