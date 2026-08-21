#![allow(dead_code)]

use byteorder::{BigEndian, ReadBytesExt};
use flate2::read::ZlibDecoder;
use std::collections::HashMap;
use std::io::{Error, ErrorKind, Read};

pub const SAVE_HEADER: &[u8; 4] = b"MSAV";

/// Highest MSAV SaveVersion the current target accepts (Save1..Save13).
pub const MAX_BASELINE_SAVE_VERSION: i32 = crate::compat_target::CURRENT_SAVE_VERSION;
const MAX_REGION_SIZE: usize = 64 * 1024 * 1024;
const MAX_MAP_TILES: usize = 4_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct SaveMeta {
    pub version: i32,
    pub map_name: String,
    pub description: String,
    pub author: String,
    pub width: u16,
    pub height: u16,
    pub build_version: i32,
    pub waves_count: i32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tile {
    pub floor_id: i16,
    pub overlay_id: i16,
    pub block_id: i16,
    pub data: i8,
    pub floor_data: i8,
    pub overlay_data: i8,
    pub extra_data: i32,
    pub has_entity: bool,
    pub entity_data: Vec<u8>,
}

pub struct SaveIO;

impl SaveIO {
    pub fn is_msav_header<R: Read>(mut reader: R) -> bool {
        let mut header = [0u8; 4];
        if reader.read_exact(&mut header).is_err() {
            return false;
        }
        &header == SAVE_HEADER
    }
    fn read_utf<R: Read>(reader: &mut R) -> std::io::Result<String> {
        // MSAV tags use Java modified UTF-8 (writeUTF), matching the official
        // SaveVersion.writeStringMap/readStringMap.
        crate::network::codec::read_modified_utf8_public(reader)
    }

    fn read_string_map<R: Read>(reader: &mut R) -> std::io::Result<HashMap<String, String>> {
        let size = reader.read_i16::<BigEndian>()?;
        let mut map = HashMap::new();
        for _ in 0..size {
            let key = Self::read_utf(reader)?;
            let value = Self::read_utf(reader)?;
            map.insert(key, value);
        }
        Ok(map)
    }

    pub(crate) fn validate_patches_region(version: i32, patches: &[u8]) -> Result<(), Error> {
        let vanilla_empty = match version {
            11 => patches == [0u8],
            12 => patches.len() == 12 && patches[4..] == [0, 0, 0, 0, 0, 0, 0, 0],
            13.. => patches == [0, 0, 0, 2, 0, 0, 0, 0],
            _ => true,
        };
        if vanilla_empty {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::Unsupported,
                "maps with data patches or external assets are outside the certified vanilla 159.7 host scope",
            ))
        }
    }

    fn read_patches_region<R: Read>(reader: &mut R, version: i32) -> Result<(), Error> {
        let len = Self::checked_length(reader.read_i32::<BigEndian>()?, "patches")?;
        let mut data = vec![0u8; len];
        reader.read_exact(&mut data)?;
        Self::validate_patches_region(version, &data)
    }

    fn skip_region<R: Read>(reader: &mut R) -> std::io::Result<()> {
        let len = Self::checked_length(reader.read_i32::<BigEndian>()?, "region")?;
        let mut limited = reader.take(len as u64);
        let copied = std::io::copy(&mut limited, &mut std::io::sink())?;
        if copied != len as u64 {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "truncated save region",
            ));
        }
        Ok(())
    }

    fn checked_length(value: i32, label: &str) -> std::io::Result<usize> {
        let len = usize::try_from(value)
            .map_err(|_| Error::new(ErrorKind::InvalidData, format!("negative {label} length")))?;
        if len > MAX_REGION_SIZE {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!("{label} length exceeds safety limit"),
            ));
        }
        Ok(len)
    }

    pub fn read_meta<R: Read>(reader: R) -> Result<SaveMeta, Error> {
        let mut zlib = ZlibDecoder::new(reader);

        let mut magic = [0u8; 4];
        zlib.read_exact(&mut magic)?;
        if &magic != b"MSAV" {
            return Err(Error::new(ErrorKind::InvalidData, "Invalid magic bytes"));
        }

        let version = zlib.read_i32::<BigEndian>()?;
        if !(1..=MAX_BASELINE_SAVE_VERSION).contains(&version) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "SaveVersion {version} not supported (desktop.jar ships Save1..Save{MAX_BASELINE_SAVE_VERSION})"
                ),
            ));
        }

        let _meta_len = zlib.read_i32::<BigEndian>()?;
        let map = Self::read_string_map(&mut zlib)?;

        Ok(SaveMeta {
            version,
            map_name: map.get("mapname").cloned().unwrap_or_default(),
            description: map.get("description").cloned().unwrap_or_default(),
            author: map.get("author").cloned().unwrap_or_default(),
            width: map.get("width").and_then(|s| s.parse().ok()).unwrap_or(0),
            height: map.get("height").and_then(|s| s.parse().ok()).unwrap_or(0),
            build_version: map.get("build").and_then(|s| s.parse().ok()).unwrap_or(0),
            waves_count: map.get("wave").and_then(|s| s.parse().ok()).unwrap_or(0),
        })
    }

    pub fn read_map<R: Read>(reader: R) -> Result<(SaveMeta, Vec<Tile>), Error> {
        let mut zlib = ZlibDecoder::new(reader);

        let mut magic = [0u8; 4];
        zlib.read_exact(&mut magic)?;
        if &magic != b"MSAV" {
            return Err(Error::new(ErrorKind::InvalidData, "Invalid magic bytes"));
        }

        let version = zlib.read_i32::<BigEndian>()?;
        if !(1..=MAX_BASELINE_SAVE_VERSION).contains(&version) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!(
                    "SaveVersion {version} not supported (desktop.jar ships Save1..Save{MAX_BASELINE_SAVE_VERSION})"
                ),
            ));
        }

        // 1. Read meta region
        let _meta_len = zlib.read_i32::<BigEndian>()?;
        let map = Self::read_string_map(&mut zlib)?;

        let meta = SaveMeta {
            version,
            map_name: map.get("mapname").cloned().unwrap_or_default(),
            description: map.get("description").cloned().unwrap_or_default(),
            author: map.get("author").cloned().unwrap_or_default(),
            width: map.get("width").and_then(|s| s.parse().ok()).unwrap_or(0),
            height: map.get("height").and_then(|s| s.parse().ok()).unwrap_or(0),
            build_version: map.get("build").and_then(|s| s.parse().ok()).unwrap_or(0),
            waves_count: map.get("wave").and_then(|s| s.parse().ok()).unwrap_or(0),
        };

        // 2. Regions before the map: official SaveVersion.read order differs by version.
        if version <= 10 {
            Self::skip_region(&mut zlib)?;
        } else if version == 11 {
            Self::skip_region(&mut zlib)?;
            Self::read_patches_region(&mut zlib, version)?;
        } else {
            Self::read_patches_region(&mut zlib, version)?;
            Self::skip_region(&mut zlib)?;
        }

        // 3. Map region (format depends on the save version)
        let (_width, _height, tiles) = Self::read_map_region(&mut zlib, version)?;

        Ok((meta, tiles))
    }

    /// Shared map-region reader dispatching on the save version. The floor
    /// plus overlay RLE section is identical for every version (official
    /// SaveVersion.readMap / LegacySaveVersion.readMap), while the block
    /// section differs by version (legacy short-chunk vs modern i32-prefixed
    /// building chunks, see the per-version readers below).
    fn read_map_region<R: Read>(
        zlib: &mut R,
        version: i32,
    ) -> Result<(u16, u16, Vec<Tile>), Error> {
        let _map_len = zlib.read_i32::<BigEndian>()?;
        let width = zlib.read_u16::<BigEndian>()?;
        let height = zlib.read_u16::<BigEndian>()?;
        let total = (width as usize)
            .checked_mul(height as usize)
            .filter(|total| *total <= MAX_MAP_TILES)
            .ok_or_else(|| {
                Error::new(ErrorKind::InvalidData, "map dimensions exceed safety limit")
            })?;
        let mut tiles = vec![Tile::default(); total];

        Self::read_map_floors(zlib, total, &mut tiles)?;

        match version {
            1..=3 => Self::read_map_blocks_legacy(zlib, total, &mut tiles)?,
            // Save4..Save9 use ShortChunkSaveVersion (u16 chunks); Save10
            // extends SaveVersion (i32 chunks) — round-73 M1 dispatch fix.
            4..=9 => Self::read_map_blocks_short(zlib, total, &mut tiles)?,
            _ => Self::read_map_blocks_modern(zlib, total, &mut tiles)?,
        }
        Ok((width, height, tiles))
    }

    /// 4.a Floor and overlay with consecutive-run compression (identical in
    /// every save version).  // SOL-008 REMAINING: the official reader maps
    /// a floor id resolving to `Blocks.air` to `stone` on v1-v10 saves; the
    /// port has no content registry at this layer and preserves the raw id
    /// (byte-exact round trip instead).
    fn read_map_floors<R: Read>(
        zlib: &mut R,
        total: usize,
        tiles: &mut [Tile],
    ) -> std::io::Result<()> {
        let mut i = 0;
        while i < total {
            let floor_id = zlib.read_i16::<BigEndian>()?;
            let overlay_id = zlib.read_i16::<BigEndian>()?;
            let consecutives = zlib.read_u8()?;

            tiles[i].floor_id = floor_id;
            tiles[i].overlay_id = overlay_id;

            let run_end = (i + consecutives as usize + 1).min(total);
            for tile in tiles.iter_mut().take(run_end).skip(i + 1) {
                tile.floor_id = floor_id;
                tile.overlay_id = overlay_id;
            }
            i += consecutives as usize + 1;
        }
        Ok(())
    }

    /// 4.b Block section of v11+ maps (`SaveVersion.readMap`): packed byte,
    /// i32-prefixed building chunks.
    fn read_map_blocks_modern<R: Read>(
        zlib: &mut R,
        total: usize,
        tiles: &mut [Tile],
    ) -> std::io::Result<()> {
        let mut i = 0;
        while i < total {
            let block_id = zlib.read_i16::<BigEndian>()?;
            let packed = zlib.read_i8()?;

            let had_entity = (packed & 1) != 0;
            let had_data = (packed & 4) != 0;

            let mut data = 0;
            let mut floor_data = 0;
            let mut overlay_data = 0;
            let mut extra_data = 0;

            if had_data {
                data = zlib.read_i8()?;
                floor_data = zlib.read_i8()?;
                overlay_data = zlib.read_i8()?;
                extra_data = zlib.read_i32::<BigEndian>()?;
            }

            let mut is_center = true;
            if had_entity {
                is_center = zlib.read_u8()? != 0;
            }

            tiles[i].block_id = block_id;
            tiles[i].data = data;
            tiles[i].floor_data = floor_data;
            tiles[i].overlay_data = overlay_data;
            tiles[i].extra_data = extra_data;
            tiles[i].has_entity = had_entity;

            if had_entity {
                if is_center {
                    let chunk_len =
                        Self::checked_length(zlib.read_i32::<BigEndian>()?, "entity chunk")?;
                    let mut chunk_data = vec![0u8; chunk_len];
                    zlib.read_exact(&mut chunk_data)?;
                    tiles[i].entity_data = chunk_data;
                }
            } else if !had_data {
                let consecutives = zlib.read_u8()?;
                let run_end = (i + consecutives as usize + 1).min(total);
                for tile in tiles.iter_mut().take(run_end).skip(i + 1) {
                    tile.block_id = block_id;
                }
                i += consecutives as usize;
            }
            i += 1;
        }
        Ok(())
    }

    /// Block section of v4-10 maps (`ShortChunkSaveVersion.readMap`): packed
    /// byte with the entity (1), old-data (2) and new-data (4) bits; building
    /// chunks are u16-prefixed (`readLegacyShortChunk`).
    fn read_map_blocks_short<R: Read>(
        zlib: &mut R,
        total: usize,
        tiles: &mut [Tile],
    ) -> std::io::Result<()> {
        let mut i = 0;
        while i < total {
            let block_id = zlib.read_i16::<BigEndian>()?;
            let packed = zlib.read_i8()?;

            let had_entity = (packed & 1) != 0;
            let had_data_old = (packed & 2) != 0;
            let had_data = (packed & 4) != 0;

            let mut data = 0;
            let mut floor_data = 0;
            let mut overlay_data = 0;
            let mut extra_data = 0;

            if had_data {
                data = zlib.read_i8()?;
                floor_data = zlib.read_i8()?;
                overlay_data = zlib.read_i8()?;
                extra_data = zlib.read_i32::<BigEndian>()?;
            }

            let mut is_center = true;
            if had_entity {
                is_center = zlib.read_u8()? != 0;
            }

            tiles[i].block_id = block_id;
            tiles[i].data = data;
            tiles[i].floor_data = floor_data;
            tiles[i].overlay_data = overlay_data;
            tiles[i].extra_data = extra_data;
            tiles[i].has_entity = had_entity;

            if had_entity {
                if is_center {
                    // ShortChunkSaveVersion: u16 length prefix.
                    let chunk_len = zlib.read_u16::<BigEndian>()? as usize;
                    let mut chunk_data = vec![0u8; chunk_len];
                    zlib.read_exact(&mut chunk_data)?;
                    tiles[i].entity_data = chunk_data;
                }
            } else if had_data_old {
                // The old data format was a single block-specific byte
                // (official: tile.setBlock(block); tile.data = readByte()).
                tiles[i].data = zlib.read_i8()?;
            } else if !had_data {
                let consecutives = zlib.read_u8()?;
                let run_end = (i + consecutives as usize + 1).min(total);
                for tile in tiles.iter_mut().take(run_end).skip(i + 1) {
                    tile.block_id = block_id;
                }
                i += consecutives as usize;
            }
            i += 1;
        }
        Ok(())
    }

    /// Block section of v1-3 maps (`LegacySaveVersion.readMap`): no packed
    /// byte.  A legacy short chunk (u16 length) follows every block that has
    /// a building (the official reader dispatches on `block.hasBuilding()`);
    /// every other block is followed by a run byte.  Legacy chunk payloads
    /// use the old base header (u16 health, nibble-packed team/rotation) and
    /// are preserved raw in `Tile::entity_data` — the subclass bytes are not
    /// decoded by this layer.
    fn read_map_blocks_legacy<R: Read>(
        zlib: &mut R,
        total: usize,
        tiles: &mut [Tile],
    ) -> std::io::Result<()> {
        let mut i = 0;
        while i < total {
            let block_id = zlib.read_i16::<BigEndian>()?;
            tiles[i].block_id = block_id;

            if block_has_building(block_id) {
                let chunk_len = zlib.read_u16::<BigEndian>()? as usize;
                let mut chunk_data = vec![0u8; chunk_len];
                zlib.read_exact(&mut chunk_data)?;
                tiles[i].has_entity = true;
                tiles[i].entity_data = chunk_data;
            } else {
                let consecutives = zlib.read_u8()?;
                let run_end = (i + consecutives as usize + 1).min(total);
                for tile in tiles.iter_mut().take(run_end).skip(i + 1) {
                    tile.block_id = block_id;
                }
                i += consecutives as usize;
            }
            i += 1;
        }
        Ok(())
    }
}

/// Whether a block carries a Building entity (`Block.hasBuilding()` in the
/// official reader).  The port approximates it with the official `synthetic`
/// flag of the v158.1 registry — in the official content every synthetic
/// block has a build type and every non-synthetic vanilla block (walls,
/// floors, ores, decorations) has none.
/// // SOL-008 REMAINING: editor-only blocks that set an explicit buildType
/// without being synthetic are not covered by this approximation.
fn block_has_building(block_id: i16) -> bool {
    crate::game::content::block_pathing(block_id).synthetic
}

// ---------------------------------------------------------------------------
// MSAV LEGACY READER (SOL-008): full parse of v1-v3 saves.  Official layout
// (versions/LegacySaveVersion.java, Save1/Save2/Save3.java): the deflated
// stream is "MSAV" + i32 version + exactly four regions — meta (StringMap),
// content (header), map, entities — with no patches/markers/custom regions.
// ---------------------------------------------------------------------------

/// One v3 team build plan (`Save3.readEntities`): position, rotation, block
/// and the RAW i32 config the legacy format stored (newer versions wrap the
/// config in TypeIO; v3 does not).
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyPlan {
    pub team: i32,
    pub x: i16,
    pub y: i16,
    pub rotation: i16,
    pub block: i16,
    pub config: i32,
}

/// Result of parsing a v1-v3 MSAV save.  The map region is fully decoded;
/// legacy building chunks and world entities are preserved as opaque bytes
/// (their subclass payloads are not modeled by the port), and every
/// preserved-but-unsupported payload is listed in `warnings`.
#[derive(Debug, Clone)]
pub struct LegacySaveInfo {
    pub version: i32,
    pub meta: SaveMeta,
    pub width: u16,
    pub height: u16,
    pub tiles: Vec<Tile>,
    /// v3 team build plans (empty for v1/v2, which have no plans table).
    pub team_plans: Vec<LegacyPlan>,
    /// Number of legacy world-entity chunks skipped (`readLegacyEntities`).
    pub skipped_entity_chunks: usize,
    /// Explicit unsupported-legacy-data warnings.
    pub warnings: Vec<String>,
}

impl SaveIO {
    /// Reads a v1-v3 (legacy) MSAV save end to end: meta region, map region
    /// (legacy block format) and a best-effort entities region.  Building
    /// chunk payloads and world entities are preserved raw — the port does
    /// not decode their block-specific modules or subclass tails.
    pub fn read_legacy_save<R: Read>(reader: R) -> Result<LegacySaveInfo, Error> {
        let mut zlib = ZlibDecoder::new(reader);

        let mut magic = [0u8; 4];
        zlib.read_exact(&mut magic)?;
        if &magic != b"MSAV" {
            return Err(Error::new(ErrorKind::InvalidData, "Invalid magic bytes"));
        }

        let version = zlib.read_i32::<BigEndian>()?;
        if !(1..=3).contains(&version) {
            return Err(Error::new(
                ErrorKind::Unsupported,
                format!("Version {version} is not a legacy (v1-v3) save"),
            ));
        }

        // 1. Meta region (same StringMap layout in every version).
        let _meta_len = zlib.read_i32::<BigEndian>()?;
        let map = Self::read_string_map(&mut zlib)?;
        let meta = SaveMeta {
            version,
            map_name: map.get("mapname").cloned().unwrap_or_default(),
            description: map.get("description").cloned().unwrap_or_default(),
            author: map.get("author").cloned().unwrap_or_default(),
            width: map.get("width").and_then(|s| s.parse().ok()).unwrap_or(0),
            height: map.get("height").and_then(|s| s.parse().ok()).unwrap_or(0),
            build_version: map.get("build").and_then(|s| s.parse().ok()).unwrap_or(0),
            waves_count: map.get("wave").and_then(|s| s.parse().ok()).unwrap_or(0),
        };

        // 2. Content region: the temporary content mapper of the official
        // reader is irrelevant to the port (block ids are kept raw), so the
        // region is skipped.
        Self::skip_region(&mut zlib)?;

        // 3. Map region.
        let (width, height, tiles) = Self::read_map_region(&mut zlib, version)?;

        // 4. Entities region (bounded by its length prefix, like the
        // official `readRegion` length check).
        let entities_len = Self::checked_length(zlib.read_i32::<BigEndian>()?, "entities")?;
        let mut limited = zlib.take(entities_len as u64);
        let mut warnings = Vec::new();

        let team_plans = if version == 3 {
            Self::read_legacy_team_plans(&mut limited)?
        } else {
            Vec::new()
        };
        let (groups, skipped) = Self::read_legacy_entity_groups(&mut limited)?;
        if limited.limit() != 0 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                format!(
                    "legacy entities region length mismatch: {} bytes unread",
                    limited.limit()
                ),
            ));
        }
        if groups > 0 {
            warnings.push(format!(
                "unsupported legacy entity data: {skipped} world-entity chunks in {groups} group(s) preserved raw"
            ));
        }

        Ok(LegacySaveInfo {
            version,
            meta,
            width,
            height,
            tiles,
            team_plans,
            skipped_entity_chunks: skipped,
            warnings,
        })
    }

    /// `Save3.readEntities` team table: i32 team count, per team an i32 id,
    /// an i32 plan count and plans of x/y/rotation/block shorts plus a RAW
    /// i32 config (no TypeIO wrapper in v3).
    fn read_legacy_team_plans<R: Read>(reader: &mut R) -> Result<Vec<LegacyPlan>, Error> {
        let team_count = reader.read_i32::<BigEndian>()?;
        if !(0..=256).contains(&team_count) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid legacy team count",
            ));
        }
        let mut plans = Vec::new();
        for _ in 0..team_count {
            let team = reader.read_i32::<BigEndian>()?;
            let plan_count = reader.read_i32::<BigEndian>()?;
            if !(0..=1000).contains(&plan_count) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid legacy team plan count",
                ));
            }
            for _ in 0..plan_count {
                plans.push(LegacyPlan {
                    team,
                    x: reader.read_i16::<BigEndian>()?,
                    y: reader.read_i16::<BigEndian>()?,
                    rotation: reader.read_i16::<BigEndian>()?,
                    block: reader.read_i16::<BigEndian>()?,
                    config: reader.read_i32::<BigEndian>()?,
                });
            }
        }
        Ok(plans)
    }

    /// `readLegacyEntities` (Save1/Save2/Save3): u8 group count, per group an
    /// i32 amount and that many u16-prefixed entity chunks, which are skipped
    /// (their subclass payloads are not modeled).  Returns (groups, skipped).
    fn read_legacy_entity_groups<R: Read>(reader: &mut R) -> Result<(u8, usize), Error> {
        let groups = reader.read_u8()?;
        let mut skipped = 0usize;
        for _ in 0..groups {
            let amount = reader.read_i32::<BigEndian>()?;
            if !(0..=1_000_000).contains(&amount) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid legacy entity group size",
                ));
            }
            for _ in 0..amount {
                let chunk_len = reader.read_u16::<BigEndian>()? as usize;
                let mut chunk = vec![0u8; chunk_len];
                reader.read_exact(&mut chunk)?;
                skipped += 1;
            }
        }
        Ok((groups, skipped))
    }
}

/// Decoded header of a v1-v3 legacy building chunk (the `LegacySaveVersion`
/// chunk body `[u8 revision][u16 health][i8 packedrot][...]`).  The team is
/// the high nibble of `packedrot` (arc `Pack.leftByte`), or the following
/// byte when that nibble is 8; the rotation is the low nibble.  Returns the
/// header values and the offset where the module/subclass remainder starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegacyChunkHeader {
    pub revision: u8,
    /// Legacy health is an unsigned short (the modern format uses f32).
    pub health: f32,
    pub team: u8,
    pub rotation: u8,
    /// Offset in the chunk where the module/subclass payload begins.
    pub tail_start: usize,
}

pub fn decode_legacy_chunk_header(chunk: &[u8]) -> Option<LegacyChunkHeader> {
    if chunk.len() < 4 {
        return None;
    }
    let revision = chunk[0];
    let health = u16::from_be_bytes([chunk[1], chunk[2]]) as f32;
    let packedrot = chunk[3];
    let left = (packedrot >> 4) & 0xF;
    if left == 8 {
        // Teams with id >= 8 store the full team byte after packedrot.
        if chunk.len() < 5 {
            return None;
        }
        Some(LegacyChunkHeader {
            revision,
            health,
            team: chunk[4],
            rotation: packedrot & 0xF,
            tail_start: 5,
        })
    } else {
        Some(LegacyChunkHeader {
            revision,
            health,
            team: left,
            rotation: packedrot & 0xF,
            tail_start: 4,
        })
    }
}

// ---------------------------------------------------------------------------
// MSAV WRITER (SOL-008 step 1): serializes a save in the official v11
// container format (MAX_BASELINE_SAVE_VERSION; the desktop.jar 158.1 only
// ships Save1..Save11 — v12/v13 are NOT part of this baseline; the JSON
// runtime checkpoint "version 13" is the port's private format and is
// unrelated to SaveVersion). The whole file is a zlib (deflate) stream:
// header
// "MSAV" + i32 version + regions; each region is i32 length + data.
// Region "meta" is a StringMap (writeShort count + writeUTF key/value
// pairs, Java modified UTF-8). Verified byte-for-byte against the official
// desktop.jar reader.
// ---------------------------------------------------------------------------

/// Java `DataOutput.writeUTF` (modified UTF-8): u16 byte length + bytes.
/// ASCII-only values match standard UTF-8 exactly (the official server's
/// meta values are ASCII).
fn write_utf8(out: &mut Vec<u8>, value: &str) -> std::io::Result<()> {
    let bytes = value.as_bytes();
    let len = u16::try_from(bytes.len())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "writeUTF string too long"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

/// Official `SaveFileReader.writeStringMap`: writeShort size + UTF pairs.
fn write_string_map(out: &mut Vec<u8>, map: &HashMap<String, String>) -> std::io::Result<()> {
    out.extend_from_slice(&(map.len() as u16).to_be_bytes());
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for key in keys {
        write_utf8(out, key)?;
        write_utf8(out, &map[key])?;
    }
    Ok(())
}

/// Official `SaveIO.write` + `SaveVersion.writeRegion("meta")`:
/// zlib("MSAV" + i32 version + i32 meta_len + meta_string_map).
/// The produced bytes can be read back by `SaveIO::read_meta` and by the
/// official desktop.jar (SaveFileReader.readMeta).
pub fn write_msav_meta(map: &HashMap<String, String>, version: i32) -> std::io::Result<Vec<u8>> {
    let mut region = Vec::new();
    write_string_map(&mut region, map)?;
    let mut plain = Vec::new();
    plain.extend_from_slice(SAVE_HEADER);
    plain.extend_from_slice(&version.to_be_bytes());
    plain.extend_from_slice(&(region.len() as i32).to_be_bytes());
    plain.extend_from_slice(&region);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    encoder.write_all(&plain)?;
    encoder.finish()
}

/// Official `SaveVersion.writeContentHeader`: the content region maps every
/// mappable content type to its official names in id order. The port writes
/// the four types it fully registers (items, blocks, liquids, units) with
/// the exact v158.1 names from the jar-derived tables, so the official
/// reader maps the save's content ids identically.
pub fn write_msav_content_region() -> std::io::Result<Vec<u8>> {
    use crate::game::block_names::block_name_from_id;
    use crate::logic::{item_name_from_id, liquid_name_from_id, unit_name_from_id};
    let mut out = Vec::new();
    // writeByte(mappable) = 4 types.
    out.push(4);
    // items (ContentType.item ordinal 0), 22 entries.
    out.push(0);
    out.extend_from_slice(&22u16.to_be_bytes());
    for id in 0..22 {
        write_utf8(&mut out, item_name_from_id(id).unwrap_or_default())?;
    }
    // blocks (ContentType.block ordinal 1), 446 entries.
    out.push(1);
    out.extend_from_slice(&446u16.to_be_bytes());
    for id in 0..446 {
        write_utf8(&mut out, block_name_from_id(id).unwrap_or_default())?;
    }
    // liquids (ContentType.liquid ordinal 4), 11 entries.
    out.push(4);
    out.extend_from_slice(&11u16.to_be_bytes());
    for id in 0..11 {
        write_utf8(&mut out, liquid_name_from_id(id).unwrap_or_default())?;
    }
    // units (ContentType.unit ordinal 6), 69 entries (desktop 158.1 jar
    // dump: Vars.content.units() size). The port previously wrote 60 with
    // empty names for the ids its old table did not cover, which the
    // official reader cannot map; the unified registry fixes the names.
    out.push(6);
    out.extend_from_slice(&(crate::game::unit_types::UNIT_COUNT as u16).to_be_bytes());
    for id in 0..crate::game::unit_types::UNIT_COUNT as i16 {
        write_utf8(&mut out, unit_name_from_id(id).unwrap_or_default())?;
    }
    Ok(out)
}

/// Full save skeleton: zlib(header + version + meta region + content region
/// (+ patches region for v11+ in the official order)). The official reader
/// can parse the meta and content headers; the remaining regions
/// (map/entities/markers/custom) are still pending.
pub fn write_msav_skeleton(
    meta: &HashMap<String, String>,
    version: i32,
) -> std::io::Result<Vec<u8>> {
    let mut plain = Vec::new();
    plain.extend_from_slice(SAVE_HEADER);
    plain.extend_from_slice(&version.to_be_bytes());
    let meta_region = write_msav_meta_inner(meta, version)?;
    plain.extend_from_slice(&(meta_region.len() as i32).to_be_bytes());
    plain.extend_from_slice(&meta_region);
    // In Save12+ (official SaveVersion.write), patches precedes content.
    if version >= 12 {
        let patches = write_msav_content_patches_region(version)?;
        plain.extend_from_slice(&(patches.len() as i32).to_be_bytes());
        plain.extend_from_slice(&patches);
    }
    let content_region = write_msav_content_region()?;
    plain.extend_from_slice(&(content_region.len() as i32).to_be_bytes());
    plain.extend_from_slice(&content_region);
    // In Save11, patches followed content.
    if version == 11 {
        let patches = write_msav_content_patches_region(version)?;
        plain.extend_from_slice(&(patches.len() as i32).to_be_bytes());
        plain.extend_from_slice(&patches);
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    encoder.write_all(&plain)?;
    encoder.finish()
}

/// The meta region payload (string map) without the outer length prefix.
fn write_msav_meta_inner(map: &HashMap<String, String>, _version: i32) -> std::io::Result<Vec<u8>> {
    let mut region = Vec::new();
    write_string_map(&mut region, map)?;
    Ok(region)
}

/// Official `SaveVersion.writeMap`: width/height shorts, floor+overlay RLE
/// (writeShort floor, writeShort overlay, writeByte run-1), then blocks
/// (writeShort block id, writeByte packed, optional savedata bytes). The
/// port serializes the base map without building entities (packed 0); the
/// official reader reconstructs the terrain.
pub fn write_msav_map_region(
    width: usize,
    height: usize,
    floors: &[i16],
    overlays: &[i16],
    blocks: &[i16],
    dynamic_tiles: &dashmap::DashMap<i32, crate::network::world::DynamicTile>,
    version: i32,
) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.extend_from_slice(&(height as u16).to_be_bytes());
    let total = width * height;
    // floors + overlays with consecutive-run compression.
    let mut i = 0usize;
    while i < total {
        let floor = floors.get(i).copied().unwrap_or(0);
        let overlay = overlays.get(i).copied().unwrap_or(0);
        let mut consecutives = 0usize;
        while i + 1 + consecutives < total && consecutives < 255 {
            let nf = floors.get(i + 1 + consecutives).copied().unwrap_or(0);
            let no = overlays.get(i + 1 + consecutives).copied().unwrap_or(0);
            if nf != floor || no != overlay {
                break;
            }
            consecutives += 1;
        }
        out.extend_from_slice(&(floor as u16).to_be_bytes());
        out.extend_from_slice(&(overlay as u16).to_be_bytes());
        out.push(consecutives as u8);
        i += 1 + consecutives;
    }
    // Blocks with run compression; built tiles carry their Building entity.
    // Multiblock footprints must match SaveVersion.writeMap: only the center
    // writes a building chunk; occupied non-center cells still carry
    // hadEntity + isCenter=false so a later air run cannot `setBlock(air)`
    // over the footprint (desktop.jar 158.1 endMapLoad NPEs on null tiles).
    let occupancy = map_building_occupancy(dynamic_tiles);
    let mut i = 0usize;
    while i < total {
        let x = i as i32 % width as i32;
        let y = i as i32 / width as i32;
        let pos = (x << 16) | y;
        if let Some(cell) = occupancy.get(&pos) {
            out.extend_from_slice(&(cell.block as u16).to_be_bytes());
            out.push(1); // hadEntity
            out.push(u8::from(cell.is_center));
            if cell.is_center {
                let chunk = write_msav_building_chunk_for_version(&cell.tile, version)?;
                out.extend_from_slice(&chunk);
            }
            i += 1;
            continue;
        }
        let block = blocks.get(i).copied().unwrap_or(0);
        out.extend_from_slice(&(block as u16).to_be_bytes());
        out.push(0); // packed: no build, no savedata
        let mut consecutives = 0usize;
        while i + 1 + consecutives < total && consecutives < 255 {
            let next = i + 1 + consecutives;
            let nx = next as i32 % width as i32;
            let ny = next as i32 / width as i32;
            let npos = (nx << 16) | ny;
            if occupancy.contains_key(&npos) {
                break;
            }
            if blocks.get(next).copied().unwrap_or(0) != block {
                break;
            }
            consecutives += 1;
        }
        out.push(consecutives as u8);
        i += 1 + consecutives;
    }
    Ok(out)
}

struct MapOccupancyCell {
    is_center: bool,
    block: i16,
    tile: crate::network::world::DynamicTile,
}

fn map_building_occupancy(
    dynamic_tiles: &dashmap::DashMap<i32, crate::network::world::DynamicTile>,
) -> HashMap<i32, MapOccupancyCell> {
    let mut occupancy = HashMap::new();
    for entry in dynamic_tiles.iter() {
        let tile = entry.value();
        if tile.block == 0 {
            continue;
        }
        for cell in building_save_footprint(tile.position, tile.block) {
            let is_center = cell == tile.position;
            if is_center {
                occupancy.insert(
                    cell,
                    MapOccupancyCell {
                        is_center: true,
                        block: tile.block,
                        tile: tile.clone(),
                    },
                );
            } else {
                occupancy.entry(cell).or_insert(MapOccupancyCell {
                    is_center: false,
                    block: tile.block,
                    tile: tile.clone(),
                });
            }
        }
    }
    occupancy
}

fn building_save_footprint(position: i32, block: i16) -> Vec<i32> {
    let size = i32::from(crate::game::content::block_size(block)).max(1);
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    let offset = -(size - 1) / 2;
    let mut cells = Vec::with_capacity((size * size) as usize);
    for dy in 0..size {
        for dx in 0..size {
            let px = x + offset + dx;
            let py = y + offset + dy;
            cells.push((px << 16) | (py as u16 as i32));
        }
    }
    cells
}

/// Full save with meta + content + map regions (still missing entities).
pub fn write_msav_with_map(
    meta: &HashMap<String, String>,
    version: i32,
    width: usize,
    height: usize,
    floors: &[i16],
    overlays: &[i16],
    blocks: &[i16],
) -> std::io::Result<Vec<u8>> {
    let mut plain = Vec::new();
    plain.extend_from_slice(SAVE_HEADER);
    plain.extend_from_slice(&version.to_be_bytes());
    let meta_region = write_msav_meta_inner(meta, version)?;
    plain.extend_from_slice(&(meta_region.len() as i32).to_be_bytes());
    plain.extend_from_slice(&meta_region);
    let content_region = write_msav_content_region()?;
    plain.extend_from_slice(&(content_region.len() as i32).to_be_bytes());
    plain.extend_from_slice(&content_region);
    let map_region = write_msav_map_region(
        width,
        height,
        floors,
        overlays,
        blocks,
        &dashmap::DashMap::new(),
        version,
    )?;
    plain.extend_from_slice(&(map_region.len() as i32).to_be_bytes());
    plain.extend_from_slice(&map_region);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    encoder.write_all(&plain)?;
    encoder.finish()
}

/// Official `SaveVersion.writeEntities`: entity mapping (writeShort count,
/// no mods -> 0), team blocks (writeInt team count + per team: writeInt id,
/// writeInt plan count + x/y/rotation/block shorts + TypeIO config), and
/// world entities (writeInt count followed by bounded class/id/build chunks).
pub fn write_msav_entities_region(
    team_blocks: Option<&crate::engine::typeio::TeamBlocks>,
    dynamic_tiles: Option<&dashmap::DashMap<i32, crate::network::world::DynamicTile>>,
) -> std::io::Result<Vec<u8>> {
    write_msav_entities_region_with_units(team_blocks, dynamic_tiles, &[], &[], None)
}

/// Entity-region writer including live units and puddles. The public
/// two-argument wrapper above remains useful for callers that only have
/// buildings.
fn write_msav_entities_region_with_units(
    team_blocks: Option<&crate::engine::typeio::TeamBlocks>,
    _dynamic_tiles: Option<&dashmap::DashMap<i32, crate::network::world::DynamicTile>>,
    enemy_units: &[crate::network::world::EnemyUnit],
    puddles: &[(i32, f32, i16, i32)],
    runtime: Option<&crate::network::world::DynamicWorld>,
) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes()); // entity mapping (no mods)
                                                // Team blocks (official writeTeamBlocks): only teams with plans are
                                                // serialized (TeamData.plans non-empty), mirroring the active-team list.
    let teams: Vec<&crate::engine::typeio::TeamPlans> = team_blocks
        .map(|tb| tb.teams.iter().filter(|t| !t.plans.is_empty()).collect())
        .unwrap_or_default();
    out.extend_from_slice(&(teams.len() as i32).to_be_bytes());
    for team in teams {
        out.extend_from_slice(&team.team.to_be_bytes());
        out.extend_from_slice(&(team.plans.len() as i32).to_be_bytes());
        for plan in &team.plans {
            out.extend_from_slice(&plan.x.to_be_bytes());
            out.extend_from_slice(&plan.y.to_be_bytes());
            out.extend_from_slice(&plan.rotation.to_be_bytes());
            out.extend_from_slice(&plan.block.to_be_bytes());
            crate::engine::typeio::write_object_bytes(&mut out, &plan.config)?;
        }
    }
    // SaveVersion.writeWorldEntities: only entities with serialize()==true.
    // BuildingComp is @EntityDef(serialize=false), so map buildings live
    // exclusively in the map region — never as class-6 world entities.
    out.extend_from_slice(&((enemy_units.len() + puddles.len()) as i32).to_be_bytes());
    for unit in enemy_units {
        // Most legacy unit subclasses have additional tails which the port
        // does not model.  UnitEntity (3) and MechUnit (4) are emitted with
        // their exact v11 write layouts; the others intentionally use the
        // strict, self-contained UnitEntity layout as a compatibility
        // fallback.  This still preserves every modeled EnemyUnit field and
        // lets the official reader consume the complete bounded chunk.
        let class_id = match unit.entity_class {
            3 | 4 | 5 | 23 | 26 => unit.entity_class,
            _ => 3,
        };
        let body = write_unit_entity_body(unit, class_id, runtime)?;
        let mut chunk = Vec::with_capacity(5 + body.len());
        chunk.push(class_id);
        chunk.extend_from_slice(&unit.id.to_be_bytes());
        chunk.extend_from_slice(&body);
        out.extend_from_slice(&(chunk.len() as i32).to_be_bytes());
        out.extend_from_slice(&chunk);
    }
    // Puddles (round 73 A2): `SaveVersion.writeWorldEntities` includes every
    // entity with `serialize()==true`, and `Puddle.serialize()` returns true.
    // Chunk = [b classId 13][i entity id][Puddle.write] with
    // `Puddle.write(Writes)` = s 1 (rev), f amount, s liquid.id, i tile.pos
    // (TypeIO.writeTile), f x, f y — all verified in the desktop.jar 158.1
    // bytecode. x/y are the tile center world coordinates.
    for (position, amount, liquid, entity_id) in puddles {
        let mut chunk = Vec::with_capacity(24);
        chunk.push(13); // mindustry.gen.Puddle.classId()
        chunk.extend_from_slice(&entity_id.to_be_bytes());
        chunk.extend_from_slice(&1i16.to_be_bytes()); // Puddle.write revision
        chunk.extend_from_slice(&amount.to_be_bytes());
        chunk.extend_from_slice(&liquid.to_be_bytes());
        chunk.extend_from_slice(&position.to_be_bytes());
        let x = (*position >> 16) as f32 * 8.0 + 4.0;
        let y = (*position & 0xFFFF) as f32 * 8.0 + 4.0;
        chunk.extend_from_slice(&x.to_be_bytes());
        chunk.extend_from_slice(&y.to_be_bytes());
        out.extend_from_slice(&(chunk.len() as i32).to_be_bytes());
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Serialize the class-specific unit save body used by desktop.jar 158.1.
/// UnitEntity (3) and MechUnit (4) write revision 9; PayloadUnit (5) writes
/// revision 7 and inserts the payload list before the plan queue. Confirmed
/// with `javap` on the 158.1 jar (`UnitEntity.write` / `PayloadUnit.write`).
pub(crate) fn write_unit_entity_body(
    unit: &crate::network::world::EnemyUnit,
    class_id: u8,
    runtime: Option<&crate::network::world::DynamicWorld>,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let revision: i16 = match class_id {
        5 | 17 | 18 | 26 => 7,
        16 | 21 | 23 => 8,
        0 | 19 | 29 | 30 | 31 | 32 | 33 => 5,
        43 | 45 | 46 => 2,
        _ => 9,
    };
    let mut out = Vec::with_capacity(120);
    out.extend_from_slice(&revision.to_be_bytes());
    out.push(0); // TypeIO.writeAbilities: no ability data is modeled
    if class_id == 4 {
        out.extend_from_slice(&unit.rotation.to_be_bytes()); // MechUnit.baseRotation
    }
    crate::network::units::controller::write_unit_controller_sync(&mut out, runtime, unit)?;
    out.extend_from_slice(&unit.elevation.to_be_bytes());
    out.extend_from_slice(&unit.flag.to_be_bytes());
    out.extend_from_slice(&unit.health.to_be_bytes());
    out.push(0); // isShooting
    out.extend_from_slice(&(-1i32).to_be_bytes()); // mineTile: null
    out.push(0); // mounts
    if matches!(class_id, 5 | 23 | 26) {
        out.write_i(i32::try_from(unit.payloads.len()).unwrap_or(i32::MAX))?;
        for carried in &unit.payloads {
            crate::network::units::controller::write_carried_payload(&mut out, carried)?;
        }
    }
    crate::network::wire::write_unit_plans_queue(&mut out, &unit.build_plans, false)?;
    out.extend_from_slice(&unit.rotation.to_be_bytes());
    out.extend_from_slice(&unit.shield.to_be_bytes());
    out.push(0); // spawnedByCore

    // UnitEntity has one ItemStack (not an arbitrary item list).  Preserve
    // the first positive stack; additional logic-carried stacks are outside
    // the official v11 representation and are intentionally omitted.
    let item = unit.items.iter().find(|(_, amount)| *amount > 0);
    out.extend_from_slice(&item.map(|(id, _)| *id).unwrap_or(-1).to_be_bytes());
    out.extend_from_slice(&item.map(|(_, amount)| *amount).unwrap_or(0).to_be_bytes());

    // P1: the official wire is a StatusEntry collection
    // (`i statusCount + [s statusId + f duration]`); emit the authoritative
    // collection when present, otherwise the legacy single status.
    if !unit.statuses.is_empty() {
        out.extend_from_slice(&(unit.statuses.len() as i32).to_be_bytes());
        for entry in &unit.statuses {
            out.extend_from_slice(&entry.effect.to_be_bytes());
            out.extend_from_slice(&entry.time.to_be_bytes());
        }
    } else if unit.status_effect >= 0 {
        out.extend_from_slice(&1i32.to_be_bytes());
        out.extend_from_slice(&unit.status_effect.to_be_bytes());
        out.extend_from_slice(&unit.status_duration.to_be_bytes());
    } else {
        out.extend_from_slice(&0i32.to_be_bytes());
    }
    out.push(unit.team);
    out.extend_from_slice(&unit.unit_type.to_be_bytes());
    out.push(u8::from(unit.update_building)); // updateBuilding
    out.extend_from_slice(&unit.velocity_x.to_be_bytes());
    out.extend_from_slice(&unit.velocity_y.to_be_bytes());
    out.extend_from_slice(&unit.x.to_be_bytes());
    out.extend_from_slice(&unit.y.to_be_bytes());
    Ok(out)
}

/// Official `MapMarkers.write`: JsonIO.writeBytes of the objective markers
/// map. Empty markers serialize as `{}` (arc JsonIO of an empty IntMap).
pub fn write_msav_markers_region() -> std::io::Result<Vec<u8>> {
    Ok(b"{}".to_vec())
}

/// Official `writeCustomChunks`: writeInt(0) — no custom chunks.
pub fn write_msav_custom_region() -> std::io::Result<Vec<u8>> {
    Ok(0i32.to_be_bytes().to_vec())
}

/// Official empty `writeContentPatches` / `writeDataPatches`.
///
/// - Save11: `writeByte(0)`
/// - Save12 reader (`Save12.readDataPatches`): ignored version int + patchAmount 0 + imageAmount 0 (12 bytes)
/// - Save13 writer (`SaveVersion.writeDataPatches`): `patchFormatVersion=2` + total 0 (8 bytes)
///
/// The official 159.7 JAR never writes version 12 (`SaveIO.getVersion()` is Save13).
pub fn write_msav_content_patches_region(version: i32) -> std::io::Result<Vec<u8>> {
    match version {
        v if v >= 13 => Ok(vec![0, 0, 0, 2, 0, 0, 0, 0]),
        12 => Ok(vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        _ => Ok(vec![0]),
    }
}

/// Complete v11-v13 save with every official region: meta, content, patches,
/// map, entities, markers, custom. The official reader can load this save
/// (SaveIO.load): the world is reconstructed with the map terrain and no
/// live entities or buildings.
/// World data for `write_msav_complete`.
pub struct MsavWorld<'a> {
    pub width: usize,
    pub height: usize,
    pub floors: &'a [i16],
    pub overlays: &'a [i16],
    pub blocks: &'a [i16],
    pub team_blocks: Option<&'a crate::engine::typeio::TeamBlocks>,
    /// Built tiles (block != 0) serialized as Building entities in the map.
    pub dynamic_tiles: &'a dashmap::DashMap<i32, crate::network::world::DynamicTile>,
    /// Live hostile units serialized after buildings in the entities region.
    pub enemy_units: &'a [crate::network::world::EnemyUnit],
    /// Authoritative puddles (round 73): `(position, amount, liquid, entity_id)`.
    pub puddles: &'a [(i32, f32, i16, i32)],
    /// Live world used to emit TypeIO controllers (CommandAI/LogicAI/Player).
    pub runtime: Option<&'a crate::network::world::DynamicWorld>,
}

pub fn write_msav_complete(
    meta: &HashMap<String, String>,
    version: i32,
    world: &MsavWorld<'_>,
) -> std::io::Result<Vec<u8>> {
    let mut plain = Vec::new();
    plain.extend_from_slice(SAVE_HEADER);
    plain.extend_from_slice(&version.to_be_bytes());
    let meta_region = write_msav_meta_inner(meta, version)?;
    let content_region = write_msav_content_region()?;
    let patches_region = write_msav_content_patches_region(version)?;
    let map_region = write_msav_map_region(
        world.width,
        world.height,
        world.floors,
        world.overlays,
        world.blocks,
        world.dynamic_tiles,
        version,
    )?;
    let entities_region = write_msav_entities_region_with_units(
        world.team_blocks,
        Some(world.dynamic_tiles),
        world.enemy_units,
        world.puddles,
        world.runtime,
    )?;
    let markers_region = write_msav_markers_region()?;
    let custom_region = write_msav_custom_region()?;
    // Official region order per version (SaveVersion.read + versions/Save*.java,
    // mirrored by world_stream::extract_msav_regions):
    //   v1-6   meta, content, map, entities
    //   v7     meta, content, map, entities, custom
    //   v8-10  meta, content, map, entities, markers, custom
    //   v11    meta, content, patches, map, entities, markers, custom
    //   v12+   meta, patches, content, map, entities, markers, custom
    let regions: Vec<Vec<u8>> = match version {
        1..=6 => vec![meta_region, content_region, map_region, entities_region],
        7 => vec![
            meta_region,
            content_region,
            map_region,
            entities_region,
            custom_region,
        ],
        8..=10 => vec![
            meta_region,
            content_region,
            map_region,
            entities_region,
            markers_region,
            custom_region,
        ],
        11 => vec![
            meta_region,
            content_region,
            patches_region,
            map_region,
            entities_region,
            markers_region,
            custom_region,
        ],
        _ => vec![
            meta_region,
            patches_region,
            content_region,
            map_region,
            entities_region,
            markers_region,
            custom_region,
        ],
    };
    for region in regions {
        plain.extend_from_slice(&(region.len() as i32).to_be_bytes());
        plain.extend_from_slice(&region);
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    use std::io::Write;
    encoder.write_all(&plain)?;
    encoder.finish()
}

/// Module flags from the v158.1 content registry.  A DynamicTile can also
/// carry a module on a block which normally has no such module (for example,
/// an item can be injected into a wall by the server), hence the `!empty`
/// checks below in addition to these official block flags.
const OFFICIAL_ITEM_BLOCKS: &[i16] = &[
    181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 199, 200,
    201, 202, 203, 204, 205, 210, 211, 212, 213, 214, 215, 245, 246, 247, 248, 249, 253, 257, 258,
    259, 260, 262, 263, 266, 267, 268, 269, 270, 271, 272, 273, 274, 275, 276, 277, 278, 279, 280,
    281, 282, 308, 310, 311, 312, 315, 316, 324, 325, 326, 327, 328, 330, 331, 332, 333, 334, 335,
    336, 337, 338, 339, 340, 341, 342, 343, 344, 345, 346, 347, 348, 349, 350, 351, 352, 357, 358,
    361, 362, 363, 364, 365, 367, 368, 370, 371, 374, 375, 377, 378, 379, 380, 381, 382, 383, 386,
    387, 388, 389, 390, 391, 392, 393, 394, 395, 404, 405, 406, 407, 408, 409, 412, 418, 421, 422,
    423, 425, 426, 427, 428,
];
const OFFICIAL_POWER_BLOCKS: &[i16] = &[
    182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200,
    201, 202, 203, 210, 211, 212, 213, 214, 244, 245, 246, 247, 248, 249, 251, 252, 253, 254, 255,
    256, 263, 271, 279, 280, 281, 284, 285, 294, 302, 303, 304, 306, 307, 308, 309, 310, 311, 312,
    313, 314, 315, 316, 317, 318, 319, 320, 321, 322, 323, 324, 327, 328, 329, 330, 331, 332, 333,
    334, 335, 336, 337, 338, 354, 355, 356, 359, 364, 366, 372, 373, 376, 377, 378, 379, 380, 381,
    382, 383, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 402, 403, 404,
    405, 406, 407, 408, 409, 410, 411, 419, 420, 421, 422, 423, 425, 426, 428,
];
const OFFICIAL_LIQUID_BLOCKS: &[i16] = &[
    182, 186, 189, 192, 193, 194, 195, 197, 198, 200, 201, 202, 204, 209, 211, 212, 213, 214, 215,
    249, 252, 253, 254, 281, 283, 284, 285, 286, 287, 288, 289, 290, 291, 292, 293, 294, 295, 296,
    297, 298, 299, 300, 301, 310, 311, 315, 316, 320, 321, 322, 323, 324, 325, 326, 327, 328, 329,
    330, 331, 332, 334, 335, 336, 337, 338, 349, 350, 351, 352, 353, 354, 355, 357, 358, 360, 361,
    362, 363, 364, 365, 366, 367, 368, 369, 370, 371, 373, 374, 375, 382, 383, 385, 389, 390, 391,
    392, 393, 394, 395, 397, 408, 409, 414, 415, 426, 427, 433,
];

fn has_official_module(block: i16, modules: &[i16]) -> bool {
    modules.binary_search(&block).is_ok()
}

fn is_storage_block(block: i16) -> bool {
    // 345..348 are container/vault variants in v158.1.  Keep the two legacy
    // ids as storage aliases too: old maps can retain those ids in DynamicTile
    // data, and setting the bit is read-valid even when the current registry
    // resolves the id to a wall.
    matches!(block, 158 | 159 | 345..=348)
}

fn building_version(block: i16) -> u8 {
    // Leading chunk byte is `Building.version()` of the *subclass*, used as
    // `read(Reads, byte revision)`. Confirmed against desktop.jar 158.1:
    // Building.version()=0 (no-op read), GeneratorBuild=1, LogicBuild=4.
    // The writeBase format version (the byte *inside* writeAll) stays 3.
    match block {
        308..=323 => 1,
        431..=433 | 442 => 4,
        _ => 0,
    }
}

/// Official Building.writeBase (v11): health f32, rotation|0x80 byte, team
/// byte, version byte (3), enabled byte, moduleBitmask byte, then the present
/// modules and efficiency bytes.  `writeAll` additionally emits the
/// block-specific tail after this base.
fn write_building_base(
    out: &mut Vec<u8>,
    tile: &crate::network::world::DynamicTile,
) -> std::io::Result<()> {
    out.extend_from_slice(&tile.health.to_be_bytes());
    out.push((tile.rotation & 0x7f) | 0x80);
    out.push(tile.team);
    out.push(3); // no fog: base format version
    out.push(u8::from(tile.enabled));
    let has_items = !tile.inventory.is_empty()
        || is_storage_block(tile.block)
        || has_official_module(tile.block, OFFICIAL_ITEM_BLOCKS);
    let has_power = tile.power_stored > 0.0
        || !tile.power_links.is_empty()
        || has_official_module(tile.block, OFFICIAL_POWER_BLOCKS);
    let has_liquids =
        tile.stored_liquid >= 0 || has_official_module(tile.block, OFFICIAL_LIQUID_BLOCKS);
    let bitmask =
        u8::from(has_items) | (u8::from(has_power) << 1) | (u8::from(has_liquids) << 2) | (1 << 3); // consume module is always present in v11
    out.push(bitmask);
    if has_items {
        let entries: Vec<(i16, i32)> = tile
            .inventory
            .iter()
            .copied()
            .filter(|(_, amount)| *amount > 0)
            .collect();
        out.extend_from_slice(
            &(u16::try_from(entries.len())
                .map_err(|_| Error::new(ErrorKind::InvalidInput, "too many item entries"))?)
            .to_be_bytes(),
        );
        for (item, amount) in entries {
            out.extend_from_slice(&item.to_be_bytes());
            out.extend_from_slice(&amount.to_be_bytes());
        }
    }
    if has_power {
        // PowerModule.write: u16 link count, packed tile positions, f32 status.
        let links = &tile.power_links;
        let count = u16::try_from(links.len())
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "too many power links"))?;
        out.extend_from_slice(&count.to_be_bytes());
        for link in links {
            out.extend_from_slice(&link.to_be_bytes());
        }
        out.extend_from_slice(&tile.power_stored.to_be_bytes());
    }
    if has_liquids {
        let liquids: Vec<(i16, f32)> = if !tile.liquid_inventory.is_empty() {
            tile.liquid_inventory
                .iter()
                .copied()
                .filter(|(_, amount)| amount.is_finite() && *amount > 0.0)
                .collect()
        } else if tile.stored_liquid >= 0 && tile.liquid_amount > 0.0 {
            vec![(tile.stored_liquid, tile.liquid_amount)]
        } else {
            Vec::new()
        };
        let count = u16::try_from(liquids.len())
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "too many liquid entries"))?;
        out.extend_from_slice(&count.to_be_bytes());
        for (liquid, amount) in liquids {
            out.extend_from_slice(&liquid.to_be_bytes());
            out.extend_from_slice(&amount.to_be_bytes());
        }
    }
    // DynamicWorld does not model time scaling/disablers, so their optional
    // module bits remain clear and these are full efficiency values.
    out.push(255);
    out.push(255);
    Ok(())
}

fn write_building_tail(
    out: &mut Vec<u8>,
    tile: &crate::network::world::DynamicTile,
) -> std::io::Result<()> {
    // PowerGenerator.GeneratorBuild.write(): productionEfficiency and
    // generateTime. ConsumeGenerator (combustion/thermal/steam/etc.) and
    // SolarGenerator inherit this exact tail. DynamicTile does not track
    // these animation values, so the official defaults (zero) are valid.
    let generator = matches!(tile.block, 308..=314 | 320..=323);
    if generator {
        out.extend_from_slice(&0.0f32.to_be_bytes());
        out.extend_from_slice(&0.0f32.to_be_bytes());
    }
    // Nuclear/impact reactors append one value to GeneratorBuild; the
    // variable reactor appends three values (heat, instability, warmup).
    match tile.block {
        315 | 316 => out.extend_from_slice(&0.0f32.to_be_bytes()),
        323 => {
            out.extend_from_slice(&0.0f32.to_be_bytes());
            out.extend_from_slice(&0.0f32.to_be_bytes());
            out.extend_from_slice(&0.0f32.to_be_bytes());
        }
        431..=433 | 442 => write_logic_build_tail(out, tile)?,
        398..=401 => write_payload_conveyor_tail(out, tile)?,
        _ => {
            if let Some(payload) = tile.payload.as_deref() {
                // PayloadBlock.write: payVector.x/y, payRotation, Payload.write.
                // Used by constructors / payload sources that inherit PayloadBlock.
                out.extend_from_slice(&0.0f32.to_be_bytes());
                out.extend_from_slice(&0.0f32.to_be_bytes());
                out.extend_from_slice(&tile.payload_rotation.to_be_bytes());
                crate::network::units::controller::write_carried_payload(out, payload)?;
            }
        }
    }
    Ok(())
}

/// LogicBuild.write (desktop.jar 158.1, revision 4): compressed program,
/// non-null vars, unused memory count, optional privileged ipt, tag/icon,
/// wait timers, accumulator.
fn write_logic_build_tail(
    out: &mut Vec<u8>,
    tile: &crate::network::world::DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    let compressed = crate::network::buildings::config::logic_payload(&tile.config)
        .map(|payload| payload.to_vec())
        .unwrap_or_default();
    out.write_i(i32::try_from(compressed.len()).unwrap_or(i32::MAX))?;
    out.extend_from_slice(&compressed);
    out.write_i(0)?; // no executor vars (recompiled from source on load)
    out.write_i(0)?; // memory unused
    if tile.block == 442 {
        out.write_s(8)?; // world-processor default ipt
    }
    out.write_typeio_string(None)?;
    out.write_s(0)?; // iconTag
    out.write_us(0)?; // wait count
    out.write_f(0.0)?; // accumulator
    Ok(())
}

/// PayloadConveyorBuild.write (158.1): progress, itemRotation, Payload.write.
fn write_payload_conveyor_tail(
    out: &mut Vec<u8>,
    tile: &crate::network::world::DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    out.write_f(tile.payload_progress)?;
    out.write_f(tile.payload_rotation)?;
    match tile.payload.as_deref() {
        Some(payload) => crate::network::units::controller::write_carried_payload(out, payload)?,
        None => out.write_bool(false)?,
    }
    Ok(())
}

/// Building.writeAll body (without the map's leading Building.version byte).
pub(crate) fn write_building_all_body(
    tile: &crate::network::world::DynamicTile,
) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    write_building_base(&mut body, tile)?;
    write_building_tail(&mut body, tile)?;
    Ok(body)
}

/// Official writeMap tile-with-build: packed 1, center boolean true, then a
/// chunk (int length) of [Building.version + Building.writeAll].
pub fn write_msav_building_chunk(
    tile: &crate::network::world::DynamicTile,
) -> std::io::Result<Vec<u8>> {
    write_msav_building_chunk_for_version(tile, 11)
}

/// Modern (SaveVersion / v10+) building chunk: i32 length prefix.
/// v4-v9 legacy readers expect a u16 prefix instead (round-73 M1).
pub fn write_msav_building_chunk_for_version(
    tile: &crate::network::world::DynamicTile,
    version: i32,
) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    body.push(building_version(tile.block));
    body.extend_from_slice(&write_building_all_body(tile)?);
    let mut chunk = Vec::new();
    if (4..=9).contains(&version) {
        // ShortChunkSaveVersion.readLegacyShortChunk = readUnsignedShort.
        let len = u16::try_from(body.len())
            .map_err(|_| Error::new(ErrorKind::InvalidData, "building chunk too large"))?;
        chunk.extend_from_slice(&len.to_be_bytes());
    } else {
        chunk.extend_from_slice(&(body.len() as i32).to_be_bytes());
    }
    chunk.extend_from_slice(&body);
    Ok(chunk)
}

/// Apply a map-region building subclass tail (`extra_data`) onto a live tile.
/// Logic processors restore their compressed program as a TypeIO byte[]
/// config; payload conveyors restore the carried payload.
pub fn apply_msav_building_tail(
    tile: &mut crate::network::world::DynamicTile,
    extra: &[u8],
) -> std::io::Result<()> {
    if extra.is_empty() {
        return Ok(());
    }
    match tile.block {
        431..=433 | 442 => apply_logic_build_tail(tile, extra),
        398..=401 => apply_payload_conveyor_tail(tile, extra),
        _ => {
            if extra.len() >= 13 {
                // PayloadBlock.write: 3 floats + Payload.write.
                apply_payload_block_tail(tile, extra)?;
            }
            Ok(())
        }
    }
}

fn apply_logic_build_tail(
    tile: &mut crate::network::world::DynamicTile,
    extra: &[u8],
) -> std::io::Result<()> {
    use crate::network::codec::Reads;
    let mut cursor = std::io::Cursor::new(extra);
    let len = cursor.read_i()?;
    if len < 0 {
        return Ok(());
    }
    let len = len as usize;
    if cursor.position() as usize + len > extra.len() {
        return Ok(());
    }
    let start = cursor.position() as usize;
    let compressed = extra[start..start + len].to_vec();
    if crate::network::buildings::config::valid_logic_payload(&compressed) {
        let mut config = vec![crate::network::buildings::config::TYPEIO_BYTE_ARRAY];
        config.extend_from_slice(&(compressed.len() as i32).to_be_bytes());
        config.extend_from_slice(&compressed);
        tile.config = config;
    }
    Ok(())
}

fn apply_payload_conveyor_tail(
    tile: &mut crate::network::world::DynamicTile,
    extra: &[u8],
) -> std::io::Result<()> {
    use crate::network::codec::Reads;
    let mut cursor = std::io::Cursor::new(extra);
    tile.payload_progress = cursor.read_f().unwrap_or(0.0);
    tile.payload_rotation = cursor.read_f().unwrap_or(0.0);
    if let Ok(Some(payload)) = read_carried_payload(&mut cursor) {
        tile.payload = Some(Box::new(payload));
    }
    Ok(())
}

fn apply_payload_block_tail(
    tile: &mut crate::network::world::DynamicTile,
    extra: &[u8],
) -> std::io::Result<()> {
    use crate::network::codec::Reads;
    let mut cursor = std::io::Cursor::new(extra);
    let _ = cursor.read_f()?;
    let _ = cursor.read_f()?;
    tile.payload_rotation = cursor.read_f().unwrap_or(0.0);
    if let Ok(Some(payload)) = read_carried_payload(&mut cursor) {
        tile.payload = Some(Box::new(payload));
    }
    Ok(())
}

/// `Payload.read` / `TypeIO.readPayload` (desktop.jar 158.1).
pub(crate) fn read_carried_payload<R: crate::network::codec::Reads + std::io::Seek>(
    input: &mut R,
) -> std::io::Result<Option<crate::network::world::CarriedPayload>> {
    use crate::network::codec::Reads;
    use crate::network::world::{CarriedBuildPayload, CarriedPayload, DynamicTile};
    if !input.read_bool()? {
        return Ok(None);
    }
    match input.read_b()? {
        0 => {
            let class_id = input.read_b()?;
            let unit = read_unit_write(input, class_id)?;
            Ok(Some(CarriedPayload::Unit(unit)))
        }
        1 => {
            let block = input.read_s()?;
            let version = input.read_b()?;
            let mut tile = DynamicTile {
                block,
                health: crate::game::content::block_health(block).max(1.0),
                ..DynamicTile::default()
            };
            read_write_base(input, &mut tile)?;
            Ok(Some(CarriedPayload::Build(CarriedBuildPayload {
                tile,
                version,
                sync: Vec::new(),
            })))
        }
        _ => Ok(None),
    }
}

fn read_write_base<R: crate::network::codec::Reads>(
    input: &mut R,
    tile: &mut crate::network::world::DynamicTile,
) -> std::io::Result<()> {
    use crate::network::codec::Reads;
    tile.health = input.read_f()?;
    let raw_rot = input.read_b()?;
    tile.rotation = raw_rot & 0x7f;
    tile.team = input.read_b()?;
    if raw_rot & 0x80 == 0 {
        return Ok(());
    }
    let version = input.read_b()?;
    if version >= 1 {
        tile.enabled = input.read_b()? == 1;
    }
    let bits = if version >= 2 { input.read_b()? } else { 0 };
    if bits & 1 != 0 {
        let count = usize::from(input.read_us()?);
        for _ in 0..count {
            let item = input.read_s()?;
            let amount = input.read_i()?;
            if item >= 0 && amount > 0 {
                tile.inventory.push((item, amount));
            }
        }
    }
    if bits & 2 != 0 {
        let count = usize::from(input.read_us()?);
        for _ in 0..count {
            tile.power_links.push(input.read_i()?);
        }
        tile.power_stored = input.read_f()?;
    }
    if bits & 4 != 0 {
        let count = usize::from(input.read_us()?);
        for _ in 0..count {
            let liquid = input.read_s()?;
            let amount = input.read_f()?;
            if liquid >= 0 && amount.is_finite() && amount > 0.0 {
                tile.liquid_inventory.push((liquid, amount));
            }
        }
        if let Some((id, amount)) = tile
            .liquid_inventory
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .copied()
        {
            tile.stored_liquid = id;
            tile.liquid_amount = amount;
        }
    }
    if bits & 16 != 0 {
        let _ = input.read_f()?;
        let _ = input.read_f()?;
    }
    if bits & 32 != 0 {
        let _ = input.read_f()?;
    }
    if version <= 2 {
        let _ = input.read_b()?;
    }
    if version >= 3 {
        let _ = input.read_b()?;
        let _ = input.read_b()?;
    }
    if version == 4 {
        let _ = input.read_d()?;
    }
    Ok(())
}

/// Inverse of [`write_unit_entity_body`] for the modeled class layouts.
/// Consume the revision-dependent prefix of `UnitEntity.write` /
/// `UnitEntityLegacy*.write` so the caller can read the controller next.
///
/// desktop.jar 158.1 `UnitEntityLegacyPoly.read` (and the other generated
/// unit `read` methods) switch on the short revision:
/// - 0..=3: two discarded floats (legacy ability data)
/// - 4: one discarded float
/// - 5+: `TypeIO.readAbilities` (`b count` + `count` floats)
///
/// MechUnit current writes (`revision >= 7`) insert `baseRotation` after
/// abilities. Older revisions do not.
pub(crate) fn read_unit_write_preamble<R: crate::network::codec::Reads>(
    input: &mut R,
    class_id: u8,
) -> std::io::Result<i16> {
    use crate::network::codec::Reads;
    let revision = input.read_s()?;
    match revision {
        0..=3 => {
            let _ = input.read_f()?;
            let _ = input.read_f()?;
        }
        4 => {
            let _ = input.read_f()?;
        }
        _ => {
            let abilities = usize::from(input.read_b()?);
            for _ in 0..abilities {
                let _ = input.read_f()?;
            }
        }
    }
    if class_id == 4 && revision >= 7 {
        let _ = input.read_f()?;
    }
    Ok(revision)
}

pub(crate) fn read_unit_write<R: crate::network::codec::Reads + std::io::Seek>(
    input: &mut R,
    class_id: u8,
) -> std::io::Result<crate::network::world::EnemyUnit> {
    use crate::game::status::ActiveStatus;
    use crate::network::codec::Reads;
    use crate::network::units::controller::read_unit_controller;
    use crate::network::world::EnemyUnit;

    let revision = read_unit_write_preamble(input, class_id)?;
    let controller = read_unit_controller(input, 0)?;
    let elevation = input.read_f()?;
    let flag = input.read_d()?;
    let health = input.read_f()?;
    let _shooting = input.read_bool()?;
    let _mine = input.read_i()?;
    let mounts = input.read_b()? as usize;
    for _ in 0..mounts {
        let _ = input.read_b()?;
        let _ = input.read_f()?;
        let _ = input.read_f()?;
    }
    let mut payloads = Vec::new();
    if matches!(class_id, 5 | 23 | 26) {
        let count = input.read_i()?.max(0) as usize;
        for _ in 0..count.min(16) {
            if let Some(payload) = read_carried_payload(input)? {
                payloads.push(payload);
            }
        }
    }
    let plan_count = input.read_i()?.max(0) as usize;
    let mut build_plans = Vec::new();
    for _ in 0..plan_count.min(50) {
        let breaking = input.read_bool()?;
        let position = input.read_i()?;
        let (block, rotation, config) = if breaking {
            (0i16, 0u8, vec![0])
        } else {
            let block = input.read_s()?;
            let rotation = input.read_b()?;
            let _has_config = input.read_b()?;
            let config =
                crate::engine::typeio::read_object_bytes(input).unwrap_or_else(|_| vec![0]);
            (block, rotation, config)
        };
        build_plans.push(crate::network::world::UnitBuildPlan {
            breaking,
            position,
            block,
            rotation,
            config,
        });
    }
    let rotation = input.read_f()?;
    let shield = input.read_f()?;
    let _spawned = input.read_bool()?;
    let item = input.read_s()?;
    let amount = input.read_i()?;
    let status_count = input.read_i()?.max(0) as usize;
    let mut statuses = Vec::new();
    for _ in 0..status_count.min(32) {
        let effect = input.read_s()?;
        let time = input.read_f()?;
        if effect >= 0 {
            statuses.push(ActiveStatus::simple(effect, time));
        }
    }
    let team = input.read_b()?;
    let unit_type = input.read_s()?;
    // LegacyPoly.read: updateBuilding appears at revision 3, velocity at 4.
    let update_building = if revision >= 3 {
        input.read_bool()?
    } else {
        true
    };
    let (velocity_x, velocity_y) = if revision >= 4 {
        (input.read_f()?, input.read_f()?)
    } else {
        (0.0, 0.0)
    };
    let x = input.read_f()?;
    let y = input.read_f()?;

    let spec = crate::network::units::enemy_spec(unit_type);
    Ok(EnemyUnit {
        id: 0,
        unit_type,
        entity_class: class_id,
        team,
        x,
        y,
        rotation,
        health,
        shield,
        status_effect: statuses.first().map(|s| s.effect).unwrap_or(-1),
        status_duration: statuses.first().map(|s| s.time).unwrap_or(f32::MAX),
        statuses,
        velocity_x,
        velocity_y,
        elevation,
        payloads,
        flag,
        items: if item >= 0 && amount > 0 {
            vec![(item, amount)]
        } else {
            Vec::new()
        },
        mine_progress: 0.0,
        attack_reload: 0.0,
        secondary_attack_reload: 0.0,
        tertiary_attack_reload: 0.0,
        quaternary_attack_reload: 0.0,
        move_speed: spec.map(|s| s.speed).unwrap_or(1.0),
        attack_damage: spec.map(|s| s.attack_damage).unwrap_or(0.0),
        attack_reload_time: spec.map(|s| s.attack_reload).unwrap_or(1.0),
        attack_range: spec.map(|s| s.attack_range).unwrap_or(0.0),
        authority: controller.authority,
        build_plans,
        update_building,
        status_agg: None,
    })
}

// ---------------------------------------------------------------------------
// CONTENT IDENTITY (SOL-008): a stable SHA-256 over the exact serialized
// content header + map region bytes.  Both writers are deterministic for a
// given world, so the digest identifies the map/content of a save
// independently of its meta tags or save slot.  The digest is implemented
// dependency-free (FIPS 180-4) because the crate has no crypto dependency.
// ---------------------------------------------------------------------------

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Minimal dependency-free SHA-256 (FIPS 180-4) with an incremental API.
/// Used only for stable content identity; not a constant-time primitive.
struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Sha256 {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buffer[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (i, k) in SHA256_K.iter().enumerate() {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(*k)
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }

    fn finish(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buf_len != 56 {
            self.update(&[0]);
        }
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);
        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

fn sha256_hex(digest: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Stable content identity of a save: SHA-256 over the exact serialized
/// content header + map region (the bytes `write_msav_content_region` and
/// `write_msav_map_region` emit).  Deterministic for a given world; used by
/// `listener.rs` (via `write_msav_complete`) to fingerprint saved maps.
pub fn msav_content_hash(world: &MsavWorld<'_>) -> std::io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(&write_msav_content_region()?);
    hasher.update(&write_msav_map_region(
        world.width,
        world.height,
        world.floors,
        world.overlays,
        world.blocks,
        world.dynamic_tiles,
        11,
    )?);
    Ok(hasher.finish())
}

/// Hex form of [`msav_content_hash`].
pub fn msav_content_hash_hex(world: &MsavWorld<'_>) -> std::io::Result<String> {
    Ok(sha256_hex(&msav_content_hash(world)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn msav_meta_round_trips_with_port_reader() {
        let mut map = HashMap::new();
        map.insert("mapname".into(), "frontier".into());
        map.insert("wave".into(), "12".into());
        map.insert("tick".into(), "3600".into());
        map.insert("width".into(), "300".into());
        map.insert("height".into(), "300".into());
        map.insert("build".into(), "158".into());
        let bytes = write_msav_meta(&map, 11).unwrap();
        // The port's own reader must accept the produced file.
        let meta = SaveIO::read_meta(&bytes[..]).unwrap();
        assert_eq!(meta.version, 11);
        assert_eq!(meta.map_name, "frontier");
        assert_eq!(meta.width, 300);
        assert_eq!(meta.height, 300);
    }

    #[test]
    fn writes_a_sample_msav_for_official_reader() {
        // Emits target/msav-meta-test.msav so the official desktop.jar can
        // parse it (SaveIO.getMeta) — the cross-implementation proof.
        let mut map = HashMap::new();
        map.insert("mapname".into(), "frontier".into());
        map.insert("wave".into(), "12".into());
        map.insert("tick".into(), "3600".into());
        map.insert("width".into(), "300".into());
        map.insert("height".into(), "300".into());
        map.insert("build".into(), "158".into());
        map.insert("playtime".into(), "1234".into());
        map.insert("saved".into(), "1700000000000".into());
        // Use the real maze map data so the official reader reconstructs it.
        let map_bytes = include_bytes!("../dummy_world.dat");
        let network = crate::engine::world_stream::inspect_map(map_bytes).unwrap();
        let plans = crate::engine::typeio::TeamBlocks {
            teams: vec![crate::engine::typeio::TeamPlans {
                team: 1,
                plans: vec![crate::engine::typeio::TeamBlockPlan {
                    x: 45,
                    y: 100,
                    rotation: 0,
                    block: 216,
                    config: vec![0],
                }],
            }],
        };
        // A built tile serialized as a Building entity (copper wall with a
        // stored item to exercise the ItemModule).
        let dynamic = dashmap::DashMap::new();
        let mut wall = crate::network::world::DynamicTile {
            position: (45 << 16) | 100,
            block: 216,
            team: 1,
            rotation: 0,
            config: vec![0],
            enabled: true,
            message: None,
            occupied: vec![(45 << 16) | 100],
            stored_item: -1,
            stored_amount: 0,
            production_progress: 0.0,
            transport_progress: 0.0,
            ammo_units: 0.0,
            inventory: vec![(0, 5)],
            power_stored: 0.0,
            power_links: Vec::new(),
            liquid_inventory: Vec::new(),
            stored_liquid: -1,
            liquid_amount: 0.0,
            output_liquid_amount: 0.0,
            junction_items: Vec::new(),
            mass_driver_incoming: Vec::new(),
            mass_driver_rotation: 90.0,
            mass_driver_waiting: Vec::new(),
            payload: None,
            payload_progress: 0.0,
            payload_rotation: 0.0,
            payload_accum: Vec::new(),
            health: 320.0,
            door_open: false,
            shield: 0.0,
            light_color: -1_900_545,
            memory: Vec::new(),
            duct_rec_dir: 0,
            unloader_offset: 0,
            conveyor_items: Vec::new(),
            factory_command: None,
            stack_state: 0,
            stack_link: -1,
            stack_cooldown: 0.0,
            generation: 0,
        };
        wall.occupied = vec![wall.position];
        dynamic.insert(wall.position, wall.clone());
        // Exercise all three official module families in the emitted sample:
        // StorageBuild (items), ConsumeGeneratorBuild (items + power), and
        // PumpBuild (liquids).
        let mut container = wall.clone();
        container.position = (46 << 16) | 100;
        container.block = 345; // container in the v158.1 registry
        container.inventory = vec![(1, 12)];
        dynamic.insert(container.position, container);
        let mut generator = wall.clone();
        generator.position = (47 << 16) | 100;
        generator.block = 308; // combustion-generator
        generator.inventory = vec![(2, 3)];
        generator.power_stored = 42.5;
        dynamic.insert(generator.position, generator);
        let mut pump = wall.clone();
        pump.position = (48 << 16) | 100;
        pump.block = 283; // mechanical-pump in this content registry
        pump.inventory.clear();
        pump.stored_liquid = 0;
        pump.liquid_amount = 7.25;
        dynamic.insert(pump.position, pump);
        let enemy_units = vec![
            crate::network::world::EnemyUnit {
                id: 7001,
                unit_type: 0,
                entity_class: 3,
                team: 2,
                x: 80.0,
                y: 96.0,
                rotation: -90.0,
                health: 150.0,
                shield: 0.0,
                status_effect: -1,
                status_duration: f32::MAX,
                statuses: Vec::new(),
                velocity_x: 0.0,
                velocity_y: 0.0,
                elevation: 0.0,
                payloads: Vec::new(),
                flag: 0.0,
                items: Vec::new(),
                mine_progress: 0.0,
                attack_reload: 0.0,
                secondary_attack_reload: 0.0,
                tertiary_attack_reload: 0.0,
                quaternary_attack_reload: 0.0,
                move_speed: 0.5,
                attack_damage: 9.0,
                attack_reload_time: 13.0,
                attack_range: 145.0,
                authority: crate::network::world::UnitAuthority::DefaultAi,
                build_plans: Vec::new(),
                update_building: true,
                status_agg: None,
            },
            crate::network::world::EnemyUnit {
                id: 7002,
                unit_type: 5,
                entity_class: 4,
                team: 2,
                x: 88.0,
                y: 96.0,
                rotation: -90.0,
                health: 120.0,
                shield: 0.0,
                status_effect: -1,
                status_duration: f32::MAX,
                statuses: Vec::new(),
                velocity_x: 0.0,
                velocity_y: 0.0,
                elevation: 0.0,
                payloads: Vec::new(),
                flag: 0.0,
                items: Vec::new(),
                mine_progress: 0.0,
                attack_reload: 0.0,
                secondary_attack_reload: 0.0,
                tertiary_attack_reload: 0.0,
                quaternary_attack_reload: 0.0,
                move_speed: 0.55,
                attack_damage: 13.0,
                attack_reload_time: 24.0,
                attack_range: 156.0,
                authority: crate::network::world::UnitAuthority::DefaultAi,
                build_plans: Vec::new(),
                update_building: true,
                status_agg: None,
            },
        ];
        eprintln!(
            "SAMPLE-DYNAMIC len={} has45={}",
            dynamic.len(),
            dynamic.contains_key(&((45 << 16) | 100))
        );
        let bytes = write_msav_complete(
            &map,
            11,
            &MsavWorld {
                width: network.width as usize,
                height: network.height as usize,
                floors: &network.floors,
                overlays: &network.overlays,
                puddles: &[],
                blocks: &network.blocks,
                team_blocks: Some(&plans),
                dynamic_tiles: &dynamic,
                enemy_units: &enemy_units,
                runtime: None,
            },
        )
        .unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("msav-meta-test.msav");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        // Buildings live in the map region (BuildingComp.serialize == false).
        // The entities region only carries the UnitEntity and MechUnit chunks.
        let entities =
            write_msav_entities_region_with_units(None, Some(&dynamic), &enemy_units, &[], None)
                .unwrap();
        assert_eq!(i32::from_be_bytes(entities[6..10].try_into().unwrap()), 2);
        let mut entity_pos = 10;
        for expected in [3u8, 4] {
            let len = i32::from_be_bytes(entities[entity_pos..entity_pos + 4].try_into().unwrap())
                as usize;
            entity_pos += 4;
            assert_eq!(entities[entity_pos], expected);
            entity_pos += len;
        }
        assert_eq!(entity_pos, entities.len());
    }

    #[test]
    fn map_region_round_trips_dimensions() {
        let floors = vec![0i16; 4];
        let overlays = vec![0i16; 4];
        let mut blocks = vec![0i16; 4];
        blocks[0] = 341; // core
        let region = write_msav_map_region(
            2,
            2,
            &floors,
            &overlays,
            &blocks,
            &dashmap::DashMap::new(),
            11,
        )
        .unwrap();
        // width, height shorts + floor/overlay pairs with runs.
        assert_eq!(&region[0..2], &[0, 2]);
        assert_eq!(&region[2..4], &[0, 2]);
        // total floor/overlay bytes for 4 tiles: 2 shorts per tile + run bytes.
        assert!(region.len() > 8);
    }

    #[test]
    #[ignore]
    fn export_msav_v7_and_v11_for_jar_probe() {
        // Round-73 M1 probe: export v7 + v11 saves with a building so the
        // desktop.jar can be asked to load them (run with --ignored).
        let tile = crate::network::world::DynamicTile {
            position: (10 << 16) | 10,
            block: 216,
            team: 1,
            health: 1_000.0,
            occupied: vec![(10 << 16) | 10],
            ..Default::default()
        };
        let tiles = dashmap::DashMap::new();
        tiles.insert((10 << 16) | 10, tile);
        let mut blocks = vec![0i16; 300 * 300];
        blocks[10 * 300 + 10] = 216;
        let floors = vec![0i16; 300 * 300];
        let overlays = vec![0i16; 300 * 300];
        let meta = [("mapname".to_string(), "probe-v7".to_string())]
            .into_iter()
            .collect();
        for version in [7, 11] {
            let world = MsavWorld {
                width: 300,
                height: 300,
                floors: &floors,
                overlays: &overlays,
                blocks: &blocks,
                puddles: &[],
                team_blocks: None,
                dynamic_tiles: &tiles,
                enemy_units: &[],
                runtime: None,
            };
            let bytes = write_msav_complete(&meta, version, &world).unwrap();
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!("msav-probe-v{version}.msav"));
            std::fs::write(&path, bytes).unwrap();
        }
    }

    #[test]
    fn building_chunks_use_the_official_prefix_per_save_version() {
        // Round-73 M1: Save4..Save9 (ShortChunkSaveVersion family) read
        // building chunks with a u16 length prefix
        // (SaveFileReader.readLegacyShortChunk); Save10+ use the modern i32
        // prefix. The port used to write i32 for every version, which made
        // desktop.jar 158.1 reject v4-v9 saves with buildings.
        let tile = crate::network::world::DynamicTile {
            position: 0,
            block: 216, // copper wall
            team: 1,
            health: 1_000.0,
            occupied: vec![0],
            ..Default::default()
        };
        let tiles = dashmap::DashMap::new();
        tiles.insert(0, tile);
        let floors = vec![0i16; 4];
        let overlays = vec![0i16; 4];
        let blocks = vec![0i16; 4];
        // Mini-parser for the map region: floors/overlays use consecutive-run
        // compression (u16 floor, u16 overlay, u8 run) and the block section
        // interleaves (u16 block, u8 packed, [u8 center + chunk]) entries with
        // (u16 block, u8 packed=0, u8 run) runs.
        let parse = |region: &[u8], prefix_width: usize| -> usize {
            let total = 4usize;
            let mut pos = 4usize; // width + height
            let mut covered = 0usize;
            while covered < total {
                pos += 4;
                covered += 1 + region[pos] as usize;
                pos += 1;
            }
            covered = 0;
            let mut chunk_len = 0usize;
            while covered < total {
                let block = u16::from_be_bytes(region[pos..pos + 2].try_into().unwrap());
                pos += 2;
                let packed = region[pos];
                pos += 1;
                if packed & 1 != 0 {
                    assert_eq!(block, 216, "the build carries the wall block");
                    pos += 1; // center byte
                    chunk_len = if prefix_width == 2 {
                        u16::from_be_bytes(region[pos..pos + 2].try_into().unwrap()) as usize
                    } else {
                        i32::from_be_bytes(region[pos..pos + 4].try_into().unwrap()) as usize
                    };
                    assert!(chunk_len >= 5, "chunk body present");
                    pos += prefix_width + chunk_len;
                    covered += 1;
                } else {
                    covered += 1 + region[pos] as usize;
                    pos += 1;
                }
            }
            assert_eq!(pos, region.len(), "consumed exactly");
            chunk_len
        };
        for version in 4..=9 {
            let region =
                write_msav_map_region(2, 2, &floors, &overlays, &blocks, &tiles, version).unwrap();
            parse(&region, 2); // u16 chunk prefix (ShortChunkSaveVersion)
        }
        for version in [10, 11] {
            let region =
                write_msav_map_region(2, 2, &floors, &overlays, &blocks, &tiles, version).unwrap();
            parse(&region, 4); // i32 chunk prefix (SaveVersion)
        }
        // The reader dispatches the same way: v7 parses as short chunks,
        // v10/v11 as modern ones.
        for version in [7, 10, 11] {
            let region =
                write_msav_map_region(2, 2, &floors, &overlays, &blocks, &tiles, version).unwrap();
            // read_map_region expects the container framing (i32 length).
            let mut wrapped = Vec::with_capacity(region.len() + 4);
            wrapped.extend_from_slice(&(region.len() as i32).to_be_bytes());
            wrapped.extend_from_slice(&region);
            let (width, height, read_tiles) =
                SaveIO::read_map_region(&mut std::io::Cursor::new(wrapped), version).unwrap();
            assert_eq!((width, height), (2, 2));
            assert_eq!(
                read_tiles[0].block_id, 216,
                "v{version} reads the building back"
            );
        }
    }

    #[test]
    fn content_region_has_official_type_order() {
        let region = write_msav_content_region().unwrap();
        // Sequential parse matching SaveVersion.readContentHeader.
        let mut pos = 0usize;
        let read_u8 = |pos: &mut usize| -> u8 {
            let v = region[*pos];
            *pos += 1;
            v
        };
        let read_u16 = |pos: &mut usize| -> usize {
            let v = u16::from_be_bytes(region[*pos..*pos + 2].try_into().unwrap()) as usize;
            *pos += 2;
            v
        };
        let read_utf = |pos: &mut usize| -> String {
            let len = read_u16(pos);
            let s = String::from_utf8_lossy(&region[*pos..*pos + len]).into_owned();
            *pos += len;
            s
        };
        let mapped = read_u8(&mut pos) as usize;
        assert_eq!(mapped, 4);
        let mut seen = Vec::new();
        for _ in 0..mapped {
            let ty = read_u8(&mut pos);
            let total = read_u16(&mut pos);
            let mut names = Vec::new();
            for _ in 0..total {
                names.push(read_utf(&mut pos));
            }
            seen.push((ty, total, names));
        }
        assert_eq!(seen[0].0, 0);
        assert_eq!(seen[0].1, 22);
        assert_eq!(seen[0].2[0], "copper");
        assert_eq!(seen[1].0, 1);
        assert_eq!(seen[1].1, 446);
        assert_eq!(seen[1].2[0], "air");
        assert_eq!(seen[2].0, 4);
        assert_eq!(seen[2].1, 11);
        assert_eq!(seen[2].2[0], "water");
        assert_eq!(seen[3].0, 6);
        // 69 unit ids: the full desktop 158.1 unit registry (jar dump).
        assert_eq!(seen[3].1, crate::game::unit_types::UNIT_COUNT);
        assert_eq!(seen[3].2[0], "dagger");
        assert_eq!(
            seen[3].2.last().map(String::as_str),
            Some("turret-unit-build-tower"),
            "last unit id 68 has its official name"
        );
        assert!(
            seen[3].2.iter().all(|name| !name.is_empty()),
            "every unit id carries an official name (no empty placeholders)"
        );
        assert_eq!(pos, region.len(), "no trailing bytes");
    }

    #[test]
    fn msav_meta_string_map_matches_official_layout() {
        // Byte-level: after zlib inflate the layout is
        // "MSAV" + i32 13 + i32 meta_len + string_map.
        let mut map = HashMap::new();
        map.insert("wave".into(), "3".into());
        let bytes = write_msav_meta(&map, 11).unwrap();
        use flate2::read::ZlibDecoder;
        let mut plain = Vec::new();
        ZlibDecoder::new(&bytes[..])
            .read_to_end(&mut plain)
            .unwrap();
        assert_eq!(&plain[0..4], b"MSAV");
        let version = i32::from_be_bytes(plain[4..8].try_into().unwrap());
        assert_eq!(version, 11);
        let meta_len = i32::from_be_bytes(plain[8..12].try_into().unwrap()) as usize;
        assert_eq!(plain.len(), 12 + meta_len);
        // string_map: u16 count=1, then "wave" (u16 4 + 4 bytes), "3" (u16 1 + 1 byte).
        let count = u16::from_be_bytes(plain[12..14].try_into().unwrap());
        assert_eq!(count, 1);
        let key_len = u16::from_be_bytes(plain[14..16].try_into().unwrap()) as usize;
        assert_eq!(&plain[16..16 + key_len], b"wave");
        let val_len =
            u16::from_be_bytes(plain[16 + key_len..18 + key_len].try_into().unwrap()) as usize;
        assert_eq!(&plain[18 + key_len..18 + key_len + val_len], b"3");
    }

    #[test]
    fn entities_region_contains_unit_chunks_only() {
        let dynamic = dashmap::DashMap::new();
        for (index, position) in [
            (0, (45 << 16) | 100),
            (1, (46 << 16) | 100),
            (2, (47 << 16) | 100),
            (3, (48 << 16) | 100),
        ] {
            dynamic.insert(
                position,
                crate::network::world::DynamicTile {
                    position,
                    block: 216 + index,
                    team: 1,
                    rotation: 0,
                    config: vec![],
                    enabled: true,
                    message: None,
                    occupied: vec![position],
                    stored_item: -1,
                    stored_amount: 0,
                    production_progress: 0.0,
                    transport_progress: 0.0,
                    ammo_units: 0.0,
                    inventory: vec![],
                    power_stored: 0.0,
                    power_links: Vec::new(),
                    liquid_inventory: Vec::new(),
                    stored_liquid: -1,
                    liquid_amount: 0.0,
                    output_liquid_amount: 0.0,
                    junction_items: vec![],
                    mass_driver_incoming: vec![],
                    mass_driver_rotation: 0.0,
                    mass_driver_waiting: vec![],
                    payload: None,
                    payload_progress: 0.0,
                    payload_rotation: 0.0,
                    payload_accum: vec![],
                    health: 100.0,
                    door_open: false,
                    shield: 0.0,
                    light_color: -1,
                    memory: vec![],
                    duct_rec_dir: 0,
                    unloader_offset: 0,
                    conveyor_items: vec![],
                    factory_command: None,
                    stack_state: 0,
                    stack_link: -1,
                    stack_cooldown: 0.0,
                    generation: 0,
                },
            );
        }
        let make_unit = |id: i32, class_id: u8| crate::network::world::EnemyUnit {
            id,
            unit_type: 0,
            entity_class: class_id,
            team: 2,
            x: 80.0,
            y: 96.0,
            rotation: -90.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 0.0,
            payloads: vec![],
            flag: 0.0,
            items: vec![],
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: 0.5,
            attack_damage: 9.0,
            attack_reload_time: 13.0,
            attack_range: 145.0,
            authority: crate::network::world::UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        };
        let units = vec![make_unit(7001, 3), make_unit(7002, 4)];
        let region =
            write_msav_entities_region_with_units(None, Some(&dynamic), &units, &[], None).unwrap();
        // mapping short + team count + world entity count
        assert_eq!(u16::from_be_bytes(region[0..2].try_into().unwrap()), 0);
        assert_eq!(i32::from_be_bytes(region[2..6].try_into().unwrap()), 0);
        let count_pos = 6;
        assert_eq!(
            i32::from_be_bytes(region[count_pos..count_pos + 4].try_into().unwrap()),
            2
        );
        let mut pos = count_pos + 4;
        for expected in [3u8, 4] {
            let len = i32::from_be_bytes(region[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            assert!(len >= 5);
            assert_eq!(region[pos], expected);
            pos += len;
        }
        assert_eq!(pos, region.len());
    }

    #[test]
    fn team_blocks_region_round_trips() {
        // Build team plans and verify the encoded region decodes identically
        // (TeamBlocks::decode is the official reader's algorithm).
        let tb = crate::engine::typeio::TeamBlocks {
            teams: vec![crate::engine::typeio::TeamPlans {
                team: 1,
                plans: vec![
                    crate::engine::typeio::TeamBlockPlan {
                        x: 45,
                        y: 100,
                        rotation: 0,
                        block: 216,
                        config: vec![0],
                    },
                    crate::engine::typeio::TeamBlockPlan {
                        x: 46,
                        y: 100,
                        rotation: 1,
                        block: 270,
                        config: vec![5, 0, 0, 3], // item selection (lead)
                    },
                ],
            }],
        };
        let region = write_msav_entities_region(Some(&tb), None).unwrap();
        // entity mapping short (0) + team count i32 (1) + ...
        assert_eq!(u16::from_be_bytes(region[0..2].try_into().unwrap()), 0);
        assert_eq!(i32::from_be_bytes(region[2..6].try_into().unwrap()), 1);
        // team id 1, plan count 2
        assert_eq!(i32::from_be_bytes(region[6..10].try_into().unwrap()), 1);
        assert_eq!(i32::from_be_bytes(region[10..14].try_into().unwrap()), 2);
        // Decode the team-blocks section (skip the entity mapping short).
        let (decoded, consumed) = crate::engine::typeio::TeamBlocks::decode(&region[2..]).unwrap();
        assert_eq!(consumed, region.len() - 2 - 4, "stops before entity count");
        assert_eq!(decoded.teams.len(), 1);
        assert_eq!(decoded.teams[0].plans.len(), 2);
        assert_eq!(decoded.teams[0].plans[0].block, 216);
        assert_eq!(decoded.teams[0].plans[1].config, vec![5, 0, 0, 3]);
    }

    // --- SOL-008: legacy v1-v3 fixtures -----------------------------------

    fn utf_bytes(value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let bytes = crate::network::codec::encode_modified_utf8(value);
        out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        out.extend_from_slice(&bytes);
        out
    }

    fn string_map(tags: &[(&str, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(tags.len() as i16).to_be_bytes());
        for (key, value) in tags {
            out.extend_from_slice(&utf_bytes(key));
            out.extend_from_slice(&utf_bytes(value));
        }
        out
    }

    /// Official v1-v3 container: MSAV + i32 version + exactly four regions
    /// meta/content/map/entities (versions/LegacyRegionSaveVersion.read).
    fn legacy_msav_fixture(version: i32, map: &[u8], entities: &[u8]) -> Vec<u8> {
        let meta = string_map(&[
            ("mapname", "legacy-fixture"),
            ("description", "synthetic legacy save"),
            ("author", "rust-tests"),
            ("width", "4"),
            ("height", "4"),
            ("build", "126"),
            ("wave", "3"),
        ]);
        let content = write_msav_content_region().unwrap();
        let regions: [&[u8]; 4] = [&meta, &content, map, entities];
        let mut msav = b"MSAV".to_vec();
        msav.extend_from_slice(&version.to_be_bytes());
        for region in regions {
            msav.extend_from_slice(&(region.len() as i32).to_be_bytes());
            msav.extend_from_slice(region);
        }
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        use std::io::Write;
        encoder.write_all(&msav).unwrap();
        encoder.finish().unwrap()
    }

    /// 4x4 legacy map: floor run over all 16 tiles, a core-nucleus (341,
    /// synthetic) building chunk at tile 0 and an air run covering the rest.
    fn legacy_map_fixture() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&4u16.to_be_bytes()); // width
        out.extend_from_slice(&4u16.to_be_bytes()); // height
        out.extend_from_slice(&1i16.to_be_bytes()); // floor
        out.extend_from_slice(&0i16.to_be_bytes()); // overlay
        out.push(15); // run covers all 16 tiles
                      // Tile 0: core-nucleus with a legacy chunk body
                      // [revision 0][health u16 4000][packedrot 0x10][legacy modules][cons.valid][subclass].
        out.extend_from_slice(&341i16.to_be_bytes());
        let mut chunk = Vec::new();
        chunk.push(0);
        chunk.extend_from_slice(&4000u16.to_be_bytes());
        chunk.push(0x10); // team nibble 1, rotation nibble 0
        chunk.push(2); // legacy ItemModule: two entries
        chunk.extend_from_slice(&0i16.to_be_bytes());
        chunk.extend_from_slice(&2000i32.to_be_bytes());
        chunk.extend_from_slice(&1i16.to_be_bytes());
        chunk.extend_from_slice(&1500i32.to_be_bytes());
        chunk.push(1); // cons.valid
        chunk.push(0); // subclass tail byte
        out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        out.extend_from_slice(&chunk);
        // Tiles 1..16: air, one run covering the rest.
        out.extend_from_slice(&0i16.to_be_bytes());
        out.push(14);
        out
    }

    /// `readLegacyEntities` payload: one group with two opaque u16 chunks.
    fn legacy_entities_with_groups() -> Vec<u8> {
        let mut out = Vec::new();
        out.push(1); // one group
        out.extend_from_slice(&2i32.to_be_bytes()); // two entities
        for _ in 0..2 {
            let chunk = [1u8, 2, 3, 4, 5];
            out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
            out.extend_from_slice(&chunk);
        }
        out
    }

    /// `Save3.readEntities` payload: one team with two plans (raw i32 config)
    /// followed by a zero-group `readLegacyEntities` tail.
    fn legacy_v3_entities_fixture() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1i32.to_be_bytes()); // one team
        out.extend_from_slice(&1i32.to_be_bytes()); // team id 1
        out.extend_from_slice(&2i32.to_be_bytes()); // two plans
        for (x, y, rotation, block, config) in [
            (45i16, 100i16, 0i16, 216i16, 42i32),
            (46i16, 100i16, 1i16, 270i16, 5i32),
        ] {
            out.extend_from_slice(&x.to_be_bytes());
            out.extend_from_slice(&y.to_be_bytes());
            out.extend_from_slice(&rotation.to_be_bytes());
            out.extend_from_slice(&block.to_be_bytes());
            out.extend_from_slice(&config.to_be_bytes());
        }
        out.push(0); // no entity groups
        out
    }

    fn msav_regions(compressed: &[u8]) -> (i32, Vec<Vec<u8>>) {
        use flate2::read::ZlibDecoder;
        use std::io::Read;
        let mut plain = Vec::new();
        ZlibDecoder::new(compressed)
            .read_to_end(&mut plain)
            .unwrap();
        assert_eq!(&plain[..4], b"MSAV");
        let version = i32::from_be_bytes(plain[4..8].try_into().unwrap());
        let mut pos = 8;
        let mut regions = Vec::new();
        while pos < plain.len() {
            let len = i32::from_be_bytes(plain[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            regions.push(plain[pos..pos + len].to_vec());
            pos += len;
        }
        (version, regions)
    }

    #[test]
    fn legacy_v1_v2_v3_saves_parse_meta_map_and_entities() {
        for version in [1, 2, 3] {
            let entities = if version == 3 {
                legacy_v3_entities_fixture()
            } else {
                legacy_entities_with_groups()
            };
            let bytes = legacy_msav_fixture(version, &legacy_map_fixture(), &entities);
            // read_meta must not reject legacy versions.
            let meta = SaveIO::read_meta(&bytes[..]).unwrap();
            assert_eq!(meta.version, version);
            assert_eq!(meta.map_name, "legacy-fixture");
            assert_eq!(meta.width, 4);
            assert_eq!(meta.height, 4);
            // Full parse through the legacy reader.
            let save = SaveIO::read_legacy_save(&bytes[..]).unwrap();
            assert_eq!(save.version, version);
            assert_eq!(save.width, 4);
            assert_eq!(save.height, 4);
            assert_eq!(save.tiles.len(), 16);
            // Floor run covers everything; the core chunk sits at tile 0.
            assert!(save.tiles.iter().all(|t| t.floor_id == 1));
            assert_eq!(save.tiles[0].block_id, 341);
            assert!(save.tiles[0].has_entity);
            // Legacy chunk header: revision 0, health 4000, team 1, rotation 0.
            let header = decode_legacy_chunk_header(&save.tiles[0].entity_data).unwrap();
            assert_eq!(header.revision, 0);
            assert_eq!(header.health, 4000.0);
            assert_eq!(header.team, 1);
            assert_eq!(header.rotation, 0);
            // The air run covers tiles 1..16.
            assert!(save.tiles[1..].iter().all(|t| t.block_id == 0));
            // read_map agrees on the same version dispatch.
            let (meta, tiles) = SaveIO::read_map(&bytes[..]).unwrap();
            assert_eq!(meta.version, version);
            assert_eq!(tiles.len(), 16);
            assert_eq!(tiles[0].block_id, 341);
        }
    }

    #[test]
    fn legacy_v3_save_parses_team_plans() {
        let bytes = legacy_msav_fixture(3, &legacy_map_fixture(), &legacy_v3_entities_fixture());
        let save = SaveIO::read_legacy_save(&bytes[..]).unwrap();
        assert_eq!(save.team_plans.len(), 2);
        assert_eq!(save.team_plans[0].team, 1);
        assert_eq!(save.team_plans[0].x, 45);
        assert_eq!(save.team_plans[0].y, 100);
        assert_eq!(save.team_plans[0].rotation, 0);
        assert_eq!(save.team_plans[0].block, 216);
        assert_eq!(save.team_plans[0].config, 42);
        assert_eq!(save.team_plans[1].config, 5);
        assert_eq!(save.skipped_entity_chunks, 0);
        assert!(save.warnings.is_empty());
    }

    #[test]
    fn legacy_entity_chunks_are_skipped_with_explicit_warnings() {
        let bytes = legacy_msav_fixture(1, &legacy_map_fixture(), &legacy_entities_with_groups());
        let save = SaveIO::read_legacy_save(&bytes[..]).unwrap();
        assert_eq!(save.skipped_entity_chunks, 2);
        assert_eq!(save.warnings.len(), 1);
        assert!(save.warnings[0].contains("unsupported legacy entity data"));
        // Save2 shares the exact entities layout.
        let bytes2 = legacy_msav_fixture(2, &legacy_map_fixture(), &legacy_entities_with_groups());
        let save2 = SaveIO::read_legacy_save(&bytes2[..]).unwrap();
        assert_eq!(save2.skipped_entity_chunks, 2);
        assert_eq!(save2.warnings.len(), 1);
    }

    #[test]
    fn real_canyon_v2_save_parses_through_legacy_reader() {
        // Differential fixture: canyon.msav is a genuine Save2 save shipped
        // with the Java sources (250x250, legacy block format, one
        // core-nucleus building chunk, zero-group entities region).
        let path = std::path::Path::new("../core/assets/maps/default/canyon.msav");
        if !path.exists() {
            return;
        }
        let msav = std::fs::read(path).unwrap();
        let save = SaveIO::read_legacy_save(&msav[..]).unwrap();
        assert_eq!(save.version, 2);
        assert_eq!(save.width, 250);
        assert_eq!(save.height, 250);
        assert_eq!(save.tiles.len(), 62500);
        // The single legacy building chunk (old id 241 = core-nucleus) with
        // the header verified byte-by-byte against the real save.
        let chunks: Vec<&Tile> = save.tiles.iter().filter(|t| t.has_entity).collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].block_id, 241);
        let header = decode_legacy_chunk_header(&chunks[0].entity_data).unwrap();
        assert_eq!(header.revision, 0);
        assert_eq!(header.health, 4000.0);
        assert_eq!(header.team, 1);
        assert_eq!(header.rotation, 0);
        // Entities region has zero groups.
        assert_eq!(save.skipped_entity_chunks, 0);
        assert!(save.warnings.is_empty());
        // read_map agrees.
        let (meta, tiles) = SaveIO::read_map(&msav[..]).unwrap();
        assert_eq!(meta.width, 250);
        assert_eq!(meta.height, 250);
        assert_eq!(tiles.len(), 62500);
        assert_eq!(tiles.iter().filter(|t| t.has_entity).count(), 1);
    }

    #[test]
    fn legacy_chunk_header_decodes_team_8_extension_byte() {
        // packedrot nibble 8 means the full team byte follows (Pack.leftByte
        // == 8 in LegacySaveVersion.readMap).
        let chunk = [0u8, 0x0f, 0x27, 0x80, 9, 7, 7];
        let header = decode_legacy_chunk_header(&chunk).unwrap();
        assert_eq!(header.health, 0x0f27 as f32);
        assert_eq!(header.team, 9);
        assert_eq!(header.rotation, 0);
        assert_eq!(header.tail_start, 5);
        assert!(decode_legacy_chunk_header(&[0, 0, 0]).is_none());
    }

    #[test]
    fn write_msav_complete_emits_official_region_order_per_version() {
        let mut map = HashMap::new();
        map.insert("mapname".into(), "frontier".into());
        map.insert("width".into(), "2".into());
        map.insert("height".into(), "2".into());
        let floors = [1i16; 4];
        let overlays = [0i16; 4];
        let blocks = [0i16; 4];
        let empty_tiles = dashmap::DashMap::new();
        let make_world = || MsavWorld {
            width: 2,
            height: 2,
            floors: &floors,
            overlays: &overlays,
            puddles: &[],
            blocks: &blocks,
            team_blocks: None,
            dynamic_tiles: &empty_tiles,
            enemy_units: &[],
            runtime: None,
        };
        // v11 (the official 158.1 current writer): meta, content, patches,
        // map, entities, markers, custom.
        let (version, regions) =
            msav_regions(&write_msav_complete(&map, 11, &make_world()).unwrap());
        assert_eq!(version, 11);
        assert_eq!(regions.len(), 7);
        assert_eq!(regions[2], vec![0], "patches region is writeByte(0)");
        assert_eq!(
            &regions[3][..4],
            &[0, 2, 0, 2],
            "map region leads with width/height"
        );
        // v13 (official layout): meta, patches, content, map, entities, markers, custom.
        let (_, regions) = msav_regions(&write_msav_complete(&map, 13, &make_world()).unwrap());
        assert_eq!(regions.len(), 7);
        assert_eq!(
            regions[1],
            vec![0, 0, 0, 2, 0, 0, 0, 0],
            "v13 patches precede content"
        );
        let (_, regions) = msav_regions(&write_msav_complete(&map, 12, &make_world()).unwrap());
        assert_eq!(
            regions[1],
            vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            "v12 empty patches are version+patchAmount+imageAmount"
        );
        // v4: meta, content, map, entities — no patches/markers/custom.
        let (_, regions) = msav_regions(&write_msav_complete(&map, 4, &make_world()).unwrap());
        assert_eq!(regions.len(), 4);
        // v7: meta, content, map, entities, custom.
        let (_, regions) = msav_regions(&write_msav_complete(&map, 7, &make_world()).unwrap());
        assert_eq!(regions.len(), 5);
        assert_eq!(regions[4], 0i32.to_be_bytes());
    }

    #[test]
    fn write_msav_complete_round_trips_through_read_map() {
        let mut map = HashMap::new();
        map.insert("mapname".into(), "frontier".into());
        map.insert("width".into(), "2".into());
        map.insert("height".into(), "2".into());
        let floors = [1i16; 4];
        let overlays = [0i16; 4];
        let blocks = [0i16; 4];
        // A built tile at (0,0): the writer emits the Building chunk (packed
        // entity bit + center + i32 length + body) for DynamicTiles, not for
        // bare base blocks (that matches the official reader).
        let dynamic_tiles = dashmap::DashMap::new();
        dynamic_tiles.insert(
            0i32,
            crate::network::world::DynamicTile {
                position: 0,
                block: 341,
                team: 1,
                rotation: 0,
                config: vec![],
                enabled: true,
                message: None,
                occupied: vec![0],
                stored_item: -1,
                stored_amount: 0,
                production_progress: 0.0,
                transport_progress: 0.0,
                ammo_units: 0.0,
                inventory: vec![],
                power_stored: 0.0,
                power_links: Vec::new(),
                liquid_inventory: Vec::new(),
                stored_liquid: -1,
                liquid_amount: 0.0,
                output_liquid_amount: 0.0,
                junction_items: vec![],
                mass_driver_incoming: vec![],
                mass_driver_rotation: 0.0,
                mass_driver_waiting: vec![],
                payload: None,
                payload_progress: 0.0,
                payload_rotation: 0.0,
                payload_accum: vec![],
                health: 4000.0,
                door_open: false,
                shield: 0.0,
                light_color: -1,
                memory: vec![],
                duct_rec_dir: 0,
                unloader_offset: 0,
                conveyor_items: vec![],
                factory_command: None,
                stack_state: 0,
                stack_link: -1,
                stack_cooldown: 0.0,
                generation: 0,
            },
        );
        let world = MsavWorld {
            width: 2,
            height: 2,
            floors: &floors,
            puddles: &[],
            overlays: &overlays,
            blocks: &blocks,
            team_blocks: None,
            dynamic_tiles: &dynamic_tiles,
            enemy_units: &[],
            runtime: None,
        };
        // v11-v13 writer output (with patches region) must parse through read_map.
        for version in [11, 12, 13] {
            let bytes = write_msav_complete(&map, version, &world).unwrap();
            let (meta, tiles) = SaveIO::read_map(&bytes[..]).unwrap();
            assert_eq!(meta.version, version);
            assert_eq!(tiles.len(), 4);
            assert_eq!(tiles[0].block_id, 341);
            assert!(tiles[0].has_entity);
        }

        // Versions beyond MAX_BASELINE_SAVE_VERSION are rejected.
        for version in [14, 15] {
            let bytes = write_msav_complete(&map, version, &world).unwrap();
            let err = SaveIO::read_map(&bytes[..]).unwrap_err();
            let message = err.to_string();
            assert!(
                message.contains("SaveVersion") && message.contains("not supported"),
                "{message}"
            );
        }
    }

    #[test]
    #[ignore = "exports a v11 MSAV for the desktop.jar reader probe"]
    fn export_v11_msav_for_jar_probe() {
        let map = std::collections::HashMap::from([
            ("mapname".to_string(), "probe".to_string()),
            ("width".to_string(), "2".to_string()),
            ("height".to_string(), "2".to_string()),
            ("build".to_string(), "159".to_string()),
        ]);
        let floors = [0i16; 4];
        let overlays = [0i16; 4];
        let blocks = [0i16; 4];
        let tiles = dashmap::DashMap::new();
        let world = MsavWorld {
            width: 2,
            height: 2,
            floors: &floors,
            puddles: &[],
            overlays: &overlays,
            blocks: &blocks,
            team_blocks: None,
            dynamic_tiles: &tiles,
            enemy_units: &[],
            runtime: None,
        };
        let bytes = write_msav_complete(&map, 13, &world).unwrap();
        std::fs::create_dir_all("target/msav-probe").unwrap();
        std::fs::write("target/msav-probe/probe-v13.msav", bytes).unwrap();
    }

    #[test]
    fn msav_round_trip_per_version_v4_to_v13() {
        // P1: the writer/reader round-trip must hold for every baseline version
        // the 159.7 reader accepts (Save1..Save13). v1-v3 are metadata-only
        // (rejected by host_map); v4-v13 use the modern i32-prefixed map layout
        // and must parse with their per-version region order.
        let map = std::collections::HashMap::from([
            ("mapname".to_string(), "versions".to_string()),
            ("width".to_string(), "2".to_string()),
            ("height".to_string(), "2".to_string()),
            ("build".to_string(), "159".to_string()),
        ]);
        let floors = [0i16, 1, 2, 3];
        let overlays = [0i16; 4];
        let blocks = [0i16, 216, 0, 341];
        let tiles = dashmap::DashMap::new();
        for version in 4..=13 {
            let world = MsavWorld {
                width: 2,
                height: 2,
                floors: &floors,
                overlays: &overlays,
                puddles: &[],
                blocks: &blocks,
                team_blocks: None,
                dynamic_tiles: &tiles,
                enemy_units: &[],
                runtime: None,
            };
            let bytes = write_msav_complete(&map, version, &world).unwrap();
            let (meta, read_tiles) = SaveIO::read_map(&bytes[..]).unwrap();
            assert_eq!(meta.version, version, "version {version} round trip");
            assert_eq!(read_tiles.len(), 4, "version {version} tile count");
            // The map layout for v4+ is the modern i32-prefixed block section:
            // block ids survive the round trip.
            let wall = read_tiles
                .iter()
                .find(|tile| tile.block_id == 216)
                .expect("copper wall survives the round trip");
            assert_eq!(wall.extra_data, 0);
        }
    }

    #[test]
    fn sha256_matches_official_vectors() {
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(
            sha256_hex(&hasher.finish()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut hasher = Sha256::new();
        hasher.update(b"");
        assert_eq!(
            sha256_hex(&hasher.finish()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let mut hasher = Sha256::new();
        hasher.update(b"The quick brown fox jumps over the lazy dog");
        assert_eq!(
            sha256_hex(&hasher.finish()),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
        // Split updates across block boundaries agree with one-shot hashing.
        let mut one_shot = Sha256::new();
        one_shot.update(&[0x61; 100]);
        let mut split = Sha256::new();
        for chunk in [&[0x61; 64][..], &[0x61; 36][..]] {
            split.update(chunk);
        }
        assert_eq!(one_shot.finish(), split.finish());
    }

    #[test]
    fn msav_content_hash_is_deterministic_and_content_sensitive() {
        let floors = [1i16; 4];
        let overlays = [0i16; 4];
        let blocks = [0i16; 4];
        let world = MsavWorld {
            width: 2,
            height: 2,
            floors: &floors,
            puddles: &[],
            overlays: &overlays,
            blocks: &blocks,
            team_blocks: None,
            dynamic_tiles: &dashmap::DashMap::new(),
            enemy_units: &[],
            runtime: None,
        };
        let first = msav_content_hash(&world).unwrap();
        assert_eq!(first, msav_content_hash(&world).unwrap());
        assert_eq!(msav_content_hash_hex(&world).unwrap().len(), 64);
        let mut other = [0i16; 4];
        other[0] = 216;
        let world2 = MsavWorld {
            width: 2,
            height: 2,
            floors: &floors,
            overlays: &overlays,
            puddles: &[],
            blocks: &other,
            team_blocks: None,
            dynamic_tiles: &dashmap::DashMap::new(),
            enemy_units: &[],
            runtime: None,
        };
        assert_ne!(first, msav_content_hash(&world2).unwrap());
    }

    fn sample_unit(
        team: u8,
        authority: crate::network::world::UnitAuthority,
    ) -> crate::network::world::EnemyUnit {
        crate::network::world::EnemyUnit {
            id: 7001,
            unit_type: 0,
            entity_class: 3,
            team,
            x: 80.0,
            y: 96.0,
            rotation: -90.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 0.0,
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: 0.5,
            attack_damage: 9.0,
            attack_reload_time: 13.0,
            attack_range: 145.0,
            authority,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        }
    }

    #[test]
    fn unit_entity_body_controller_tag_offset_matches_typeio() {
        use crate::network::codec::Reads;
        use crate::network::units::controller::read_unit_controller;

        let logic = sample_unit(
            1,
            crate::network::world::UnitAuthority::Logic {
                processor_pos: (12 << 16) | 8,
                remaining_ticks: 400.0,
                processor_generation: 0,
            },
        );
        let body = write_unit_entity_body(&logic, 3, None).unwrap();
        // UnitEntity.write: s revision, b abilities, then TypeIO controller.
        assert_eq!(&body[..2], &9i16.to_be_bytes());
        assert_eq!(body[2], 0);
        assert_eq!(body[3], 3, "LogicAI tag sits immediately after abilities");
        let restored =
            read_unit_controller(&mut std::io::Cursor::new(&body[3..]), logic.id).unwrap();
        assert!(matches!(
            restored.authority,
            crate::network::world::UnitAuthority::Logic {
                processor_pos, ..
            } if processor_pos == (12 << 16) | 8
        ));

        let command = sample_unit(1, crate::network::world::UnitAuthority::Command);
        let body = write_unit_entity_body(&command, 3, None).unwrap();
        assert_eq!(body[3], 9);

        let wave = sample_unit(2, crate::network::world::UnitAuthority::DefaultAi);
        let body = write_unit_entity_body(&wave, 3, None).unwrap();
        assert_eq!(body[3], 2);

        let mech = sample_unit(
            1,
            crate::network::world::UnitAuthority::Logic {
                processor_pos: 7,
                remaining_ticks: 10.0,
                processor_generation: 0,
            },
        );
        let body = write_unit_entity_body(&mech, 4, None).unwrap();
        // MechUnit inserts baseRotation (f) before the controller.
        assert_eq!(&body[..2], &9i16.to_be_bytes());
        assert_eq!(body[2], 0);
        assert_eq!(body[7], 3, "MechUnit controller follows baseRotation");
    }

    #[test]
    fn msav_roundtrip_keeps_controller_status_and_payload_state() {
        use crate::game::status::ActiveStatus;
        use crate::network::codec::Reads;
        use crate::network::units::controller::read_unit_controller;
        use crate::network::world::{CarriedBuildPayload, CarriedPayload, DynamicTile};

        let mut unit = sample_unit(
            1,
            crate::network::world::UnitAuthority::Logic {
                processor_pos: 99,
                remaining_ticks: 500.0,
                processor_generation: 0,
            },
        );
        unit.statuses = vec![ActiveStatus::simple(4, 12.0)];
        unit.status_effect = 4;
        unit.status_duration = 12.0;
        unit.payloads = vec![CarriedPayload::Build(CarriedBuildPayload {
            tile: DynamicTile {
                position: 0,
                block: 216,
                team: 1,
                occupied: vec![0],
                health: 1.0,
                ..DynamicTile::default()
            },
            version: 0,
            sync: Vec::new(),
        })];

        let body = write_unit_entity_body(&unit, 3, None).unwrap();
        let controller =
            read_unit_controller(&mut std::io::Cursor::new(&body[3..]), unit.id).unwrap();
        assert!(matches!(
            controller.authority,
            crate::network::world::UnitAuthority::Logic {
                processor_pos: 99,
                remaining_ticks,
                ..
            } if (remaining_ticks - 600.0).abs() < f32::EPSILON
        ));
        // Status collection: after controller comes elevation..team; the
        // count is present and non-zero so a Java load keeps the entry.
        assert!(body.windows(4).any(|w| w == 1i32.to_be_bytes().as_slice()));
        assert_eq!(unit.payloads.len(), 1);
        assert_eq!(unit.statuses[0].effect, 4);
        assert!((unit.statuses[0].time - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn unit_entity_body_logic_controller_writes_processor_pos_at_tag3() {
        let logic = sample_unit(
            42,
            crate::network::world::UnitAuthority::Logic {
                processor_pos: (12 << 16) | 8,
                remaining_ticks: 10.0,
                processor_generation: 0,
            },
        );
        let body = write_unit_entity_body(&logic, 3, None).unwrap();
        assert_eq!(body[3], 3);
        assert_eq!(
            i32::from_be_bytes(body[4..8].try_into().unwrap()),
            (12 << 16) | 8
        );
    }

    #[test]
    fn official_159_7_save13_fixture_reads() {
        let bytes =
            include_bytes!("../../tests/fixtures/official-159.7/official-save13-empty.msav");
        let (meta, tiles) = SaveIO::read_map(&bytes[..]).expect("official Save13");
        assert_eq!(meta.version, 13);
        assert!(!tiles.is_empty());
    }

    #[test]
    fn official_save12_fixture_reads() {
        let bytes =
            include_bytes!("../../tests/fixtures/official-159.7/official-save12-empty.msav");
        let (meta, tiles) = SaveIO::read_map(&bytes[..]).expect("official Save12");
        assert_eq!(meta.version, 12);
        assert_eq!(tiles.len(), 4);
    }

    #[test]
    fn official_save12_patch_header_exact() {
        let patches = write_msav_content_patches_region(12).unwrap();
        assert_eq!(
            patches.len(),
            12,
            "Save12 empty readDataPatches is 12 bytes"
        );
        assert!(patches.iter().all(|b| *b == 0));
        let fixture =
            include_bytes!("../../tests/fixtures/official-159.7/official-save12-empty.msav");
        let (meta, _) = SaveIO::read_map(&fixture[..]).expect("fixture readable");
        assert_eq!(meta.version, 12);
    }

    #[test]
    fn save12_and_save13_patch_formats_are_not_interchangeable() {
        let save12 = write_msav_content_patches_region(12).unwrap();
        let save13 = write_msav_content_patches_region(13).unwrap();
        assert_eq!(save12.len(), 12);
        assert_eq!(save13.len(), 8);
        assert_ne!(save12, save13);
        assert!(SaveIO::validate_patches_region(12, &save13).is_err());
    }

    #[test]
    #[ignore = "run manually to refresh tests/fixtures/official-159.7/official-save12-empty.msav"]
    fn export_official_save12_fixture() {
        let mut map = HashMap::new();
        map.insert("mapname".into(), "official-save12-empty".into());
        map.insert("width".into(), "2".into());
        map.insert("height".into(), "2".into());
        let floors = [1i16; 4];
        let overlays = [0i16; 4];
        let blocks = [0i16; 4];
        let empty_tiles = dashmap::DashMap::new();
        let world = MsavWorld {
            width: 2,
            height: 2,
            floors: &floors,
            overlays: &overlays,
            puddles: &[],
            blocks: &blocks,
            team_blocks: None,
            dynamic_tiles: &empty_tiles,
            enemy_units: &[],
            runtime: None,
        };
        let bytes = write_msav_complete(&map, 12, &world).unwrap();
        let path = Path::new("tests/fixtures/official-159.7/official-save12-empty.msav");
        std::fs::write(path, &bytes).unwrap();
    }

    #[test]
    fn rust_save12_empty_patches_round_trip_read_map() {
        let mut map = HashMap::new();
        map.insert("mapname".into(), "save12-empty".into());
        map.insert("width".into(), "2".into());
        map.insert("height".into(), "2".into());
        let floors = [1i16; 4];
        let overlays = [0i16; 4];
        let blocks = [0i16; 4];
        let empty_tiles = dashmap::DashMap::new();
        let world = MsavWorld {
            width: 2,
            height: 2,
            floors: &floors,
            overlays: &overlays,
            puddles: &[],
            blocks: &blocks,
            team_blocks: None,
            dynamic_tiles: &empty_tiles,
            enemy_units: &[],
            runtime: None,
        };
        let bytes = write_msav_complete(&map, 12, &world).unwrap();
        let (meta, tiles) = SaveIO::read_map(&bytes[..]).unwrap();
        assert_eq!(meta.version, 12);
        assert_eq!(tiles.len(), 4);
    }
}
