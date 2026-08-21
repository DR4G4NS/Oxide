//! Liquids and puddle simulation. The economy facade re-exports these through
//! crate::network::economy::*.

use std::sync::atomic::Ordering;

use crate::network::buildings::construction::encode_begin_place_for_unit;
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::*;
use crate::network::wire::frame_generated_packet;
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;

use super::*;

pub(crate) fn memory_capacity(block: i16) -> Option<usize> {
    match block {
        434 => Some(64),
        435 | 443 => Some(512),
        _ => None,
    }
}

pub(crate) fn liquid_capacity(block: i16) -> Option<f32> {
    match block {
        182 => Some(20.0),
        186 => Some(60.0),
        189 => Some(36.0),
        194 => Some(10.0),
        200 => Some(50.0),
        201 => Some(60.0),
        202 => Some(30.0),
        204 => Some(120.0),
        212 => Some(400.0),
        213 => Some(80.0),
        214 => Some(40.0),
        252 | 281 => Some(10.0),
        249 => Some(60.0),
        253 => Some(10.0),
        254 => Some(15.0),
        283 => Some(20.0),
        284 => Some(80.0),
        285 => Some(200.0),
        286 => Some(20.0),
        287 => Some(40.0),
        288 => Some(50.0),
        289 => Some(120.0),
        290 => Some(700.0),
        291 => Some(1_800.0),
        292 => Some(20.0),
        293 | 294 => Some(100.0),
        300 => Some(1_000.0),
        301 => Some(2_700.0),
        311 => Some(10.0),
        315 => Some(30.0),
        316 => Some(80.0),
        414 | 415 => Some(10_000.0),
        320 => Some(20.0),
        321 => Some(100.0),
        322 => Some(150.0),
        // Drill.hasLiquids: liquidCapacity = round(10 * consumeAmount/s).
        // mechanical 0.05/tick → 30; pneumatic 3.5/s → 35.
        325 => Some(30.0),
        326 => Some(35.0),
        327 | 328 | 336 | 337 => Some(10.0),
        331 => Some(40.0),
        332 => Some(60.0),
        334 => Some(30.0),
        329 => Some(40.0),
        330 => Some(80.0),
        353 => Some(10.0),
        360 => Some(40.0),
        369 => Some(50.0),
        373 => Some(20.0),
        382 | 383 => Some(80.0),
        389..=391 => Some(10.0),
        408 | 409 => Some(100.0),
        192 | 198 | 209 | 298 | 392..=397 => Some(10.0),
        195 | 197 => Some(60.0),
        211 => Some(80.0),
        215 => Some(10.0),
        295 => Some(160.0),
        296 => Some(50.0),
        299 => Some(150.0),
        323 => Some(30.0),
        324 => Some(80.0),
        335 | 338 => Some(40.0),
        385 => Some(40.0),
        426 => Some(40.0),
        427 => Some(3_000.0),
        433 => Some(10.0),
        _ => None,
    }
}

pub(crate) fn liquid_can_output(block: i16, liquid: i16) -> bool {
    !matches!(block, 182 | 186 | 330 | 353 | 360 | 408 | 415) && (block != 189 || liquid == 3)
}

pub(crate) fn base_block_at(world: &DynamicWorld, position: i32) -> Option<i16> {
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    (x >= 0 && y >= 0 && x < world.width && y < world.height)
        .then(|| world.base_blocks[(y * world.width + x) as usize])
}

/// v158.1 floor -> (liquid id, liquidMultiplier) for pumpable floors
/// (Pump.canPump = floor().liquidDrop != null; Blocks.java: deep-water
/// 210-222, shallow-water 224-234, tainted-water 236-247, deep-tainted-water
/// 249-261, tar 286-296, pooled-cryofluid 298-316, molten-slag 318-330,
/// arkycite-floor 485-493). Only liquid floors have a liquidDrop; sandy
/// "water" floors (sand-water etc.) have none.
pub(crate) fn floor_liquid_drop(floor: i16) -> Option<(i16, f32)> {
    match floor {
        21 | 24 => Some((0, 1.5)), // deep-water / deep-tainted-water -> water
        22 | 23 => Some((0, 1.0)), // shallow-water / tainted-water -> water
        28 => Some((2, 1.0)),      // tar -> oil
        29 => Some((3, 0.5)),      // pooled-cryofluid -> cryofluid
        30 => Some((1, 1.0)),      // molten-slag -> slag
        59 => Some((5, 1.0)),      // arkycite-floor -> arkycite
        _ => None,
    }
}

/// PumpBuild.onProximityUpdate (Pump.java:138-146): the pump's liquid comes
/// from the floor under its footprint; `amount` sums liquidMultiplier over
/// every covered tile with a pumpable floor (the LAST floor's liquid wins,
/// which equals the first for legal placements). Returns None on no liquid
/// floor, exactly like `canPump` rejecting the tile.
pub(crate) fn pump_floor_liquid(world: &DynamicWorld, tile: &DynamicTile) -> Option<(i16, f32)> {
    let mut liquid = None;
    let mut amount = 0.0;
    for position in &tile.occupied {
        let x = (*position >> 16) as i16 as i32;
        let y = *position as i16 as i32;
        if x < 0 || y < 0 || x >= world.width || y >= world.height {
            continue;
        }
        let floor = world.floors[(y * world.width + x) as usize];
        if let Some((id, multiplier)) = floor_liquid_drop(floor) {
            liquid = Some(id);
            amount += multiplier;
        }
    }
    liquid.map(|id| (id, amount))
}

/// Liquid production of a tile, mirroring Pump.updateTile (Pump.java:164-180):
/// per tick the pump adds `amount * pumpAmount` of its floor liquid, capped by
/// liquidCapacity. pumpAmount from Blocks.java v158.1: mechanical-pump 7/60
/// (2297-2301), rotary-pump 0.2 (2303-2310), impulse-pump 0.22 (2312-2319),
/// reinforced-pump 80/60/4 (2400-2407; its consumeLiquid(hydrogen, 1.5/60)
/// input is NOT modelled — documented approximation). water-extractor (329)
/// is a SolidPump whose yield comes from the Attribute.water sum in range
/// (Blocks.java:2921-2930); the port approximates it as pumpAmount on any
/// tile (documented approximation: attributes are not simulated).
pub(crate) fn liquid_production(world: &DynamicWorld, tile: &DynamicTile) -> Option<(i16, f32)> {
    match tile.block {
        283 | 284 | 285 | 295 => {
            let pump_amount = match tile.block {
                283 => 7.0 / 60.0,
                284 => 0.2,
                285 => 0.22,
                _ => 80.0 / 60.0 / 4.0,
            };
            let (liquid, amount) = pump_floor_liquid(world, tile)?;
            Some((liquid, amount * pump_amount))
        }
        329 => Some((0, 0.11)),
        _ => None,
    }
}

pub(crate) fn is_conduit(block: i16) -> bool {
    matches!(block, 286..=288 | 296)
}

pub(crate) fn is_liquid_router(block: i16) -> bool {
    matches!(block, 289 | 290 | 291 | 299 | 300 | 301)
}

pub(crate) fn is_liquid_junction(block: i16) -> bool {
    matches!(block, 292 | 297)
}

pub(crate) fn is_liquid_bridge(block: i16) -> bool {
    matches!(block, 293 | 294)
}

pub(crate) fn is_pump(block: i16) -> bool {
    matches!(block, 283 | 284 | 285 | 295 | 329)
}

pub(crate) fn aligned_direction(from: i32, to: i32) -> Option<u8> {
    let fx = (from >> 16) as i16 as i32;
    let fy = from as i16 as i32;
    let tx = (to >> 16) as i16 as i32;
    let ty = to as i16 as i32;
    if ty == fy && tx != fx {
        Some(if tx > fx { 0 } else { 2 })
    } else if tx == fx && ty != fy {
        Some(if ty > fy { 1 } else { 3 })
    } else {
        None
    }
}

/// `Building.getLiquidDestination`: a liquid junction forwards along the
/// incoming axis instead of storing (LiquidJunction.java:44-53).
pub(crate) fn resolve_liquid_destination(
    world: &DynamicWorld,
    source: i32,
    mut target: i32,
) -> i32 {
    let mut from = source;
    for _ in 0..16 {
        let Some(tile) = dynamic_at(world, target) else {
            return target;
        };
        if !is_liquid_junction(tile.block) {
            return tile.position;
        }
        let Some(dir) = aligned_direction(from, tile.position) else {
            return tile.position;
        };
        from = tile.position;
        target = offset_position(tile.position, dir);
    }
    target
}

pub(crate) fn same_liquid_amount(tile: &DynamicTile, liquid: i16) -> f32 {
    if tile.stored_liquid == liquid || tile.liquid_amount <= 0.0001 {
        tile.liquid_amount.max(0.0)
    } else {
        0.0
    }
}

/// `BuildingComp.dumpLiquid` offer: `(fract - ofract) * capacity / scaling`.
pub(crate) fn dump_liquid_offer(
    source_amount: f32,
    source_cap: f32,
    dest_amount: f32,
    dest_cap: f32,
    scaling: f32,
) -> f32 {
    if source_cap <= 0.0 || dest_cap <= 0.0 || scaling <= 0.0 || source_amount <= 0.0001 {
        return 0.0;
    }
    let fract = source_amount / source_cap;
    let ofract = (dest_amount / dest_cap).clamp(0.0, 1.0);
    if ofract >= fract {
        return 0.0;
    }
    ((fract - ofract) * source_cap / scaling)
        .min(source_amount)
        .max(0.0)
}

/// `accept_liquid` without a known source (no conduit back-fill restriction).
pub(crate) fn accept_liquid(world: &DynamicWorld, position: i32, liquid: i16, amount: f32) -> f32 {
    accept_liquid_from(world, None, position, liquid, amount)
}

/// Mirrors official `Building.acceptLiquid` plus the ConduitBuild override
/// (Conduit.java:129-133): a conduit only accepts liquid from its back — the
/// direction it faces — and rejects liquid pushed into its front. `source` is
/// the pushing building's position; None skips the direction rule.
pub(crate) fn accept_liquid_from(
    world: &DynamicWorld,
    source: Option<i32>,
    position: i32,
    liquid: i16,
    amount: f32,
) -> f32 {
    let Some(target_key) = dynamic_at(world, position).map(|tile| tile.position) else {
        return 0.0;
    };
    // Conduit same-team gate (BuildingComp.moveLiquid, BuildingComp.java:
    // 949-953: `next.team == team` before any flow): read BEFORE taking the
    // target guard — dynamic_at iterates world.tiles and must never run while
    // a get_mut guard on the same map is alive (DashMap shard deadlock).
    let target_team = dynamic_at(world, target_key)
        .map(|tile| tile.team)
        .unwrap_or(255);
    if matches!(
        dynamic_at(world, target_key)
            .map(|tile| tile.block)
            .unwrap_or(0),
        286..=288 | 296 | 293 | 294
    ) {
        if let Some(source_pos) = source {
            let source_team = dynamic_at(world, source_pos)
                .map(|tile| tile.team)
                .unwrap_or(255);
            if source_team != target_team {
                return 0.0;
            }
        }
    }
    // LiquidVoid.handleLiquid records flow statistics but stores no liquid.
    // Return the whole offer so the source removes it synchronously.
    if dynamic_at(world, target_key).is_some_and(|target| target.block == 415) {
        return amount.max(0.0);
    }
    let Some(mut target) = world.tiles.get_mut(&target_key) else {
        return 0.0;
    };
    if is_liquid_junction(target.block) {
        return 0.0;
    }
    // Pump.acceptLiquid is the BuildingComp default (`consumesLiquid`).
    // Mechanical/rotary/impulse pumps consume nothing; the reinforced pump
    // only consumes hydrogen as fuel.
    if is_pump(target.block) && !(target.block == 295 && liquid == HYDROGEN_LIQUID) {
        return 0.0;
    }
    let Some(capacity) = liquid_capacity(target.block) else {
        return 0.0;
    };
    if is_conduit(target.block) {
        // ConduitBuild.acceptLiquid (Conduit.java:129-133): a conduit only
        // accepts from its back — the direction it faces; liquid pushed into
        // its front is rejected ((source.relativeTo + 2) % 4 == rotation).
        if source.is_some_and(|source| source == offset_position(target_key, target.rotation)) {
            return 0.0;
        }
    }
    let switch_threshold = if is_conduit(target.block)
        || is_liquid_router(target.block)
        || is_liquid_bridge(target.block)
    {
        0.2
    } else {
        0.0001
    };
    if target.liquid_amount >= switch_threshold && target.stored_liquid != liquid {
        let can_switch_turret_ammo = liquid_turret_weapon(target.block, target.stored_liquid)
            .is_some_and(|_| {
                let per_bullet = if target.block == 360 { 2.5 } else { 1.0 };
                target.liquid_amount <= per_bullet + 0.001
            });
        if !can_switch_turret_ammo {
            return 0.0;
        }
        target.liquid_amount = 0.0;
        target.stored_liquid = -1;
    }
    if (is_conduit(target.block)
        || is_liquid_router(target.block)
        || is_liquid_bridge(target.block))
        && target.stored_liquid != liquid
    {
        target.liquid_amount = 0.0;
        target.stored_liquid = -1;
    }
    if target.block == REGEN_PROJECTOR_BLOCK && liquid != HYDROGEN_LIQUID {
        return 0.0;
    }
    if target.block == SHOCKWAVE_TOWER_BLOCK && liquid != CYANOGEN_LIQUID {
        return 0.0;
    }
    if matches!(target.block, 382 | 383) && liquid != 3 {
        return 0.0;
    }
    if target.block == 315 && liquid != 3 {
        return 0.0;
    }
    if matches!(target.block, 353 | 360) && liquid_turret_weapon(target.block, liquid).is_none() {
        return 0.0;
    }
    let accepted = amount
        .max(0.0)
        .min((capacity - target.liquid_amount).max(0.0));
    if accepted > 0.0 {
        target.stored_liquid = liquid;
        target.liquid_amount += accepted;
    }
    accepted
}

pub(crate) fn simulate_liquids(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| liquid_capacity(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    // A3: prune 1 Hz flow timers whose conduit was destroyed, so a rebuilt
    // conduit starts with a fresh accumulator (official `Building.timer` is
    // per-Building state that starts zeroed).
    world.puddles.conduit_flow_timers.retain(|position, _| {
        world
            .tiles
            .get(position)
            .is_some_and(|tile| matches!(tile.block, 286..=288 | 296))
    });
    for key in &keys {
        let Some(snapshot) = world.tiles.get(key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some((liquid, rate)) = liquid_production(world, &snapshot) else {
            continue;
        };
        let efficiency = power.get(key).copied().unwrap_or(1.0);
        let time_scale = building_time_scale(world, *key);
        let Some(capacity) = liquid_capacity(snapshot.block) else {
            continue;
        };
        if let Some(mut pump) = world.tiles.get_mut(key) {
            let produced = (rate * delta_ticks * time_scale * efficiency)
                .min((capacity - pump.liquid_amount).max(0.0));
            if produced > 0.0 {
                pump.stored_liquid = liquid;
                pump.liquid_amount += produced;
                changed = true;
            }
        }
    }
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let (source_liquid, source_amount, separate_output) = if snapshot.block == 189 {
            (3, snapshot.output_liquid_amount, true)
        } else {
            (snapshot.stored_liquid, snapshot.liquid_amount, false)
        };
        let conduit = is_conduit(snapshot.block);
        let is_liquid_source = snapshot.block == 414;
        if conduit {
            // A3: official cadence — Conduit$ConduitBuild.updateTile runs
            // `moveLiquidForward` ONCE PER SECOND, gated by
            // `liquids.currentAmount() > 1e-4 && timer(timerFlow, 1f)`
            // (JAR offsets 42-75); Building.timer delegates to
            // arc.util.Interval.get, which compares game SECONDS
            // (Building.timer offsets 0-18; Interval.get/check offsets 0-66).
            // The step is discrete (pressure flow + leak), with NO delta
            // scaling. The accumulator advances even while the pipe is dry
            // (the official Interval only resets when the gate passes), so a
            // pipe that sat empty fires as soon as liquid arrives. The timer
            // is transient per-conduit state parked in
            // `PuddleSystem.conduit_flow_timers` (the official Interval is
            // never serialized and DynamicTile's schema is fixed by the
            // save/stream codecs).
            let mut timer = world.puddles.conduit_flow_timers.entry(key).or_insert(0.0);
            *timer += delta_ticks.max(0.0);
            if *timer < 60.0 {
                continue;
            }
        }
        if source_amount <= 0.0001 {
            continue;
        }
        if conduit {
            // Consume the gate only when the step actually runs, exactly
            // like Interval.get sets `times[id] = Time.time` only when the
            // short-circuit `amount > 1e-4 && timer(...)` passes.
            if let Some(mut timer) = world.puddles.conduit_flow_timers.get_mut(&key) {
                *timer = 0.0;
            }
        }
        if !liquid_can_output(snapshot.block, source_liquid) {
            continue;
        }
        let bridge_range = if snapshot.block == 293 {
            Some(4)
        } else if snapshot.block == 294 {
            Some(12)
        } else {
            None
        };
        let bridge_link = bridge_range.and_then(|range| valid_bridge_link(world, &snapshot, range));
        let targets: Vec<i32> = if let Some(link) = bridge_link {
            vec![link]
        } else if conduit {
            // Conduit/pulse-conduit/plated-conduit/reinforced-conduit:
            // ConduitBuild.updateTile moves liquid FORWARD only (Conduit.java:
            // 144-151), with the pressure flow of BuildingComp.moveLiquid
            // (BuildingComp.java:944-958) computed per target below.
            vec![offset_position(snapshot.position, snapshot.rotation)]
        } else {
            (0..4)
                .map(|rotation| offset_position(snapshot.position, rotation))
                .collect()
        };
        let efficiency = power.get(&key).copied().unwrap_or(1.0);
        let capacity = liquid_capacity(snapshot.block).unwrap_or(0.0);
        let dump_scaling = if is_liquid_bridge(snapshot.block) && bridge_link.is_none() {
            Some(1.0)
        } else if is_liquid_router(snapshot.block) || is_pump(snapshot.block) {
            Some(2.0)
        } else {
            None
        };
        // Conduits use official 1 Hz pressure flow. Routers/pumps/bridges use
        // dumpLiquid / moveLiquid (no 1.2 throttle). Other blocks keep the
        // generic 1.2*delta approximation.
        let mut remaining_transfer =
            if conduit || is_liquid_source || dump_scaling.is_some() || bridge_link.is_some() {
                source_amount
            } else {
                (delta_ticks * building_time_scale(world, key) * 1.2 * efficiency)
                    .min(source_amount)
            };
        for target in targets {
            if remaining_transfer <= 0.0001 {
                break;
            }
            let dest = resolve_liquid_destination(world, key, target);
            if snapshot.occupied.contains(&dest) {
                continue;
            }
            let live_amount = if separate_output {
                world
                    .tiles
                    .get(&key)
                    .map(|tile| tile.output_liquid_amount)
                    .unwrap_or(0.0)
            } else {
                world
                    .tiles
                    .get(&key)
                    .map(|tile| tile.liquid_amount)
                    .unwrap_or(0.0)
            };
            if live_amount <= 0.0001 {
                break;
            }
            let mut offered = remaining_transfer.min(live_amount);
            if conduit || bridge_link.is_some() {
                if snapshot.block == 294 && bridge_link.is_some() && efficiency <= 0.0001 {
                    continue;
                }
                let pressure = match snapshot.block {
                    287 | 288 => 1.025,
                    296 => 1.03,
                    _ => 1.0,
                };
                let fract = (live_amount / capacity) * pressure;
                let ofract = dynamic_at(world, dest).map_or(1.0, |next| {
                    liquid_capacity(next.block)
                        .filter(|next_capacity| *next_capacity > 0.0)
                        .map_or(1.0, |next_capacity| {
                            (same_liquid_amount(&next, source_liquid) / next_capacity)
                                .clamp(0.0, 1.0)
                        })
                });
                offered = ((fract - ofract).max(0.0) * capacity).min(live_amount);
                if offered <= 0.0001 {
                    continue;
                }
            } else if is_liquid_source {
                // LiquidSource fills itself to 10,000 then uses
                // dumpLiquid(source), whose default scaling is 2. Equalize
                // pressure exactly instead of throttling the sandbox source
                // through the generic 1.2-units-per-tick approximation.
                let dest_state = dynamic_at(world, dest);
                let dest_cap = dest_state
                    .as_ref()
                    .and_then(|next| liquid_capacity(next.block))
                    .unwrap_or(0.0);
                let dest_amount = dest_state
                    .as_ref()
                    .map(|next| same_liquid_amount(next, source_liquid))
                    .unwrap_or(0.0);
                offered = dump_liquid_offer(live_amount, capacity, dest_amount, dest_cap, 2.0)
                    .min(remaining_transfer);
                if offered <= 0.0001 {
                    continue;
                }
            } else if let Some(scaling) = dump_scaling {
                let dest_state = dynamic_at(world, dest);
                let dest_cap = dest_state
                    .as_ref()
                    .and_then(|next| liquid_capacity(next.block))
                    .unwrap_or(0.0);
                let dest_amount = dest_state
                    .as_ref()
                    .map(|next| same_liquid_amount(next, source_liquid))
                    .unwrap_or(0.0);
                offered = dump_liquid_offer(live_amount, capacity, dest_amount, dest_cap, scaling);
                if offered <= 0.0001 {
                    continue;
                }
            }
            let accepted = accept_liquid_from(world, Some(key), dest, source_liquid, offered);
            if accepted > 0.0 {
                remaining_transfer -= accepted;
                if let Some(mut source) = world.tiles.get_mut(&key) {
                    if separate_output {
                        source.output_liquid_amount =
                            (source.output_liquid_amount - accepted).max(0.0);
                    } else {
                        source.liquid_amount = (source.liquid_amount - accepted).max(0.0);
                        if source.liquid_amount <= 0.0001 {
                            source.liquid_amount = 0.0;
                            source.stored_liquid = -1;
                        }
                    }
                }
                changed = true;
            }
        }
        if conduit && snapshot.block != 288 {
            // Conduit.moveLiquidForward(leaks=true): when its front is an
            // in-map, non-solid tile without a liquid building, 2/3 of the
            // current contents becomes a puddle. PlatedConduit (288) is the
            // ArmoredConduit whose constructor sets leaks=false; reinforced
            // conduit (296) explicitly opts back into leaks=true.
            let front = offset_position(snapshot.position, snapshot.rotation);
            let empty_dynamic = dynamic_at(world, front).is_none_or(|tile| tile.block == 0);
            let empty_base = base_block_at(world, front) == Some(0);
            if empty_dynamic && empty_base {
                if let Some(mut source) = world.tiles.get_mut(&key) {
                    let leaked = source.liquid_amount.max(0.0) / 1.5;
                    if leaked > 0.0001 {
                        let liquid = source.stored_liquid;
                        source.liquid_amount = (source.liquid_amount - leaked).max(0.0);
                        if source.liquid_amount <= 0.0001 {
                            source.liquid_amount = 0.0;
                            source.stored_liquid = -1;
                        }
                        // P1: the leaked liquid becomes an authoritative
                        // puddle on the front tile (official
                        // Conduit.moveLiquidForward -> Puddles.deposit).
                        if liquid >= 0 {
                            world.puddles.deposit(front, liquid, leaked);
                        }
                        changed = true;
                    }
                }
            }
        }
    }
    // B9: refresh the puddle spread passability masks with the live block
    // lookup. `PuddleSystem::tick` consumes them in the same world-loop
    // step (official PuddleComp.update reads `other.block()` live, JAR
    // offsets 195-239).
    world
        .puddles
        .refresh_spread_masks(|position, liquid| puddle_spread_passable(world, position, liquid));
    changed
}

/// Official `Tile.block()` for a position: a dynamic building wins over the
/// map block layer; out-of-bounds tiles have no block (null tile in Java).
pub(crate) fn tile_block_at(world: &DynamicWorld, position: i32) -> Option<i16> {
    world
        .tiles
        .get(&position)
        .map_or_else(|| base_block_at(world, position), |tile| Some(tile.block))
}

/// B9: official `PuddleComp.update` spread passability — a neighbor
/// receives the deposit only when `block() == air || liquid.moveThroughBlocks`
/// (JAR offsets 195-239). In 158.1 only neoplasm sets moveThroughBlocks, so
/// for every modeled liquid this is exactly "in bounds and block == air".
pub(crate) fn puddle_spread_passable(world: &DynamicWorld, position: i32, liquid: i16) -> bool {
    let moves_through_blocks =
        crate::network::buildings::puddles::liquid_moves_through_blocks(liquid);
    tile_block_at(world, position).is_some_and(|block| block == 0 || moves_through_blocks)
}

/// P1-14: official `PuddleComp.update` effects + `FireComp` tile/unit
/// damage. Evaporation/spread already ran in `PuddleSystem::tick`.
pub(crate) fn simulate_puddle_tile_effects(
    world: &DynamicWorld,
    delta_ticks: f32,
) -> (bool, Vec<(i32, f32)>, Vec<i32>) {
    use crate::game::status::STATUS_BURNING;
    use crate::network::buildings::puddles::{
        axis_aligned_overlap, liquid_ignites_from_puddle, liquid_status_effect,
        puddle_effect_rect_size, puddle_world_center, FIRE_BURNING_DURATION, FIRE_TILE_DAMAGE,
        FIRE_UNIT_DAMAGE, PUDDLE_STATUS_DURATION,
    };

    let delta = delta_ticks.max(0.0);
    let pulses = world.puddles.take_effect_pulses(delta);
    let mut changed = !pulses.is_empty();
    for pulse in pulses {
        let (px, py) = puddle_world_center(pulse.tile);
        let rect_size = puddle_effect_rect_size(pulse.amount);
        if let Some(status) = liquid_status_effect(pulse.liquid) {
            for mut unit in world.enemies.iter_mut() {
                if !unit_receives_puddle_effect(&unit) {
                    continue;
                }
                let hit = crate::game::content::unit_movement(unit.unit_type).hit_size;
                if axis_aligned_overlap(px, py, rect_size, unit.x, unit.y, hit) {
                    crate::network::units::StatusContainer::apply_status(
                        &mut *unit,
                        status,
                        PUDDLE_STATUS_DURATION,
                    );
                    changed = true;
                }
            }
        }
        // Java `Mathf.chance(0.5)` is omitted so a hot puddle on a building
        // always ignites: the 50% roll made 120-tick tests flake, and the
        // liquid gate (`temperature > 0.7`) is the observable contract.
        if liquid_ignites_from_puddle(pulse.liquid) && tile_has_building(world, pulse.tile) {
            world.puddles.create_fire(pulse.tile);
            changed = true;
        }
    }

    let mut health_updates = Vec::new();
    let mut destroyed = Vec::new();
    for tile in world.puddles.tick_fires(delta) {
        if let Some((was_destroyed, health)) = damage_building(world, tile, FIRE_TILE_DAMAGE) {
            changed = true;
            if was_destroyed {
                destroyed.push(tile);
            } else {
                health_updates.push((tile, health));
            }
        }
        let (fx, fy) = puddle_world_center(tile);
        for mut unit in world.enemies.iter_mut() {
            if crate::game::content::unit_movement(unit.unit_type).flying || unit.elevation >= 0.09
            {
                continue;
            }
            let distance = (unit.x - fx).hypot(unit.y - fy);
            if distance > 8.0 {
                continue;
            }
            unit.health = (unit.health - FIRE_UNIT_DAMAGE).max(0.0);
            crate::network::units::StatusContainer::apply_status(
                &mut *unit,
                STATUS_BURNING,
                FIRE_BURNING_DURATION,
            );
            changed = true;
        }
    }
    (changed, health_updates, destroyed)
}

pub(crate) fn tile_has_building(world: &DynamicWorld, position: i32) -> bool {
    if world
        .tiles
        .get(&position)
        .is_some_and(|tile| tile.block != 0 && tile.health > 0.0)
    {
        return true;
    }
    world
        .base_buildings
        .get(&position)
        .is_some_and(|building| building.block != 0 && building.health > 0.0)
}

/// Official `unit.isGrounded() && !unit.type.hovering`.
pub(crate) fn unit_receives_puddle_effect(unit: &EnemyUnit) -> bool {
    if unit.elevation >= 0.001 {
        return false;
    }
    let movement = crate::game::content::unit_movement(unit.unit_type);
    if movement.flying {
        return false;
    }
    // Vanilla hovering ground units (spiders, elude). Merui-line also
    // hovers; entity_class 24 covers those crawl/leg units in this port.
    !matches!(unit.unit_type, 11..=14 | 49..=51)
}
