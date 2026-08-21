use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{Cursor, Error, ErrorKind, Read, Write};

/// The validated network-world template embedded in the server binary. It is the
/// 158.1-compatible base whose map/content/metadata sections are replaced by MSAV
/// data; its player section is personalized per connection.
pub fn embedded_template() -> &'static [u8] {
    include_bytes!("../dummy_world.dat")
}

const MAX_WORLD_STREAM_SIZE: u64 = 64 * 1024 * 1024;
const PLAYER_REVISION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedPlayer {
    pub id: i32,
    pub name: String,
    pub color: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetworkMap {
    pub width: u16,
    pub height: u16,
    pub blocks: Vec<i16>,
    pub block_centers: Vec<bool>,
    pub floors: Vec<i16>,
    pub overlays: Vec<i16>,
    /// Raw `Tile.data` byte persisted by SaveVersion for block-specific state.
    /// Natural cave walls use it as their static-darkness depth.
    pub tile_data: Vec<u8>,
    pub buildings: Vec<NetworkBuilding>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetworkBuilding {
    pub position: i32,
    pub block: i16,
    pub health: f32,
    pub rotation: u8,
    pub team: u8,
    /// Complete BuildingComp base-module state from the map entity chunk.
    pub inventory: Vec<(i16, i32)>,
    pub power_links: Vec<i32>,
    pub power_status: f32,
    pub liquids: Vec<(i16, f32)>,
    pub enabled: bool,
    /// Subclass bytes (config/progress) are retained rather than discarded.
    /// Their schema is block-specific and is decoded by the live simulation.
    pub extra_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkMetadata {
    pub rules: String,
    pub locales: String,
    pub tags: Vec<(String, String)>,
}

impl NetworkMap {
    /// Tile positions marked with the official `Blocks.spawn` overlay (ID 1).
    pub fn enemy_spawns(&self) -> Vec<(i16, i16)> {
        self.overlays
            .iter()
            .enumerate()
            .filter(|(_, overlay)| **overlay == 1)
            .map(|(index, _)| {
                (
                    (index % usize::from(self.width)) as i16,
                    (index / usize::from(self.width)) as i16,
                )
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy)]
struct PlayerOffsets {
    wave: usize,
    wave_time: usize,
    tick: usize,
    id: usize,
    color: usize,
    name_length: usize,
    name: usize,
    name_end: usize,
    x: usize,
    y: usize,
}

/// Rewrites the player embedded in a Mindustry 8 `NetworkIO.writeWorld` stream.
/// This is necessary because the initial world contains the local player before
/// that player is added to normal entity snapshots.
pub fn personalize(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
) -> std::io::Result<Vec<u8>> {
    personalize_impl(compressed_template, player_id, name, color, None, None)
}

pub fn personalize_with_position(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
    position: Option<(f32, f32)>,
) -> std::io::Result<Vec<u8>> {
    personalize_impl(compressed_template, player_id, name, color, position, None)
}

#[allow(clippy::too_many_arguments)]
pub fn personalize_with_state(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
    position: (f32, f32),
    wave: u32,
    wave_time: f32,
    tick: f64,
) -> std::io::Result<Vec<u8>> {
    let wave = i32::try_from(wave)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "wave exceeds protocol range"))?;
    if !wave_time.is_finite() || wave_time < 0.0 || !tick.is_finite() || tick < 0.0 {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "invalid world timing state",
        ));
    }
    personalize_impl(
        compressed_template,
        player_id,
        name,
        color,
        Some(position),
        Some((wave, wave_time, tick)),
    )
}

/// Produces the exact `NetworkIO.writeWorld` layout consumed by the user's
/// Mindustry 158.1 desktop build. The newer source template stores an 8-byte
/// data-patch header before rules; 158.1 instead expects a content-patch count
/// immediately after ContentHeader.
#[allow(clippy::too_many_arguments)]
pub fn personalize_desktop_158_with_state(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
    position: (f32, f32),
    wave: u32,
    wave_time: f32,
    tick: f64,
) -> std::io::Result<Vec<u8>> {
    personalize_desktop_158_with_state_pvp(
        compressed_template,
        player_id,
        name,
        color,
        position,
        wave,
        wave_time,
        tick,
        false,
    )
}

/// Like `personalize_desktop_158_with_state` but rewrites the embedded rules
/// JSON to set `"pvp": true` when the server runs in PvP mode (the official
/// `state.rules.pvp` is serialized into the world stream via
/// `NetworkIO.writeWorld` -> `JsonIO.write(state.rules)`).
#[allow(clippy::too_many_arguments)]
pub fn personalize_desktop_158_with_state_pvp(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
    position: (f32, f32),
    wave: u32,
    wave_time: f32,
    tick: f64,
    pvp: bool,
) -> std::io::Result<Vec<u8>> {
    personalize_desktop_158_with_state_mode(
        compressed_template,
        player_id,
        name,
        color,
        position,
        wave,
        wave_time,
        tick,
        pvp,
        false,
    )
}

/// Personalize a world stream for the current official client (Build 159.7).
/// Keeps the Save13 `writeDataPatches` prefix required by `NetworkIO.writeWorld`.
#[allow(clippy::too_many_arguments)]
pub fn personalize_current_with_state_mode(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
    position: (f32, f32),
    wave: u32,
    wave_time: f32,
    tick: f64,
    pvp: bool,
    sandbox: bool,
) -> std::io::Result<Vec<u8>> {
    let template = if pvp || sandbox {
        inject_rules_mode(compressed_template, pvp, sandbox)?
    } else {
        compressed_template.to_vec()
    };
    personalize_with_state(
        &template, player_id, name, color, position, wave, wave_time, tick,
    )
}

/// Historical helper: rewrite a 159.7 template into the 158.1 `NetworkIO.writeWorld`
/// layout (no leading data-patch header; 1-byte content patches after the content header).
#[allow(clippy::too_many_arguments)]
pub fn personalize_desktop_158_with_state_mode(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
    position: (f32, f32),
    wave: u32,
    wave_time: f32,
    tick: f64,
    pvp: bool,
    sandbox: bool,
) -> std::io::Result<Vec<u8>> {
    let template = if pvp || sandbox {
        inject_rules_mode(compressed_template, pvp, sandbox)?
    } else {
        compressed_template.to_vec()
    };
    let personalized = personalize_with_state(
        &template, player_id, name, color, position, wave, wave_time, tick,
    )?;
    let mut world = decompress_limited(&personalized, "personalized world stream")?;
    let (_, content_end) = locate_network_content_range(&world)?;
    if world.get(..8) != Some(&[0, 0, 0, 2, 0, 0, 0, 0]) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "unexpected data-patch header in network template",
        ));
    }
    world.drain(..8);
    world.insert(content_end - 8, 0); // zero ContentPatch entries

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&world)?;
    encoder.finish()
}

fn personalize_impl(
    compressed_template: &[u8],
    player_id: i32,
    name: &str,
    color: i32,
    position: Option<(f32, f32)>,
    state: Option<(i32, f32, f64)>,
) -> std::io::Result<Vec<u8>> {
    if name.is_empty() || name.len() > u16::MAX as usize {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "invalid player name length",
        ));
    }

    let mut decoder = ZlibDecoder::new(compressed_template);
    let mut world = Vec::new();
    decoder
        .by_ref()
        .take(MAX_WORLD_STREAM_SIZE + 1)
        .read_to_end(&mut world)?;
    if world.len() as u64 > MAX_WORLD_STREAM_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "world stream is too large",
        ));
    }

    let offsets = locate_player(&world)?;
    world[offsets.id..offsets.id + 4].copy_from_slice(&player_id.to_be_bytes());
    world[offsets.color..offsets.color + 4].copy_from_slice(&color.to_be_bytes());
    if let Some((wave, wave_time, tick)) = state {
        world[offsets.wave..offsets.wave + 4].copy_from_slice(&wave.to_be_bytes());
        world[offsets.wave_time..offsets.wave_time + 4].copy_from_slice(&wave_time.to_be_bytes());
        world[offsets.tick..offsets.tick + 8].copy_from_slice(&tick.to_be_bytes());
    }
    world[offsets.name_length..offsets.name_length + 2]
        .copy_from_slice(&(name.len() as u16).to_be_bytes());
    world.splice(
        offsets.name..offsets.name_end,
        name.as_bytes().iter().copied(),
    );
    if let Some((x, y)) = position {
        let name_delta = name.len() as isize - (offsets.name_end - offsets.name) as isize;
        let x_offset = offsets
            .x
            .checked_add_signed(name_delta)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid player position offset"))?;
        let y_offset = offsets
            .y
            .checked_add_signed(name_delta)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid player position offset"))?;
        world[x_offset..x_offset + 4].copy_from_slice(&x.to_be_bytes());
        world[y_offset..y_offset + 4].copy_from_slice(&y.to_be_bytes());
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&world)?;
    encoder.finish()
}

pub fn inspect(compressed: &[u8]) -> std::io::Result<EmbeddedPlayer> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut world = Vec::new();
    decoder
        .by_ref()
        .take(MAX_WORLD_STREAM_SIZE + 1)
        .read_to_end(&mut world)?;
    if world.len() as u64 > MAX_WORLD_STREAM_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "world stream is too large",
        ));
    }
    let offsets = locate_player(&world)?;
    Ok(EmbeddedPlayer {
        id: i32::from_be_bytes(world[offsets.id..offsets.id + 4].try_into().unwrap()),
        color: i32::from_be_bytes(world[offsets.color..offsets.color + 4].try_into().unwrap()),
        name: String::from_utf8(world[offsets.name..offsets.name_end].to_vec())
            .map_err(|err| Error::new(ErrorKind::InvalidData, err))?,
    })
}

/// Extracts the authoritative block layer from a Mindustry 8 network world.
pub fn inspect_map(compressed: &[u8]) -> std::io::Result<NetworkMap> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut world = Vec::new();
    decoder
        .by_ref()
        .take(MAX_WORLD_STREAM_SIZE + 1)
        .read_to_end(&mut world)?;
    if world.len() as u64 > MAX_WORLD_STREAM_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "world stream is too large",
        ));
    }

    let offsets = locate_player(&world)?;
    let mut cursor = Cursor::new(world.as_slice());
    cursor.set_position(offsets.name_end as u64);
    // selected block, selected rotation, shooting, team, typing, unit ref, x/y
    advance(&mut cursor, 2 + 4 + 1 + 1 + 1 + 5 + 8, world.len())?;
    let selected_content = read_content_names(&mut cursor)?;
    let canonical_blocks = canonical_block_names()?;
    let block_remap = content_id_remap(&selected_content[1], &canonical_blocks)?;
    read_network_map_remapped(&mut cursor, Some(&block_remap))
}

pub fn inspect_metadata(compressed: &[u8]) -> std::io::Result<NetworkMetadata> {
    let world = decompress_limited(compressed, "world stream")?;
    read_network_metadata(&world)
}

pub fn inspect_timing(compressed: &[u8]) -> std::io::Result<(u32, f32, f64)> {
    let world = decompress_limited(compressed, "world stream")?;
    let offsets = locate_player(&world)?;
    let wave = i32::from_be_bytes(world[offsets.wave..offsets.wave + 4].try_into().unwrap());
    if wave < 0 {
        return Err(Error::new(ErrorKind::InvalidData, "negative world wave"));
    }
    let wave_time = f32::from_be_bytes(
        world[offsets.wave_time..offsets.wave_time + 4]
            .try_into()
            .unwrap(),
    );
    let tick = f64::from_be_bytes(world[offsets.tick..offsets.tick + 8].try_into().unwrap());
    if !wave_time.is_finite() || wave_time < 0.0 || !tick.is_finite() || tick < 0.0 {
        return Err(Error::new(ErrorKind::InvalidData, "invalid world timing"));
    }
    Ok((wave as u32, wave_time, tick))
}

pub fn inspect_team_count(compressed: &[u8]) -> std::io::Result<u32> {
    let world = decompress_limited(compressed, "world stream")?;
    let (_, map_end) = locate_network_map_range(&world)?;
    let mut cursor = Cursor::new(world.as_slice());
    cursor.set_position(map_end as u64);
    let teams = cursor.read_i32::<BigEndian>()?;
    u32::try_from(teams).map_err(|_| Error::new(ErrorKind::InvalidData, "negative team count"))
}

/// The raw UBJSON markers section carried by a network world stream.
pub fn inspect_markers(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    let world = decompress_limited(compressed, "world stream")?;
    let (_, map_end) = locate_network_map_range(&world)?;
    let (markers_start, custom_start, _) = locate_trailing_sections(&world, map_end)?;
    Ok(world[markers_start..custom_start].to_vec())
}

/// The raw custom-chunk section (`writeCustomChunks`) carried by a network
/// world stream.
pub fn inspect_custom_chunks(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    let world = decompress_limited(compressed, "world stream")?;
    let (_, map_end) = locate_network_map_range(&world)?;
    let (_, custom_start, custom_end) = locate_trailing_sections(&world, map_end)?;
    Ok(world[custom_start..custom_end].to_vec())
}

/// Decodes the `SaveVersion.writeTeamBlocks` section that follows the map
/// region of a network world stream (team ID → non-empty build plans).
/// The extracted plans are re-emitted verbatim by the world stream, so a
/// decoded `TeamBlocks` value is the authoritative per-team plan table.
pub fn inspect_team_plans(compressed: &[u8]) -> std::io::Result<crate::engine::typeio::TeamBlocks> {
    let world = decompress_limited(compressed, "world stream")?;
    let (_, map_end) = locate_network_map_range(&world)?;
    let (markers_start, _, _) = locate_trailing_sections(&world, map_end)?;
    let (plans, consumed) = crate::engine::typeio::TeamBlocks::decode(&world[map_end..])?;
    if map_end + consumed != markers_start {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "team-block section has trailing bytes before markers",
        ));
    }
    Ok(plans)
}

/// Replaces the `SaveVersion.writeMap` section of a network-world template with
/// the map region from an official MSAV file. Both formats use the exact same
/// map codec; the surrounding network rules, player and content mapping remain
/// those of the validated template.
pub fn replace_map_from_msav(
    compressed_template: &[u8],
    compressed_msav: &[u8],
) -> std::io::Result<Vec<u8>> {
    let mut template = decompress_limited(compressed_template, "world stream")?;
    let (metadata_start, metadata_end) = locate_network_metadata_range(&template)?;
    let (content_start, content_end) = locate_network_content_range(&template)?;
    let (map_start, map_end) = locate_network_map_range(&template)?;
    let (markers_start, _custom_start, custom_end) = locate_trailing_sections(&template, map_end)?;
    let extracted = extract_msav_regions(compressed_msav)?;
    let map = extracted.map;
    let msav_content = extracted.content;
    let metadata = metadata_from_msav_meta(&extracted.meta)?;
    let encoded_metadata = encode_network_metadata(&metadata)?;

    let mut map_cursor = Cursor::new(map.as_slice());
    let _ = read_network_map(&mut map_cursor)?;
    if map_cursor.position() != map.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "MSAV map region has trailing bytes",
        ));
    }

    // Splice from the end of the stream backwards so earlier offsets stay valid:
    // markers + custom chunks, then team blocks, then map, content and metadata.
    let mut markers_and_custom = extracted.markers;
    markers_and_custom.extend_from_slice(&extracted.custom);
    template.splice(markers_start..custom_end, markers_and_custom);
    template.splice(map_end..markers_start, extracted.team_blocks);
    template.splice(map_start..map_end, map);
    template.splice(content_start..content_end, msav_content);
    template.splice(metadata_start..metadata_end, encoded_metadata);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&template)?;
    let compressed = encoder.finish()?;
    // Validate the finished stream, including all offsets after the splice.
    let _ = inspect_map(&compressed)?;
    let world = decompress_limited(&compressed, "converted world stream")?;
    let (_, map_end) = locate_network_map_range(&world)?;
    let _ = locate_trailing_sections(&world, map_end)?;
    Ok(compressed)
}

/// Replaces the `SaveVersion.writeTeamBlocks` section of a network-world
/// template with the given live plans, exactly like the official server's
/// `NetServer.sendWorldData` (which writes the current `TeamData.plans` of
/// every active team on each connection). The template's own plans (from host
/// time) are superseded; late joiners see the current ghost builds.
pub fn replace_team_blocks(
    compressed_template: &[u8],
    plans: &crate::engine::typeio::TeamBlocks,
) -> std::io::Result<Vec<u8>> {
    let mut template = decompress_limited(compressed_template, "world stream")?;
    let (_, map_end) = locate_network_map_range(&template)?;
    let (markers_start, _, _custom_end) = locate_trailing_sections(&template, map_end)?;
    let encoded = plans.encode()?;
    // Splice backwards so earlier offsets stay valid.
    template.splice(map_end..markers_start, encoded);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&template)?;
    let compressed = encoder.finish()?;
    // Re-validate the finished stream, including all offsets after the splice.
    let world = decompress_limited(&compressed, "re-encoded world stream")?;
    let (_, map_end) = locate_network_map_range(&world)?;
    let (markers_start, _, _) = locate_trailing_sections(&world, map_end)?;
    let (decoded, consumed) = crate::engine::typeio::TeamBlocks::decode(&world[map_end..])?;
    if map_end + consumed != markers_start {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "team-block section has trailing bytes after replacement",
        ));
    }
    if decoded != *plans {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "team-block section did not round trip after replacement",
        ));
    }
    Ok(compressed)
}

fn reject_nonvanilla_patches(version: i32, patches: &[u8]) -> std::io::Result<()> {
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

struct ExtractedMsav {
    save_version: i32,
    meta: Vec<u8>,
    content: Vec<u8>,
    map: Vec<u8>,
    team_blocks: Vec<u8>,
    /// Overlay from `SaveVersion.readEntityMapping` (empty for v4, which
    /// skips the mapping table). Each pair is `(classId, entityName)`.
    entity_mapping: Vec<(u16, String)>,
    /// Remainder of the entities region after the team-block table
    /// (`SaveVersion.writeWorldEntities`). Empty for legacy v1-v3.
    world_entities: Vec<u8>,
    markers: Vec<u8>,
    custom: Vec<u8>,
}

fn decompress_limited(compressed: &[u8], label: &str) -> std::io::Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut decoded = Vec::new();
    decoder
        .by_ref()
        .take(MAX_WORLD_STREAM_SIZE + 1)
        .read_to_end(&mut decoded)?;
    if decoded.len() as u64 > MAX_WORLD_STREAM_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("{label} is too large"),
        ));
    }
    Ok(decoded)
}

/// Rewrites the embedded rules JSON of a compressed network template to force
/// `"pvp": true` (official `state.rules.pvp` serialized into the world
/// stream). The rules string is the first MUTF-8 after the 8-byte data-patch
/// header; the stream is recompressed with the patched rules (length changed,
/// all following offsets are relative to the recompressed buffer, so
/// `personalize_impl` re-locates them afterwards).
fn inject_rules_pvp(compressed_template: &[u8]) -> std::io::Result<Vec<u8>> {
    inject_rules_mode(compressed_template, true, false)
}

/// Applies mode fields to the serialized Rules JSON after loading map rules,
/// matching `Gamemode.apply`. Sandbox intentionally forces the v158.1 preset:
/// infinite resources, editable rules, waves available, and no wave timer.
fn inject_rules_mode(
    compressed_template: &[u8],
    pvp: bool,
    sandbox: bool,
) -> std::io::Result<Vec<u8>> {
    let world = decompress_limited(compressed_template, "template for mode rules")?;
    if world.len() < 8 || world[..8] != [0, 0, 0, 2, 0, 0, 0, 0] {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "unexpected data-patch header in mode template",
        ));
    }
    let mut cursor = Cursor::new(&world[8..]);
    let rules = crate::network::codec::read_modified_utf8_public(&mut cursor)?;
    let consumed = cursor.position() as usize;
    let rules_start = 8;
    let rules_end = rules_start + consumed;
    // The template rules use arc's unquoted-key JSON ({waveSpacing:...});
    // convert to strict JSON first (same helper as parse_wave_rules).
    let strict_rules = crate::network::units::arc_json_to_strict(&rules);
    let mut rules_value: serde_json::Value = serde_json::from_str(&strict_rules)
        .map_err(|err| Error::new(ErrorKind::InvalidData, format!("invalid rules JSON: {err}")))?;
    if let serde_json::Value::Object(map) = &mut rules_value {
        if pvp {
            map.insert("pvp".to_string(), serde_json::Value::Bool(true));
        }
        if sandbox {
            map.insert(
                "infiniteResources".to_string(),
                serde_json::Value::Bool(true),
            );
            map.insert("allowEditRules".to_string(), serde_json::Value::Bool(true));
            map.insert("waves".to_string(), serde_json::Value::Bool(true));
            map.insert("waveTimer".to_string(), serde_json::Value::Bool(false));
        }
    }
    let patched = serde_json::to_string(&rules_value)
        .map_err(|err| Error::new(ErrorKind::InvalidData, format!("rules JSON: {err}")))?;
    let patched_bytes = crate::network::codec::encode_modified_utf8(&patched);
    let mut out = Vec::with_capacity(world.len() + 8);
    out.extend_from_slice(&world[..rules_start]);
    out.extend_from_slice(&(patched_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(&patched_bytes);
    out.extend_from_slice(&world[rules_end..]);
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&out)?;
    encoder.finish()
}

fn locate_network_map_range(world: &[u8]) -> std::io::Result<(usize, usize)> {
    let offsets = locate_player(world)?;
    let mut cursor = Cursor::new(world);
    cursor.set_position(offsets.name_end as u64);
    advance(&mut cursor, 2 + 4 + 1 + 1 + 1 + 5 + 8, world.len())?;
    skip_content_header(&mut cursor)?;
    let start = cursor.position() as usize;
    let _ = read_network_map(&mut cursor)?;
    Ok((start, cursor.position() as usize))
}

fn locate_network_metadata_range(world: &[u8]) -> std::io::Result<(usize, usize)> {
    let mut cursor = Cursor::new(world);
    let patch_version = cursor.read_i32::<BigEndian>()?;
    let assets = cursor.read_i32::<BigEndian>()?;
    if patch_version != 2 || assets != 0 {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "network templates with data assets are not supported",
        ));
    }
    let start = cursor.position() as usize;
    skip_utf(&mut cursor)?;
    skip_utf(&mut cursor)?;
    skip_string_map(&mut cursor)?;
    Ok((start, cursor.position() as usize))
}

/// Locates the trailing network sections that follow the map region:
/// team build plans, the UBJSON markers object and the custom-chunk payload.
/// Returns `(markers_start, custom_start, custom_end)`; `custom_end` must be
/// the end of the stream.
fn locate_trailing_sections(
    world: &[u8],
    map_end: usize,
) -> std::io::Result<(usize, usize, usize)> {
    let mut cursor = Cursor::new(world);
    cursor.set_position(map_end as u64);
    skip_team_blocks(&mut cursor)?;
    let markers_start = cursor.position() as usize;
    let markers_len = ubjson_len(world, markers_start)?;
    let custom_start = markers_start + markers_len;
    let mut cursor = Cursor::new(world);
    cursor.set_position(custom_start as u64);
    let chunks = cursor.read_i32::<BigEndian>()?;
    if !(0..=256).contains(&chunks) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid network custom chunk count",
        ));
    }
    for _ in 0..chunks {
        skip_utf(&mut cursor)?;
        let length = cursor.read_i32::<BigEndian>()?;
        let length = usize::try_from(length)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "negative network chunk length"))?;
        if length > MAX_WORLD_STREAM_SIZE as usize {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "network custom chunk is too large",
            ));
        }
        advance(&mut cursor, length, world.len())?;
    }
    let custom_end = cursor.position() as usize;
    if custom_end != world.len() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "world stream has trailing bytes after custom chunks",
        ));
    }
    Ok((markers_start, custom_start, custom_end))
}

fn skip_team_blocks(cursor: &mut Cursor<&[u8]>) -> std::io::Result<()> {
    let start = cursor.position() as usize;
    let (_, consumed) = crate::engine::typeio::TeamBlocks::decode(&cursor.get_ref()[start..])?;
    cursor.set_position((start + consumed) as u64);
    Ok(())
}

struct SplitEntities {
    mapping: Vec<(u16, String)>,
    team_blocks: Vec<u8>,
    world_entities: Vec<u8>,
}

fn extract_team_blocks(entities: &[u8], has_mapping: bool) -> std::io::Result<Vec<u8>> {
    Ok(split_entities_region(entities, has_mapping)?.team_blocks)
}

fn split_entities_region(entities: &[u8], has_mapping: bool) -> std::io::Result<SplitEntities> {
    let mut cursor = Cursor::new(entities);
    // v4 saves carry no entity-ID mapping (`Save4.readEntities`); all other
    // supported versions read `readEntityMapping` before the team table.
    let mut mapping = Vec::new();
    if has_mapping {
        let mappings = cursor.read_i16::<BigEndian>()?;
        if !(0..=4096).contains(&mappings) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid entity mapping count",
            ));
        }
        for _ in 0..mappings {
            let id = cursor.read_i16::<BigEndian>()?;
            // EntityMapping.idMap is length 256; a save that remaps outside
            // that range throws ArrayIndexOutOfBoundsException in 158.1.
            if !(0..=255).contains(&id) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "entity mapping id is outside idMap",
                ));
            }
            let name = read_utf(&mut cursor)?;
            mapping.push((id as u16, name));
        }
    }
    let start = cursor.position() as usize;
    let (_, consumed) = crate::engine::typeio::TeamBlocks::decode(&entities[start..])?;
    let team_end = start + consumed;
    Ok(SplitEntities {
        mapping,
        team_blocks: entities[start..team_end].to_vec(),
        world_entities: entities[team_end..].to_vec(),
    })
}

fn read_network_metadata(world: &[u8]) -> std::io::Result<NetworkMetadata> {
    let mut cursor = Cursor::new(world);
    let patch_version = cursor.read_i32::<BigEndian>()?;
    let assets = cursor.read_i32::<BigEndian>()?;
    if patch_version != 2 || assets != 0 {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "network streams with data assets are not supported",
        ));
    }
    let rules = read_utf(&mut cursor)?;
    let locales = read_utf(&mut cursor)?;
    let tags = read_string_map(&mut cursor)?;
    Ok(NetworkMetadata {
        rules,
        locales,
        tags,
    })
}

fn metadata_from_msav_meta(meta: &[u8]) -> std::io::Result<NetworkMetadata> {
    let tags = read_string_map(&mut Cursor::new(meta))?;
    let rules = tags
        .iter()
        .find_map(|(key, value)| (key == "rules").then(|| value.clone()))
        .unwrap_or_else(|| "{}".to_owned());
    let locales = tags
        .iter()
        .find_map(|(key, value)| (key == "locales").then(|| value.clone()))
        .unwrap_or_else(|| "{}".to_owned());
    Ok(NetworkMetadata {
        rules,
        locales,
        tags,
    })
}

fn encode_network_metadata(metadata: &NetworkMetadata) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    write_utf(&mut output, &metadata.rules)?;
    write_utf(&mut output, &metadata.locales)?;
    let count = i16::try_from(metadata.tags.len())
        .map_err(|_| Error::new(ErrorKind::InvalidData, "too many map tags"))?;
    output.write_i16::<BigEndian>(count)?;
    for (key, value) in &metadata.tags {
        write_utf(&mut output, key)?;
        write_utf(&mut output, value)?;
    }
    Ok(output)
}

fn locate_network_content_range(world: &[u8]) -> std::io::Result<(usize, usize)> {
    let offsets = locate_player(world)?;
    let mut cursor = Cursor::new(world);
    cursor.set_position(offsets.name_end as u64);
    advance(&mut cursor, 2 + 4 + 1 + 1 + 1 + 5 + 8, world.len())?;
    let start = cursor.position() as usize;
    skip_content_header(&mut cursor)?;
    Ok((start, cursor.position() as usize))
}

fn network_content_names(world: &[u8]) -> std::io::Result<Vec<Vec<String>>> {
    let offsets = locate_player(world)?;
    let mut cursor = Cursor::new(world);
    cursor.set_position(offsets.name_end as u64);
    advance(&mut cursor, 2 + 4 + 1 + 1 + 1 + 5 + 8, world.len())?;
    let start = cursor.position() as usize;
    let mut content_cursor = Cursor::new(&world[start..]);
    read_content_names(&mut content_cursor)
}

fn read_content_names(cursor: &mut Cursor<&[u8]>) -> std::io::Result<Vec<Vec<String>>> {
    let mapped = cursor.read_u8()?;
    if mapped > 18 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid content type count",
        ));
    }
    let mut names = vec![Vec::new(); 18];
    for _ in 0..mapped {
        let content_type = usize::from(cursor.read_u8()?);
        if content_type >= names.len() {
            return Err(Error::new(ErrorKind::InvalidData, "invalid content type"));
        }
        let total = cursor.read_i16::<BigEndian>()?;
        if !(0..=4096).contains(&total) {
            return Err(Error::new(ErrorKind::InvalidData, "invalid content count"));
        }
        for _ in 0..total {
            let length = usize::from(cursor.read_u16::<BigEndian>()?);
            let start = cursor.position() as usize;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= cursor.get_ref().len())
                .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated content name"))?;
            names[content_type].push(
                String::from_utf8(cursor.get_ref()[start..end].to_vec())
                    .map_err(|err| Error::new(ErrorKind::InvalidData, err))?,
            );
            cursor.set_position(end as u64);
        }
    }
    Ok(names)
}

fn content_id_remap(saved: &[String], current: &[String]) -> std::io::Result<Vec<i16>> {
    saved
        .iter()
        .map(|name| {
            let name = block_name_fallback(name);
            let id = current
                .iter()
                .position(|current| current == name)
                .map_or(Ok(-1), |id| {
                    i16::try_from(id)
                        .map_err(|_| Error::new(ErrorKind::InvalidData, "content ID is too large"))
                })?;
            Ok(id)
        })
        .collect()
}

fn block_name_fallback(name: &str) -> &str {
    match name {
        "dart-mech-pad" | "dart-ship-pad" | "javelin-ship-pad" | "trident-ship-pad"
        | "glaive-ship-pad" | "alpha-mech-pad" | "tau-mech-pad" | "omega-mech-pad"
        | "delta-mech-pad" => "legacy-mech-pad",
        "draug-factory" | "spirit-factory" | "phantom-factory" | "wraith-factory"
        | "dagger-factory" | "crawler-factory" => "legacy-unit-factory",
        "ghoul-factory" | "revenant-factory" => "legacy-unit-factory-air",
        "titan-factory" | "fortress-factory" => "legacy-unit-factory-ground",
        "mass-conveyor" => "payload-conveyor",
        "turbine-generator" => "steam-generator",
        "rocks" | "cliffs" => "stone-wall",
        "sporerocks" => "spore-wall",
        "icerocks" => "ice-wall",
        "dunerocks" => "dune-wall",
        "sandrocks" => "sand-wall",
        "shalerocks" => "shale-wall",
        "snowrocks" => "snow-wall",
        "saltrocks" => "salt-wall",
        "dirtwall" => "dirt-wall",
        "ignarock" => "basalt",
        "holostone" => "dacite",
        "holostone-wall" => "dacite-wall",
        "rock" => "boulder",
        "snowrock" => "snow-boulder",
        "craters" => "crater-stone",
        "deepwater" => "deep-water",
        "water" => "shallow-water",
        "sand" => "sand-floor",
        "slag" => "molten-slag",
        "cryofluidmixer" => "cryofluid-mixer",
        "block-forge" => "constructor",
        "block-unloader" => "payload-unloader",
        "block-loader" => "payload-loader",
        "thermal-pump" => "impulse-pump",
        "alloy-smelter" => "surge-smelter",
        "steam-vent" => "rhyolite-vent",
        "fabricator" => "tank-fabricator",
        "basic-reconstructor" => "refabricator",
        _ => name,
    }
}

fn canonical_block_names() -> std::io::Result<Vec<String>> {
    include_str!("../game/block_names.tsv")
        .lines()
        .enumerate()
        .map(|(expected_id, line)| {
            let (id, name) = line.split_once('\t').ok_or_else(|| {
                Error::new(ErrorKind::InvalidData, "invalid block name manifest row")
            })?;
            let id = id
                .parse::<usize>()
                .map_err(|err| Error::new(ErrorKind::InvalidData, err))?;
            if id != expected_id || name.is_empty() {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "non-contiguous block name manifest",
                ));
            }
            Ok(name.to_owned())
        })
        .collect()
}

/// World-entity payload of an MSAV (`writeWorldEntities` after the team table)
/// together with the save version and `readEntityMapping` overlay needed to
/// decode chunk framing (u16 vs i32, serialized id vs allocated id).
#[derive(Debug, Clone)]
pub struct MsavWorldEntitySection {
    pub save_version: i32,
    pub mapping: Vec<(u16, String)>,
    pub bytes: Vec<u8>,
}

/// World-entity bytes of an MSAV (`writeWorldEntities` after the team table).
pub fn msav_world_entity_bytes(compressed_msav: &[u8]) -> std::io::Result<Vec<u8>> {
    Ok(msav_world_entity_section(compressed_msav)?.bytes)
}

pub fn msav_world_entity_section(
    compressed_msav: &[u8],
) -> std::io::Result<MsavWorldEntitySection> {
    let extracted = extract_msav_regions(compressed_msav)?;
    Ok(MsavWorldEntitySection {
        save_version: extracted.save_version,
        mapping: extracted.entity_mapping,
        bytes: extracted.world_entities,
    })
}

fn extract_msav_regions(compressed_msav: &[u8]) -> std::io::Result<ExtractedMsav> {
    let decoded = decompress_limited(compressed_msav, "MSAV")?;
    let mut cursor = Cursor::new(decoded.as_slice());
    let mut magic = [0u8; 4];
    cursor.read_exact(&mut magic)?;
    if &magic != b"MSAV" {
        return Err(Error::new(ErrorKind::InvalidData, "invalid MSAV magic"));
    }
    let version = cursor.read_i32::<BigEndian>()?;
    // Round-73 M2 (documented extension): the 158.1 baseline only ships
    // Save1..Save11 (SaveIO.versionArray bytecode). The extractor accepts
    // v12/v13 as a PORT EXTENSION — the region layout was taken from the
    // newer source tree and the user's own fixtures are v13. This does NOT
    // claim 158.1 interop for those versions (the official desktop cannot
    // open them); `SaveIO::read_meta`/`read_map` still reject them so the
    // save-reader boundary stays baseline-strict.
    if !(1..=13).contains(&version) {
        return Err(Error::new(
            ErrorKind::Unsupported,
            format!("MSAV version {version} is not supported"),
        ));
    }
    // Official region layout (SaveVersion.read, versions/Save*.java):
    //   meta, [patches >=12], content, map, entities, [markers >=8], custom.
    // Official layouts (versions/Save*.java):
    //   v1-3  LegacySaveVersion:     meta, content, map, entities (no
    //         markers, no custom region; the map uses the legacy block
    //         format and the entities region is readLegacyEntities — v3
    //         prefixes a raw-i32 team plans table).
    //   v4    LegacySaveVersion2:   meta, content, map, entities (no markers,
    //         no custom region; the entities region has NO entity-ID mapping,
    //         Save4.readEntities reads team blocks + world entities directly).
    //   v5-6  LegacyRegionSaveVersion: meta, content, map, entities (no
    //         markers, no custom region at all).
    //   v7    ShortChunkSaveVersion:   meta, content, map, entities, custom.
    //   v8-10 SaveVersion.read:        meta, content, map, entities, markers, custom.
    //   v11   Save11.read:             meta, content, patches, map, entities,
    //         markers, custom (patches AFTER content, unlike >=12).
    //   v12+  SaveVersion.read:        meta, patches, content, map, entities,
    //         markers, custom.
    let (content_region, map_region, markers_region, custom_region) = match version {
        1..=3 => (1usize, 2usize, None, None),
        4..=6 => (1usize, 2usize, None, None),
        7 => (1, 2, None, Some(4)),
        8..=10 => (1, 2, Some(4), Some(5)),
        11 => (1, 3, Some(5), Some(6)),
        12..=13 => (2, 3, Some(5), Some(6)),
        _ => unreachable!(),
    };
    let entities_region = map_region + 1;
    let last_region = custom_region.unwrap_or(entities_region);
    let mut regions = Vec::with_capacity(last_region + 1);
    for _ in 0..=last_region {
        let length = cursor.read_i32::<BigEndian>()?;
        let length = usize::try_from(length)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "negative MSAV region length"))?;
        if length > MAX_WORLD_STREAM_SIZE as usize {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "MSAV region is too large",
            ));
        }
        let start = cursor.position() as usize;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= decoded.len())
            .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated MSAV region"))?;
        regions.push(decoded[start..end].to_vec());
        cursor.set_position(end as u64);
    }
    if cursor.position() != decoded.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "MSAV has trailing bytes after its custom-chunk region",
        ));
    }
    let content = regions[content_region].clone();
    if version == 11 {
        reject_nonvanilla_patches(version, &regions[2])?;
    } else if version >= 12 {
        reject_nonvanilla_patches(version, &regions[1])?;
    }
    let map = regions[map_region].clone();
    let (entity_mapping, team_blocks, world_entities) = match version {
        // v1-3 entities: legacy groups (v3 additionally carries a raw-i32
        // team plans table).  No entity-ID mapping exists before v5.
        1..=3 => (
            Vec::new(),
            extract_legacy_entities(&regions[entities_region], version)?,
            Vec::new(),
        ),
        // Save4 (v4) skips `readEntityMapping`: its entities region starts
        // with the team-block table, followed directly by world entities.
        // Every newer version (v5+) prefixes the table with the mapping.
        _ => {
            let split = split_entities_region(&regions[entities_region], version >= 5)?;
            (split.mapping, split.team_blocks, split.world_entities)
        }
    };
    let markers = match markers_region {
        Some(index) => validate_markers(&regions[index])?,
        // Writers predating the markers region serialize an empty object.
        None => b"{}".to_vec(),
    };
    let custom = match custom_region {
        Some(index) => validate_custom_chunks(&regions[index])?,
        // Writers predating the custom region serialize an empty chunk list.
        None => 0i32.to_be_bytes().to_vec(),
    };
    let map = if version <= 3 {
        // Legacy block section: no packed byte; building chunks are u16
        // prefixed.  The chunk/run split depends on `block.hasBuilding()`,
        // resolved through the save's own content header names.
        let mut content_cursor = Cursor::new(content.as_slice());
        let content_names = read_content_names(&mut content_cursor)?;
        if content_names[1].is_empty() {
            return Err(Error::new(
                ErrorKind::Unsupported,
                "v1-v3 save has no block list in its content header",
            ));
        }
        let canonical_blocks = canonical_block_names()?;
        let remap = content_id_remap(&content_names[1], &canonical_blocks)?;
        upgrade_legacy_map_chunks(&map, &remap)?
    } else if version <= 9 {
        upgrade_short_map_chunks(&map)?
    } else {
        map
    };
    Ok(ExtractedMsav {
        save_version: version,
        meta: regions[0].clone(),
        content,
        map,
        team_blocks,
        entity_mapping,
        world_entities,
        markers,
        custom,
    })
}

/// Validates the UBJSON markers payload (`MapMarkers.write` uses
/// `JsonIO.writeBytes` with `UBJsonWriter`) and returns it verbatim, so the
/// network stream carries the exact marker object of the map.
fn validate_markers(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let consumed = ubjson_len(bytes, 0)?;
    if consumed != bytes.len() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "markers region has trailing bytes",
        ));
    }
    Ok(bytes.to_vec())
}

/// Validates the `writeCustomChunks` payload (int count, then UTF name +
/// int length + opaque chunk data) and returns it verbatim.
fn validate_custom_chunks(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut cursor = Cursor::new(bytes);
    let chunks = cursor.read_i32::<BigEndian>()?;
    if !(0..=256).contains(&chunks) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid custom chunk count",
        ));
    }
    for _ in 0..chunks {
        skip_utf(&mut cursor)?;
        let length = cursor.read_i32::<BigEndian>()?;
        let length = usize::try_from(length)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "negative custom chunk length"))?;
        if length > MAX_WORLD_STREAM_SIZE as usize {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "custom chunk is too large",
            ));
        }
        advance(&mut cursor, length, bytes.len())?;
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "custom chunk region has trailing bytes",
        ));
    }
    Ok(bytes.to_vec())
}

/// Total byte length of the single UBJSON value starting at `start`.
/// Mirrors the exact grammar of `arc.util.serialization.UBJsonWriter`
/// (verified against the 158.1 desktop JAR): objects/arrays bracket values,
/// names and `S` strings are length-prefixed (`i`/`I`/`l`), primitive arrays
/// may carry `$` type and `#` count markers, and scalars use `i`/`I`/`l`/`L`
/// for integers, `d`/`D` for float32/float64, `T`/`F`/`Z`/`N`/`C` for the rest.
fn ubjson_len(data: &[u8], start: usize) -> std::io::Result<usize> {
    fn value_len(data: &[u8], pos: usize, depth: usize) -> std::io::Result<usize> {
        if depth > 64 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "UBJSON nesting too deep",
            ));
        }
        let byte = *data
            .get(pos)
            .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON value"))?;
        Ok(match byte {
            b'{' => {
                let mut p = pos + 1;
                loop {
                    let next = *data.get(p).ok_or_else(|| {
                        Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON object")
                    })?;
                    if next == b'}' {
                        return Ok(p + 1 - pos);
                    }
                    let (_, key) = ubjson_string(data, p)?;
                    p += key;
                    p += value_len(data, p, depth + 1)?;
                }
            }
            b'[' => {
                let mut p = pos + 1;
                let mut typed: Option<u8> = None;
                loop {
                    let next = *data.get(p).ok_or_else(|| {
                        Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON array")
                    })?;
                    if next == b']' {
                        return Ok(p + 1 - pos);
                    }
                    if next == b'$' {
                        typed = Some(*data.get(p + 1).ok_or_else(|| {
                            Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON type marker")
                        })?);
                        p += 2;
                        continue;
                    }
                    if next == b'#' {
                        let (count, count_len) = ubjson_count(data, p + 1)?;
                        p += 1 + count_len;
                        for _ in 0..count {
                            p += match typed {
                                Some(t) => ubjson_typed_len(data, p, t)?,
                                None => value_len(data, p, depth + 1)?,
                            };
                        }
                        continue;
                    }
                    p += match typed {
                        Some(t) => ubjson_typed_len(data, p, t)?,
                        None => value_len(data, p, depth + 1)?,
                    };
                }
            }
            b'Z' | b'T' | b'F' | b'N' => 1,
            b'i' | b'U' => 2,
            b'I' | b'C' => 3,
            b'l' | b'd' => 5,
            b'L' | b'D' => 9,
            b'S' => {
                let (_, total) = ubjson_string(data, pos + 1)?;
                total + 1
            }
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown UBJSON value type {other:#04x}"),
                ));
            }
        })
    }
    fn ubjson_string(data: &[u8], pos: usize) -> std::io::Result<(usize, usize)> {
        let marker = *data
            .get(pos)
            .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON string"))?;
        let (length, width) = match marker {
            b'i' => {
                let length = usize::from(*data.get(pos + 1).ok_or_else(|| {
                    Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON string")
                })?);
                (length, 2usize)
            }
            b'I' => {
                let mut raw = [0u8; 2];
                raw.copy_from_slice(data.get(pos + 1..pos + 3).ok_or_else(|| {
                    Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON string")
                })?);
                (usize::from(u16::from_be_bytes(raw)), 3usize)
            }
            b'l' => {
                let mut raw = [0u8; 4];
                raw.copy_from_slice(data.get(pos + 1..pos + 5).ok_or_else(|| {
                    Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON string")
                })?);
                (
                    usize::try_from(i32::from_be_bytes(raw)).map_err(|_| {
                        Error::new(ErrorKind::InvalidData, "negative UBJSON string length")
                    })?,
                    5usize,
                )
            }
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown UBJSON string length marker {other:#04x}"),
                ));
            }
        };
        if length > MAX_WORLD_STREAM_SIZE as usize {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "UBJSON string is too large",
            ));
        }
        let total = width + length;
        if pos + total > data.len() {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "truncated UBJSON string",
            ));
        }
        Ok((length, total))
    }
    fn ubjson_count(data: &[u8], pos: usize) -> std::io::Result<(u64, usize)> {
        let marker = *data
            .get(pos)
            .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON count"))?;
        let (value, width) = match marker {
            b'i' => (u64::from(data[pos + 1]), 2usize),
            b'U' => (u64::from(data[pos + 1]), 2),
            b'I' => {
                let mut raw = [0u8; 2];
                raw.copy_from_slice(data.get(pos + 1..pos + 3).ok_or_else(|| {
                    Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON count")
                })?);
                (u64::from(u16::from_be_bytes(raw)), 3)
            }
            b'l' => {
                let mut raw = [0u8; 4];
                raw.copy_from_slice(data.get(pos + 1..pos + 5).ok_or_else(|| {
                    Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON count")
                })?);
                (u64::from(u32::from_be_bytes(raw)), 5)
            }
            b'L' => {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(data.get(pos + 1..pos + 9).ok_or_else(|| {
                    Error::new(ErrorKind::UnexpectedEof, "truncated UBJSON count")
                })?);
                (u64::from_be_bytes(raw), 9)
            }
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown UBJSON count marker {other:#04x}"),
                ));
            }
        };
        if value > 1_000_000 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "UBJSON count is too large",
            ));
        }
        Ok((value, width))
    }
    fn ubjson_typed_len(data: &[u8], pos: usize, ty: u8) -> std::io::Result<usize> {
        Ok(match ty {
            b'Z' | b'T' | b'F' | b'N' | b'i' | b'U' => 1,
            b'I' | b'C' => 2,
            b'l' | b'd' => 4,
            b'L' | b'D' => 8,
            // `$ S` typed arrays write each element without its `S` byte.
            b'S' => {
                let (_, total) = ubjson_string(data, pos)?;
                total
            }
            other => {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    format!("unknown UBJSON typed element {other:#04x}"),
                ));
            }
        })
    }
    let consumed = value_len(data, start, 0)?;
    if start + consumed > data.len() {
        return Err(Error::new(
            ErrorKind::UnexpectedEof,
            "UBJSON value exceeds payload",
        ));
    }
    Ok(consumed)
}

fn upgrade_short_map_chunks(map: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut input = Cursor::new(map);
    let width = input.read_u16::<BigEndian>()?;
    let height = input.read_u16::<BigEndian>()?;
    let total = usize::from(width)
        .checked_mul(usize::from(height))
        .filter(|total| *total <= 4_000_000)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid legacy map dimensions"))?;
    let mut output = Vec::with_capacity(map.len() + 1024);
    output.write_u16::<BigEndian>(width)?;
    output.write_u16::<BigEndian>(height)?;

    let mut index = 0usize;
    while index < total {
        let floor = input.read_i16::<BigEndian>()?;
        let overlay = input.read_i16::<BigEndian>()?;
        let run = input.read_u8()?;
        output.write_i16::<BigEndian>(floor)?;
        output.write_i16::<BigEndian>(overlay)?;
        output.write_u8(run)?;
        index = index
            .checked_add(usize::from(run) + 1)
            .filter(|index| *index <= total)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "legacy floor run exceeds map"))?;
    }

    index = 0;
    while index < total {
        let block = input.read_i16::<BigEndian>()?;
        let packed = input.read_u8()?;
        output.write_i16::<BigEndian>(block)?;
        output.write_u8(packed)?;
        let has_entity = packed & 1 != 0;
        let has_old_data = packed & 2 != 0;
        let has_new_data = packed & 4 != 0;
        if has_new_data {
            let mut data = [0u8; 7];
            input.read_exact(&mut data)?;
            output.write_all(&data)?;
        }
        if has_entity {
            let center = input.read_u8()?;
            output.write_u8(center)?;
            if center != 0 {
                let length = usize::from(input.read_u16::<BigEndian>()?);
                let start = input.position() as usize;
                let end = start
                    .checked_add(length)
                    .filter(|end| *end <= map.len())
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::UnexpectedEof,
                            "legacy building chunk is truncated",
                        )
                    })?;
                output.write_i32::<BigEndian>(
                    i32::try_from(length)
                        .map_err(|_| Error::new(ErrorKind::InvalidData, "chunk is too large"))?,
                )?;
                output.write_all(&map[start..end])?;
                input.set_position(end as u64);
            }
        } else if has_old_data {
            output.write_u8(input.read_u8()?)?;
        } else if !has_new_data {
            let run = input.read_u8()?;
            output.write_u8(run)?;
            index = index
                .checked_add(usize::from(run))
                .filter(|index| *index < total)
                .ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "legacy block run exceeds map")
                })?;
        }
        index += 1;
    }
    if input.position() != map.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "legacy map region has trailing bytes",
        ));
    }
    Ok(output)
}

/// Converts a v1-v3 (`LegacySaveVersion.readMap`) map region to the modern
/// packed-byte format used by the network world stream.  The legacy block
/// section has no per-tile packed byte: a building chunk (u16 length)
/// follows every block that has a building, otherwise a run byte follows.
/// `remap` maps the saved block ids (as listed in the save's own content
/// header) to modern registry ids; the official `synthetic` flag decides the
/// chunk/run split (`Block.hasBuilding()` in the official reader).
/// // SOL-008 REMAINING: multiblock footprints written as air in legacy
/// saves are not re-expanded (the port has no per-id size table here), so
/// legacy cores render at 1x1 on the network path.
fn upgrade_legacy_map_chunks(map: &[u8], remap: &[i16]) -> std::io::Result<Vec<u8>> {
    let mut input = Cursor::new(map);
    let width = input.read_u16::<BigEndian>()?;
    let height = input.read_u16::<BigEndian>()?;
    let total = usize::from(width)
        .checked_mul(usize::from(height))
        .filter(|total| *total <= 4_000_000)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid legacy map dimensions"))?;
    let mut output = Vec::with_capacity(map.len() + 1024);
    output.write_u16::<BigEndian>(width)?;
    output.write_u16::<BigEndian>(height)?;

    let mut index = 0usize;
    while index < total {
        let floor = input.read_i16::<BigEndian>()?;
        let overlay = input.read_i16::<BigEndian>()?;
        let run = input.read_u8()?;
        output.write_i16::<BigEndian>(floor)?;
        output.write_i16::<BigEndian>(overlay)?;
        output.write_u8(run)?;
        index = index
            .checked_add(usize::from(run) + 1)
            .filter(|index| *index <= total)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "legacy floor run exceeds map"))?;
    }

    let has_building = |block: i16| -> bool {
        let id = usize::try_from(block).unwrap_or(usize::MAX);
        remap.get(id).is_some_and(|&modern| {
            modern >= 0 && crate::game::content::block_pathing(modern).synthetic
        })
    };
    index = 0;
    while index < total {
        let block = input.read_i16::<BigEndian>()?;
        output.write_i16::<BigEndian>(block)?;
        if has_building(block) {
            let length = usize::from(input.read_u16::<BigEndian>()?);
            let start = input.position() as usize;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= map.len())
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::UnexpectedEof,
                        "legacy building chunk is truncated",
                    )
                })?;
            let chunk = &map[start..end];
            input.set_position(end as u64);
            let converted = convert_legacy_building_chunk(chunk)?;
            output.write_u8(1)?; // packed: has entity
            output.write_u8(1)?; // center
            output.write_i32::<BigEndian>(
                i32::try_from(converted.len())
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "chunk is too large"))?,
            )?;
            output.write_all(&converted)?;
        } else {
            let run = input.read_u8()?;
            output.write_u8(0)?; // packed: no entity, no data
            output.write_u8(run)?;
            index = index
                .checked_add(usize::from(run))
                .filter(|index| *index < total)
                .ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "legacy block run exceeds map")
                })?;
        }
        index += 1;
    }
    if input.position() != map.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "legacy map region has trailing bytes",
        ));
    }
    Ok(output)
}

/// Translates a v1-v3 legacy building chunk to the modern base header:
/// `[u8 revision][u16 health][i8 packedrot][modules+subclass]` →
/// `[revision][f32 health][(rotation & 0x7f) | 0x80][team][entityVersion 0][tail]`.
/// The module/subclass remainder is preserved raw; the modern network reader
/// consumes the legacy consume bool (entity version 0) and keeps the rest as
/// `extra_data`, so the chunk stays bounded.
/// // SOL-008 REMAINING: the legacy ItemModule/PowerModule/LiquidModule
/// layouts and the subclass tails are not decoded — legacy buildings load
/// with default module state on the network path.
fn convert_legacy_building_chunk(chunk: &[u8]) -> std::io::Result<Vec<u8>> {
    let header = crate::engine::save_io::decode_legacy_chunk_header(chunk)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "legacy building chunk is too short"))?;
    let tail = &chunk[header.tail_start..];
    let mut out = Vec::with_capacity(chunk.len() + 8);
    out.push(header.revision);
    out.extend_from_slice(&header.health.to_be_bytes());
    out.push((header.rotation & 0x7f) | 0x80);
    out.push(header.team);
    out.push(0); // entity base version 0: no persisted modules
    out.extend_from_slice(tail);
    Ok(out)
}

/// Extracts the team-blocks section of a v1-v3 entities region.  Save1/Save2
/// have no plans table at all (the region is `readLegacyEntities` only), so
/// the empty official team table is synthesized; Save3 carries a raw-i32
/// plans table (no TypeIO wrapper) followed by the legacy entity groups.
/// The opaque entity chunks are validated and skipped.
/// // SOL-008 REMAINING: legacy world-entity payloads are not decoded; the
/// MSAV reader preserves them raw, but the network path discards them.
fn extract_legacy_entities(entities: &[u8], version: i32) -> std::io::Result<Vec<u8>> {
    let mut cursor = Cursor::new(entities);
    let mut plans_bytes = Vec::new();
    if version == 3 {
        // Save3.readEntities: i32 team count, per team id/plan-count, then
        // x/y/rotation/block shorts + a RAW i32 config per plan.
        let team_count = cursor.read_i32::<BigEndian>()?;
        if !(0..=256).contains(&team_count) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid legacy team count",
            ));
        }
        let mut teams = Vec::with_capacity(team_count as usize);
        for _ in 0..team_count {
            let team = cursor.read_i32::<BigEndian>()?;
            let plan_count = cursor.read_i32::<BigEndian>()?;
            if !(0..=1000).contains(&plan_count) {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid legacy plan count",
                ));
            }
            let mut plans = Vec::with_capacity(plan_count as usize);
            for _ in 0..plan_count {
                let x = cursor.read_i16::<BigEndian>()?;
                let y = cursor.read_i16::<BigEndian>()?;
                let rotation = cursor.read_i16::<BigEndian>()?;
                let block = cursor.read_i16::<BigEndian>()?;
                let config = cursor.read_i32::<BigEndian>()?;
                // v3 stored the raw int; the network stream needs the
                // TypeIO Integer form (tag 1 + value).
                let mut config_bytes = vec![1u8];
                config_bytes.extend_from_slice(&config.to_be_bytes());
                plans.push(crate::engine::typeio::TeamBlockPlan {
                    x,
                    y,
                    rotation,
                    block,
                    config: config_bytes,
                });
            }
            teams.push(crate::engine::typeio::TeamPlans { team, plans });
        }
        plans_bytes = crate::engine::typeio::TeamBlocks { teams }.encode()?;
    }
    // readLegacyEntities tail: u8 group count, per group an i32 amount, then
    // that many u16-prefixed opaque entity chunks.
    let groups = cursor.read_u8()?;
    for _ in 0..groups {
        let amount = cursor.read_i32::<BigEndian>()?;
        if !(0..=1_000_000).contains(&amount) {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "invalid legacy entity group size",
            ));
        }
        for _ in 0..amount {
            let length = usize::from(cursor.read_u16::<BigEndian>()?);
            advance(&mut cursor, length, entities.len())?;
        }
    }
    if cursor.position() != entities.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "legacy entities region has trailing bytes",
        ));
    }
    if plans_bytes.is_empty() {
        // v1/v2 have no plans table: synthesize the empty official team
        // table (sharded team, no plans), mirroring the empty markers
        // synthesis for pre-marker save versions.
        let mut out = Vec::new();
        out.extend_from_slice(&1i32.to_be_bytes());
        out.extend_from_slice(&1i32.to_be_bytes());
        out.extend_from_slice(&0i32.to_be_bytes());
        Ok(out)
    } else {
        Ok(plans_bytes)
    }
}

fn skip_content_header(cursor: &mut Cursor<&[u8]>) -> std::io::Result<()> {
    let mapped = cursor.read_u8()?;
    if mapped > 18 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid content type count",
        ));
    }
    for _ in 0..mapped {
        let content_type = cursor.read_u8()?;
        if content_type >= 18 {
            return Err(Error::new(ErrorKind::InvalidData, "invalid content type"));
        }
        let total = cursor.read_i16::<BigEndian>()?;
        if !(0..=4096).contains(&total) {
            return Err(Error::new(ErrorKind::InvalidData, "invalid content count"));
        }
        for _ in 0..total {
            skip_utf(cursor)?;
        }
    }
    Ok(())
}

fn read_module_count(cursor: &mut Cursor<&[u8]>, legacy: bool) -> std::io::Result<usize> {
    let count = if legacy {
        usize::from(cursor.read_u8()?)
    } else {
        usize::try_from(cursor.read_i16::<BigEndian>()?)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "negative module count"))?
    };
    if count > 4096 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "module count exceeds limit",
        ));
    }
    Ok(count)
}

fn read_network_map(cursor: &mut Cursor<&[u8]>) -> std::io::Result<NetworkMap> {
    read_network_map_remapped(cursor, None)
}

fn read_network_map_remapped(
    cursor: &mut Cursor<&[u8]>,
    block_remap: Option<&[i16]>,
) -> std::io::Result<NetworkMap> {
    const MAX_TILES: usize = 4_000_000;
    let remap = |id: i16| -> std::io::Result<i16> {
        if id < 0 {
            return Err(Error::new(ErrorKind::InvalidData, "negative block ID"));
        }
        block_remap.map_or(Ok(id), |mapping| {
            let mapped = mapping
                .get(id as usize)
                .copied()
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "unmapped block ID"))?;
            if mapped < 0 {
                return Err(Error::new(
                    ErrorKind::Unsupported,
                    format!("map uses unavailable legacy block ID {id}"),
                ));
            }
            Ok(mapped)
        })
    };
    let width = cursor.read_u16::<BigEndian>()?;
    let height = cursor.read_u16::<BigEndian>()?;
    let total = usize::from(width)
        .checked_mul(usize::from(height))
        .filter(|total| *total <= MAX_TILES)
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid map dimensions"))?;

    let mut floors = vec![0i16; total];
    let mut overlays = vec![0i16; total];
    let mut index = 0usize;
    while index < total {
        let floor = remap(cursor.read_i16::<BigEndian>()?)?;
        let overlay = remap(cursor.read_i16::<BigEndian>()?)?;
        let run = usize::from(cursor.read_u8()?) + 1;
        let end = index
            .checked_add(run)
            .filter(|end| *end <= total)
            .ok_or_else(|| Error::new(ErrorKind::InvalidData, "floor run exceeds map"))?;
        floors[index..end].fill(floor);
        overlays[index..end].fill(overlay);
        index = end;
    }

    let mut blocks = vec![0i16; total];
    let mut block_centers = vec![true; total];
    let mut tile_data = vec![0u8; total];
    let mut buildings = Vec::new();
    index = 0;
    while index < total {
        let block = remap(cursor.read_i16::<BigEndian>()?)?;
        let packed = cursor.read_u8()?;
        let has_entity = packed & 1 != 0;
        let has_old_data = packed & 2 != 0;
        let has_new_data = packed & 4 != 0;
        if has_new_data {
            // SaveVersion.writeMap writes Tile.data, floorData, overlayData and
            // extraData. Leg pathfinding consumes the first byte through
            // Tile.staticDarkness()/legSolid().
            tile_data[index] = cursor.read_u8()?;
            advance(cursor, 6, cursor.get_ref().len())?;
        }
        blocks[index] = block;
        if has_entity {
            let center = cursor.read_u8()? != 0;
            block_centers[index] = center;
            if center {
                let length = cursor.read_i32::<BigEndian>()?;
                let length = usize::try_from(length)
                    .ok()
                    .filter(|length| *length <= MAX_WORLD_STREAM_SIZE as usize)
                    .ok_or_else(|| Error::new(ErrorKind::InvalidData, "invalid building chunk"))?;
                if length < 7 {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "building chunk is too short",
                    ));
                }
                let chunk_start = cursor.position() as usize;
                let chunk_end = chunk_start
                    .checked_add(length)
                    .filter(|end| *end <= cursor.get_ref().len())
                    .ok_or_else(|| {
                        Error::new(ErrorKind::UnexpectedEof, "building chunk exceeds world")
                    })?;
                let _revision = cursor.read_u8()?;
                let raw_health = cursor.read_f32::<BigEndian>()?;
                let raw_rotation = cursor.read_u8()?;
                let rotation = raw_rotation & 0x7f;
                let team = cursor.read_u8()?;
                if !raw_health.is_finite() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "invalid building health",
                    ));
                }
                // BuildingComp.readBase: Math.min(read.f(), block.health).
                // Headless Java saves sometimes persist -1.0; treat non-positive
                // values as full health so official MSAVs load for parity tests.
                let max_health = crate::game::content::block_health(block);
                let health = raw_health.min(max_health);
                let health = if health <= 0.0 { max_health } else { health };

                // BuildingComp.readBase() is shared by every vanilla block.  The
                // old reader used to jump straight to chunk_end here, which made
                // map factories/power nodes appear empty.  Decode the versioned
                // base and modules exactly as 158.1 does; block-specific bytes
                // remain in extra_data for the subclass decoder.
                let mut inventory = Vec::new();
                let mut power_links = Vec::new();
                let mut power_status = 0.0;
                let mut liquids = Vec::new();
                let mut enabled = true;
                if raw_rotation & 0x80 != 0 {
                    let version = cursor.read_u8()?;
                    if version >= 1 {
                        enabled = cursor.read_u8()? == 1;
                    }
                    // Versions 0/1 did not persist the module bitmask; it was
                    // derived from the block's runtime module set.  The network
                    // reader intentionally has no content registry at this
                    // stage, so do not guess that every optional module exists:
                    // interpreting arbitrary legacy subclass bytes as modules
                    // would overrun the bounded building chunk.  We retain the
                    // legacy module bytes in `extra_data` for later block-aware
                    // decoding and consume only the legacy consume flag below.
                    let module_bits = if version >= 2 { cursor.read_u8()? } else { 0 };
                    if module_bits & 1 != 0 {
                        let count = read_module_count(cursor, false)?;
                        for _ in 0..count {
                            let item = cursor.read_i16::<BigEndian>()?;
                            let amount = cursor.read_i32::<BigEndian>()?;
                            if item >= 0 && amount > 0 {
                                inventory.push((item, amount));
                            }
                        }
                    }
                    if module_bits & 2 != 0 {
                        let count = read_module_count(cursor, false)?;
                        for _ in 0..count {
                            power_links.push(cursor.read_i32::<BigEndian>()?);
                        }
                        power_status = cursor.read_f32::<BigEndian>()?;
                        if !power_status.is_finite() {
                            power_status = 0.0;
                        }
                    }
                    if module_bits & 4 != 0 {
                        let count = read_module_count(cursor, false)?;
                        for _ in 0..count {
                            let liquid = cursor.read_i16::<BigEndian>()?;
                            let amount = cursor.read_f32::<BigEndian>()?;
                            if liquid >= 0 && amount.is_finite() && amount > 0.0 {
                                liquids.push((liquid, amount));
                            }
                        }
                    }
                    if module_bits & 16 != 0 {
                        advance(cursor, 8, chunk_end)?;
                    }
                    if module_bits & 32 != 0 {
                        advance(cursor, 4, chunk_end)?;
                    }
                    if version <= 2 {
                        advance(cursor, 1, chunk_end)?;
                    }
                    if version >= 3 {
                        advance(cursor, 2, chunk_end)?;
                    }
                    if version == 4 {
                        advance(cursor, 8, chunk_end)?;
                    }
                }
                let extra_start = cursor.position() as usize;
                let x = (index % usize::from(width)) as i32;
                let y = (index / usize::from(width)) as i32;
                buildings.push(NetworkBuilding {
                    position: (x << 16) | y,
                    block,
                    health,
                    rotation,
                    team,
                    inventory,
                    power_links,
                    power_status,
                    liquids,
                    enabled,
                    extra_data: cursor.get_ref()[extra_start..chunk_end].to_vec(),
                });
                cursor.set_position(chunk_end as u64);
            }
        } else if has_old_data {
            advance(cursor, 1, cursor.get_ref().len())?;
        } else if !has_new_data {
            let run = usize::from(cursor.read_u8()?);
            let end = index
                .checked_add(run + 1)
                .filter(|end| *end <= total)
                .ok_or_else(|| Error::new(ErrorKind::InvalidData, "block run exceeds map"))?;
            blocks[index..end].fill(block);
            index += run;
        }
        index += 1;
    }
    Ok(NetworkMap {
        width,
        height,
        blocks,
        block_centers,
        floors,
        overlays,
        tile_data,
        buildings,
    })
}

fn locate_player(world: &[u8]) -> std::io::Result<PlayerOffsets> {
    let mut cursor = Cursor::new(world);
    let patch_version = cursor.read_i32::<BigEndian>()?;
    if patch_version != 2 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("unsupported data patch version {patch_version}"),
        ));
    }
    let assets = cursor.read_i32::<BigEndian>()?;
    if assets != 0 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "templates with external data assets are not supported",
        ));
    }

    skip_utf(&mut cursor)?; // rules
    skip_utf(&mut cursor)?; // map locales
    let tags = cursor.read_i16::<BigEndian>()?;
    if !(0..=1024).contains(&tags) {
        return Err(Error::new(ErrorKind::InvalidData, "invalid map tag count"));
    }
    for _ in 0..tags {
        skip_utf(&mut cursor)?;
        skip_utf(&mut cursor)?;
    }

    let wave = cursor.position() as usize;
    advance(&mut cursor, 4, world.len())?;
    let wave_time = cursor.position() as usize;
    advance(&mut cursor, 4, world.len())?;
    let tick = cursor.position() as usize;
    // tick and two RNG seeds
    advance(&mut cursor, 8 + 8 + 8, world.len())?;
    let id = cursor.position() as usize;
    advance(&mut cursor, 4, world.len())?;
    let player_start = cursor.position() as usize;
    let revision = cursor.read_u16::<BigEndian>()?;
    if revision != PLAYER_REVISION {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("unsupported embedded player revision {revision}"),
        ));
    }

    // admin, boosting
    advance(&mut cursor, 2, world.len())?;
    let color = cursor.position() as usize;
    // color, command, mouse X/Y
    advance(&mut cursor, 4 + 1 + 4 + 4, world.len())?;
    let exists = cursor.read_u8()?;
    if exists != 1 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "embedded player has no name",
        ));
    }
    let name_length = cursor.position() as usize;
    let length = cursor.read_u16::<BigEndian>()? as usize;
    let name = cursor.position() as usize;
    let name_end = name
        .checked_add(length)
        .filter(|end| *end <= world.len())
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated player name"))?;
    let x = name_end
        .checked_add(2 + 4 + 1 + 1 + 1 + 5)
        .filter(|offset| offset + 8 <= world.len())
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated player position"))?;
    let y = x + 4;

    debug_assert_eq!(player_start + 4, color);
    Ok(PlayerOffsets {
        wave,
        wave_time,
        tick,
        id,
        color,
        name_length,
        name,
        name_end,
        x,
        y,
    })
}

fn skip_utf(cursor: &mut Cursor<&[u8]>) -> std::io::Result<()> {
    let length = cursor.read_u16::<BigEndian>()? as usize;
    advance(cursor, length, cursor.get_ref().len())
}

fn read_utf(cursor: &mut Cursor<&[u8]>) -> std::io::Result<String> {
    // NetworkIO/SaveVersion strings are Java modified UTF-8 (writeUTF), not
    // plain UTF-8; byte counts differ for emoji/NUL.
    crate::network::codec::read_modified_utf8_public(cursor)
}

fn write_utf(output: &mut Vec<u8>, value: &str) -> std::io::Result<()> {
    use crate::network::codec::encode_modified_utf8;
    let bytes = encode_modified_utf8(value);
    let length = u16::try_from(bytes.len())
        .map_err(|_| Error::new(ErrorKind::InvalidData, "UTF string is too long"))?;
    output.write_u16::<BigEndian>(length)?;
    output.write_all(&bytes)
}

fn read_string_map(cursor: &mut Cursor<&[u8]>) -> std::io::Result<Vec<(String, String)>> {
    let count = cursor.read_i16::<BigEndian>()?;
    if !(0..=1024).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid string map size",
        ));
    }
    (0..count)
        .map(|_| Ok((read_utf(cursor)?, read_utf(cursor)?)))
        .collect()
}

fn skip_string_map(cursor: &mut Cursor<&[u8]>) -> std::io::Result<()> {
    let count = cursor.read_i16::<BigEndian>()?;
    if !(0..=1024).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid string map size",
        ));
    }
    for _ in 0..count {
        skip_utf(cursor)?;
        skip_utf(cursor)?;
    }
    Ok(())
}

fn advance(cursor: &mut Cursor<&[u8]>, amount: usize, total: usize) -> std::io::Result<()> {
    let next = (cursor.position() as usize)
        .checked_add(amount)
        .filter(|next| *next <= total)
        .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated world stream"))?;
    cursor.set_position(next as u64);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::typeio::{TeamBlockPlan, TeamBlocks, TeamPlans};
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    fn decompress(data: &[u8]) -> Vec<u8> {
        let mut decoder = ZlibDecoder::new(data);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).unwrap();
        out
    }

    fn read_utf_at(data: &[u8], offset: &mut usize) -> String {
        let length = u16::from_be_bytes(data[*offset..*offset + 2].try_into().unwrap()) as usize;
        let start = *offset;
        *offset += 2 + length;
        // The decoder expects the full [u16 len][payload] framing.
        crate::network::codec::read_modified_utf8_public(&mut std::io::Cursor::new(
            &data[start..*offset],
        ))
        .unwrap()
    }

    fn utf_bytes(value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let bytes = crate::network::codec::encode_modified_utf8(value);
        out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        out.extend_from_slice(&bytes);
        out
    }

    /// Extracts the content-header and map regions from the embedded network
    /// template (they use the same codecs as the MSAV regions).
    fn template_regions() -> (Vec<u8>, Vec<u8>) {
        let raw = decompress(embedded_template());
        let mut offset = 8; // data-patch header
        read_utf_at(&raw, &mut offset); // rules
        read_utf_at(&raw, &mut offset); // locales
        let tags = i16::from_be_bytes(raw[offset..offset + 2].try_into().unwrap());
        offset += 2;
        for _ in 0..tags {
            read_utf_at(&raw, &mut offset);
            read_utf_at(&raw, &mut offset);
        }
        offset += 4 + 4 + 8 + 8 + 8 + 4; // wave, wavetime, tick, seeds, player id
        let revision = u16::from_be_bytes(raw[offset..offset + 2].try_into().unwrap());
        assert_eq!(revision, 2);
        offset += 2;
        offset += 1 + 1 + 4 + 1 + 4 + 4 + 1; // admin, boosting, color, command, mouse, name exists
        let name_len = u16::from_be_bytes(raw[offset..offset + 2].try_into().unwrap()) as usize;
        offset += 2 + name_len;
        offset += 2 + 4 + 1 + 1 + 1 + 1 + 4 + 8; // selected block, rotation, flags, unit ref, x/y
        let content_start = offset;
        let mapped = raw[offset];
        offset += 1;
        for _ in 0..mapped {
            offset += 1; // content type
            let total = i16::from_be_bytes(raw[offset..offset + 2].try_into().unwrap());
            offset += 2;
            for _ in 0..total {
                read_utf_at(&raw, &mut offset);
            }
        }
        let content_end = offset;
        let map_start = offset;
        let map_end = {
            let mut cursor = Cursor::new(raw.as_slice());
            cursor.set_position(map_start as u64);
            let _ = read_network_map(&mut cursor).unwrap();
            cursor.position() as usize
        };
        (
            raw[content_start..content_end].to_vec(),
            raw[map_start..map_end].to_vec(),
        )
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

    /// Builds an MSAV whose map/content regions come from the embedded
    /// template, with the given team blocks, markers and custom chunk
    /// payloads. `version` selects the official region layout (see
    /// SaveVersion.read and versions/Save11.java).
    fn fixture_msav(
        version: i32,
        team_blocks: &[u8],
        markers: &[u8],
        custom: &[u8],
        map_name: &str,
    ) -> Vec<u8> {
        let (content, map) = template_regions();
        let meta = string_map(&[
            ("mapname", map_name),
            ("description", "fixture"),
            ("author", "rust-tests"),
            ("width", "300"),
            ("height", "300"),
            ("build", "158"),
            ("wave", "1"),
            ("rules", "{\"waves\":true}"),
            ("locales", "{}"),
        ]);
        let entities = {
            let mut out = Vec::new();
            out.extend_from_slice(&0i16.to_be_bytes()); // entity mapping count
            out.extend_from_slice(team_blocks);
            out
        };
        // v11: meta, content, patches, map, entities, markers, custom.
        // v12+: meta, patches, content, map, entities, markers, custom.
        let patches_v11 = [0u8];
        let patches_v12 = [0u8; 12];
        let patches_v13 = [0, 0, 0, 2, 0, 0, 0, 0];
        let patches: &[u8] = match version {
            12 => &patches_v12,
            v if v >= 13 => &patches_v13,
            _ => &patches_v11,
        };
        let regions: Vec<&[u8]> = if version >= 12 {
            vec![&meta, patches, &content, &map, &entities, markers, custom]
        } else {
            vec![&meta, &content, patches, &map, &entities, markers, custom]
        };
        let mut msav = b"MSAV".to_vec();
        msav.extend_from_slice(&version.to_be_bytes());
        for region in regions {
            msav.extend_from_slice(&(region.len() as i32).to_be_bytes());
            msav.extend_from_slice(region);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&msav).unwrap();
        encoder.finish().unwrap()
    }

    fn empty_team_blocks() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1i32.to_be_bytes()); // one team
        out.extend_from_slice(&1i32.to_be_bytes()); // sharded
        out.extend_from_slice(&0i32.to_be_bytes()); // no plans
        out
    }

    /// A v4 (Save4) MSAV: meta, content, map, entities — no markers and no
    /// custom region, and the entities region starts directly with the team
    /// table (Save4.readEntities skips `readEntityMapping`). A zero world
    /// entity count trails the table, mirroring real v4 editor saves.
    fn fixture_msav_v4(team_blocks: &[u8], map: &[u8], map_name: &str) -> Vec<u8> {
        let (content, _) = template_regions();
        let meta = string_map(&[
            ("mapname", map_name),
            ("description", "fixture"),
            ("author", "rust-tests"),
            ("width", "256"),
            ("height", "256"),
            ("build", "126"),
            ("wave", "1"),
            ("rules", "{\"waves\":true}"),
            ("locales", "{}"),
        ]);
        let mut entities = team_blocks.to_vec();
        entities.extend_from_slice(&0i32.to_be_bytes()); // world entities: none
        let regions: [&[u8]; 4] = [&meta, &content, map, &entities];
        let mut msav = b"MSAV".to_vec();
        msav.extend_from_slice(&4i32.to_be_bytes());
        for region in regions {
            msav.extend_from_slice(&(region.len() as i32).to_be_bytes());
            msav.extend_from_slice(region);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&msav).unwrap();
        encoder.finish().unwrap()
    }

    /// Short-chunk (pre-v10) map region: a 4x4 field with one versioned
    /// building (entity version 0, no persisted modules) at the origin.
    fn short_format_map_with_building() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&4u16.to_be_bytes()); // width
        out.extend_from_slice(&4u16.to_be_bytes()); // height
        out.extend_from_slice(&1i16.to_be_bytes()); // floor
        out.extend_from_slice(&0i16.to_be_bytes()); // overlay
        out.push(15); // run covers all 16 tiles
                      // Tile 0: a building chunk in the pre-v10 short format (u16 length).
                      // Chunk payload: revision 0, health, rot with the version flag,
                      // team, entity version 0 (no module bitmask, one legacy bool).
        out.extend_from_slice(&62i16.to_be_bytes()); // block
        out.push(1); // packed: has entity
        out.push(1); // center
        let mut chunk = Vec::new();
        chunk.push(0); // chunk revision
        chunk.extend_from_slice(&500.0f32.to_be_bytes());
        chunk.push(0x80); // rotation + version flag
        chunk.push(1); // team
        chunk.push(0); // entity version 0
        chunk.push(1); // legacy consume bool (version <= 2)
        out.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        out.extend_from_slice(&chunk);
        // Tiles 1..16: air, no entity, one run covering the rest.
        out.extend_from_slice(&0i16.to_be_bytes());
        out.push(0);
        out.push(14);
        out
    }

    #[test]
    fn msav_conversion_reads_v4_entities_without_mapping() {
        let plan = TeamBlockPlan {
            x: 10,
            y: 20,
            rotation: 1,
            block: 257,
            config: vec![1, 0, 0, 0, 42],
        };
        let team_blocks = TeamBlocks {
            teams: vec![TeamPlans {
                team: 1,
                plans: vec![plan],
            }],
        }
        .encode()
        .unwrap();
        let msav = fixture_msav_v4(&team_blocks, &short_format_map_with_building(), "old-map");
        let converted = replace_map_from_msav(embedded_template(), &msav).unwrap();

        // No markers/custom regions exist in v4: the network stream must
        // carry the synthesized empty forms.
        assert_eq!(inspect_markers(&converted).unwrap(), b"{}");
        assert_eq!(
            inspect_custom_chunks(&converted).unwrap(),
            0i32.to_be_bytes()
        );
        // Team plans parse without an entity-ID mapping prefix.
        let plans = inspect_team_plans(&converted).unwrap();
        assert_eq!(plans.teams.len(), 1);
        assert_eq!(plans.teams[0].team, 1);
        assert_eq!(plans.teams[0].plans.len(), 1);
        assert_eq!(plans.teams[0].plans[0].block, 257);
        assert_eq!(plans.teams[0].plans[0].config, vec![1, 0, 0, 0, 42]);
        let section = msav_world_entity_section(&msav).unwrap();
        assert_eq!(section.save_version, 4);
        assert!(section.mapping.is_empty());
        assert_eq!(section.bytes, 0i32.to_be_bytes());
        // The short-chunk building survives the u16 -> i32 length upgrade and
        // decodes its version-0 base (modules absent, legacy bool consumed).
        let map = inspect_map(&converted).unwrap();
        assert_eq!(map.buildings.len(), 1);
        let building = &map.buildings[0];
        assert_eq!(building.position, 0);
        assert_eq!(building.block, 62);
        assert_eq!(building.team, 1);
        assert_eq!(building.rotation, 0);
        // BuildingComp.readBase clamps to block.health (carbon-vent = 40).
        assert!((building.health - crate::game::content::block_health(62)).abs() < 0.001);
        assert!(building.enabled);
        assert!(building.inventory.is_empty());
        assert!(building.power_links.is_empty());
        assert!(
            building.extra_data.is_empty(),
            "v0 base must consume its bool"
        );
    }

    #[test]
    fn official_fortress_v4_map_loads_with_short_chunks() {
        // Differential fixture: fortress.msav is a Save4 editor save shipped
        // with the Java sources (256x256, short chunks, no entity mapping).
        let path = std::path::Path::new("../core/assets/maps/default/fortress.msav");
        if !path.exists() {
            return;
        }
        let msav = std::fs::read(path).unwrap();
        let converted = replace_map_from_msav(embedded_template(), &msav).unwrap();
        let map = inspect_map(&converted).unwrap();
        assert_eq!(map.width, 256);
        assert_eq!(map.height, 256);
        assert_eq!(map.buildings.len(), 13);
        // Core-shard at (116,73); v4 content table id 181 remaps to 339.
        assert!(map.buildings.iter().any(|building| {
            building.block == 339 && building.position == (116i32 << 16) | 73 && building.team == 1
        }));
        // Scrap walls (230/231) and mechanical drills (325).
        assert!(map.buildings.iter().any(|building| building.block == 230));
        assert!(map.buildings.iter().any(|building| building.block == 231));
        assert!(map.buildings.iter().any(|building| building.block == 325));
        assert!(map.buildings.iter().any(|building| building.block == 349));
        // The entities region has one team and no plans.
        let plans = inspect_team_plans(&converted).unwrap();
        assert_eq!(plans.teams.len(), 1);
        assert_eq!(plans.teams[0].team, 1);
        assert!(plans.teams[0].plans.is_empty());
        assert_eq!(inspect_markers(&converted).unwrap(), b"{}");
        assert_eq!(
            inspect_custom_chunks(&converted).unwrap(),
            0i32.to_be_bytes()
        );
    }

    #[test]
    fn msav_conversion_carries_markers_custom_chunks_and_team_plans() {
        // A non-empty team build plan with a validated TypeIO Integer config.
        let plan = TeamBlockPlan {
            x: 10,
            y: 20,
            rotation: 1,
            block: 257,
            config: vec![1, 0, 0, 0, 42],
        };
        let team_blocks = TeamBlocks {
            teams: vec![TeamPlans {
                team: 1,
                plans: vec![plan],
            }],
        }
        .encode()
        .unwrap();
        // A marker object with one string field, encoded with the exact
        // UBJsonWriter grammar (name without `S`, `S` value with `i` length).
        let markers = b"{i\x03tagSi\x05hello}".to_vec();
        // One custom chunk named "test-chunk" with 3 payload bytes.
        let mut custom = Vec::new();
        custom.extend_from_slice(&1i32.to_be_bytes());
        custom.extend_from_slice(&utf_bytes("test-chunk"));
        custom.extend_from_slice(&3i32.to_be_bytes());
        custom.extend_from_slice(&[9, 8, 7]);

        let msav = fixture_msav(11, &team_blocks, &markers, &custom, "fixture-hills");
        let converted = replace_map_from_msav(embedded_template(), &msav).unwrap();

        assert_eq!(inspect_markers(&converted).unwrap(), markers);
        assert_eq!(inspect_custom_chunks(&converted).unwrap(), custom);
        let team_count = inspect_team_count(&converted).unwrap();
        assert_eq!(team_count, 1);
        // The team plan must survive: one team, one plan, exact plan bytes.
        let world = decompress(&converted);
        let (_, map_end) = locate_network_map_range(&world).unwrap();
        let (decoded, consumed) = TeamBlocks::decode(&world[map_end..]).unwrap();
        assert_eq!(consumed, team_blocks.len());
        assert_eq!(decoded.teams.len(), 1);
        assert_eq!(decoded.teams[0].plans.len(), 1);
        assert_eq!(decoded.teams[0].plans[0].block, 257);
        assert_eq!(decoded.teams[0].plans[0].config, vec![1, 0, 0, 0, 42]);
    }

    /// A minimal map region in the pre-v10 short-chunk format (u16 chunk
    /// lengths, no per-tile `Tile.data` block): a 4x4 empty field.
    fn short_format_map() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&4u16.to_be_bytes()); // width
        out.extend_from_slice(&4u16.to_be_bytes()); // height
        out.extend_from_slice(&1i16.to_be_bytes()); // floor
        out.extend_from_slice(&0i16.to_be_bytes()); // overlay
        out.push(15); // run covers all 16 tiles
        out.extend_from_slice(&0i16.to_be_bytes()); // block air
        out.push(0); // packed: no entity, no data
        out.push(15); // run covers all 16 tiles
        out
    }

    #[test]
    fn msav_conversion_uses_empty_markers_for_pre_marker_save_versions() {
        // Version 7 has no markers region: content(1), map(2), entities(3),
        // custom(4). The conversion must synthesize the empty `{}` object.
        let (content, _) = template_regions();
        let map = short_format_map();
        let meta = string_map(&[
            ("mapname", "old-map"),
            ("width", "300"),
            ("height", "300"),
            ("build", "126"),
            ("wave", "1"),
            ("rules", "{\"waves\":true}"),
            ("locales", "{}"),
        ]);
        let entities = {
            let mut out = Vec::new();
            out.extend_from_slice(&0i16.to_be_bytes());
            out.extend_from_slice(&empty_team_blocks());
            out
        };
        let custom = 0i32.to_be_bytes().to_vec();
        let regions: [&[u8]; 5] = [&meta, &content, &map, &entities, &custom];
        let mut msav = b"MSAV".to_vec();
        msav.extend_from_slice(&7i32.to_be_bytes());
        for region in regions {
            msav.extend_from_slice(&(region.len() as i32).to_be_bytes());
            msav.extend_from_slice(region);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&msav).unwrap();
        let msav = encoder.finish().unwrap();

        let converted = replace_map_from_msav(embedded_template(), &msav).unwrap();
        assert_eq!(inspect_markers(&converted).unwrap(), b"{}");
        assert_eq!(inspect_custom_chunks(&converted).unwrap(), custom);
    }

    #[test]
    fn ubjson_scanner_accepts_arc_writer_grammar() {
        for value in [
            b"{}".as_slice(),
            b"{i\x03tagSi\x05hello}".as_slice(),
            b"[i\x01i\x02i\x03]".as_slice(),
            b"[$i#l\x00\x00\x00\x03\x01\x02\x03]".as_slice(),
            b"Z".as_slice(),
            b"l\x00\x00\x00\x05".as_slice(),
            b"{i\x01a{i\x01bL\x00\x00\x00\x00\x00\x00\x00\x07}}".as_slice(),
        ] {
            match ubjson_len(value, 0) {
                Ok(n) => assert_eq!(n, value.len(), "len mismatch for {:?}", value),
                Err(e) => panic!("ERR for {:?}: {}", value, e),
            }
        }
        assert!(ubjson_len(b"{", 0).is_err());
        assert!(ubjson_len(b"i", 0).is_err());
        assert_eq!(ubjson_len(b"{}x", 0).unwrap(), 2); // trailing bytes are not part of the value
    }

    #[test]
    fn inject_rules_pvp_sets_pvp_flag_in_world_stream() {
        // The official server serializes state.rules.pvp into the world
        // stream (NetworkIO.writeWorld -> JsonIO.write(state.rules)); the
        // client reads it to enable PvP UI/teams. inject_rules_pvp rewrites
        // the embedded rules JSON of the bundled template.
        let template = include_bytes!("../dummy_world.dat");
        let injected = inject_rules_pvp(template).unwrap();
        // Decompress and read the rules string (first MUTF-8 after header).
        let world = decompress_limited(&injected, "pvp-injected").unwrap();
        assert_eq!(&world[..8], &[0, 0, 0, 2, 0, 0, 0, 0]);
        let mut cursor = Cursor::new(&world[8..]);
        let rules = crate::network::codec::read_modified_utf8_public(&mut cursor).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&crate::network::units::arc_json_to_strict(&rules)).unwrap();
        assert_eq!(
            value.get("pvp"),
            Some(&serde_json::Value::Bool(true)),
            "rules JSON must carry pvp:true after injection"
        );
        // The default template rules (without injection) has no pvp:true.
        let plain = decompress_limited(template, "plain").unwrap();
        let mut pcursor = Cursor::new(&plain[8..]);
        let plain_rules = crate::network::codec::read_modified_utf8_public(&mut pcursor).unwrap();
        let pvalue: serde_json::Value =
            serde_json::from_str(&crate::network::units::arc_json_to_strict(&plain_rules)).unwrap();
        assert_ne!(
            pvalue.get("pvp"),
            Some(&serde_json::Value::Bool(true)),
            "default template must not already be pvp"
        );
    }

    #[test]
    fn inject_rules_sandbox_applies_the_complete_mode_preset() {
        // A Survival map commonly stores infiniteResources=false and
        // waveTimer=true. Hosting it with `--mode sandbox` must override those
        // values in the Rules JSON consumed by the client.
        let template = include_bytes!("../dummy_world.dat");
        let injected = inject_rules_mode(template, false, true).unwrap();
        let world = decompress_limited(&injected, "sandbox-injected").unwrap();
        let mut cursor = Cursor::new(&world[8..]);
        let rules = crate::network::codec::read_modified_utf8_public(&mut cursor).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&crate::network::units::arc_json_to_strict(&rules)).unwrap();
        for key in ["infiniteResources", "allowEditRules", "waves"] {
            assert_eq!(
                value.get(key),
                Some(&serde_json::Value::Bool(true)),
                "sandbox Rules must carry {key}:true"
            );
        }
        assert_eq!(
            value.get("waveTimer"),
            Some(&serde_json::Value::Bool(false)),
            "sandbox Rules must carry waveTimer:false"
        );
        assert_ne!(
            value.get("pvp"),
            Some(&serde_json::Value::Bool(true)),
            "sandbox must not accidentally enable PvP"
        );
    }

    #[test]
    fn official_fortress_building_chunks_retain_base_modules() {
        // Official campaign map (GPLv3, Anuken) under third_party/mindustry-maps.
        // Parsing it exercises real SaveVersion entity chunks rather than a
        // hand-made stream, and verifies that module bytes are consumed before
        // subclass data is retained.
        let path = std::path::Path::new("third_party/mindustry-maps/frontier.msav");
        if !path.exists() {
            return;
        }
        let msav = std::fs::read(path).unwrap();
        let converted = replace_map_from_msav(embedded_template(), &msav).unwrap();
        let map = inspect_map(&converted).unwrap();
        assert!(!map.buildings.is_empty());
        eprintln!(
            "frontier buildings={} items={} links={} liquids={} extra={}",
            map.buildings.len(),
            map.buildings
                .iter()
                .filter(|b| !b.inventory.is_empty())
                .count(),
            map.buildings
                .iter()
                .filter(|b| !b.power_links.is_empty())
                .count(),
            map.buildings
                .iter()
                .filter(|b| !b.liquids.is_empty())
                .count(),
            map.buildings
                .iter()
                .filter(|b| !b.extra_data.is_empty())
                .count()
        );
        assert!(
            map.buildings.iter().any(|building| {
                !building.inventory.is_empty()
                    || !building.power_links.is_empty()
                    || !building.liquids.is_empty()
                    || !building.extra_data.is_empty()
            }),
            "vanilla building modules/subclass bytes were discarded"
        );
    }

    #[test]
    fn personalize_with_pvp_produces_valid_stream() {
        let template = include_bytes!("../dummy_world.dat");
        let stream = personalize_desktop_158_with_state_pvp(
            template,
            42,
            "pvptest",
            -5_915_137, // 0xffa665ff as i32
            (320.0, 800.0),
            1,
            3600.0,
            0.0,
            true,
        )
        .unwrap();
        // The final stream is the network layout (data-patch header already
        // stripped); it must be non-empty and comparable in size to the
        // non-pvp stream. The rules injection is verified at the template
        // level (inject_rules_pvp_sets_pvp_flag_in_world_stream); the client
        // decode of the final stream is exercised by smoke_join.
        let _ = std::fs::create_dir_all("target/protocol-158-fixtures");
        let _ = std::fs::write("target/protocol-158-fixtures/pvp-stream.bin", &stream);
        let world = decompress_limited(&stream, "pvp stream").unwrap();
        assert!(!world.is_empty(), "stream must not be empty");
        let plain = personalize_desktop_158_with_state(
            include_bytes!("../dummy_world.dat"),
            42,
            "pvptest",
            -5_915_137,
            (320.0, 800.0),
            1,
            3600.0,
            0.0,
        )
        .unwrap();
        let plain_world = decompress_limited(&plain, "plain stream").unwrap();
        assert!(
            (world.len() as i64 - plain_world.len() as i64).abs() < 4096,
            "pvp rules injection should not change stream size much ({} vs {})",
            world.len(),
            plain_world.len()
        );
    }

    #[test]
    fn save_versions_around_the_159_7_writer_extract() {
        // Save1..Save13 are accepted. Official current writer is Save13.
        let markers = b"{i\x03tagSi\x05hello}".to_vec();
        let mut custom = Vec::new();
        custom.extend_from_slice(&1i32.to_be_bytes());
        custom.extend_from_slice(&utf_bytes("test-chunk"));
        custom.extend_from_slice(&3i32.to_be_bytes());
        custom.extend_from_slice(&[9, 8, 7]);
        for version in [11, 12, 13] {
            let msav = fixture_msav(
                version,
                &empty_team_blocks(),
                &markers,
                &custom,
                "v-extract",
            );
            assert!(
                extract_msav_regions(&msav).is_ok(),
                "v{version} extracts (documented extension)"
            );
        }
    }

    #[test]
    fn current_personalize_keeps_159_7_data_patch_prefix() {
        let stream = personalize_current_with_state_mode(
            include_bytes!("../dummy_world.dat"),
            42,
            "join",
            -5_915_137,
            (320.0, 800.0),
            1,
            3600.0,
            0.0,
            false,
            false,
        )
        .unwrap();
        let world = decompress_limited(&stream, "current stream").unwrap();
        assert_eq!(&world[..8], &[0, 0, 0, 2, 0, 0, 0, 0]);
    }
}
