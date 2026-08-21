//! Building-domain services.
//!
//! The original port accumulated placement, TypeIO configuration and every
//! building simulation inside `network::listener`.  These modules are the
//! compatibility boundary between the wire-oriented `DynamicTile` model and
//! typed building behaviour.  Keeping that boundary explicit is important:
//! Java's `Object config`, a compressed logic payload and a serialized save
//! tail are all byte arrays, but they are not interchangeable protocols.

pub(crate) mod construction;
#[cfg(test)]
mod construction_tests;
pub(crate) use construction::{
    add_team_plan, apply_build_plans, base_block, base_origin, block_footprint, block_footprint_in,
    consume_requirements, consume_requirements_for, consume_requirements_impl, dynamic_at,
    effective_block, effective_building_team, encode_begin_break, encode_begin_place,
    encode_begin_place_for_unit, encode_deconstruct_finish, finish_pending_break,
    finish_pending_build, live_team_building_count, network_template_with_plans,
    placement_footprint_is_replaceable, refund_requirements, refund_requirements_for,
    refund_requirements_impl, remove_team_plan, remove_team_plan_from, schedule_break,
    schedule_build, simulate_breaks, simulate_constructions, ModeRestream,
};
pub(crate) mod plans;
pub(crate) use plans::{
    assist_visual_plan, pause_player_build_queue, rebuild_plan, sync_unit_build_plans, AssistTarget,
};
pub(crate) mod config;
pub mod placement;
pub mod power;
pub mod puddles;
pub mod reactor;
pub mod sandbox;
pub mod snapshot;
