//! Snapshot layout parity tests for the BlockSnapshot building codecs.
//!
//! Every layout asserted here is derived from the official Java
//! `Building.writeSync`/subclass `write()` overrides in the local
//! Mindustry 8 checkout (core/src/mindustry/world/Building.java and
//! world/blocks/...) and cross-checked against the `server-release.jar`
//! module flags via `tools/inspect/InspectBlockPower.java`.
//!
//! Base layout (Building.writeBase):
//!   f32 health, byte rotation|0x80, byte team, byte version(3),
//!   byte enabled, byte moduleBitmask, then ItemModule/PowerModule/
//!   LiquidModule, then byte efficiency*255, byte optionalEfficiency*255.
//! ItemModule:  s16 count, (s16 item, i32 amount)*
//! PowerModule: s16 linkCount, i32 pos*, f32 status
//! LiquidModule:s16 count, (s16 liquid, f32 amount)*

use std::collections::HashMap;
use std::io::{Cursor, Read};

use oxide::network::codec::Reads;
use oxide::network::listener::{
    encode_dynamic_tile_sync, is_block_snapshot_supported, is_core_block, valid_logic_config,
};
use oxide::network::world::DynamicTile;

fn tile(block: i16) -> DynamicTile {
    DynamicTile {
        position: (45 << 16) | 100,
        block,
        rotation: 2,
        team: 1,
        config: vec![0],
        occupied: vec![(45 << 16) | 100],
        enabled: true,
        message: None,
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
        stack_link: -1,
        stack_cooldown: 0.0,
        generation: 0,
    }
}

fn encode(block: i16, mutate: impl FnOnce(&mut DynamicTile)) -> Vec<u8> {
    let mut t = tile(block);
    mutate(&mut t);
    let mut sync = Vec::new();
    encode_dynamic_tile_sync(&mut sync, &t, &HashMap::new(), None).unwrap();
    sync
}

fn decode(sync: Vec<u8>) -> Cursor<Vec<u8>> {
    let mut input = Cursor::new(sync);
    // base header
    let health = input.read_f().unwrap();
    assert!(health.is_finite() && health > 0.0, "health must be finite");
    assert_eq!(
        input.read_b().unwrap(),
        2 | 0x80,
        "rotation | new-format marker"
    );
    assert_eq!(input.read_b().unwrap(), 1, "team");
    assert_eq!(input.read_b().unwrap(), 3, "base version");
    assert_eq!(input.read_b().unwrap(), 1, "enabled");
    input
}

/// Like `decode`, but asserts an explicit `enabled` byte (for blocks whose
/// base enabled state is toggled, e.g. a disabled switch).
fn decode_base_manual(sync: Vec<u8>, enabled: u8) -> Cursor<Vec<u8>> {
    let mut input = Cursor::new(sync);
    let health = input.read_f().unwrap();
    assert!(health.is_finite() && health > 0.0, "health must be finite");
    assert_eq!(
        input.read_b().unwrap(),
        2 | 0x80,
        "rotation | new-format marker"
    );
    assert_eq!(input.read_b().unwrap(), 1, "team");
    assert_eq!(input.read_b().unwrap(), 3, "base version");
    assert_eq!(input.read_b().unwrap(), enabled, "enabled");
    input
}

fn read_item_module(input: &mut Cursor<Vec<u8>>, expected: &[(i16, i32)]) {
    let count = input.read_s().unwrap();
    assert_eq!(count as usize, expected.len(), "item count");
    for (item, amount) in expected {
        assert_eq!(input.read_s().unwrap(), *item);
        assert_eq!(input.read_i().unwrap(), *amount);
    }
}

fn read_liquid_module(input: &mut Cursor<Vec<u8>>, expected: &[(i16, f32)]) {
    let count = input.read_s().unwrap();
    assert_eq!(count as usize, expected.len(), "liquid count");
    for (liquid, amount) in expected {
        assert_eq!(input.read_s().unwrap(), *liquid);
        assert!((input.read_f().unwrap() - *amount).abs() < 0.001);
    }
}

fn read_power_module(input: &mut Cursor<Vec<u8>>, links: &[i32], status: f32) {
    let count = input.read_s().unwrap();
    assert_eq!(count as usize, links.len(), "power link count");
    for link in links {
        assert_eq!(input.read_i().unwrap(), *link);
    }
    assert!(
        (input.read_f().unwrap() - status).abs() < 0.001,
        "power status"
    );
}

fn read_base_tail(input: &mut Cursor<Vec<u8>>, efficiency: u8, optional: u8) {
    assert_eq!(input.read_b().unwrap(), efficiency, "efficiency");
    assert_eq!(input.read_b().unwrap(), optional, "optional efficiency");
}

/// Powered blocks with an empty power map serialize 0 efficiency.
fn read_base_tail_powered(input: &mut Cursor<Vec<u8>>) {
    read_base_tail(input, 0, 255);
}

/// Blocks without a power module serialize 1.0 efficiency.
fn read_base_tail_passive(input: &mut Cursor<Vec<u8>>) {
    read_base_tail(input, 255, 255);
}

fn assert_consumed(input: &mut Cursor<Vec<u8>>) {
    assert_eq!(
        input.position() as usize,
        input.get_ref().len(),
        "snapshot has leftover bytes: {}",
        input.get_ref().len() - input.position() as usize
    );
}

// ---------------------------------------------------------------------------
// Simple walls and base-only families
// ---------------------------------------------------------------------------

#[test]
fn wall_snapshot_is_base_building_only() {
    for block in [216, 217, 230, 234, 235, 242] {
        let mut input = decode(encode(block, |_| {}));
        // Official moduleBitmask() always sets 1<<3 (8), even with no modules.
        assert_eq!(input.read_b().unwrap(), 8, "module bits");
        read_base_tail(&mut input, 255, 255);
        assert_consumed(&mut input);
    }
}

#[test]
fn heat_conductor_snapshot_is_base_only() {
    for block in [206, 207, 208] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 8);
        read_base_tail(&mut input, 255, 255);
        assert_consumed(&mut input);
    }
}

#[test]
fn thruster_and_power_diode_snapshot_are_base_only() {
    for block in [234, 305] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 8);
        read_base_tail(&mut input, 255, 255);
        assert_consumed(&mut input);
    }
}

#[test]
fn incinerator_and_repair_tower_carry_power_and_liquids() {
    for block in [198, 397] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 2 | 4 | 8, "module bits");
        read_power_module(&mut input, &[], 0.0);
        read_liquid_module(&mut input, &[]);
        read_base_tail_powered(&mut input);
        assert_consumed(&mut input);
    }
}

#[test]
fn slag_incinerator_carries_liquids_only() {
    let mut input = decode(encode(209, |_| {}));
    assert_eq!(input.read_b().unwrap(), 4 | 8);
    read_liquid_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_consumed(&mut input);
}

#[test]
fn overflow_duct_carries_items_only() {
    for block in [275, 276] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 1 | 8);
        read_item_module(&mut input, &[]);
        read_base_tail(&mut input, 255, 255);
        assert_consumed(&mut input);
    }
}

#[test]
fn duct_bridge_modules_follow_block_flags() {
    // 277 duct-bridge: items; 298 reinforced-bridge-conduit: liquids.
    let mut input = decode(encode(277, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 8);
    read_item_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_consumed(&mut input);

    let mut input = decode(encode(298, |_| {}));
    assert_eq!(input.read_b().unwrap(), 4 | 8);
    read_liquid_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_consumed(&mut input);
}

#[test]
fn reinforced_liquid_blocks_carry_liquids_only() {
    for block in [295, 296, 297, 299, 300, 301] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 4);
        read_liquid_module(&mut input, &[]);
        read_base_tail(&mut input, 255, 255);
        assert_consumed(&mut input);
    }
}

#[test]
fn liquid_tank_snapshot_preserves_amount_and_type() {
    // P1-13: LiquidModule.write is (count, (id, amount)*). A filled Serpulo
    // tank/container must publish that pair so the client does not roll back
    // to empty or swap the liquid type.
    for (block, liquid, amount) in [(290, 0i16, 690.0f32), (291, 2, 1_234.5)] {
        let mut input = decode(encode(block, |t| {
            t.stored_liquid = liquid;
            t.liquid_amount = amount;
        }));
        assert_eq!(input.read_b().unwrap(), 4, "LiquidModule only");
        read_liquid_module(&mut input, &[(liquid, amount)]);
        read_base_tail(&mut input, 255, 255);
        assert_consumed(&mut input);
    }
}

// ---------------------------------------------------------------------------
// Doors, shields, lights
// ---------------------------------------------------------------------------

#[test]
fn door_snapshot_appends_open_flag() {
    for block in [228, 229, 239] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 8);
        read_base_tail(&mut input, 255, 255);
        assert!(!input.read_bool().unwrap(), "closed door");
        assert_consumed(&mut input);

        let mut input = decode(encode(block, |t| t.door_open = true));
        assert_eq!(input.read_b().unwrap(), 8);
        read_base_tail(&mut input, 255, 255);
        assert!(input.read_bool().unwrap(), "open door");
        assert_consumed(&mut input);
    }
}

#[test]
fn shield_wall_snapshot_appends_shield_float() {
    let mut input = decode(encode(244, |t| t.shield = 350.0));
    assert_eq!(input.read_b().unwrap(), 2 | 8, "power module");
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert!((input.read_f().unwrap() - 350.0).abs() < 0.001);
    assert_consumed(&mut input);
}

#[test]
fn light_snapshot_appends_color_int() {
    let mut input = decode(encode(419, |t| t.light_color = 0x11223344));
    assert_eq!(input.read_b().unwrap(), 2 | 8);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_i().unwrap(), 0x11223344);
    assert_consumed(&mut input);
}

// ---------------------------------------------------------------------------
// Logistics
// ---------------------------------------------------------------------------

#[test]
fn conveyor_snapshot_matches_conveyor_build_write() {
    // Each queued item carries its own position along the belt (official
    // ConveyorBuild keeps ids/xs/ys arrays and animates them locally).
    let mut input = decode(encode(257, |t| {
        t.conveyor_items = vec![(5, 0.5), (5, 0.1)];
        t.stored_item = 5;
        t.stored_amount = 2;
        t.transport_progress = 0.5;
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 8, "item module");
    read_item_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_i().unwrap(), 2, "len");
    let y = |progress: f32| (progress * 255.0 - 128.0) as i8 as u8;
    assert_eq!(input.read_s().unwrap(), 5, "item id");
    assert_eq!(input.read_b().unwrap(), 0, "x offset");
    assert_eq!(
        input.read_b().unwrap(),
        y(0.1),
        "official array starts at the logical rear"
    );
    assert_eq!(input.read_s().unwrap(), 5, "item id");
    assert_eq!(input.read_b().unwrap(), 0, "x offset");
    assert_eq!(
        input.read_b().unwrap(),
        y(0.5),
        "official ids[len-1] is the logical front"
    );
    assert_consumed(&mut input);
}

#[test]
fn conveyor_snapshot_repairs_legacy_overflow_and_caps_official_capacity() {
    let mut input = decode(encode(257, |t| {
        t.conveyor_items = vec![(5, 21_086.43), (6, 21_085.29), (7, 21_085.23), (8, 500.0)];
        t.stored_item = 5;
        t.stored_amount = 4;
        t.transport_progress = 21_086.43;
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 8, "item module");
    read_item_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_i().unwrap(), 3, "official conveyor capacity");

    let y = |progress: f32| (progress * 255.0 - 128.0) as i8 as u8;
    for (item, progress) in [(7, 0.2), (6, 0.6), (5, 1.0)] {
        assert_eq!(input.read_s().unwrap(), item);
        assert_eq!(input.read_b().unwrap(), 0, "x offset");
        assert_eq!(input.read_b().unwrap(), y(progress), "healed y offset");
    }
    assert_consumed(&mut input);
}

#[test]
fn stack_conveyor_snapshot_matches_stack_conveyor_build_write() {
    // plastanium conveyor: items only. A far-out-of-range link yields -1.
    let mut input = decode(encode(259, |t| {
        t.config = vec![7, 0, 0, 0, 0, 0, 0x04, 0xd2, 0]
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 8);
    read_item_module(&mut input, &[]);
    read_base_tail_passive(&mut input);
    assert_eq!(
        input.read_i().unwrap(),
        -1,
        "no link configured for zero offset"
    );
    assert_eq!(input.read_f().unwrap(), 0.0, "cooldown");
    assert_consumed(&mut input);

    // Round 74: the stack IS the ItemModule — conveyor_items serialize as
    // the item module (the legacy port wrote the empty `inventory` field,
    // so the client-side plastanium belt looked empty).
    let mut input = decode(encode(259, |t| {
        t.conveyor_items = vec![(5, 0.0); 3];
        t.stored_item = 5;
        t.stored_amount = 3;
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 8);
    read_item_module(&mut input, &[(5, 3)]);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_i().unwrap(), -1);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_consumed(&mut input);

    // surge conveyor: items + power.
    let mut input = decode(encode(279, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_i().unwrap(), -1);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_consumed(&mut input);
}

#[test]
fn duct_snapshot_matches_duct_build_write() {
    let mut input = decode(encode(272, |t| t.duct_rec_dir = 3));
    assert_eq!(input.read_b().unwrap(), 1 | 8);
    read_item_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_b().unwrap(), 3, "recDir");
    assert_consumed(&mut input);
}

#[test]
fn duct_router_snapshot_matches_duct_router_build_write() {
    let mut input = decode(encode(274, |t| t.config = vec![5, 0, 0, 9]));
    assert_eq!(input.read_b().unwrap(), 1 | 8);
    read_item_module(&mut input, &[]);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_s().unwrap(), 9, "sortItem silicon");
    assert_consumed(&mut input);

    // surge router: items + power.
    let mut input = decode(encode(280, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_s().unwrap(), -1, "unconfigured sortItem");
    assert_consumed(&mut input);
}

#[test]
fn directional_unloader_snapshot_matches_build_write() {
    let mut input = decode(encode(278, |t| {
        t.config = vec![5, 0, 0, 3];
        t.unloader_offset = 4;
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 8);
    read_item_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_s().unwrap(), 3, "unloadItem graphite");
    assert_eq!(input.read_s().unwrap(), 4, "offset");
    assert_consumed(&mut input);
}

#[test]
fn unit_cargo_unload_point_snapshot_matches_build_write() {
    let mut input = decode(encode(282, |t| t.config = vec![5, 0, 0, 0]));
    assert_eq!(input.read_b().unwrap(), 1 | 8);
    read_item_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_s().unwrap(), 0, "item copper");
    assert!(!input.read_bool().unwrap(), "stale");
    assert_consumed(&mut input);
}

// ---------------------------------------------------------------------------
// Production
// ---------------------------------------------------------------------------

#[test]
fn drill_snapshots_follow_block_module_flags() {
    // mechanical drill: items + liquids, no power.
    let mut input = decode(encode(325, |t| {
        t.production_progress = 123.0;
        t.transport_progress = 0.25;
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 4 | 8, "items + liquids");
    read_item_module(&mut input, &[]);
    read_liquid_module(&mut input, &[]);
    read_base_tail_passive(&mut input);
    assert!((input.read_f().unwrap() - 123.0).abs() < 0.001, "progress");
    assert!((input.read_f().unwrap() - 0.25).abs() < 0.001, "warmup");
    assert_consumed(&mut input);

    // laser drill: items + power + liquids.
    let mut input = decode(encode(327, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0, "progress");
    assert_eq!(input.read_f().unwrap(), 0.0, "warmup");
    assert_consumed(&mut input);

    // beam drill and burst drill share the drill layout.
    for block in [335, 338] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
        read_item_module(&mut input, &[]);
        read_power_module(&mut input, &[], 0.0);
        read_liquid_module(&mut input, &[]);
        read_base_tail_powered(&mut input);
        assert_eq!(input.read_f().unwrap(), 0.0, "progress");
        assert_eq!(input.read_f().unwrap(), 0.0, "warmup");
        assert_consumed(&mut input);
    }
}

#[test]
fn separator_snapshot_appends_progress_warmup_seed() {
    for block in [193, 194] {
        let mut input = decode(encode(block, |t| {
            t.production_progress = 30.0;
            t.transport_progress = 0.75;
        }));
        assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
        read_item_module(&mut input, &[]);
        read_power_module(&mut input, &[], 0.0);
        read_liquid_module(&mut input, &[]);
        read_base_tail_powered(&mut input);
        assert!((input.read_f().unwrap() - 30.0).abs() < 0.001);
        assert!((input.read_f().unwrap() - 0.75).abs() < 0.001);
        assert_eq!(input.read_i().unwrap(), 0, "seed");
        assert_consumed(&mut input);
    }
}

#[test]
fn wall_crafter_modules_follow_block_flags() {
    // cliff crusher: items + power; large cliff crusher: items + power + liquids.
    let mut input = decode(encode(333, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert_consumed(&mut input);

    let mut input = decode(encode(334, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert_consumed(&mut input);
}

#[test]
fn heat_producer_snapshot_appends_heat_after_warmup() {
    // HeatProducerBuild.write = GenericCrafter (progress, warmup) + heat.
    // Module flags per block: 202 items+power+liquids, 203 items+power,
    // 204 items+liquids, 205 items, 215 items+liquids.
    for (block, has_power, has_liquids) in [
        (202, true, true),
        (203, true, false),
        (204, false, true),
        (205, false, false),
        (215, false, true),
    ] {
        let mut input = decode(encode(block, |t| {
            t.production_progress = 40.0;
            t.output_liquid_amount = 0.5;
        }));
        let bits = input.read_b().unwrap();
        assert_eq!(
            bits,
            1 | (u8::from(has_power) << 1) | (u8::from(has_liquids) << 2) | 8,
            "module bits"
        );
        read_item_module(&mut input, &[]);
        if has_power {
            read_power_module(&mut input, &[], 0.0);
        }
        if has_liquids {
            // output_liquid_amount is exposed through the LiquidModule as
            // cryofluid (id 3) until the client consumes it.
            read_liquid_module(&mut input, &[(3, 0.5)]);
        }
        if has_power {
            read_base_tail_powered(&mut input);
        } else {
            read_base_tail_passive(&mut input);
        }
        let craft_time = if block == 202 { 120.0 } else { 80.0 };
        assert!(
            (input.read_f().unwrap() - 40.0 / craft_time).abs() < 0.001,
            "progress"
        );
        let warmup = if has_power { 0.0 } else { 1.0 };
        assert!((input.read_f().unwrap() - warmup).abs() < 0.001, "warmup");
        assert!((input.read_f().unwrap() - 0.5).abs() < 0.001, "heat");
        assert_consumed(&mut input);
    }
}

#[test]
fn cultivator_snapshot_has_legacy_read_warmup_float() {
    let mut input = decode(encode(330, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0, "progress");
    assert_eq!(input.read_f().unwrap(), 0.0, "warmup");
    assert_eq!(input.read_f().unwrap(), 0.0, "legacyReadWarmup");
    assert_consumed(&mut input);
}

#[test]
fn liquid_factory_snapshots_use_generic_crafter_layout() {
    for block in [192, 195, 197, 211] {
        let mut input = decode(encode(block, |_| {}));
        let bits = input.read_b().unwrap();
        assert!(bits & 1 == 1, "item module present");
        read_item_module(&mut input, &[]);
        if bits & 2 != 0 {
            read_power_module(&mut input, &[], 0.0);
        }
        if bits & 4 != 0 {
            read_liquid_module(&mut input, &[]);
        }
        if bits & 2 != 0 {
            read_base_tail_powered(&mut input);
        } else {
            read_base_tail_passive(&mut input);
        }
        // GenericCrafterBuild.write: progress + warmup.
        assert_eq!(input.read_f().unwrap(), 0.0, "progress");
        assert_eq!(input.read_f().unwrap(), 0.0, "warmup");
        assert_consumed(&mut input);
    }
}

// ---------------------------------------------------------------------------
// Power
// ---------------------------------------------------------------------------

#[test]
fn power_node_snapshot_writes_configured_links_in_power_module() {
    // config = TypeIO Point2[] { (2, 3) } relative to tile (45, 100).
    let packed: i32 = (2 << 16) | (3 & 0xffff);
    let mut config = vec![8, 1];
    config.extend_from_slice(&packed.to_be_bytes());
    let mut input = decode(encode(302, |t| t.config = config));
    assert_eq!(input.read_b().unwrap(), 2);
    read_power_module(&mut input, &[((45 + 2) << 16) | (100 + 3)], 0.0);
    read_base_tail_passive(&mut input);
    assert_consumed(&mut input);
}

#[test]
fn power_node_snapshot_single_point2_config() {
    let mut config = vec![7];
    config.extend_from_slice(&(-1i32).to_be_bytes());
    config.extend_from_slice(&2i32.to_be_bytes());
    let mut input = decode(encode(303, |t| t.config = config));
    assert_eq!(input.read_b().unwrap(), 2);
    read_power_module(&mut input, &[((45 - 1) << 16) | (100 + 2)], 0.0);
    read_base_tail_passive(&mut input);
    assert_consumed(&mut input);
}

#[test]
fn sandbox_sources_serialize_authoritative_selection_and_runtime_state() {
    let linked = (47 << 16) | 100;
    let mut input = decode(encode(410, |t| t.power_links = vec![linked]));
    assert_eq!(input.read_b().unwrap(), 2, "PowerSource has PowerModule");
    read_power_module(&mut input, &[linked], 1.0);
    read_base_tail_passive(&mut input);
    assert_consumed(&mut input);

    let mut input = decode(encode(412, |t| t.config = vec![5, 0, 0, 3]));
    assert_eq!(input.read_b().unwrap(), 1 | 8, "ItemSource modules");
    read_item_module(&mut input, &[]);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_s().unwrap(), 3, "selected graphite");
    assert_consumed(&mut input);

    let mut input = decode(encode(414, |t| {
        t.config = vec![5, 4, 0, 0];
        t.stored_liquid = 0;
        t.liquid_amount = 9_950.0;
    }));
    assert_eq!(input.read_b().unwrap(), 4 | 8, "LiquidSource modules");
    read_liquid_module(&mut input, &[(0, 9_950.0)]);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_s().unwrap(), 0, "selected water");
    assert_consumed(&mut input);

    for block in 410..=415 {
        assert!(is_block_snapshot_supported(block), "sandbox block {block}");
    }
}

#[test]
fn generator_snapshots_follow_block_module_flags() {
    // thermal: power only; rtg: items + power; steam: items + power + liquids.
    let mut input = decode(encode(309, |_| {}));
    assert_eq!(input.read_b().unwrap(), 2 | 8);
    read_power_module(&mut input, &[], 1.0); // producer graph status
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_f().unwrap(), 1.0, "productionEfficiency");
    assert_eq!(input.read_f().unwrap(), 0.0, "generateTime");
    assert_consumed(&mut input);

    let mut input = decode(encode(312, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 1.0);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_consumed(&mut input);

    let mut input = decode(encode(310, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 1.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_consumed(&mut input);
}

#[test]
fn nuclear_and_impact_reactors_append_heat_or_warmup() {
    // NuclearReactorBuild.write: generator fields + heat.
    let mut input = decode(encode(315, |t| {
        t.inventory = vec![(7, 30)];
        t.stored_liquid = 3;
        t.liquid_amount = 2.0;
        t.output_liquid_amount = 0.42;
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[(7, 30)]);
    read_power_module(&mut input, &[], 1.0);
    read_liquid_module(&mut input, &[(3, 2.0)]);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_f().unwrap(), 1.0, "productionEfficiency");
    assert_eq!(input.read_f().unwrap(), 0.0, "generateTime");
    assert!((input.read_f().unwrap() - 0.42).abs() < 0.001, "heat");
    assert_consumed(&mut input);

    // ImpactReactorBuild.write: generator fields + warmup.
    let mut input = decode(encode(316, |t| t.output_liquid_amount = 9.0));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 1.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_passive(&mut input);
    assert_eq!(input.read_f().unwrap(), 1.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert!((input.read_f().unwrap() - 9.0).abs() < 0.001, "warmup");
    assert_consumed(&mut input);
}

#[test]
fn variable_reactor_appends_heat_instability_warmup() {
    let mut input = decode(encode(323, |_| {}));
    assert_eq!(input.read_b().unwrap(), 2 | 4 | 8);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0); // productionEfficiency
    assert_eq!(input.read_f().unwrap(), 0.0); // generateTime
    assert_eq!(input.read_f().unwrap(), 0.0); // heat
    assert_eq!(input.read_f().unwrap(), 0.0); // instability
    assert_eq!(input.read_f().unwrap(), 0.0); // warmup
    assert_consumed(&mut input);
}

#[test]
fn heater_generator_appends_heat_after_generator_fields() {
    let mut input = decode(encode(324, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 0.0); // heat
    assert_consumed(&mut input);
}

// ---------------------------------------------------------------------------
// Cores, turrets, units
// ---------------------------------------------------------------------------

#[test]
fn core_snapshot_writes_item_module_and_null_command_pos() {
    let mut input = decode(encode(339, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1, "item module only");
    read_item_module(&mut input, &[]);
    read_base_tail(&mut input, 255, 255);
    assert!(input.read_f().unwrap().is_nan(), "commandPos x");
    assert!(input.read_f().unwrap().is_nan(), "commandPos y");
    assert_consumed(&mut input);
}

#[test]
fn turret_snapshots_include_liquid_module_per_1581() {
    // Duo: ItemModule + LiquidModule + ammo queue.
    let mut input = decode(encode(349, |t| {
        t.stored_item = 0;
        t.ammo_units = 6.0;
        t.production_progress = 10.0;
    }));
    assert_eq!(input.read_b().unwrap(), 1 | 4 | 8, "items + liquids");
    read_item_module(&mut input, &[]);
    read_liquid_module(&mut input, &[]);
    read_base_tail_passive(&mut input); // item turret efficiency comes from ammo
    assert!(
        (input.read_f().unwrap() - 10.0).abs() < 0.001,
        "reloadCounter"
    );
    assert_eq!(input.read_f().unwrap(), 90.0, "rotation");
    assert_eq!(input.read_b().unwrap(), 1, "ammo.size");
    assert_eq!(input.read_s().unwrap(), 0, "ammo item");
    assert_eq!(input.read_s().unwrap(), 6, "ammo amount");
    assert_consumed(&mut input);

    // Lancer: PowerModule + LiquidModule + reload + rotation.
    let mut input = decode(encode(354, |t| {
        t.production_progress = 20.0;
    }));
    assert_eq!(input.read_b().unwrap(), 2 | 4 | 8, "power + liquids");
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert!((input.read_f().unwrap() - 20.0).abs() < 0.001);
    assert_eq!(input.read_f().unwrap(), 90.0);
    assert_consumed(&mut input);

    // Afflict: PowerModule only.
    let mut input = decode(encode(372, |_| {}));
    assert_eq!(input.read_b().unwrap(), 2 | 8);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0, "reloadCounter");
    assert_eq!(input.read_f().unwrap(), 90.0, "rotation");
    assert_consumed(&mut input);
}

#[test]
fn tractor_beam_and_point_defense_append_rotation() {
    for block in [356, 359, 384] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 2 | 8, "power module");
        read_power_module(&mut input, &[], 0.0);
        read_base_tail_powered(&mut input);
        // Building rotation: tile helper uses rotation 2 -> 180°.
        assert_eq!(input.read_f().unwrap(), 180.0, "rotation");
        assert_consumed(&mut input);
    }
    // repair turret adds liquids.
    let mut input = decode(encode(385, |_| {}));
    assert_eq!(input.read_b().unwrap(), 2 | 4 | 8);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 180.0);
    assert_consumed(&mut input);
}

#[test]
fn unit_assembler_snapshot_matches_build_write() {
    let mut input = decode(encode(393, |t| t.production_progress = 55.0));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    // PayloadBlockBuild.write
    assert_eq!(input.read_f().unwrap(), 0.0, "payVector.x");
    assert_eq!(input.read_f().unwrap(), 0.0, "payVector.y");
    assert_eq!(input.read_f().unwrap(), 0.0, "payRotation");
    assert!(!input.read_bool().unwrap(), "payload");
    // UnitAssemblerBuild.write
    assert!((input.read_f().unwrap() - 55.0).abs() < 0.001, "progress");
    assert_eq!(input.read_b().unwrap(), 0, "units.size");
    assert_eq!(input.read_s().unwrap(), 0, "PayloadSeq size");
    assert!(input.read_f().unwrap().is_nan(), "commandPos x");
    assert!(input.read_f().unwrap().is_nan(), "commandPos y");
    assert_consumed(&mut input);
}

#[test]
fn assembler_module_snapshot_matches_payload_block_write() {
    let mut input = decode(encode(396, |_| {}));
    assert_eq!(input.read_b().unwrap(), 2 | 8);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert!(!input.read_bool().unwrap());
    assert_consumed(&mut input);
}

#[test]
fn reconstructor_prime_refabricator_uses_revision_3_layout() {
    // moduleBitmask(): items|power|liquids|1<<3 = 15 (verified via javap on
    // desktop.jar ReconstructorBuild/UnitBuild/PayloadBlockBuild.write).
    let mut input = decode(encode(392, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    // PayloadBlockBuild.write (rotation 2 => payRotation 180 degrees)
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 180.0);
    assert!(!input.read_bool().unwrap());
    // ReconstructorBuild.write: progress + commandPos + command
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert!(input.read_f().unwrap().is_nan());
    assert!(input.read_f().unwrap().is_nan());
    assert_eq!(input.read_b().unwrap(), 255, "null command");
    assert_consumed(&mut input);
}

#[test]
fn reconstructor_command_lands_in_command_slot_not_optional_efficiency() {
    // A reconstructor configured with a unit command must serialize the
    // command id in the final writeCommand byte and keep optionalEfficiency
    // at its real value. (Regression: the command leaked into the
    // optionalEfficiency slot and the final byte was always 255.)
    let mut input = decode(encode(380, |t| t.config = vec![23, 0, 3])); // assist id 3
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 8, "module bits");
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 0.0);
    assert_eq!(input.read_f().unwrap(), 180.0, "payRotation (rotation 2)");
    assert!(!input.read_bool().unwrap());
    assert_eq!(input.read_f().unwrap(), 0.0, "progress");
    assert!(input.read_f().unwrap().is_nan());
    assert!(input.read_f().unwrap().is_nan());
    assert_eq!(input.read_b().unwrap(), 3, "command id must be assist(3)");
    assert_consumed(&mut input);
}

// ---------------------------------------------------------------------------
// Logic and campaign
// ---------------------------------------------------------------------------

#[test]
fn logic_processor_snapshots_match_logic_build_write() {
    // micro processor: no modules; hyper processor adds liquids.
    for (block, bits) in [(431, 8), (432, 8), (433, 12)] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), bits, "module bits");
        if bits & 4 != 0 {
            read_liquid_module(&mut input, &[]);
        }
        read_base_tail(&mut input, 255, 255);
        // compressed program length + payload
        let compressed_len = input.read_i().unwrap();
        assert!(compressed_len > 0 && compressed_len < 1024);
        let mut compressed = vec![0u8; compressed_len as usize];
        input.read_exact(&mut compressed).unwrap();
        // variables, memory, [privileged ipt], tag, iconTag, waits, accumulator
        assert_eq!(input.read_i().unwrap(), 0, "variables");
        assert_eq!(input.read_i().unwrap(), 0, "memory");
        // Official LogicBuild.write (158.1): only privileged processors
        // serialize instructionsPerTick. Hyper (433) is not privileged and
        // must NOT write the field (VerifyProtocol158 full run).
        if block == 442 {
            assert_eq!(input.read_s().unwrap(), 8, "world-processor ipt");
        }
        assert_eq!(input.read_b().unwrap(), 0, "null tag");
        assert_eq!(input.read_s().unwrap(), 0, "iconTag");
        assert_eq!(input.read_s().unwrap(), 0, "waits");
        assert_eq!(input.read_f().unwrap(), 0.0, "accumulator");
        assert_consumed(&mut input);
    }
}

#[test]
fn world_only_logic_buildings_match_official_subclass_tails() {
    // Blocks.worldProcessor/worldCell/worldMessage/worldSwitch (442-445) are
    // the same Java build classes as their public counterparts, but their
    // IDs were previously falling through to the generator codec. The local
    // 158.1 sources define the exact tails: privileged LogicBuild writes ipt,
    // MemoryBuild writes its full capacity, and Message/Switch append their
    // normal fields. Byte lengths match a desktop.jar 158.1 Building.writeAll
    // fixture (health=40, team=sharded, empty program/memory/message):
    // 442=45, 443=4111, 444=13, 445=12.
    assert_eq!(encode(442, |_| {}).len(), 45);
    assert_eq!(encode(443, |_| {}).len(), 4111);
    assert_eq!(encode(444, |_| {}).len(), 13);
    assert_eq!(encode(445, |_| {}).len(), 12);

    let mut input = decode(encode(442, |_| {}));
    assert_eq!(input.read_b().unwrap(), 8, "world processor module bits");
    read_base_tail(&mut input, 255, 255);
    let compressed_len = input.read_i().unwrap();
    assert!(compressed_len > 0 && compressed_len < 1024);
    let mut compressed = vec![0u8; compressed_len as usize];
    input.read_exact(&mut compressed).unwrap();
    assert_eq!(input.read_i().unwrap(), 0, "variables");
    assert_eq!(input.read_i().unwrap(), 0, "legacy memory");
    assert_eq!(input.read_s().unwrap(), 8, "world processor ipt");
    assert_eq!(input.read_b().unwrap(), 0, "null tag");
    assert_eq!(input.read_s().unwrap(), 0, "iconTag");
    assert_eq!(input.read_s().unwrap(), 0, "waits");
    assert_eq!(input.read_f().unwrap(), 0.0, "accumulator");
    assert_consumed(&mut input);

    let mut input = decode(encode(443, |t| t.memory = vec![1.5, -2.0]));
    assert_eq!(input.read_b().unwrap(), 8, "world cell module bits");
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_i().unwrap(), 512, "world-cell capacity");
    assert_eq!(input.read_l().unwrap(), 1.5f64.to_bits() as i64);
    assert_eq!(input.read_l().unwrap(), (-2.0f64).to_bits() as i64);
    for _ in 2..512 {
        assert_eq!(input.read_l().unwrap(), 0.0f64.to_bits() as i64);
    }
    assert_consumed(&mut input);

    let mut input = decode(encode(444, |t| t.message = Some("world".into())));
    assert_eq!(input.read_b().unwrap(), 8, "world message module bits");
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_utf().unwrap(), "world");
    assert_consumed(&mut input);

    let mut input = decode_base_manual(encode(445, |t| t.enabled = false), 0);
    assert_eq!(input.read_b().unwrap(), 8, "world switch module bits");
    // SwitchBuild writes enabled in the base header and appends it again.
    read_base_tail(&mut input, 255, 255);
    assert!(!input.read_bool().unwrap(), "world switch disabled");
    assert_consumed(&mut input);

    for block in 442..=445 {
        assert!(is_block_snapshot_supported(block), "world block {block}");
    }
}

#[test]
fn logic_processor_passes_client_program_through_verbatim() {
    // Build the exact LogicBlock.compress output: zlib of
    // [1, int sourceLen, source, int linkCount, links...].
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut container = Vec::new();
    container.push(1); // container version
    container.extend_from_slice(&3i32.to_be_bytes()); // source len
    container.extend_from_slice(b"abc"); // source
    container.extend_from_slice(&0i32.to_be_bytes()); // link count
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&container).unwrap();
    let program = encoder.finish().unwrap();
    let mut typeio_object = vec![14]; // TypeIO byte[]
    typeio_object.extend_from_slice(&(program.len() as i32).to_be_bytes());
    typeio_object.extend_from_slice(&program);

    assert!(valid_logic_config(&program));
    assert!(!valid_logic_config(&[1, 2, 3, 4, 5])); // not zlib
    assert!(!valid_logic_config(&[0])); // null sentinel is handled by the handler

    for block in [431, 432, 433] {
        // DynamicTile stores the complete TileConfig TypeIO object. The
        // snapshot must extract its byte[] payload instead of attempting to
        // inflate the leading object tag as zlib data.
        let mut input = decode(encode(block, |t| t.config = typeio_object.clone()));
        let expected_bits = if block == 433 { 12 } else { 8 };
        assert_eq!(input.read_b().unwrap(), expected_bits, "module bits");
        if expected_bits & 4 != 0 {
            read_liquid_module(&mut input, &[]);
        }
        read_base_tail(&mut input, 255, 255);
        let compressed_len = input.read_i().unwrap() as usize;
        assert_eq!(compressed_len, program.len(), "program length");
        let mut received = vec![0u8; compressed_len];
        input.read_exact(&mut received).unwrap();
        assert_eq!(received, program, "program bytes verbatim");
        assert_eq!(input.read_i().unwrap(), 0, "variables");
        assert_eq!(input.read_i().unwrap(), 0, "memory");
        if block == 442 {
            assert_eq!(input.read_s().unwrap(), 8, "world-processor ipt");
        }
        assert_eq!(input.read_b().unwrap(), 0, "null tag");
        assert_eq!(input.read_s().unwrap(), 0, "iconTag");
        assert_eq!(input.read_s().unwrap(), 0, "waits");
        assert_eq!(input.read_f().unwrap(), 0.0, "accumulator");
        assert_consumed(&mut input);
    }
}

#[test]
fn memory_snapshot_writes_cell_count_and_values() {
    // MemoryBuild.write always emits the FULL capacity (64 for memory-cell,
    // 512 for memory-bank): `i memory.length + f64*length`. Unwritten cells
    // are zeros (regression: empty tile.memory used to emit length 0, so the
    // client built a 0-length memory and all logic read/write was a no-op).
    let mut input = decode(encode(434, |t| t.memory = vec![1.5, -2.0]));
    assert_eq!(input.read_b().unwrap(), 8);
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_i().unwrap(), 64, "memory-cell capacity");
    assert_eq!(input.read_l().unwrap(), 1.5f64.to_bits() as i64);
    assert_eq!(input.read_l().unwrap(), (-2.0f64).to_bits() as i64);
    for _ in 2..64 {
        assert_eq!(input.read_l().unwrap(), 0.0f64.to_bits() as i64);
    }
    assert_consumed(&mut input);

    let mut input = decode(encode(435, |t| t.memory = vec![7.0]));
    assert_eq!(input.read_b().unwrap(), 8);
    read_base_tail(&mut input, 255, 255);
    assert_eq!(input.read_i().unwrap(), 512, "memory-bank capacity");
    assert_eq!(input.read_l().unwrap(), 7.0f64.to_bits() as i64);
    for _ in 1..512 {
        assert_eq!(input.read_l().unwrap(), 0.0f64.to_bits() as i64);
    }
    assert_consumed(&mut input);
}

#[test]
fn logic_display_and_canvas_snapshots_match_build_write() {
    for block in [436, 437, 438] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 8);
        read_base_tail(&mut input, 255, 255);
        assert!(!input.read_bool().unwrap(), "no transform matrix");
        assert_consumed(&mut input);
    }
    for block in [439, 440] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 8);
        read_base_tail(&mut input, 255, 255);
        assert_eq!(input.read_i().unwrap(), 0, "canvas data length");
        assert_consumed(&mut input);
    }
}

#[test]
fn message_and_switch_snapshots_match_build_write() {
    for block in [429, 441] {
        let mut input = decode(encode(block, |_| {}));
        assert_eq!(input.read_b().unwrap(), 8);
        read_base_tail(&mut input, 255, 255);
        assert_eq!(input.read_utf().unwrap(), "", "message string");
        assert_consumed(&mut input);
    }
    let mut input = decode(encode(430, |_| {}));
    assert_eq!(input.read_b().unwrap(), 8);
    read_base_tail(&mut input, 255, 255);
    assert!(input.read_bool().unwrap(), "switch enabled");
    assert_consumed(&mut input);

    // A toggled-off switch serializes the disabled state in the base byte
    // and the trailing bool (SwitchBlock.java write/readBase share `enabled`).
    let mut input = decode_base_manual(encode(430, |t| t.enabled = false), 0);
    assert_eq!(input.read_b().unwrap(), 8, "no modules");
    read_base_tail(&mut input, 255, 255);
    assert!(!input.read_bool().unwrap(), "switch disabled");
    assert_consumed(&mut input);
}

#[test]
fn base_shield_projectors_snapshot_as_base_with_power_module() {
    // BaseShieldBuild (shield-projector 255, large 256) DOES override
    // Building.write (BaseShield.java): base + PowerModule + efficiency
    // bytes, then `f smoothRadius` + `bool broken`. Verified against the
    // desktop.jar 158.1 (22-byte layout). Omitting the tail corrupts the
    // client's sequential block-snapshot batch.
    for block in [255, 256] {
        let mut input = decode(encode(block, |_| {}));
        // moduleBitmask(): power (2) | 1<<3 always-on (8).
        assert_eq!(input.read_b().unwrap(), 2 | 8, "power module bits");
        read_power_module(&mut input, &[], 0.0);
        read_base_tail_powered(&mut input);
        // smoothRadius = radius * efficiency (0 with no power); broken=false.
        assert!((input.read_f().unwrap() - 0.0).abs() < 0.001);
        assert_eq!(input.read_b().unwrap(), 0, "broken flag");
        assert_consumed(&mut input);
    }
}

#[test]
fn radar_snapshot_appends_progress() {
    let mut input = decode(encode(251, |t| t.production_progress = 7.5));
    assert_eq!(input.read_b().unwrap(), 2 | 8);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert!((input.read_f().unwrap() - 7.5).abs() < 0.001);
    assert_consumed(&mut input);
}

#[test]
fn launch_pad_and_accelerator_snapshots_match_build_write() {
    let mut input = decode(encode(425, |t| t.production_progress = 12.0));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert!(
        (input.read_f().unwrap() - 12.0).abs() < 0.001,
        "launchCounter"
    );
    assert_consumed(&mut input);

    let mut input = decode(encode(426, |_| {}));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 4 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_liquid_module(&mut input, &[]);
    read_base_tail_powered(&mut input);
    assert_eq!(input.read_f().unwrap(), 0.0, "launchCounter");
    assert_consumed(&mut input);

    let mut input = decode(encode(428, |t| t.production_progress = 3.0));
    assert_eq!(input.read_b().unwrap(), 1 | 2 | 8);
    read_item_module(&mut input, &[]);
    read_power_module(&mut input, &[], 0.0);
    read_base_tail_powered(&mut input);
    assert!((input.read_f().unwrap() - 3.0).abs() < 0.001, "progress");
    assert_consumed(&mut input);
}

// ---------------------------------------------------------------------------
// Coverage: every constructible block has a codec
// ---------------------------------------------------------------------------

#[test]
fn every_constructible_block_has_a_snapshot_codec() {
    // Block IDs with a build cost (block_requirements.tsv). All 245 must be
    // accepted by is_block_snapshot_supported.
    let constructible = [
        181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, 192, 193, 194, 195, 196, 197, 198,
        199, 200, 201, 202, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 215, 216,
        217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234,
        235, 236, 237, 238, 239, 240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252,
        253, 254, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271, 272,
        273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287, 288, 289, 290,
        291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303, 304, 305, 306, 307, 308,
        309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321, 322, 323, 324, 325, 326,
        327, 328, 329, 330, 331, 332, 333, 334, 335, 336, 337, 338, 339, 340, 341, 342, 343, 344,
        345, 346, 347, 348, 349, 350, 351, 352, 353, 354, 355, 356, 357, 358, 359, 360, 361, 362,
        363, 364, 365, 366, 367, 368, 369, 370, 371, 372, 373, 374, 375, 376, 377, 378, 379, 380,
        381, 382, 383, 384, 385, 386, 387, 388, 389, 390, 391, 392, 393, 394, 395, 396, 397, 398,
        399, 400, 401, 402, 403, 404, 405, 406, 407, 408, 409, 419, 425, 426, 427, 428, 429, 430,
        431, 432, 433, 434, 435, 436, 437, 438, 439, 440, 441,
    ];
    assert_eq!(constructible.len(), 245);
    let missing: Vec<i16> = constructible
        .iter()
        .copied()
        // Cores (339-344) have a codec (encode_core_sync) but are excluded
        // from the periodic batch (BlockFlag.synced); the individual
        // RequestBlockSnapshot reply still covers them.
        .filter(|block| !is_block_snapshot_supported(*block) && !is_core_block(*block))
        .collect();
    assert!(
        missing.is_empty(),
        "blocks without a snapshot codec: {missing:?}"
    );
}

#[test]
fn memory_bank_snapshot_fits_arcnet_frame_limit() {
    // A memory-bank (435) serializes 512 f64 cells (~4.1 KB per snapshot).
    // The official client reads block snapshots sequentially and the ArcNet
    // frame is capped at 32 KiB; a batch that exceeds it is silently dropped.
    // The full per-tile payload (pos + block + writeSync) must stay far below
    // 32 KiB so batching (official maxSnapshotSize=800 B) never overflows.
    use std::io::Read;
    let mut input = decode(encode(435, |t| {
        t.memory = vec![0.0; 512];
        t.memory[0] = 1.0;
        t.memory[511] = -1.0;
    }));
    assert_eq!(input.read_b().unwrap(), 8);
    read_base_tail(&mut input, 255, 255);
    let cells = input.read_i().unwrap();
    assert_eq!(cells, 512);
    let mut buf = vec![0u8; 512 * 8];
    input.read_exact(&mut buf).unwrap();
    assert_eq!(buf[0..8], 1.0f64.to_bits().to_be_bytes());
    assert_eq!(buf[512 * 8 - 8..], (-1.0f64).to_bits().to_be_bytes());
    assert_consumed(&mut input);
    // pos(4) + block(2) + base header(9) + i(4) + 4096 payload: the per-tile
    // payload (~4.1 KB) is one order of magnitude below the 32 KiB ArcNet
    // frame cap, so byte-based batching (maxSnapshotSize=800 B) can never
    // overflow a frame even with several memory banks per batch.
}
