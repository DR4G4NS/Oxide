//! Thorium reactor domain model.
//!
//! The wire model still stores the timer and heat in legacy `DynamicTile`
//! fields. This module is the only place that interprets those fields as a
//! `NuclearReactorState`, preventing unrelated codecs from reusing them as a
//! liquid or crafting progress by accident.

use crate::network::economy::building_time_scale;
use crate::network::world::DynamicWorld;

pub const THORIUM_REACTOR_BLOCK: i16 = 315;
pub const THORIUM_ITEM: i16 = 7;
pub const CRYOFLUID: i16 = 3;
pub const ITEM_CAPACITY: i32 = 30;
pub const ITEM_DURATION: f32 = 360.0;
pub const HEATING: f32 = 0.02;
pub const COOLANT_POWER: f32 = 0.5;
pub const AMBIENT_COOLDOWN_TIME: f32 = 60.0 * 20.0;
pub const OVERHEAT_THRESHOLD: f32 = 0.999;
pub const EXPLOSION_RADIUS: f32 = 19.0 * 8.0;
pub const EXPLOSION_DAMAGE: f32 = 1250.0 * 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NuclearReactorState {
    pub fuel: i32,
    pub coolant: f32,
    pub heat: f32,
    pub fuel_timer: f32,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NuclearReactorStep {
    pub state: NuclearReactorState,
    pub fuel_consumed: i32,
    pub coolant_consumed: f32,
    pub overheated: bool,
}

/// Pure official-state transition, separated from storage/network concerns.
pub fn advance_nuclear_reactor(
    mut state: NuclearReactorState,
    delta_ticks: f32,
    time_scale: f32,
) -> NuclearReactorStep {
    let delta_ticks = delta_ticks.max(0.0);
    let scaled_delta = delta_ticks * time_scale.max(0.0);
    let initial_fuel = state.fuel.max(0);
    let fullness = initial_fuel as f32 / ITEM_CAPACITY as f32;

    if initial_fuel > 0 && state.enabled {
        // Official NuclearReactor$NuclearReactorBuild.updateTile JAR offsets
        // 43-68: `heat += fullness * heating * Math.min(delta(), 4f)` — the
        // per-frame heat cap `min(delta, 4)` is part of the official model
        // (a laggy 1 s frame advances heat by at most 4 ticks' worth). One
        // Rust world-loop step is the analogue of one official frame, so the
        // cap applies to the scaled batch delta. At 60 TPS (delta = 1 per
        // step) `min(1, 4) = 1` and the integration is identical to the
        // previous uncapped per-tick formula. `delta()` already includes
        // timeScale (Building.delta = Time.delta * timeScale), hence
        // `scaled_delta.min(4.0)`.
        state.heat += fullness * HEATING * scaled_delta.min(4.0);
        state.fuel_timer += scaled_delta;
    } else {
        state.heat = (state.heat - delta_ticks / AMBIENT_COOLDOWN_TIME).max(0.0);
    }

    let mut fuel_consumed = 0;
    while state.fuel > 0 && state.fuel_timer >= ITEM_DURATION {
        state.fuel_timer -= ITEM_DURATION;
        state.fuel -= 1;
        fuel_consumed += 1;
    }
    if state.fuel <= 0 {
        state.fuel = 0;
        state.fuel_timer = state.fuel_timer.min(ITEM_DURATION);
    }

    let coolant_consumed = state
        .coolant
        .max(0.0)
        .min(state.heat.max(0.0) / COOLANT_POWER);
    state.coolant = (state.coolant - coolant_consumed).max(0.0);
    state.heat = (state.heat - coolant_consumed * COOLANT_POWER).clamp(0.0, 1.0);
    let overheated = state.heat >= OVERHEAT_THRESHOLD;

    NuclearReactorStep {
        state,
        fuel_consumed,
        coolant_consumed,
        overheated,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReactorOverheat {
    pub position: i32,
    pub explosion_enabled: bool,
}

/// Advances all thorium reactors and returns domain events for the network
/// coordinator to resolve after no DashMap guards are held.
pub fn tick(world: &DynamicWorld, delta_ticks: f32) -> (bool, Vec<ReactorOverheat>) {
    let reactors: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == THORIUM_REACTOR_BLOCK)
        .map(|tile| tile.position)
        .collect();
    let mut changed = false;
    let mut events = Vec::new();

    for position in reactors {
        let Some(snapshot) = world.tiles.get(&position).map(|tile| tile.clone()) else {
            continue;
        };
        let fuel = snapshot
            .inventory
            .iter()
            .find_map(|(item, amount)| (*item == THORIUM_ITEM).then_some(*amount))
            .unwrap_or(0)
            .max(0);
        let coolant = if snapshot.stored_liquid == CRYOFLUID {
            snapshot.liquid_amount.max(0.0)
        } else {
            0.0
        };
        let step = advance_nuclear_reactor(
            NuclearReactorState {
                fuel,
                coolant,
                heat: snapshot.output_liquid_amount.clamp(0.0, 1.0),
                fuel_timer: snapshot.production_progress.max(0.0),
                enabled: snapshot.enabled,
            },
            delta_ticks,
            building_time_scale(world, position),
        );

        if let Some(mut reactor) = world.tiles.get_mut(&position) {
            if step.fuel_consumed > 0 {
                if let Some((_, amount)) = reactor
                    .inventory
                    .iter_mut()
                    .find(|(item, _)| *item == THORIUM_ITEM)
                {
                    *amount = (*amount - step.fuel_consumed).max(0);
                }
                reactor.inventory.retain(|(_, amount)| *amount > 0);
            }
            reactor.production_progress = step.state.fuel_timer;
            reactor.output_liquid_amount = step.state.heat;
            if reactor.stored_liquid == CRYOFLUID {
                reactor.liquid_amount = step.state.coolant;
                if reactor.liquid_amount <= 0.0001 {
                    reactor.liquid_amount = 0.0;
                    reactor.stored_liquid = -1;
                }
            }
            reactor.liquid_inventory.clear();
            if reactor.stored_liquid >= 0 && reactor.liquid_amount > 0.0001 {
                let liquid = reactor.stored_liquid;
                let amount = reactor.liquid_amount;
                reactor.liquid_inventory.push((liquid, amount));
            }
        }
        changed |= delta_ticks > 0.0
            && (fuel > 0
                || coolant > 0.0
                || snapshot.output_liquid_amount > 0.0
                || step.fuel_consumed > 0);
        if step.overheated {
            events.push(ReactorOverheat {
                position,
                explosion_enabled: world.wave_rules.read().reactor_explosions,
            });
        }
    }
    (changed, events)
}

pub fn fuel_fullness(world: &DynamicWorld, position: i32) -> f32 {
    world.tiles.get(&position).map_or(0.0, |tile| {
        tile.inventory
            .iter()
            .find_map(|(item, amount)| (*item == THORIUM_ITEM).then_some(*amount))
            .unwrap_or(0)
            .max(0) as f32
            / ITEM_CAPACITY as f32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_uncoolled_reactor_overheats_at_official_rate() {
        // B11: the official per-frame cap Math.min(delta, 4) means a batch
        // of 50 ticks advances heat by min(50,4)=4 ticks' worth in ONE step;
        // at the official 60 Hz cadence (delta=1 per step) the reactor
        // overheats on tick 50.
        let state = NuclearReactorState {
            fuel: 30,
            coolant: 0.0,
            heat: 0.0,
            fuel_timer: 0.0,
            enabled: true,
        };
        // A zero-delta step is a no-op; the loop then advances exactly 50
        // single-tick frames.
        let mut step = advance_nuclear_reactor(state, 0.0, 1.0);
        for tick in 1..=50 {
            step = advance_nuclear_reactor(step.state, 1.0, 1.0);
            if tick < 50 {
                assert!(
                    !step.overheated,
                    "overheat only at tick 50 (tick {tick}, heat {})",
                    step.state.heat
                );
            }
        }
        assert!(
            step.overheated,
            "full uncooled reactor overheats at tick 50"
        );
        assert!((step.state.heat - 1.0).abs() < 0.0001);
    }

    #[test]
    fn heat_integration_caps_the_batch_delta_at_four_ticks() {
        // B11: NuclearReactor$NuclearReactorBuild.updateTile JAR offsets
        // 43-68 apply `Math.min(delta(), 4f)` per frame; a single laggy
        // step (delta 10 or 60) contributes at most 4 * heating.
        let state = NuclearReactorState {
            fuel: 30,
            coolant: 0.0,
            heat: 0.0,
            fuel_timer: 0.0,
            enabled: true,
        };
        let capped = advance_nuclear_reactor(state, 10.0, 1.0);
        assert!((capped.state.heat - 0.02 * 4.0).abs() < 0.0001);
        let capped_60 = advance_nuclear_reactor(state, 60.0, 1.0);
        assert!((capped_60.state.heat - 0.02 * 4.0).abs() < 0.0001);
        // At 60 TPS (delta = 1) the cap never binds and the integration is
        // exactly fullness * heating per tick.
        let per_tick = advance_nuclear_reactor(state, 1.0, 1.0);
        assert!((per_tick.state.heat - 0.02).abs() < 0.0001);
        // timeScale is part of delta(): scaled 2 ticks still cap at 4.
        let scaled = advance_nuclear_reactor(state, 3.0, 2.0);
        assert!((scaled.state.heat - 0.02 * 4.0).abs() < 0.0001);
    }

    #[test]
    fn cryofluid_removes_half_a_heat_unit_per_liquid_unit() {
        let result = advance_nuclear_reactor(
            NuclearReactorState {
                fuel: 30,
                coolant: 1.0,
                heat: 0.6,
                fuel_timer: 0.0,
                enabled: false,
            },
            0.0,
            1.0,
        );
        assert!((result.state.heat - 0.1).abs() < 0.0001);
        assert_eq!(result.coolant_consumed, 1.0);
    }
}
