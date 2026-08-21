//! Session replay/spawn/outbound transport helpers. Session facade re-exports
//! through crate::network::session::*.

use crate::network::codec::Reads;
use crate::network::listener::*;
use crate::network::wire::encode::encode_state_snapshot_for;
use crate::network::world::*;

use super::*;

pub async fn read_frame(reader: &mut OwnedReadHalf) -> std::io::Result<Vec<u8>> {
    let len = reader.read_u16().await? as usize;
    if len == 0 || len > MAX_PACKET_SIZE {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "invalid ArcNet frame length",
        ));
    }
    let mut frame = vec![0; len];
    let mut received = 0;
    while received < len {
        match reader.read(&mut frame[received..]).await {
            Ok(0) => {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    format!(
                        "peer closed an ArcNet frame after {received}/{len} bytes (prefix={:02x?})",
                        &frame[..received.min(4)]
                    ),
                ));
            }
            Ok(amount) => received += amount,
            Err(err) => return Err(err),
        }
    }
    Ok(frame)
}

/// ArcNet framework Ping frame: [u16 len=7][b -2][b 0][i32 id][b isReply=0].
/// (len counts the bytes AFTER the u16 prefix: 1 id + 1 type + 4 id + 1 flag.)
pub fn framework_ping(id: i32) -> [u8; 9] {
    let mut frame = [0u8; 9];
    frame[0..2].copy_from_slice(&7u16.to_be_bytes());
    frame[2] = FRAMEWORK_MESSAGE_ID as u8;
    frame[3] = 0; // FrameworkMessage.Ping
    frame[4..8].copy_from_slice(&id.to_be_bytes());
    frame
}

/// ArcNet framework Ping reply: [u16 len=7][b -2][b 0][i32 id][b isReply=1].
pub fn framework_ping_reply(id: i32) -> [u8; 9] {
    let mut frame = framework_ping(id);
    frame[8] = 1;
    frame
}

/// Serializes a compressed world stream into the ArcNet `StreamBegin`
/// (packet id 0, stream type `Net.packetIdWorldStream` = 2) and `StreamChunk`
/// (packet id 1) frames. Used both for the initial connect handshake and for
/// re-streaming connected players after a hot map swap (frames are queued
/// through the per-connection outbound channel).
pub fn world_stream_frames(world: &[u8]) -> std::io::Result<Vec<Vec<u8>>> {
    let stream_id = 1i32;
    let mut begin = Vec::with_capacity(9);
    begin.extend_from_slice(&stream_id.to_be_bytes());
    begin.extend_from_slice(&(world.len() as i32).to_be_bytes());
    begin.push(2); // Net.packetIdWorldStream
    let mut frames = Vec::new();
    let mut frame = Vec::new();
    write_tcp_packet(&mut frame, 0, &begin, false)?;
    frames.push(frame);

    for chunk in world.chunks(1024) {
        let mut payload = Vec::with_capacity(chunk.len() + 6);
        payload.extend_from_slice(&stream_id.to_be_bytes());
        payload.extend_from_slice(&(chunk.len() as i16).to_be_bytes());
        payload.extend_from_slice(chunk);
        let mut frame = Vec::new();
        write_tcp_packet(&mut frame, 1, &payload, false)?;
        frames.push(frame);
    }
    Ok(frames)
}

pub async fn send_world_stream(socket: &mut OwnedWriteHalf, world: &[u8]) -> std::io::Result<()> {
    for frame in world_stream_frames(world)? {
        socket.write_all(&frame).await?;
    }
    Ok(())
}

pub async fn send_generated_packet(
    writer: &mut OwnedWriteHalf,
    packet_id: u8,
    payload: &[u8],
    compress: bool,
) -> std::io::Result<()> {
    let frame = frame_generated_packet(packet_id, payload, compress)?;
    writer.write_all(&frame).await
}

pub async fn send_generated_packet_prefer_udp(
    udp: &Arc<UdpSocket>,
    endpoint: Option<SocketAddr>,
    writer: &mut OwnedWriteHalf,
    packet_id: u8,
    payload: &[u8],
    compress: bool,
) -> std::io::Result<()> {
    let _ = compress;
    let compress = crate::network::codec::should_lz4_compress(packet_id, payload.len());
    if let Some(endpoint) = endpoint {
        let mut datagram = Vec::with_capacity(payload.len() + 16);
        crate::network::codec::write_serialized_packet(&mut datagram, packet_id, payload)?;
        if udp.send_to(&datagram, endpoint).await.is_ok() {
            return Ok(());
        }
    }
    send_generated_packet(writer, packet_id, payload, compress).await
}

#[allow(clippy::too_many_arguments)]
pub fn encode_all_player_snapshots(world: &DynamicWorld) -> std::io::Result<Vec<Vec<u8>>> {
    let mut sessions: Vec<_> = world
        .player_sessions
        .iter()
        .map(|session| session.value().clone())
        .collect();
    sessions.sort_unstable_by_key(|session| session.unit_id);
    sessions
        .iter()
        .map(|session| {
            let combat = world.players.get(&session.unit_id);
            encode_initial_entity_snapshot(session, combat.as_deref())
        })
        .collect()
}

pub async fn send_player_spawn(
    writer: &mut OwnedWriteHalf,
    player: &SessionPlayer,
    world: &DynamicWorld,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    // Snapshot the player combat state into an owned value first: a DashMap
    // `players` Ref held across `.await` (or across the packet sends) would
    // keep a shard lock suspended indefinitely and can deadlock resumptions
    // against the same shard.
    let combat = world.players.get(&player.unit_id).map(|c| c.clone());
    // PlayerSpawnCall requires a CoreBuild tile. Sending a persisted player
    // coordinate makes the official client ignore the spawn entirely. In
    // PvP the spawn tile is the player's OWN team's core.
    let team = combat.as_ref().map(|state| state.team).unwrap_or(1);
    let tile_position = core_position_for_team(world, team);
    let mut spawn = Vec::with_capacity(8);
    spawn.write_i(tile_position)?;
    spawn.write_i(player.id)?;
    send_generated_packet(writer, PLAYER_SPAWN_PACKET_ID, &spawn, false).await?;

    let snapshot = encode_initial_entity_snapshot(player, combat.as_ref())?;
    send_generated_packet(writer, ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true).await
}

pub async fn send_all_player_snapshots(
    writer: &mut OwnedWriteHalf,
    world: &DynamicWorld,
) -> std::io::Result<()> {
    for snapshot in encode_all_player_snapshots(world)? {
        send_generated_packet(writer, ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true).await?;
    }
    Ok(())
}

pub async fn send_state_snapshot(
    writer: &mut OwnedWriteHalf,
    state: &crate::state::game_state::GameState,
    world: &DynamicWorld,
) -> std::io::Result<()> {
    let payload = encode_state_snapshot_for(state, Some(world))?;
    send_generated_packet(writer, STATE_SNAPSHOT_PACKET_ID, &payload, true).await
}

pub(crate) async fn replay_active_projectiles(
    writer: &mut OwnedWriteHalf,
    world: &DynamicWorld,
) -> std::io::Result<()> {
    let mut projectiles: Vec<_> = world
        .projectiles
        .iter()
        .map(|projectile| (*projectile.key(), projectile.value().clone()))
        .collect();
    projectiles.sort_unstable_by_key(|(id, _)| *id);
    for (_, projectile) in projectiles {
        let target = (projectile.team == 1)
            .then(|| {
                world
                    .enemies
                    .get(&projectile.target_id)
                    .map(|enemy| (enemy.x, enemy.y))
            })
            .flatten();
        let payload =
            crate::network::combat::encode_projectile_replay_payload(&projectile, target)?;
        send_generated_packet(writer, CREATE_BULLET_PACKET_ID, &payload, false).await?;
    }
    Ok(())
}

pub async fn replay_dynamic_tiles(
    writer: &mut OwnedWriteHalf,
    player: &SessionPlayer,
    tiles: &DashMap<i32, DynamicTile>,
) -> std::io::Result<()> {
    let mut snapshot: Vec<_> = tiles.iter().map(|tile| tile.value().clone()).collect();
    snapshot.sort_unstable_by_key(|tile| tile.position);
    for tile in snapshot {
        if tile.block == 0 {
            let mut payload = Vec::new();
            crate::network::codec::Writes::write_i(&mut payload, tile.position)?;
            send_generated_packet(writer, REMOVE_TILE_PACKET_ID, &payload, false).await?;
            continue;
        }
        let team = tile.team;
        let plan = BuildPlan {
            breaking: false,
            position: tile.position,
            block: tile.block,
            rotation: tile.rotation,
            config: tile.config,
        };
        let payload = encode_construct_finish(player, &plan, tile.rotation, team)?;
        send_generated_packet(writer, CONSTRUCT_FINISH_PACKET_ID, &payload, false).await?;
    }
    Ok(())
}

pub async fn replay_pending_builds(
    writer: &mut OwnedWriteHalf,
    player: &SessionPlayer,
    pending_builds: &DashMap<i32, PendingBuild>,
) -> std::io::Result<()> {
    let mut snapshot: Vec<_> = pending_builds
        .iter()
        .map(|build| build.value().clone())
        .collect();
    snapshot.sort_unstable_by_key(|build| build.position);
    for pending in snapshot {
        let payload = encode_begin_place(player, &pending)?;
        send_generated_packet(writer, BEGIN_PLACE_PACKET_ID, &payload, false).await?;
    }
    Ok(())
}

pub async fn replay_pending_breaks(
    writer: &mut OwnedWriteHalf,
    player: &SessionPlayer,
    pending_breaks: &DashMap<i32, PendingBreak>,
) -> std::io::Result<()> {
    let mut positions: Vec<_> = pending_breaks
        .iter()
        .map(|pending| pending.position)
        .collect();
    positions.sort_unstable();
    for position in positions {
        let payload = encode_begin_break(player, position)?;
        send_generated_packet(writer, BEGIN_BREAK_PACKET_ID, &payload, false).await?;
    }
    Ok(())
}

/// Wire id + payload length of a packet for diagnostics (PROTOCOL-RULES
/// rule 9). `Packet::read` decodes ids 0/1/2/3 into typed variants, so the
/// label maps them back explicitly (the raw id is not stored in them);
/// without this the generic unhandled arm logged an empty id for
/// StreamBegin/StreamChunk/WorldStream (adversarial QA finding H3).
pub(crate) fn packet_id_label(packet: &Packet) -> (String, usize) {
    match packet {
        Packet::Unknown { id, payload } => (id.to_string(), payload.len()),
        Packet::StreamBegin(_) => ("0".into(), 0),
        Packet::StreamChunk(p) => ("1".into(), p.data.len()),
        Packet::WorldStream(_) => ("2".into(), 0),
        Packet::ConnectPacket(_) => ("3".into(), 0),
        _ => (String::new(), 0),
    }
}
