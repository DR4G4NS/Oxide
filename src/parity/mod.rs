//! P0-00 — Differential probe harness (directory module after M3 rehearsal split).
//! Shared fixture harness + topic modules by authoritative parity domain.
//! Mechanical movement only; assertions and fixtures unchanged.

use crate::logic::ExecutorState;
use crate::network::world::DynamicTile;
use crate::network::world::DynamicWorld;
use crate::state::game_state::GameState;
use dashmap::DashMap;

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

pub(super) mod bullet_status;
pub(super) mod command;
pub(super) mod controller_save;
pub(super) mod floor_status;
pub(super) mod lease;
pub(super) mod logic;
pub(super) mod logic_owner;
pub(super) mod logic_takeover;
pub(super) mod move_timing;
pub(super) mod payload;
pub(super) mod possession;
pub(super) mod power;
pub(super) mod power_timing;
pub(super) mod status;
pub(super) mod ubind;
use self::command::compare_command_fixture;
use self::logic::compare_logic_fixture;
use self::power::compare_power_fixture;
use self::status::compare_status_fixture;
use self::ubind::compare_ubind_fixture;
use self::ubind::compare_ubind_object_fixture;
use self::ubind::compare_ubind_reinsert_fixture;

// Shared harness infrastructure:

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/parity/fixtures");

const EXPECTED_VERSION: &str = "158.1";

// ---------------------------------------------------------------------------
// Fixture loading and validation
// ---------------------------------------------------------------------------

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(FIXTURES_DIR).join(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read fixture {}: {error}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|error| panic!("fixture {} is not parseable JSON: {error}", path.display()))
}

fn require_fields(fixture: &Value, probe: &str, fields: &[&str]) -> Result<(), String> {
    for field in fields {
        if fixture.get(*field).is_none() {
            return Err(format!(
                "parity error: fixture '{probe}' is missing required field '{field}'"
            ));
        }
    }
    Ok(())
}

fn as_str<'a>(fixture: &'a Value, probe: &str, field: &str) -> Result<&'a str, String> {
    fixture
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("parity error: fixture '{probe}' field '{field}' must be a string"))
}

fn as_u64(fixture: &Value, probe: &str, field: &str) -> Result<u64, String> {
    fixture.get(field).and_then(Value::as_u64).ok_or_else(|| {
        format!("parity error: fixture '{probe}' field '{field}' must be an integer")
    })
}

fn as_bool(fixture: &Value, probe: &str, field: &str) -> Result<bool, String> {
    fixture
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("parity error: fixture '{probe}' field '{field}' must be a boolean"))
}

fn validate_common(fixture: &Value) -> Result<String, String> {
    let probe = fixture
        .get("probe_name")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>")
        .to_string();
    require_fields(fixture, &probe, &["probe_version", "probe_name", "tick"])?;
    match fixture.get("probe_version").and_then(Value::as_str) {
        Some(version) if version == EXPECTED_VERSION => {}
        other => {
            return Err(format!(
                "parity error: fixture '{probe}' was captured with probe_version {other:?}, \
                 expected {EXPECTED_VERSION:?}. Probes must run against the official \
                 desktop.jar 158.1 (tools/parity_probe_158.sh enforces this at capture time)."
            ));
        }
    }
    Ok(probe)
}

// ---------------------------------------------------------------------------
// Logic probe (ParLogic158): LExecutor state after N runOnce calls
// ---------------------------------------------------------------------------

fn user_vars(state: &ExecutorState) -> BTreeMap<String, (bool, f64)> {
    state
        .vars
        .iter()
        .filter(|variable| !variable.name.starts_with('@'))
        .map(|variable| (variable.name.clone(), (variable.isobj, variable.numval)))
        .collect()
}

fn compare_executor_state(
    state: &ExecutorState,
    fixture: &Value,
    probe: &str,
) -> Result<(), String> {
    let counter = state.counter as u64;
    let expected_counter = as_u64(fixture, probe, "counter")?;
    if counter != expected_counter {
        return Err(format!(
            "parity mismatch: field 'counter': java 158.1 = {expected_counter}, rust = {counter}"
        ));
    }

    let expected_text = as_str(fixture, probe, "text")?;
    if state.text_buffer != expected_text {
        return Err(format!(
            "parity mismatch: field 'text':\n  java 158.1 = {expected_text:?}\n  rust       = {:?}",
            state.text_buffer
        ));
    }

    let vars = user_vars(state);
    let expected_vars = fixture
        .get("vars")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("parity error: fixture '{probe}' field 'vars' must be an object"))?;
    if vars.len() != expected_vars.len() {
        return Err(format!(
            "parity mismatch: field 'vars': java 158.1 defines {} variables, rust has {} \
             (java: {:?}, rust: {:?})",
            expected_vars.len(),
            vars.len(),
            expected_vars.keys().collect::<Vec<_>>(),
            vars.keys().collect::<Vec<_>>(),
        ));
    }
    for (name, expected) in expected_vars {
        let (isobj, num) = vars.get(name).ok_or_else(|| {
            format!("parity mismatch: field 'vars.{name}': java 158.1 defines it, rust does not")
        })?;
        let expected_isobj = expected
            .get("isobj")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                format!("parity error: fixture '{probe}' vars.{name}.isobj must be a boolean")
            })?;
        let expected_num = expected.get("num").and_then(Value::as_f64).ok_or_else(|| {
            format!("parity error: fixture '{probe}' vars.{name}.num must be a number")
        })?;
        if *isobj != expected_isobj {
            return Err(format!(
                "parity mismatch: field 'vars.{name}.isobj': java 158.1 = {expected_isobj}, rust = {isobj}"
            ));
        }
        if *num != expected_num {
            return Err(format!(
                "parity mismatch: field 'vars.{name}.num': java 158.1 = {expected_num}, rust = {num}"
            ));
        }
    }
    Ok(())
}

fn tile_at_pos(position: i32, block: i16) -> DynamicTile {
    DynamicTile {
        position,
        block,
        rotation: 0,
        team: 1,
        config: vec![0],
        enabled: true,
        message: None,
        occupied: vec![position],
        stored_item: -1,
        stored_amount: 0,
        production_progress: 0.0,
        transport_progress: 0.0,
        ammo_units: 0.0,
        inventory: Vec::new(),
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
        health: f32::MAX,
        door_open: false,
        shield: 0.0,
        light_color: -1_900_545,
        memory: Vec::new(),
        duct_rec_dir: 0,
        unloader_offset: 0,
        conveyor_items: Vec::new(),
        factory_command: None,
        stack_state: 0,
        stack_link: 0,
        stack_cooldown: 0.0,
        generation: 0,
    }
}

fn parity_bare_world(save_name: &str) -> DynamicWorld {
    let width = 16i32;
    let height = 16i32;
    DynamicWorld {
        game_state: GameState::new(),
        width,
        height,
        sharded_unit_cap: 8,
        core_position: 0,
        core_max_health: 0.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: vec![0i16; (width * height) as usize],
        base_centers: vec![false; (width * height) as usize],
        tile_data: Vec::new(),
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: Vec::new(),
        overlays: Vec::new(),
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
        save_path: PathBuf::from(format!("/tmp/{save_name}")),
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

fn compare_bool(
    fixture: &Value,
    probe: &str,
    field: &str,
    rust: bool,
    java: bool,
) -> Result<(), String> {
    let _ = (fixture, probe);
    if rust != java {
        Err(format!(
            "parity mismatch: field '{field}': java 158.1 = {java}, rust = {rust}"
        ))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Payload RPC probe (ParPayload158): build/unit/drop pickup lifecycle
// ---------------------------------------------------------------------------

fn approx_json_f32(expected: &Value, actual: f32) -> bool {
    let Some(target) = expected.as_f64() else {
        return false;
    };
    (target as f32 - actual).abs() <= 0.001f32.max((target as f32).abs() * 1e-5)
}

// ---------------------------------------------------------------------------
// Controller save probe (ParControllerSave158): MSAV controller round-trip
// ---------------------------------------------------------------------------

#[test]
fn fixture_missing_required_field_fails_naming_the_field() {
    let mut fixture_value = fixture("logic-601.json");
    fixture_value.as_object_mut().unwrap().remove("text");
    let error = compare_logic_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'text'"),
        "error must name the missing field, got: {error}"
    );

    let mut fixture_value = fixture("power-unlink.json");
    fixture_value.as_object_mut().unwrap().remove("unlinked");
    let error = compare_power_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'unlinked'"),
        "error must name the missing field, got: {error}"
    );

    let mut fixture_value = fixture("ubind-20.json");
    fixture_value.as_object_mut().unwrap().remove("unit_count");
    let error = compare_ubind_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'unit_count'"),
        "error must name the missing field, got: {error}"
    );

    let mut fixture_value = fixture("ubind-object.json");
    fixture_value.as_object_mut().unwrap().remove("cases");
    let error = compare_ubind_object_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'cases'"),
        "error must name the missing field, got: {error}"
    );

    let mut fixture_value = fixture("ubind-reinsert.json");
    fixture_value.as_object_mut().unwrap().remove("scenarios");
    let error = compare_ubind_reinsert_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'scenarios'"),
        "error must name the missing field, got: {error}"
    );

    let mut fixture_value = fixture("command-queue.json");
    fixture_value
        .as_object_mut()
        .unwrap()
        .remove("cap_queue_size");
    let error = compare_command_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'cap_queue_size'"),
        "error must name the missing field, got: {error}"
    );

    let mut fixture_value = fixture("status.json");
    fixture_value.as_object_mut().unwrap().remove("disarmed");
    let error = compare_status_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'disarmed'"),
        "error must name the missing field, got: {error}"
    );
}

#[test]
fn fixture_without_probe_version_is_rejected() {
    let mut fixture_value = fixture("logic-601.json");
    fixture_value
        .as_object_mut()
        .unwrap()
        .remove("probe_version");
    let error = compare_logic_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("probe_version"),
        "error must name the missing field, got: {error}"
    );
}

#[test]
fn fixture_from_another_build_is_rejected() {
    let mut fixture_value = fixture("logic-601.json");
    fixture_value["probe_version"] = serde_json::json!("158.2");
    let error = compare_logic_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("158.1") && error.contains("158.2"),
        "error must report both the fixture version and the expected one, got: {error}"
    );
}

#[test]
fn mismatch_reports_the_divergent_field() {
    // Corrupt one variable: the failure must name vars.m.num.
    let mut fixture_value = fixture("logic-601.json");
    fixture_value["vars"]["m"]["num"] = serde_json::json!(99.0);
    let error = compare_logic_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("vars.m.num"),
        "error must name the divergent field, got: {error}"
    );

    // Corrupt the text buffer: the failure must name 'text'.
    let mut fixture_value = fixture("logic-601.json");
    fixture_value["text"] = serde_json::json!("n=1n=2n=3");
    let error = compare_logic_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'text'"),
        "error must name the divergent field, got: {error}"
    );

    // Corrupt one link set on the power probe: must name linked.node_links.
    let mut fixture_value = fixture("power-unlink.json");
    fixture_value["linked"]["node_links"] = serde_json::json!([]);
    let error = compare_power_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("linked.node_links"),
        "error must name the divergent field, got: {error}"
    );

    // Corrupt the ubind sequence: must name 'text' (the 20-bind sequence).
    let mut fixture_value = fixture("ubind-20.json");
    fixture_value["text"] =
        serde_json::json!("11 12 13 14 15 11 12 13 14 15 11 12 13 14 15 11 12 13 14 14 ");
    let error = compare_ubind_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("'text'"),
        "error must name the divergent field, got: {error}"
    );

    // Corrupt one Unit-object ubind case: must name cases.<name>.bound.
    let mut fixture_value = fixture("ubind-object.json");
    fixture_value["cases"]["same_team_player"]["bound"] = serde_json::json!(false);
    let error = compare_ubind_object_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("cases.same_team_player.bound"),
        "error must name the divergent field, got: {error}"
    );

    // Corrupt a reinsert sequence: must name the scenario text field.
    let mut fixture_value = fixture("ubind-reinsert.json");
    fixture_value["scenarios"]["readd"]["text"] = serde_json::json!("11 12 13 ");
    let error = compare_ubind_reinsert_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("scenarios.readd.text"),
        "error must name the divergent field, got: {error}"
    );

    let mut fixture_value = fixture("status.json");
    fixture_value["disarmed"]["can_shoot"] = serde_json::json!(true);
    let error = compare_status_fixture(&fixture_value).unwrap_err();
    assert!(
        error.contains("disarmed.can_shoot"),
        "error must name the divergent field, got: {error}"
    );
}
