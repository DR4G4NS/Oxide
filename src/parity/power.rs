//! Parity differential probes — power domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::buildings::power::apply_configuration;
use crate::network::world::DynamicWorld;

use serde_json::Value;
use std::collections::HashSet;
use std::collections::VecDeque;

use super::{
    as_bool, as_u64, fixture, parity_bare_world, require_fields, tile_at_pos, validate_common,
};

fn parity_power_world() -> DynamicWorld {
    parity_bare_world("parity-power-unlink.json")
}

pub(super) fn link_config(dx: i32, dy: i32) -> Vec<u8> {
    let packed = (dx << 16) | (dy as u16 as i32);
    let mut config = vec![8u8, 1u8];
    config.extend_from_slice(&packed.to_be_bytes());
    config
}

pub(super) fn links_sorted(world: &DynamicWorld, position: i32) -> Vec<i32> {
    let mut links = world
        .tiles
        .get(&position)
        .map(|tile| tile.power_links.clone())
        .unwrap_or_default();
    links.sort_unstable();
    links
}

fn connected_via_links(world: &DynamicWorld, a: i32, b: i32) -> bool {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(a);
    seen.insert(a);
    while let Some(position) = queue.pop_front() {
        if position == b {
            return true;
        }
        if let Some(tile) = world.tiles.get(&position) {
            for link in tile.power_links.iter().copied() {
                if seen.insert(link) {
                    queue.push_back(link);
                }
            }
        }
    }
    false
}

fn compare_phase_links(
    fixture: &Value,
    probe: &str,
    phase: &str,
    world: &DynamicWorld,
    source: i32,
    node: i32,
) -> Result<(), String> {
    let phase_value = fixture.get(phase).ok_or_else(|| {
        format!("parity error: fixture '{probe}' is missing required field '{phase}'")
    })?;
    for (field, position) in [("source_links", source), ("node_links", node)] {
        let expected: Vec<i64> = phase_value
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                format!("parity error: fixture '{probe}' {phase}.{field} must be an array")
            })?
            .iter()
            .map(|value| {
                value.as_i64().ok_or_else(|| {
                    format!("parity error: fixture '{probe}' {phase}.{field} must contain integers")
                })
            })
            .collect::<Result<_, _>>()?;
        let actual: Vec<i64> = links_sorted(world, position)
            .into_iter()
            .map(i64::from)
            .collect();
        if actual != expected {
            return Err(format!(
                "parity mismatch: field '{phase}.{field}': java 158.1 = {expected:?}, rust = {actual:?}"
            ));
        }
    }
    Ok(())
}

pub(super) fn compare_power_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &["source_pos", "node_pos", "linked", "unlinked"],
    )?;
    let source = as_u64(fixture, &probe, "source_pos")? as i32;
    let node = as_u64(fixture, &probe, "node_pos")? as i32;

    let world = parity_power_world();
    world.tiles.insert(source, tile_at_pos(source, 410));
    world.tiles.insert(node, tile_at_pos(node, 302));

    // Link the node to the source (relative offsets), then unlink, mirroring
    // the probe's two official PowerNode config dispatches.
    let source_x = source >> 16;
    let source_y = source as i16 as i32;
    let node_x = node >> 16;
    let node_y = node as i16 as i32;
    if !apply_configuration(
        &world,
        node,
        &link_config(source_x - node_x, source_y - node_y),
    ) {
        return Err("parity error: rust apply_configuration rejected the link".to_string());
    }
    compare_phase_links(fixture, &probe, "linked", &world, source, node)?;
    let same_graph = connected_via_links(&world, source, node);
    let expected_same = as_bool(&fixture["linked"], &probe, "same_graph")?;
    if same_graph != expected_same {
        return Err(format!(
            "parity mismatch: field 'linked.same_graph': java 158.1 = {expected_same}, rust = {same_graph}"
        ));
    }

    if !apply_configuration(&world, node, &[8, 0]) {
        return Err("parity error: rust apply_configuration rejected the unlink".to_string());
    }
    compare_phase_links(fixture, &probe, "unlinked", &world, source, node)?;
    let same_graph = connected_via_links(&world, source, node);
    let expected_same = as_bool(&fixture["unlinked"], &probe, "same_graph")?;
    if same_graph != expected_same {
        return Err(format!(
            "parity mismatch: field 'unlinked.same_graph': java 158.1 = {expected_same}, rust = {same_graph}"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Power timing probe (ParPowerTiming158): graph→diode, unlink split, beam
// rescan and payload topology at N/N+1/N+2.
// ---------------------------------------------------------------------------

#[test]
fn power_unlink_matches_java_1581() {
    compare_power_fixture(&fixture("power-unlink.json")).unwrap_or_else(|error| panic!("{error}"));
}
