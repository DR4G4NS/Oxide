//! Build-plan helpers and assist-visual replanning. Owned by the buildings
//! domain; the listener adapter re-exports these so existing callers are
//! unchanged.

use crate::network::decoders::BuildPlan;
use crate::network::economy::default_unit_command;
use crate::network::world::{
    ControlledUnit, DynamicTile, DynamicWorld, EnemyUnit, SessionPlayer, UnitBuildPlan,
};

use crate::network::units::unit_orders::unit_build_speed;

pub(crate) fn pause_player_build_queue(world: &DynamicWorld, session: &SessionPlayer) {
    if let ControlledUnit::Standard(unit_id) = session.controlled_unit {
        if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
            unit.update_building = false;
        }
    }
    let stale = std::time::Instant::now() - std::time::Duration::from_secs(6);
    for mut build in world.pending_builds.iter_mut() {
        if build.builder.id == session.id {
            build.last_seen = stale;
        }
    }
}

pub(crate) fn sync_unit_build_plans(
    world: &DynamicWorld,
    player: &SessionPlayer,
    plans: &[BuildPlan],
    update_building: bool,
) {
    let ControlledUnit::Standard(unit_id) = player.controlled_unit else {
        return;
    };
    let Some(mut unit) = world.enemies.get_mut(&unit_id) else {
        return;
    };
    if unit_build_speed(unit.unit_type).is_none() {
        return;
    }
    unit.build_plans = plans
        .iter()
        .map(|plan| crate::network::world::UnitBuildPlan {
            breaking: plan.breaking,
            position: plan.position,
            block: plan.block,
            rotation: plan.rotation,
            config: plan.config.clone(),
        })
        .collect();
    unit.update_building = update_building;
}

pub(crate) enum AssistTarget {
    Pending(i32),
    Rebuild(i32),
}

pub(crate) fn assist_visual_plan(world: &DynamicWorld, unit: &EnemyUnit) -> Option<BuildPlan> {
    let command = world
        .unit_orders
        .get(&unit.id)
        .map(|order| order.command)
        .unwrap_or_else(|| default_unit_command(unit.unit_type));
    if unit.team != 1 || command != 3 || unit_build_speed(unit.unit_type).is_none() {
        return None;
    }
    let now = std::time::Instant::now();
    let mut candidates: Vec<_> = world
        .pending_builds
        .iter()
        .filter(|build| {
            now.duration_since(build.last_seen) <= std::time::Duration::from_millis(300)
        })
        .map(|build| {
            let x = (build.position >> 16) as i16 as f32 * 8.0;
            let y = build.position as i16 as f32 * 8.0;
            (
                (x - unit.x).hypot(y - unit.y),
                BuildPlan {
                    breaking: false,
                    position: build.position,
                    block: build.block,
                    rotation: build.rotation,
                    config: build.config.clone(),
                },
            )
        })
        .collect();
    let primary_rebuilder = world.enemies.iter().any(|builder| {
        builder.team == 1
            && world
                .unit_orders
                .get(&builder.id)
                .map(|order| order.command)
                .unwrap_or_else(|| default_unit_command(builder.unit_type))
                == 2
    });
    if primary_rebuilder {
        candidates.extend(world.tiles.iter().filter_map(|tile| {
            let (block, rotation, _, config) = rebuild_plan(&tile)?;
            (tile.production_progress > 0.0).then(|| {
                let x = (tile.position >> 16) as i16 as f32 * 8.0;
                let y = tile.position as i16 as f32 * 8.0;
                (
                    (x - unit.x).hypot(y - unit.y),
                    BuildPlan {
                        breaking: false,
                        position: tile.position,
                        block,
                        rotation,
                        config,
                    },
                )
            })
        }));
    }
    candidates
        .into_iter()
        .filter(|(distance, _)| *distance <= 1_500.0)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, plan)| plan)
}

pub(crate) fn rebuild_plan(tile: &DynamicTile) -> Option<(i16, u8, u8, Vec<u8>)> {
    let block = i16::try_from(tile.stored_amount.checked_sub(1)?).ok()?;
    (tile.block == 0 && (1..446).contains(&block) && tile.team == 1)
        .then(|| (block, tile.rotation, tile.team, tile.config.clone()))
}
