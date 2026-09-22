//! Power/mass-driver/reactor phases.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::angle_between;
use crate::network::buildings::snapshot::*;
use crate::network::combat::enemy::damage_building;
use crate::network::combat::*;
use crate::network::economy::spec::{
    angle_near, generator_fuel, inventory_add, inventory_remove, inventory_total,
    mass_driver_state, move_toward_angle, valid_mass_driver_link,
};
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::network::wire::encode::encode_build_destroyed_frame;
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;
use tracing::{debug, error, info, warn};

use super::*;

pub fn simulate_mass_drivers(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let mut keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == 271)
        .map(|tile| *tile.key())
        .collect();
    keys.sort_unstable();
    let mut changed = false;

    for key in &keys {
        if let Some(mut driver) = world.tiles.get_mut(key) {
            let efficiency = power.get(key).copied().unwrap_or(0.0);
            let scaled_delta = delta_ticks * building_time_scale(world, *key);
            driver.production_progress =
                (driver.production_progress - scaled_delta * efficiency).max(0.0);
            for (_, _, _, remaining) in &mut driver.mass_driver_incoming {
                *remaining = (*remaining - scaled_delta).max(0.0);
            }
            changed |= driver.production_progress > 0.0 || !driver.mass_driver_incoming.is_empty();
        }
        let mut completed_sources = Vec::new();
        loop {
            let arrived = world.tiles.get(key).and_then(|driver| {
                driver
                    .mass_driver_incoming
                    .iter()
                    .position(|(_, _, _, remaining)| *remaining <= 0.0)
                    .map(|index| (index, driver.mass_driver_incoming[index]))
            });
            let Some((index, (source, item, amount, _))) = arrived else {
                break;
            };
            if let Some(mut driver) = world.tiles.get_mut(key) {
                let capacity = (240 - inventory_total(&driver.inventory)).max(0);
                inventory_add(&mut driver.inventory, item, amount.min(capacity));
                if index < driver.mass_driver_incoming.len() {
                    driver.mass_driver_incoming.remove(index);
                }
                driver.production_progress = 200.0;
                changed = true;
            }
            completed_sources.push(source);
        }
        for source in completed_sources {
            let still_in_flight = world.tiles.get(key).is_some_and(|driver| {
                driver
                    .mass_driver_incoming
                    .iter()
                    .any(|(queued_source, _, _, _)| *queued_source == source)
            });
            if !still_in_flight {
                if let Some(mut receiver) = world.tiles.get_mut(key) {
                    let old_len = receiver.mass_driver_waiting.len();
                    receiver
                        .mass_driver_waiting
                        .retain(|queued| *queued != source);
                    changed |= receiver.mass_driver_waiting.len() != old_len;
                }
            }
        }
    }

    // Preserve queue order, but discard shooters whose power/link/range is no longer valid.
    for key in &keys {
        let waiting = world
            .tiles
            .get(key)
            .map(|driver| driver.mass_driver_waiting.clone())
            .unwrap_or_default();
        let mut cleaned = Vec::new();
        for shooter in waiting {
            if cleaned.contains(&shooter) {
                continue;
            }
            let valid = world.tiles.get(&shooter).is_some_and(|source| {
                source.block == 271
                    && power.get(&shooter).copied().unwrap_or(0.0) > 0.0
                    && valid_mass_driver_link(world, &source) == Some(*key)
            });
            if valid {
                cleaned.push(shooter);
            }
        }
        if let Some(mut receiver) = world.tiles.get_mut(key) {
            if receiver.mass_driver_waiting != cleaned {
                receiver.mass_driver_waiting = cleaned;
                changed = true;
            }
        }
    }

    // Sources join the target's ordered waiting set before either side aligns.
    for key in &keys {
        let Some(source) = world.tiles.get(key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(target_key) = valid_mass_driver_link(world, &source) else {
            if let Some((item, _)) = source.inventory.first().copied() {
                changed |= dump_factory_output(world, *key, item);
            }
            continue;
        };
        let efficiency = power.get(key).copied().unwrap_or(0.0);
        if efficiency <= 0.0 || inventory_total(&source.inventory) < 10 {
            continue;
        }
        let Some(target) = world.tiles.get(&target_key).map(|tile| tile.clone()) else {
            continue;
        };
        if 120 - inventory_total(&target.inventory) < 10 {
            continue;
        }
        if let Some(mut receiver) = world.tiles.get_mut(&target_key) {
            if !receiver.mass_driver_waiting.contains(key) {
                receiver.mass_driver_waiting.push(*key);
                changed = true;
            }
        }
    }

    // Match the official 5 degrees/tick alignment for accepting and ready shooters.
    for key in &keys {
        let Some(driver) = world.tiles.get(key).map(|tile| tile.clone()) else {
            continue;
        };
        let efficiency = power.get(key).copied().unwrap_or(0.0);
        if efficiency <= 0.0 {
            continue;
        }
        // Round 74: the alignment targets use explicit machine conditions
        // instead of the SYNC state (mass_driver_state now reflects the
        // official idle/accepting/shooting visuals, not the readiness).
        let target_rotation = if !driver.mass_driver_waiting.is_empty() {
            // accepting: turn toward the first waiting shooter.
            driver
                .mass_driver_waiting
                .first()
                .copied()
                .map(|shooter| angle_between(driver.position, shooter))
        } else if driver.production_progress <= 0.0001 && inventory_total(&driver.inventory) >= 10 {
            // ready to shoot: turn toward the link.
            valid_mass_driver_link(world, &driver).and_then(|target| {
                world.tiles.get(&target).and_then(|receiver| {
                    (120 - inventory_total(&receiver.inventory) >= 10)
                        .then_some(angle_between(driver.position, target))
                })
            })
        } else {
            None
        };
        if let Some(target_rotation) = target_rotation {
            let rotation = move_toward_angle(
                driver.mass_driver_rotation,
                target_rotation,
                5.0 * efficiency * delta_ticks.max(0.0),
            );
            if let Some(mut live) = world.tiles.get_mut(key) {
                if (live.mass_driver_rotation - rotation).abs() > 0.0001 {
                    live.mass_driver_rotation = rotation;
                    changed = true;
                }
            }
        }
    }

    for key in keys {
        let Some(source) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if valid_mass_driver_link(world, &source).is_none()
            || power.get(&key).copied().unwrap_or(0.0) <= 0.0
            || source.production_progress > 0.0001
            || inventory_total(&source.inventory) < 10
        {
            continue;
        }
        let Some(target_key) = valid_mass_driver_link(world, &source) else {
            continue;
        };
        let Some(target) = world.tiles.get(&target_key).map(|tile| tile.clone()) else {
            continue;
        };
        let target_rotation = angle_between(source.position, target.position);
        let target_accepting = !target.mass_driver_waiting.is_empty()
            && 120 - inventory_total(&target.inventory) >= 10;
        if !target_accepting
            || target.mass_driver_waiting.first().copied() != Some(key)
            || !angle_near(source.mass_driver_rotation, target_rotation, 2.0)
            || !angle_near(target.mass_driver_rotation, target_rotation + 180.0, 2.0)
        {
            continue;
        }
        let sx = (source.position >> 16) as i16 as f32 * 8.0;
        let sy = source.position as i16 as f32 * 8.0;
        let tx = (target.position >> 16) as i16 as f32 * 8.0;
        let ty = target.position as i16 as f32 * 8.0;
        let travel_ticks = ((tx - sx).hypot(ty - sy) / 5.5).min(200.0);
        let mut cargo = Vec::new();
        let mut total = 0;
        for (item, amount) in &source.inventory {
            let transferred = (*amount).min(120 - total);
            if transferred > 0 {
                cargo.push((*item, transferred));
                total += transferred;
            }
            if total >= 120 {
                break;
            }
        }
        if total < 10 {
            continue;
        }
        if let Some(mut sender) = world.tiles.get_mut(&key) {
            for (item, amount) in &cargo {
                inventory_remove(&mut sender.inventory, *item, *amount);
            }
            sender.production_progress = 200.0;
        }
        if let Some(mut receiver) = world.tiles.get_mut(&target_key) {
            for (item, amount) in cargo {
                receiver
                    .mass_driver_incoming
                    .push((key, item, amount, travel_ticks));
            }
        }
        changed = true;
    }
    changed
}

pub fn simulate_generators(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == 308)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(mut generator) = world.tiles.get_mut(&key) else {
            continue;
        };
        if generator.production_progress <= 0.0 {
            let fuel = generator
                .inventory
                .iter()
                .find_map(|(item, amount)| (*amount > 0).then_some(*item))
                .filter(|item| generator_fuel(*item).is_some());
            if let Some(item) = fuel {
                let (_, duration_multiplier) = generator_fuel(item).unwrap();
                if inventory_remove(&mut generator.inventory, item, 1) {
                    generator.stored_item = item;
                    generator.production_progress = 120.0 * duration_multiplier;
                    changed = true;
                }
            }
        }
        if generator.production_progress > 0.0 {
            generator.production_progress = (generator.production_progress
                - delta_ticks * building_time_scale(world, key))
            .max(0.0);
            if generator.production_progress == 0.0 {
                generator.stored_item = -1;
            }
            changed = true;
        }
    }
    changed
}

pub fn simulate_reactors_with_network(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let (mut changed, overheats) = reactor::tick(world, delta_ticks);
    changed |= simulate_impact_reactors(world, delta_ticks);

    for overheat in overheats {
        // NuclearReactor.updateTile calls kill() at heat >= .999. Sending the
        // ordinary BuildDestroyed call lets the official client run its own
        // reactor explosion effect/sound while the server remains authority
        // for the resulting radial damage.
        if damage_building(world, overheat.position, f32::MAX).is_some() {
            if let Ok(frame) = encode_build_destroyed_frame(overheat.position) {
                out.broadcast(frame);
            }
            changed = true;
        }
        if overheat.explosion_enabled {
            let x = (overheat.position >> 16) as i16 as f32 * 8.0;
            let y = overheat.position as i16 as f32 * 8.0;
            // The two existing splash services partition default-team and
            // opposing targets. Together they implement Damage.damage's
            // team-agnostic reactor blast without duplicating packet logic.
            changed |= apply_allied_splash_damage(
                world,
                out,
                x,
                y,
                reactor::EXPLOSION_DAMAGE,
                reactor::EXPLOSION_RADIUS,
                1.0,
                -1,
                0.0,
            );
            changed |= apply_enemy_splash_damage(
                world,
                out,
                x,
                y,
                reactor::EXPLOSION_DAMAGE,
                reactor::EXPLOSION_RADIUS,
                1.0,
                -1,
                0.0,
                1.0,
            );
        }
    }
    changed
}

/// ImpactReactor simulation: consumes blast compound (item 14, 140 ticks duration)
/// and cryofluid (liquid 3, 0.25/tick = 15/s). Warmup approaches 1.0 at 0.001/tick
/// (matching official ImpactReactor.warmupSpeed) and stores the warmup float in
/// `output_liquid_amount` so `encode_power_generator_sync` broadcasts the exact state.
pub fn simulate_impact_reactors(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == 316)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    let power = crate::network::economy::compute_power_efficiency(world);
    for key in keys {
        // Snapshot power.status and overdrive scale before mutating the
        // reactor tile (DM004).
        let status = power.get(&key).copied().unwrap_or(0.0);
        let time_scale = building_time_scale(world, key);
        let Some(mut reactor) = world.tiles.get_mut(&key) else {
            continue;
        };
        let fuel_item = 14;
        let duration = 140.0;
        let scaled_delta = delta_ticks * time_scale;

        let cryo_amount = crate::network::economy::stored_liquid_amount(&reactor, 3);
        let cryo_rate = (0.25 * scaled_delta).min(cryo_amount);
        // ConsumeLiquid.efficiency needs enough cryo for one edelta of
        // consumePower's companion consumeLiquid(0.25). Warmup only climbs
        // when efficiency >= 0.9999, i.e. ~0.25 units buffered.
        let has_cryo = cryo_amount >= 0.25 * scaled_delta.max(0.000_001);

        if reactor.production_progress <= 0.0
            && inventory_remove(&mut reactor.inventory, fuel_item, 1)
        {
            reactor.stored_item = fuel_item;
            reactor.production_progress += duration;
            changed = true;
        }
        let operating = (reactor.production_progress > 0.0
            || inventory_count(&reactor.inventory, fuel_item) > 0)
            && has_cryo
            && reactor.enabled
            && status >= 0.99;
        let old_warmup = reactor.output_liquid_amount;
        // Mindustry: warmup = Mathf.lerpDelta(warmup, 1f, 0.001f * timeScale).
        // Cooldown uses a smooth decay so transient conveyor delays do not wipe warmup.
        let new_warmup = if operating {
            let lerped = old_warmup + (1.0 - old_warmup) * (0.001 * scaled_delta).clamp(0.0, 1.0);
            if (1.0 - lerped).abs() <= 0.001 {
                1.0
            } else {
                lerped
            }
        } else {
            let lerped = old_warmup + (0.0 - old_warmup) * (0.01 * scaled_delta).clamp(0.0, 1.0);
            if lerped <= 0.0001 {
                0.0
            } else {
                lerped
            }
        };

        if (new_warmup - old_warmup).abs() > f32::EPSILON {
            reactor.output_liquid_amount = new_warmup;
            changed = true;
        }

        if operating {
            if let Some((_, amount)) = reactor
                .liquid_inventory
                .iter_mut()
                .find(|(liquid, _)| *liquid == 3)
            {
                *amount = (*amount - cryo_rate).max(0.0);
            } else {
                reactor.liquid_amount = (reactor.liquid_amount - cryo_rate).max(0.0);
                if reactor.liquid_amount <= 0.0001 {
                    reactor.liquid_amount = 0.0;
                    if reactor.stored_liquid == 3 {
                        reactor.stored_liquid = -1;
                    }
                }
            }
            if reactor.production_progress > 0.0 {
                reactor.production_progress = (reactor.production_progress - scaled_delta).max(0.0);
            }
            if reactor.production_progress <= 0.0 {
                if inventory_remove(&mut reactor.inventory, fuel_item, 1) {
                    reactor.stored_item = fuel_item;
                    reactor.production_progress += duration;
                } else {
                    reactor.stored_item = -1;
                }
            }
            changed = true;
        }
    }
    changed
}
