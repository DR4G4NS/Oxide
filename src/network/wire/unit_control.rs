//! Unit-control admission and selection. The listener adapter re-exports
//! these through crate::network::listener::*.

use std::sync::atomic::Ordering;

use crate::network::buildings::construction::{dynamic_at, effective_building_team};
use crate::network::buildings::snapshot::is_core_block;
use crate::network::combat::enemy::base_building_at;
use crate::network::protocol::*;
use crate::network::units::controller::unit_uses_command_ai;
use crate::network::units::*;
use crate::network::wire::auth::player_team;
use crate::network::wire::encode::frame_generated_packet;
use crate::network::world::{
    ControlledUnit, DynamicTile, DynamicWorld, EnemyUnit, PendingBreak, PendingBuild,
    SessionPlayer, TeamCore,
};

pub(crate) fn unit_control_allowed(
    world: &DynamicWorld,
    admin: &crate::state::administration::Administration,
    player: &SessionPlayer,
    control_type: u8,
    id: i32,
) -> bool {
    let actor_team = player_team(world, player);
    let possession_allowed = world.wave_rules.read().possession_allowed;
    let action_allowed = admin.allow_action(
        &crate::state::administration::PlayerAction::new(
            player.uuid.clone(),
            player.admin,
            crate::state::administration::ActionType::Control,
        )
        .with_unit(id),
    );
    if !possession_allowed
        || !action_allowed
        || crate::network::economy::player_is_dead(world, player)
    {
        return false;
    }
    match control_type {
        1 => {
            resolve_controllable_building(world, id, actor_team).is_some()
                && controlling_player_for_building(world, id).is_none()
                && player.controlled_unit != ControlledUnit::Building(id)
        }
        2 => {
            world.enemies.get(&id).is_some_and(|unit| {
                unit.team == actor_team
                && unit.health > 0.0
                && unit_player_controllable(unit.unit_type)
                // P0-01: possession is resolved from the authority model
                // (`unit_possessed_by`), never from order existence. A logic
                // binding (Logic authority or a transient ucontrol order,
                // kinds 6-9) keeps the unit conservatively non-possessable
                // until LogicAI authority is fully modeled.
                && !unit_bound_to_logic(world, id)
            }) && unit_possessed_by(world, id).is_none()
                && player.controlled_unit != ControlledUnit::Standard(id)
        }
        _ => false,
    }
}

pub(crate) fn is_controllable_block(block: i16) -> bool {
    // BuildTurret, the one-cell Router ControlBlock and actual Turret
    // subclasses. Parallax (TractorBeamTurret) and Segment
    // (PointDefenseTurret) do not implement ControlBlock; Distributor is a
    // Router subclass but explicitly rejects control when size != 1.
    matches!(block, 252 | 266 | 349..=355 | 357 | 358 | 360..=376)
}

pub(crate) fn resolve_controllable_building(
    world: &DynamicWorld,
    position: i32,
    actor_team: u8,
) -> Option<(i32, f32, f32)> {
    if let Some(tile) = dynamic_at(world, position).filter(|tile| {
        tile.team == actor_team && tile.block != 0 && is_controllable_block(tile.block)
    }) {
        let x = (tile.position >> 16) as i16 as f32 * 8.0;
        let y = tile.position as i16 as f32 * 8.0;
        return Some((tile.position, x, y));
    }
    base_building_at(world, position)
        .filter(|building| {
            building.team == actor_team
                && building.health > 0.0
                && is_controllable_block(building.block)
        })
        .map(|building| {
            let x = (building.position >> 16) as i16 as f32 * 8.0;
            let y = building.position as i16 as f32 * 8.0;
            (building.position, x, y)
        })
}

/// Applies the state transition performed by `InputHandler.unitControl` after
/// its validation gate. The player's persistent Alpha ID is intentionally not
/// changed; `controlled_unit` is what Player.writeSync exposes to clients.
pub(crate) fn apply_unit_control(
    world: &DynamicWorld,
    player: &mut SessionPlayer,
    control_type: u8,
    id: i32,
) -> Option<ControlledUnit> {
    let previous = player.controlled_unit;
    let next = match control_type {
        1 => {
            let team = player_team(world, player);
            let (position, x, y) = resolve_controllable_building(world, id, team)?;
            player.x = x;
            player.y = y;
            ControlledUnit::Building(position)
        }
        2 => {
            let unit = world.enemies.get(&id)?;
            player.x = unit.x;
            player.y = unit.y;
            player.rotation = unit.rotation;
            drop(unit);
            ControlledUnit::Standard(id)
        }
        _ => return None,
    };
    match next {
        ControlledUnit::Standard(unit_id) => switch_player_unit(world, player, Some(unit_id)),
        ControlledUnit::Core | ControlledUnit::Building(_) => {
            switch_player_unit(world, player, None);
            player.controlled_unit = next;
        }
    }
    player.shooting = false;
    player.boosting = false;
    player.mining_position = None;
    player.mining_progress = 0.0;
    world.player_sessions.insert(player.unit_id, player.clone());
    Some(previous)
}

pub(crate) fn encode_unit_reference(
    output: &mut Vec<u8>,
    player: &SessionPlayer,
    controlled: ControlledUnit,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;
    match controlled {
        ControlledUnit::Core => {
            output.write_b(2)?;
            output.write_i(player.unit_id)?;
        }
        ControlledUnit::Standard(id) => {
            output.write_b(2)?;
            output.write_i(id)?;
        }
        ControlledUnit::Building(position) => {
            output.write_b(1)?;
            output.write_i(position)?;
        }
    }
    Ok(())
}

pub(crate) fn encode_unit_despawn_frame(
    player: &SessionPlayer,
    controlled: ControlledUnit,
) -> std::io::Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(5);
    encode_unit_reference(&mut payload, player, controlled)?;
    frame_generated_packet(UNIT_DESPAWN_PACKET_ID, &payload, false)
}

pub(crate) fn building_control_select_allowed(
    world: &DynamicWorld,
    player: &SessionPlayer,
    position: i32,
) -> bool {
    let team = player_team(world, player);
    let (origin, block) = if let Some(tile) = dynamic_at(world, position).filter(|tile| {
        tile.team == team
            && tile.block != 0
            && (is_core_block(tile.block) || is_controllable_block(tile.block))
    }) {
        (tile.position, tile.block)
    } else if let Some(building) = base_building_at(world, position).filter(|building| {
        building.team == team
            && building.health > 0.0
            && (is_core_block(building.block) || is_controllable_block(building.block))
    }) {
        (building.position, building.block)
    } else {
        return false;
    };
    // Official Build.canControlSelect: CoreBlock requires unit.isPlayer().
    if is_core_block(block) && player.controlled_unit != ControlledUnit::Core {
        return false;
    }
    let possession_allowed = world.wave_rules.read().possession_allowed;
    if possession_allowed {
        return true;
    }
    let best_core_pos = best_core_position_for_team(world, team, player.x, player.y);
    match best_core_pos {
        Some(pos) => pos == origin,
        None => false,
    }
}

pub(crate) fn best_core_position_for_team(
    world: &DynamicWorld,
    team: u8,
    player_x: f32,
    player_y: f32,
) -> Option<i32> {
    let cores = live_team_cores(world, team);
    if cores.is_empty() {
        return None;
    }
    let single = cores.len() == 1;
    let env = world.wave_rules.read().env;
    let mut best: Option<(i32, i32, f32)> = None;
    for core in cores.iter() {
        if !single {
            let Some(core_unit) = crate::game::unit_types::core_block_unit_type(core.block) else {
                continue;
            };
            if !crate::game::unit_types::unit_supports_env(core_unit, env) {
                continue;
            }
        }
        let size = crate::game::content::block_size(core.block) as i32;
        let cx = (core.position >> 16) as i16 as f32 * 8.0;
        let cy = core.position as i16 as f32 * 8.0;
        let dx = cx - player_x;
        let dy = cy - player_y;
        let dist2 = dx * dx + dy * dy;
        match best {
            None => best = Some((core.position, size, dist2)),
            Some((_, best_size, best_dist2)) => {
                if size > best_size || (size == best_size && dist2 < best_dist2) {
                    best = Some((core.position, size, dist2));
                }
            }
        }
    }
    best.map(|(pos, _, _)| pos)
}

fn live_team_cores(world: &DynamicWorld, team: u8) -> Vec<TeamCore> {
    use std::collections::HashSet;
    // Preserve TeamData.cores / registration order (Arc Seq.min keeps first on ties).
    // Extra cores discovered from buildings/tiles are appended in position order.
    let mut cores: Vec<TeamCore> = Vec::new();
    let mut seen: HashSet<i32> = HashSet::new();
    let push_core = |cores: &mut Vec<TeamCore>,
                     seen: &mut HashSet<i32>,
                     position: i32,
                     block: i16,
                     health: f32| {
        if !is_core_block(block) || health <= 0.0 || !seen.insert(position) {
            return;
        }
        cores.push(TeamCore {
            position,
            block,
            health,
            max_health: health,
        });
    };
    if let Some(list) = world.team_core_lists.get(&team) {
        for core in list.iter() {
            if core_alive_at(world, core.position, core.block) {
                push_core(
                    &mut cores,
                    &mut seen,
                    core.position,
                    core.block,
                    core.health,
                );
            }
        }
    } else if let Some(core) = world.cores.get(&team) {
        if core_alive_at(world, core.position, core.block) {
            push_core(
                &mut cores,
                &mut seen,
                core.position,
                core.block,
                core.health,
            );
        }
    }
    let mut extras: Vec<TeamCore> = Vec::new();
    for building in world.base_buildings.iter() {
        if building.team == team
            && is_core_block(building.block)
            && building.health > 0.0
            && !seen.contains(&building.position)
        {
            extras.push(TeamCore {
                position: building.position,
                block: building.block,
                health: building.health,
                max_health: building.health,
            });
        }
    }
    for tile in world.tiles.iter() {
        if tile.team == team
            && is_core_block(tile.block)
            && tile.health > 0.0
            && !seen.contains(&tile.position)
        {
            extras.push(TeamCore {
                position: tile.position,
                block: tile.block,
                health: tile.health,
                max_health: tile.health,
            });
        }
    }
    extras.sort_by_key(|core| core.position);
    for core in extras {
        push_core(
            &mut cores,
            &mut seen,
            core.position,
            core.block,
            core.health,
        );
    }
    cores
}

fn core_alive_at(world: &DynamicWorld, position: i32, block: i16) -> bool {
    if let Some(tile) = world.tiles.get(&position) {
        return is_core_block(tile.block) && tile.health > 0.0;
    }
    if let Some(building) = base_building_at(world, position) {
        return building.block == block && building.health > 0.0;
    }
    true
}

pub(crate) fn encode_unit_building_control_select_frame(
    player: &SessionPlayer,
    controlled: ControlledUnit,
    building: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let mut payload = Vec::with_capacity(9);
    encode_unit_reference(&mut payload, player, controlled)?;
    payload.write_i(building)?; // TypeIO.writeBuilding
    frame_generated_packet(UNIT_BUILDING_CONTROL_SELECT_PACKET_ID, &payload, false)
}

/// Resolved target of a `RequestBlockSnapshot` for an acting player's team.
/// Official `NetServer.requestBlockSnapshot` (158.1) answers only when
/// `build.team == player.team()`; in-progress constructs are visible to the
/// same owning team so enemy construction state is never leaked (SOL-002).
#[derive(Debug, Clone)]
pub(crate) enum SnapshotTarget {
    PendingBuild(PendingBuild),
    PendingBreak(PendingBreak),
    Building(DynamicTile),
    None,
}

/// SOL-002 gate shared by the `RequestBlockSnapshot` handler and its tests:
/// resolves which authoritative building (finished, constructing or being
/// deconstructed) the actor may see at `position`. `actor_team` comes from
/// `player_team`, never from the packet payload.
pub(crate) fn request_block_snapshot_target(
    world: &DynamicWorld,
    position: i32,
    actor_team: u8,
) -> SnapshotTarget {
    if let Some(build) = world
        .pending_builds
        .iter()
        .find(|build| {
            build.team == actor_team
                && (build.position == position || build.occupied.contains(&position))
        })
        .map(|build| build.clone())
    {
        return SnapshotTarget::PendingBuild(build);
    }
    if let Some(build) = world
        .pending_breaks
        .iter()
        .find(|build| {
            // dashmap-guard: allow DM900 reason="effective_building_team only reads world state while this shared pending_breaks iterator guard is live"
            effective_building_team(world, build.position) == actor_team
                && (build.position == position || build.occupied.contains(&position))
        })
        .map(|build| build.clone())
    {
        return SnapshotTarget::PendingBreak(build);
    }
    if let Some(tile) = dynamic_at(world, position).filter(|tile| tile.team == actor_team) {
        return SnapshotTarget::Building(tile.clone());
    }
    SnapshotTarget::None
}
