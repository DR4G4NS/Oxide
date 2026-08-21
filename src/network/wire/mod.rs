//! Wire frame-format layer. Domain code builds official 158.1 frame layouts
//! here; the listener/session adapters own socket delivery.

pub(crate) mod encode;
pub(crate) use encode::{
    batch_block_snapshot_entries, block_snapshot_requires_world, coalesce_build_health,
    encode_block_snapshot, encode_block_snapshot_entry, encode_block_snapshots,
    encode_block_snapshots_with_threshold, encode_build_destroyed_frame,
    encode_build_health_update_frame, encode_construct_block_snapshot, encode_debug_status_client,
    encode_enemy_entity_snapshots, encode_initial_entity_snapshot, encode_player_disconnect_frames,
    encode_state_snapshot, encode_state_snapshot_for, encode_unit_spawn_payload,
    finish_block_snapshot_batch, frame_generated_packet, max_synced_plans, state_snapshot_teams,
    take_coalesced_build_health, write_puddle_sync, write_unit_plans_queue, write_unit_sync,
    ENEMY_SNAPSHOT_BATCH_BYTES, PUDDLE_ENTITY_CLASS_ID,
};

pub(crate) mod auth;
pub(crate) use auth::{
    actor_action_allowed, player_team, rejected_action_aftermath, session_action_allowed,
    session_action_allowed_full,
};

pub(crate) mod transfer;
pub(crate) use transfer::{
    broadcast_player_snapshot, broadcast_respawn, deposit_player_inventory,
    encode_take_items_frame, encode_transfer_item_to_frame, enemy_weapon_mount_count,
    item_storage_target, nearest_opposing_unit, player_can_transfer, respawn_session_player,
    withdraw_items_to_player, ItemStorageTarget,
};
pub(crate) mod tile_config;
pub(crate) use tile_config::{
    apply_rotate_block, apply_tile_config, broadcast_placement_power_configs,
    configured_unit_command, encode_rotate_block_frame, encode_tile_config_broadcast,
    encode_tile_config_frame, unit_factory_plan, valid_tile_config,
};
pub(crate) mod client_snapshot;
pub(crate) use client_snapshot::{
    apply_client_snapshot, apply_client_snapshot_with_speed, apply_controlled_client_snapshot,
    mine_result, raw_mine_result, update_mining,
};
pub(crate) mod unit_control;
pub(crate) use unit_control::{
    apply_unit_control, building_control_select_allowed, encode_unit_building_control_select_frame,
    encode_unit_despawn_frame, encode_unit_reference, is_controllable_block,
    request_block_snapshot_target, resolve_controllable_building, unit_control_allowed,
    SnapshotTarget,
};
pub(crate) mod bootstrap;
pub(crate) use bootstrap::{
    apply_game_mode_to_wave_rules, apply_wave_rules_overrides, assign_team_for_join,
    emit_game_over_packet, emit_game_over_packet_with_winner, enforce_strict_spawn_groups,
    extend_attack_spawns, extend_attack_spawns_for_team, fresh_world_from_template,
    fresh_world_from_template_for_mode, host_map, mode_transition_rules, network_building_tile,
    parse_team_id, resolve_host_map, HostMapResult, HostMapSource, HOST_MAP_EVENTS,
};
pub(crate) mod persistence;
pub(crate) use persistence::{
    apply_loaded_team_cores, apply_loaded_team_items, decode_typeio_string,
    encode_construct_finish, encode_construct_finish_for_unit, encode_typeio_string, load_tiles,
    outbound_typeio_object, persist_tiles, persist_world_sync, sanitize_standalone_payload,
    sanitize_unit_payloads, snapshot_persisted_world, valid_build_position, PersistJob,
    PersistenceWorker,
};

pub(crate) mod outbound;
pub(crate) use outbound::{
    BLOCK_SNAPSHOT_INTERVAL, HEALTH_SYNC_INTERVAL, MAX_SNAPSHOT_SIZE, SLOW_CONSUMER_DROP_LIMIT,
};

pub use outbound::{ChatRateLimiter, OUTBOUND_QUEUE_CAPACITY};
