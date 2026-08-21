//! Team core inventory access. Owned by the economy domain; the listener
//! adapter re-exports these through crate::network::listener::* so
//! wire/simulation callers are unchanged. Team 1 reads the legacy
//! GameState.core_items; other teams read their team_items entry.

use crate::network::world::DynamicWorld;
use crate::state::game_state::{GameMode, GameState};

/// Snapshot of `team`'s core item inventory (official `TeamData.items`).
/// Team 1 reads the legacy `GameState.core_items`; other teams read their
/// `team_items` entry (default: the official 22-slot empty array). No locks
/// are held on return.
pub(crate) fn items_for_team(world: &DynamicWorld, team: u8) -> Vec<i32> {
    // Team 0 (neutral tiles) routes to the sharded default.
    let team = if team == 0 { 1 } else { team };
    if team == 1 {
        world.game_state.core_items.read().clone()
    } else {
        world
            .game_state
            .team_items
            .get(&team)
            .map(|items| items.clone())
            .unwrap_or_else(|| vec![0; 22])
    }
}

/// Mutable view of `team`'s core item inventory. Team 1 borrows the legacy
/// `GameState.core_items`; other teams borrow (lazily creating) their
/// `team_items` entry. The guard derefs to `&mut Vec<i32>` exactly like the
/// old `core_items.write()`, so existing call patterns keep working.
///
/// NOTE: never call `items_for_team_mut` (or iterate `team_items`) again
/// while this guard is alive — the DashMap shard lock is held and the same
/// map must not be re-entered (project rule: snapshot first, no mut-guard
/// iteration).
pub(crate) enum TeamItemsMut<'a> {
    Legacy(parking_lot::RwLockWriteGuard<'a, Vec<i32>>),
    Team(dashmap::mapref::one::RefMut<'a, u8, Vec<i32>>),
}

impl std::ops::Deref for TeamItemsMut<'_> {
    type Target = Vec<i32>;
    fn deref(&self) -> &Vec<i32> {
        match self {
            Self::Legacy(guard) => guard,
            Self::Team(guard) => guard,
        }
    }
}

impl std::ops::DerefMut for TeamItemsMut<'_> {
    fn deref_mut(&mut self) -> &mut Vec<i32> {
        match self {
            Self::Legacy(guard) => guard,
            Self::Team(guard) => guard,
        }
    }
}

/// Team-neutral (0) tiles route to team 1 (the sharded default).
pub(crate) fn items_for_team_mut(world: &DynamicWorld, team: u8) -> TeamItemsMut<'_> {
    // Team 0 (neutral tiles) routes to the sharded default.
    let team = if team == 0 { 1 } else { team };
    if team == 1 {
        TeamItemsMut::Legacy(world.game_state.core_items.write())
    } else {
        TeamItemsMut::Team(
            world
                .game_state
                .team_items
                .entry(team)
                .or_insert_with(|| vec![0; 22]),
        )
    }
}

pub(crate) fn has_requirements(state: &GameState, block: i16) -> bool {
    if *state.mode.read() == GameMode::Sandbox {
        return true;
    }
    let items = state.core_items.read();
    crate::game::content::block_requirements(block)
        .iter()
        .all(|(item, amount)| items.get(*item).is_some_and(|stored| stored >= amount))
}
