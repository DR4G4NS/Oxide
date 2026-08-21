//! Player item-transfer RPC frames, respawn and snapshot broadcast helpers.
//! The listener adapter re-exports these through crate::network::listener::*.

use crate::network::economy::*;
use crate::network::units::*;
use crate::network::world::*;

use std::sync::atomic::Ordering;

use crate::network::protocol::*;

use crate::network::buildings::construction::{base_block, base_origin, dynamic_at};
use crate::network::economy::inventory::items_for_team_mut;
use crate::network::economy::spec::{
    inventory_add, inventory_count, inventory_remove, inventory_total, storage_capacity,
    storage_linked_to_core,
};
use crate::network::wire::auth::player_team;
use crate::network::wire::encode::{encode_initial_entity_snapshot, frame_generated_packet};

pub(crate) fn nearest_opposing_unit(
    world: &DynamicWorld,
    team: u8,
    x: f32,
    y: f32,
) -> Option<(i32, f32, f32)> {
    world
        .enemies
        .iter()
        .filter(|unit| unit.team != team)
        .map(|unit| ((unit.x - x).hypot(unit.y - y), unit.id, unit.x, unit.y))
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, id, x, y)| (id, x, y))
}

pub(crate) fn enemy_weapon_mount_count(unit_type: i16) -> u8 {
    match unit_type {
        3 | 18 | 19 | 30 => 3,
        8 | 12 | 14 | 22 | 25 | 26 | 27 | 28 | 31 | 32 | 33 => 2,
        13 => 4,
        34 => 5,
        20 | 24 => 0,
        _ => 1,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ItemStorageTarget {
    Core {
        position: i32,
        team: u8,
    },
    Dynamic {
        position: i32,
        linked_to_core: bool,
        capacity: i32,
        team: u8,
    },
}

impl ItemStorageTarget {
    fn position(self) -> i32 {
        match self {
            Self::Core { position, .. } | Self::Dynamic { position, .. } => position,
        }
    }
}

pub(crate) fn item_storage_target(
    world: &DynamicWorld,
    requested: i32,
) -> Option<ItemStorageTarget> {
    let base_origin = base_origin(world, requested);
    if base_origin == world.core_position && matches!(base_block(world, base_origin), 339..=344) {
        // The target core's OWN team (official `CoreBuild.items` of that
        // team); maps without a registered per-team core fall back to 1.
        let team = crate::network::world::core_team_at_position(world, base_origin).unwrap_or(1);
        return Some(ItemStorageTarget::Core {
            position: base_origin,
            team,
        });
    }
    let storage = dynamic_at(world, requested)?;
    let capacity = storage_capacity(storage.block)?;
    (storage.team != 0).then(|| ItemStorageTarget::Dynamic {
        position: storage.position,
        linked_to_core: storage_linked_to_core(world, &storage),
        capacity,
        team: storage.team,
    })
}

pub(crate) fn player_can_transfer(
    player: &SessionPlayer,
    world: &DynamicWorld,
    target: ItemStorageTarget,
) -> bool {
    const ITEM_TRANSFER_RANGE: f32 = 220.0;
    let position = target.position();
    let x = (position >> 16) as i16 as f32 * 8.0;
    let y = position as i16 as f32 * 8.0;
    (player.x - x).hypot(player.y - y) <= ITEM_TRANSFER_RANGE
        && world
            .players
            .get(&player.unit_id)
            .is_none_or(|combat| !combat.dead)
        && {
            // SOL-002: only transfer items with the owner team (or derelict).
            let owner = match target {
                ItemStorageTarget::Core { team, .. } | ItemStorageTarget::Dynamic { team, .. } => {
                    team
                }
            };
            owner == 0 || owner == player_team(world, player)
        }
}

pub(crate) fn deposit_player_inventory(
    player: &mut SessionPlayer,
    world: &DynamicWorld,
    requested: i32,
) -> Option<(i32, i16, i32)> {
    let target = item_storage_target(world, requested)?;
    if !player_can_transfer(player, world, target)
        || !(0..22).contains(&player.carried_item)
        || player.carried_amount <= 0
    {
        return None;
    }
    let item = player.carried_item;
    let requested_amount = player.carried_amount;
    let accepted = match target {
        ItemStorageTarget::Core { team, .. }
        | ItemStorageTarget::Dynamic {
            linked_to_core: true,
            team,
            ..
        } => {
            crate::network::core_inventory::deposit_core_items(world, team, item, requested_amount)
        }
        ItemStorageTarget::Dynamic {
            position, capacity, ..
        } => {
            let mut storage = world.tiles.get_mut(&position)?;
            let accepted =
                requested_amount.min(capacity.saturating_sub(inventory_total(&storage.inventory)));
            inventory_add(&mut storage.inventory, item, accepted);
            accepted
        }
    };
    if accepted <= 0 {
        return None;
    }
    player.carried_amount -= accepted;
    if player.carried_amount == 0 {
        player.carried_item = -1;
    }
    Some((target.position(), item, accepted))
}

pub(crate) fn withdraw_items_to_player(
    player: &mut SessionPlayer,
    world: &DynamicWorld,
    requested: i32,
    item: i16,
    amount: i32,
) -> Option<(i32, i32)> {
    const ALPHA_ITEM_CAPACITY: i32 = 30;
    let target = item_storage_target(world, requested)?;
    if !player_can_transfer(player, world, target)
        || (player.carried_amount > 0 && player.carried_item != item)
    {
        return None;
    }
    let capacity = ALPHA_ITEM_CAPACITY.saturating_sub(player.carried_amount);
    let wanted = amount.min(capacity);
    if wanted <= 0 {
        return None;
    }
    let taken = match target {
        ItemStorageTarget::Core { team, .. }
        | ItemStorageTarget::Dynamic {
            linked_to_core: true,
            team,
            ..
        } => {
            let mut items = items_for_team_mut(world, team);
            let stored = items.get_mut(item as usize)?;
            let taken = wanted.min(*stored);
            *stored -= taken;
            taken
        }
        ItemStorageTarget::Dynamic { position, .. } => {
            let mut storage = world.tiles.get_mut(&position)?;
            let available = inventory_count(&storage.inventory, item);
            let taken = wanted.min(available);
            if taken > 0 {
                let removed = inventory_remove(&mut storage.inventory, item, taken);
                debug_assert!(removed);
            }
            taken
        }
    };
    if taken <= 0 {
        return None;
    }
    player.carried_item = item;
    player.carried_amount += taken;
    Some((target.position(), taken))
}

pub(crate) fn encode_take_items_frame(
    building: i32,
    item: i16,
    amount: i32,
    unit: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(15);
    payload.write_i(building)?;
    payload.write_s(item)?;
    payload.write_i(amount)?;
    payload.write_b(2)?; // TypeIO.writeUnit: standard unit reference
    payload.write_i(unit)?;
    frame_generated_packet(TAKE_ITEMS_PACKET_ID, &payload, false)
}

pub(crate) fn encode_transfer_item_to_frame(
    unit: i32,
    item: i16,
    amount: i32,
    x: f32,
    y: f32,
    building: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(23);
    payload.write_b(2)?; // TypeIO.writeUnit: standard unit reference
    payload.write_i(unit)?;
    payload.write_s(item)?;
    payload.write_i(amount)?;
    payload.write_f(x)?;
    payload.write_f(y)?;
    payload.write_i(building)?;
    frame_generated_packet(TRANSFER_ITEM_TO_PACKET_ID, &payload, false)
}

pub(crate) fn broadcast_player_snapshot(
    player: &SessionPlayer,
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
) -> std::io::Result<()> {
    let combat = world.players.get(&player.unit_id);
    let payload = encode_initial_entity_snapshot(player, combat.as_deref())?;
    out.broadcast(frame_generated_packet(
        ENTITY_SNAPSHOT_PACKET_ID,
        &payload,
        true,
    )?);
    Ok(())
}

pub(crate) fn respawn_session_player(
    player: &mut SessionPlayer,
    world: &DynamicWorld,
) -> Option<i32> {
    let old_unit_id = player.unit_id;
    let (_, mut combat) = world.players.remove(&old_unit_id)?;
    let new_unit_id = world
        .next_player_unit_id
        .fetch_add(1, Ordering::Relaxed)
        .max(2_500_000);
    let (core_x, core_y) = core_world_for_team(world, combat.team);
    combat.unit_id = new_unit_id;
    combat.x = core_x;
    combat.y = core_y;
    combat.health = 150.0;
    combat.shield = 0.0;
    combat.status_effect = -1;
    combat.statuses.clear();
    crate::network::units::StatusContainer::clear_statuses(&mut combat);
    combat.status_duration = 0.0;
    combat.dead = false;
    combat.respawn_timer = 0.0;
    world.players.insert(new_unit_id, combat.clone());
    world
        .player_profiles
        .insert(combat.uuid.clone(), combat.clone());

    world.player_sessions.remove(&old_unit_id);
    // Respawning at the core is Player.clearUnit().
    switch_player_unit(world, player, None);
    player.unit_id = new_unit_id;
    player.x = core_x;
    player.y = core_y;
    player.shooting = false;
    player.boosting = false;
    player.mining_position = None;
    player.mining_progress = 0.0;
    player.carried_item = -1;
    player.carried_amount = 0;
    world.player_sessions.insert(new_unit_id, player.clone());
    Some(old_unit_id)
}

pub(crate) fn broadcast_respawn(
    out: &dyn crate::network::outbound::FrameEmit,
    player: &SessionPlayer,
    world: &DynamicWorld,
    old_unit_id: Option<i32>,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    if let Some(old_unit_id) = old_unit_id {
        let mut despawn = Vec::with_capacity(5);
        despawn.write_b(2)?; // TypeIO standard unit reference
        despawn.write_i(old_unit_id)?;
        out.broadcast(frame_generated_packet(
            UNIT_DESPAWN_PACKET_ID,
            &despawn,
            false,
        )?);
    }
    let mut spawn = Vec::with_capacity(8);
    let team = world
        .players
        .get(&player.unit_id)
        .map(|state| state.team)
        .unwrap_or(1);
    spawn.write_i(core_position_for_team(world, team))?;
    spawn.write_i(player.id)?;
    out.broadcast(frame_generated_packet(
        PLAYER_SPAWN_PACKET_ID,
        &spawn,
        false,
    )?);
    broadcast_player_snapshot(player, world, out)
}
