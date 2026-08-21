//! Parity differential probes — lease domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::logic::compile;
use crate::logic::ExecutorState;
use crate::network::world::DynamicWorld;

use dashmap::DashMap;

use serde_json::Value;
use std::sync::Arc;

use super::{
    as_bool, as_str, as_u64, fixture, parity_bare_world, require_fields, tile_at_pos,
    validate_common,
};

use super::ubind::ubind_probe_unit;

#[derive(Default)]
struct LeaseObservation {
    /// First tick on which the unit is logic-controlled (-1 = never).
    acquire_tick: i64,
    timer_at_acquire: f32,
    /// Remaining lease after the ticks 599/600/601/602.
    boundary: [Option<f32>; 4],
    /// At tick 599: the lease holder is THIS processor position.
    controller_is_build: bool,
    /// First tick on which the (previously controlled) unit is released
    /// (-1 = never).
    release_tick: i64,
    /// Whether the unit was still logic-controlled when destroyed.
    still_logic_at_destroy: bool,
}

impl LeaseObservation {
    fn new() -> Self {
        Self {
            acquire_tick: -1,
            release_tick: -1,
            ..Self::default()
        }
    }
}

fn replay_lease_scenario(
    world: &DynamicWorld,
    processor_pos: i32,
    program: &str,
    ticks: u64,
    destroy_at: Option<u64>,
    unit_id: i32,
) -> LeaseObservation {
    use crate::network::world::UnitAuthority;

    let compiled = compile(program)
        .unwrap_or_else(|| panic!("parity error: lease program must compile in rust"));
    let mut state = ExecutorState::new(compiled, Vec::new());
    let connections: DashMap<i32, crate::network::world::PendingConnection> = DashMap::new();
    let view = crate::logic::WorldView {
        world,
        processor_pos,
        out: &connections,
    };
    let mut obs = LeaseObservation::new();
    for tick in 1..=ticks as i64 {
        // Units update before buildings: the LogicAI lease clock first.
        crate::network::simulation::simulate_logic_control_leases(world, 1.0);
        // Then the processor's single instruction for this tick.
        state.run_tick(Some(&view), 1);

        let authority = world
            .enemies
            .get(&unit_id)
            .map(|unit| unit.authority)
            .unwrap_or(UnitAuthority::DefaultAi);
        match authority {
            UnitAuthority::Logic {
                processor_pos: holder,
                remaining_ticks,
                ..
            } => {
                if obs.acquire_tick < 0 {
                    obs.acquire_tick = tick;
                    obs.timer_at_acquire = remaining_ticks;
                }
                match tick {
                    599 => {
                        obs.boundary[0] = Some(remaining_ticks);
                        obs.controller_is_build = holder == processor_pos;
                    }
                    600 => obs.boundary[1] = Some(remaining_ticks),
                    601 => obs.boundary[2] = Some(remaining_ticks),
                    602 => obs.boundary[3] = Some(remaining_ticks),
                    _ => {}
                }
            }
            _ => {
                if obs.acquire_tick >= 0 && obs.release_tick < 0 {
                    obs.release_tick = tick;
                }
            }
        }

        if destroy_at == Some(tick as u64) {
            obs.still_logic_at_destroy = matches!(authority, UnitAuthority::Logic { .. });
            // Official destruction: the tile no longer holds the processor.
            world.tiles.remove(&processor_pos);
        }
    }
    obs
}

pub(super) fn lease_num(fixture: &Value, probe: &str, field: &str) -> Result<f64, String> {
    fixture
        .get(field)
        .and_then(Value::as_f64)
        .ok_or_else(|| format!("parity error: fixture '{probe}' field '{field}' must be a number"))
}

fn compare_lease_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &[
            "program",
            "acquire_tick",
            "timer_at_acquire",
            "timer_at_599",
            "timer_at_600",
            "timer_at_601",
            "timer_at_602",
            "controller_is_build_at_599",
            "release_tick",
            "destroy_at",
            "reacquire_tick",
            "still_logic_at_destroy",
            "destroy_release_tick",
            "processor_team",
        ],
    )?;
    let program = as_str(fixture, &probe, "program")?;
    let ticks = as_u64(fixture, &probe, "tick")?;
    let processor_team = as_u64(fixture, &probe, "processor_team")? as u8;
    let destroy_at = as_u64(fixture, &probe, "destroy_at")?;

    // Same scenario as the probe: a micro processor (431) of the executor
    // team and one dagger (the Java unit's default CommandAI maps to the
    // port's default authority for a player-commandable team).
    let world = parity_bare_world("parity-lease-600.json");
    let processor_pos = (1 << 16) | 1;
    let mut processor_tile = tile_at_pos(processor_pos, 431);
    processor_tile.team = processor_team;
    world.tiles.insert(processor_pos, processor_tile);
    let unit_id = 3_000_001;
    world
        .enemies
        .insert(unit_id, ubind_probe_unit(unit_id, processor_team, 0.0));
    let default_authority = {
        let unit = world.enemies.get(&unit_id).unwrap();
        // dashmap-guard: allow DM900 reason="default_unit_authority reads wave rules and game state only; it does not access world.enemies"
        crate::network::units::default_unit_authority(&world, &unit)
    };
    world.enemies.get_mut(&unit_id).unwrap().authority = default_authority;
    let world = Arc::new(world);

    // Scenario A: single ucontrol, no refresh — the 600-tick lease boundary.
    let a = replay_lease_scenario(&world, processor_pos, program, ticks, None, unit_id);

    // Scenario B: the SAME (still valid) processor re-acquires, then the
    // tile is destroyed at `destroy_at`.
    let b = replay_lease_scenario(
        &world,
        processor_pos,
        program,
        ticks,
        Some(destroy_at),
        unit_id,
    );

    let expect_tick = |field: &str| -> Result<i64, String> {
        lease_num(fixture, &probe, field).map(|value| value as i64)
    };
    let expect_timer = |field: &str| -> Result<f32, String> {
        lease_num(fixture, &probe, field).map(|value| value as f32)
    };
    let close = |a: f32, b: f32| (a - b).abs() < 0.0001;

    let mut failures = Vec::new();
    let expected = expect_tick("acquire_tick")?;
    if a.acquire_tick != expected {
        failures.push(format!(
            "acquire_tick: java 158.1 = {expected}, rust = {}",
            a.acquire_tick
        ));
    }
    let expected = expect_timer("timer_at_acquire")?;
    if !close(a.timer_at_acquire, expected) {
        failures.push(format!(
            "timer_at_acquire: java 158.1 = {expected}, rust = {}",
            a.timer_at_acquire
        ));
    }
    for (index, field) in [
        "timer_at_599",
        "timer_at_600",
        "timer_at_601",
        "timer_at_602",
    ]
    .iter()
    .enumerate()
    {
        let expected = expect_timer(field)?;
        match a.boundary[index] {
            Some(actual) if close(actual, expected) => {}
            actual => failures.push(format!(
                "{field}: java 158.1 = {expected}, rust = {}",
                actual.map_or("released".to_string(), |v| v.to_string())
            )),
        }
    }
    let expected = as_bool(fixture, &probe, "controller_is_build_at_599")?;
    if a.controller_is_build != expected {
        failures.push(format!(
            "controller_is_build_at_599: java 158.1 = {expected}, rust = {}",
            a.controller_is_build
        ));
    }
    let expected = expect_tick("release_tick")?;
    if a.release_tick != expected {
        failures.push(format!(
            "release_tick: java 158.1 = {expected}, rust = {}",
            a.release_tick
        ));
    }
    let expected = expect_tick("reacquire_tick")?;
    if b.acquire_tick != expected {
        failures.push(format!(
            "reacquire_tick: java 158.1 = {expected}, rust = {}",
            b.acquire_tick
        ));
    }
    let expected = as_bool(fixture, &probe, "still_logic_at_destroy")?;
    if b.still_logic_at_destroy != expected {
        failures.push(format!(
            "still_logic_at_destroy: java 158.1 = {expected}, rust = {}",
            b.still_logic_at_destroy
        ));
    }
    let expected = expect_tick("destroy_release_tick")?;
    if b.release_tick != expected {
        failures.push(format!(
            "destroy_release_tick: java 158.1 = {expected}, rust = {}",
            b.release_tick
        ));
    }
    if !failures.is_empty() {
        return Err(format!(
            "parity mismatch: fixture '{probe}' lease lifecycle diverges: {}",
            failures.join("; ")
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Logic-owner probe (ParLogicOwner158): same-tile processor replacement
// ---------------------------------------------------------------------------

#[test]
fn lease_600_matches_java_1581() {
    // Differential P0-03: acquisition tick, the exact 600-tick lease
    // boundary (timer 3.0/2.0/1.0/0.0 at ticks 599-602, release on 603) and
    // the destroy-release tick (50 -> 51) replayed by the Rust engine.
    compare_lease_fixture(&fixture("lease-600.json")).unwrap_or_else(|error| panic!("{error}"));
}
