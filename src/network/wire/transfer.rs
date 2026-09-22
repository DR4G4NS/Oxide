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
use crate::network::wire::encode::{encode_initial_entity_snapshot_in, frame_generated_packet};

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

// Every id is listed explicitly (no range patterns) so this table stays a
// 1:1 auditable mirror of the desktop.jar probe dump.
#[allow(clippy::manual_range_patterns)]
pub(crate) fn enemy_weapon_mount_count(unit_type: i16) -> u8 {
    // Authoritative v160.5 oracle (desktop.jar): full ContentLoader dump with
    // per-type UnitType.init applied, so every mirror weapon is counted the
    // way UnitType.init appends flipped copies to `weapons` and WeaponsComp
    // sizes its mounts array. Weapon.mirror defaults TRUE.
    match unit_type {
        0 | 1 | 2 => 2,
        3 => 6,
        4 | 5 | 6 | 7 => 2,
        8 => 3,
        9 | 10 => 1,
        11 => 2,
        12 => 4,
        13 => 8,
        14 => 3,
        15 => 1,
        16 | 17 => 2,
        18 | 19 => 6,
        20 => 0,
        21 => 2,
        22 => 4,
        23 => 1,
        24 => 0,
        25 => 3,
        26 => 4,
        27 | 28 => 3,
        29 => 1,
        30 => 4,
        31 => 3,
        32 | 33 => 4,
        34 => 6,
        35 | 36 | 37 => 2,
        38 | 39 | 40 => 1,
        41 => 5,
        42 | 43 => 1,
        44 => 4,
        45 => 2,
        46 => 1,
        47 | 48 | 49 => 2,
        50 | 51 => 1,
        52 => 2,
        53 => 1,
        54 => 2,
        55 => 1,
        56 | 57 => 0,
        58 => 1,
        59 => 3,
        60 => 2,
        61 | 62 | 63 | 64 => 0,
        65 | 66 => 1,
        67 => 2,
        68 => 1,
        69 => 0,
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
    Conveyor {
        position: i32,
        team: u8,
    },
}

impl ItemStorageTarget {
    fn position(self) -> i32 {
        match self {
            Self::Core { position, .. }
            | Self::Dynamic { position, .. }
            | Self::Conveyor { position, .. } => position,
        }
    }
}

pub(crate) fn item_storage_target(
    world: &DynamicWorld,
    requested: i32,
) -> Option<ItemStorageTarget> {
    let base_origin = base_origin(world, requested);
    let base_block_id = base_block(world, base_origin);
    if matches!(base_block_id, 339..=344) {
        let team = crate::network::world::core_team_at_position(world, base_origin)
            .or_else(|| world.base_buildings.get(&base_origin).map(|b| b.team))
            .unwrap_or(1);
        return Some(ItemStorageTarget::Core {
            position: base_origin,
            team,
        });
    }
    let storage = dynamic_at(world, requested)?;
    if matches!(storage.block, 339..=344) {
        return Some(ItemStorageTarget::Core {
            position: storage.position,
            team: storage.team,
        });
    }
    if is_plain_conveyor(storage.block) || matches!(storage.block, 259 | 272 | 273 | 279) {
        return Some(ItemStorageTarget::Conveyor {
            position: storage.position,
            team: storage.team,
        });
    }
    let capacity = storage_capacity(storage.block)
        .or(match storage.block {
            271 => Some(120),
            266 => Some(1),
            267 => Some(2),
            265 => Some(32),
            268 | 269 => Some(1),
            262 | 263 => Some(10),
            _ => None,
        })
        .unwrap_or(30);
    Some(ItemStorageTarget::Dynamic {
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
                ItemStorageTarget::Core { team, .. }
                | ItemStorageTarget::Dynamic { team, .. }
                | ItemStorageTarget::Conveyor { team, .. } => team,
            };
            owner == 0 || owner == player_team(world, player)
        }
        && {
            // BuildingComp.allowDeposit: cores always accept; otherwise
            // `!state.rules.onlyDepositCore`.
            if world.wave_rules.read().only_deposit_core {
                matches!(target, ItemStorageTarget::Core { .. })
            } else {
                true
            }
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
        ItemStorageTarget::Conveyor { position, .. } => {
            let mut conveyor = world.tiles.get_mut(&position)?;
            let max_capacity: usize = if matches!(conveyor.block, 259 | 279) {
                10
            } else {
                4
            };
            let space = max_capacity.saturating_sub(conveyor.conveyor_items.len());
            let accepted = (requested_amount as usize).min(space) as i32;
            for _ in 0..accepted {
                conveyor.conveyor_items.push((item, 0.0));
            }
            if accepted > 0 {
                let front = conveyor.conveyor_items.first().copied();
                conveyor.stored_item = front.map(|(item, _)| item).unwrap_or(-1);
                conveyor.stored_amount = i32::try_from(conveyor.conveyor_items.len()).unwrap_or(0);
                conveyor.transport_progress = front.map(|(_, progress)| progress).unwrap_or(0.0);
                if matches!(conveyor.block, 259 | 279) && conveyor.stack_link == -1 {
                    conveyor.stack_link = position;
                }
            }
            accepted
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
        ItemStorageTarget::Conveyor { position, .. } => {
            let mut conveyor = world.tiles.get_mut(&position)?;
            // Materialize the legacy stack before removing items. Otherwise
            // rebuilding the counters from an empty queue erases the remainder.
            if conveyor.conveyor_items.is_empty() && matches!(conveyor.block, 259 | 279) {
                let items = if conveyor.inventory.is_empty() {
                    vec![(conveyor.stored_item, conveyor.stored_amount)]
                } else {
                    std::mem::take(&mut conveyor.inventory)
                };
                for (stored, count) in items {
                    if stored >= 0 {
                        let progress = conveyor.transport_progress;
                        conveyor.conveyor_items.extend(std::iter::repeat_n(
                            (stored, progress),
                            count.clamp(0, 10) as usize,
                        ));
                    }
                }
            }
            let mut taken = 0;
            conveyor.conveyor_items.retain(|&(conveyor_item, _)| {
                if conveyor_item == item && taken < wanted {
                    taken += 1;
                    false
                } else {
                    true
                }
            });
            if taken == 0 && !conveyor.inventory.is_empty() {
                let available = inventory_count(&conveyor.inventory, item);
                let count = wanted.min(available);
                if count > 0 {
                    inventory_remove(&mut conveyor.inventory, item, count);
                    taken = count;
                }
            } else if taken == 0 && conveyor.stored_item == item && conveyor.stored_amount > 0 {
                let count = wanted.min(conveyor.stored_amount);
                conveyor.stored_amount -= count;
                if conveyor.stored_amount == 0 {
                    conveyor.stored_item = -1;
                }
                taken = count;
            }
            if taken > 0
                && (is_plain_conveyor(conveyor.block) || matches!(conveyor.block, 259 | 279))
            {
                let front = conveyor.conveyor_items.first().copied();
                conveyor.stored_item = front.map(|(item, _)| item).unwrap_or(-1);
                conveyor.stored_amount = i32::try_from(conveyor.conveyor_items.len()).unwrap_or(0);
                conveyor.transport_progress = front.map(|(_, progress)| progress).unwrap_or(0.0);
                if conveyor.conveyor_items.is_empty()
                    && conveyor.inventory.is_empty()
                    && conveyor.stored_amount <= 0
                {
                    conveyor.stack_link = -1;
                    conveyor.stack_cooldown = 0.0;
                }
            }
            taken
        }
        ItemStorageTarget::Dynamic { position, .. } => {
            let mut storage = world.tiles.get_mut(&position)?;
            let available = inventory_count(&storage.inventory, item);
            let mut taken = wanted.min(available);
            if taken > 0 {
                let removed = inventory_remove(&mut storage.inventory, item, taken);
                debug_assert!(removed);
            } else if storage.stored_item == item && storage.stored_amount > 0 {
                let count = wanted.min(storage.stored_amount);
                storage.stored_amount -= count;
                if storage.stored_amount == 0 {
                    storage.stored_item = -1;
                }
                taken = count;
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
    let combat = world
        .players
        .get(&player.unit_id)
        .map(|entry| entry.clone());
    let payload = encode_initial_entity_snapshot_in(player, combat.as_ref(), Some(world))?;
    out.broadcast(frame_generated_packet(
        ENTITY_SNAPSHOT_PACKET_ID,
        &payload,
        true,
    )?);
    Ok(())
}

/// Official `InputHandler.unitClear`: if the possessed unit carries a
/// `dockedType` with `coreUnitDock`, spawn that core ship at the left unit
/// instead of `playerSpawn` at the core. Serpulo alpha/beta/gamma do not
/// dock and fall through to [`respawn_session_player`] (ASTRA C07).
pub(crate) fn unit_clear_session_player(
    player: &mut SessionPlayer,
    world: &DynamicWorld,
) -> Option<i32> {
    let team = world
        .players
        .get(&player.unit_id)
        .map(|combat| combat.team)
        .unwrap_or(1);
    let possessing = matches!(player.controlled_unit, ControlledUnit::Standard(_));
    let docked = player.docked_type.unwrap_or_else(|| {
        crate::network::wire::unit_control::player_core_unit_content_id(
            world, team, player.x, player.y,
        )
    });
    if possessing && crate::game::unit_types::core_unit_dock(docked) {
        let (x, y, rotation) = match player.controlled_unit {
            ControlledUnit::Standard(id) => world
                .enemies
                .get(&id)
                .map(|unit| (unit.x, unit.y, unit.rotation))
                .unwrap_or((player.x, player.y, player.rotation)),
            _ => (player.x, player.y, player.rotation),
        };
        switch_player_unit(world, player, None);
        player.controlled_unit = ControlledUnit::Core;
        player.x = x;
        player.y = y;
        player.rotation = rotation;
        player.docked_type = None;
        if let Some(mut combat) = world.players.get_mut(&player.unit_id) {
            combat.x = x;
            combat.y = y;
            combat.health = enemy_spec(docked)
                .map(|spec| spec.health)
                .unwrap_or(combat.health);
            combat.dead = false;
        }
        world.player_sessions.insert(player.unit_id, player.clone());
        return None;
    }
    player.docked_type = None;
    respawn_session_player(player, world)
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
    combat.health = crate::network::wire::unit_control::best_core_position_for_team(
        world,
        combat.team,
        core_x,
        core_y,
    )
    .and_then(|pos| {
        world
            .team_core_lists
            .get(&combat.team)
            .and_then(|list| {
                list.iter()
                    .find(|core| core.position == pos)
                    .map(|c| c.block)
            })
            .or_else(|| world.tiles.get(&pos).map(|tile| tile.block))
    })
    .and_then(crate::game::unit_types::core_block_unit_type)
    .and_then(enemy_spec)
    .map(|spec| spec.health)
    .unwrap_or(150.0);
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
