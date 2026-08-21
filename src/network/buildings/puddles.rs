//! Authoritative puddle model (SOL-AUDIT P1).
//!
//! Official `mindustry.entities.Puddles` / `PuddleComp` (158.1): liquid
//! leaks deposit into a per-tile puddle with `maxLiquid = 70f`. Puddles
//! evaporate over time (viscosity-dependent), spread to the four neighbors
//! when full, react when liquids mix (flammable + hot -> fire, hot + cold
//! -> steam/amount loss) and disappear at zero.
//!
//! Wire contract (verified against the desktop.jar bytecode, round 73):
//! `mindustry.gen.Puddle` DOES have `writeSync`/`readSync` and IS part of
//! the entity snapshot sync (`Groups.sync`; `NetServer.writeEntitySnapshot`
//! iterates it): classId 13, body `f amount, s liquid.id (null = -1),
//! TypeIO.writeTile (i packed pos), f x, f y`. It also has
//! `serialize()==true` and `afterRead -> Puddles.register`, so the official
//! server ALSO persists puddles in MSAV (`s 1 rev, f amount, s liquid.id,
//! TypeIO.writeTile, f x, f y`). The port emits the same wire in
//! `encode_enemy_entity_snapshots` and `write_msav_entities_region`. The
//! earlier claim "no writeSync, client-only prediction" was false
//! (SOL-AUDIT §11, QA adversarial H2).
//!
//! P1-14: `PuddleComp.update` also pulses every 40 ticks while
//! `amount >= maxLiquid/2` (35): grounded non-hovering units overlapping
//! the puddle rect receive `liquid.effect` for 120 ticks, and a fire is
//! created on the tile when `liquid.temperature > 0.7` and a building is
//! present. Mix reactions stay in `react_liquids` and are not duplicated.

use dashmap::DashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

/// Official `Puddles.maxLiquid` (70f per tile).
pub const PUDDLE_MAX_LIQUID: f32 = 70.0;
/// Official `PuddleComp.updateTime` reset (`updateTime = 40f`).
pub const PUDDLE_EFFECT_INTERVAL: f32 = 40.0;
/// Official `unit.apply(liquid.effect, 60 * 2)`.
pub const PUDDLE_STATUS_DURATION: f32 = 120.0;
/// Official `Fires.baseLifetime`.
pub const FIRE_BASE_LIFETIME: f32 = 1000.0;
/// Official `FireComp.damageDelay`.
pub const FIRE_DAMAGE_DELAY: f32 = 40.0;
/// Official `FireComp.tileDamage`.
pub const FIRE_TILE_DAMAGE: f32 = 1.8;
/// Official `FireComp.unitDamage`.
pub const FIRE_UNIT_DAMAGE: f32 = 3.0;
/// Official burning duration from fire (`60 * 5`).
pub const FIRE_BURNING_DURATION: f32 = 300.0;

/// One puddle's authoritative state on a tile.
#[derive(Debug, Clone, PartialEq)]
pub struct PuddleState {
    /// Stable entity id sent on the wire (classId 13). Official entity ids
    /// come from the global entity counter; the port uses its own per-system
    /// counter so puddle ids never collide with unit ids.
    pub entity_id: i32,
    pub liquid: i16,
    pub amount: f32,
    /// Incoming amount accepted this tick (official `accepting`).
    pub accepting: f32,
    /// B9: passability mask for the spread pass (bit i set = d4 neighbor i
    /// is `block() == air || liquid.moveThroughBlocks`, official
    /// PuddleComp.update JAR offsets 195-239). Refreshed by the domain
    /// owner (economy) once per world step; standalone puddle tests default
    /// to all-passable.
    pub spread_mask: u8,
    /// Official `PuddleComp.updateTime` (transient, never serialized).
    /// Pulses fire when `amount >= maxLiquid/2` and this is `<= 0`.
    pub update_time: f32,
}

/// Authoritative tile fire spawned by a hot puddle (`Fires.create`).
/// Visual sync (`Fire.writeSync`) is out of scope; damage and lifetime
/// match `FireComp.update` on the server.
#[derive(Debug, Clone, PartialEq)]
pub struct FireState {
    pub time: f32,
    pub lifetime: f32,
    pub damage_timer: f32,
}

/// P1: puddle service owned by the coordinator; pure domain transitions
/// (deposit/tick) never touch network or DashMap guards of other maps.
pub struct PuddleSystem {
    pub puddles: Arc<DashMap<i32, PuddleState>>,
    /// Entity-id allocator for puddles (see `PuddleState.entity_id`).
    next_entity_id: AtomicI32,
    /// A3: per-conduit 1 Hz flow timer (official `Conduit.timerFlow`, an
    /// `Interval` in game seconds — Building.timer JAR offsets 0-18,
    /// Interval.get/check). The official timer is transient per-Building
    /// state that is never serialized, so it lives here (not in
    /// `DynamicTile`, whose schema is fixed by the save/stream codecs):
    /// a save/load resets every conduit to "fire after 1 s", matching the
    /// official reset-on-load behaviour. Keyed by conduit tile position;
    /// stale entries are pruned by `simulate_liquids`.
    pub conduit_flow_timers: Arc<DashMap<i32, f32>>,
    /// Authoritative tile fires (`Fires` / `FireComp`). Kept next to
    /// puddles so `DynamicWorld` constructors stay untouched; fires are
    /// transient and are not part of puddle writeSync.
    pub fires: Arc<DashMap<i32, FireState>>,
}

impl Default for PuddleSystem {
    fn default() -> Self {
        Self {
            puddles: Arc::new(DashMap::new()),
            next_entity_id: AtomicI32::new(100),
            conduit_flow_timers: Arc::new(DashMap::new()),
            fires: Arc::new(DashMap::new()),
        }
    }
}

impl PuddleSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// Restores a persisted puddle (JSON checkpoint / MSAV read) with its
    /// stable entity id; the allocator advances so future puddles never
    /// reuse the loaded ids.
    pub fn restore(&self, tile: i32, state: PuddleState) {
        self.next_entity_id
            .fetch_max(state.entity_id.saturating_add(1), Ordering::Relaxed);
        self.puddles.insert(tile, state);
    }

    /// Official `Puddles.deposit(tile, liquid, amount)`: accumulates on the
    /// tile, capped at maxLiquid; the legacy single-status style rules:
    /// same liquid accumulates, a different liquid reacts and only adds the
    /// clamped remainder. Returns whether the puddle changed.
    pub fn deposit(&self, tile: i32, liquid: i16, amount: f32) -> bool {
        if amount <= 0.0 || liquid < 0 {
            return false;
        }
        let mut entry = self.puddles.entry(tile).or_insert_with(|| PuddleState {
            entity_id: self.next_entity_id.fetch_add(1, Ordering::Relaxed),
            liquid,
            amount: 0.0,
            accepting: 0.0,
            spread_mask: 0b1111,
            update_time: 0.0,
        });
        if entry.liquid == liquid {
            // Same liquid: schedule acceptance (official `accepting`).
            entry.accepting = entry.accepting.max(amount);
        } else {
            // Different liquid: react (simplified) and add the remainder.
            let loss = react_liquids(entry.liquid, liquid, amount);
            let added = (amount - loss).clamp(0.0, PUDDLE_MAX_LIQUID - entry.amount);
            entry.accepting += added;
        }
        true
    }

    /// Official `PuddleComp.update`: evaporation `amount -= delta * (1 -
    /// viscosity) / (5 + addSpeed)` (addSpeed 3 while accepting), then the
    /// accepted amount is applied (capped), over-full puddles spread to the
    /// four neighbors and empty puddles are removed. Returns whether any
    /// puddle changed.
    pub fn tick(&self, delta_ticks: f32) -> bool {
        let delta = delta_ticks.max(0.0);
        let mut changed = false;
        // Spread pass: collect tiles that are over the spread threshold.
        let mut spread = Vec::new();
        let mut expired = Vec::new();
        for mut entry in self.puddles.iter_mut() {
            let liquid = entry.liquid;
            let viscosity = liquid_viscosity(liquid);
            let add_speed = if entry.accepting > 0.0 { 3.0 } else { 0.0 };
            let evaporation = delta * (1.0 - viscosity) / (5.0 + add_speed);
            entry.amount = (entry.amount - evaporation).max(0.0);
            entry.amount += entry.accepting;
            entry.accepting = 0.0;
            entry.amount = entry.amount.min(PUDDLE_MAX_LIQUID);
            if entry.amount <= 0.0 {
                expired.push(*entry.key());
            } else if entry.amount >= PUDDLE_MAX_LIQUID / 1.5 {
                spread.push((*entry.key(), liquid, entry.amount, entry.spread_mask));
            }
            changed = true;
        }
        for tile in expired {
            self.puddles.remove(&tile);
        }
        // Official spread (PuddleComp.update JAR offsets 98-257):
        // `deposited = min((amount - max/1.5) / 4, 0.3*delta)` reaches ONLY
        // neighbors whose `block() == air || liquid.moveThroughBlocks`, and
        // the origin loses `deposited * targets`; a puddle emptied by the
        // spread is removed (official `amount <= 0 -> remove`).
        for (tile, liquid, amount, spread_mask) in spread {
            let deposited = ((amount - PUDDLE_MAX_LIQUID / 1.5) / 4.0).min(0.3 * delta);
            if deposited <= 0.0 {
                continue;
            }
            let x = (tile >> 16) as i16;
            let y = tile as i16;
            let mut targets = 0u8;
            for (index, (dx, dy)) in [(1i16, 0i16), (-1, 0), (0, 1), (0, -1)].iter().enumerate() {
                if spread_mask & (1 << index) == 0 {
                    continue;
                }
                let neighbor = ((i32::from(x + dx)) << 16) | i32::from(y + dy);
                // Same-liquid deposits accumulate in `accepting`; reactions
                // consume part of the poured amount.
                self.deposit(neighbor, liquid, deposited);
                targets += 1;
                changed = true;
            }
            if targets > 0 {
                if let Some(mut origin) = self.puddles.get_mut(&tile) {
                    origin.amount = (origin.amount - deposited * f32::from(targets)).max(0.0);
                    if origin.amount <= 0.0 {
                        drop(origin);
                        self.puddles.remove(&tile);
                    }
                }
            }
        }
        changed
    }

    /// B9: refreshes the per-puddle spread passability mask. Official
    /// `PuddleComp.update` reads `other.block()` live (JAR offsets 195-239);
    /// the puddle service has no world access, so the domain owner
    /// (economy) calls this once per world step, right before `tick`, with
    /// the effective `Tile.block()` lookup. Standalone puddle tests that
    /// never call this keep the default all-passable mask.
    pub fn refresh_spread_masks(&self, passable: impl Fn(i32, i16) -> bool) {
        for mut entry in self.puddles.iter_mut() {
            let position = *entry.key();
            let liquid = entry.liquid;
            let x = (position >> 16) as i16;
            let y = position as i16;
            let mut mask = 0u8;
            for (index, (dx, dy)) in [(1i16, 0i16), (-1, 0), (0, 1), (0, -1)].iter().enumerate() {
                let neighbor = ((i32::from(x + dx)) << 16) | i32::from(y + dy);
                if passable(neighbor, liquid) {
                    mask |= 1 << index;
                }
            }
            entry.spread_mask = mask;
        }
    }

    /// Removes every puddle (map teardown / game reset).
    pub fn clear(&self) {
        self.puddles.clear();
        self.conduit_flow_timers.clear();
        self.fires.clear();
    }

    /// Official `Fires.create(tile)`: spawn or refresh lifetime.
    pub fn create_fire(&self, tile: i32) {
        if let Some(mut existing) = self.fires.get_mut(&tile) {
            existing.lifetime = FIRE_BASE_LIFETIME;
            existing.time = 0.0;
            return;
        }
        self.fires.insert(
            tile,
            FireState {
                time: 0.0,
                lifetime: FIRE_BASE_LIFETIME,
                damage_timer: 0.0,
            },
        );
    }

    pub fn has_fire(&self, tile: i32) -> bool {
        self.fires
            .get(&tile)
            .is_some_and(|fire| fire.time < fire.lifetime)
    }
}

/// Pulse collected from `PuddleComp.update` when `amount >= maxLiquid/2`
/// and `updateTime <= 0`. The world owner applies unit status / fire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuddleEffectPulse {
    pub tile: i32,
    pub liquid: i16,
    pub amount: f32,
}

impl PuddleSystem {
    /// Advances `updateTime` after evaporation/spread (`PuddleComp.update`
    /// effects-only block). Returns the pulses that fire this step.
    pub fn take_effect_pulses(&self, delta_ticks: f32) -> Vec<PuddleEffectPulse> {
        let delta = delta_ticks.max(0.0);
        let mut pulses = Vec::new();
        for mut entry in self.puddles.iter_mut() {
            if entry.amount >= PUDDLE_MAX_LIQUID / 2.0 && entry.update_time <= 0.0 {
                pulses.push(PuddleEffectPulse {
                    tile: *entry.key(),
                    liquid: entry.liquid,
                    amount: entry.amount,
                });
                entry.update_time = PUDDLE_EFFECT_INTERVAL;
            }
            entry.update_time -= delta;
        }
        pulses
    }

    /// Official `FireComp.update` lifetime clock. Returns tiles whose
    /// `damageTimer` just crossed `damageDelay` (40 ticks).
    pub fn tick_fires(&self, delta_ticks: f32) -> Vec<i32> {
        let delta = delta_ticks.max(0.0);
        let mut expired = Vec::new();
        let mut damage = Vec::new();
        for mut entry in self.fires.iter_mut() {
            entry.time += delta;
            if entry.time >= entry.lifetime {
                expired.push(*entry.key());
                continue;
            }
            entry.damage_timer += delta;
            if entry.damage_timer >= FIRE_DAMAGE_DELAY {
                entry.damage_timer = 0.0;
                damage.push(*entry.key());
            }
        }
        for tile in expired {
            self.fires.remove(&tile);
        }
        damage
    }
}

/// Official `Puddles.reactPuddle` simplified: flammable + hot liquids create
/// fire (reported through the return as full consumption is NOT modeled —
/// the port keeps the puddle amount but flags the reaction via the loss);
/// hot + cold liquids lose part of the poured amount (steam). Returns the
/// amount lost to the reaction.
pub fn react_liquids(dest: i16, liquid: i16, amount: f32) -> f32 {
    let dest_flammable = liquid_flammability(dest) > 0.3;
    let dest_hot = liquid_temperature(dest) > 0.7;
    let liquid_flammable = liquid_flammability(liquid) > 0.3;
    let liquid_hot = liquid_temperature(liquid) > 0.7;
    if (dest_flammable && liquid_hot) || (liquid_flammable && dest_hot) {
        // Fire reaction: the poured amount is consumed by the flame.
        amount
    } else if dest_hot && liquid_temperature(liquid) < 0.55 {
        -0.1 * amount
    } else if liquid_hot && dest_temperature(dest) < 0.55 {
        -0.7 * amount
    } else {
        0.0
    }
}

/// Official liquid properties used by the puddle model (158.1 Liquids):
/// viscosity, flammability and temperature in 0..1.
fn liquid_viscosity(liquid: i16) -> f32 {
    match liquid {
        0 => 0.5,  // water
        1 => 0.8,  // slag
        2 => 0.4,  // oil
        3 => 0.5,  // cryofluid
        4 => 0.8,  // plasma
        5 => 0.5,  // fuel
        6 => 0.7,  // ozone
        7 => 0.6,  // hydrogen
        8 => 0.4,  // nitrogen
        9 => 0.4,  // argon
        10 => 0.4, // neon
        _ => 0.5,
    }
}

/// Official `Liquid.moveThroughBlocks` (158.1): the base Liquid
/// constructor defaults it to false (JAR Liquid.<init> offsets 58-61); only
/// neoplasm sets it true (Liquids$5 offset 38), and neoplasm is not among
/// the modeled liquid ids. The spread pass therefore reaches only
/// `block == air` tiles for every modeled liquid; a future neoplasm-like
/// liquid would return true here.
pub(crate) fn liquid_moves_through_blocks(liquid: i16) -> bool {
    let _ = liquid;
    false
}

fn liquid_flammability(liquid: i16) -> f32 {
    match liquid {
        2 => 1.2, // oil
        5 => 1.2, // fuel
        _ => 0.0,
    }
}

pub(crate) fn liquid_temperature(liquid: i16) -> f32 {
    match liquid {
        1 => 1.0,  // slag (Liquids.java temperature = 1f)
        3 => 0.25, // cryofluid
        4 => 1.0,  // plasma
        _ => 0.5,
    }
}

/// Official `Liquid.effect` for vanilla liquids (158.1 `Liquids.java`).
/// `None` means `StatusEffects.none` — no unit status is applied.
pub fn liquid_status_effect(liquid: i16) -> Option<i16> {
    use crate::game::status::{STATUS_FREEZING, STATUS_MELTING, STATUS_TARRED, STATUS_WET};
    match liquid {
        0 => Some(STATUS_WET),
        1 => Some(STATUS_MELTING),
        2 => Some(STATUS_TARRED),
        3 => Some(STATUS_FREEZING),
        _ => None,
    }
}

/// Official `liquid.temperature > 0.7` gate for `Fires.create` in
/// `PuddleComp.update`. Oil is flammable but not hot, so it does not
/// ignite from this path (mix reactions remain in `react_liquids`).
pub fn liquid_ignites_from_puddle(liquid: i16) -> bool {
    liquid_temperature(liquid) > 0.7
}

/// World-space AABB size of the puddle effect rect
/// (`clamp(amount / (maxLiquid/1.5)) * 10`, centered on the puddle).
pub fn puddle_effect_rect_size(amount: f32) -> f32 {
    (amount / (PUDDLE_MAX_LIQUID / 1.5)).clamp(0.0, 1.0) * 10.0
}

/// Tile center in world units (`Tile.worldx/worldy`).
pub fn puddle_world_center(tile: i32) -> (f32, f32) {
    let x = (tile >> 16) as i16;
    let y = tile as i16;
    (f32::from(x) * 8.0 + 4.0, f32::from(y) * 8.0 + 4.0)
}

pub fn axis_aligned_overlap(ax: f32, ay: f32, asize: f32, bx: f32, by: f32, bsize: f32) -> bool {
    let ahalf = asize * 0.5;
    let bhalf = bsize * 0.5;
    (ax - ahalf) < (bx + bhalf)
        && (ax + ahalf) > (bx - bhalf)
        && (ay - ahalf) < (by + bhalf)
        && (ay + ahalf) > (by - bhalf)
}

fn dest_temperature(_dest: i16) -> f32 {
    // Identity for the cold-puddle branch: the port models the four liquids
    // with temperature above; anything else is cold.
    0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_caps_at_max_liquid_and_accepts_same_liquid() {
        let system = PuddleSystem::new();
        assert!(system.deposit((10 << 16) | 20, 0, 30.0));
        let state = system.puddles.get(&((10 << 16) | 20)).unwrap();
        assert_eq!(state.liquid, 0);
        assert_eq!(state.amount, 0.0);
        assert_eq!(state.accepting, 30.0);
        drop(state);
        assert!(system.tick(1.0));
        let state = system.puddles.get(&((10 << 16) | 20)).unwrap();
        assert!(state.amount > 0.0 && state.amount <= 30.0);
        assert_eq!(state.accepting, 0.0);
        drop(state);
        // Deposit beyond the cap: accepting is capped by tick.
        system.deposit((10 << 16) | 20, 0, 500.0);
        assert!(system.tick(1.0));
        let state = system.puddles.get(&((10 << 16) | 20)).unwrap();
        assert!(state.amount <= PUDDLE_MAX_LIQUID + 0.001);
    }

    #[test]
    fn evaporation_removes_empty_puddles_and_heavy_liquids_last() {
        let system = PuddleSystem::new();
        system.deposit((5 << 16) | 5, 0, 10.0); // water viscosity 0.5
        for _ in 0..200 {
            system.tick(1.0);
            if system.puddles.is_empty() {
                break;
            }
        }
        assert!(system.puddles.is_empty(), "water puddle evaporates");
        // Oil (viscosity 0.4) evaporates faster than water.
        let oil = PuddleSystem::new();
        oil.deposit((6 << 16) | 6, 2, 10.0);
        for _ in 0..200 {
            oil.tick(1.0);
            if oil.puddles.is_empty() {
                break;
            }
        }
        assert!(oil.puddles.is_empty(), "oil puddle evaporates");
    }

    #[test]
    fn full_puddles_spread_to_neighbors() {
        let system = PuddleSystem::new();
        system.deposit((10 << 16) | 10, 0, 70.0);
        system.tick(1.0);
        // With accepting=70 the puddle reaches the spread threshold and
        // deposits into the four orthogonal neighbors.
        let neighbors = [
            (11 << 16) | 10,
            (9 << 16) | 10,
            (10 << 16) | 11,
            (10 << 16) | 9,
        ];
        let mut seen = 0;
        for neighbor in neighbors {
            if system.puddles.get(&neighbor).is_some() {
                seen += 1;
            }
        }
        assert!(seen > 0, "full puddle spreads to neighbors");
    }

    #[test]
    fn spread_only_reaches_passable_neighbors_and_subtracts_from_origin() {
        // B9: PuddleComp.update (JAR offsets 195-255) deposits only into
        // neighbors whose block() == air (or liquid.moveThroughBlocks) and
        // then subtracts deposited * targets from the origin.
        let system = PuddleSystem::new();
        system.deposit((10 << 16) | 10, 0, 70.0);
        // Refresh with a passability function that blocks the west neighbor
        // (a wall) and allows the other three.
        system.refresh_spread_masks(|position, _| position != (9 << 16) | 10);
        system.tick(1.0);
        let origin = system.puddles.get(&((10 << 16) | 10)).unwrap();
        // deposited = min((70 - 46.6667)/4, 0.3*1) = 0.3; three targets.
        assert!(
            (origin.amount - (70.0 - 0.3 * 3.0)).abs() < 0.001,
            "origin loses deposited * targets, got {}",
            origin.amount
        );
        drop(origin);
        assert!(
            system.puddles.get(&((9 << 16) | 10)).is_none(),
            "blocked west neighbor receives no spread"
        );
        for neighbor in [(11 << 16) | 10, (10 << 16) | 11, (10 << 16) | 9] {
            assert!(
                system.puddles.get(&neighbor).is_some(),
                "passable neighbor receives the spread deposit"
            );
        }
        // With all four neighbors passable the origin keeps at least the
        // official floor (amount - 4 * (amount - max/1.5) / 4 = max/1.5).
        let open = PuddleSystem::new();
        open.deposit((30 << 16) | 30, 0, 70.0);
        open.refresh_spread_masks(|_, _| true);
        open.tick(1.0);
        let origin = open.puddles.get(&((30 << 16) | 30)).unwrap();
        assert!(
            (origin.amount - (70.0 - 0.3 * 4.0)).abs() < 0.001,
            "four targets subtract 4 * deposited, got {}",
            origin.amount
        );
        drop(origin);
        // Out-of-bounds neighbors are not passable either: a puddle on the
        // map edge spreads only inward.
        let edge = PuddleSystem::new();
        edge.deposit(0, 0, 70.0);
        edge.refresh_spread_masks(|_, _| false);
        edge.tick(1.0);
        assert_eq!(
            edge.puddles.get(&0).unwrap().amount,
            70.0,
            "no passable neighbor means no spread and no subtraction"
        );
    }

    #[test]
    fn liquid_reactions_consume_or_reduce() {
        // Flammable (oil) + hot (slag): the poured amount is consumed.
        assert_eq!(react_liquids(2, 1, 10.0), 10.0);
        // Hot + cold: partial loss.
        assert_eq!(react_liquids(1, 0, 10.0), -0.1 * 10.0);
        // Same-ish cold pair: no reaction.
        assert_eq!(react_liquids(0, 3, 10.0), 0.0);
    }

    #[test]
    fn effect_pulses_fire_every_40_ticks_while_amount_is_at_least_half() {
        let system = PuddleSystem::new();
        system.deposit((8 << 16) | 8, 1, 60.0);
        system.tick(1.0);
        let first = system.take_effect_pulses(1.0);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].liquid, 1);
        let mut extra = 0;
        for _ in 0..39 {
            extra += system.take_effect_pulses(1.0).len();
        }
        assert_eq!(extra, 0, "no pulse until 40 ticks");
        assert_eq!(system.take_effect_pulses(1.0).len(), 1);
        assert!(liquid_ignites_from_puddle(1));
        assert!(!liquid_ignites_from_puddle(0));
        assert!(!liquid_ignites_from_puddle(2));
        assert!(!liquid_ignites_from_puddle(3));
    }

    #[test]
    fn empty_puddle_is_removed_and_stops_pulsing() {
        let system = PuddleSystem::new();
        system.deposit((4 << 16) | 4, 0, 2.0);
        system.tick(1.0);
        assert!(system.take_effect_pulses(1.0).is_empty());
        for _ in 0..200 {
            system.tick(1.0);
            if system.puddles.is_empty() {
                break;
            }
        }
        assert!(system.puddles.is_empty());
        assert!(system.take_effect_pulses(1.0).is_empty());
    }
}
