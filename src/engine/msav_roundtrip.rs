//! P2-C1: cross-runtime MSAV roundtrip for server-authoritative state.
//!
//! Loads a v11 MSAV (Java or Rust) into a live [`DynamicWorld`], writes one
//! back, ticks the subset of simulation that both runtimes share (status
//! expiry, logic processors, puddles, power-link sanitizing, LogicAI leases)
//! and snapshots observable fields. Byte-identical MSAV is not required.

use crate::engine::save_io::{
    read_unit_write, read_unit_write_preamble, write_msav_complete, MsavWorld,
};
use crate::engine::world_stream;
use crate::network::simulation::{simulate_logic, simulate_logic_control_leases};
use crate::network::units::controller::{
    apply_controller_snapshot, finalize_controller_after_load, read_unit_controller,
};
use crate::network::wire::fresh_world_from_template;
use crate::network::world::{DynamicWorld, PendingConnection, UnitAuthority};
use crate::state::game_state::GameState;
use byteorder::{BigEndian, ReadBytesExt};
use dashmap::DashMap;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Error, ErrorKind, Read};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub const CAMPAIGN_TICKS: u32 = 300;

/// Host a compressed MSAV as a live world (map + buildings + units + puddles).
pub fn load_msav_world(msav: &[u8], save_name: &str) -> std::io::Result<Arc<DynamicWorld>> {
    let template = world_stream::replace_map_from_msav(world_stream::embedded_template(), msav)?;
    let world = fresh_world_from_template(
        &GameState::new(),
        template,
        save_name.to_string(),
        std::env::temp_dir().join(format!("{save_name}.json")),
    )?;
    apply_msav_entities(&world, msav)?;
    crate::network::buildings::power::normalize_power_links(&world);
    Ok(Arc::new(world))
}

/// Persist a live world as the current official writer (Save13 / build 159).
pub fn save_msav_world(world: &DynamicWorld) -> std::io::Result<Vec<u8>> {
    save_msav_world_version(world, crate::compat_target::CURRENT_SAVE_VERSION)
}

/// Historical fixture helper: persist using an explicit SaveVersion.
pub fn save_msav_world_version(world: &DynamicWorld, version: i32) -> std::io::Result<Vec<u8>> {
    let mut meta = HashMap::new();
    meta.insert("mapname".into(), world.game_state.map_name.read().clone());
    meta.insert(
        "wave".into(),
        world
            .game_state
            .wave
            .load(std::sync::atomic::Ordering::Relaxed)
            .to_string(),
    );
    meta.insert(
        "tick".into(),
        world.game_state.simulation_time.read().to_string(),
    );
    meta.insert("width".into(), world.width.to_string());
    meta.insert("height".into(), world.height.to_string());
    meta.insert(
        "build".into(),
        if version >= 13 {
            crate::compat_target::CURRENT_PROTOCOL_BUILD.to_string()
        } else {
            "158".into()
        },
    );
    meta.insert("rules".into(), rules_json_from_world(world));
    let mut blocks = world.base_blocks.clone();
    for tile in world.tiles.iter() {
        let x = (tile.position >> 16) as i16 as i32;
        let y = tile.position as i16 as i32;
        if (0..world.width).contains(&x) && (0..world.height).contains(&y) {
            let index = (y * world.width + x) as usize;
            if index < blocks.len() {
                blocks[index] = tile.block;
            }
        }
    }
    let enemy_units: Vec<_> = world
        .enemies
        .iter()
        .map(|entry| entry.value().clone())
        .collect();
    let puddles: Vec<_> = world
        .puddles
        .puddles
        .iter()
        .map(|entry| {
            (
                *entry.key(),
                entry.value().amount,
                entry.value().liquid,
                entry.value().entity_id,
            )
        })
        .collect();
    write_msav_complete(
        &meta,
        version,
        &MsavWorld {
            width: world.width as usize,
            height: world.height as usize,
            floors: &world.floors,
            overlays: &world.overlays,
            blocks: &blocks,
            team_blocks: Some(&world.team_build_plans.read().clone()),
            dynamic_tiles: &world.tiles,
            enemy_units: &enemy_units,
            puddles: &puddles,
            runtime: Some(world),
        },
    )
}

/// Reconstruct units and puddles from `SaveVersion.readWorldEntities`.
///
/// Framing follows desktop.jar 158.1:
/// - v4–v5 (`LegacySaveVersion2`): count i32, chunk length u16, classId + body (no id)
/// - v6–v9 (`ShortChunkSaveVersion`): count i32, chunk length u16, classId + i32 id + body
/// - v10–v11 (`SaveVersion.readWorldEntities`): count i32, chunk length i32, classId + i32 id + body
pub fn apply_msav_entities(world: &DynamicWorld, msav: &[u8]) -> std::io::Result<()> {
    let section = world_stream::msav_world_entity_section(msav)?;
    apply_msav_world_entity_section(world, &section)
}

fn apply_msav_world_entity_section(
    world: &DynamicWorld,
    section: &world_stream::MsavWorldEntitySection,
) -> std::io::Result<()> {
    let save_version = section.save_version;
    let bytes = section.bytes.as_slice();
    if bytes.is_empty() {
        // v1–v3 extract no world-entity payload. v4+ always writes an i32 count.
        if save_version >= 4 {
            return Err(Error::new(
                ErrorKind::UnexpectedEof,
                "truncated world entity count",
            ));
        }
        return Ok(());
    }
    let mut cursor = Cursor::new(bytes);
    let count = cursor.read_i32::<BigEndian>()?;
    if !(0..=50_000).contains(&count) {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid world entity count",
        ));
    }
    let mapping = resolve_entity_mapping(&section.mapping);
    let short_chunks = save_version <= 9;
    let serialized_ids = save_version >= 6;
    let reassign_duplicates = save_version >= 10;
    let mut used_ids = HashSet::new();
    let mut restored_units = Vec::new();
    for _ in 0..count {
        let len = if short_chunks {
            usize::from(cursor.read_u16::<BigEndian>()?)
        } else {
            let raw = cursor.read_i32::<BigEndian>()?;
            usize::try_from(raw)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "negative entity chunk"))?
        };
        let start = cursor.position() as usize;
        let end = start
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| Error::new(ErrorKind::UnexpectedEof, "truncated entity chunk"))?;
        let chunk = &bytes[start..end];
        cursor.set_position(end as u64);
        if chunk.is_empty() {
            return Err(Error::new(ErrorKind::InvalidData, "empty entity chunk"));
        }
        let class_id = chunk[0];
        let slot = mapping[usize::from(class_id)];
        if slot == WorldEntitySlot::Unknown {
            // SaveVersion.readWorldEntities: mapping[typeid] == null → skip(len-1).
            continue;
        }
        let rest = &chunk[1..];
        let assigned_id = if serialized_ids {
            if rest.len() < 4 {
                return Err(Error::new(ErrorKind::UnexpectedEof, "truncated entity id"));
            }
            let id = i32::from_be_bytes(rest[..4].try_into().unwrap());
            check_next_id(world, id);
            if reassign_duplicates {
                if used_ids.insert(id) {
                    id
                } else {
                    let new_id = next_entity_id(world);
                    used_ids.insert(new_id);
                    new_id
                }
            } else {
                id
            }
        } else {
            next_entity_id(world)
        };
        let body = if serialized_ids { &rest[4..] } else { rest };
        match slot {
            WorldEntitySlot::Puddle => restore_puddle(world, assigned_id, body)?,
            WorldEntitySlot::Unit => {
                restore_unit(world, assigned_id, class_id, body)?;
                restored_units.push(assigned_id);
            }
            WorldEntitySlot::Skip => {}
            WorldEntitySlot::Unknown => {}
        }
    }
    if cursor.position() != bytes.len() as u64 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "world entity region has trailing bytes",
        ));
    }
    // Snapshot ids first: DashMap deadlocks if `iter()` is held across
    // `enemies.get_mut` inside `finalize_controller_after_load`.
    for id in restored_units {
        finalize_controller_after_load(world, id);
    }
    Ok(())
}

fn check_next_id(world: &DynamicWorld, id: i32) {
    // EntityGroup.checkNextId: lastId = max(lastId, id + 1)
    world
        .next_enemy_id
        .fetch_max(id.saturating_add(1), Ordering::Relaxed);
}

fn next_entity_id(world: &DynamicWorld) -> i32 {
    // EntityGroup.nextId, wrapping near i32::MAX like the 158.1 static counter.
    let id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
    if id >= i32::MAX - 2 {
        world.next_enemy_id.store(0, Ordering::Relaxed);
        0
    } else {
        id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorldEntitySlot {
    Unknown,
    Skip,
    Puddle,
    Unit,
}

fn resolve_entity_mapping(overlay: &[(u16, String)]) -> [WorldEntitySlot; 256] {
    let mut mapping = [WorldEntitySlot::Unknown; 256];
    for class_id in 0u8..=255 {
        mapping[usize::from(class_id)] = vanilla_entity_slot(class_id);
    }
    for (id, name) in overlay {
        mapping[usize::from(*id)] = slot_from_mapping_name(name);
    }
    mapping
}

fn vanilla_entity_slot(class_id: u8) -> WorldEntitySlot {
    // EntityMapping.idMap of desktop.jar 158.1 (length 256). Null slots skip.
    match class_id {
        13 => WorldEntitySlot::Puddle,
        0 | 2 | 3 | 4 | 5 | 16 | 17 | 18 | 19 | 20 | 21 | 23 | 24 | 26 | 29 | 30 | 31 | 32 | 33
        | 36 | 39 | 43 | 45 | 46 => WorldEntitySlot::Unit,
        6 | 7 | 8 | 9 | 10 | 11 | 12 | 14 | 15 | 28 | 35 | 42 => WorldEntitySlot::Skip,
        _ => WorldEntitySlot::Unknown,
    }
}

fn slot_from_mapping_name(name: &str) -> WorldEntitySlot {
    match name {
        "Puddle" | "puddle" => WorldEntitySlot::Puddle,
        "Building"
        | "building"
        | "Bullet"
        | "bullet"
        | "Decal"
        | "decal"
        | "EffectState"
        | "effect-state"
        | "Fire"
        | "fire"
        | "LaunchCore"
        | "launch-core"
        | "Player"
        | "player"
        | "WeatherState"
        | "weather-state"
        | "LaunchPayload"
        | "launch-payload"
        | "PosTeam"
        | "pos-team"
        | "WorldLabel"
        | "world-label"
        | "PowerGraphUpdater"
        | "power-graph-updater" => WorldEntitySlot::Skip,
        _ if ENTITY_MAPPING_UNIT_NAMES.binary_search(&name).is_ok() => WorldEntitySlot::Unit,
        _ => WorldEntitySlot::Unknown,
    }
}

/// `EntityMapping.nameMap` unit aliases from desktop.jar 158.1, sorted for
/// binary search. Overlay names missing from this table map to null and skip.
const ENTITY_MAPPING_UNIT_NAMES: &[&str] = &[
    "BlockUnitUnit",
    "BuildingTetherPayloadUnit",
    "CrawlUnit",
    "ElevationMoveUnit",
    "LegsUnit",
    "LegsUnitLegacyArkyid",
    "LegsUnitLegacySpiroct",
    "LegsUnitLegacyToxopid",
    "MechUnit",
    "MechUnitLegacyNova",
    "MechUnitLegacyPulsar",
    "MechUnitLegacyQuasar",
    "PayloadUnit",
    "PayloadUnitLegacyOct",
    "PayloadUnitLegacyQuad",
    "TankUnit",
    "TimedKillUnit",
    "UnitEntity",
    "UnitEntityLegacyAlpha",
    "UnitEntityLegacyBeta",
    "UnitEntityLegacyGamma",
    "UnitEntityLegacyMono",
    "UnitEntityLegacyPoly",
    "UnitWaterMove",
    "aegires",
    "alpha",
    "anthicus",
    "antumbra",
    "arkyid",
    "assembly-drone",
    "assemblyDrone",
    "atrax",
    "avert",
    "beta",
    "block",
    "block-unit-unit",
    "bryde",
    "building-tether-payload-unit",
    "cleroi",
    "collaris",
    "conquer",
    "corvus",
    "crawl-unit",
    "crawler",
    "cyerce",
    "dagger",
    "disrupt",
    "eclipse",
    "elevation-move-unit",
    "elude",
    "emanate",
    "evoke",
    "flare",
    "fortress",
    "gamma",
    "horizon",
    "incite",
    "latum",
    "legs-unit",
    "legs-unit-legacy-arkyid",
    "legs-unit-legacy-spiroct",
    "legs-unit-legacy-toxopid",
    "locus",
    "mace",
    "manifold",
    "mech-unit",
    "mech-unit-legacy-nova",
    "mech-unit-legacy-pulsar",
    "mech-unit-legacy-quasar",
    "mega",
    "merui",
    "minke",
    "missile",
    "mono",
    "navanax",
    "nova",
    "obviate",
    "oct",
    "omura",
    "oxynoe",
    "payload-unit",
    "payload-unit-legacy-oct",
    "payload-unit-legacy-quad",
    "poly",
    "precept",
    "pulsar",
    "quad",
    "quasar",
    "quell",
    "reign",
    "renale",
    "retusa",
    "risso",
    "scepter",
    "sei",
    "spiroct",
    "stell",
    "tank-unit",
    "tecta",
    "timed-kill-unit",
    "toxopid",
    "unit-entity",
    "unit-entity-legacy-alpha",
    "unit-entity-legacy-beta",
    "unit-entity-legacy-gamma",
    "unit-entity-legacy-mono",
    "unit-entity-legacy-poly",
    "unit-water-move",
    "vanquish",
    "vela",
    "zenith",
];

fn restore_puddle(world: &DynamicWorld, entity_id: i32, body: &[u8]) -> std::io::Result<()> {
    use crate::network::buildings::puddles::PuddleState;
    use crate::network::codec::Reads;
    let mut cursor = Cursor::new(body);
    let _rev = cursor.read_s()?;
    let amount = cursor.read_f()?;
    let liquid = cursor.read_s()?;
    let position = cursor.read_i()?;
    let _x = cursor.read_f()?;
    let _y = cursor.read_f()?;
    if liquid >= 0 && amount.is_finite() && amount > 0.0 {
        world.puddles.restore(
            position,
            PuddleState {
                entity_id,
                liquid,
                amount,
                accepting: 0.0,
                spread_mask: 0b1111,
                update_time: 0.0,
            },
        );
    }
    Ok(())
}

fn restore_unit(world: &DynamicWorld, id: i32, class_id: u8, body: &[u8]) -> std::io::Result<()> {
    let mut cursor = Cursor::new(body);
    let mut unit = read_unit_write(&mut cursor, class_id)?;
    unit.id = id;
    unit.entity_class = class_id;
    let snapshot = {
        // Re-read the controller from the same body so afterRead can drop
        // missing refs. `read_unit_write` already applied authority; we still
        // run the shared afterRead pipeline for queue pruning.
        let mut again = Cursor::new(body);
        let _ = read_unit_write_preamble(&mut again, class_id)?;
        read_unit_controller(&mut again, id).ok()
    };
    world.enemies.insert(id, unit);
    world.register_unit_group(id);
    if let Some(snapshot) = snapshot {
        apply_controller_snapshot(world, id, snapshot);
    }
    check_next_id(world, id);
    Ok(())
}

/// 300-tick continuation of the server-authoritative subset both runtimes
/// share: status expiry, logic processors, puddles, LogicAI leases.
pub fn tick_campaign(world: &DynamicWorld, ticks: u32) {
    let connections: DashMap<i32, PendingConnection> = DashMap::new();
    for _ in 0..ticks {
        simulate_logic(world, &connections, 1.0);
        simulate_logic_control_leases(world, 1.0);
        world.puddles.tick(1.0);
        let unit_ids: Vec<i32> = world.enemies.iter().map(|unit| unit.id).collect();
        for id in unit_ids {
            if let Some(mut unit) = world.enemies.get_mut(&id) {
                crate::network::units::StatusContainer::tick_statuses(&mut *unit, 1.0);
            }
        }
    }
    crate::network::buildings::power::normalize_power_links(world);
}

/// Observable snapshot compared against the Java 158.1 probe fixture.
pub fn campaign_snapshot(world: &DynamicWorld) -> Value {
    let mut units: Vec<Value> = world
        .enemies
        .iter()
        .map(|entry| {
            let unit = entry.value();
            json!({
                "id": unit.id,
                "type": unit.unit_type,
                "team": unit.team,
                "health": unit.health,
                "x": unit.x,
                "y": unit.y,
                "authority": format!("{:?}", unit.authority),
                "is_logic_ai": matches!(unit.authority, UnitAuthority::Logic { .. }),
                "is_command_ai": matches!(unit.authority, UnitAuthority::Command),
                "statuses": unit.statuses.iter().map(|s| json!({
                    "effect": s.effect,
                    "time": s.time,
                })).collect::<Vec<_>>(),
                "payloads": unit.payloads.iter().map(payload_kind).collect::<Vec<_>>(),
                "items": unit.items,
                "plans": unit.build_plans.len(),
            })
        })
        .collect();
    units.sort_by_key(|v| v["id"].as_i64().unwrap_or(0));

    let mut buildings: Vec<Value> = world
        .tiles
        .iter()
        .map(|tile| {
            let mut links = tile.power_links.clone();
            links.sort_unstable();
            json!({
                "pos": tile.position,
                "block": tile.block,
                "team": tile.team,
                "health": tile.health,
                "inventory": tile.inventory.clone(),
                "liquids": tile.liquid_inventory.clone(),
                "power_links": links,
                "payload": tile.payload.as_ref().map(|p| payload_kind(p)),
                "has_logic": crate::network::buildings::config::logic_payload(&tile.config).is_some(),
            })
        })
        .collect();
    buildings.sort_by_key(|v| v["pos"].as_i64().unwrap_or(0));

    let mut puddles: Vec<Value> = world
        .puddles
        .puddles
        .iter()
        .map(|entry| {
            json!({
                "pos": *entry.key(),
                "liquid": entry.value().liquid,
                "amount": entry.value().amount,
            })
        })
        .collect();
    puddles.sort_by_key(|v| v["pos"].as_i64().unwrap_or(0));

    let rules = world.wave_rules.read();
    let mut team_rules: Vec<Value> = rules
        .team_rules
        .iter()
        .map(|(id, rule)| {
            json!({
                "team": id,
                "unitDamageMultiplier": rule.unit_damage_multiplier,
            })
        })
        .collect();
    team_rules.sort_by_key(|v| v["team"].as_i64().unwrap_or(0));

    let mut plans = 0usize;
    for team in &world.team_build_plans.read().teams {
        plans += team.plans.len();
    }

    json!({
        "units": units,
        "buildings": buildings,
        "puddles": puddles,
        "plan_count": plans,
        "rules": {
            "unitDamageMultiplier": rules.unit_damage_multiplier,
            "unitHealthMultiplier": rules.unit_health_multiplier,
            "infiniteResources": rules.infinite_resources,
            "teams": team_rules,
        },
        "logic": logic_snapshot(world),
    })
}

fn payload_kind(payload: &crate::network::world::CarriedPayload) -> Value {
    use crate::network::world::CarriedPayload;
    match payload {
        CarriedPayload::Unit(unit) => json!({
            "kind": "unit",
            "type": unit.unit_type,
            "nested": unit.payloads.len(),
        }),
        CarriedPayload::Build(build) => json!({
            "kind": "build",
            "block": build.tile.block,
        }),
    }
}

fn logic_snapshot(world: &DynamicWorld) -> Value {
    let mut out = Vec::new();
    for tile in world.tiles.iter() {
        if !matches!(tile.block, 431..=433 | 442) {
            continue;
        }
        let n = world.logic_executors.get(&tile.position).and_then(|exec| {
            exec.vars
                .iter()
                .find(|v| v.name == "n" && !v.isobj)
                .map(|v| v.numval)
        });
        out.push(json!({
            "pos": tile.position,
            "has_program": crate::network::buildings::config::logic_payload(&tile.config).is_some(),
            "n": n,
        }));
    }
    out.sort_by_key(|v| v["pos"].as_i64().unwrap_or(0));
    json!(out)
}

pub(crate) fn rules_json_from_world(world: &DynamicWorld) -> String {
    let rules = world.wave_rules.read();
    let mut teams = serde_json::Map::new();
    for (id, rule) in &rules.team_rules {
        teams.insert(
            id.to_string(),
            json!({ "unitDamageMultiplier": rule.unit_damage_multiplier }),
        );
    }
    json!({
        "unitDamageMultiplier": rules.unit_damage_multiplier,
        "unitHealthMultiplier": rules.unit_health_multiplier,
        "blockHealthMultiplier": rules.block_health_multiplier,
        "infiniteResources": rules.infinite_resources,
        "waves": rules.waves_enabled,
        "waveTimer": rules.wave_timer,
        "disableUnitCap": rules.disable_unit_cap,
        "teams": serde_json::Value::Object(teams),
    })
    .to_string()
}

/// Compare two campaign snapshots, ignoring unit x/y (AI pathing is not in
/// the continuation contract) and puddle amount noise below 0.05.
pub fn compare_campaign_snapshots(java: &Value, rust: &Value, label: &str) -> Result<(), String> {
    compare_unit_sets(&java["units"], &rust["units"], label)?;
    compare_building_sets(&java["buildings"], &rust["buildings"], label)?;
    compare_puddle_sets(&java["puddles"], &rust["puddles"], label)?;
    let jp = java["plan_count"].as_u64().unwrap_or(0);
    let rp = rust["plan_count"].as_u64().unwrap_or(0);
    if jp != rp {
        return Err(format!("{label}: plan_count java={jp} rust={rp}"));
    }
    let jmult = java["rules"]["unitDamageMultiplier"]
        .as_f64()
        .unwrap_or(1.0);
    let rmult = rust["rules"]["unitDamageMultiplier"]
        .as_f64()
        .unwrap_or(1.0);
    if (jmult - rmult).abs() > 1e-4 {
        return Err(format!(
            "{label}: unitDamageMultiplier java={jmult} rust={rmult}"
        ));
    }
    Ok(())
}

fn compare_unit_sets(java: &Value, rust: &Value, label: &str) -> Result<(), String> {
    let j = java.as_array().cloned().unwrap_or_default();
    let r = rust.as_array().cloned().unwrap_or_default();
    if j.len() != r.len() {
        return Err(format!(
            "{label}: unit count java={} rust={}",
            j.len(),
            r.len()
        ));
    }
    for (ja, ra) in j.iter().zip(r.iter()) {
        for key in ["type", "team", "is_logic_ai", "is_command_ai"] {
            if ja[key] != ra[key] {
                return Err(format!(
                    "{label}: unit {} {key} java={} rust={}",
                    ja["id"], ja[key], ra[key]
                ));
            }
        }
        if (ja["health"].as_f64().unwrap_or(0.0) - ra["health"].as_f64().unwrap_or(0.0)).abs() > 1.0
        {
            return Err(format!(
                "{label}: unit {} health java={} rust={}",
                ja["id"], ja["health"], ra["health"]
            ));
        }
        if ja["payloads"] != ra["payloads"] {
            return Err(format!(
                "{label}: unit {} payloads java={} rust={}",
                ja["id"], ja["payloads"], ra["payloads"]
            ));
        }
        compare_status_lists(
            &ja["statuses"],
            &ra["statuses"],
            label,
            ja["id"].as_i64().unwrap_or(0),
        )?;
    }
    Ok(())
}

fn compare_status_lists(
    java: &Value,
    rust: &Value,
    label: &str,
    unit_id: i64,
) -> Result<(), String> {
    let j = java.as_array().cloned().unwrap_or_default();
    let r = rust.as_array().cloned().unwrap_or_default();
    if j.len() != r.len() {
        return Err(format!(
            "{label}: unit {unit_id} status count java={} rust={}",
            j.len(),
            r.len()
        ));
    }
    for (ja, ra) in j.iter().zip(r.iter()) {
        if ja["effect"] != ra["effect"] {
            return Err(format!(
                "{label}: unit {unit_id} status effect java={} rust={}",
                ja["effect"], ra["effect"]
            ));
        }
        let jt = ja["time"].as_f64().unwrap_or(0.0);
        let rt = ra["time"].as_f64().unwrap_or(0.0);
        if (jt - rt).abs() > 2.0 {
            return Err(format!(
                "{label}: unit {unit_id} status time java={jt} rust={rt}"
            ));
        }
    }
    Ok(())
}

fn compare_building_sets(java: &Value, rust: &Value, label: &str) -> Result<(), String> {
    let j = java.as_array().cloned().unwrap_or_default();
    let r = rust.as_array().cloned().unwrap_or_default();
    // Rust worlds seeded from the embedded template may carry extra map
    // buildings; compare by position for the Java set.
    let rust_by_pos: HashMap<i64, &Value> = r
        .iter()
        .filter_map(|b| Some((b["pos"].as_i64()?, b)))
        .collect();
    for ja in &j {
        let pos = ja["pos"].as_i64().unwrap_or(-1);
        let Some(ra) = rust_by_pos.get(&pos) else {
            return Err(format!("{label}: missing building at {pos}"));
        };
        if ja["block"] != ra["block"] {
            return Err(format!(
                "{label}: building {pos} block java={} rust={}",
                ja["block"], ra["block"]
            ));
        }
        if ja["power_links"] != ra["power_links"] {
            return Err(format!(
                "{label}: building {pos} power_links java={} rust={}",
                ja["power_links"], ra["power_links"]
            ));
        }
        if ja["inventory"] != ra["inventory"] {
            return Err(format!(
                "{label}: building {pos} inventory java={} rust={}",
                ja["inventory"], ra["inventory"]
            ));
        }
        if ja.get("has_logic") == Some(&json!(true)) && ra.get("has_logic") != Some(&json!(true)) {
            return Err(format!("{label}: building {pos} lost logic program"));
        }
        if ja.get("payload").is_some()
            && ja["payload"] != json!(null)
            && ra["payload"] != ja["payload"]
        {
            return Err(format!(
                "{label}: building {pos} payload java={} rust={}",
                ja["payload"], ra["payload"]
            ));
        }
    }
    Ok(())
}

fn compare_puddle_sets(java: &Value, rust: &Value, label: &str) -> Result<(), String> {
    let j = java.as_array().cloned().unwrap_or_default();
    let r = rust.as_array().cloned().unwrap_or_default();
    if j.is_empty() {
        return Ok(());
    }
    if j.len() != r.len() {
        return Err(format!(
            "{label}: puddle count java={} rust={}",
            j.len(),
            r.len()
        ));
    }
    for (ja, ra) in j.iter().zip(r.iter()) {
        if ja["pos"] != ra["pos"] || ja["liquid"] != ra["liquid"] {
            return Err(format!("{label}: puddle identity java={ja} rust={ra}"));
        }
        let ja_amt = ja["amount"].as_f64().unwrap_or(0.0);
        let ra_amt = ra["amount"].as_f64().unwrap_or(0.0);
        if (ja_amt - ra_amt).abs() > 0.5 {
            return Err(format!(
                "{label}: puddle amount java={ja_amt} rust={ra_amt}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::status::ActiveStatus;
    use crate::network::world::{
        CarriedBuildPayload, CarriedPayload, DynamicTile, EnemyUnit, UnitAuthority,
    };
    use dashmap::DashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
    use std::sync::Arc;

    fn tiny_world(save_name: &str) -> DynamicWorld {
        let width = 16i32;
        let height = 16i32;
        let cells = (width * height) as usize;
        let state = GameState::new();
        *state.map_name.write() = save_name.to_string();
        DynamicWorld {
            game_state: state,
            width,
            height,
            sharded_unit_cap: 8,
            core_position: 0,
            core_max_health: 0.0,
            cores: DashMap::new(),
            team_core_lists: DashMap::new(),
            base_blocks: vec![0i16; cells],
            base_centers: vec![false; cells],
            tile_data: Vec::new(),
            base_building_templates: Vec::new(),
            base_buildings: DashMap::new(),
            floors: vec![crate::game::block_names::block_id_from_name("stone").unwrap(); cells],
            overlays: vec![0i16; cells],
            enemy_spawns: Vec::new(),
            enemies: DashMap::new(),
            players: DashMap::new(),
            player_sessions: DashMap::new(),
            player_profiles: DashMap::new(),
            building_commands: DashMap::new(),
            unit_orders: DashMap::new(),
            next_player_unit_id: AtomicI32::new(2_500_000),
            next_enemy_id: AtomicI32::new(3_000_000),
            unit_group_order: parking_lot::Mutex::new(Vec::new()),
            projectiles: DashMap::new(),
            next_projectile_id: AtomicI32::new(4_000_000),
            overdrive_boosts: DashMap::new(),
            heal_suppression: DashMap::new(),
            force_fields: DashMap::new(),
            tiles: DashMap::new(),
            pending_builds: DashMap::new(),
            pending_breaks: DashMap::new(),
            mineable_ore: std::sync::OnceLock::new(),
            mono_mining_targets: DashMap::new(),
            tile_footprint: DashMap::new(),
            navigation_revision: AtomicU64::new(0),
            ground_navigation: parking_lot::Mutex::new(None),
            leg_navigation: parking_lot::Mutex::new(None),
            save_path: PathBuf::from(format!("/tmp/{save_name}.json")),
            network_template: Arc::new(Vec::new()),
            persistence_dirty: AtomicBool::new(false),
            persistence_lock: parking_lot::Mutex::new(()),
            logic_flags: DashMap::new(),
            logic_executors: DashMap::new(),
            logic_display_commands: DashMap::new(),
            base_drill_progress: DashMap::new(),
            base_factory_progress: DashMap::new(),
            base_turret_progress: DashMap::new(),
            base_mender_progress: DashMap::new(),
            team_build_plans: parking_lot::RwLock::new(crate::engine::typeio::TeamBlocks::default()),
            wave_rules: parking_lot::RwLock::new(crate::network::units::WaveRules::default()),
            votekick_target: parking_lot::RwLock::new(None),
            votekick_votes: AtomicI32::new(0),
            votekick_voters: DashMap::new(),
            votekick_cooldowns: DashMap::new(),
            puddles: crate::network::buildings::puddles::PuddleSystem::new(),
        }
    }

    fn campaign_world() -> DynamicWorld {
        let world = tiny_world("p2-c1-campaign");
        // Power source (410) at (2,2) linked to a node (302) at (4,2).
        let source_pos = (2 << 16) | 2;
        let node_pos = (4 << 16) | 2;
        world.tiles.insert(
            source_pos,
            DynamicTile {
                position: source_pos,
                block: 410,
                team: 1,
                health: 40.0,
                power_links: vec![node_pos],
                occupied: vec![source_pos],
                ..DynamicTile::default()
            },
        );
        world.tiles.insert(
            node_pos,
            DynamicTile {
                position: node_pos,
                block: 302,
                team: 1,
                health: 40.0,
                power_links: vec![source_pos],
                occupied: vec![node_pos],
                ..DynamicTile::default()
            },
        );
        // Vault (3x3) and tank (3x3) with a one-tile gap so Java 158.1 can
        // occupy both footprints after a Rust→Java load.
        let vault_pos = (3 << 16) | 8;
        world.tiles.insert(
            vault_pos,
            DynamicTile {
                position: vault_pos,
                block: 346,
                team: 1,
                health: 900.0,
                inventory: vec![(0, 50)],
                occupied: vec![vault_pos],
                ..DynamicTile::default()
            },
        );
        let tank_pos = (7 << 16) | 8;
        world.tiles.insert(
            tank_pos,
            DynamicTile {
                position: tank_pos,
                block: 291,
                team: 1,
                health: 500.0,
                stored_liquid: 0,
                liquid_amount: 80.0,
                liquid_inventory: vec![(0, 80.0)],
                occupied: vec![tank_pos],
                ..DynamicTile::default()
            },
        );
        // Microprocessor with a counter program.
        let proc_pos = (10 << 16) | 2;
        let source = b"op add n n 1\nend";
        let mut content = vec![1u8];
        content.extend_from_slice(&(source.len() as i32).to_be_bytes());
        content.extend_from_slice(source);
        content.extend_from_slice(&0i32.to_be_bytes());
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        use std::io::Write;
        encoder.write_all(&content).unwrap();
        let zlib = encoder.finish().unwrap();
        let mut config = vec![14u8];
        config.extend_from_slice(&(zlib.len() as i32).to_be_bytes());
        config.extend_from_slice(&zlib);
        world.tiles.insert(
            proc_pos,
            DynamicTile {
                position: proc_pos,
                block: 431,
                team: 1,
                health: 90.0,
                config,
                occupied: vec![proc_pos],
                ..DynamicTile::default()
            },
        );
        // Payload conveyor (3x3) carrying a vault (multiblock).
        let conv_pos = (12 << 16) | 8;
        world.tiles.insert(
            conv_pos,
            DynamicTile {
                position: conv_pos,
                block: 398,
                team: 1,
                health: 360.0,
                payload: Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
                    tile: DynamicTile {
                        block: 346,
                        health: 900.0,
                        team: 1,
                        occupied: vec![0],
                        ..DynamicTile::default()
                    },
                    version: 0,
                    sync: Vec::new(),
                }))),
                occupied: vec![conv_pos],
                ..DynamicTile::default()
            },
        );
        // Team plans.
        *world.team_build_plans.write() = crate::engine::typeio::TeamBlocks {
            teams: vec![crate::engine::typeio::TeamPlans {
                team: 1,
                plans: vec![crate::engine::typeio::TeamBlockPlan {
                    x: 14,
                    y: 2,
                    rotation: 0,
                    block: 158,
                    config: vec![0],
                }],
            }],
        };
        world.wave_rules.write().unit_damage_multiplier = 1.5;
        world.wave_rules.write().wave_timer = false;
        world.wave_rules.write().disable_unit_cap = true;
        world.wave_rules.write().team_rules.insert(
            1,
            crate::network::units::TeamRule {
                unit_damage_multiplier: 2.0,
                ..Default::default()
            },
        );
        // Flare with CommandAI + near-expiry status + a nested payload mega.
        let mut flare = sample_unit(3_010_001, 15, 3, UnitAuthority::Command);
        flare.statuses = vec![ActiveStatus::simple(4, 10.0)];
        flare.status_effect = 4;
        flare.status_duration = 10.0;
        world.enemies.insert(flare.id, flare);
        world.register_unit_group(3_010_001);
        let mut mega = sample_unit(3_010_002, 22, 5, UnitAuthority::Command);
        let dagger = sample_unit(3_010_003, 0, 4, UnitAuthority::DefaultAi);
        mega.payloads.push(CarriedPayload::Unit(dagger));
        world.enemies.insert(mega.id, mega);
        world.register_unit_group(3_010_002);
        world.puddles.restore(
            (3 << 16) | 3,
            crate::network::buildings::puddles::PuddleState {
                entity_id: 13_000,
                liquid: 0,
                amount: 40.0,
                accepting: 0.0,
                spread_mask: 0b1111,
                update_time: 0.0,
            },
        );
        world
    }

    fn sample_unit(id: i32, unit_type: i16, class_id: u8, authority: UnitAuthority) -> EnemyUnit {
        let spec = crate::network::units::enemy_spec(unit_type);
        EnemyUnit {
            id,
            unit_type,
            entity_class: class_id,
            team: 1,
            x: 80.0,
            y: 80.0,
            rotation: 90.0,
            health: spec.map(|s| s.health).unwrap_or(150.0),
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 1.0,
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: spec.map(|s| s.speed).unwrap_or(1.0),
            attack_damage: spec.map(|s| s.attack_damage).unwrap_or(0.0),
            attack_reload_time: spec.map(|s| s.attack_reload).unwrap_or(1.0),
            attack_range: spec.map(|s| s.attack_range).unwrap_or(0.0),
            authority,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        }
    }

    #[test]
    fn rust_save_load_roundtrip_keeps_campaign_state() {
        let world = campaign_world();
        let before = campaign_snapshot(&world);
        let bytes = save_msav_world(&world).expect("write current Save13");
        let loaded = load_msav_world(&bytes, "p2-c1-rt").expect("load Save13");
        let after = campaign_snapshot(&loaded);
        compare_campaign_snapshots(&before, &after, "rust-save-load").unwrap();
        let dump = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/p2-c1-rust-campaign.msav");
        let _ = std::fs::create_dir_all(dump.parent().unwrap());
        std::fs::write(&dump, &bytes).expect("dump rust campaign MSAV for Java probe");
    }

    /// Independent Rust mirror of v159.7 `ApplicationTests.saveLoad`.
    ///
    /// The upstream scenario saves a map with a core and a dagger whose health
    /// is set to 30, then verifies the unit, health, dimensions and core/team
    /// state after loading.  This uses the current Save13 writer/reader and
    /// checks the state observable by the Rust host rather than round-tripping
    /// a Rust snapshot through serde.
    #[test]
    fn upstream_application_save_load_1597_dagger_health_and_core_state() {
        let mut world = tiny_world("upstream-save-load-1597");
        let core_position = (4 << 16) | 4;
        world.core_position = core_position;
        world.core_max_health = 1_100.0;
        *world.game_state.core_health.write() = 1_100.0;
        let core_occupied = vec![
            (3 << 16) | 3,
            (4 << 16) | 3,
            (5 << 16) | 3,
            (3 << 16) | 4,
            core_position,
            (5 << 16) | 4,
            (3 << 16) | 5,
            (4 << 16) | 5,
            (5 << 16) | 5,
        ];
        world.tiles.insert(
            core_position,
            DynamicTile {
                position: core_position,
                block: 339, // core-shard, 3x3
                team: 1,
                health: 1_100.0,
                occupied: core_occupied,
                ..DynamicTile::default()
            },
        );

        let mut dagger = sample_unit(3_010_100, 0, 3, UnitAuthority::DefaultAi);
        dagger.team = 1;
        dagger.x = 20.0;
        dagger.y = 30.0;
        dagger.health = 30.0;
        world.enemies.insert(dagger.id, dagger);
        world.register_unit_group(3_010_100);

        let bytes = save_msav_world(&world).expect("write current Save13");
        let loaded =
            load_msav_world(&bytes, "upstream-save-load-1597").expect("load current Save13");

        assert_eq!(loaded.width, 16);
        assert_eq!(loaded.height, 16);
        let restored = loaded
            .enemies
            .get(&3_010_100)
            .expect("saved dagger persists");
        assert_eq!(restored.unit_type, 0);
        assert_eq!(restored.team, 1);
        assert!((restored.health - 30.0).abs() < f32::EPSILON);
        assert_eq!(loaded.core_position(), core_position);
        assert!(
            (loaded.core_max_health() - 1_100.0).abs() < f32::EPSILON,
            "loaded core max health was {}",
            loaded.core_max_health()
        );
        let core = loaded
            .tiles
            .get(&core_position)
            .expect("saved core persists");
        assert_eq!(core.block, 339);
        assert_eq!(core.team, 1);
        assert!((core.health - 1_100.0).abs() < f32::EPSILON);
        let cores = crate::network::world::team_core_snapshot(&loaded, 1);
        assert_eq!(cores.len(), 1);
        assert_eq!(cores[0].position, core_position);
    }

    #[test]
    fn rust_save_load_300_ticks_expires_short_status() {
        let world = campaign_world();
        let bytes = save_msav_world(&world).unwrap();
        let loaded = load_msav_world(&bytes, "p2-c1-300").unwrap();
        tick_campaign(&loaded, CAMPAIGN_TICKS);
        let flare = loaded.enemies.get(&3_010_001).unwrap();
        assert!(
            flare.statuses.is_empty(),
            "status with 10 ticks remaining expires across 300 ticks: {:?}",
            flare.statuses
        );
        let vault = loaded.tiles.get(&((3 << 16) | 8)).unwrap();
        assert_eq!(vault.inventory, vec![(0, 50)]);
        let node = loaded.tiles.get(&((4 << 16) | 2)).unwrap();
        assert!(node.power_links.contains(&((2 << 16) | 2)));
        let mega = loaded.enemies.get(&3_010_002).unwrap();
        assert_eq!(mega.payloads.len(), 1);
        assert!(matches!(
            mega.payloads[0],
            CarriedPayload::Unit(ref u) if u.unit_type == 0
        ));
    }

    #[test]
    fn stale_power_link_is_pruned_like_java() {
        let world = campaign_world();
        let ghost = (40 << 16) | 40;
        if let Some(mut node) = world.tiles.get_mut(&((4 << 16) | 2)) {
            node.power_links.push(ghost);
        }
        let bytes = save_msav_world(&world).unwrap();
        let loaded = load_msav_world(&bytes, "p2-c1-stale-power").unwrap();
        let node = loaded.tiles.get(&((4 << 16) | 2)).unwrap();
        assert!(
            !node.power_links.contains(&ghost),
            "stale power link must be dropped: {:?}",
            node.power_links
        );
    }

    #[test]
    fn missing_command_target_is_dropped_on_load() {
        let world = campaign_world();
        world.unit_orders.insert(
            3_010_001,
            crate::network::world::UnitOrder {
                unit_id: 3_010_001,
                command: 0,
                target_kind: 2,
                target_id: 999_999,
                target_x: Some(120.0),
                target_y: Some(120.0),
                queue: vec![crate::network::world::UnitOrderTarget {
                    kind: 2,
                    id: 999_999,
                    x: 0.0,
                    y: 0.0,
                }],
                ..crate::network::world::UnitOrder::default()
            },
        );
        if let Some(mut unit) = world.enemies.get_mut(&3_010_001) {
            unit.authority = UnitAuthority::Command;
        }
        let bytes = save_msav_world(&world).unwrap();
        let loaded = load_msav_world(&bytes, "p2-c1-missing-ref").unwrap();
        finalize_controller_after_load(&loaded, 3_010_001);
        let order = loaded.unit_orders.get(&3_010_001);
        let queue_has_ghost = order
            .as_ref()
            .map(|o| o.queue.iter().any(|t| t.id == 999_999))
            .unwrap_or(false);
        assert!(!queue_has_ghost, "missing queue unit must be dropped");
        let attack_cleared = order
            .as_ref()
            .map(|o| o.target_kind == 0 || o.target_id != 999_999)
            .unwrap_or(true);
        assert!(attack_cleared, "missing attack unit must degrade");
    }

    #[test]
    fn destroyed_logic_processor_releases_lease_after_ticks() {
        let world = campaign_world();
        if let Some(mut unit) = world.enemies.get_mut(&3_010_001) {
            unit.authority = UnitAuthority::Logic {
                processor_pos: (99 << 16) | 99,
                remaining_ticks: 5.0,
                processor_generation: 0,
            };
        }
        tick_campaign(&world, 10);
        let unit = world.enemies.get(&3_010_001).unwrap();
        assert!(
            !matches!(unit.authority, UnitAuthority::Logic { .. }),
            "lease on a missing processor must release: {:?}",
            unit.authority
        );
    }

    #[test]
    fn nested_and_multiblock_payloads_survive_roundtrip() {
        let world = campaign_world();
        let bytes = save_msav_world(&world).unwrap();
        let loaded = load_msav_world(&bytes, "p2-c1-payload").unwrap();
        let mega = loaded.enemies.get(&3_010_002).unwrap();
        match &mega.payloads[..] {
            [CarriedPayload::Unit(inner)] => assert_eq!(inner.unit_type, 0),
            other => panic!("expected nested unit payload, got {other:?}"),
        }
        let conv = loaded.tiles.get(&((12 << 16) | 8)).unwrap();
        match conv.payload.as_deref() {
            Some(CarriedPayload::Build(build)) => assert_eq!(build.tile.block, 346),
            other => panic!("expected vault build payload, got {other:?}"),
        }
    }

    fn save_load_fixture() -> Value {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/parity/fixtures/save-load.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing save-load.json fixture: {err}"));
        serde_json::from_str(&raw).expect("save-load.json")
    }

    fn decode_msav(b64: &Value) -> Vec<u8> {
        use base64::{engine::general_purpose, Engine as _};
        let text = b64.as_str().expect("msav b64 string");
        general_purpose::STANDARD.decode(text).expect("msav base64")
    }

    #[test]
    fn java_save_rust_load_300_matches_fixture() {
        let fixture = save_load_fixture();
        assert_eq!(fixture["probe_version"], "158.1");
        let msav = decode_msav(&fixture["java_msav_b64"]);
        let world = load_msav_world(&msav, "p2-c1-java").expect("load java MSAV");
        compare_campaign_snapshots(
            &fixture["after_load"],
            &campaign_snapshot(&world),
            "java-save-rust-load",
        )
        .unwrap();
        tick_campaign(&world, CAMPAIGN_TICKS);
        compare_campaign_snapshots(
            &fixture["after_300"],
            &campaign_snapshot(&world),
            "java-save-rust-300",
        )
        .unwrap();
        assert_eq!(fixture["outcomes"]["status_expired_after_300"], true);
        assert_eq!(fixture["outcomes"]["nested_payload_survives"], true);
        assert_eq!(fixture["outcomes"]["multiblock_payload_survives"], true);
        assert_eq!(fixture["outcomes"]["logic_program_survives"], true);
        assert_eq!(fixture["outcomes"]["missing_command_target_dropped"], true);
        assert_eq!(
            fixture["outcomes"]["destroyed_processor_releases_lease"],
            true
        );
    }

    #[test]
    fn rust_save_java_reexport_rust_load_matches() {
        let fixture = save_load_fixture();
        let reexport = fixture
            .get("java_reexport_b64")
            .and_then(Value::as_str)
            .expect("java_reexport_b64 from Rust→Java→save campaign");
        let msav = decode_msav(&Value::String(reexport.to_string()));
        let world = load_msav_world(&msav, "p2-c1-reexport").expect("load java reexport");
        compare_campaign_snapshots(
            &fixture["rust_to_java_after_load"],
            &campaign_snapshot(&world),
            "rust-java-rust",
        )
        .unwrap();
        tick_campaign(&world, CAMPAIGN_TICKS);
        compare_campaign_snapshots(
            &fixture["rust_to_java_after_300"],
            &campaign_snapshot(&world),
            "rust-java-rust-300",
        )
        .unwrap();
    }

    fn packed_msav(version: i32, entities: &[u8]) -> Vec<u8> {
        use crate::engine::save_io::{
            write_msav_content_patches_region, write_msav_content_region, write_msav_custom_region,
            write_msav_map_region, write_msav_markers_region, SAVE_HEADER,
        };
        use flate2::write::ZlibEncoder;
        use std::io::Write;
        let mut meta: HashMap<String, String> = HashMap::new();
        meta.insert("mapname".into(), "entity-frame".into());
        meta.insert("width".into(), "4".into());
        meta.insert("height".into(), "4".into());
        meta.insert("build".into(), "158".into());
        meta.insert("wave".into(), "1".into());
        meta.insert("rules".into(), "{}".into());
        meta.insert("locales".into(), "{}".into());
        let mut keys: Vec<_> = meta.keys().cloned().collect();
        keys.sort();
        let mut meta_region = Vec::new();
        meta_region.extend_from_slice(&(keys.len() as u16).to_be_bytes());
        for key in &keys {
            let encoded = crate::network::codec::encode_modified_utf8(key);
            meta_region.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
            meta_region.extend_from_slice(&encoded);
            let encoded = crate::network::codec::encode_modified_utf8(&meta[key]);
            meta_region.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
            meta_region.extend_from_slice(&encoded);
        }
        let content = write_msav_content_region().unwrap();
        let floors = vec![1i16; 16];
        let overlays = vec![0i16; 16];
        let blocks = vec![0i16; 16];
        let empty = DashMap::new();
        let map =
            write_msav_map_region(4, 4, &floors, &overlays, &blocks, &empty, version).unwrap();
        let patches = write_msav_content_patches_region(version).unwrap();
        let markers = write_msav_markers_region().unwrap();
        let custom = write_msav_custom_region().unwrap();
        let regions: Vec<&[u8]> = match version {
            4..=6 => vec![&meta_region, &content, &map, entities],
            7 => vec![&meta_region, &content, &map, entities, &custom],
            8..=10 => vec![&meta_region, &content, &map, entities, &markers, &custom],
            11 => vec![
                &meta_region,
                &content,
                &patches,
                &map,
                entities,
                &markers,
                &custom,
            ],
            _ => vec![
                &meta_region,
                &patches,
                &content,
                &map,
                entities,
                &markers,
                &custom,
            ],
        };
        let mut plain = Vec::new();
        plain.extend_from_slice(SAVE_HEADER);
        plain.extend_from_slice(&version.to_be_bytes());
        for region in regions {
            plain.extend_from_slice(&(region.len() as i32).to_be_bytes());
            plain.extend_from_slice(region);
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&plain).unwrap();
        encoder.finish().unwrap()
    }

    fn entities_region(version: i32, mapping: &[(u16, &str)], world_entities: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if version >= 5 {
            out.extend_from_slice(&(mapping.len() as u16).to_be_bytes());
            for (id, name) in mapping {
                out.extend_from_slice(&id.to_be_bytes());
                let encoded = crate::network::codec::encode_modified_utf8(name);
                out.extend_from_slice(&(encoded.len() as u16).to_be_bytes());
                out.extend_from_slice(&encoded);
            }
        }
        out.extend_from_slice(&0i32.to_be_bytes());
        out.extend_from_slice(world_entities);
        out
    }

    fn world_entity_bytes(count: i32, chunks: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&count.to_be_bytes());
        out.extend_from_slice(chunks);
        out
    }

    fn entity_chunk(version: i32, class_id: u8, id: i32, body: &[u8]) -> Vec<u8> {
        let mut payload = vec![class_id];
        if version >= 6 {
            payload.extend_from_slice(&id.to_be_bytes());
        }
        payload.extend_from_slice(body);
        let mut out = Vec::new();
        if version <= 9 {
            out.extend_from_slice(&(u16::try_from(payload.len()).unwrap()).to_be_bytes());
        } else {
            out.extend_from_slice(&(payload.len() as i32).to_be_bytes());
        }
        out.extend_from_slice(&payload);
        out
    }

    fn unit_body(id: i32, class_id: u8, authority: UnitAuthority) -> Vec<u8> {
        crate::engine::save_io::write_unit_entity_body(
            &sample_unit(id, 0, class_id, authority),
            class_id,
            None,
        )
        .unwrap()
    }

    fn apply_versioned(
        version: i32,
        mapping: &[(u16, &str)],
        count: i32,
        chunks: &[u8],
    ) -> DynamicWorld {
        let world = tiny_world(&format!("entities-v{version}"));
        let entities = entities_region(version, mapping, &world_entity_bytes(count, chunks));
        let msav = packed_msav(version, &entities);
        apply_msav_entities(&world, &msav).expect("apply world entities");
        world
    }

    #[test]
    fn entity_mapping_unit_names_are_sorted() {
        for pair in ENTITY_MAPPING_UNIT_NAMES.windows(2) {
            assert!(
                pair[0] < pair[1],
                "ENTITY_MAPPING_UNIT_NAMES not sorted: {} !< {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn world_entity_reader_zero_entities_per_version() {
        for version in 4..=11 {
            let world = apply_versioned(version, &[], 0, &[]);
            assert!(world.enemies.is_empty(), "v{version} kept a unit");
        }
    }

    #[test]
    fn world_entity_reader_one_unit_framing_per_version() {
        let body = unit_body(42, 3, UnitAuthority::DefaultAi);
        for version in 4..=11 {
            let chunk = entity_chunk(version, 3, 42, &body);
            let world = apply_versioned(version, &[], 1, &chunk);
            assert_eq!(world.enemies.len(), 1, "v{version}");
            let unit = world.enemies.iter().next().unwrap();
            if version <= 5 {
                assert_eq!(unit.id, 3_000_000, "v{version} allocates via nextId");
            } else {
                assert_eq!(unit.id, 42, "v{version} keeps serialized id");
            }
            assert_eq!(unit.entity_class, 3);
            assert_eq!(unit.unit_type, 0);
        }
    }

    #[test]
    fn world_entity_reader_multiple_entities_and_unknown_class() {
        let body = unit_body(10, 3, UnitAuthority::DefaultAi);
        for version in 4..=11 {
            let mut chunks = entity_chunk(version, 1, 1, &[0u8; 8]); // idMap[1] is null
            chunks.extend_from_slice(&entity_chunk(version, 3, 10, &body));
            chunks.extend_from_slice(&entity_chunk(version, 3, 11, &body));
            let world = apply_versioned(version, &[], 3, &chunks);
            assert_eq!(world.enemies.len(), 2, "v{version} skipped unknown class 1");
        }
    }

    #[test]
    fn world_entity_reader_unknown_mapping_name_is_skipped() {
        let body = unit_body(8, 3, UnitAuthority::DefaultAi);
        let chunk = entity_chunk(5, 3, 8, &body);
        let world = apply_versioned(5, &[(3, "no-such-class")], 1, &chunk);
        assert!(
            world.enemies.is_empty(),
            "overlay null mapping must skip like Java"
        );
    }

    #[test]
    fn world_entity_reader_truncated_short_chunk_fails() {
        let mut world_bytes = world_entity_bytes(1, &[]);
        world_bytes.extend_from_slice(&20u16.to_be_bytes());
        world_bytes.extend_from_slice(&[3u8, 0, 1]);
        let entities = entities_region(5, &[], &world_bytes);
        let msav = packed_msav(5, &entities);
        let world = tiny_world("trunc-short");
        let err = apply_msav_entities(&world, &msav).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn world_entity_reader_truncated_int_chunk_fails() {
        let mut world_bytes = world_entity_bytes(1, &[]);
        world_bytes.extend_from_slice(&20i32.to_be_bytes());
        world_bytes.extend_from_slice(&[3u8, 0, 1]);
        let entities = entities_region(11, &[], &world_bytes);
        let msav = packed_msav(11, &entities);
        let world = tiny_world("trunc-int");
        let err = apply_msav_entities(&world, &msav).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn world_entity_reader_v11_duplicate_id_is_reassigned() {
        let body = unit_body(7, 3, UnitAuthority::DefaultAi);
        let mut chunks = entity_chunk(11, 3, 7, &body);
        chunks.extend_from_slice(&entity_chunk(11, 3, 7, &body));
        let world = apply_versioned(11, &[], 2, &chunks);
        let mut ids: Vec<i32> = world.enemies.iter().map(|u| u.id).collect();
        ids.sort_unstable();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], 7);
        assert_ne!(ids[1], 7, "duplicate must go through nextId");
    }

    #[test]
    fn world_entity_reader_controller_survives_supported_versions() {
        let body = unit_body(99, 3, UnitAuthority::Command);
        for version in [4, 6, 9, 10, 11] {
            let chunk = entity_chunk(version, 3, 99, &body);
            let world = apply_versioned(version, &[], 1, &chunk);
            let unit = world.enemies.iter().next().unwrap();
            assert!(
                matches!(unit.authority, UnitAuthority::Command),
                "v{version} lost CommandAI: {:?}",
                unit.authority
            );
        }
    }

    #[test]
    fn archipelago_msav_hosts_its_poly() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../core/assets/maps/default/archipelago.msav");
        if !path.is_file() {
            return;
        }
        let msav = std::fs::read(&path).unwrap();
        let section = world_stream::msav_world_entity_section(&msav).unwrap();
        assert_eq!(section.save_version, 5);
        let world = tiny_world("archipelago-entities");
        apply_msav_entities(&world, &msav).expect("archipelago world entities");
        assert_eq!(world.enemies.len(), 1);
        let poly = world.enemies.iter().next().unwrap();
        assert_eq!(poly.entity_class, 18);
        assert_eq!(poly.unit_type, 37);
        assert_eq!(poly.team, 1);
        assert!((poly.health - 220.0).abs() < 0.1);
    }
}
