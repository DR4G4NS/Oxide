//! Rust port of `mindustry.io.TypeIO` object codec, restricted to the subset
//! used by team build plans (`SaveVersion.writeTeamBlocks`/`readTeamBlocks`).
//!
//! Official authority: `core/src/mindustry/io/TypeIO.java` (writeObject /
//! readObject) and `core/src/mindustry/io/SaveVersion.java` (team blocks).
//! Build-plan configs are validated structurally and kept as raw bytes so a
//! save→network round trip is lossless; decoding every value into Rust types
//! is not needed for any current consumer.

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Cursor, Error, ErrorKind, Read, Seek, Write};

/// Maximum object nesting depth accepted while validating plan configs.
const MAX_NESTING: usize = 24;
/// `TypeIO.maxArraySize` used for `readObjectSafe` (build-plan context).
const MAX_ARRAY_SIZE: usize = 1000;
/// `TypeIO.maxByteArraySize`.
const MAX_BYTE_ARRAY_SIZE: usize = 40_000;
/// Plans per team are capped by the official reader (`Math.min(blocks, 1000)`).
const MAX_PLANS_PER_TEAM: usize = 1000;

/// One `BlockPlan` entry of a team: position, rotation, block ID and the raw
/// validated TypeIO config object that follows it.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TeamBlockPlan {
    pub x: i16,
    pub y: i16,
    pub rotation: i16,
    pub block: i16,
    #[serde(default)]
    pub config: Vec<u8>,
}

/// `SaveVersion.writeTeamBlocks` payload: team ID → plans.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TeamBlocks {
    #[serde(default)]
    pub teams: Vec<TeamPlans>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct TeamPlans {
    pub team: i32,
    #[serde(default)]
    pub plans: Vec<TeamBlockPlan>,
}

impl TeamBlocks {
    /// Decodes the official team-blocks section. Returns the plans and the
    /// exact number of bytes consumed (callers re-emit the section verbatim,
    /// so a validated raw round trip is preserved).
    pub fn decode(bytes: &[u8]) -> std::io::Result<(Self, usize)> {
        let mut cursor = Cursor::new(bytes);
        let team_count = cursor.read_i32::<BigEndian>()?;
        if !(0..=256).contains(&team_count) {
            return Err(Error::new(ErrorKind::InvalidData, "invalid team count"));
        }
        let mut teams = Vec::with_capacity(team_count as usize);
        for _ in 0..team_count {
            let team = cursor.read_i32::<BigEndian>()?;
            let plans_count = cursor.read_i32::<BigEndian>()?;
            if !(0..=MAX_PLANS_PER_TEAM as i32).contains(&plans_count) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid team plan count",
                ));
            }
            let mut plans = Vec::with_capacity(plans_count as usize);
            for _ in 0..plans_count {
                let x = cursor.read_i16::<BigEndian>()?;
                let y = cursor.read_i16::<BigEndian>()?;
                let rotation = cursor.read_i16::<BigEndian>()?;
                let block = cursor.read_i16::<BigEndian>()?;
                let config = read_object_bytes(&mut cursor)?;
                plans.push(TeamBlockPlan {
                    x,
                    y,
                    rotation,
                    block,
                    config,
                });
            }
            teams.push(TeamPlans { team, plans });
        }
        Ok((Self { teams }, cursor.position() as usize))
    }

    /// Encodes the official team-blocks section (`writeTeamBlocks`).
    pub fn encode(&self) -> std::io::Result<Vec<u8>> {
        let mut output = Vec::new();
        output.write_i32::<BigEndian>(
            i32::try_from(self.teams.len())
                .map_err(|_| Error::new(ErrorKind::InvalidData, "too many teams"))?,
        )?;
        for team in &self.teams {
            output.write_i32::<BigEndian>(team.team)?;
            output.write_i32::<BigEndian>(
                i32::try_from(team.plans.len())
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "too many plans"))?,
            )?;
            for plan in &team.plans {
                output.write_i16::<BigEndian>(plan.x)?;
                output.write_i16::<BigEndian>(plan.y)?;
                output.write_i16::<BigEndian>(plan.rotation)?;
                output.write_i16::<BigEndian>(plan.block)?;
                output.write_all(&plan.config)?;
            }
        }
        Ok(output)
    }
}

fn read_string<R: Read>(cursor: &mut R, limit: usize) -> std::io::Result<String> {
    let exists = cursor.read_u8()?;
    if exists == 0 {
        return Ok(String::new());
    }
    let length = usize::from(cursor.read_u16::<BigEndian>()?);
    if length > limit {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "TypeIO string exceeds safety limit",
        ));
    }
    let mut buf = vec![0u8; length];
    cursor.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|err| Error::new(ErrorKind::InvalidData, err))
}

fn skip_exact<R: Read>(cursor: &mut R, length: usize, label: &str) -> std::io::Result<()> {
    let mut buf = vec![0u8; length];
    cursor.read_exact(&mut buf).map_err(|err| {
        Error::new(
            ErrorKind::InvalidData,
            format!("truncated TypeIO {label}: {err}"),
        )
    })
}

/// Validates one TypeIO object (tag + payload) against the official
/// `TypeIO.readObject` grammar and returns its raw bytes, so callers can
/// re-emit the exact same object with `writeBuildPlan`. Requires a seekable
/// reader (every production call site passes a `Cursor`).
pub fn read_object_bytes<R: Read + Seek>(cursor: &mut R) -> std::io::Result<Vec<u8>> {
    read_object_bytes_nested(cursor, 0)
}

fn read_object_bytes_nested<R: Read + Seek>(
    cursor: &mut R,
    depth: usize,
) -> std::io::Result<Vec<u8>> {
    if depth > MAX_NESTING {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "TypeIO object nesting exceeds safety limit",
        ));
    }
    let start = cursor.stream_position()?;
    let tag = cursor.read_u8()?;
    match tag {
        0 => {} // null
        1 => {
            cursor.read_i32::<BigEndian>()?;
        }
        2 => {
            cursor.read_i64::<BigEndian>()?;
        }
        3 => {
            cursor.read_f32::<BigEndian>()?;
        }
        4 => {
            read_string(cursor, 1200)?;
        }
        5 => {
            cursor.read_u8()?;
            cursor.read_i16::<BigEndian>()?;
        }
        6 => {
            let length = cursor.read_i16::<BigEndian>()? as usize;
            if length > MAX_ARRAY_SIZE {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "TypeIO int sequence exceeds safety limit",
                ));
            }
            skip_exact(cursor, length * 4, "int sequence")?;
        }
        7 => {
            cursor.read_i32::<BigEndian>()?;
            cursor.read_i32::<BigEndian>()?;
        }
        8 => {
            let length = usize::from(cursor.read_u8()?);
            skip_exact(cursor, length * 4, "Point2 array")?;
        }
        9 => {
            cursor.read_u8()?;
            cursor.read_i16::<BigEndian>()?;
        }
        10 => {
            cursor.read_u8()?;
        }
        11 => {
            cursor.read_f64::<BigEndian>()?;
        }
        12 => {
            cursor.read_i32::<BigEndian>()?;
        }
        13 => {
            cursor.read_i16::<BigEndian>()?;
        }
        14 => {
            let length = usize::try_from(cursor.read_i32::<BigEndian>()?)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "negative byte array length"))?;
            if length > MAX_BYTE_ARRAY_SIZE {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "TypeIO byte array exceeds safety limit",
                ));
            }
            skip_exact(cursor, length, "byte array")?;
        }
        15 => {
            cursor.read_u8()?;
        }
        16 => {
            let length = usize::try_from(cursor.read_i32::<BigEndian>()?)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "negative boolean array length"))?;
            if length > MAX_ARRAY_SIZE {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "TypeIO boolean array exceeds safety limit",
                ));
            }
            skip_exact(cursor, length, "boolean array")?;
        }
        17 => {
            cursor.read_i32::<BigEndian>()?;
        }
        18 => {
            let length = cursor.read_i16::<BigEndian>()? as usize;
            if length > MAX_ARRAY_SIZE {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "TypeIO Vec2 array exceeds safety limit",
                ));
            }
            skip_exact(cursor, length * 8, "Vec2 array")?;
        }
        19 => {
            cursor.read_f32::<BigEndian>()?;
            cursor.read_f32::<BigEndian>()?;
        }
        20 => {
            cursor.read_u8()?;
        }
        21 => {
            let length = usize::try_from(cursor.read_i16::<BigEndian>()?)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "negative int array length"))?;
            if length > MAX_ARRAY_SIZE {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "TypeIO int array exceeds safety limit",
                ));
            }
            skip_exact(cursor, length * 4, "int array")?;
        }
        22 => {
            let length = usize::try_from(cursor.read_i32::<BigEndian>()?)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "negative object array length"))?;
            if length > MAX_ARRAY_SIZE {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "TypeIO object array exceeds safety limit",
                ));
            }
            for _ in 0..length {
                read_object_bytes_nested(cursor, depth + 1)?;
            }
        }
        23 => {
            cursor.read_u16::<BigEndian>()?;
        }
        other => {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("unknown TypeIO object tag {other}"),
            ));
        }
    }
    let end = cursor.stream_position()?;
    cursor.seek(std::io::SeekFrom::Start(start))?;
    let mut raw = vec![0u8; (end - start) as usize];
    cursor.read_exact(&mut raw)?;
    Ok(raw)
}

/// Serializes a raw (already validated) TypeIO object — the `writeBuildPlan`
/// counterpart of `read_object_bytes`. The bytes are trusted as validated.
pub fn write_object_bytes<W: Write>(cursor: &mut W, raw: &[u8]) -> std::io::Result<()> {
    cursor.write_all(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(config: &[u8]) {
        let mut input = Cursor::new(config);
        let read = read_object_bytes(&mut input).unwrap();
        assert_eq!(read, config);
        let mut output = Vec::new();
        write_object_bytes(&mut output, &read).unwrap();
        assert_eq!(output, config);
    }

    #[test]
    fn typeio_plan_configs_round_trip() {
        round_trip(&[0]); // null
        round_trip(&[1, 0, 0, 0, 42]); // Integer
        round_trip(&[3, 0x3f, 0x80, 0, 0]); // Float
        round_trip(&[4, 1, 0, 3, b'a', b'b', b'c']); // String
        round_trip(&[4, 0]); // null string
        round_trip(&[5, 2, 0, 18]); // Content: type 2, id 18
        round_trip(&[7, 0, 0, 1, 0, 0, 0, 2, 0]); // Point2(256, 512)
        round_trip(&[10, 1]); // Boolean
        round_trip(&[11, 0, 0, 0, 0, 0, 0, 0, 0]); // Double
        round_trip(&[12, 0, 0, 0, 0]); // Building box
        round_trip(&[19, 0, 0, 0, 0, 0, 0, 0, 0]); // Vec2
        round_trip(&[20, 1]); // Team
        round_trip(&[23, 0, 4]); // UnitCommand id 4
        round_trip(&[14, 0, 0, 0, 3, 9, 8, 7]); // byte[3]
        round_trip(&[16, 0, 0, 0, 2, 1, 0]); // boolean[2]
        round_trip(&[6, 0, 2, 0, 0, 0, 5, 0, 0, 0, 6]); // IntSeq(5,6)
        round_trip(&[18, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]); // Vec2[1]
        round_trip(&[22, 0, 0, 0, 2, 0, 1, 0, 0, 0, 7]); // Object[null, Integer 7]
        round_trip(&[21, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2]); // int[2] (official writeInts: short length)
    }

    #[test]
    fn typeio_rejects_oversized_arrays() {
        let mut input = Cursor::new([14u8, 0, 0, 0x30, 0x00, 0x00]);
        assert!(read_object_bytes(&mut input).is_err());
        let mut input = Cursor::new([6u8, 0x04, 0x00]);
        assert!(read_object_bytes(&mut input).is_err());
        let mut input = Cursor::new([99u8]);
        assert!(read_object_bytes(&mut input).is_err());
    }

    #[test]
    fn team_blocks_decode_encode_round_trip() {
        let plans = TeamBlocks {
            teams: vec![
                TeamPlans {
                    team: 1,
                    plans: vec![TeamBlockPlan {
                        x: 10,
                        y: 20,
                        rotation: 1,
                        block: 257,
                        config: vec![1, 0, 0, 0, 42],
                    }],
                },
                TeamPlans {
                    team: 2,
                    plans: vec![
                        TeamBlockPlan {
                            x: 1,
                            y: 2,
                            rotation: 0,
                            block: 98,
                            config: vec![0],
                        },
                        TeamBlockPlan {
                            x: 3,
                            y: 4,
                            rotation: 2,
                            block: 100,
                            config: vec![7, 0, 0, 1, 0, 0, 0, 2, 0],
                        },
                    ],
                },
            ],
        };
        let encoded = plans.encode().unwrap();
        let (decoded, consumed) = TeamBlocks::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded, plans);
    }
}
