//! Block/state/entity snapshot and unit-spawn/plan wire encoders. The
//! listener adapter re-exports these through crate::network::listener::*.

use crate::network::codec::{write_tcp_packet, Writes};
use crate::network::decoders::BuildPlan;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::network::world::*;

use crate::network::buildings::plans::assist_visual_plan;
use crate::network::buildings::snapshot::{
    encode_dynamic_tile_sync, encode_factory_snapshot_tiles, is_batch_snapshot_supported,
    is_block_snapshot_supported, is_core_block, is_snapshot_item_turret,
};
use crate::network::economy::spec::{storage_capacity, MONO_MINE_RANGE};
use crate::network::units::controller::{controlling_session_for_unit, valid_item_stack};
use crate::network::wire::persistence::outbound_typeio_object;
use crate::network::wire::transfer::enemy_weapon_mount_count;
use crate::network::wire::transfer::nearest_opposing_unit;

use std::io::{Error, ErrorKind};
use std::sync::atomic::Ordering;

use crate::state::game_state::{GameMode, GameState};

pub(crate) fn encode_block_snapshots(
    world: &DynamicWorld,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<Vec<Vec<u8>>> {
    encode_block_snapshots_with_threshold(
        world,
        power,
        crate::network::parallel::SNAPSHOT_PARALLEL_THRESHOLD,
    )
    .map(|(frames, _)| frames)
}

pub(crate) fn encode_block_snapshots_with_threshold(
    world: &DynamicWorld,
    power: &std::collections::HashMap<i32, f32>,
    parallel_threshold: usize,
) -> std::io::Result<(Vec<Vec<u8>>, crate::network::parallel::ParallelExecution)> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let mut tiles: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| is_batch_snapshot_supported(tile.block))
        .map(|tile| tile.value().clone())
        .collect();
    tiles.sort_unstable_by_key(|tile| tile.position);

    // Snapshot DashMap values before entering Rayon. Codecs that need live
    // topology/targets remain sequential; all other codecs receive only an
    // owned DynamicTile clone plus the immutable power HashMap, so no DashMap
    // guard or world lock crosses into the worker pool.
    let mut encoded: Vec<Option<std::io::Result<Vec<u8>>>> =
        std::iter::repeat_with(|| None).take(tiles.len()).collect();
    let mut parallel_indexes = Vec::new();
    for (index, tile) in tiles.iter().enumerate() {
        if block_snapshot_requires_world(tile.block) {
            encoded[index] = Some(encode_block_snapshot_entry(tile, power, Some(world)));
        } else {
            parallel_indexes.push(index);
        }
    }
    let mapped =
        crate::network::parallel::map_ordered(&parallel_indexes, parallel_threshold, |index| {
            encode_block_snapshot_entry(&tiles[*index], power, None)
        });
    let execution = mapped.execution;
    for (index, entry) in parallel_indexes.into_iter().zip(mapped.values) {
        encoded[index] = Some(entry);
    }
    let entries: Vec<Vec<u8>> = encoded
        .into_iter()
        .map(|entry| {
            entry
                .ok_or_else(|| Error::other("missing encoded block snapshot entry"))
                .and_then(|entry| entry)
        })
        .collect::<std::io::Result<_>>()?;

    Ok((batch_block_snapshot_entries(&entries)?, execution))
}

/// These codecs read topology, team inventories, simulation time or live
/// targets. Keeping them on the caller thread avoids lock contention and makes
/// the Rayon closure a pure transform over captured data.
pub(crate) fn block_snapshot_requires_world(block: i16) -> bool {
    matches!(
        block,
        261 | 262 | 263 | 271 | 293 | 294 | 302..=304 | 402 | 403 | 410
    ) || is_snapshot_item_turret(block)
        || matches!(block, 353 | 354 | 355 | 360 | 366 | 369 | 372 | 373 | 376)
        || storage_capacity(block).is_some()
}

pub(crate) fn encode_block_snapshot_entry(
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
    world: Option<&DynamicWorld>,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let mut entry = Vec::new();
    entry.write_i(tile.position)?;
    entry.write_s(tile.block)?;
    encode_dynamic_tile_sync(&mut entry, tile, power, world)?;
    Ok(entry)
}

pub(crate) fn batch_block_snapshot_entries(entries: &[Vec<u8>]) -> std::io::Result<Vec<Vec<u8>>> {
    let mut batches: Vec<Vec<u8>> = Vec::new();
    let mut current_count = 0usize;
    let mut current_bytes = 0usize;
    let mut current_data: Vec<u8> = Vec::new();
    // Official NetServer.writeBlockSnapshots cuts a batch once its size
    // exceeds maxSnapshotSize (800 B); the client reads block snapshots
    // sequentially, so an over-large batch (memory banks, processors,
    // payloads) would also exceed the 32 KiB ArcNet frame limit and be
    // silently dropped. Batch by bytes, not by a fixed tile count.
    const MAX_BATCH_BYTES: usize = 800;
    for entry in entries {
        if current_count > 0 && current_bytes + entry.len() > MAX_BATCH_BYTES {
            batches.push(finish_block_snapshot_batch(current_count, &current_data)?);
            current_count = 0;
            current_data.clear();
            current_bytes = 0;
        }
        current_bytes += entry.len();
        current_data.extend_from_slice(entry);
        current_count += 1;
    }
    if current_count > 0 {
        batches.push(finish_block_snapshot_batch(current_count, &current_data)?);
    }
    Ok(batches)
}

/// Frames one completed block-snapshot batch (round 74g: extracted from
/// encode_factory_snapshot_tiles so the single-pass encoder can reuse the
/// exact already-serialized bytes).
pub(crate) fn finish_block_snapshot_batch(amount: usize, data: &[u8]) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let amount = i16::try_from(amount)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "too many block snapshots"))?;
    let data_len = i16::try_from(data.len())
        .map_err(|_| Error::new(ErrorKind::InvalidData, "block snapshot is too large"))?;
    let mut payload = Vec::with_capacity(data.len() + 4);
    payload.write_s(amount)?;
    payload.write_s(data_len)?; // TypeIO.writeBytes
    payload.extend_from_slice(data);
    frame_generated_packet(BLOCK_SNAPSHOT_PACKET_ID, &payload, false)
}

pub(crate) fn encode_construct_block_snapshot(
    position: i32,
    target_block: i16,
    rotation: u8,
    team: u8,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let size = crate::game::content::block_size(target_block).clamp(1, 16);
    let construct_block = 4 + i16::from(size); // build1..build16 are IDs 5..20.
    let mut data = Vec::new();
    data.write_i(position)?;
    data.write_s(construct_block)?;
    data.write_f(crate::game::content::block_health(target_block).max(1.0))?;
    data.write_b((rotation % 4) | 128)?;
    data.write_b(team)?;
    data.write_b(3)?;
    data.write_b(1)?;
    data.write_b(0)?;
    data.write_b(255)?;
    data.write_b(255)?;
    let mut payload = Vec::with_capacity(data.len() + 4);
    payload.write_s(1)?;
    payload
        .write_s(i16::try_from(data.len()).map_err(|_| {
            Error::new(ErrorKind::InvalidData, "construct snapshot is too large")
        })?)?;
    payload.extend_from_slice(&data);
    frame_generated_packet(BLOCK_SNAPSHOT_PACKET_ID, &payload, false)
}

pub(crate) fn encode_block_snapshot(
    world: &DynamicWorld,
    tile: &DynamicTile,
    power: &std::collections::HashMap<i32, f32>,
) -> std::io::Result<Option<Vec<u8>>> {
    // The individual RequestBlockSnapshot reply covers every existing
    // building, including cores (339-344) that the periodic batch skips.
    if !is_block_snapshot_supported(tile.block) && !is_core_block(tile.block) {
        return Ok(None);
    }
    encode_factory_snapshot_tiles(std::slice::from_ref(tile), power, Some(world)).map(Some)
}

/// Encodes a BlockSnapshotCallPacket payload batch for the given tiles.
/// `pub` so integration tests (tests/snapshot_parity_tests.rs) can verify the
/// official layouts; the server path is unchanged.
pub fn encode_state_snapshot() -> std::io::Result<Vec<u8>> {
    encode_state_snapshot_for(&GameState::new(), None)
}

pub(crate) fn encode_state_snapshot_for(
    state: &GameState,
    world: Option<&DynamicWorld>,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let mut payload = Vec::with_capacity(48);
    payload.write_f(*state.wave_time.read())?;
    payload.write_i(state.wave.load(Ordering::Relaxed) as i32)?;
    payload.write_i(state.enemies_count.load(Ordering::Relaxed) as i32)?;
    payload.write_bool(state.is_paused.load(Ordering::Relaxed))?;
    payload.write_bool(state.game_over.load(Ordering::Relaxed))?;
    payload.write_i(0)?;
    payload.write_b(60)?;
    payload.write_l(0)?;
    payload.write_l(0)?;
    // coreData mirrors `StateSnapshotCallPacket` (158.1): b teamCount, then
    // per team `b team` + an ItemModule (official `NetServer.writeStateSnapshot`:
    // `dataStream.writeByte(activeTeams); for(TeamData data : teams.present)
    // { writeByte(data.team.id); data.cores.first().items.write(...); }`).
    // Each team reports ITS OWN core inventory — team 1 is the legacy
    // `GameState.core_items`, PvP teams their `team_items` entry.
    let active_teams = state_snapshot_teams(state, world);
    let mut core_data = Vec::with_capacity(4 + active_teams.len() * 8);
    core_data.write_b(u8::try_from(active_teams.len()).map_err(Error::other)?)?;
    for team in active_teams {
        core_data.write_b(team)?;
        let team_items: Vec<i32> = if team == 1 {
            state.core_items.read().clone()
        } else {
            state
                .team_items
                .get(&team)
                .map(|items| items.clone())
                .unwrap_or_else(|| vec![0; 22])
        };
        let team_present = team_items.iter().filter(|amount| **amount > 0).count();
        core_data.write_s(i16::try_from(team_present).map_err(Error::other)?)?;
        for (id, amount) in team_items
            .iter()
            .enumerate()
            .filter(|(_, amount)| **amount > 0)
        {
            core_data.write_s(i16::try_from(id).map_err(Error::other)?)?;
            core_data.write_i(*amount)?;
        }
    }
    payload.write_s(i16::try_from(core_data.len()).map_err(Error::other)?)?;
    payload.extend_from_slice(&core_data);
    Ok(payload)
}

/// Teams reported in the StateSnapshot coreData: team 1 (the map core) plus,
/// in PvP, every team that currently has a connected player (the official
/// `NetServer` writes `state.teams.getActive()` for all core-bearing teams).
pub(crate) fn state_snapshot_teams(state: &GameState, world: Option<&DynamicWorld>) -> Vec<u8> {
    let Some(world) = world else {
        return vec![1];
    };
    let registered = crate::network::world::registered_core_teams(world);
    // Legacy/custom worlds without an MSAV core have no Teams.active entry;
    // preserve the port's compatibility snapshot using connected teams.
    if registered.is_empty() {
        let mut teams = vec![1u8];
        if *state.mode.read() == GameMode::Pvp {
            teams.extend(world.players.iter().map(|entry| entry.value().team));
        }
        teams.sort_unstable();
        teams.dedup();
        return teams;
    }
    let mut teams: Vec<u8> = registered
        .into_iter()
        .filter(|team| {
            *team != 0
                && crate::network::world::team_core_snapshot(world, *team)
                    .iter()
                    .any(|core| core.health > 0.0)
        })
        .collect();
    if *state.mode.read() != GameMode::Pvp && teams.is_empty() {
        teams.push(1);
    }
    teams.sort_unstable();
    teams.dedup();
    teams
}

pub(crate) fn encode_initial_entity_snapshot(
    player: &SessionPlayer,
    combat: Option<&PlayerCombatState>,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    // A possessed standard/block unit lives in the normal unit/building
    // snapshot stream. Do not keep emitting the player's old core Alpha: the
    // official server despawns it and Player.writeSync points at the possessed
    // TypeIO unit reference instead.
    let core_controlled = player.controlled_unit == ControlledUnit::Core;
    let world_x = if core_controlled {
        combat.map_or(player.x, |state| state.x)
    } else {
        player.x
    };
    let world_y = if core_controlled {
        combat.map_or(player.y, |state| state.y)
    } else {
        player.y
    };
    let health = combat.map_or(150.0, |state| state.health);
    let shield = combat.map_or(0.0, |state| state.shield);
    // PvP: the unit and player syncs carry the player's real team
    // (NetServer.assignTeam), not the hardcoded sharded team 1.
    let team = combat.map_or(1, |state| state.team);
    let mut entities = Vec::new();

    // UnitEntityLegacyAlpha / Syncc.writeSync.
    if core_controlled {
        entities.write_i(player.unit_id)?;
        entities.write_b(ALPHA_CLASS_ID)?;
        entities.write_b(0)?; // abilities
        entities.write_f(player.mouse_x)?; // aim X
        entities.write_f(player.mouse_y)?; // aim Y
        entities.write_b(0)?; // player controller
        entities.write_i(player.id)?;
        entities.write_f(1.0)?; // elevation
        entities.write_l(0)?; // flag (double 0 has the same wire representation)
        entities.write_f(health)?;
        entities.write_bool(player.shooting && health > 0.0)?;
        entities.write_i(player.mining_position.unwrap_or(-1))?;
        entities.write_b(1)?; // weapon mounts
        entities.write_b(0)?; // mount flags
        entities.write_f(world_x)?;
        entities.write_f(world_y)?;
        entities.write_i(0)?; // build plans
        entities.write_f(player.rotation)?;
        entities.write_f(shield)?;
        entities.write_bool(true)?; // spawned by core
        let (carried_item, carried_amount) =
            valid_item_stack(player.carried_item, player.carried_amount);
        entities.write_s(carried_item)?;
        entities.write_i(carried_amount)?;
        if let Some(state) = combat.filter(|state| state.status_effect >= 0) {
            entities.write_i(1)?;
            entities.write_s(state.status_effect)?;
            entities.write_f(state.status_duration)?;
        } else {
            entities.write_i(0)?;
        }
        entities.write_b(team)?;
        entities.write_s(ALPHA_CONTENT_ID)?;
        entities.write_bool(false)?; // update building
        entities.write_f(0.0)?; // velocity X
        entities.write_f(3.0)?; // velocity Y
        entities.write_f(world_x)?;
        entities.write_f(world_y)?;
    }

    // Player / Syncc.writeSync.
    entities.write_i(player.id)?;
    entities.write_b(PLAYER_CLASS_ID)?;
    entities.write_bool(player.admin)?; // admin
    entities.write_bool(player.boosting)?;
    entities.write_i(player.color)?;
    entities.write_f(player.mouse_x)?;
    entities.write_f(player.mouse_y)?;
    entities.write_typeio_string(Some(&player.name))?;
    entities.write_s(-1)?; // selected block
    entities.write_i(0)?; // selected rotation
    entities.write_bool(player.shooting && health > 0.0)?;
    entities.write_b(team)?; // player team
    entities.write_bool(false)?; // typing
    match player.controlled_unit {
        ControlledUnit::Core => {
            entities.write_b(2)?;
            entities.write_i(player.unit_id)?;
        }
        ControlledUnit::Standard(unit_id) => {
            entities.write_b(2)?;
            entities.write_i(unit_id)?;
        }
        ControlledUnit::Building(position) => {
            entities.write_b(1)?;
            entities.write_i(position)?;
        }
    }
    entities.write_f(world_x)?;
    entities.write_f(world_y)?;

    let data_len = i16::try_from(entities.len())
        .map_err(|_| Error::new(ErrorKind::InvalidData, "entity snapshot is too large"))?;
    let mut payload = Vec::with_capacity(entities.len() + 4);
    payload.write_s(if core_controlled { 2 } else { 1 })?;
    payload.write_s(data_len)?; // TypeIO.writeBytes
    payload.extend_from_slice(&entities);
    Ok(payload)
}

/// Official NetServer.maxSnapshotSize: entity sync data is flushed into
/// separate `entitySnapshot` packets once a batch exceeds 800 bytes (the
/// value is below the UDP safe limit since snapshots are also compressed).
/// Without batching, many concurrent units overflow the i16 data length of
/// `TypeIO.writeBytes` and the connection is dropped.
pub(crate) const ENEMY_SNAPSHOT_BATCH_BYTES: usize =
    crate::network::wire::outbound::MAX_SNAPSHOT_SIZE;

/// Encodes all enemy units as one or more `entitySnapshot` payloads, flushing
/// a new packet every `ENEMY_SNAPSHOT_BATCH_BYTES` like the official
/// `NetServer.writeEntitySnapshotsAll`. Returns an empty vec when no enemies
/// exist.
pub(crate) fn encode_enemy_entity_snapshots(world: &DynamicWorld) -> std::io::Result<Vec<Vec<u8>>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let enemies: Vec<_> = world.enemies.iter().map(|entry| entry.clone()).collect();
    // Authoritative puddles (round 73 A2): the official server syncs them in
    // the same entity snapshot stream (classId 13, `Puddle.writeSync` = f
    // amount, s liquid.id, TypeIO.writeTile (i packed pos), f x, f y). Sorted
    // by tile for deterministic batching.
    let puddles: Vec<_> = {
        let mut list: Vec<_> = world
            .puddles
            .puddles
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();
        list.sort_unstable_by_key(|(position, _)| *position);
        list
    };
    if enemies.is_empty() && puddles.is_empty() {
        return Ok(Vec::new());
    }
    let (core_x, core_y) = core_world(world);
    let mut payloads = Vec::new();
    let mut entities = Vec::with_capacity(96);
    let mut batch_count = 0usize;
    for enemy in enemies {
        let is_mono_miner = enemy.team == 1 && enemy.unit_type == MONO.unit_type;
        // Round 74: the mining beam target comes from the CACHED mining
        // target (world.mono_mining_targets) and only while the mono is
        // within its mineRange (70 u). The official MinerAI sets
        // unit.mineTile only then — the client draws the mining laser
        // whenever mineTile != null, so the legacy port (which always aimed
        // at the nearest ore, however far) showed a beam stretching across
        // the map. Reusing the cache also removes a per-snapshot ore scan.
        let mining_position = if is_mono_miner && enemy.secondary_attack_reload < 30.0 {
            world.mono_mining_targets.get(&enemy.id).and_then(|entry| {
                let (position, _) = *entry;
                if position == 0 {
                    return None;
                }
                let target_x = (position >> 16) as i16 as f32 * 8.0;
                let target_y = position as i16 as f32 * 8.0;
                ((target_x - enemy.x).hypot(target_y - enemy.y) <= MONO_MINE_RANGE)
                    .then_some(position)
            })
        } else {
            None
        };
        let controlling_player = controlling_session_for_unit(world, enemy.id);
        let (aim_x, aim_y) = if let Some(player) = controlling_player.as_ref() {
            (player.mouse_x, player.mouse_y)
        } else if let Some(position) = mining_position {
            (
                (position >> 16) as i16 as f32 * 8.0,
                position as i16 as f32 * 8.0,
            )
        } else if is_mono_miner {
            // Traveling/depositing mono: MineWeapon.update's non-mining
            // branch points the mount forward along the unit's rotation.
            let rotation = (enemy.rotation - 90.0).to_radians();
            (
                enemy.x + rotation.cos() * 20.0,
                enemy.y + rotation.sin() * 20.0,
            )
        } else if enemy.team == world.wave_rules.read().wave_team {
            (core_x, core_y)
        } else {
            nearest_opposing_unit(world, enemy.team, enemy.x, enemy.y)
                .map(|(_, x, y)| (x, y))
                .unwrap_or((enemy.x, enemy.y))
        };
        entities.write_i(enemy.id)?;
        entities.write_b(enemy.entity_class)?;
        let assist_plan = assist_visual_plan(world, &enemy);
        write_unit_sync(
            &mut entities,
            Some(world),
            &enemy,
            aim_x,
            aim_y,
            mining_position,
            assist_plan.as_ref(),
        )?;
        batch_count += 1;
        if entities.len() > ENEMY_SNAPSHOT_BATCH_BYTES {
            let data_len = i16::try_from(entities.len())
                .map_err(|_| Error::new(ErrorKind::InvalidData, "enemy snapshot is too large"))?;
            let amount = i16::try_from(batch_count)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "too many enemy entities"))?;
            let mut payload = Vec::with_capacity(entities.len() + 4);
            payload.write_s(amount)?;
            payload.write_s(data_len)?;
            payload.extend_from_slice(&entities);
            payloads.push(payload);
            entities.clear();
            batch_count = 0;
        }
    }
    // Puddles ride the same batched stream after the units (official
    // Groups.sync iteration; entity order is irrelevant to the client).
    for (position, puddle) in &puddles {
        let x = ((position >> 16) as f32 * 8.0) + 4.0;
        let y = ((position & 0xFFFF) as f32 * 8.0) + 4.0;
        entities.write_i(puddle.entity_id)?;
        entities.write_b(PUDDLE_ENTITY_CLASS_ID)?;
        write_puddle_sync(&mut entities, puddle, *position, x, y)?;
        batch_count += 1;
        if entities.len() > ENEMY_SNAPSHOT_BATCH_BYTES {
            let data_len = i16::try_from(entities.len())
                .map_err(|_| Error::new(ErrorKind::InvalidData, "enemy snapshot is too large"))?;
            let amount = i16::try_from(batch_count)
                .map_err(|_| Error::new(ErrorKind::InvalidData, "too many enemy entities"))?;
            let mut payload = Vec::with_capacity(entities.len() + 4);
            payload.write_s(amount)?;
            payload.write_s(data_len)?;
            payload.extend_from_slice(&entities);
            payloads.push(payload);
            entities.clear();
            batch_count = 0;
        }
    }
    if batch_count > 0 {
        let data_len = i16::try_from(entities.len())
            .map_err(|_| Error::new(ErrorKind::InvalidData, "enemy snapshot is too large"))?;
        let amount = i16::try_from(batch_count)
            .map_err(|_| Error::new(ErrorKind::InvalidData, "too many enemy entities"))?;
        let mut payload = Vec::with_capacity(entities.len() + 4);
        payload.write_s(amount)?;
        payload.write_s(data_len)?;
        payload.extend_from_slice(&entities);
        payloads.push(payload);
    }
    Ok(payloads)
}

/// `mindustry.gen.Puddle.classId()` in desktop.jar 158.1 (EntityMapping
/// idMap index 13, verified with a live JVM probe).
pub(crate) const PUDDLE_ENTITY_CLASS_ID: u8 = 13;

/// Official `Puddle.writeSync(Writes)` (158.1): `f amount, s liquid.id
/// (null = -1), TypeIO.writeTile (i packed pos), f x, f y`.
pub(crate) fn write_puddle_sync(
    output: &mut Vec<u8>,
    puddle: &crate::network::buildings::puddles::PuddleState,
    position: i32,
    x: f32,
    y: f32,
) -> std::io::Result<()> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;
    output.write_f(puddle.amount)?;
    output.write_s(puddle.liquid)?;
    output.write_i(position)?;
    output.write_f(x)?;
    output.write_f(y)?;
    Ok(())
}

pub(crate) fn encode_unit_spawn_payload(
    world: &DynamicWorld,
    unit: &EnemyUnit,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let (core_x, core_y) = core_world_for_team(world, world.wave_rules.read().default_team);
    let (aim_x, aim_y) = if unit.team == world.wave_rules.read().wave_team {
        (core_x, core_y)
    } else {
        nearest_opposing_unit(world, unit.team, unit.x, unit.y)
            .map(|(_, x, y)| (x, y))
            .unwrap_or((unit.x, unit.y))
    };
    let mut payload = Vec::with_capacity(100);
    payload.write_i(unit.id)?;
    payload.write_b(unit.entity_class)?;
    write_unit_sync(&mut payload, Some(world), unit, aim_x, aim_y, None, None)?;
    Ok(payload)
}

pub(crate) fn write_unit_plans_queue(
    output: &mut Vec<u8>,
    plans: &[crate::network::world::UnitBuildPlan],
    network: bool,
) -> std::io::Result<()> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;
    let used = if network {
        max_synced_plans(plans)
    } else {
        plans.len()
    };
    output.write_i(i32::try_from(used).unwrap_or(i32::MAX))?;
    for plan in plans.iter().take(used) {
        output.write_b(u8::from(plan.breaking))?;
        output.write_i(plan.position)?;
        if !plan.breaking {
            output.write_s(plan.block)?;
            output.write_b(plan.rotation)?;
            output.write_b(1)?;
            output.extend_from_slice(&outbound_typeio_object(&plan.config));
        }
    }
    Ok(())
}

use crate::network::world::*;

use super::*;

pub(crate) fn frame_generated_packet(
    packet_id: u8,
    payload: &[u8],
    compress: bool,
) -> std::io::Result<Vec<u8>> {
    let _ = compress;
    let mut frame = Vec::new();
    write_tcp_packet(
        &mut frame,
        packet_id,
        payload,
        crate::network::codec::should_lz4_compress(packet_id, payload.len()),
    )?;
    Ok(frame)
}

/// Encodes the reliable/unreliable `DebugStatusClient` calls sent in response
/// to `RequestDebugStatus`. `DebugStatusClientCallPacket.write` always emits
/// three ints: flags, the last client snapshot acknowledged by the server, and
/// `snapshotsSent` (which defaults to zero in NetServer's request path).
pub(crate) fn encode_debug_status_client(
    packet_id: u8,
    flags: i32,
    last_client_snapshot: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    if !matches!(
        packet_id,
        DEBUG_STATUS_CLIENT_PACKET_ID | DEBUG_STATUS_CLIENT_UNRELIABLE_PACKET_ID
    ) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "invalid debug status packet id",
        ));
    }
    let mut payload = Vec::with_capacity(12);
    payload.write_i(flags)?;
    payload.write_i(last_client_snapshot)?;
    payload.write_i(0)?; // snapshotsSent: NetServer does not populate it.
    frame_generated_packet(packet_id, &payload, false)
}

/// Queue a building health sample. Later samples for the same tile replace
/// earlier ones so N hits inside `HEALTH_SYNC_INTERVAL` become one RPC.
pub(crate) fn coalesce_build_health(
    pending: &mut std::collections::HashMap<i32, f32>,
    position: i32,
    health: f32,
) {
    pending.insert(position, health);
}

pub(crate) fn take_coalesced_build_health(
    pending: &mut std::collections::HashMap<i32, f32>,
) -> Vec<(i32, f32)> {
    let mut updates: Vec<_> = pending.drain().collect();
    updates.sort_unstable_by_key(|(position, _)| *position);
    updates
}

pub(crate) fn encode_build_health_update_frame(updates: &[(i32, f32)]) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let value_count = updates
        .len()
        .checked_mul(2)
        .and_then(|count| i32::try_from(count).ok())
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "too many building health updates"))?;
    let mut payload = Vec::with_capacity(4 + updates.len() * 8);
    payload.write_i(value_count)?;
    for (position, health) in updates {
        payload.write_i(*position)?;
        payload.write_i(health.to_bits() as i32)?;
    }
    frame_generated_packet(BUILD_HEALTH_UPDATE_PACKET_ID, &payload, false)
}

pub(crate) fn encode_build_destroyed_frame(position: i32) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let mut payload = Vec::with_capacity(4);
    payload.write_i(position)?;
    frame_generated_packet(BUILD_DESTROYED_PACKET_ID, &payload, false)
}

pub(crate) fn encode_player_disconnect_frames(
    player: &SessionPlayer,
) -> std::io::Result<[Vec<u8>; 2]> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    let mut disconnect = Vec::with_capacity(4);
    disconnect.write_i(player.id)?;
    let disconnect = frame_generated_packet(PLAYER_DISCONNECT_PACKET_ID, &disconnect, false)?;

    let mut despawn = Vec::with_capacity(5);
    despawn.write_b(2)?; // TypeIO standard unit reference
    despawn.write_i(player.unit_id)?;
    let despawn = frame_generated_packet(UNIT_DESPAWN_PACKET_ID, &despawn, false)?;
    Ok([disconnect, despawn])
}

/// Official `TypeIO.getMaxPlans`: cap 20, stop early when config bytes > 500.
pub(crate) fn max_synced_plans(plans: &[crate::network::world::UnitBuildPlan]) -> usize {
    let mut used = plans.len().min(20);
    let mut total = 0usize;
    for (index, plan) in plans.iter().take(used).enumerate() {
        total += plan.config.len();
        if total > 500 {
            used = index + 1;
            break;
        }
    }
    used
}

pub(crate) fn write_unit_sync(
    output: &mut Vec<u8>,
    world: Option<&DynamicWorld>,
    unit: &EnemyUnit,
    aim_x: f32,
    aim_y: f32,
    mining_position: Option<i32>,
    build_plan: Option<&BuildPlan>,
) -> std::io::Result<()> {
    use crate::network::codec::{write_tcp_packet, Writes};
    use crate::network::decoders::BuildPlan;

    output.write_b(0)?; // abilities
    output.write_f(aim_x)?;
    output.write_f(aim_y)?;
    if matches!(unit.entity_class, 4 | 17 | 19 | 32) {
        output.write_f(unit.rotation)?; // mech base rotation
    }
    write_unit_controller_sync(output, world, unit)?;
    output.write_f(if unit.entity_class == 3 {
        1.0
    } else {
        unit.elevation.clamp(0.0, 1.0)
    })?;
    output.write_l(0)?; // flag
    output.write_f(unit.health)?;
    let attacking = world
        .and_then(|world| controlling_session_for_unit(world, unit.id))
        .map_or_else(
            || {
                unit.attack_damage > 0.0
                    && (unit.x - aim_x).hypot(unit.y - aim_y) <= unit.attack_range
            },
            |player| player.shooting,
        );
    output.write_bool(attacking)?;
    output.write_i(mining_position.unwrap_or(-1))?;
    let weapon_mounts = enemy_weapon_mount_count(unit.unit_type);
    output.write_b(weapon_mounts)?;
    for _ in 0..weapon_mounts {
        output.write_b(u8::from(attacking))?;
        output.write_f(aim_x)?;
        output.write_f(aim_y)?;
    }
    if matches!(unit.entity_class, 5 | 23 | 26) {
        output.write_i(i32::try_from(unit.payloads.len()).unwrap_or(i32::MAX))?;
        for carried in &unit.payloads {
            write_carried_payload(output, carried)?;
        }
    }
    let overlay: Option<crate::network::world::UnitBuildPlan> =
        build_plan.map(|plan| crate::network::world::UnitBuildPlan {
            breaking: plan.breaking,
            position: plan.position,
            block: plan.block,
            rotation: plan.rotation,
            config: plan.config.clone(),
        });
    let plans: Vec<crate::network::world::UnitBuildPlan> = if !unit.build_plans.is_empty() {
        unit.build_plans.clone()
    } else {
        overlay.into_iter().collect()
    };
    write_unit_plans_queue(output, &plans, true)?;
    output.write_f(unit.rotation)?;
    output.write_f(unit.shield)?;
    output.write_bool(false)?; // spawned by core
    let (carried_item, carried_amount) = if unit.unit_type == MONO.unit_type {
        valid_item_stack(
            unit.tertiary_attack_reload.round() as i16 - 1,
            unit.secondary_attack_reload.max(0.0).round() as i32,
        )
    } else {
        (0, 0)
    };
    output.write_s(carried_item)?;
    output.write_i(carried_amount)?;
    // B14: official UnitEntity.write revision 9 (JAR offsets 147-191)
    // emits the StatusEntry COLLECTION (`i count + [s id + f duration]`),
    // falling back to the legacy single status only when the collection is
    // empty (mirrors the MSAV body in save_io.rs).
    if !unit.statuses.is_empty() {
        output.write_i(i32::try_from(unit.statuses.len()).unwrap_or(i32::MAX))?;
        for entry in &unit.statuses {
            output.write_s(entry.effect)?;
            output.write_f(entry.time.max(0.0))?;
        }
    } else if unit.status_effect >= 0 {
        output.write_i(1)?;
        output.write_s(unit.status_effect)?;
        output.write_f(unit.status_duration.max(0.0))?;
    } else {
        output.write_i(0)?;
    }
    output.write_b(unit.team)?;
    output.write_s(unit.unit_type)?;
    output.write_bool(
        unit.update_building && (!unit.build_plans.is_empty() || build_plan.is_some()),
    )?;
    output.write_f(unit.velocity_x)?;
    output.write_f(unit.velocity_y)?;
    output.write_f(unit.x)?;
    output.write_f(unit.y)?;
    Ok(())
}
