//! Client session/transport layer.
//!
//! Extracted from `listener.rs` (P2 listener split): the per-connection
//! `handle_tcp` task that owns the client session lifecycle (handshake,
//! frame reads, packet dispatch, teardown) plus the TCP frame I/O
//! helpers (`read_frame`, `send_world_stream`).

use crate::network::buildings::construction::apply_build_plans;
use crate::network::buildings::construction::encode_begin_break;
use crate::network::buildings::construction::encode_begin_place;
use crate::network::buildings::construction::remove_team_plan;
use crate::network::codec::{
    read_packet, write_tcp_packet, Reads, FRAMEWORK_MESSAGE_ID, MAX_PACKET_SIZE,
};
use crate::network::combat::encode_create_bullet_payload;
use crate::network::combat::projectile_armor_multiplier;
use crate::network::combat::spawn_projectile;
use crate::network::economy::compute_power_efficiency;
use crate::network::economy::payload::{
    apply_request_build_payload, apply_request_drop_payload, apply_request_unit_payload,
};
use crate::network::listener::apply_client_snapshot;
use crate::network::listener::apply_rotate_block;
use crate::network::listener::apply_tile_config;
use crate::network::listener::broadcast_except;
use crate::network::listener::broadcast_player_snapshot;
use crate::network::listener::broadcast_respawn;
use crate::network::listener::deposit_player_inventory;
use crate::network::listener::encode_block_snapshot;
use crate::network::listener::encode_construct_block_snapshot;
use crate::network::listener::encode_construct_finish;
use crate::network::listener::encode_rotate_block_frame;
use crate::network::listener::encode_take_items_frame;
use crate::network::listener::encode_tile_config_frame;
use crate::network::listener::encode_transfer_item_to_frame;
use crate::network::listener::handle_client_command;
use crate::network::listener::player_team;
use crate::network::listener::request_block_snapshot_target;
use crate::network::listener::respawn_session_player;
use crate::network::listener::send_message_to_player;
use crate::network::listener::unit_control_allowed;
use crate::network::listener::update_mining;
use crate::network::listener::valid_client_noop_payload;
use crate::network::listener::withdraw_items_to_player;
use crate::network::listener::SnapshotTarget;
use crate::network::listener::*;
use crate::network::packets::Packet;
use crate::network::protocol::*;
use crate::network::world::core_position_for_team;
use crate::network::world::{
    core_world_for_team, DynamicTile, DynamicWorld, PendingBreak, PendingBuild, PendingConnection,
    PlayerCombatState, Projectile, SessionPlayer, WorldStore,
};
use crate::state::administration::Administration;
use crate::state::game_state::GameMode;
use dashmap::DashMap;
use std::collections::HashSet;
use std::io::{Error, ErrorKind};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpStream, UdpSocket};
use tracing::{debug, info, warn};

mod combat;
pub use combat::{spawn_team_projectile, update_player_combat};
mod replay;
pub(crate) use replay::replay_active_projectiles;
pub use replay::{
    encode_all_player_snapshots, read_frame, replay_dynamic_tiles, replay_pending_breaks,
    replay_pending_builds, send_all_player_snapshots, send_generated_packet,
    send_generated_packet_prefer_udp, send_player_spawn, send_state_snapshot, send_world_stream,
    world_stream_frames,
};
pub(crate) use replay::{framework_ping, framework_ping_reply, packet_id_label};

use crate::network::buildings::construction::{
    dynamic_at, effective_building_team, network_template_with_plans,
};
#[allow(clippy::too_many_arguments)]
pub async fn handle_tcp(
    socket: TcpStream,
    peer: SocketAddr,
    connection_id: i32,
    admin: Administration,
    mut outbound: tokio::sync::mpsc::Receiver<Vec<u8>>,
    mut udp_inbound: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    connections: Arc<DashMap<i32, PendingConnection>>,
    store: WorldStore,
    udp_socket: Arc<UdpSocket>,
) -> std::io::Result<()> {
    socket.set_nodelay(true)?;
    let (mut reader, mut writer) = socket.into_split();
    writer
        .write_all(&framework_registration(REGISTER_TCP, connection_id))
        .await?;
    let mut joined = false;
    let mut session_player: Option<SessionPlayer> = None;
    // P1 anticheat: wall-clock baseline of the last accepted ClientSnapshot
    // (official NetConnection.lastReceivedClientTime).
    let mut last_client_snapshot_time = std::time::Instant::now();
    // Official Config.snapshotInterval default is 200 ms (5 Hz). 50 ms (20 Hz)
    // is a documented deviation: 200 ms made units/conveyors visibly stutter
    // on desktop 158.1, and the client interpolates the higher rate without
    // hitting the 20 s UDP timeout.
    let mut snapshot_interval = tokio::time::interval(crate::network::listener::SNAPSHOT_INTERVAL);
    snapshot_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Arc TcpConnection.keepAliveMillis = 8000: if TCP has been quiet that
    // long, send a Framework Ping (the repo's RTT probe; KeepAlive also
    // satisfies the client 12 s read timeout). First tick is delayed so the
    // RegisterTCP write is not immediately followed by a ping.
    let keepalive_start = tokio::time::Instant::now() + crate::network::listener::TCP_KEEPALIVE;
    let mut keepalive_interval =
        tokio::time::interval_at(keepalive_start, crate::network::listener::TCP_KEEPALIVE);
    keepalive_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Round 74d develop: framework PING round trip (the 158.1 client's
    // in-game ping is the RTT of ITS pings, which we echo below; ours
    // measures the same trip from the server side). The client answers a
    // non-reply Ping with a reply carrying the same id (Connection.
    // notifyReceived, bytecode 158.1). 0 = not measured yet.
    let mut keepalive_sent_at: Option<(std::time::Instant, i32)> = None;
    let mut next_ping_id: i32 = 1;
    let mut last_tcp_read = std::time::Instant::now();
    let mut last_udp_comm = std::time::Instant::now();
    let mut packet_rate = crate::network::listener::ChatRateLimiter::new();
    let connection_state = connections
        .get(&connection_id)
        .map(|connection| connection.value().clone())
        .or_else(|| {
            // Tests construct sessions without the shared registry; the
            // diagnostics degrade gracefully.
            None
        });

    loop {
        // Reload the shared world each iteration so `host` hot-swaps and the
        // re-streamed ConnectConfirm apply to the new map immediately.
        let world = store.load();
        if let Some(player) = session_player.as_mut() {
            if let Some(profile) = world.player_profiles.get(&player.uuid) {
                if player.unit_id != profile.unit_id {
                    player.unit_id = profile.unit_id;
                    player.x = profile.x;
                    player.y = profile.y;
                    player.shooting = false;
                    player.boosting = false;
                }
            }
        }
        let tcp_deadline = tokio::time::Instant::now()
            + crate::network::listener::TCP_TIMEOUT.saturating_sub(last_tcp_read.elapsed());
        let frame = tokio::select! {
            result = read_frame(&mut reader) => {
                last_tcp_read = std::time::Instant::now();
                result.map_err(|err| {
                    if err.kind() == ErrorKind::UnexpectedEof {
                        let phase = if joined {
                            "after joining"
                        } else if session_player.is_some() {
                            "while loading the world"
                        } else {
                            "before ConnectPacket"
                        };
                        Error::new(ErrorKind::UnexpectedEof, format!("{phase}: {err}"))
                    } else {
                        err
                    }
                })?
            }
            message = outbound.recv() => {
                let message = message.ok_or_else(|| {
                    Error::new(ErrorKind::ConnectionAborted, "outbound channel closed")
                })?;
                if let Some(connection) = &connection_state {
                    connection.outbound_queued.fetch_sub(1, Ordering::Relaxed);
                }
                writer.write_all(&message).await?;
                continue;
            }
            message = udp_inbound.recv(), if joined => {
                last_udp_comm = std::time::Instant::now();
                message.ok_or_else(|| {
                    Error::new(ErrorKind::ConnectionAborted, "UDP inbound channel closed")
                })?
            }
            _ = tokio::time::sleep_until(tcp_deadline) => {
                return Err(Error::new(
                    ErrorKind::TimedOut,
                    format!(
                        "ArcNet TCP timeout ({} ms without a TCP read)",
                        crate::network::listener::TCP_TIMEOUT.as_millis()
                    ),
                ));
            }
            _ = keepalive_interval.tick() => {
                // Framework Ping (type 0) instead of the one-way KeepAlive:
                // the client replies, so the server can measure the RTT.
                let ping_id = next_ping_id;
                next_ping_id = next_ping_id.wrapping_add(1);
                keepalive_sent_at = Some((std::time::Instant::now(), ping_id));
                writer.write_all(&framework_ping(ping_id)).await?;
                if let Some(endpoint) = connections
                    .get(&connection_id)
                    .and_then(|connection| *connection.udp_endpoint.read())
                {
                    if udp_needs_keepalive(last_udp_comm, std::time::Instant::now()) {
                        let _ = udp_socket
                            .send_to(&framework_keepalive_udp(), endpoint)
                            .await;
                        last_udp_comm = std::time::Instant::now();
                    }
                }
                continue;
            }
            _ = snapshot_interval.tick(), if joined => {
                // P1: the 50 ms state/entity snapshots prefer the client's
                // registered UDP endpoint (unreliable transport) with a TCP
                // fallback, so a slow or UDP-less client still gets them.
                let udp_endpoint = connections
                    .get(&connection_id)
                    .and_then(|connection| *connection.udp_endpoint.read());
                let state_payload = encode_state_snapshot_for(&world.game_state, Some(&world))?;
                send_generated_packet_prefer_udp(
                    &udp_socket,
                    udp_endpoint,
                    &mut writer,
                    STATE_SNAPSHOT_PACKET_ID,
                    &state_payload,
                    true,
                )
                .await?;
                for snapshot in encode_all_player_snapshots(&world)? {
                    send_generated_packet_prefer_udp(
                        &udp_socket,
                        udp_endpoint,
                        &mut writer,
                        ENTITY_SNAPSHOT_PACKET_ID,
                        &snapshot,
                        true,
                    )
                    .await?;
                }
                for payload in encode_enemy_entity_snapshots(&world)? {
                    send_generated_packet_prefer_udp(
                        &udp_socket,
                        udp_endpoint,
                        &mut writer,
                        ENTITY_SNAPSHOT_PACKET_ID,
                        &payload,
                        true,
                    )
                    .await?;
                }
                continue;
            }
        };
        let received_at = std::time::Instant::now();
        if let Some(connection) = &connection_state {
            connection.last_packet_epoch_ms.store(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0),
                Ordering::Relaxed,
            );
        }
        let packet_data = read_packet(std::io::Cursor::new(frame))?;
        if packet_data.is_empty() || packet_data[0] as i8 == FRAMEWORK_MESSAGE_ID {
            // ArcNet framework message. Payload layout (PacketSerializer.
            // writeFramework, 158.1): [b type][...]. Ping = [b 0][i32 id]
            // [b isReply]; KeepAlive = [b 2] (no answer).
            if let Some((ping_id, is_reply)) = parse_framework_ping(&packet_data) {
                if is_reply {
                    // Answer to OUR ping: measure the server-side RTT.
                    if let (Some(connection), Some((sent, sent_id))) =
                        (&connection_state, keepalive_sent_at.take())
                    {
                        if sent_id == ping_id {
                            connection.last_keepalive_rtt_ms.store(
                                received_at.duration_since(sent).as_millis() as u64,
                                Ordering::Relaxed,
                            );
                        }
                    }
                } else {
                    // The client's ping (it displays this RTT): echo it back
                    // with isReply=true, exactly like Connection.notifyReceived.
                    writer.write_all(&framework_ping_reply(ping_id)).await?;
                }
            }
            continue;
        }
        if !record_inbound_game_packet(&mut packet_rate, &packet_data) {
            let ip = peer.ip().to_string();
            warn!(
                "Blacklisting IP {} as potential DOS attack - packet spam",
                ip
            );
            admin.add_dos_ban(&ip);
            return Err(Error::new(
                ErrorKind::ConnectionAborted,
                "packet spam limit exceeded",
            ));
        }
        if joined {
            if let Some(local) = session_player.as_mut() {
                if let Some(active) = world
                    .player_sessions
                    .iter()
                    .find(|active| active.id == local.id)
                {
                    *local = active.value().clone();
                }
            }
        }

        let id = packet_data[0];
        match Packet::read(std::io::Cursor::new(&packet_data[1..]), id) {
            Ok(Packet::ConnectPacket(connect)) => {
                // P0-11: the 158.1 client does not send ConnectPacket until
                // RegisterUDP completes. Reject an early ConnectPacket so a
                // session is never promoted from TCP-only.
                if !udp_handshake_complete(&connections, connection_id) {
                    warn!(
                        "Ignoring ConnectPacket from {} (connection {}); UDP is not registered",
                        peer, connection_id
                    );
                    continue;
                }
                // Official NetServer.connect (158.1) validation order:
                // idInUse null ids, id/name ban, recent kick, player limit
                // (admin bypass), mods incompatibility, whitelist,
                // version/type, strict duplicates, empty name, locale, build.
                let ip = peer.ip().to_string();
                // 1. uuid/usid null -> idInUse (the port always decodes a
                //    uuid; the usid is not tracked per connection yet).
                // 2. id ban (Administration.isIDBanned) and name ban
                //    (Administration.isNameBanned) -> banned.
                if admin.is_banned(&ip, &connect.uuid) {
                    // Official NetServer sends KickCallPacket(58/59) with
                    // reason banned(3) BEFORE closing. Writing the frame
                    // directly to the TCP writer (round-73 A1): frames
                    // enqueued on the outbound channel were dropped when the
                    // session task exited, so the client only saw EOF.
                    if let Ok(frame) = kick_reason_frame(3) {
                        writer.write_all(&frame).await?;
                    }
                    return Err(Error::new(ErrorKind::PermissionDenied, "client is banned"));
                }
                if admin.is_name_banned(&connect.name) {
                    if let Ok(frame) = kick_reason_frame(3) {
                        // banned (direct write, round-73 A1)
                        writer.write_all(&frame).await?;
                    }
                    return Ok(());
                }
                // 3. recent kick: `Time.millis() < getKickTime(uuid, ip)`
                //    -> KickReason.recentKick (4) (M4: consulted by
                //    uuid+ip like Administration.getKickTime). Expired
                //    cooldowns are cleared so the maps stay bounded.
                if admin
                    .kick_time(&connect.uuid, &ip)
                    .is_some_and(|until| std::time::Instant::now() < until)
                {
                    if let Ok(frame) = kick_reason_frame(4) {
                        // recentKick (direct write, round-73 A1)
                        writer.write_all(&frame).await?;
                    }
                    return Ok(());
                }
                admin.clear_kick_time(&connect.uuid, &ip);
                // 4. player limit (admin bypass).
                let current_players = world.player_sessions.len() as u32;
                if admin.is_at_player_limit(current_players, &connect.uuid) {
                    if let Ok(frame) = kick_reason_frame(14) {
                        // playerLimit (direct write, round-73 A1)
                        writer.write_all(&frame).await?;
                    }
                    return Ok(());
                }
                // 5. mods incompatibility: the vanilla server registers no
                //    mods, so any client mod is "Unnecessary" and the
                //    connection is kicked with a formatted message
                //    (`con.kick(result.toString(), 0)` — no cooldown).
                if !connect.mods.is_empty() {
                    let mut result = String::from("[accent]Incompatible mods![]\n\n");
                    result.push_str("Unnecessary mods:[lightgray]\n> ");
                    result.push_str(&connect.mods.join("\n> "));
                    if let Ok(frame) = kick_message_frame(&result) {
                        // incompatible mods (direct write, round-73 A1)
                        writer.write_all(&frame).await?;
                    }
                    warn!(
                        "Kicked {}: {} incompatible mod(s): {}",
                        peer,
                        connect.mods.len(),
                        connect.mods.join(", ")
                    );
                    return Ok(());
                }
                // 6. whitelist (uuid; the usid is not tracked per player).
                if !admin.is_whitelisted(&connect.uuid) {
                    if let Ok(frame) = kick_reason_frame(13) {
                        // whitelist (direct write, round-73 A1)
                        writer.write_all(&frame).await?;
                    }
                    return Ok(());
                }
                // 7. version/type + empty name (order matches the JAR).
                const SERVER_BUILD: i32 = crate::compat_target::CURRENT_PROTOCOL_BUILD;
                const SERVER_TYPE: &str = "official";
                let mut kick_reason: Option<u8> = None;
                if connect.version_type != SERVER_TYPE {
                    kick_reason = Some(12); // typeMismatch
                } else if connect.version == -1 {
                    kick_reason = Some(9); // customClient
                } else if connect.version != SERVER_BUILD {
                    kick_reason = Some(if connect.version > SERVER_BUILD { 2 } else { 1 });
                } else if connect.name.trim().is_empty() {
                    kick_reason = Some(8); // nameEmpty
                }
                // 8. strict duplicates (official `preventDuplicates =
                //    headless && admins.isStrict()`): duplicate names ->
                //    nameInUse (6), duplicate uuid/usid -> idInUse (7).
                let strict = world.game_state.strict_mode.load(Ordering::Relaxed);
                let duplicate_name = strict
                    && world.player_sessions.iter().any(|session| {
                        session
                            .value()
                            .name
                            .trim()
                            .eq_ignore_ascii_case(connect.name.trim())
                    });
                if kick_reason.is_none() && duplicate_name {
                    kick_reason = Some(6); // nameInUse
                }
                if kick_reason.is_none() && strict {
                    let duplicate_id = world
                        .player_sessions
                        .iter()
                        .any(|session| session.value().uuid == connect.uuid);
                    if duplicate_id {
                        kick_reason = Some(7); // idInUse
                    }
                }
                if let Some(reason) = kick_reason {
                    if let Ok(frame) = kick_reason_frame(reason) {
                        // version/type/nameEmpty/nameInUse/idInUse
                        // (direct write, round-73 A1)
                        writer.write_all(&frame).await?;
                    }
                    return Ok(());
                }
                info!(
                    "Mindustry client connected: name={}, uuid={}, build={}",
                    connect.name, connect.uuid, connect.version
                );
                if let Some(connection) = connections.get(&connection_id) {
                    *connection.player_name.write() = Some(connect.name.clone());
                }
                let uuid = connect.uuid.clone();
                let player_id = 1_000_000i32.wrapping_add(connection_id);
                let unit_id = 2_000_000i32.wrapping_add(connection_id);
                // Official NetServer.assignTeam runs on ConnectPacket (before
                // sendWorldData) so the world stream and core spawn already
                // use the PvP team. ConnectConfirm only finishes the join.
                let persisted_team = world
                    .player_profiles
                    .get(&uuid)
                    .map(|profile| profile.team)
                    .unwrap_or(1);
                let team = assign_team_for_join(&world, &uuid, persisted_team);
                let (spawn_x, spawn_y) = core_world_for_team(&world, team);
                let mut combat = world
                    .player_profiles
                    .get(&uuid)
                    .map(|profile| profile.clone())
                    .unwrap_or(PlayerCombatState {
                        uuid: uuid.clone(),
                        player_id,
                        unit_id,
                        x: spawn_x,
                        y: spawn_y,
                        health: 150.0,
                        shield: 0.0,
                        status_effect: -1,
                        status_duration: 0.0,
                        statuses: Vec::new(),
                        dead: false,
                        respawn_timer: 0.0,
                        team: 1,
                    });
                combat.player_id = player_id;
                combat.unit_id = unit_id;
                combat.team = team;
                world.players.insert(unit_id, combat.clone());
                world.player_profiles.insert(uuid.clone(), combat.clone());
                world.persistence_dirty.store(true, Ordering::Relaxed);
                let template = network_template_with_plans(&world)?;
                let mode = *world.game_state.mode.read();
                let world_stream =
                    crate::engine::world_stream::personalize_current_with_state_mode(
                        &template,
                        player_id,
                        &connect.name,
                        connect.color,
                        (combat.x, combat.y),
                        world.game_state.wave.load(Ordering::Relaxed),
                        *world.game_state.wave_time.read(),
                        f64::from(*world.game_state.simulation_time.read()),
                        mode == GameMode::Pvp,
                        mode == GameMode::Sandbox,
                    )?;
                let is_admin_player = admin.is_admin(&connect.uuid);
                let session = SessionPlayer {
                    id: player_id,
                    controlled_unit: crate::network::world::ControlledUnit::Core,
                    unit_id,
                    uuid,
                    name: connect.name,
                    color: connect.color,
                    last_snapshot: -1,
                    x: combat.x,
                    y: combat.y,
                    mouse_x: spawn_x,
                    mouse_y: spawn_y,
                    rotation: 90.0,
                    boosting: false,
                    shooting: false,
                    last_command: None,
                    active_plans: HashSet::new(),
                    mining_position: None,
                    mining_progress: 0.0,
                    mining_updated: std::time::Instant::now(),
                    carried_item: -1,
                    carried_amount: 0,
                    preview_plan_group: -1,
                    preview_plans: Vec::new(),
                    last_shot: std::time::Instant::now() - std::time::Duration::from_secs(1),
                    admin: is_admin_player,
                    chat_rate: crate::network::listener::ChatRateLimiter::new(),
                };
                session_player = Some(session);
                // Track the connected player for console `players`/`ban <name>`
                // (impl-console Administration registry; unregistered in teardown).
                admin.register_connection(crate::state::administration::ConnectedPlayer {
                    uuid: session_player.as_ref().unwrap().uuid.clone(),
                    name: session_player.as_ref().unwrap().name.clone(),
                    ip: peer.ip().to_string(),
                    player_id: session_player.as_ref().unwrap().id,
                    unit_id: session_player.as_ref().unwrap().unit_id,
                });
                send_world_stream(&mut writer, &world_stream).await?;
            }
            Ok(Packet::Unknown {
                id: CONNECT_CONFIRM_PACKET_ID,
                ..
            }) => {
                joined = true;
                info!("Client {} finished loading the world", peer);
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "ConnectConfirm before ConnectPacket",
                    )
                })?;
                world.player_sessions.insert(player.unit_id, player.clone());
                send_player_spawn(&mut writer, player, &world).await?;
                send_all_player_snapshots(&mut writer, &world).await?;
                replay_dynamic_tiles(&mut writer, player, &world.tiles).await?;
                let mut damaged_buildings: Vec<_> = world
                    .tiles
                    .iter()
                    .filter(|tile| tile.block != 0)
                    .filter_map(|tile| {
                        let health = dynamic_tile_health(&tile);
                        (health < crate::game::content::block_health(tile.block))
                            .then_some((tile.position, health))
                    })
                    .collect();
                damaged_buildings.extend(world.base_buildings.iter().filter_map(|building| {
                    let maximum = crate::game::content::block_health(building.block);
                    (building.health < maximum).then_some((building.position, building.health))
                }));
                if !damaged_buildings.is_empty() {
                    writer
                        .write_all(&encode_build_health_update_frame(&damaged_buildings)?)
                        .await?;
                }
                replay_pending_builds(&mut writer, player, &world.pending_builds).await?;
                replay_pending_breaks(&mut writer, player, &world.pending_breaks).await?;
                send_state_snapshot(&mut writer, &world.game_state, &world).await?;
                let power = compute_power_efficiency(&world);
                for frame in encode_block_snapshots(&world, &power)? {
                    writer.write_all(&frame).await?;
                }
                // Round 74f: power-node configs AFTER the block snapshots —
                // the 158.1 client only activates/merges node power graphs
                // through the PowerNode config handler (Building.add never
                // runs for update=false nodes), so links arriving only via
                // snapshots leave the node at "+0/s" and its linked
                // machines unpowered client-side.
                // Snapshot power-node configs before any `.await`: DashMap
                // `iter()` holds a per-shard lock, and a live iterator across
                // `write_all` would keep that lock suspended.
                let mut node_frames = Vec::new();
                for tile in world.tiles.iter() {
                    if crate::network::buildings::power::is_power_node(tile.block)
                        && !tile.power_links.is_empty()
                    {
                        let position = tile.position;
                        let config = tile.config.clone();
                        if let Ok(frame) = encode_tile_config_frame(player, position, &config) {
                            node_frames.push(frame);
                        }
                    }
                }
                for frame in node_frames {
                    writer.write_all(&frame).await?;
                }
                replay_active_projectiles(&mut writer, &world).await?;
                // A client joining while the game is already over must see
                // the game-over condition (the packet is only broadcast at
                // the moment the game ends; without this re-send the client
                // hangs on a frozen world with no game-over screen).
                if world.game_state.game_over.load(Ordering::Relaxed) {
                    let winner = world.wave_rules.read().wave_team;
                    send_generated_packet(&mut writer, GAME_OVER_PACKET_ID, &[winner], false)
                        .await?;
                }
            }
            Ok(Packet::Unknown {
                id: CLIENT_SNAPSHOT_PACKET_ID,
                payload,
            }) => {
                let snapshot = decode_client_snapshot(&payload)?;
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "snapshot before ConnectPacket")
                })?;
                let unit_matches = match player.controlled_unit {
                    crate::network::world::ControlledUnit::Core => {
                        snapshot.unit_id == player.unit_id || snapshot.unit_id == -1
                    }
                    crate::network::world::ControlledUnit::Standard(unit_id) => {
                        snapshot.unit_id == unit_id || snapshot.unit_id == -1
                    }
                    // BlockUnit IDs are local transient entity IDs; TypeIO
                    // identifies them by tile position everywhere persistent.
                    crate::network::world::ControlledUnit::Building(_) => true,
                };
                if snapshot.snapshot_id > player.last_snapshot && unit_matches {
                    let alive = world
                        .players
                        .get(&player.unit_id)
                        .is_none_or(|combat| !combat.dead);
                    if alive {
                        if let Some(combat) = world.players.get(&player.unit_id) {
                            player.x = combat.x;
                            player.y = combat.y;
                        }
                        // P1 anticheat: official NetServer.clientSnapshot
                        // measures the elapsed wall time since the previous
                        // snapshot (`lastReceivedClientTime`, capped at
                        // 1500 ms) and limits the applied movement to
                        // `elapsed/1000*60*speed*1.1` in strict mode.
                        let now = std::time::Instant::now();
                        let elapsed_ms = now
                            .duration_since(last_client_snapshot_time)
                            .as_millis()
                            .min(1500) as u64;
                        last_client_snapshot_time = now;
                        let strict = world.game_state.strict_mode.load(Ordering::Relaxed);
                        // M3: a correction beyond the official 112-unit
                        // threshold returns the authoritative position to
                        // push back to this client via SetPosition(110).
                        let correction = apply_controlled_client_snapshot(
                            player, &snapshot, &world, strict, elapsed_ms,
                        );
                        if let Some((x, y)) = correction {
                            let mut payload = Vec::new();
                            crate::network::codec::Writes::write_f(&mut payload, x)?;
                            crate::network::codec::Writes::write_f(&mut payload, y)?;
                            let frame =
                                frame_generated_packet(SET_POSITION_PACKET_ID, &payload, false)?;
                            if let Some(connection) = connections.get(&connection_id) {
                                enqueue_outbound(&connection, frame, true);
                            }
                        }
                        if let Some(mut combat) = world.players.get_mut(&player.unit_id) {
                            combat.x = player.x;
                            combat.y = player.y;
                            let profile = combat.clone();
                            let uuid = profile.uuid.clone();
                            drop(combat);
                            world.player_profiles.insert(uuid, profile);
                            world.persistence_dirty.store(true, Ordering::Relaxed);
                        }
                        if matches!(
                            player.controlled_unit,
                            crate::network::world::ControlledUnit::Core
                                | crate::network::world::ControlledUnit::Standard(_)
                        ) {
                            apply_build_plans(
                                player,
                                &snapshot.plans,
                                &world,
                                &connections,
                                &admin,
                                snapshot.building,
                            )?;
                        }
                        if player.controlled_unit == crate::network::world::ControlledUnit::Core {
                            update_mining(player, &snapshot, &world, &connections)?;
                        }
                        // Possessed standard units and BlockUnit turret
                        // proxies fire through their own authoritative mount
                        // simulation. Running Alpha's 17-tick bullet here as
                        // well produced a duplicate, wrong weapon on every
                        // controlled-unit snapshot.
                        if player.controlled_unit == crate::network::world::ControlledUnit::Core {
                            update_player_combat(player, &world, &connections)?;
                        }
                    } else {
                        player.last_snapshot = snapshot.snapshot_id;
                    }
                    world.player_sessions.insert(player.unit_id, player.clone());
                    let combat = world.players.get(&player.unit_id);
                    let payload = encode_initial_entity_snapshot(player, combat.as_deref())?;
                    let frame = frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &payload, true)?;
                    broadcast(&connections, frame);
                }
            }
            Ok(Packet::Unknown {
                id: COMMAND_BUILDING_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "CommandBuilding before ConnectPacket",
                    )
                })?;
                let (buildings, target_x, target_y) = decode_command_building(&payload)?;
                let actor_team = player_team(&world, player);
                // Official InputHandler.commandBuilding runs
                // `admins.allowAction(player, ActionType.commandBuilding)`.
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::CommandBuilding,
                    None,
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                if apply_command_building_for_team(
                    &world, actor_team, &buildings, target_x, target_y,
                ) {
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                broadcast_except(
                    &connections,
                    connection_id,
                    encode_command_building_frame(player, &buildings, target_x, target_y)?,
                );
            }
            Ok(Packet::Unknown {
                id: COMMAND_UNITS_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "CommandUnits before ConnectPacket")
                })?;
                let request = decode_command_units(&payload)?;
                let actor_team = player_team(&world, player);
                // Official InputHandler.commandUnits runs
                // `admins.allowAction(player, ActionType.commandUnits)`.
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::CommandUnits,
                    None,
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                if apply_command_units_for_team(&world, actor_team, &request) {
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                broadcast_except(
                    &connections,
                    connection_id,
                    encode_command_units_frame(player, &payload)?,
                );
            }
            Ok(Packet::Unknown {
                id: SET_UNIT_COMMAND_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "SetUnitCommand before ConnectPacket",
                    )
                })?;
                let (unit_ids, command) = decode_set_unit_command(&payload)?;
                let actor_team = player_team(&world, player);
                if apply_set_unit_command_for_team(&world, actor_team, &unit_ids, command) {
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                broadcast_except(
                    &connections,
                    connection_id,
                    encode_set_unit_command_frame(player, &payload)?,
                );
            }
            Ok(Packet::Unknown {
                id: SET_UNIT_STANCE_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "SetUnitStance before ConnectPacket")
                })?;
                let (unit_ids, stance, enable) = decode_set_unit_stance(&payload)?;
                let actor_team = player_team(&world, player);
                if apply_set_unit_stance_for_team(&world, actor_team, &unit_ids, stance, enable) {
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                broadcast_except(
                    &connections,
                    connection_id,
                    encode_set_unit_stance_frame(player, &payload)?,
                );
            }
            Ok(Packet::Unknown {
                id: REQUEST_BLOCK_SNAPSHOT_PACKET_ID,
                payload,
            }) if joined => {
                use crate::network::buildings::construction::apply_build_plans;
                use crate::network::buildings::construction::encode_begin_break;
                use crate::network::buildings::construction::encode_begin_place;
                use crate::network::buildings::construction::remove_team_plan;
                use crate::network::codec::{
                    read_packet, write_tcp_packet, Reads, FRAMEWORK_MESSAGE_ID, MAX_PACKET_SIZE,
                };
                use crate::network::combat::encode_create_bullet_payload;
                use crate::network::combat::projectile_armor_multiplier;
                use crate::network::combat::spawn_projectile;
                use crate::network::economy::compute_power_efficiency;
                use crate::network::economy::payload::{
                    apply_request_build_payload, apply_request_drop_payload,
                    apply_request_unit_payload,
                };
                use crate::network::listener::apply_client_snapshot;
                use crate::network::listener::apply_rotate_block;
                use crate::network::listener::apply_tile_config;
                use crate::network::listener::broadcast_except;
                use crate::network::listener::broadcast_player_snapshot;
                use crate::network::listener::broadcast_respawn;
                use crate::network::listener::deposit_player_inventory;
                use crate::network::listener::encode_block_snapshot;
                use crate::network::listener::encode_construct_block_snapshot;
                use crate::network::listener::encode_construct_finish;
                use crate::network::listener::encode_rotate_block_frame;
                use crate::network::listener::encode_take_items_frame;
                use crate::network::listener::encode_tile_config_frame;
                use crate::network::listener::encode_transfer_item_to_frame;
                use crate::network::listener::handle_client_command;
                use crate::network::listener::player_team;
                use crate::network::listener::request_block_snapshot_target;
                use crate::network::listener::respawn_session_player;
                use crate::network::listener::send_message_to_player;
                use crate::network::listener::unit_control_allowed;
                use crate::network::listener::update_mining;
                use crate::network::listener::valid_client_noop_payload;
                use crate::network::listener::withdraw_items_to_player;
                use crate::network::listener::SnapshotTarget;
                use crate::network::packets::Packet;
                use crate::network::protocol::*;
                use crate::network::world::core_position_for_team;
                use tracing::{debug, info, warn};
                let mut input = std::io::Cursor::new(payload);
                let position = input.read_i()?;
                // Official NetServer.requestBlockSnapshot (158.1): the
                // snapshot is sent only when `build.team == player.team()`.
                // The actor's team is resolved from the live session state
                // (SOL-002), never from the packet, and the same gate applies
                // to in-progress constructs so enemy plans are not leaked.
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "RequestBlockSnapshot before ConnectPacket",
                    )
                })?;
                let actor_team = player_team(&world, player);
                match request_block_snapshot_target(&world, position, actor_team) {
                    SnapshotTarget::PendingBuild(build) => {
                        let frame = encode_construct_block_snapshot(
                            build.position,
                            build.block,
                            build.rotation,
                            build.team,
                        )?;
                        writer.write_all(&frame).await?;
                    }
                    SnapshotTarget::PendingBreak(build) => {
                        let rotation = dynamic_at(&world, build.position)
                            .map(|tile| tile.rotation)
                            .unwrap_or(0);
                        let frame = encode_construct_block_snapshot(
                            build.position,
                            build.block,
                            rotation,
                            effective_building_team(&world, build.position),
                        )?;
                        writer.write_all(&frame).await?;
                    }
                    SnapshotTarget::Building(tile) => {
                        let power = compute_power_efficiency(&world);
                        if let Some(frame) = encode_block_snapshot(&world, &tile, &power)? {
                            writer.write_all(&frame).await?;
                        }
                    }
                    SnapshotTarget::None => {}
                }
            }
            Ok(Packet::Unknown {
                id: REQUEST_ITEM_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "RequestItem before ConnectPacket")
                })?;
                let (position, item, amount) = decode_request_item(&payload)?;
                // Official InputHandler.withdrawItem runs
                // `admins.allowAction(player, ActionType.withdrawItem)`.
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::WithdrawItem,
                    Some(position),
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                if let Some((origin, taken)) =
                    withdraw_items_to_player(player, &world, position, item, amount)
                {
                    world.player_sessions.insert(player.unit_id, player.clone());
                    broadcast(
                        &connections,
                        encode_take_items_frame(origin, item, taken, player.unit_id)?,
                    );
                    broadcast_player_snapshot(player, &world, &connections)?;
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(Packet::Unknown {
                id: ROTATE_BLOCK_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "RotateBlock before ConnectPacket")
                })?;
                let (position, direction) = decode_rotate_block(&payload)?;
                // Official InputHandler.rotateBlock runs
                // `admins.allowAction(player, ActionType.rotate)` before the
                // rotation (SOL-002/004/P0-4).
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::Rotate,
                    Some(position),
                    None,
                    None,
                ) {
                    Ok(true) => {
                        if let Some(origin) =
                            apply_rotate_block(player, &world, position, direction)
                        {
                            broadcast(
                                &connections,
                                encode_rotate_block_frame(player, origin, direction)?,
                            );
                            world.persistence_dirty.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(false) => {}
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
            }
            Ok(Packet::Unknown {
                id: TRANSFER_INVENTORY_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "TransferInventory before ConnectPacket",
                    )
                })?;
                let position = decode_building_reference(&payload, "TransferInventory")?;
                // Official InputHandler.depositItem runs
                // `admins.allowAction(player, ActionType.depositItem)`.
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::DepositItem,
                    Some(position),
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                if let Some((origin, item, transferred)) =
                    deposit_player_inventory(player, &world, position)
                {
                    world.player_sessions.insert(player.unit_id, player.clone());
                    broadcast(
                        &connections,
                        encode_transfer_item_to_frame(
                            player.unit_id,
                            item,
                            transferred,
                            player.x,
                            player.y,
                            origin,
                        )?,
                    );
                    broadcast_player_snapshot(player, &world, &connections)?;
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(Packet::Unknown {
                id: UNIT_CLEAR_PACKET_ID,
                payload,
            }) if joined => {
                if !payload.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "UnitClear packet must be empty",
                    ));
                }
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "UnitClear before ConnectPacket")
                })?;
                // Official InputHandler.unitClear runs
                // `admins.allowAction(player, ActionType.respawn)`.
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::Respawn,
                    None,
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                if let Some(old_unit_id) = respawn_session_player(player, &world) {
                    broadcast_respawn(&connections, player, &world, Some(old_unit_id))?;
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(Packet::Unknown {
                id: TILE_CONFIG_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "TileConfig before ConnectPacket")
                })?;
                if let Some((position, config)) = decode_tile_config(&payload)? {
                    debug!(
                        "TileConfig from {}: pos={position} config={config:?}",
                        player.name
                    );
                    // Official InputHandler.tapConfig runs
                    // `admins.allowAction(player, ActionType.configure)`
                    // before applying the config (P0-4).
                    match session_action_allowed(
                        &admin,
                        player,
                        &connections,
                        crate::state::administration::ActionType::Configure,
                        Some(position),
                        None,
                        None,
                    ) {
                        Ok(true) => {
                            if apply_tile_config(player, &world, position, &config) {
                                let frame = encode_tile_config_frame(player, position, &config)?;
                                broadcast(&connections, frame);
                                world.persistence_dirty.store(true, Ordering::Relaxed);
                            }
                        }
                        Ok(false) => {}
                        Err(frame) => {
                            writer.write_all(&frame).await?;
                            return Err(Error::new(
                                ErrorKind::ConnectionAborted,
                                "kicked by anti-spam",
                            ));
                        }
                    }
                }
            }
            Ok(Packet::Unknown {
                id: PING_PACKET_ID,
                payload,
            }) if payload.len() == 8 => {
                // P1: ping responses go over UDP when the endpoint exists.
                let udp_endpoint = connections
                    .get(&connection_id)
                    .and_then(|connection| *connection.udp_endpoint.read());
                send_generated_packet_prefer_udp(
                    &udp_socket,
                    udp_endpoint,
                    &mut writer,
                    PING_RESPONSE_PACKET_ID,
                    &payload,
                    false,
                )
                .await?;
            }
            Ok(Packet::Unknown {
                id: SEND_CHAT_PACKET_ID,
                payload,
            }) => {
                if let Ok(message) = decode_typeio_string(&payload) {
                    // Official chat (NetClient.chat): rate limit
                    // `chatRate.allow(2000, chatSpamLimit)`, then commands
                    // starting with '/' are handled by netServer.clientCommands;
                    // plain messages broadcast via SendMessageCallPacket2.
                    let sender = session_player.as_ref().cloned();
                    if let Some(mut player) = sender {
                        if !player.chat_rate.allow(2000, 2) {
                            send_message_to_player(
                                &connections,
                                player.id,
                                "[scarlet]You are sending messages too quickly.",
                            );
                            continue;
                        }
                        info!("<{}> {}", player.name, message);
                        if message.starts_with('/') {
                            handle_client_command(
                                &world,
                                &connections,
                                &mut player,
                                &message,
                                &admin,
                            );
                            if let Some(updated) = session_player.as_mut() {
                                *updated = player;
                            }
                            continue;
                        }
                        let frame = encode_chat_message2_frame(&player, &message)?;
                        broadcast(&connections, frame);
                    } else {
                        info!("<{}> {}", peer, message);
                        let response = encode_typeio_string(&format!(
                            "[accent]<{}>[] {}",
                            peer.ip(),
                            message
                        ))?;
                        let frame =
                            frame_generated_packet(SEND_MESSAGE_PACKET_ID, &response, false)?;
                        broadcast(&connections, frame);
                    }
                }
            }
            Ok(Packet::Unknown {
                id: DROP_ITEM_PACKET_ID,
                payload,
            }) if joined => {
                let _angle = decode_drop_item(&payload)?;
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "DropItem before ConnectPacket")
                })?;
                // Official InputHandler.dropItem clears the carried stack; the
                // dropped item lands as an effect for all clients.
                if player.carried_item >= 0 && player.carried_amount > 0 {
                    player.carried_item = -1;
                    player.carried_amount = 0;
                    world.player_sessions.insert(player.unit_id, player.clone());
                    broadcast_player_snapshot(player, &world, &connections)?;
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(Packet::Unknown {
                id: DELETE_PLANS_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "DeletePlans before ConnectPacket")
                })?;
                let positions = decode_delete_plans(&payload)?;
                if positions.is_empty() {
                    continue;
                }
                // M5: official InputHandler.removePlanned runs
                // `admins.allowAction(player, ActionType.removePlanned)`
                // (JAR bytecode offsets 0-28) before removing team plans.
                match session_action_allowed_full(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::RemovePlanned,
                    &positions,
                    &[],
                    &[],
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                // Official InputHandler.deletePlans (158.1) removes only the
                // acting player's OWN team plans (`player.team().data().plans`);
                // packet positions are untrusted, so every removal below is
                // scoped to the session's team. Snapshot the shared maps
                // before mutating (anti-deadlock) and drop the guards before
                // removal so no DashMap/parking_lot lock is held across writes.
                let team = player_team(&world, player);
                let pending_build_keys: Vec<_> = world
                    .pending_builds
                    .iter()
                    .map(|entry| (entry.position, entry.team, entry.occupied.clone()))
                    .collect();
                // PendingBreak has no team ownership field, so ownership cannot
                // be proved safely; leave pending breaks untouched rather than
                // guess. The snapshot below only feeds the actor's own plan
                // list cleanup.
                let pending_break_keys: Vec<_> = world
                    .pending_breaks
                    .iter()
                    .map(|entry| (entry.position, entry.occupied.clone()))
                    .collect();
                for position in &positions {
                    player.active_plans.retain(|(_, plan_position, _)| {
                        *plan_position != *position
                            && !pending_build_keys
                                .iter()
                                .any(|(_, _, occupied)| occupied.contains(position))
                            && !pending_break_keys
                                .iter()
                                .any(|(_, occupied)| occupied.contains(position))
                    });
                    for (build_position, build_team, occupied) in &pending_build_keys {
                        if *build_team == team
                            && (build_position == position || occupied.contains(position))
                        {
                            world.pending_builds.remove(build_position);
                        }
                    }
                    remove_team_plan(&world, team, (position >> 16) as i16, *position as i16);
                }
                world.persistence_dirty.store(true, Ordering::Relaxed);
                let frame = encode_delete_plans_forward(player.id, &positions)?;
                broadcast_except(&connections, player.id, frame);
            }
            Ok(Packet::Unknown {
                id: PING_LOCATION_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "PingLocation before ConnectPacket")
                })?;
                let (x, y, text) = decode_ping_location(&payload)?;
                // M5: official InputHandler.pingLocation runs
                // `admins.allowAction(player, ActionType.pingLocation)`
                // (JAR bytecode offsets 0-28); without the gate this RPC was
                // an unbounded broadcast amplifier.
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::PingLocation,
                    None,
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                let frame = encode_ping_location_forward(player.id, x, y, text.as_deref())?;
                broadcast_except(&connections, player.id, frame);
            }
            Ok(Packet::Unknown {
                id: UNIT_CONTROL_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "UnitControl before ConnectPacket")
                })?;
                let (control_type, id) = decode_unit_control(&payload)?;
                let valid = unit_control_allowed(&world, &admin, player, control_type, id);
                if valid {
                    let previous = apply_unit_control(&world, player, control_type, id)
                        .expect("validated UnitControl target disappeared");
                    let controlled_id = match player.controlled_unit {
                        crate::network::world::ControlledUnit::Building(position) => position,
                        crate::network::world::ControlledUnit::Standard(unit_id) => unit_id,
                        crate::network::world::ControlledUnit::Core => id,
                    };
                    if previous == crate::network::world::ControlledUnit::Core {
                        broadcast(&connections, encode_unit_despawn_frame(player, previous)?);
                    }
                    let frame =
                        encode_unit_control_forward(player.id, control_type, controlled_id)?;
                    broadcast_except(&connections, connection_id, frame);
                    broadcast_player_snapshot(player, &world, &connections)?;
                    // The controlled unit's controller tag changes from
                    // CommandAI to Player immediately; do not wait for the
                    // next 50 ms periodic entity batch.
                    for snapshot in encode_enemy_entity_snapshots(&world)? {
                        broadcast(
                            &connections,
                            frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true)?,
                        );
                    }
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(Packet::Unknown {
                id: CLIENT_PLAN_SNAPSHOT_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "ClientPlanSnapshot before ConnectPacket",
                    )
                })?;
                let (group_id, plans_raw) = decode_client_plan_snapshot(&payload)?;
                player.preview_plan_group = group_id;
                player.preview_plans = plans_raw;
                let frame = encode_client_plan_snapshot_received(
                    player.id,
                    group_id,
                    &player.preview_plans,
                )?;
                // Official forwarding is team-scoped (SOL-002): plan previews
                // must not leak to enemy players in PvP/Attack. The sender's
                // own team is resolved from the live authoritative player
                // state, never from the packet.
                let actor_team = player_team(&world, player);
                broadcast_plan_snapshot_team(&world, &connections, player.id, actor_team, frame);
            }
            Ok(Packet::Unknown {
                id: BUILDING_CONTROL_SELECT_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_mut().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "BuildingControlSelect before ConnectPacket",
                    )
                })?;
                // InputHandler.buildingControlSelect invokes the generated
                // UnitBuildingControlSelect call (packet 140), then the
                // building's onControlSelect. The core implementation clears
                // the possessed unit and requests a fresh core unit.
                let build = decode_building_control_select(&payload)?;
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::BuildSelect,
                    Some(build),
                    None,
                    None,
                ) {
                    Ok(true) if building_control_select_allowed(&world, player, build) => {
                        let before = player.controlled_unit;
                        broadcast(
                            &connections,
                            encode_unit_building_control_select_frame(player, before, build)?,
                        );
                        if let Some(old_unit_id) = respawn_session_player(player, &world) {
                            broadcast_respawn(&connections, player, &world, Some(old_unit_id))?;
                            world.persistence_dirty.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(true) => {}
                    Ok(false) => {}
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
            }
            // These generated client calls are present in desktop 158.1 but
            // have no authoritative state to apply in this server. Consume
            // their already-delimited payloads explicitly instead of falling
            // through the generic Unknown arm. The fixed/variable lengths are
            // verified from the corresponding JAR packet write methods.
            Ok(Packet::Unknown {
                id: TILE_TAP_PACKET_ID,
                payload,
            }) => {
                if !valid_client_noop_payload(TILE_TAP_PACKET_ID, &payload) {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "TileTap payload must contain one packed tile position",
                    ));
                }
                debug!("Ignoring TileTap from {} (no-op)", peer);
            }
            Ok(Packet::Unknown {
                id: REQUEST_DEBUG_STATUS_PACKET_ID,
                payload,
            }) => {
                if !valid_client_noop_payload(REQUEST_DEBUG_STATUS_PACKET_ID, &payload) {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "RequestDebugStatus payload must be empty",
                    ));
                }
                // NetServer.requestDebugStatus is a server-only diagnostic
                // callback. It answers only after the connection has a Player;
                // a pre-connect packet is consumed but cannot receive a reply.
                let Some(player) = session_player.as_ref() else {
                    continue;
                };
                let flags = 8 // ConnectPacket has begun connecting
                    | if joined { 2 | 4 } else { 0 }; // connected + Player.add()
                let reliable = encode_debug_status_client(
                    DEBUG_STATUS_CLIENT_PACKET_ID,
                    flags,
                    player.last_snapshot,
                )?;
                let unreliable = encode_debug_status_client(
                    DEBUG_STATUS_CLIENT_UNRELIABLE_PACKET_ID,
                    flags,
                    player.last_snapshot,
                )?;
                writer.write_all(&reliable).await?;
                writer.write_all(&unreliable).await?;
            }
            Ok(Packet::Unknown {
                id: MENU_CHOOSE_PACKET_ID,
                payload,
            }) => {
                if !valid_client_noop_payload(MENU_CHOOSE_PACKET_ID, &payload) {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "MenuChoose payload must contain menu id and option",
                    ));
                }
                debug!("Ignoring MenuChoose from {} (no-op)", peer);
            }
            Ok(Packet::Unknown {
                id: TEXT_INPUT_RESULT_PACKET_ID,
                payload,
            }) => {
                if !valid_client_noop_payload(TEXT_INPUT_RESULT_PACKET_ID, &payload) {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "TextInputResult payload has an invalid TypeIO string",
                    ));
                }
                debug!("Ignoring TextInputResult from {} (no-op)", peer);
            }
            // -------------------------------------------------------------
            // P0-3: the 11 C->S call packets that previously fell through to
            // the generic Unknown arm. Each is decoded with its official
            // 158.1 layout; AdminRequest and SetPlayerTeamEditor have real
            // authority effects, the rest are validated no-ops matching the
            // vanilla server (mod hooks with no registered handler).
            // -------------------------------------------------------------
            Ok(Packet::Unknown {
                id: ADMIN_REQUEST_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(ErrorKind::InvalidData, "AdminRequest before ConnectPacket")
                })?;
                let (target_id, action, params) = decode_admin_request(&payload)?;
                // Official NetServer.adminRequest (158.1): only admins may
                // act, and the target must exist and not be another admin.
                if !player.admin {
                    warn!(
                        "ACCESS DENIED: player {} attempted admin action {} without admin",
                        player.name, action
                    );
                    continue;
                }
                let target_connection_id = target_id - 1_000_000;
                // Clone the connection and DROP the DashMap Ref immediately:
                // the kick/ban branch removes the entry from the same map,
                // and a live Ref would deadlock the shard write lock
                // (project rule: never hold a Ref while mutating the map).
                let Some(target_connection) = connections
                    .get(&target_connection_id)
                    .map(|connection| connection.clone())
                else {
                    warn!(
                        "Admin action {} on nonexistent player {}",
                        action, target_id
                    );
                    continue;
                };
                let target_session = world
                    .player_sessions
                    .iter()
                    .find(|session| session.id == target_id)
                    .map(|entry| entry.value().clone());
                let Some(target_session) = target_session else {
                    warn!("Admin action {} on unknown session {}", action, target_id);
                    continue;
                };
                if target_session.admin && target_session.uuid != player.uuid {
                    warn!("Admin action {} on another admin is denied", action);
                    continue;
                }
                // AdminAction ordinals: 0 kick, 1 ban, 2 trace, 3 wave, 4 switchTeam.
                match action {
                    0 | 1 => {
                        if action == 1 {
                            admin.ban_uuid(&target_session.uuid);
                            admin.ban_ip(&target_connection.ip.to_string());
                        }
                        let reason = if action == 1 { 3 } else { 0 }; // banned / kick
                        if let Ok(frame) = kick_reason_frame(reason) {
                            enqueue_outbound(&target_connection, frame, true);
                        }
                        // M4: every production kick path goes through
                        // `handleKicked(uuid, ip, duration)`; the official
                        // AdminRequest kick uses `player.kick(reason)` with
                        // duration 0, so no cooldown is registered.
                        admin.handle_kicked(
                            &target_session.uuid,
                            &target_connection.ip.to_string(),
                            std::time::Duration::ZERO,
                        );
                        let _ = admin.persist();
                        // Dropping the sender makes the connection task exit.
                        connections.remove(&target_connection_id);
                        info!(
                            "Admin {} {} player {}",
                            player.name,
                            if action == 1 { "banned" } else { "kicked" },
                            target_session.name
                        );
                    }
                    2 => {
                        // Trace: TraceInfoCallPacket (134) with entity + trace.
                        let trace =
                            encode_trace_info_frame(&target_session, &target_connection, &admin)?;
                        if let Some(connection) = connections
                            .get(&(player.id - 1_000_000))
                            .map(|connection| connection.clone())
                        {
                            enqueue_outbound(&connection, trace, false);
                        }
                    }
                    3 => {
                        // wave: admins may skip the wave (no verification).
                        *world.game_state.wave_time.write() = 0.0;
                        info!("Admin {} skipped the wave", player.name);
                    }
                    4 => {
                        // switchTeam: the decoder already validated the
                        // params as one TypeIO object and requires a Team.
                        let crate::network::decoders::AdminRequestParams::Team(team) = params
                        else {
                            unreachable!("decoder validated the Team param");
                        };
                        if let Some(mut combat) = world.players.get_mut(&target_session.unit_id) {
                            combat.team = team;
                            let profile = combat.clone();
                            drop(combat);
                            world
                                .player_profiles
                                .insert(target_session.uuid.clone(), profile.clone());
                        }
                        // P0-05: a team change propagates to the unit the
                        // player possesses — `PlayerComp.team(Team)` sets
                        // `unit.team(team)` (PlayerComp.java:266-271) and
                        // `PlayerComp.update` re-asserts it every tick while
                        // possessed (PlayerComp.java:224-225). Possession
                        // itself survives the change.
                        if let crate::network::world::ControlledUnit::Standard(controlled_id) =
                            target_session.controlled_unit
                        {
                            if let Some(mut unit) = world.enemies.get_mut(&controlled_id) {
                                unit.team = team;
                            }
                        }
                        if let Ok(snapshot) = encode_initial_entity_snapshot(
                            &target_session,
                            world.players.get(&target_session.unit_id).as_deref(),
                        ) {
                            if let Ok(frame) =
                                frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true)
                            {
                                broadcast(&connections, frame);
                            }
                        }
                        world.persistence_dirty.store(true, Ordering::Relaxed);
                        info!(
                            "Admin {} switched {} to team {}",
                            player.name, target_session.name, team
                        );
                    }
                    _ => unreachable!("action ordinal validated by decoder"),
                }
            }
            Ok(Packet::Unknown {
                id: SET_PLAYER_TEAM_EDITOR_PACKET_ID,
                payload,
            }) if joined => {
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "SetPlayerTeamEditor before ConnectPacket",
                    )
                })?;
                // Official handleServer: HudFragment.setPlayerTeamEditor +
                // `setPlayerTeamEditor__forward(con, player, team)` — the
                // editor team visualization is broadcast to other clients.
                let team = decode_set_player_team_editor(&payload)?;
                let mut forward = Vec::new();
                forward.extend_from_slice(&player.id.to_be_bytes());
                forward.push(team);
                let frame =
                    frame_generated_packet(SET_PLAYER_TEAM_EDITOR_PACKET_ID, &forward, false)?;
                broadcast_except(&connections, player.id, frame);
            }
            Ok(Packet::Unknown {
                id: CLIENT_LOGIC_DATA_RELIABLE_PACKET_ID | CLIENT_LOGIC_DATA_UNRELIABLE_PACKET_ID,
                payload,
            }) if joined => {
                // Official NetServer.clientLogicData*: mod hook; vanilla has
                // no registered channel handlers, so this is a validated
                // no-op on the official server as well.
                let (channel, _value) = decode_client_logic_data(&payload)?;
                debug!(
                    "ClientLogicData channel '{}' from {} (no vanilla handler)",
                    channel, peer
                );
            }
            Ok(Packet::Unknown {
                id: REQUEST_BUILD_PAYLOAD_PACKET_ID,
                payload,
            }) if joined => {
                // M5: official InputHandler.requestBuildPayload (JAR
                // bytecode offsets 0-105): range check first —
                // `unit.within(build, tilesize*size*1.2f + tilesize*5f)`
                // (offsets 26-58) — then `allowAction(player, pickupBlock,
                // build.tile)` (offsets 59-105).
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "RequestBuildPayload before ConnectPacket",
                    )
                })?;
                let position = decode_request_build_payload(&payload)?;
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::PickupBlock,
                    Some(position),
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                let frames = apply_request_build_payload(&world, player, position);
                if !frames.is_empty() {
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                for frame in frames {
                    broadcast(&connections, frame);
                }
            }
            Ok(Packet::Unknown {
                id: REQUEST_DROP_PAYLOAD_PACKET_ID,
                payload,
            }) if joined => {
                // M5: official InputHandler.dropPayload runs
                // `admins.allowAction(player, ActionType.dropPayload, tile)`
                // (JAR bytecode offsets 0-74).
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "RequestDropPayload before ConnectPacket",
                    )
                })?;
                let (x, y) = decode_request_drop_payload(&payload)?;
                // World -> tile coordinates (World.toTile = floor(v/8)).
                let tile_x = (x / 8.0).floor() as i32;
                let tile_y = (y / 8.0).floor() as i32;
                let tile_pos = (tile_x << 16) | tile_y;
                match session_action_allowed(
                    &admin,
                    player,
                    &connections,
                    crate::state::administration::ActionType::DropPayload,
                    Some(tile_pos),
                    None,
                    None,
                ) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(frame) => {
                        writer.write_all(&frame).await?;
                        return Err(Error::new(
                            ErrorKind::ConnectionAborted,
                            "kicked by anti-spam",
                        ));
                    }
                }
                let frames = apply_request_drop_payload(&world, player, x, y);
                if !frames.is_empty() {
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                for frame in frames {
                    broadcast(&connections, frame);
                }
            }
            Ok(Packet::Unknown {
                id: REQUEST_UNIT_PAYLOAD_PACKET_ID,
                payload,
            }) if joined => {
                // 158.1 InputHandler.requestUnitPayload has no admin gate.
                let player = session_player.as_ref().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidData,
                        "RequestUnitPayload before ConnectPacket",
                    )
                })?;
                let (_unit_type, id) = decode_request_unit_payload(&payload)?;
                let frames = apply_request_unit_payload(&world, player, id);
                if !frames.is_empty() {
                    world.persistence_dirty.store(true, Ordering::Relaxed);
                }
                for frame in frames {
                    broadcast(&connections, frame);
                }
            }
            Ok(Packet::Unknown {
                id:
                    SERVER_BINARY_PACKET_RELIABLE_PACKET_ID | SERVER_BINARY_PACKET_UNRELIABLE_PACKET_ID,
                payload,
            }) if joined => {
                let (packet_type, _contents) = decode_server_packet(&payload, true)?;
                debug!(
                    "ServerBinaryPacket '{}' from {} (no custom handler)",
                    packet_type, peer
                );
            }
            Ok(Packet::Unknown {
                id: SERVER_PACKET_RELIABLE_PACKET_ID | SERVER_PACKET_UNRELIABLE_PACKET_ID,
                payload,
            }) if joined => {
                let (packet_type, _contents) = decode_server_packet(&payload, false)?;
                debug!(
                    "ServerPacket '{}' from {} (no custom handler)",
                    packet_type, peer
                );
            }
            Ok(packet) => {
                // PROTOCOL-RULES rule 9: unknown/unhandled packets are
                // diagnosed (wire id, peer, phase, payload length) instead
                // of being silently dropped. Deliberate deviation: the
                // official server closes the connection on unknown ids
                // (arc.net.Server -> Connection.close); we warn and keep
                // the session alive so corrupt/modded/future clients do not
                // kill the transport (documented in PROTOCOL-RULES rule 9).
                let (label, len) = packet_id_label(&packet);
                warn!(
                    "Unhandled packet id {} from {} (phase: {}, {} bytes) — dropped",
                    label,
                    peer,
                    if joined { "joined" } else { "pre-join" },
                    len
                );
            }
            Err(err) => return Err(err),
        }
    }
}
