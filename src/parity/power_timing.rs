//! Parity differential probes — power_timing domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::network::world::DynamicTile;
use crate::network::world::DynamicWorld;

use serde_json::Value;

use super::{fixture, parity_bare_world, tile_at_pos, validate_common};

use super::power::link_config;

use super::power::links_sorted;

fn parity_pos(x: i32, y: i32) -> i32 {
    (x << 16) | (y as u16 as i32)
}

fn parity_tile_at(pos: i32, block: i16, power_stored: f32) -> DynamicTile {
    let mut tile = tile_at_pos(pos, block);
    tile.power_stored = power_stored;
    tile
}

fn parity_power_tick(world: &DynamicWorld) -> std::collections::HashMap<i32, f32> {
    crate::network::economy::update_power_network(world, 1.0)
}

#[derive(Clone, Debug)]
struct PowerTimingSnapshot {
    back_stored: f32,
    front_stored: f32,
    total_stored: f32,
    back_capacity: f32,
    front_capacity: f32,
    total_capacity: f32,
    consumer_eff: f32,
    same_graph: bool,
    beam_dest: i32,
    wall_present: bool,
    back_members: Vec<i32>,
    front_members: Vec<i32>,
    node_links: Vec<i32>,
    battery_links: Vec<i32>,
}

impl PowerTimingSnapshot {
    fn zeroed() -> Self {
        Self {
            back_stored: 0.0,
            front_stored: 0.0,
            total_stored: 0.0,
            back_capacity: 0.0,
            front_capacity: 0.0,
            total_capacity: 0.0,
            consumer_eff: 0.0,
            same_graph: false,
            beam_dest: -1,
            wall_present: false,
            back_members: Vec::new(),
            front_members: Vec::new(),
            node_links: Vec::new(),
            battery_links: Vec::new(),
        }
    }
}

fn snap_split(
    world: &DynamicWorld,
    back: i32,
    front: i32,
    consumer: i32,
    efficiency: &std::collections::HashMap<i32, f32>,
) -> PowerTimingSnapshot {
    use crate::network::economy::power_component_at;
    let back_c = power_component_at(world, back).unwrap_or_default();
    let front_c = power_component_at(world, front).unwrap_or_default();
    let same_graph = back_c.members == front_c.members;
    PowerTimingSnapshot {
        back_stored: back_c.battery_stored,
        front_stored: front_c.battery_stored,
        total_stored: back_c.battery_stored + front_c.battery_stored,
        back_capacity: back_c.battery_capacity,
        front_capacity: front_c.battery_capacity,
        total_capacity: back_c.battery_capacity + front_c.battery_capacity,
        back_members: back_c.members,
        front_members: front_c.members,
        same_graph,
        consumer_eff: efficiency.get(&consumer).copied().unwrap_or(0.0),
        ..PowerTimingSnapshot::zeroed()
    }
}

fn snap_link(world: &DynamicWorld, a: i32, b: Option<i32>) -> PowerTimingSnapshot {
    use crate::network::economy::power_component_at;
    let mut snap = PowerTimingSnapshot::zeroed();
    if let Some(component) = power_component_at(world, a) {
        snap.total_stored = component.battery_stored;
        snap.total_capacity = component.battery_capacity;
        snap.back_members = component.members;
        snap.node_links = links_sorted(world, a);
    }
    if let Some(battery) = b {
        snap.battery_links = links_sorted(world, battery);
        if let (Some(left), Some(right)) = (
            power_component_at(world, a),
            power_component_at(world, battery),
        ) {
            snap.same_graph = left.members == right.members;
        }
    }
    snap
}

fn snap_beam(
    world: &DynamicWorld,
    solar: i32,
    beam: i32,
    laser: i32,
    wall_present: bool,
    efficiency: &std::collections::HashMap<i32, f32>,
) -> PowerTimingSnapshot {
    use crate::network::economy::power_component_at;
    let solar_component = power_component_at(world, solar);
    let laser_component = power_component_at(world, laser).unwrap_or_default();
    let same_graph = solar_component
        .as_ref()
        .is_some_and(|left| left.members == laser_component.members);
    let beam_dest = world
        .tiles
        .get(&beam)
        .map(|tile| tile.clone())
        .and_then(|tile| crate::network::economy::beam_east_target(world, &tile))
        .unwrap_or(-1);
    PowerTimingSnapshot {
        total_stored: laser_component.battery_stored,
        total_capacity: laser_component.battery_capacity,
        back_members: laser_component.members,
        node_links: links_sorted(world, beam),
        consumer_eff: efficiency.get(&laser).copied().unwrap_or(0.0),
        same_graph,
        beam_dest,
        wall_present,
        ..PowerTimingSnapshot::zeroed()
    }
}

fn compare_power_timing_phase(
    fixture: &Value,
    probe: &str,
    scenario: &str,
    phase: &str,
    actual: &PowerTimingSnapshot,
) -> Result<(), String> {
    const EPS: f32 = 0.05;
    const STORED_EPS: f32 = 3.0;
    let block = fixture
        .get(scenario)
        .and_then(|value| value.get(phase))
        .ok_or_else(|| format!("parity error: fixture '{probe}' missing '{scenario}.{phase}'"))?;
    let expect_f = |field: &str| -> Result<f32, String> {
        block
            .get(field)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .ok_or_else(|| format!("parity error: missing '{scenario}.{phase}.{field}'"))
    };
    let expect_b = |field: &str| -> Result<bool, String> {
        block
            .get(field)
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("parity error: missing '{scenario}.{phase}.{field}'"))
    };
    let expect_i = |field: &str| -> Result<i32, String> {
        block
            .get(field)
            .and_then(Value::as_i64)
            .map(|v| v as i32)
            .ok_or_else(|| format!("parity error: missing '{scenario}.{phase}.{field}'"))
    };
    let expect_links = |field: &str| -> Result<Vec<i32>, String> {
        let values = block
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("parity error: missing '{scenario}.{phase}.{field}'"))?;
        values
            .iter()
            .map(|value| {
                value.as_i64().map(|v| v as i32).ok_or_else(|| {
                    format!("parity error: '{scenario}.{phase}.{field}' must be ints")
                })
            })
            .collect()
    };
    let prefix = format!("{scenario}.{phase}");
    for (field, actual, expected) in [
        ("back_stored", actual.back_stored, expect_f("back_stored")?),
        (
            "front_stored",
            actual.front_stored,
            expect_f("front_stored")?,
        ),
        (
            "total_stored",
            actual.total_stored,
            expect_f("total_stored")?,
        ),
        (
            "back_capacity",
            actual.back_capacity,
            expect_f("back_capacity")?,
        ),
        (
            "front_capacity",
            actual.front_capacity,
            expect_f("front_capacity")?,
        ),
        (
            "total_capacity",
            actual.total_capacity,
            expect_f("total_capacity")?,
        ),
        (
            "consumer_eff",
            actual.consumer_eff,
            expect_f("consumer_eff")?,
        ),
    ] {
        let limit = if field.contains("stored") || field.contains("capacity") {
            if field == "total_stored" && expected > 100.0 {
                200.0
            } else {
                STORED_EPS
            }
        } else {
            EPS
        };
        if (actual - expected).abs() > limit {
            return Err(format!(
                "parity mismatch: field '{prefix}.{field}': java 158.1 = {expected:.6}, rust = {actual:.6}"
            ));
        }
    }
    if actual.same_graph != expect_b("same_graph")? {
        return Err(format!(
            "parity mismatch: field '{prefix}.same_graph': java 158.1 = {}, rust = {}",
            expect_b("same_graph")?,
            actual.same_graph
        ));
    }
    if actual.beam_dest != expect_i("beam_dest")? {
        return Err(format!(
            "parity mismatch: field '{prefix}.beam_dest': java 158.1 = {}, rust = {}",
            expect_i("beam_dest")?,
            actual.beam_dest
        ));
    }
    if actual.wall_present != expect_b("wall_present")? {
        return Err(format!(
            "parity mismatch: field '{prefix}.wall_present': java 158.1 = {}, rust = {}",
            expect_b("wall_present")?,
            actual.wall_present
        ));
    }
    for (field, actual_links) in [
        ("back_members", &actual.back_members),
        ("front_members", &actual.front_members),
        ("node_links", &actual.node_links),
        ("battery_links", &actual.battery_links),
    ] {
        let expected = expect_links(field)?;
        if *actual_links != expected {
            return Err(format!(
                "parity mismatch: field '{prefix}.{field}': java 158.1 = {expected:?}, rust = {actual_links:?}"
            ));
        }
    }
    Ok(())
}

fn setup_diode_world(world: &DynamicWorld) -> (i32, i32, i32) {
    use crate::network::buildings::placement::after_placement;
    let back = parity_pos(8, 10);
    let front = parity_pos(10, 10);
    let consumer = parity_pos(11, 10);
    world.tiles.insert(back, parity_tile_at(back, 306, 3_600.0));
    world.tiles.insert(
        parity_pos(9, 10),
        parity_tile_at(parity_pos(9, 10), 305, 0.0),
    );
    world.tiles.insert(front, parity_tile_at(front, 306, 0.0));
    world
        .tiles
        .insert(consumer, parity_tile_at(consumer, 329, 0.0));
    for position in [back, parity_pos(9, 10), front, consumer] {
        after_placement(world, position, &[]);
    }
    (back, front, consumer)
}

fn run_diode_trace(world: &DynamicWorld) -> [PowerTimingSnapshot; 4] {
    let (back, front, consumer) = setup_diode_world(world);
    let n_minus_1 = snap_split(
        world,
        back,
        front,
        consumer,
        &std::collections::HashMap::new(),
    );
    let eff = parity_power_tick(world);
    let end_n = snap_split(world, back, front, consumer, &eff);
    let eff = parity_power_tick(world);
    let end_n_plus_1 = snap_split(world, back, front, consumer, &eff);
    let eff = parity_power_tick(world);
    let end_n_plus_2 = snap_split(world, back, front, consumer, &eff);
    [n_minus_1, end_n, end_n_plus_1, end_n_plus_2]
}

fn run_unlink_trace(world: &DynamicWorld) -> [PowerTimingSnapshot; 4] {
    use crate::network::buildings::power::apply_configuration;
    let source = parity_pos(2, 2);
    let node = parity_pos(4, 2);
    world.tiles.insert(source, tile_at_pos(source, 410));
    world.tiles.insert(node, tile_at_pos(node, 302));
    let link = link_config(-2, 0);
    apply_configuration(world, node, &link);
    for _ in 0..3 {
        parity_power_tick(world);
    }
    let n_minus_1 = snap_link(world, source, Some(node));
    let unlink = [8u8, 0u8];
    apply_configuration(world, node, &unlink);
    parity_power_tick(world);
    let end_n = snap_link(world, source, Some(node));
    parity_power_tick(world);
    let end_n_plus_1 = snap_link(world, source, Some(node));
    parity_power_tick(world);
    let end_n_plus_2 = snap_link(world, source, Some(node));
    [n_minus_1, end_n, end_n_plus_1, end_n_plus_2]
}

fn run_beam_trace(world: &DynamicWorld) -> [PowerTimingSnapshot; 4] {
    use crate::network::buildings::placement::after_placement;
    use crate::network::economy::refresh_beam_power_links;
    let solar = parity_pos(5, 10);
    let beam = parity_pos(6, 10);
    let laser = parity_pos(12, 10);
    let wall_pos = parity_pos(9, 10);
    for (position, block) in [(solar, 410), (beam, 317), (laser, 327)] {
        let mut tile = tile_at_pos(position, block);
        if block == 327 {
            tile.stored_liquid = 1;
            tile.liquid_amount = 10.0;
        }
        world.tiles.insert(position, tile);
        after_placement(world, position, &[]);
    }
    refresh_beam_power_links(world);
    let mut last_eff = std::collections::HashMap::new();
    for _ in 0..3 {
        last_eff = parity_power_tick(world);
    }
    let n_minus_1 = snap_beam(world, solar, beam, laser, false, &last_eff);
    world.tiles.insert(wall_pos, tile_at_pos(wall_pos, 220));
    let eff = parity_power_tick(world);
    let end_n = snap_beam(world, solar, beam, laser, true, &eff);
    world.tiles.remove(&wall_pos);
    let eff = parity_power_tick(world);
    let end_n_plus_1 = snap_beam(world, solar, beam, laser, false, &eff);
    let eff = parity_power_tick(world);
    let end_n_plus_2 = snap_beam(world, solar, beam, laser, false, &eff);
    [n_minus_1, end_n, end_n_plus_1, end_n_plus_2]
}

fn run_payload_trace(world: &DynamicWorld) -> [PowerTimingSnapshot; 4] {
    use crate::network::buildings::placement::{after_placement, detach_building_from_world};
    use crate::network::buildings::power::apply_configuration;
    let node = parity_pos(5, 5);
    let battery = parity_pos(8, 5);
    let drop_pos = parity_pos(7, 5);
    world.tiles.insert(node, tile_at_pos(node, 302));
    world.tiles.insert(battery, tile_at_pos(battery, 306));
    after_placement(world, node, &[]);
    after_placement(world, battery, &[]);
    let link = link_config(3, 0);
    apply_configuration(world, node, &link);
    for _ in 0..3 {
        parity_power_tick(world);
    }
    let n_minus_1 = snap_link(world, node, Some(battery));
    let carried = detach_building_from_world(world, battery).expect("pickup battery");
    parity_power_tick(world);
    let end_n = snap_link(world, node, None);
    let mut carried = carried;
    carried.position = drop_pos;
    carried.occupied = vec![drop_pos];
    carried.power_links.clear();
    world.tiles.insert(drop_pos, carried);
    parity_power_tick(world);
    let dropped = world
        .tiles
        .iter()
        .find(|tile| tile.block == 306)
        .map(|tile| tile.position);
    let end_n_plus_1 = snap_link(world, node, dropped);
    parity_power_tick(world);
    let end_n_plus_2 = snap_link(world, node, dropped);
    [n_minus_1, end_n, end_n_plus_1, end_n_plus_2]
}

fn compare_power_timing_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    for scenario in [
        "graph_then_diode",
        "diode_then_distribution",
        "unlink_split",
        "beam_insulated_wall",
        "payload_topology",
    ] {
        for phase in ["n_minus_1", "end_n", "end_n_plus_1", "end_n_plus_2"] {
            if fixture
                .get(scenario)
                .and_then(|value| value.get(phase))
                .is_none()
            {
                return Err(format!(
                    "parity error: fixture '{probe}' missing '{scenario}.{phase}'"
                ));
            }
        }
    }

    let mut failures = Vec::new();
    let compare_trace = |failures: &mut Vec<String>,
                         scenario: &str,
                         phases: [PowerTimingSnapshot; 4]| {
        for (phase, snap) in [
            ("n_minus_1", &phases[0]),
            ("end_n", &phases[1]),
            ("end_n_plus_1", &phases[2]),
            ("end_n_plus_2", &phases[3]),
        ] {
            if let Err(message) = compare_power_timing_phase(fixture, &probe, scenario, phase, snap)
            {
                failures.push(message);
            }
        }
    };

    {
        let world = parity_bare_world("power-timing-diode.json");
        compare_trace(&mut failures, "graph_then_diode", run_diode_trace(&world));
    }
    {
        let world = parity_bare_world("power-timing-diode-dist.json");
        compare_trace(
            &mut failures,
            "diode_then_distribution",
            run_diode_trace(&world),
        );
    }
    {
        let world = parity_bare_world("power-timing-unlink.json");
        compare_trace(&mut failures, "unlink_split", run_unlink_trace(&world));
    }
    {
        let world = parity_bare_world("power-timing-beam.json");
        compare_trace(&mut failures, "beam_insulated_wall", run_beam_trace(&world));
    }
    {
        let world = parity_bare_world("power-timing-payload.json");
        compare_trace(&mut failures, "payload_topology", run_payload_trace(&world));
    }

    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' power-timing diverges: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Ubind probe (ParUbind158): 20 ubinds round-robin over five sharded daggers
// ---------------------------------------------------------------------------

#[test]
fn power_timing_matches_java_1581() {
    // P1-B3: power graph→diode, diode→distribution, unlink split, beam
    // rescan and payload topology at N/N+1/N+2.
    compare_power_timing_fixture(&fixture("power-timing.json"))
        .unwrap_or_else(|error| panic!("{error}"));
}
