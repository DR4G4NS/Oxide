//! Unit status container: StatusContainer trait, status application/ticking
//! and floor-status integration. Units facade re-exports through
//! crate::network::units::*.

use crate::network::world::*;
use dashmap::DashMap;

use super::*;

/// of the first entry.
pub(crate) trait StatusContainer {
    fn status_effect(&self) -> i16;
    fn status_fields(
        &mut self,
    ) -> (
        &mut i16,
        &mut f32,
        &mut Vec<crate::game::status::ActiveStatus>,
    );
    fn statuses_ref(&self) -> &[crate::game::status::ActiveStatus];
    fn unit_type_id(&self) -> i16 {
        -1
    }
    fn take_status_damage(&mut self, pierce: f32, normal: f32) {
        let _ = (pierce, normal);
    }

    fn apply_status(&mut self, status: i16, duration: f32) {
        let immune = immune_to_status(self.unit_type_id(), status);
        let reaction = {
            let (effect, duration_field, statuses) = self.status_fields();
            if statuses.is_empty() && *effect >= 0 {
                statuses.push(crate::game::status::ActiveStatus::simple(
                    *effect,
                    *duration_field,
                ));
            }
            let reaction = crate::game::status::apply_status(statuses, status, duration, immune);
            let (legacy, time) = crate::game::status::sync_legacy_view(statuses);
            *effect = legacy;
            *duration_field = time;
            reaction
        };
        self.take_status_damage(reaction.pierce_damage, reaction.damage.max(0.0));
    }

    fn clear_statuses(&mut self) {
        let (effect, duration_field, statuses) = self.status_fields();
        statuses.clear();
        *effect = -1;
        *duration_field = 0.0;
    }

    fn tick_statuses(&mut self, delta_ticks: f32) -> bool {
        let tick = {
            let (effect, duration_field, statuses) = self.status_fields();
            if statuses.is_empty() && *effect >= 0 {
                statuses.push(crate::game::status::ActiveStatus::simple(
                    *effect,
                    *duration_field,
                ));
            }
            let tick = crate::game::status::tick_statuses(statuses, delta_ticks);
            let (legacy, time) = crate::game::status::sync_legacy_view(statuses);
            *effect = legacy;
            *duration_field = time;
            tick
        };
        let mut normal = tick.normal;
        if normal > 0.0 {
            let armor = match self.unit_type_id() {
                1 => 4.0,
                2 => 9.0,
                3 => 10.0,
                4 => 18.0,
                5 => 1.0,
                6 => 4.0,
                7 | 8 => 9.0,
                11 => 3.0,
                12 => 5.0,
                16 => 3.0,
                17 => 5.0,
                18 => 9.0,
                _ => 0.0,
            };
            normal = (normal - armor).max(normal * 0.1);
        }
        self.take_status_damage(tick.pierce, normal);
        self.refresh_status_aggregate();
        tick.changed
    }

    fn status_multipliers_composite(&self) -> (f32, f32, f32) {
        crate::game::status::status_multipliers_composite(self.status_effect(), self.statuses_ref())
    }

    fn computed_status_aggregate(&self) -> crate::game::status::StatusAggregate {
        if self.statuses_ref().is_empty() && self.status_effect() >= 0 {
            crate::game::status::aggregate_statuses(&[crate::game::status::ActiveStatus::simple(
                self.status_effect(),
                1.0,
            )])
        } else {
            crate::game::status::aggregate_statuses(self.statuses_ref())
        }
    }

    fn status_aggregate_cached(&self) -> Option<crate::game::status::StatusAggregate> {
        None
    }

    fn set_status_aggregate_cache(&mut self, _agg: crate::game::status::StatusAggregate) {}

    fn status_aggregate(&self) -> crate::game::status::StatusAggregate {
        self.status_aggregate_cached()
            .unwrap_or_else(|| self.computed_status_aggregate())
    }

    fn refresh_status_aggregate(&mut self) {
        let agg = self.computed_status_aggregate();
        self.set_status_aggregate_cache(agg);
    }
}

pub(crate) fn status_multipliers_composite(
    legacy_effect: i16,
    statuses: &[crate::game::status::ActiveStatus],
) -> (f32, f32, f32) {
    crate::game::status::status_multipliers_composite(legacy_effect, statuses)
}

/// Official `UnitComp.isGrounded()` (`elevation < 0.001f`).
pub(crate) fn unit_is_grounded(elevation: f32) -> bool {
    elevation < 0.001
}

/// Official `StatusComp.update` floor gate: grounded and not hovering/flying.
pub(crate) fn unit_receives_floor_status(unit_type: i16, elevation: f32) -> bool {
    if !unit_is_grounded(elevation) {
        return false;
    }
    let movement = crate::game::content::unit_movement(unit_type);
    if movement.flying {
        return false;
    }
    !crate::game::content::unit_hovering(unit_type)
}

/// Official `Posc.floorOn()` for a world tile (air when a building occupies it).
/// Uses `World.toTile` (`Math.round(coord / 8)`).
pub(crate) fn floor_id_under_unit(
    world: &crate::network::world::DynamicWorld,
    x: f32,
    y: f32,
) -> i16 {
    let tile_x = (x / 8.0).round() as i32;
    let tile_y = (y / 8.0).round() as i32;
    if tile_x < 0 || tile_y < 0 || tile_x >= world.width || tile_y >= world.height {
        return 0;
    }
    let pos = (tile_x << 16) | tile_y;
    if world
        .tiles
        .get(&pos)
        .is_some_and(|tile| tile.block != 0 && tile.health > 0.0)
    {
        return 0;
    }
    if world
        .base_buildings
        .get(&pos)
        .is_some_and(|building| building.block != 0 && building.health > 0.0)
    {
        return 0;
    }
    world.floors[(tile_y * world.width + tile_x) as usize]
}

/// Reapply the tile floor's status through the normal `apply_status` path.
pub(crate) fn reapply_floor_status<T: StatusContainer>(
    unit: &mut T,
    world: &crate::network::world::DynamicWorld,
    x: f32,
    y: f32,
    unit_type: i16,
    elevation: f32,
) -> bool {
    if !unit_receives_floor_status(unit_type, elevation) {
        return false;
    }
    let floor_id = floor_id_under_unit(world, x, y);
    let (status, duration) = crate::game::content::floor_status(floor_id);
    if status < 0 || duration <= 0.0 {
        return false;
    }
    unit.apply_status(status, duration);
    true
}

pub(crate) fn tick_unit_statuses_with_floor(
    unit: &mut crate::network::world::EnemyUnit,
    world: &crate::network::world::DynamicWorld,
    delta_ticks: f32,
) -> bool {
    reapply_floor_status(unit, world, unit.x, unit.y, unit.unit_type, unit.elevation);
    StatusContainer::tick_statuses(unit, delta_ticks)
}

pub(crate) fn immune_to_status(unit_type: i16, status_effect: i16) -> bool {
    matches!(
        (unit_type, status_effect),
        (1, 1) | (8, 1) | (11, 1 | 8) | (40..=42, 1 | 8)
    )
}
