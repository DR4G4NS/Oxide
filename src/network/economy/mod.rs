//! Economy simulation: logistics, liquids, factories, reconstructors,
//! menders, projectors, power network.

use crate::network::buildings::construction::{block_footprint, dynamic_at};
use crate::network::buildings::snapshot::{
    dynamic_tile_health, encode_conveyor_sync, encode_dynamic_tile_sync,
    is_batch_snapshot_supported,
};
use crate::network::codec::Writes;
use crate::network::combat::enemy::{apply_enemy_support_abilities, damage_building};
use crate::network::combat::unit_combat::effective_unit_speed;
use crate::network::combat::*;
use crate::network::decoders::apply_set_unit_command;
use crate::network::protocol::*;
use crate::network::simulation::{simulate_assist_units, simulate_builder_units};
use crate::network::units::mining::heal_building_for_team;
use crate::network::units::*;
use crate::network::wire::client_snapshot::raw_mine_result;
use crate::network::wire::encode::{
    encode_build_destroyed_frame, encode_build_health_update_frame, encode_unit_spawn_payload,
    frame_generated_packet,
};
use crate::network::wire::tile_config::{configured_unit_command, unit_factory_plan};
use crate::network::world::*;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::io::Error;
use std::sync::atomic::Ordering;
use tracing::{debug, info, warn};

/// Official `Conveyor.itemSpace` and `Conveyor.capacity` (158.1).
pub(crate) const CONVEYOR_ITEM_SPACE: f32 = 0.4;
const CONVEYOR_CAPACITY: usize = 3;

/// Memory cell (434) / memory bank (435) capacity, from Blocks.java
/// (memory-cell memoryCapacity=64, memory-bank memoryCapacity=512, size 2).
pub(crate) mod inventory;
pub(crate) use inventory::{has_requirements, items_for_team, items_for_team_mut, TeamItemsMut};
pub(crate) mod spec;
pub(crate) use spec::{
    accept_logistics_item, accept_logistics_item_from, angle_near, configured_item,
    configured_link, dominant_drill_ore, drill_parameters, dump_drill_item, factory_recipe,
    generator_fuel, incoming_bridge_sources, inventory_add, inventory_count, inventory_remove,
    inventory_total, is_supported_item_turret, item_transport_capacity, item_transport_speed,
    liquid_turret_weapon, mass_driver_state, move_toward_angle, offset_position,
    power_turret_weapon, route_instant_item, simulate_junctions, simulate_reactors,
    simulate_unloaders, storage_capacity, storage_linked_to_core, turret_ammo, turret_can_target,
    turret_max_ammo, turret_shots, turret_target_allowed, unit_type_is_flying, valid_bridge_link,
    valid_mass_driver_link, FactoryRecipe, TurretAmmo, MONO_MINE_RANGE,
};

pub(crate) mod payload;
pub(crate) use payload::{
    apply_request_build_payload, apply_request_drop_payload, apply_request_unit_payload,
    building_can_pickup, building_center, carried_payload_requirements,
    choose_payload_router_rotation, constructor_item_capacity, decode_constructor_recipe,
    drop_carried_build, drop_carried_build_at, dump_deconstructor_items,
    encode_payload_dropped_frame, encode_picked_build_payload_frame,
    encode_picked_unit_payload_frame, insert_into_payload_conveyor, offset_position_by,
    payload_block_accepts, payload_block_limit, payload_capacity, payload_carrier,
    payload_conveyor_move_time, payload_fits_limit, payload_unit_drop_jitter, payload_used,
    payload_used_of, player_is_dead, refresh_build_payload_sync, transfer_payload_forward,
    valid_payload_mass_driver_link, LARGE_CONSTRUCTOR_RECIPES, NEW_LARGE_CODEC_RECIPES,
    SMALL_CONSTRUCTOR_RECIPES,
};

mod transport;
pub(crate) use transport::{
    accept_plain_conveyor_item, aligned_conveyor_next_max, conveyor_accept_minitem,
    conveyor_facing_edge, conveyor_rotates, conveyor_side_insert_index, drill_liquid_boost,
    is_plain_conveyor, normalized_conveyor_items, sanitize_conveyor_queue, simulate_base_drills,
    simulate_base_factories, simulate_logistics, tile_relative_to,
};

mod liquids;
pub(crate) use liquids::{
    accept_liquid, accept_liquid_from, aligned_direction, base_block_at, dump_liquid_offer,
    floor_liquid_drop, is_conduit, is_liquid_bridge, is_liquid_junction, is_liquid_router, is_pump,
    liquid_can_output, liquid_capacity, liquid_production, memory_capacity, puddle_spread_passable,
    pump_floor_liquid, resolve_liquid_destination, same_liquid_amount, simulate_liquids,
    simulate_puddle_tile_effects, tile_block_at, tile_has_building, unit_receives_puddle_effect,
};

mod factories;
pub(crate) use factories::MenderSpec;
pub(crate) use factories::{
    can_create_unit, core_unit_modifier, default_unit_command, dump_factory_output,
    encode_unit_despawn_frame_legacy, encode_unit_entered_payload_frame, liquid_factory_recipe,
    reconstructor_item_capacity, reconstructor_recipe, reconstructor_upgrade, separator_spec,
    sharded_unit_cap, simulate_factories, simulate_liquid_factories, simulate_reconstructors,
    simulate_separators, simulate_unit_factories, simulate_unit_payload_entries,
    spawn_factory_unit, team_unit_cap, unit_factory_item_capacity, unit_factory_recipe,
};

mod support;
pub(crate) use support::PowerRole;
pub(crate) use support::{
    absorb_enemy_projectile, angles_within, block_can_emp_boost, block_is_suppressable,
    building_heal_suppressed, building_time_scale, can_receive_overdrive, force_broken, lerp_delta,
    mender_spec, oct_force_field_absorb, overdrive_spec, projectile_building_hit,
    projectile_in_tecta_arc, projectile_position, quasar_force_field_absorb,
    segment_intersects_circle, set_force_broken, simulate_force_projectors, simulate_menders,
    simulate_navanax_suppression, simulate_oct_force_fields, simulate_overdrives,
    simulate_regen_projectors, simulate_shock_mines, simulate_shockwave_towers,
    simulate_tecta_shield_arcs, tecta_arc_origin, tecta_shield_arc_absorb,
};
pub(crate) use support::{
    CYANOGEN_LIQUID, FORCE_PROJECTOR_BLOCK, HYDROGEN_LIQUID, REGEN_PROJECTOR_BLOCK,
    SHOCKWAVE_TOWER_BLOCK, SHOCK_MINE_BLOCK,
};

mod power;
pub(crate) use power::{
    active_cycle_item, apply_power_diode_transfers, beam_east_target, beam_node_spec,
    beam_nodes_connected, beam_target, calculate_power_network, compute_power_efficiency,
    continuous_liquid_efficiency, economy_is_power_node, effective_power_role,
    floor_power_attribute, generator_efficiency, item_flammability, item_radioactivity,
    output_item_fits, power_component_at, power_connected, power_graph_components, power_role,
    power_role_vertices, refresh_beam_power_links, should_consume_power, snapshot_for_component,
    stored_liquid_amount, update_power_network,
};

mod erekir;
pub(crate) use erekir::{
    assembler_plan, assembler_tier, beam_drill_facing, configured_item_local, deliver_item_to,
    duct_accept_item, duct_bridge_input_occupied, duct_bridge_link, duct_output_targets,
    duct_speed, duct_store_item, dump_erekir_drill, erekir_drill_spec, erekir_heat_at,
    erekir_liquid_turret_ammo, erekir_offset_by, erekir_power_turret_weapon,
    erekir_turret_accept_ammo, erekir_turret_ammo_spec, erekir_turret_params,
    erekir_turret_pull_ammo, heat_apply_craft, heat_block_spec, heat_inputs_available,
    heat_value_at, is_erekir_conveyor_block, is_erekir_duct_block, is_erekir_turret_block,
    module_tier, peek_unloader_item, ready_source_item, relative_direction, remove_source_item,
    simulate_erekir_assemblers, simulate_erekir_crafters, simulate_erekir_drills,
    simulate_erekir_ducts, simulate_erekir_turrets, simulate_heat_network, take_unloader_item,
    tiles_adjacent, wall_ore_drop, HeatKind,
};

#[cfg(test)]
mod tests;
