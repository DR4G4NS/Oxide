use crate::network::codec::{read_packet, write_tcp_packet, FRAMEWORK_MESSAGE_ID, MAX_PACKET_SIZE};
use crate::network::packets::Packet;
use crate::state::administration::Administration;
use crate::state::game_state::{GameMode, GameState};
use dashmap::DashMap;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::{HashMap, HashSet};
use std::io::{Error, ErrorKind};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tracing::{debug, error, info, warn};

use crate::network::buildings::{
    config as building_config, placement as building_placement, power as power_nodes, reactor,
    sandbox::SandboxSystem,
};
// P2: block snapshot codecs live in the snapshot domain; re-export the
// public entry points so existing callers and integration tests keep
// working unchanged.
pub use crate::network::buildings::snapshot::{
    angle_between, broadcast_plan_snapshot_team, build_payload_version, dynamic_tile_health,
    encode_accelerator_sync, encode_base_only_with_items_or_liquids_sync,
    encode_basic_modules_sync, encode_battery_sync, encode_build_tower_sync,
    encode_campaign_pad_sync, encode_canvas_sync, encode_conveyor_sync, encode_core_sync,
    encode_directional_unloader_sync, encode_door_sync, encode_drill_sync, encode_duct_router_sync,
    encode_duct_sync, encode_dynamic_tile_sync, encode_factory_snapshot_tiles,
    encode_force_projector_sync, encode_generic_crafter_sync, encode_heat_source_sync,
    encode_heater_generator_sync, encode_item_bridge_sync, encode_item_logistics_sync,
    encode_item_source_sync, encode_junction_sync, encode_launch_pad_sync, encode_light_sync,
    encode_liquid_block_sync, encode_liquid_bridge_sync, encode_liquid_only_base_sync,
    encode_liquid_source_sync, encode_logic_display_sync, encode_logic_processor_sync,
    encode_mass_driver_sync, encode_memory_sync, encode_mender_sync, encode_message_sync,
    encode_overdrive_sync, encode_payload_block_base_sync, encode_payload_build_sync,
    encode_payload_constructor_sync, encode_payload_conveyor_sync,
    encode_payload_deconstructor_sync, encode_payload_loader_sync, encode_payload_mass_driver_sync,
    encode_power_generator_sync, encode_power_liquid_base_sync, encode_power_node_sync,
    encode_radar_sync, encode_reconstructor_sync, encode_regen_projector_sync,
    encode_separator_sync, encode_shield_sync, encode_shield_wall_sync, encode_shock_mine_sync,
    encode_shockwave_tower_sync, encode_simple_items_sync, encode_simple_power_sync,
    encode_simple_wall_sync, encode_stack_conveyor_sync, encode_storage_sync, encode_switch_sync,
    encode_turret_rotation_sync, encode_turret_sync, encode_unit_assembler_sync,
    encode_unit_cargo_loader_sync, encode_unit_cargo_unload_point_sync, encode_unit_factory_sync,
    encode_variable_reactor_sync, encode_wall_crafter_sync, generic_crafter_time,
    is_batch_snapshot_supported, is_block_snapshot_supported, is_core_block,
    is_pickup_payload_supported, is_simple_liquid_snapshot, is_snapshot_item_turret,
    power_node_links, turret_snapshot_rotation, valid_logic_config, write_liquid_module,
    write_primary_liquid_module,
};
use crate::network::combat::*;
pub use crate::network::decoders::{
    apply_command_building, apply_command_building_for_team, apply_command_units,
    apply_command_units_for_team, apply_set_unit_command, apply_set_unit_command_for_team,
    apply_set_unit_stance, apply_set_unit_stance_for_team, command_resets_target, command_target,
    decode_admin_request, decode_building_control_select, decode_building_reference,
    decode_client_logic_data, decode_client_plan_snapshot, decode_client_snapshot,
    decode_command_building, decode_command_units, decode_delete_plans, decode_drop_item,
    decode_ping_location, decode_request_build_payload, decode_request_drop_payload,
    decode_request_item, decode_request_unit_payload, decode_rotate_block, decode_server_packet,
    decode_set_player_team_editor, decode_set_unit_command, decode_set_unit_stance,
    decode_tile_config, decode_unit_command_config, decode_unit_control,
    encode_client_plan_snapshot_received, encode_command_building_frame,
    encode_command_units_frame, encode_delete_plans_forward, encode_ping_location_forward,
    encode_set_unit_command_frame, encode_set_unit_stance_frame, encode_trace_info_frame,
    encode_unit_control_forward, read_typeio_object_raw, skip_typeio_object, unit_allows_command,
    unit_allows_stance, BuildPlan, ClientSnapshot, CommandUnitsRequest,
};
use crate::network::economy::*;
use crate::network::protocol::*;
pub use crate::network::runtime::{save_slot_path, spawn_runtime_commands};
pub use crate::network::session::send_state_snapshot;
pub use crate::network::session::{
    encode_all_player_snapshots, handle_tcp, read_frame, replay_dynamic_tiles,
    replay_pending_breaks, replay_pending_builds, send_all_player_snapshots, send_generated_packet,
    send_generated_packet_prefer_udp, send_player_spawn, send_world_stream, spawn_team_projectile,
    update_player_combat, world_stream_frames,
};
pub use crate::network::simulation::{
    simulate_aegires_energy_fields, simulate_allied_oxynoe_repair, simulate_allied_units,
    simulate_assist_units, simulate_builder_units, simulate_enemy_point_defense,
    simulate_enemy_statuses, simulate_generators, simulate_impact_reactors, simulate_logic,
    simulate_logic_build, simulate_logic_fire, simulate_logic_mining, simulate_mass_drivers,
    simulate_mono_mining, simulate_payload_carriers, simulate_payload_constructors,
    simulate_payload_conveyors, simulate_payload_deconstructors, simulate_payload_loaders,
    simulate_payload_mass_drivers, simulate_pvp_player_damage, simulate_reactors_with_network,
    simulate_support_units, simulate_unit_collisions, simulate_unit_elevation,
    simulate_waves_and_enemies, simulation_delta_for_tps, simulation_delta_from_elapsed,
    spawn_world_simulation, update_pvp_auto_pause,
};
use crate::network::units::*;
use crate::network::world::*;

pub(crate) use crate::network::wire::bootstrap::{
    apply_game_mode_to_wave_rules, apply_wave_rules_overrides, assign_team_for_join,
    emit_game_over_packet, emit_game_over_packet_with_winner, enforce_strict_spawn_groups,
    extend_attack_spawns, extend_attack_spawns_for_team, fresh_world_from_template_for_mode,
    mode_transition_rules, network_building_tile, parse_team_id,
};
pub use crate::network::wire::bootstrap::{
    fresh_world_from_template, host_map, resolve_host_map, HostMapResult, HostMapSource,
    HOST_MAP_EVENTS,
};

pub(crate) use crate::network::wire::persistence::{
    apply_loaded_team_cores, apply_loaded_team_items, encode_construct_finish,
    encode_construct_finish_for_unit, outbound_typeio_object, persist_world_sync,
    sanitize_standalone_payload, sanitize_unit_payloads, snapshot_persisted_world,
    valid_build_position, CorePersistenceSource, PersistJob, PersistenceWorker,
};
pub use crate::network::wire::persistence::{
    decode_typeio_string, encode_typeio_string, load_tiles, persist_tiles,
};

pub(crate) use crate::network::outbound::{
    broadcast, broadcast_except, enqueue_outbound, enqueue_outbound_routed,
};
pub use crate::network::wire::encode::encode_state_snapshot;
pub(crate) use crate::network::wire::encode::{
    batch_block_snapshot_entries, block_snapshot_requires_world, coalesce_build_health,
    encode_block_snapshot, encode_block_snapshot_entry, encode_block_snapshots,
    encode_block_snapshots_with_threshold, encode_build_destroyed_frame,
    encode_build_health_update_frame, encode_construct_block_snapshot, encode_debug_status_client,
    encode_enemy_entity_snapshots, encode_initial_entity_snapshot, encode_player_disconnect_frames,
    encode_state_snapshot_for, encode_unit_spawn_payload, finish_block_snapshot_batch,
    frame_generated_packet, max_synced_plans, state_snapshot_teams, take_coalesced_build_health,
    write_puddle_sync, write_unit_plans_queue, write_unit_sync, ENEMY_SNAPSHOT_BATCH_BYTES,
    PUDDLE_ENTITY_CLASS_ID,
};
pub use crate::network::wire::outbound::{ChatRateLimiter, OUTBOUND_QUEUE_CAPACITY};
pub(crate) use crate::network::wire::outbound::{
    BLOCK_SNAPSHOT_INTERVAL, HEALTH_SYNC_INTERVAL, MAX_SNAPSHOT_SIZE, SLOW_CONSUMER_DROP_LIMIT,
};

pub(crate) use crate::network::wire::unit_control::{
    apply_unit_control, building_control_select_allowed, encode_unit_building_control_select_frame,
    encode_unit_despawn_frame, encode_unit_reference, is_controllable_block,
    request_block_snapshot_target, resolve_controllable_building, unit_control_allowed,
    SnapshotTarget,
};

pub(crate) use crate::network::wire::client_snapshot::{
    apply_client_snapshot, apply_controlled_client_snapshot, mine_result, raw_mine_result,
    update_mining,
};

pub(crate) use crate::network::wire::auth::{
    actor_action_allowed, player_team, rejected_action_aftermath, session_action_allowed,
    session_action_allowed_full,
};

pub(crate) use crate::network::wire::transfer::{
    broadcast_player_snapshot, broadcast_respawn, deposit_player_inventory,
    encode_take_items_frame, encode_transfer_item_to_frame, enemy_weapon_mount_count,
    item_storage_target, nearest_opposing_unit, player_can_transfer, respawn_session_player,
    withdraw_items_to_player, ItemStorageTarget,
};

pub(crate) use crate::network::wire::tile_config::{
    apply_rotate_block, apply_tile_config, broadcast_placement_power_configs,
    configured_unit_command, encode_rotate_block_frame, encode_tile_config_broadcast,
    encode_tile_config_frame, unit_factory_plan, valid_tile_config,
};

// M4 Stage A: extracted domain modules. The listener adapter re-exports
// them so existing `crate::network::listener::<sym>` callers are unchanged.
pub(crate) use crate::network::economy::{
    has_requirements, items_for_team, items_for_team_mut, TeamItemsMut,
};

/// Official `NetServer.blockSyncTime`. Clients simulate conveyor/drill visuals
/// locally between these correction snapshots; forcing every block back to
/// authoritative state each second causes the visible one-second snapping
/// Official `NetServer.healthSyncTime` (0.5 s). Damaged buildings are queued
/// Official `Administration.Config.snapshotInterval` default is **200 ms**.
/// This server sends entity/state snapshots every 50 ms (20 Hz) so units and
/// conveyors interpolate without the stutter seen at vanilla 5 Hz. Desktop
/// 158.1 accepts the higher rate; the client UDP timeout is 20 s.
pub(crate) const SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
pub(crate) const OFFICIAL_SNAPSHOT_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(200);
/// Arc `TcpConnection.keepAliveMillis`.
pub(crate) const TCP_KEEPALIVE: std::time::Duration = std::time::Duration::from_millis(8000);
/// Arc `TcpConnection.timeoutMillis`.
pub(crate) const TCP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(12000);
/// Arc `UdpConnection.keepAliveMillis`.
pub(crate) const UDP_KEEPALIVE: std::time::Duration = std::time::Duration::from_millis(19000);
/// `Administration.Config.packetSpamLimit` default and `Ratekeeper` window
/// used by `ArcNetProvider.received` (`allow(3000, packetSpamLimit)`).
pub(crate) const PACKET_SPAM_WINDOW_MS: u64 = 3000;
pub(crate) const PACKET_SPAM_LIMIT: u32 = 300;

use crate::network::buildings::construction::{
    add_team_plan, apply_build_plans, block_footprint, block_footprint_in, consume_requirements,
    consume_requirements_for, dynamic_at, encode_begin_place_for_unit, finish_pending_build,
    network_template_with_plans, refund_requirements, remove_team_plan, remove_team_plan_from,
    schedule_break, schedule_build, simulate_breaks, simulate_constructions,
};

pub struct NetworkListener {
    pub port: u16,
    pub state: GameState,
    pub admin: Administration,
    pub server_info: ServerInfo,
    pub save_path: PathBuf,
    pub map_path: Option<PathBuf>,
    /// Target world simulation TPS (official 60). The world loop runs at
    /// this rate with delta = 60/tps game ticks per step (SOL-005: --tps
    /// previously drove only the legacy empty TickEngine, not the world).
    pub tps: u32,
    control: NetworkControl,
    control_receiver:
        parking_lot::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<RuntimeCommand>>>,
}

impl NetworkListener {
    pub fn new(port: u16, state: GameState, admin: Administration) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            port,
            state,
            admin,
            server_info: ServerInfo::default(),
            save_path: PathBuf::from("world-delta.json"),
            map_path: None,
            tps: 10,
            control: NetworkControl { sender },
            control_receiver: parking_lot::Mutex::new(Some(receiver)),
        }
    }

    pub fn control(&self) -> NetworkControl {
        self.control.clone()
    }

    pub fn with_save_path(mut self, save_path: PathBuf) -> Self {
        self.save_path = save_path;
        self
    }

    pub fn with_map_path(mut self, map_path: Option<PathBuf>) -> Self {
        self.map_path = map_path;
        self
    }

    /// Sets the target world simulation TPS (official 60). The world loop
    /// steps at this rate with delta = 60/tps game ticks per tick.
    pub fn with_tps(mut self, tps: u32) -> Self {
        self.tps = tps.max(1);
        self
    }

    pub fn with_server_info(
        mut self,
        name: String,
        description: String,
        build: i32,
        version_type: String,
    ) -> Self {
        self.server_info = ServerInfo {
            name,
            description,
            build,
            version_type,
        };
        self
    }

    /// Starts TCP and UDP on the same port. UDP is mandatory in ArcNet: clients
    /// do not send their ConnectPacket until RegisterUDP has completed.
    pub async fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        let addr = format!("0.0.0.0:{}", self.port);
        let tcp = TcpListener::bind(&addr).await?;
        let udp = UdpSocket::bind(&addr).await?;
        let std_sock = udp.into_std()?;
        std_sock.set_nonblocking(true)?;
        let tokio_std = std_sock.try_clone()?;
        let std_udp = Arc::new(std_sock);
        let udp = Arc::new(UdpSocket::from_std(tokio_std)?);
        info!("ArcNet TCP/UDP server listening on {}", addr);

        let connections = Arc::new(DashMap::<i32, PendingConnection>::new());
        let (network_template, map_name) = if let Some(path) = &self.map_path {
            let msav = std::fs::read(path)?;
            let template = crate::engine::world_stream::replace_map_from_msav(
                include_bytes!("../../dummy_world.dat"),
                &msav,
            )?;
            let meta = crate::engine::save_io::SaveIO::read_meta(msav.as_slice())?;
            let configured_name = self.state.map_name.read().clone();
            let map_name = if configured_name != "maze" {
                configured_name
            } else {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("custom")
                    .to_owned()
            };
            info!(
                "Loaded official MSAV map {} ({}x{})",
                path.display(),
                meta.width,
                meta.height
            );
            (template, map_name)
        } else {
            (
                include_bytes!("../../dummy_world.dat").to_vec(),
                self.state.map_name.read().clone(),
            )
        };
        *self.state.map_name.write() = map_name.clone();
        let world = fresh_world_from_template(
            &self.state,
            network_template,
            map_name,
            self.save_path.clone(),
        )?;
        let width = world.width;
        let height = world.height;
        let loaded = load_tiles(&self.save_path, Some((width, height)))?;
        let active_map_name = self.state.map_name.read().clone();
        if let Some(saved_map_name) = &loaded.map_name {
            if saved_map_name != &active_map_name {
                return Err(Box::new(Error::new(
                    ErrorKind::InvalidData,
                    format!(
                        "save belongs to map '{saved_map_name}', selected map is '{active_map_name}'"
                    ),
                )));
            }
        } else if self.map_path.is_some() && self.save_path.exists() {
            return Err(Box::new(Error::new(
                ErrorKind::InvalidData,
                "legacy save has no map identity; use a new --save-file for the selected MSAV",
            )));
        }
        apply_loaded_team_cores(&world, &loaded);
        apply_loaded_team_items(&world, &loaded);
        if let Some(simulation_time) = loaded.simulation_time {
            *self.state.simulation_time.write() = simulation_time;
        }
        for (name, value) in &loaded.logic_flags {
            world.logic_flags.insert(name.clone(), *value);
        }
        // Authoritative puddles (revision 14, round 73): restored with their
        // stable entity ids so clients that rejoin see the same puddle
        // entities; the allocator advances past the loaded ids.
        for puddle in &loaded.puddles {
            world.puddles.restore(
                puddle.position,
                crate::network::buildings::puddles::PuddleState {
                    entity_id: puddle.entity_id,
                    liquid: puddle.liquid,
                    amount: puddle.amount,
                    accepting: 0.0,
                    spread_mask: 0b1111,
                    update_time: 0.0,
                },
            );
        }
        *world.game_state.game_stats.write() = loaded.game_stats;
        if let Some(items) = loaded.core_items {
            *self.state.core_items.write() = items;
        }
        if let Some(wave) = loaded.wave {
            self.state.wave.store(wave, Ordering::Relaxed);
        }
        if let Some(wave_time) = loaded.wave_time {
            *self.state.wave_time.write() = wave_time;
        }
        if let Some(core_health) = loaded.core_health {
            *self.state.core_health.write() = core_health;
        }
        // Game-over is ephemeral runtime state (like the official server,
        // which does not serialize state.gameOver): a restart always boots
        // a live game. The persisted `game_over` field (if present in old
        // saves) is deliberately ignored on load.
        for enemy in loaded.enemies {
            world.next_enemy_id.store(
                world
                    .next_enemy_id
                    .load(Ordering::Relaxed)
                    .max(enemy.id.saturating_add(1)),
                Ordering::Relaxed,
            );
            let enemy_id = enemy.id;
            world.enemies.insert(enemy_id, enemy);
            world.register_unit_group(enemy_id);
        }
        self.state
            .enemies_count
            .store(hostile_unit_count(&world), Ordering::Relaxed);
        for player in loaded.players {
            world.player_profiles.insert(player.uuid.clone(), player);
        }
        for command in loaded.building_commands {
            world.building_commands.insert(command.position, command);
        }
        for order in loaded.unit_orders {
            world.unit_orders.insert(order.unit_id, order);
        }
        // P0-01: checkpoints written before the authority field existed load
        // units as DefaultAi; persisted ACTIVE RTS commands must keep Command
        // authority. Default orders without targets stay at the unit default.
        let rts_commanded: Vec<i32> = world
            .unit_orders
            .iter()
            .filter(|order| unit_order_has_active_rts_target(order.value()))
            .map(|order| *order.key())
            .collect();
        for unit_id in rts_commanded {
            if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
                if unit.authority == UnitAuthority::DefaultAi {
                    unit.authority = UnitAuthority::Command;
                }
            }
        }
        if loaded.core_health.is_none() {
            *self.state.core_health.write() = world.core_max_health;
        }
        let saved_base_health: HashMap<_, _> = loaded
            .base_building_health
            .iter()
            .map(|entry| (entry.position, entry.health))
            .collect();
        for template in &world.base_building_templates {
            let destroyed = loaded
                .tiles
                .get(&template.position)
                .is_some_and(|tile| tile.block == 0);
            if !destroyed {
                let maximum = crate::game::content::block_health(template.block);
                let health = saved_base_health
                    .get(&template.position)
                    .copied()
                    .filter(|health| (0.0..=maximum).contains(health))
                    .unwrap_or(template.health)
                    .clamp(f32::MIN_POSITIVE, maximum);
                world.base_buildings.insert(
                    template.position,
                    BaseBuildingState {
                        health,
                        inventory: Vec::new(),
                        ..template.clone()
                    },
                );
            }
        }
        // Saved dynamic entries override the map template, while untouched map
        // buildings (including their decoded modules) remain present.
        for entry in loaded.tiles.iter() {
            world.tiles.insert(*entry.key(), entry.value().clone());
        }
        // Saved links/configs may come from an older server revision or a map
        // whose reverse PowerModule edge was incomplete. Repair them before
        // the simulation or the first client can observe the world.
        power_nodes::normalize_power_links(&world);
        if crate::network::core_inventory::clamp_core_inventories(&world) {
            info!("Clamped persisted core inventory to the loaded core topology");
            world.persistence_dirty.store(true, Ordering::Relaxed);
        }
        *world.team_build_plans.write() = loaded.team_build_plans;
        // Pending construction progress is intentionally transient and is not
        // present in the checkpoint. Its persisted TeamPlans half cannot be
        // resumed safely, so discard it instead of publishing immortal ghosts.
        cancel_transient_world_actions(&world);
        let control_receiver = self.control_receiver.lock().take().ok_or_else(|| {
            Error::new(ErrorKind::AlreadyExists, "network listener already started")
        })?;
        let store = WorldStore::new(world);
        spawn_runtime_commands(
            store.clone(),
            connections.clone(),
            control_receiver,
            self.admin.clone(),
        );
        spawn_world_simulation(
            store.clone(),
            connections.clone(),
            self.tps,
            self.admin.clone(),
        );
        self.spawn_udp(udp.clone(), connections.clone());
        self.spawn_tcp(tcp, connections, store, udp, std_udp);
        Ok(())
    }

    /// Kept for callers of the initial API. Unlike the old implementation this
    /// starts the required UDP endpoint too.
    pub async fn start_tcp_listener(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.start().await
    }

    fn spawn_udp(&self, socket: Arc<UdpSocket>, connections: Arc<DashMap<i32, PendingConnection>>) {
        let state = self.state.clone();
        let admin = self.admin.clone();
        let info = self.server_info.clone();
        let port = self.port;
        tokio::spawn(async move {
            let mut buf = [0u8; 16_384];
            loop {
                let (len, source) = match socket.recv_from(&mut buf).await {
                    Ok(value) => value,
                    Err(err) => {
                        error!("UDP receive error: {}", err);
                        continue;
                    }
                };

                if len >= 2 && buf[0] as i8 == FRAMEWORK_MESSAGE_ID {
                    match buf[1] {
                        DISCOVER_HOST => {
                            let response = encode_server_info(&info, &state, &admin, port);
                            if let Err(err) = socket.send_to(&response, source).await {
                                warn!(
                                    "Could not answer discovery request from {}: {}",
                                    source, err
                                );
                            }
                        }
                        REGISTER_UDP if len == FRAMEWORK_PACKET_LEN => {
                            let id = i32::from_be_bytes([buf[2], buf[3], buf[4], buf[5]]);
                            match apply_register_udp(&connections, id, source) {
                                RegisterUdpOutcome::Bound => {
                                    info!(
                                        "Registered UDP endpoint {} for connection {}",
                                        source, id
                                    );
                                }
                                RegisterUdpOutcome::AlreadyBound => {}
                                RegisterUdpOutcome::UnknownId | RegisterUdpOutcome::IpMismatch => {
                                    warn!(
                                        "Rejected invalid UDP registration {} from {}",
                                        id, source
                                    );
                                }
                            }
                        }
                        REGISTER_UDP => {
                            warn!(
                                "Rejected malformed UDP registration from {}: expected {} bytes, received {}",
                                source, FRAMEWORK_PACKET_LEN, len
                            );
                        }
                        _ => {}
                    }
                } else if !route_inbound_udp(&connections, source, &buf[..len]) {
                    // Unregistered addresses are ignored (ArcNet drops UDP
                    // that does not match udpRemoteAddress).
                }
            }
        });
    }

    fn spawn_tcp(
        &self,
        listener: TcpListener,
        connections: Arc<DashMap<i32, PendingConnection>>,
        store: WorldStore,
        udp_socket: Arc<UdpSocket>,
        std_udp: Arc<std::net::UdpSocket>,
    ) {
        let state = self.state.clone();
        let admin = self.admin.clone();
        tokio::spawn(async move {
            loop {
                let (socket, peer) = match listener.accept().await {
                    Ok(value) => value,
                    Err(err) => {
                        error!("TCP accept error: {}", err);
                        continue;
                    }
                };

                let ip = peer.ip().to_string();
                if admin.is_dos_blacklisted(&ip) || admin.banned_ips.contains(&ip) {
                    warn!("Rejected banned client {}", peer);
                    continue;
                }
                if connection_limit_blocks_accept(admin.get_player_limit(), connections.len()) {
                    warn!("Rejected {} because the connection limit was reached", peer);
                    continue;
                }

                let id = allocate_connection_id(&connections);
                // P0-9: bounded outbound queues. A slow consumer fills the
                // queue, drops are counted, and the connection is torn down
                // past the drop limit instead of growing without bound.
                let (outbound_tx, outbound_rx) =
                    tokio::sync::mpsc::channel(OUTBOUND_QUEUE_CAPACITY);
                let (udp_inbound_tx, udp_inbound_rx) = tokio::sync::mpsc::unbounded_channel();
                connections.insert(
                    id,
                    PendingConnection {
                        ip: peer.ip(),
                        outbound: outbound_tx,
                        udp_inbound: udp_inbound_tx,
                        udp_endpoint: Arc::new(parking_lot::RwLock::new(None)),
                        udp_socket: Some(std_udp.clone()),
                        player_name: Arc::new(parking_lot::RwLock::new(None)),
                        outbound_drops: Arc::new(AtomicU64::new(0)),
                        critical_drops: Arc::new(AtomicU64::new(0)),
                        last_keepalive_rtt_ms: Arc::new(AtomicU64::new(0)),
                        last_packet_epoch_ms: Arc::new(AtomicU64::new(0)),
                        outbound_queued: Arc::new(AtomicU64::new(0)),
                    },
                );
                state.players_count.fetch_add(1, Ordering::Relaxed);
                let task_connections = connections.clone();
                let task_state = state.clone();
                let task_admin = admin.clone();
                let teardown_admin = admin.clone();
                let task_store = store.clone();
                let task_udp = udp_socket.clone();
                tokio::spawn(async move {
                    info!("TCP client {} assigned ArcNet connection {}", peer, id);
                    if let Err(err) = handle_tcp(
                        socket,
                        peer,
                        id,
                        task_admin,
                        outbound_rx,
                        udp_inbound_rx,
                        task_connections.clone(),
                        task_store.clone(),
                        task_udp,
                    )
                    .await
                    {
                        if err.kind() == ErrorKind::UnexpectedEof {
                            info!("Connection {} ({}) closed by peer: {}", id, peer, err);
                        } else {
                            warn!("Connection {} ({}) closed: {}", id, peer, err);
                        }
                    }
                    task_connections.remove(&id);
                    // Resolve the current world so a hot-swapped map is cleaned
                    // against its own player tables.
                    let task_world = task_store.load();
                    let player_id = 1_000_000i32.wrapping_add(id);
                    let unit_id = task_world
                        .player_sessions
                        .iter()
                        .find_map(|session| (session.id == player_id).then_some(*session.key()))
                        .unwrap_or(2_000_000i32.wrapping_add(id));
                    if let Some((_, mut session)) = task_world.player_sessions.remove(&unit_id) {
                        if let Ok(frames) = encode_player_disconnect_frames(&session) {
                            for frame in frames {
                                broadcast(&task_connections, frame);
                            }
                        }
                        // Console registry: forget the disconnected player.
                        teardown_admin.unregister_connection(&session.uuid);
                        // PlayerComp.remove() calls clearUnit(), including the
                        // exact incoming-save/old-restore transition.
                        if matches!(session.controlled_unit, ControlledUnit::Standard(_)) {
                            crate::network::buildings::plans::pause_player_build_queue(
                                &task_world,
                                &session,
                            );
                            crate::network::units::switch_player_unit(
                                &task_world,
                                &mut session,
                                None,
                            );
                        } else {
                            crate::network::buildings::plans::pause_player_build_queue(
                                &task_world,
                                &session,
                            );
                        }
                    }
                    if task_world.players.remove(&unit_id).is_some() {
                        task_world.persistence_dirty.store(true, Ordering::Relaxed);
                    }
                    task_state.players_count.fetch_sub(1, Ordering::Relaxed);
                });
            }
        });
    }
}

/// Arc `Server.generateId`: `Rand.nextInt()` until the id is unused in
/// pending/active connections. 0 is skipped so RegisterTCP never looks like
/// an unset connectionID (the 158.1 client treats that as the handshake id).
pub(crate) fn allocate_connection_id(connections: &DashMap<i32, PendingConnection>) -> i32 {
    loop {
        let id: i32 = rand::random();
        if id != 0 && !connections.contains_key(&id) {
            return id;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegisterUdpOutcome {
    Bound,
    AlreadyBound,
    UnknownId,
    IpMismatch,
}

/// Arc `Server` RegisterUDP: bind `udpRemoteAddress` once, confirm over TCP
/// with a default `RegisterUDP` (connectionID 0, matching `new RegisterUDP()`),
/// and refuse unknown or already-bound ids without creating a second session.
pub(crate) fn apply_register_udp(
    connections: &DashMap<i32, PendingConnection>,
    connection_id: i32,
    source: SocketAddr,
) -> RegisterUdpOutcome {
    let Some(connection) = connections.get(&connection_id) else {
        return RegisterUdpOutcome::UnknownId;
    };
    if connection.ip != source.ip() {
        return RegisterUdpOutcome::IpMismatch;
    }
    {
        let mut endpoint = connection.udp_endpoint.write();
        if endpoint.is_some() {
            return RegisterUdpOutcome::AlreadyBound;
        }
        *endpoint = Some(source);
    }
    enqueue_outbound(
        &connection,
        framework_registration(REGISTER_UDP, 0).to_vec(),
        true,
    );
    RegisterUdpOutcome::Bound
}

/// ConnectPacket is accepted only after RegisterUDP bound `udpRemoteAddress`.
pub(crate) fn udp_handshake_complete(
    connections: &DashMap<i32, PendingConnection>,
    connection_id: i32,
) -> bool {
    connections
        .get(&connection_id)
        .is_some_and(|connection| connection.udp_endpoint.read().is_some())
}

/// Dispatch a non-framework UDP datagram to the connection that registered
/// `source`. Returns false when the address is unknown (ArcNet ignores it).
pub(crate) fn route_inbound_udp(
    connections: &DashMap<i32, PendingConnection>,
    source: SocketAddr,
    datagram: &[u8],
) -> bool {
    let Some(connection) = connections
        .iter()
        .find(|connection| connection.udp_endpoint.read().as_ref() == Some(&source))
    else {
        return false;
    };
    if connection.udp_inbound.send(datagram.to_vec()).is_err() {
        warn!(
            "Could not route UDP packet from {} to its TCP session",
            source
        );
        return false;
    }
    true
}

/// Validates the variable tail of the verified 158.1 TextInputResult call.
/// TextInputResultCallPacket.write emits an int textInputId followed by
/// TypeIO.writeString: one tag byte (0 = null, 1 = present) and, for
/// non-null, a big-endian u16 modified-UTF byte length plus the bytes. This
/// check consumes only the packet-local payload and cannot desynchronize the
/// surrounding ArcNet frame.
fn valid_text_input_result_payload(payload: &[u8]) -> bool {
    if payload.len() < 5 {
        return false;
    }
    match payload[4] {
        0 => payload.len() == 5,
        1 => {
            if payload.len() < 7 {
                return false;
            }
            let text_len = u16::from_be_bytes([payload[5], payload[6]]) as usize;
            payload.len() == 7usize.saturating_add(text_len)
        }
        _ => false,
    }
}

pub(crate) fn valid_client_noop_payload(id: u8, payload: &[u8]) -> bool {
    match id {
        TILE_TAP_PACKET_ID => payload.len() == 4,
        REQUEST_DEBUG_STATUS_PACKET_ID => payload.is_empty(),
        MENU_CHOOSE_PACKET_ID => payload.len() == 8,
        TEXT_INPUT_RESULT_PACKET_ID => valid_text_input_result_payload(payload),
        _ => false,
    }
}

/// P1 (SOL-011): sends one generated frame preferring the client's
/// registered UDP endpoint (the unreliable transport). The UDP datagram
/// uses the same ArcNet `PacketSerializer` layout as TCP minus the TCP
/// length prefix: `[b id][s payload_len][b compress][payload]` (verified
/// against `ArcNetProvider$PacketSerializer.write` and `UdpConnection.send`
/// in desktop 158.1). Falls back to the TCP writer when the endpoint is not
/// registered or the send fails, so a UDP-less client or a lost datagram
/// never loses the frame.
/// Sends a plain formatted message (SendMessageCallPacket 91) to a single
/// player's connection (used for command responses like the official
/// `player.sendMessage(...)`).
pub(crate) fn send_message_to_player(
    connections: &DashMap<i32, PendingConnection>,
    player_id: i32,
    message: &str,
) {
    let Ok(payload) = encode_typeio_string(message) else {
        return;
    };
    let Ok(frame) = frame_generated_packet(SEND_MESSAGE_PACKET_ID, &payload, false) else {
        return;
    };
    // player_id == 1_000_000 + connection_id
    if let Some(connection) = connections.get(&(player_id - 1_000_000)) {
        enqueue_outbound(&connection, frame, false);
    }
}

/// `mindustry.core.NetClient.chat`: rate limit is `chatRate.allow(2000,
/// Config.chatSpamLimit)` — at most chatSpamLimit(2) messages per 2000 ms.
/// Official netServer.clientCommands (NetServer.java registerCommands):
/// `help`, `t` (team chat), `a` (admin chat), `sync`, `votekick`, `vote`.
/// The port implements team chat, admin chat, help, sync (best-effort) and a
/// minimal votekick with cooldown and vote counting.
pub(crate) fn handle_client_command(
    world: &DynamicWorld,
    connections: &DashMap<i32, PendingConnection>,
    player: &mut SessionPlayer,
    message: &str,
    admin: &crate::state::administration::Administration,
) {
    let mut parts = message[1..].splitn(2, ' ');
    let command = parts.next().unwrap_or("").to_ascii_lowercase();
    let args = parts.next().unwrap_or("");
    let admin_flag = player.admin;
    let team = world
        .player_profiles
        .get(&player.uuid)
        .map(|profile| profile.team)
        .unwrap_or(1);
    match command.as_str() {
        "t" | "teamchat" => {
            // Send to players on the same team (PvP).
            let raw = format!(
                "[#2ad4d6]<T> [coral][[{}[coral]]:[white] {}",
                player.name, args
            );
            let Ok(payload) = encode_typeio_string(&raw) else {
                return;
            };
            let Ok(frame) = frame_generated_packet(SEND_MESSAGE_PACKET_ID, &payload, false) else {
                return;
            };
            for entry in connections.iter() {
                let player_id = 1_000_000 + *entry.key();
                let same_team = world
                    .player_profiles
                    .iter()
                    .any(|profile| profile.player_id == player_id && profile.team == team);
                if same_team {
                    enqueue_outbound(entry.value(), frame.clone(), false);
                }
            }
        }
        "a" | "adminchat" => {
            if !admin_flag {
                send_message_to_player(
                    connections,
                    player.id,
                    "[scarlet]You must be an admin to use this command.",
                );
                return;
            }
            let raw = format!(
                "[#ffd37e]<A> [coral][[{}[coral]]:[white] {}",
                player.name, args
            );
            let Ok(payload) = encode_typeio_string(&raw) else {
                return;
            };
            let Ok(frame) = frame_generated_packet(SEND_MESSAGE_PACKET_ID, &payload, false) else {
                return;
            };
            for entry in connections.iter() {
                let player_id = 1_000_000 + *entry.key();
                let session_admin = world
                    .player_sessions
                    .iter()
                    .any(|session| session.id == player_id && session.admin);
                if session_admin {
                    enqueue_outbound(entry.value(), frame.clone(), false);
                }
            }
        }
        "help" => {
            send_message_to_player(
                connections,
                player.id,
                "[orange]-- Commands --\n[orange] /t <msg>[white] - team chat\n[orange] /a <msg>[white] - admin chat\n[orange] /votekick <player> <reason>[white] - vote to kick\n[orange] /vote <y/n>[white] - vote\n[orange] /sync[white] - re-sync world\n[orange] /help[white] - this list",
            );
        }
        "sync" => {
            let now = std::time::Instant::now();
            if now.duration_since(player.last_shot).as_secs() < 5 {
                send_message_to_player(
                    connections,
                    player.id,
                    "[scarlet]You may only /sync every 5 seconds.",
                );
                return;
            }
            player.last_shot = now;
            send_message_to_player(
                connections,
                player.id,
                "[lightgray]World synchronization requested.",
            );
        }
        "votekick" => {
            // Official NetServer votekick: >=3 players, target by name or
            // #id, reason required, 5-minute per-player cooldown, no
            // self/admin/local/other-team targets, votes start with +1.
            if connections.len() < 3 {
                send_message_to_player(
                    connections,
                    player.id,
                    "[scarlet]At least 3 players are needed to start a votekick.",
                );
                return;
            }
            if world.votekick_target.read().is_some() {
                send_message_to_player(
                    connections,
                    player.id,
                    "[scarlet]A vote is already in progress.",
                );
                return;
            }
            let mut parts = args.split_whitespace();
            let target = parts.next().unwrap_or("");
            let reason = parts.collect::<Vec<_>>().join(" ");
            if target.is_empty() {
                send_message_to_player(
                    connections,
                    player.id,
                    "[orange]Players to kick: use /votekick <name> <reason>",
                );
                return;
            }
            if reason.is_empty() {
                send_message_to_player(
                    connections,
                    player.id,
                    "[orange]You need a valid reason to kick the player. Add a reason after the player name.",
                );
                return;
            }
            // Per-player cooldown (official voteCooldown = 5 minutes).
            let now = std::time::Instant::now();
            let on_cooldown = world
                .votekick_cooldowns
                .get(&player.uuid)
                .is_some_and(|last| now.duration_since(*last).as_secs() < 5 * 60);
            if on_cooldown {
                send_message_to_player(
                    connections,
                    player.id,
                    "[scarlet]You must wait 5 minutes between votekicks.",
                );
                return;
            }
            // Find the target connection by name.
            let target_lower = target.to_lowercase();
            let target_found = connections.iter().any(|entry| {
                entry
                    .player_name
                    .read()
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase() == target_lower)
            });
            if !target_found {
                send_message_to_player(
                    connections,
                    player.id,
                    &format!("[scarlet]No player [orange]'{}'[scarlet] found.", target),
                );
                return;
            }
            world.votekick_target.write().replace(target.to_string());
            world
                .votekick_votes
                .store(1, std::sync::atomic::Ordering::Relaxed);
            world.votekick_voters.clear();
            world.votekick_voters.insert(player.uuid.clone(), 1);
            world.votekick_cooldowns.insert(player.uuid.clone(), now);
            send_message_to_player(
                connections,
                player.id,
                &format!(
                    "[lightgray]Reason:[orange] {reason}[lightgray]. Vote started for [orange]{}[lightgray]. Type /vote y or /vote n.",
                    target
                ),
            );
        }
        "vote" => {
            let choice = args.trim().to_ascii_lowercase();
            let Some(target) = world.votekick_target.read().clone() else {
                send_message_to_player(
                    connections,
                    player.id,
                    "[scarlet]Nobody is being voted on.",
                );
                return;
            };
            match choice.as_str() {
                "c" | "cancel" => {
                    // Admins can cancel the vote (official /vote c).
                    if admin_flag {
                        *world.votekick_target.write() = None;
                        world
                            .votekick_votes
                            .store(0, std::sync::atomic::Ordering::Relaxed);
                        world.votekick_voters.clear();
                        send_message_to_player(
                            connections,
                            player.id,
                            &format!(
                                "[lightgray]Vote canceled by admin [orange]{}[lightgray].",
                                player.name
                            ),
                        );
                    } else {
                        send_message_to_player(
                            connections,
                            player.id,
                            "[scarlet]Only admins can cancel the vote.",
                        );
                    }
                }
                "y" | "yes" => {
                    if world
                        .votekick_voters
                        .insert(player.uuid.clone(), 1)
                        .is_some()
                    {
                        send_message_to_player(
                            connections,
                            player.id,
                            "[scarlet]You've already voted. Sit down.",
                        );
                        return;
                    }
                    let votes = world
                        .votekick_votes
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        + 1;
                    let required = 2 + if connections.len() > 4 { 1 } else { 0 };
                    if votes >= required {
                        *world.votekick_target.write() = None;
                        world
                            .votekick_votes
                            .store(0, std::sync::atomic::Ordering::Relaxed);
                        world.votekick_voters.clear();
                        let target_lower = target.to_lowercase();
                        for entry in connections.iter() {
                            let name_matches = entry
                                .player_name
                                .read()
                                .as_ref()
                                .is_some_and(|name| name.to_lowercase() == target_lower);
                            if name_matches {
                                if let Ok(frame) = kick_reason_frame(11) {
                                    // vote
                                    enqueue_outbound(entry.value(), frame, true);
                                }
                                // M4: official VoteSession kicks with
                                // `Player.kick(KickReason.vote,
                                // kickDuration*1000)` where kickDuration =
                                // 3600 s (NetServer bytecode offsets 89-94 +
                                // VoteSession.lambda$checkPass$2), registering
                                // a one-hour recentKick cooldown.
                                let uuid = world
                                    .player_sessions
                                    .iter()
                                    .find(|session| {
                                        session.value().name.to_lowercase() == target_lower
                                    })
                                    .map(|session| session.value().uuid.clone());
                                if let Some(uuid) = uuid {
                                    admin.handle_kicked(
                                        &uuid,
                                        &entry.value().ip.to_string(),
                                        std::time::Duration::from_secs(3600),
                                    );
                                }
                            }
                        }
                    }
                }
                "n" | "no" => {
                    if world
                        .votekick_voters
                        .insert(player.uuid.clone(), -1)
                        .is_some()
                    {
                        send_message_to_player(
                            connections,
                            player.id,
                            "[scarlet]You've already voted. Sit down.",
                        );
                        return;
                    }
                    world
                        .votekick_votes
                        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                    send_message_to_player(connections, player.id, "[lightgray]Vote recorded.");
                }
                _ => {
                    send_message_to_player(
                        connections,
                        player.id,
                        "[scarlet]Vote either 'y' (yes) or 'n' (no).",
                    );
                }
            }
        }
        _ => {
            send_message_to_player(
                connections,
                player.id,
                &format!("[scarlet]Unknown command '/{}'. Use /help.", command),
            );
        }
    }
}

/// KickCallPacket2 (ID 59): TypeIO.writeKick = b reason ordinal. The official
/// server uses this typed variant for protocol validations (clientOutdated,
/// serverOutdated, typeMismatch, customClient, nameEmpty, ...). Ordinals match
/// mindustry.net.Packets.KickReason in the 158.1 jar:
/// 0 kick, 1 clientOutdated, 2 serverOutdated, 3 banned, 4 gameover,
/// 5 recentKick, 6 nameInUse, 7 idInUse, 8 nameEmpty, 9 customClient,
/// 10 serverClose, 11 vote, 12 typeMismatch, 13 whitelist, 14 playerLimit,
/// 15 serverRestarting, 16 custom.
pub(crate) fn kick_reason_frame(reason: u8) -> std::io::Result<Vec<u8>> {
    let payload = vec![reason];
    frame_generated_packet(KICK_2_PACKET_ID, &payload, false)
}

/// Kicks with an arbitrary formatted message: `KickCallPacket` whose
/// payload is a single TypeIO string (`NetConnection.kick(String)` →
/// `Call.kick(con, reason)`). Used for the incompatible-mods message.
pub(crate) fn kick_message_frame(message: &str) -> std::io::Result<Vec<u8>> {
    let payload = encode_typeio_string(message)?;
    frame_generated_packet(KICK_PACKET_ID, &payload, false)
}

/// Player / Syncc.writeSync entity payload, byte-aligned with the official
/// generated Player class (mindustry/gen/Player.java). Used both by the
/// initial EntitySnapshot (unit + player batch) and by the chat
/// SendMessageCallPacket2 so the client renders the sender's colored name.
pub(crate) fn encode_player_entity(
    player: &SessionPlayer,
    x: f32,
    y: f32,
    shooting: bool,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let mut entities = Vec::new();
    entities.write_i(player.id)?;
    entities.write_b(PLAYER_CLASS_ID)?;
    entities.write_bool(player.admin)?; // admin (impl-console Administration)
    entities.write_bool(player.boosting)?;
    entities.write_i(player.color)?;
    entities.write_f(player.mouse_x)?;
    entities.write_f(player.mouse_y)?;
    entities.write_typeio_string(Some(&player.name))?;
    entities.write_s(-1)?; // selected block
    entities.write_i(0)?; // selected rotation
    entities.write_bool(shooting)?;
    entities.write_b(1)?; // team
    entities.write_bool(false)?; // typing
    entities.write_b(2)?; // normal unit reference
    entities.write_i(player.unit_id)?;
    entities.write_f(x)?;
    entities.write_f(y)?;
    Ok(entities)
}

/// SendMessageCallPacket2 (ID 92): TypeIO.writeString(message) +
/// writeString(unformatted) + writeEntity(playersender). The formatted message
/// uses the official chat format `[name] message` so the client renders the
/// sender's colored name (NetClient.sendMessage). Falls back to the plain
/// SendMessageCallPacket (91) if the sender cannot be located.
pub(crate) fn encode_chat_message2_frame(
    sender: &SessionPlayer,
    message: &str,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    // Official chatFormatter (NetServer.java:77):
    // "[coral][[" + player.coloredName() + "[coral]]:[white] " + message
    // where coloredName() = "[#RRGGBB]" + name. The client renders the
    // sender's colored name from the embedded Player entity (message field)
    // and the raw text (unformatted field).
    let colored = format!("[#{:06X}]{}", sender.color & 0xFFFFFF, sender.name);
    let formatted = format!("[coral][[{colored}[coral]]:[white] {message}");
    let mut payload = Vec::new();
    payload.write_typeio_string(Some(&formatted))?;
    payload.write_typeio_string(Some(message))?;
    let entity = encode_player_entity(sender, sender.x, sender.y, false)?;
    payload.extend_from_slice(&entity);
    frame_generated_packet(SEND_MESSAGE_2_PACKET_ID, &payload, false)
}

pub fn framework_registration(kind: u8, connection_id: i32) -> [u8; 8] {
    let mut frame = [0u8; 8];
    frame[0..2].copy_from_slice(&(FRAMEWORK_PACKET_LEN as u16).to_be_bytes());
    frame[2] = FRAMEWORK_MESSAGE_ID as u8;
    frame[3] = kind;
    frame[4..8].copy_from_slice(&connection_id.to_be_bytes());
    frame
}

pub fn framework_keepalive() -> [u8; 4] {
    [0, 2, FRAMEWORK_MESSAGE_ID as u8, KEEP_ALIVE]
}

/// UDP KeepAlive is the PacketSerializer body without the TCP length prefix:
/// `[b -2][b 2]`.
pub fn framework_keepalive_udp() -> [u8; 2] {
    [FRAMEWORK_MESSAGE_ID as u8, KEEP_ALIVE]
}

/// ArcNet `ServerConnectFilter` / `ArcNetProvider.connected` playerLimit:
/// reject a new TCP accept when the live ArcNet connection count is already
/// at the configured cap (joined or still in RegisterUDP).
pub(crate) fn connection_limit_blocks_accept(limit: u32, current_connections: usize) -> bool {
    limit > 0 && current_connections >= limit as usize
}

/// TCP idle timeout uses only TCP reads. UDP KeepAlive / snapshots must not
/// refresh this clock (Arc `TcpConnection.lastReadTime`).
pub(crate) fn tcp_idle_timed_out(
    last_tcp_read: std::time::Instant,
    now: std::time::Instant,
) -> bool {
    now.duration_since(last_tcp_read) >= TCP_TIMEOUT
}

pub(crate) fn tcp_needs_keepalive(
    last_tcp_write: std::time::Instant,
    now: std::time::Instant,
) -> bool {
    now.duration_since(last_tcp_write) >= TCP_KEEPALIVE
}

pub(crate) fn udp_needs_keepalive(
    last_udp_comm: std::time::Instant,
    now: std::time::Instant,
) -> bool {
    now.duration_since(last_udp_comm) >= UDP_KEEPALIVE
}

/// `Ratekeeper.allow(3000, packetSpamLimit)` for game packets. Framework
/// KeepAlive/Ping do not count (P1-09 adversarial).
pub(crate) fn record_inbound_game_packet(rate: &mut ChatRateLimiter, packet_data: &[u8]) -> bool {
    if packet_data.is_empty() || packet_data[0] as i8 == FRAMEWORK_MESSAGE_ID {
        return true;
    }
    rate.allow(PACKET_SPAM_WINDOW_MS, PACKET_SPAM_LIMIT)
}

/// Framework Ping body after `read_packet`: `[b -2][b 0][i32 id][b isReply]`.
pub(crate) fn parse_framework_ping(packet_data: &[u8]) -> Option<(i32, bool)> {
    if packet_data.len() >= 7 && packet_data[0] as i8 == FRAMEWORK_MESSAGE_ID && packet_data[1] == 0
    {
        let id = i32::from_be_bytes(packet_data[2..6].try_into().ok()?);
        Some((id, packet_data[6] != 0))
    } else {
        None
    }
}

pub fn encode_server_info(
    info: &ServerInfo,
    state: &GameState,
    admin: &Administration,
    port: u16,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(500);
    write_discovery_string(&mut out, &info.name, 100);
    write_discovery_string(&mut out, &state.map_name.read(), 64);
    out.extend_from_slice(&(state.players_count.load(Ordering::Relaxed) as i32).to_be_bytes());
    out.extend_from_slice(&(state.wave.load(Ordering::Relaxed) as i32).to_be_bytes());
    out.extend_from_slice(&info.build.to_be_bytes());
    write_discovery_string(&mut out, &info.version_type, u8::MAX as usize);
    out.push(match *state.mode.read() {
        GameMode::Survival => 0,
        GameMode::Sandbox => 1,
        GameMode::Attack => 2,
        GameMode::Pvp => 3,
    });
    out.extend_from_slice(&(admin.get_player_limit() as i32).to_be_bytes());
    write_discovery_string(&mut out, &info.description, 100);
    write_discovery_string(&mut out, "", 50);
    out.extend_from_slice(&(port as i16).to_be_bytes());
    out
}

fn write_discovery_string(out: &mut Vec<u8>, value: &str, max: usize) {
    let mut end = value.len().min(max).min(u8::MAX as usize);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    out.push(end as u8);
    out.extend_from_slice(&value.as_bytes()[..end]);
}

#[cfg(test)]
mod tests;
