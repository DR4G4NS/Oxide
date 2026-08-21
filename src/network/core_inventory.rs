//! Authoritative per-team core inventory capacity and deposits.
//!
//! Mindustry shares one item module across every core owned by a team. The
//! per-item capacity is the sum of all core capacities plus adjacent Serpulo
//! containers/vaults (`StorageBlock.coreMerge`). All ingress paths must use
//! this module so a loaded or live world can never retain more than the HUD's
//! advertised capacity.

use crate::network::economy::items_for_team_mut;
use crate::network::world::{team_core_snapshot, DynamicWorld, TeamCore};
use std::collections::HashSet;

pub(crate) const ITEM_COUNT: usize = 22;

/// Official `CoreBlock.itemCapacity` values from Blocks.java 158.1.
pub(crate) const fn core_block_capacity(block: i16) -> i32 {
    match block {
        339 => 4_000,  // core-shard
        340 => 9_000,  // core-foundation
        341 => 13_000, // core-nucleus
        342 => 2_000,  // core-bastion
        343 => 3_000,  // core-citadel
        344 => 4_000,  // core-acropolis
        _ => 0,
    }
}

/// Only the Serpulo container and vault have `coreMerge=true` in 158.1.
const fn core_merge_storage_capacity(block: i16) -> i32 {
    match block {
        345 => 300,
        346 => 1_000,
        _ => 0,
    }
}

fn footprint(position: i32, block: i16) -> Vec<(i32, i32)> {
    let size = i32::from(crate::game::content::block_size(block)).max(1);
    let origin_x = (position >> 16) as i16 as i32;
    let origin_y = position as i16 as i32;
    let offset = -(size - 1) / 2;
    let mut result = Vec::with_capacity((size * size) as usize);
    for dy in 0..size {
        for dx in 0..size {
            result.push((origin_x + offset + dx, origin_y + offset + dy));
        }
    }
    result
}

fn touches_core(occupied: &[i32], core: &TeamCore) -> bool {
    let core_tiles = footprint(core.position, core.block);
    occupied.iter().any(|position| {
        let x = (position >> 16) as i16 as i32;
        let y = *position as i16 as i32;
        core_tiles
            .iter()
            .any(|(core_x, core_y)| (x - core_x).abs() + (y - core_y).abs() == 1)
    })
}

/// Maximum amount of each item shared by `team`'s cores.
pub(crate) fn core_item_capacity(world: &DynamicWorld, team: u8) -> i32 {
    let team = if team == 0 { 1 } else { team };
    let mut cores = team_core_snapshot(world, team);
    // Legacy/custom worlds may expose only `core_position`; preserve that
    // compatibility as a sharded core instead of turning every deposit into
    // a rejection. Prefer the actual tile/base block when it is available.
    if cores.is_empty() && team == 1 {
        let position = world.core_position;
        let x = (position >> 16) as i16 as i32;
        let y = position as i16 as i32;
        let base = if x >= 0 && y >= 0 && x < world.width && y < world.height {
            world.base_blocks[(y * world.width + x) as usize]
        } else {
            0
        };
        let block = world
            .tiles
            .get(&position)
            .map(|tile| tile.block)
            .filter(|block| core_block_capacity(*block) > 0)
            .or_else(|| (core_block_capacity(base) > 0).then_some(base))
            .unwrap_or(339);
        cores.push(TeamCore {
            position,
            block,
            health: *world.game_state.core_health.read(),
            max_health: world.core_max_health,
        });
    }
    if cores.is_empty() {
        return 0;
    }
    let mut capacity = cores.iter().fold(0i32, |total, core| {
        total.saturating_add(core_block_capacity(core.block))
    });
    let mut counted = HashSet::new();

    // Loaded map buildings are commonly represented in both registries.
    // Deduplicate by their origin before adding core-merge storage capacity.
    for tile in world.tiles.iter() {
        let extra = core_merge_storage_capacity(tile.block);
        if extra > 0
            && tile.team == team
            && cores.iter().any(|core| touches_core(&tile.occupied, core))
            && counted.insert(tile.position)
        {
            capacity = capacity.saturating_add(extra);
        }
    }
    for building in world.base_buildings.iter() {
        let extra = core_merge_storage_capacity(building.block);
        if extra > 0
            && building.team == team
            && cores
                .iter()
                .any(|core| touches_core(&building.occupied, core))
            && counted.insert(building.position)
        {
            capacity = capacity.saturating_add(extra);
        }
    }
    capacity.max(0)
}

/// Deposit an item stack and return the amount consumed from the source.
///
/// With vanilla's default `coreIncinerates=true`, a full core consumes the
/// excess while keeping its stored amount capped. When the rule is disabled,
/// only the available space is accepted.
pub(crate) fn deposit_core_items(world: &DynamicWorld, team: u8, item: i16, amount: i32) -> i32 {
    if !(0..ITEM_COUNT as i16).contains(&item) || amount <= 0 {
        return 0;
    }
    let capacity = core_item_capacity(world, team);
    if capacity <= 0 {
        return 0;
    }
    let core_incinerates = world.wave_rules.read().core_incinerates;
    let mut items = items_for_team_mut(world, team);
    if items.len() < ITEM_COUNT {
        items.resize(ITEM_COUNT, 0);
    }
    let stored = &mut items[item as usize];
    *stored = (*stored).clamp(0, capacity);
    let stored_amount = amount.min(capacity.saturating_sub(*stored));
    *stored = stored.saturating_add(stored_amount).min(capacity);
    if core_incinerates {
        amount
    } else {
        stored_amount
    }
}

/// Clamp legacy saves and all team inventories to their current topology.
/// Returns true when any stored value was repaired.
pub(crate) fn clamp_core_inventories(world: &DynamicWorld) -> bool {
    let mut teams: Vec<u8> = world
        .team_core_lists
        .iter()
        .map(|entry| *entry.key())
        .collect();
    teams.extend(world.cores.iter().map(|entry| *entry.key()));
    teams.extend(world.game_state.team_items.iter().map(|entry| *entry.key()));
    teams.push(1);
    teams.sort_unstable();
    teams.dedup();

    let mut changed = false;
    for team in teams {
        let capacity = core_item_capacity(world, team);
        if capacity <= 0 {
            continue;
        }
        let mut items = items_for_team_mut(world, team);
        if items.len() < ITEM_COUNT {
            items.resize(ITEM_COUNT, 0);
            changed = true;
        }
        for amount in items.iter_mut() {
            let repaired = (*amount).clamp(0, capacity);
            changed |= repaired != *amount;
            *amount = repaired;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_core_capacities_match_v158() {
        assert_eq!(core_block_capacity(339), 4_000);
        assert_eq!(core_block_capacity(340), 9_000);
        assert_eq!(core_block_capacity(341), 13_000);
        assert_eq!(core_block_capacity(342), 2_000);
        assert_eq!(core_block_capacity(343), 3_000);
        assert_eq!(core_block_capacity(344), 4_000);
        assert_eq!(core_block_capacity(345), 0);
    }

    #[test]
    fn footprint_matches_even_and_odd_core_sizes() {
        let p = (20 << 16) | 30;
        let shard = footprint(p, 339);
        assert_eq!(shard.len(), 9);
        assert!(shard.contains(&(20, 30)));
        let foundation = footprint(p, 340);
        assert_eq!(foundation.len(), 16);
        assert!(foundation.contains(&(19, 29)));
        assert!(foundation.contains(&(22, 32)));
    }
}
