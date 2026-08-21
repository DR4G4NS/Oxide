#![allow(dead_code, unused_imports)]

use crate::network::codec::{Reads, Writes};
use std::io::{Cursor, Error, ErrorKind, Read, Write};

/// Generated `TileTapCallPacket` id in Mindustry 8 / desktop build 159.7.
pub const TILE_TAP_PACKET_ID: u8 = 140;
/// Generated `ConstructFinishCallPacket` ID in desktop 159.7.
pub const CONSTRUCT_FINISH_PACKET_ID: u8 = 34;
/// Generated `DeconstructFinishCallPacket` ID in desktop 159.7.
pub const DECONSTRUCT_FINISH_PACKET_ID: u8 = 41;
/// Generated `SendChatMessageCallPacket` ID in desktop 159.7.
pub const SEND_CHAT_MESSAGE_PACKET_ID: u8 = 97;
/// Generated `SetRulesCallPacket` ID in desktop 159.7.
pub const SET_RULES_PACKET_ID: u8 = 119;
/// Generated `WorldDataBeginCallPacket` ID in desktop 159.7.
pub const WORLD_DATA_BEGIN_PACKET_ID: u8 = 164;
/// Framework StreamChunk packet ID (not a generated Call packet).
pub const STREAM_CHUNK_PACKET_ID: u8 = 1;

/// `TileTapCallPacket` is a `Loc.both` call.  CallGenerator omits its first
/// (`Player`) parameter when a client writes to the server; TypeIO therefore
/// writes only the tile on C→S and writes the player entity followed by the
/// tile on S→C.  `-1` is the TypeIO null-entity sentinel.
const TILE_TAP_NO_PLAYER: i32 = -1;

#[derive(Debug, Clone)]
pub enum RpcPacket {
    TileTap {
        player_id: i32,
        x: i16,
        y: i16,
    },
    ConstructFinish {
        tile_x: i16,
        tile_y: i16,
        block_id: u16,
        builder_id: i32,
        rotation: u8,
        team: u8,
    },
    DeconstructFinish {
        tile_x: i16,
        tile_y: i16,
        block_id: u16,
        builder_id: i32,
    },
    SendChatMessage {
        player_id: i32,
        message: String,
    },
    PlayerInfoSync {
        player_id: i32,
        name: String,
        x: f32,
        y: f32,
        unit_id: i32,
    },
    SetRules {
        rules_json: String,
    },
    WorldSyncBegin,
    /// Compatibility-only marker retained for the old API; use `StreamChunk`
    /// for the actual framework packet.
    WorldSyncChunk,
    StreamChunk {
        id: i32,
        data: Vec<u8>,
    },
}

impl RpcPacket {
    /// Pack a tile exactly like `Point2.pack(x, y)` used by TypeIO.
    fn pack_tile(x: i16, y: i16) -> i32 {
        ((x as i32) << 16) | (y as u16 as i32)
    }

    fn unpack_tile(position: i32) -> (i16, i16) {
        ((position >> 16) as i16, position as i16)
    }

    /// Read at most one TileTap payload.  The generated packet is small and
    /// bounded; rejecting an extra byte avoids buffering an
    /// attacker-controlled payload. Direction is selected by the explicit
    /// `read_client`/`read_server` entry points.
    fn read_bounded<R: Read>(r: &mut R, max: usize) -> std::io::Result<Vec<u8>> {
        let mut limited = r.take((max + 1) as u64);
        let mut payload = Vec::with_capacity(max);
        limited.read_to_end(&mut payload)?;
        if payload.len() > max {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "TileTap payload exceeds the official maximum length",
            ));
        }
        Ok(payload)
    }

    fn decode_tile_tap(payload: &[u8], with_player: bool) -> std::io::Result<Self> {
        let expected = if with_player { 8 } else { 4 };
        if payload.len() != expected {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid TileTap payload length",
            ));
        }
        let mut r = Cursor::new(payload);
        let player_id = if with_player {
            r.read_i()?
        } else {
            TILE_TAP_NO_PLAYER
        };
        let position = r.read_i()?;
        let (x, y) = Self::unpack_tile(position);
        Ok(Self::TileTap { player_id, x, y })
    }

    /// Decode the client-to-server TileTap form (`TypeIO.writeTile` only).
    pub fn read_client<R: Read>(mut r: R) -> std::io::Result<Self> {
        let payload = Self::read_bounded(&mut r, 4)?;
        Self::decode_tile_tap(&payload, false)
    }

    /// Decode the server-to-client TileTap form (`writeEntity` + tile).
    pub fn read_server<R: Read>(mut r: R) -> std::io::Result<Self> {
        let payload = Self::read_bounded(&mut r, 8)?;
        Self::decode_tile_tap(&payload, true)
    }

    /// Encode the client-to-server TileTap form.  The caller/player argument
    /// is intentionally omitted by generated `Call` code for `Loc.both`.
    pub fn write_client<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        match self {
            Self::TileTap { x, y, .. } => w.write_i(Self::pack_tile(*x, *y)),
            _ => Err(Error::new(
                ErrorKind::InvalidInput,
                "write_client is only defined for TileTap",
            )),
        }
    }

    /// Encode the server-to-client TileTap form (`writeEntity(player)` then
    /// `writeTile(tile)`).
    pub fn write_server<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        match self {
            Self::TileTap { player_id, x, y } => {
                w.write_i(*player_id)?;
                w.write_i(Self::pack_tile(*x, *y))
            }
            _ => Err(Error::new(
                ErrorKind::InvalidInput,
                "write_server is only defined for TileTap",
            )),
        }
    }

    pub fn read<R: Read>(mut r: R, id: u8) -> std::io::Result<Self> {
        match id {
            TILE_TAP_PACKET_ID => {
                // The historical `read(id)` API has no direction argument;
                // retain its server-side form for existing callers. New code
                // must use `read_client` or `read_server` explicitly because
                // the generated packet has direction-dependent layouts.
                Self::read_server(r)
            }
            STREAM_CHUNK_PACKET_ID => {
                let chunk_id = r.read_i()?;
                let len = r.read_us()? as usize;
                let mut data = vec![0; len];
                r.read_exact(&mut data)?;
                Ok(RpcPacket::StreamChunk { id: chunk_id, data })
            }
            CONSTRUCT_FINISH_PACKET_ID => {
                let tile = r.read_i()?;
                let block_id = r.read_s()? as u16;
                let unit_kind = r.read_b()?;
                let unit_id = r.read_i()?;
                let builder_id = if unit_kind == 0 { -1 } else { unit_id };
                let rotation = r.read_b()?;
                let team = r.read_b()?;
                // ConstructFinish carries a TypeIO.writeObject config after
                // the team.  The compatibility enum does not model config;
                // consume the null tag and reject non-null objects explicitly
                // rather than silently leaving a valid payload misaligned.
                let config_tag = r.read_b()?;
                if config_tag != 0 {
                    return Err(Error::new(
                        ErrorKind::Unsupported,
                        "ConstructFinish config object is unsupported",
                    ));
                }
                Ok(RpcPacket::ConstructFinish {
                    tile_x: (tile >> 16) as i16,
                    tile_y: tile as i16,
                    block_id,
                    builder_id,
                    rotation,
                    team,
                })
            }
            DECONSTRUCT_FINISH_PACKET_ID => {
                let tile = r.read_i()?;
                let block_id = r.read_s()? as u16;
                let unit_kind = r.read_b()?;
                let unit_id = r.read_i()?;
                let builder_id = if unit_kind == 0 { -1 } else { unit_id };
                Ok(RpcPacket::DeconstructFinish {
                    tile_x: (tile >> 16) as i16,
                    tile_y: tile as i16,
                    block_id,
                    builder_id,
                })
            }
            SEND_CHAT_MESSAGE_PACKET_ID => Ok(RpcPacket::SendChatMessage {
                player_id: TILE_TAP_NO_PLAYER,
                message: r.read_typeio_string()?.unwrap_or_default(),
            }),
            SET_RULES_PACKET_ID => {
                let len = r.read_i()?;
                if !(0..=100_000).contains(&len) {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "invalid rules payload length",
                    ));
                }
                let mut bytes = vec![0; len as usize];
                r.read_exact(&mut bytes)?;
                let rules_json = String::from_utf8(bytes).map_err(|_| {
                    Error::new(ErrorKind::InvalidData, "rules payload is not UTF-8")
                })?;
                Ok(RpcPacket::SetRules { rules_json })
            }
            WORLD_DATA_BEGIN_PACKET_ID => Ok(RpcPacket::WorldSyncBegin),
            _ => Err(Error::new(ErrorKind::InvalidData, "Unknown RPC packet ID")),
        }
    }

    pub fn write<W: Write>(&self, mut w: W) -> std::io::Result<()> {
        match self {
            RpcPacket::TileTap { player_id, x, y } => {
                // Keep the old directionless writer as the S→C form; callers
                // sending a client RPC must use `write_client` explicitly.
                w.write_i(*player_id)?;
                w.write_i(Self::pack_tile(*x, *y))?;
            }
            RpcPacket::ConstructFinish {
                tile_x,
                tile_y,
                block_id,
                builder_id,
                rotation,
                team,
            } => {
                // TypeIO.writeTile/writeBlock/writeUnit/writeTeam/writeObject.
                w.write_i(Self::pack_tile(*tile_x, *tile_y))?;
                w.write_s(*block_id as i16)?;
                if *builder_id < 0 {
                    w.write_b(0)?;
                    w.write_i(0)?;
                } else {
                    w.write_b(2)?; // ordinary Unit, not a BlockUnit
                    w.write_i(*builder_id)?;
                }
                w.write_b(*rotation)?;
                w.write_b(*team)?;
                w.write_b(0)?; // null config object
            }
            RpcPacket::DeconstructFinish {
                tile_x,
                tile_y,
                block_id,
                builder_id,
            } => {
                w.write_i(Self::pack_tile(*tile_x, *tile_y))?;
                w.write_s(*block_id as i16)?;
                if *builder_id < 0 {
                    w.write_b(0)?;
                    w.write_i(0)?;
                } else {
                    w.write_b(2)?;
                    w.write_i(*builder_id)?;
                }
            }
            RpcPacket::SendChatMessage { message, .. } => {
                w.write_typeio_string(Some(message))?;
            }
            RpcPacket::PlayerInfoSync { .. } => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "PlayerInfoSync has no official packet",
                ));
            }
            RpcPacket::SetRules { rules_json } => {
                let bytes = rules_json.as_bytes();
                let len = i32::try_from(bytes.len()).map_err(|_| {
                    Error::new(ErrorKind::InvalidInput, "rules payload exceeds i32 length")
                })?;
                w.write_i(len)?;
                w.write_all(bytes)?;
            }
            RpcPacket::WorldSyncBegin => {}
            RpcPacket::WorldSyncChunk => {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    "WorldSyncChunk is a compatibility marker; use StreamChunk",
                ));
            }
            RpcPacket::StreamChunk { id, data } => {
                let len = u16::try_from(data.len()).map_err(|_| {
                    Error::new(ErrorKind::InvalidInput, "stream chunk exceeds u16 length")
                })?;
                w.write_i(*id)?;
                w.write_us(len)?;
                w.write_all(data)?;
            }
        }
        Ok(())
    }

    /// Return the official packet ID, or an error for the retained
    /// compatibility-only PlayerInfoSync variant (no such generated packet
    /// exists in the desktop JAR).
    pub fn try_id(&self) -> std::io::Result<u8> {
        match self {
            RpcPacket::TileTap { .. } => Ok(TILE_TAP_PACKET_ID),
            RpcPacket::ConstructFinish { .. } => Ok(CONSTRUCT_FINISH_PACKET_ID),
            RpcPacket::DeconstructFinish { .. } => Ok(DECONSTRUCT_FINISH_PACKET_ID),
            RpcPacket::SendChatMessage { .. } => Ok(SEND_CHAT_MESSAGE_PACKET_ID),
            RpcPacket::PlayerInfoSync { .. } => Err(Error::new(
                ErrorKind::Unsupported,
                "PlayerInfoSync has no official packet",
            )),
            RpcPacket::SetRules { .. } => Ok(SET_RULES_PACKET_ID),
            RpcPacket::WorldSyncBegin => Ok(WORLD_DATA_BEGIN_PACKET_ID),
            RpcPacket::WorldSyncChunk => Err(Error::new(
                ErrorKind::Unsupported,
                "WorldSyncChunk is a compatibility marker; use StreamChunk",
            )),
            RpcPacket::StreamChunk { .. } => Ok(STREAM_CHUNK_PACKET_ID),
        }
    }

    /// Compatibility shorthand. Prefer `try_id()` when handling packets from
    /// untrusted or externally supplied variants.
    pub fn id(&self) -> u8 {
        self.try_id().unwrap_or(u8::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_generated_call_ids_are_not_legacy_synthetic_indices() {
        assert_eq!(
            RpcPacket::ConstructFinish {
                tile_x: 1,
                tile_y: 2,
                block_id: 10,
                builder_id: -1,
                rotation: 0,
                team: 1,
            }
            .id(),
            CONSTRUCT_FINISH_PACKET_ID
        );
        assert_eq!(
            RpcPacket::DeconstructFinish {
                tile_x: 1,
                tile_y: 2,
                block_id: 10,
                builder_id: -1,
            }
            .id(),
            DECONSTRUCT_FINISH_PACKET_ID
        );
        assert_eq!(
            RpcPacket::SendChatMessage {
                player_id: 42,
                message: "hello".to_string(),
            }
            .id(),
            SEND_CHAT_MESSAGE_PACKET_ID
        );
        assert_eq!(
            RpcPacket::SetRules {
                rules_json: "{}".to_string(),
            }
            .id(),
            SET_RULES_PACKET_ID
        );
        assert_eq!(RpcPacket::WorldSyncBegin.id(), WORLD_DATA_BEGIN_PACKET_ID);
        assert_eq!(
            RpcPacket::StreamChunk {
                id: 1,
                data: vec![2],
            }
            .id(),
            STREAM_CHUNK_PACKET_ID
        );
        assert_eq!(RpcPacket::WorldSyncChunk.id(), u8::MAX);
        assert_eq!(
            RpcPacket::PlayerInfoSync {
                player_id: 1,
                name: "x".to_string(),
                x: 0.0,
                y: 0.0,
                unit_id: 2,
            }
            .id(),
            u8::MAX
        );
    }

    #[test]
    fn official_simple_rpc_layouts_use_typeio_fields() {
        let chat = RpcPacket::SendChatMessage {
            player_id: 7,
            message: "hello".to_string(),
        };
        let mut bytes = Vec::new();
        chat.write(&mut bytes).unwrap();
        assert_eq!(bytes[0], 1); // TypeIO.writeString non-null tag
        assert_eq!(
            RpcPacket::read(Cursor::new(bytes), SEND_CHAT_MESSAGE_PACKET_ID)
                .unwrap()
                .id(),
            SEND_CHAT_MESSAGE_PACKET_ID
        );

        let rules = RpcPacket::SetRules {
            rules_json: "{\"pvp\":true}".to_string(),
        };
        let mut bytes = Vec::new();
        rules.write(&mut bytes).unwrap();
        assert_eq!(i32::from_be_bytes(bytes[..4].try_into().unwrap()), 12);
        assert_eq!(&bytes[4..], b"{\"pvp\":true}");

        let stream = RpcPacket::StreamChunk {
            id: 77,
            data: vec![1, 2, 3],
        };
        let mut bytes = Vec::new();
        stream.write(&mut bytes).unwrap();
        assert_eq!(&bytes[..4], &77i32.to_be_bytes());
        assert_eq!(&bytes[4..6], &3u16.to_be_bytes());
        assert_eq!(&bytes[6..], &[1, 2, 3]);
        assert!(matches!(
            RpcPacket::read(Cursor::new(bytes), STREAM_CHUNK_PACKET_ID).unwrap(),
            RpcPacket::StreamChunk { id: 77, data } if data == vec![1, 2, 3]
        ));

        let construct = RpcPacket::ConstructFinish {
            tile_x: -2,
            tile_y: 300,
            block_id: 257,
            builder_id: -1,
            rotation: 1,
            team: 5,
        };
        let mut bytes = Vec::new();
        construct.write(&mut bytes).unwrap();
        assert_eq!(&bytes[..4], &((-2i32 << 16) | 300).to_be_bytes());
        assert_eq!(&bytes[4..6], &257i16.to_be_bytes());
        assert_eq!(bytes[6], 0); // null TypeIO unit kind
        assert_eq!(bytes[11], 1); // rotation after unit kind + id
        assert_eq!(bytes[12], 5); // team
        assert_eq!(bytes[13], 0); // null config object
        assert!(matches!(
            RpcPacket::read(Cursor::new(bytes), CONSTRUCT_FINISH_PACKET_ID),
            Ok(RpcPacket::ConstructFinish {
                tile_x: -2,
                tile_y: 300,
                block_id: 257,
                builder_id: -1,
                rotation: 1,
                team: 5,
            })
        ));
    }

    #[test]
    fn tile_tap_client_layout_matches_typeio_write_tile() {
        // TypeIO.writeTile writes Point2.pack(x, y) as one big-endian int.
        let packet = RpcPacket::TileTap {
            player_id: TILE_TAP_NO_PLAYER,
            x: -2,
            y: 300,
        };
        assert_eq!(packet.id(), TILE_TAP_PACKET_ID);

        let packed = ((-2i32) << 16) | 300i32;
        let mut bytes = Vec::new();
        packet.write_client(&mut bytes).unwrap();
        assert_eq!(bytes, packed.to_be_bytes());

        let decoded = RpcPacket::read_client(Cursor::new(bytes)).unwrap();
        match decoded {
            RpcPacket::TileTap { player_id, x, y } => {
                assert_eq!(player_id, TILE_TAP_NO_PLAYER);
                assert_eq!((x, y), (-2, 300));
            }
            other => panic!("unexpected packet: {other:?}"),
        }
    }

    #[test]
    fn tile_tap_server_layout_matches_typeio_write_entity_then_tile() {
        // CallGenerator writes the Player only when the server broadcasts the
        // Loc.both call; TypeIO.writeEntity is an int entity id.
        let packet = RpcPacket::TileTap {
            player_id: 42,
            x: 45,
            y: 100,
        };
        let mut bytes = Vec::new();
        packet.write_server(&mut bytes).unwrap();

        let mut expected = 42i32.to_be_bytes().to_vec();
        expected.extend_from_slice(&(((45i32) << 16) | 100i32).to_be_bytes());
        assert_eq!(bytes, expected);

        let decoded = RpcPacket::read_server(Cursor::new(bytes)).unwrap();
        assert!(matches!(
            decoded,
            RpcPacket::TileTap {
                player_id: 42,
                x: 45,
                y: 100
            }
        ));
    }

    #[test]
    fn tile_tap_directionless_compatibility_path_is_server_form_only() {
        let packet = RpcPacket::TileTap {
            player_id: 7,
            x: 1,
            y: 2,
        };
        let mut bytes = Vec::new();
        packet.write(&mut bytes).unwrap();
        assert_eq!(
            RpcPacket::read(Cursor::new(bytes), TILE_TAP_PACKET_ID)
                .unwrap()
                .id(),
            TILE_TAP_PACKET_ID
        );

        // A directionless reader must not guess based on payload length; the
        // explicit client reader rejects the S→C form instead.
        let mut server_bytes = Vec::new();
        packet.write_server(&mut server_bytes).unwrap();
        assert!(RpcPacket::read_client(Cursor::new(server_bytes)).is_err());
    }
}
