//! World access for logic side effects.

use std::collections::HashSet;

use super::executor::{
    ExecutorState, LObject, LVar, LogicRule, RadarSort, RadarTarget, SetPropKey, UlocGroup,
    UlocKind, UlocSpec, MAX_DISPLAY_BUFFER,
};
use super::ops::LAccess;

/// Official SpawnUnitI position jitter amplitude (`Mathf.range(0.01f)`).
pub(crate) const SPAWN_POSITION_JITTER: f32 = 0.01;

/// Arc `Mathf.range(range)` → `random(-range, range)` →
/// `-range + nextFloat() * (2 * range)` with `nextFloat ∈ [0, 1)`.
/// Result interval is `[-range, range)` (lower inclusive, upper exclusive).
pub(crate) fn mathf_range_from_unit(unit: f32, range: f32) -> f32 {
    debug_assert!((0.0..1.0).contains(&unit));
    -range + unit * (range + range)
}

/// Runtime Arc `Mathf.range` using the process RNG (not a Java-identical sequence).
pub(crate) fn mathf_range(range: f32) -> f32 {
    mathf_range_from_unit(rand::random::<f32>(), range)
}

/// Official SpawnUnitI world coords: `World.unconv(logic) + jitter`.
/// `World.unconv` is tiles × 8; jitter matches `Mathf.range(0.01f)` per axis.
pub(crate) fn spawn_world_position(
    logic_x: f64,
    logic_y: f64,
    jitter_x: f32,
    jitter_y: f32,
) -> (f32, f32) {
    (
        (logic_x as f32) * 8.0 + jitter_x,
        (logic_y as f32) * 8.0 + jitter_y,
    )
}

/// Sensor result (number or building object).
#[derive(Debug, Clone, PartialEq)]
pub enum SensorValue {
    Num(f64),
    Obj(LObject),
}

/// Read-only world view the executor queries. Implemented by the server
/// against the live world (network::world::DynamicWorld).
pub struct WorldView<'a> {
    pub world: &'a crate::network::world::DynamicWorld,
    pub processor_pos: i32,
    /// Connections for authoritative broadcasts (spawn, effects).
    pub out: &'a dyn crate::network::outbound::FrameEmit,
}

impl<'a> WorldView<'a> {
    /// Team of the processor tile (fallback 1). Logic operations that mutate
    /// buildings must only affect the processor's own team (SOL-007): a PvP
    /// processor must not take/drop items into enemy buildings.
    pub fn processor_team(&self) -> u8 {
        self.world
            .tiles
            .get(&self.processor_pos)
            .map(|tile| tile.team)
            .unwrap_or(1)
    }

    /// Whether the processor may mutate the building at `pos`: same team or
    /// derelict (0).
    pub fn building_owned(&self, pos: i32) -> bool {
        let Some(tile) = self.world.tiles.get(&pos) else {
            return false;
        };
        tile.team == self.processor_team() || tile.team == 0
    }

    /// Official `MemoryBuild.readable(executor)` (desktop 158.1): valid
    /// building AND (executor privileged OR (same team AND block not
    /// privileged)). `worldCell` (443) is a privileged block; the port's
    /// privileged executors are world processors (442).
    fn memory_readable(&self, pos: i32, privileged: bool) -> bool {
        let Some(tile) = self.world.tiles.get(&pos) else {
            return false;
        };
        if privileged {
            return true;
        }
        tile.team == self.processor_team() && tile.block != 443
    }

    pub fn read_memory(&self, cell: &LVar, addr: i64, privileged: bool) -> Option<f64> {
        let LObject::Building(pos) = cell.objval else {
            return None;
        };
        if !self.memory_readable(pos, privileged) {
            return None;
        }
        let tile = self.world.tiles.get(&pos)?;
        let capacity = crate::network::economy::memory_capacity(tile.block)?;
        if addr >= 0 && (addr as usize) < capacity {
            Some(tile.memory.get(addr as usize).copied().unwrap_or(0.0))
        } else {
            None
        }
    }

    pub fn write_memory(&self, cell: &LVar, addr: i64, value: f64, privileged: bool) {
        let LObject::Building(pos) = cell.objval else {
            return;
        };
        if addr < 0 || !self.memory_readable(pos, privileged) {
            return;
        }
        if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
            let capacity = crate::network::economy::memory_capacity(tile.block);
            let Some(capacity) = capacity else {
                return;
            };
            if (addr as usize) >= capacity {
                return;
            }
            // Lazy-initialize the cell array to its official capacity so
            // writes are not discarded on a fresh tile (regression: memory
            // was born Vec::new() and every write/read was a no-op).
            if tile.memory.len() != capacity {
                tile.memory = vec![0.0; capacity];
            }
            tile.memory[addr as usize] = value;
        }
    }

    /// Official `PrintFlushI.run` gate (desktop 158.1 offsets 20-54): the
    /// target must be a MessageBuild AND (executor privileged OR (same team
    /// AND block not privileged)); the textBuffer is capped at
    /// `MessageBlock.maxTextLength` (400, JAR MessageBlock offsets 5-11).
    pub fn write_message(&self, target: &LVar, text: &str, privileged: bool) {
        let LObject::Building(pos) = target.objval else {
            return;
        };
        // DashMap shard locks are not reentrant: a live get_mut write guard on
        // `self.world.tiles` must not overlap `processor_team()`, which reads
        // the same map (get). Resolve the team before taking the guard.
        let processor_team = self.processor_team();
        if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
            let message_block = matches!(tile.block, 429 | 441 | 444);
            if message_block {
                let owned = privileged || (tile.team == processor_team && tile.block != 444);
                if owned {
                    // PrintFlushI appends min(textBuffer.length,
                    // maxTextLength) — a UTF-16 code-unit cap.
                    let mut capped: String = String::new();
                    for character in text.chars() {
                        if capped.encode_utf16().count() + character.len_utf16() > 400 {
                            break;
                        }
                        capped.push(character);
                    }
                    tile.message = Some(capped);
                }
            }
        }
    }

    /// Flushes an executor's packed DisplayCmd queue to a same-team logic
    /// display. Java's DrawFlushI accepts only LogicDisplayBuild targets and
    /// caps each display's pending queue at `LExecutor.maxDisplayBuffer`;
    /// invalid/enemy targets still cause the executor's local queue to clear
    /// (the caller performs that part).
    pub fn draw_flush(&self, target: &LVar, commands: &[u64], privileged: bool) {
        let LObject::Building(pos) = target.objval else {
            return;
        };
        let Some(tile) = self.world.tiles.get(&pos) else {
            return;
        };
        let owned =
            matches!(tile.block, 436..=438) && (privileged || tile.team == self.processor_team());
        drop(tile);
        if !owned || commands.is_empty() {
            return;
        }
        let mut queue = self.world.logic_display_commands.entry(pos).or_default();
        let room = MAX_DISPLAY_BUFFER.saturating_sub(queue.len());
        queue.extend_from_slice(&commands[..commands.len().min(room)]);
    }

    /// Official `ControlI.run` gate (desktop 158.1 offsets 20-42): the
    /// executor must be privileged OR the target must be a valid link of the
    /// processor (`LogicBuild.validLink`: same team, inside
    /// `range + target.size*8/2`, target block not privileged). The old
    /// hardcoded `team == 1` gate allowed cross-team spoofing in PvP.
    pub fn set_enabled(&self, target: &LVar, value: bool, privileged: bool) {
        let LObject::Building(pos) = target.objval else {
            return;
        };
        let Some(tile) = self.world.tiles.get(&pos) else {
            return;
        };
        let valid_link = privileged
            || (tile.team == self.processor_team()
                && tile.block != 443
                && tile.block != 444
                && tile.block != 445
                && self.processor_range().is_some_and(|range| {
                    // validLink: `target.within(this, range + size*8/2)` in
                    // world units (tile distance * 8).
                    let (px, py) = self.processor_xy();
                    let (tx, ty) = ((pos >> 16) as i16 as f64 * 8.0, pos as i16 as f64 * 8.0);
                    let size = crate::game::content::block_placement(tile.block).size as f64;
                    (px - tx).hypot(py - ty) <= f64::from(range) + size * 4.0
                }));
        drop(tile);
        if !valid_link {
            return;
        }
        if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
            tile.enabled = value;
        }
    }

    /// Processor center in world units.
    fn processor_xy(&self) -> (f64, f64) {
        (
            (self.processor_pos >> 16) as i16 as f64 * 8.0,
            self.processor_pos as i16 as f64 * 8.0,
        )
    }

    /// Official `LogicBlock.range`: micro 80, logic 176, hyper 336, world
    /// processor Float.MAX_VALUE (Blocks.java 158.1).
    fn processor_range(&self) -> Option<f32> {
        let block = self
            .world
            .tiles
            .get(&self.processor_pos)
            .map(|tile| tile.block)?;
        Some(match block {
            431 => 80.0,
            432 => 176.0,
            433 => 336.0,
            442 => f32::MAX,
            _ => 80.0,
        })
    }

    /// Official `UnitBindI.run` for a UnitType operand (desktop 158.1
    /// offsets 177-206): bind the NEXT unit of the EXECUTOR's team in the
    /// team's unit cache for this type (round-robin, one candidate per
    /// executed `ubind`), or null when the cache is empty.
    ///
    /// - Team: the processor tile's team (`exec.team`, which LogicBuild
    ///   assigns from its own team every tick) — never a fixed team.
    /// - Candidates: Java rebuilds `TeamData.unitCache` in `updateTeamStats`
    ///   by iterating `Groups.unit`. That group is an unordered Seq
    ///   (`EntityGroup` constructs `Seq(false, 32, class)`): add appends,
    ///   remove is swap-remove. The port mirrors that with
    ///   `DynamicWorld.unit_group_order`. Units never registered (older
    ///   tests that only `enemies.insert`) fall back to ascending id, which
    ///   matches first-insert spawn order. DashMap iteration is never the
    ///   observable order.
    /// - Cursor: `state.bind_cursors[type]` mirrors `exec.binds[type.id]`
    ///   (`%= seq.size` then `++`); it survives candidate list changes and
    ///   only resets when the processor is recompiled (new ExecutorState,
    ///   like Java's fresh LExecutor).
    /// - `ubind` alone never acquires Logic authority (that is ucontrol's
    ///   `checkLogicAI`); this method changes no unit.
    /// - Non-logic-controllable types (missiles, manifold, assembly-drone —
    ///   `type.logicControllable == false`) bind null.
    pub fn bind_unit(&self, state: &mut ExecutorState, unit_type: i16) {
        if !crate::game::unit_types::unit_type_logic_controllable(unit_type) {
            // Official: the type gate fails before the cache is consulted.
            state.bound_unit = None;
            return;
        }
        let team = self.processor_team();
        let mut in_order = HashSet::new();
        let mut candidates: Vec<i32> = {
            let order = self.world.unit_group_order.lock();
            let mut listed = Vec::new();
            for &id in order.iter() {
                in_order.insert(id);
                let Some(unit) = self.world.enemies.get(&id) else {
                    continue;
                };
                if unit.team == team && unit.unit_type == unit_type && unit.health > 0.0 {
                    listed.push(id);
                }
            }
            listed
        };
        let mut leftovers: Vec<i32> = self
            .world
            .enemies
            .iter()
            .filter(|unit| {
                unit.team == team
                    && unit.unit_type == unit_type
                    && unit.health > 0.0
                    && !in_order.contains(&unit.id)
            })
            .map(|unit| unit.id)
            .collect();
        leftovers.sort_unstable();
        candidates.extend(leftovers);
        if candidates.is_empty() {
            // No units of this type found: @unit = null.
            state.bound_unit = None;
            return;
        }
        let index = state.bind_cursor(unit_type) % candidates.len();
        state.advance_bind_cursor(unit_type);
        state.bound_unit = Some(candidates[index]);
    }

    /// Official `UnitBindI.run` Unit-object branch (desktop 158.1
    /// LExecutor.java:200-202, confirmed in the jar bytecode): bind the GIVEN
    /// unit when `(u.team == exec.team || exec.privileged) &&
    /// u.type.logicControllable`; otherwise null. Controller-level
    /// eligibility (`unit.controller().isLogicControllable()`, Player /
    /// CommandAI with an active target) is a `ucontrol`/`checkLogicAI` gate,
    /// not a `ubind` gate — a held reference still binds. Java does not
    /// re-check liveness here (a dead unit object still binds); the port
    /// keeps that, except that a unit id no longer present in the world maps
    /// to null (the port materializes unit objects from the live table).
    /// Like `bind_unit`, this never acquires Logic authority.
    pub fn bind_unit_object(&self, state: &ExecutorState, unit_id: i32) -> Option<i32> {
        let unit = self.world.enemies.get(&unit_id)?;
        let team_ok = unit.team == self.processor_team() || state.privileged;
        let type_ok = crate::game::unit_types::unit_type_logic_controllable(unit.unit_type);
        if team_ok && type_ok {
            Some(unit_id)
        } else {
            None
        }
    }

    /// Team of the executor's bound unit, if any. Official UnitLocateI scans
    /// the BOUND unit's team (`indexer.getFlagged(unit.team, flag)`); a
    /// bound unit is always on the executor's team (or the privileged
    /// executor chose it), so this only differs from `processor_team` for
    /// privileged executors.
    pub fn bound_unit_team(&self, state: &ExecutorState) -> Option<u8> {
        state
            .bound_unit
            .and_then(|id| self.world.enemies.get(&id).map(|unit| unit.team))
    }

    /// P0-03 acquisition/refresh gate — official `UnitControlI.checkLogicAI`
    /// plus the `ai.controlTimer = LogicAI.logicControlTimeout` refresh
    /// (desktop 158.1 LExecutor.java:322-338, 238/351). Called by EVERY
    /// `ucontrol` op (except unbind) and by `ulocate` before any effect is
    /// applied: a unit only enters Logic control through a valid instruction,
    /// and every valid one refreshes the 600-tick lease. Returns false (and
    /// changes nothing) when the gate fails — the whole instruction must then
    /// be skipped, exactly like Java's `if(... ai != null)` guard.
    pub fn refresh_logic_control(&self, state: &ExecutorState) -> bool {
        crate::network::units::refresh_logic_control(
            self.world,
            state.bound_unit,
            self.processor_pos,
            self.processor_team(),
            state.privileged,
        )
    }

    /// Official `ucontrol unbind` (desktop 158.1 LExecutor.java:347-369):
    /// `checkLogicAI` runs first — installing (and first-takeover-clearing) a
    /// LogicAI exactly like any other ucontrol — and then the switch's
    /// `unbind` case executes `unit.resetController()`. When checkLogicAI
    /// fails (player-possessed or actively RTS-commanded unit, enemy team
    /// without privilege) the unbind is a complete no-op in Java, mirrored
    /// here. The `@unit` variable is NOT cleared: the executor keeps
    /// pointing at the same unit after the unbind.
    pub fn ucontrol_unbind(&self, state: &ExecutorState) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if !crate::network::units::refresh_logic_control(
            self.world,
            Some(unit_id),
            self.processor_pos,
            self.processor_team(),
            state.privileged,
        ) {
            return;
        }
        crate::network::units::release_logic_control(self.world, unit_id);
    }

    /// Issues a move order to the bound unit (command 0, fixed target).
    pub fn ucontrol_move(&self, state: &ExecutorState, x: f64, y: f64) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        let mut order = self
            .world
            .unit_orders
            .get(&unit_id)
            .map(|order| order.clone())
            .unwrap_or_else(|| crate::network::world::UnitOrder {
                unit_id,
                command: 0,
                stances: 0,
                payload_cooldown: 0.0,
                target_kind: 0,
                target_id: -1,
                target_x: None,
                target_y: None,
                logic_control: 0,
                queue: Vec::new(),
            });
        order.command = 0;
        order.target_kind = 0;
        order.logic_control = crate::network::world::logic_control::MOVE;
        // UnitControlI stores World.unconv(tile coords) in moveX/moveY.
        order.target_x = Some((x * 8.0) as f32);
        order.target_y = Some((y * 8.0) as f32);
        order.target_id = -1;
        self.world.unit_orders.insert(unit_id, order);
    }

    /// LogicAI pathfind: store world pixels and mark PATHFIND so ControlPathfinder
    /// PathfindResult is used instead of a straight-line move.
    pub fn ucontrol_pathfind(&self, state: &ExecutorState, x: f64, y: f64) {
        self.ucontrol_move(state, x, y);
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if let Some(mut order) = self.world.unit_orders.get_mut(&unit_id) {
            order.logic_control = crate::network::world::logic_control::PATHFIND;
        }
    }

    /// LogicAI stop: ceases issuing new movement acceleration; existing
    /// velocity coasts with drag (LogicAI.java `case stop`).
    pub fn ucontrol_stop(&self, state: &ExecutorState) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if let Some(mut order) = self.world.unit_orders.get_mut(&unit_id) {
            order.logic_control = crate::network::world::logic_control::STOP;
            order.target_x = None;
            order.target_y = None;
        }
    }

    /// ucontrol flag: sets the bound unit's logic flag.
    pub fn ucontrol_flag(&self, state: &ExecutorState, value: f64) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if let Some(mut unit) = self.world.enemies.get_mut(&unit_id) {
            unit.flag = value;
        }
    }

    /// ucontrol boost: toggles the boosting stance (bit 5).
    pub fn ucontrol_boost(&self, state: &ExecutorState, enabled: bool) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if let Some(mut order) = self.world.unit_orders.get_mut(&unit_id) {
            if enabled {
                order.stances |= 1 << 5;
            } else {
                order.stances &= !(1 << 5);
            }
        }
    }

    /// ucontrol mine: sets the mining target (order kind 6).
    pub fn ucontrol_mine(&self, state: &ExecutorState, x: f64, y: f64) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if let Some(mut order) = self.world.unit_orders.get_mut(&unit_id) {
            order.command = 0;
            order.target_kind = 6;
            order.target_x = Some(x as f32);
            order.target_y = Some(y as f32);
            order.target_id = -1;
            order.stances &= !1; // no hold stance while mining
        }
    }

    /// ucontrol shoot/target: aims the bound unit at a point; fires when
    /// `shoot` is set (order kind 7 = aim+fire).
    pub fn ucontrol_shoot(&self, state: &ExecutorState, x: f64, y: f64, shoot: bool) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if let Some(mut order) = self.world.unit_orders.get_mut(&unit_id) {
            order.command = 0;
            order.target_kind = if shoot { 7 } else { 8 };
            order.target_x = Some(x as f32);
            order.target_y = Some(y as f32);
            order.target_id = -1;
            order.stances &= !1; // no hold stance while aiming
        }
    }

    /// ucontrol build: sets the construction order (target_kind 9, block in
    /// target_id). The unit moves to the site and builds with progress.
    pub fn ucontrol_build(&self, state: &ExecutorState, x: f64, y: f64, block: i16, rotation: f64) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        if let Some(mut order) = self.world.unit_orders.get_mut(&unit_id) {
            order.command = 0;
            order.target_kind = 9;
            order.target_x = Some(x as f32);
            order.target_y = Some(y as f32);
            order.target_id = i32::from(block) | (i32::from(rotation as i8 & 3) << 16);
            order.stances &= !1;
        }
    }

    /// ucontrol itemDrop: transfer items from the unit to a building.
    pub fn ucontrol_itemdrop(&self, state: &ExecutorState, to: &LVar, amount: i32) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        let LObject::Building(build_pos) = to.objval else {
            return;
        };
        // SOL-007: only drop into the processor's own team (or derelict).
        if !self.building_owned(build_pos) {
            return;
        }
        let mut remaining = amount.max(0);
        if let Some(mut unit) = self.world.enemies.get_mut(&unit_id) {
            let mut keep = Vec::new();
            for (item, amt) in std::mem::take(&mut unit.items) {
                if remaining <= 0 {
                    keep.push((item, amt));
                    continue;
                }
                let drop = amt.min(remaining);
                let rest = amt - drop;
                remaining -= drop;
                if rest > 0 {
                    keep.push((item, rest));
                }
                if let Some(mut tile) = self.world.tiles.get_mut(&build_pos) {
                    if let Some(entry) = tile.inventory.iter_mut().find(|(i, _)| *i == item) {
                        entry.1 += drop;
                    } else {
                        tile.inventory.push((item, drop));
                    }
                }
            }
            unit.items = keep;
        }
    }

    /// ucontrol itemTake: take items from a building into the unit (cap 2).
    pub fn ucontrol_itemtake(&self, state: &ExecutorState, from: &LVar, item: i16, amount: i32) {
        let Some(unit_id) = state.bound_unit else {
            return;
        };
        let LObject::Building(build_pos) = from.objval else {
            return;
        };
        // SOL-007: only take from the processor's own team (or derelict).
        if !self.building_owned(build_pos) {
            return;
        }
        let mut taken = 0;
        if let Some(mut tile) = self.world.tiles.get_mut(&build_pos) {
            if let Some(entry) = tile.inventory.iter_mut().find(|(i, _)| *i == item) {
                taken = entry.1.min(amount.max(0));
                entry.1 -= taken;
                tile.inventory.retain(|(_, amt)| *amt > 0);
            }
        }
        if taken > 0 {
            if let Some(mut unit) = self.world.enemies.get_mut(&unit_id) {
                let carried: i32 = unit.items.iter().map(|(_, a)| a).sum();
                let room = (2 - carried).max(0);
                let put = taken.min(room);
                if put > 0 {
                    if let Some(entry) = unit.items.iter_mut().find(|(i, _)| *i == item) {
                        entry.1 += put;
                    } else {
                        unit.items.push((item, put));
                    }
                }
            }
        }
    }

    /// within x y radius — distance check against the bound unit position.
    pub fn ucontrol_within(&self, state: &ExecutorState, x: f64, y: f64, radius: f64) -> bool {
        let Some(unit_id) = state.bound_unit else {
            return false;
        };
        let Some(unit) = self.world.enemies.get(&unit_id) else {
            return false;
        };
        let distance = (x - f64::from(unit.x)).hypot(y - f64::from(unit.y));
        distance <= radius
    }

    /// getBlock x y — the building object at the tile and its floor id.
    pub fn ucontrol_getblock(&self, x: f64, y: f64) -> (LObject, f64) {
        let tile_x = (x / 8.0).floor() as i16;
        let tile_y = (y / 8.0).floor() as i16;
        let pos = ((tile_x as i32) << 16) | tile_y as i32;
        let building = match self.world.tiles.get(&pos) {
            Some(tile) if tile.block != 0 => LObject::Building(pos),
            _ => LObject::Null,
        };
        let floor = self.world.floors.get(pos as usize).copied().unwrap_or(0) as f64;
        (building, floor)
    }

    /// Official SetBlockI.run (desktop 158.1 offsets 0-242): server-only,
    /// privileged-only; `tile = world.tile(x.numi(), y.numi())` (TILE
    /// coordinates, not pixels). The layer switch covers floor/ore/block —
    /// `TileLayer.settable` = [floor, ore, block]. For the block layer,
    /// `setNet(block, team, clamp(rotation, 0, 3))` only when block or team
    /// actually change, so existing health/inventory/config/links survive.
    pub fn setblock(
        &self,
        layer: crate::logic::ops::TileLayer,
        block: i16,
        x: f64,
        y: f64,
        team: u8,
        rotation: f64,
    ) {
        let tile_x = x as i32;
        let tile_y = y as i32;
        if tile_x < 0 || tile_y < 0 || tile_x >= self.world.width || tile_y >= self.world.height {
            return;
        }
        let pos = (tile_x << 16) | tile_y;
        let rotation = (rotation as i32).clamp(0, 3) as u8;
        let index = tile_y as usize * self.world.width as usize + tile_x as usize;
        let _ = index;
        match layer {
            crate::logic::ops::TileLayer::Ore | crate::logic::ops::TileLayer::Floor => {
                // Official SetBlockI supports setOverlayNet/setFloorNet for
                // these layers (JAR offsets 96-170). The port's terrain
                // arrays (`DynamicWorld.floors/overlays`) are immutable
                // through the read-only WorldView and shared with the world
                // stream; mutating them here is out of scope, so these
                // layers diagnose instead of silently writing a tile.
                tracing::warn!(
                    "setblock layer {:?} at ({tile_x},{tile_y}) is not supported by this port",
                    layer
                );
            }
            crate::logic::ops::TileLayer::Building => {
                // Not settable in 158.1 (TileLayer.settable has 3 entries);
                // SetBlockI's tableswitch covers only layers 1-3.
                let _ = (block, team, rotation);
            }
            crate::logic::ops::TileLayer::Block => {
                if block <= 0 {
                    return;
                }
                // Snapshot then mutate: DashMap shard locks are not reentrant,
                // so a live get_mut guard must not overlap tiles.get / insert
                // (assign_new_building_generation) or another tiles.get_mut.
                let existing = self
                    .world
                    .tiles
                    .get(&pos)
                    .map(|tile| (tile.block, tile.team, tile.generation, tile.health));
                if let Some((old_block, old_team, generation, health)) = existing {
                    if old_block == block && old_team == team {
                        // Official: setNet only when block/team change.
                        return;
                    }
                    // Preserve life/inventory/config/links: only the block,
                    // team, rotation and health cap change. setNet still
                    // creates a new Building object, so instance identity
                    // must change even when the block id stays a processor.
                    crate::network::world::note_building_generation(generation);
                    let new_generation = crate::network::world::next_building_generation();
                    let maximum = crate::game::content::block_health(block);
                    let health_ratio = if crate::game::content::block_health(old_block) > 0.0 {
                        (health / crate::game::content::block_health(old_block)).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
                        tile.generation = new_generation;
                        tile.block = block;
                        tile.team = team;
                        tile.rotation = rotation;
                        tile.health = (maximum * health_ratio).max(1.0);
                    }
                } else {
                    let maximum = crate::game::content::block_health(block);
                    let generation =
                        crate::network::world::assign_new_building_generation(self.world, pos);
                    self.world.tiles.insert(
                        pos,
                        crate::network::world::DynamicTile {
                            position: pos,
                            block,
                            rotation,
                            team,
                            health: maximum,
                            occupied: vec![pos],
                            generation,
                            ..Default::default()
                        },
                    );
                }
            }
        }
    }

    /// Radar scan: first unit matching the three AND-ed targets, sorted by
    /// `sort` with `order < 0` descending, within a generous radius.
    pub fn radar_find(
        &self,
        from: &LVar,
        targets: &[RadarTarget; 3],
        sort: RadarSort,
        order: f64,
    ) -> Option<i32> {
        let (origin_x, origin_y) = match from.objval {
            LObject::Building(pos) => ((pos >> 16) as i16 as f64 * 8.0, pos as i16 as f64 * 8.0),
            LObject::Unit(id) => {
                let unit = self.world.enemies.get(&id)?;
                (f64::from(unit.x), f64::from(unit.y))
            }
            _ => (0.0, 0.0),
        };
        let mut candidates: Vec<(i32, f64)> = Vec::new();
        for unit in self.world.enemies.iter() {
            if unit.health <= 0.0 {
                continue;
            }
            let mut ok = true;
            for target in targets {
                match target {
                    RadarTarget::Any => {}
                    RadarTarget::Enemy => ok &= unit.team != 1,
                    RadarTarget::Ally => ok &= unit.team == 1,
                    RadarTarget::Player => {
                        ok &= unit.team == 1 && self.world.players.contains_key(&unit.id)
                    }
                    RadarTarget::Attacker => ok &= unit.team != 1,
                    RadarTarget::Flying => ok &= unit.elevation > 0.0,
                    RadarTarget::Ground => ok &= unit.elevation <= 0.0,
                    RadarTarget::Boss => ok &= unit.entity_class == 2,
                }
            }
            if !ok {
                continue;
            }
            let key = match sort {
                RadarSort::Distance => (unit.x as f64 - origin_x).hypot(unit.y as f64 - origin_y),
                RadarSort::Health | RadarSort::MaxHealth => f64::from(unit.health),
                RadarSort::Shield => f64::from(unit.shield),
                RadarSort::Armor => 0.0,
                RadarSort::X => f64::from(unit.x),
                RadarSort::Y => f64::from(unit.y),
            };
            candidates.push((unit.id, key));
        }
        if candidates.is_empty() {
            return None;
        }
        candidates.sort_by(|a, b| {
            if order < 0.0 {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
            }
        });
        Some(candidates[0].0)
    }

    /// ulocate: find a building (by group, nearest), an ore tile, an enemy
    /// spawn, the most damaged building, or the core. Returns (building, x, y).
    ///
    /// `team` is the scanning team: official UnitLocateI uses the BOUND
    /// UNIT's team (`indexer.getFlagged(unit.team, flag)` /
    /// `Units.findDamagedTile(unit.team, ...)`), which ubind guarantees to be
    /// the executor's team. The old hardcoded `team == 1` scan let an enemy
    /// processor locate team-1 buildings.
    pub fn ulocate_find(
        &self,
        spec: &UlocSpec,
        enemy: bool,
        team: u8,
    ) -> Option<(LObject, f64, f64)> {
        let (origin_x, origin_y) = {
            let pos = self.processor_pos;
            ((pos >> 16) as i16 as f64 * 8.0, pos as i16 as f64 * 8.0)
        };
        let group_matches = |block: i16| -> bool {
            let in_range = |range: std::ops::RangeInclusive<i16>| range.contains(&block);
            match spec.group {
                UlocGroup::Core => in_range(339..=344),
                UlocGroup::Storage => in_range(345..=348),
                UlocGroup::Generator => in_range(308..=316) || in_range(320..=324),
                UlocGroup::Turret => in_range(349..=376),
                UlocGroup::Factory => in_range(377..=397),
                UlocGroup::Repair => matches!(block, 245 | 246 | 253 | 384 | 385 | 397),
                UlocGroup::Battery => matches!(block, 306 | 307),
                UlocGroup::Reactor => matches!(block, 315 | 316 | 323 | 324),
                UlocGroup::Drill => in_range(325..=333),
                UlocGroup::Shield => matches!(block, 244 | 249 | 253 | 254 | 255 | 256),
                UlocGroup::Unit => in_range(377..=409),
                UlocGroup::Resupply => block == 270,
                UlocGroup::All => block != 0,
            }
        };
        let mut best: Option<(i32, f32)> = None; // (pos, distance)
        match spec.kind {
            UlocKind::Building | UlocKind::Damaged => {
                for tile in self.world.tiles.iter() {
                    if tile.block == 0 || tile.team != team {
                        continue;
                    }
                    if spec.kind == UlocKind::Damaged {
                        // most damaged building of the scanning team
                        // (lowest health ratio)
                        let health =
                            crate::network::buildings::snapshot::dynamic_tile_health(&tile);
                        // most damaged = lowest absolute health (max health is
                        // not exposed per block in this port).
                        let key = health;
                        if best.as_ref().map(|(_, b)| key < *b).unwrap_or(true) {
                            best = Some((tile.position, key));
                        }
                    } else if group_matches(tile.block) {
                        let distance = (((tile.position >> 16) as i16 as f64 * 8.0 - origin_x)
                            .hypot(tile.position as i16 as f64 * 8.0 - origin_y))
                            as f32;
                        if best.as_ref().map(|(_, b)| distance < *b).unwrap_or(true) {
                            best = Some((tile.position, distance));
                        }
                    }
                }
                if let Some((pos, _)) = best {
                    return Some((
                        LObject::Building(pos),
                        (pos >> 16) as i16 as f64 * 8.0,
                        pos as i16 as f64 * 8.0,
                    ));
                }
            }
            UlocKind::Ore => {
                // Serpulo ore overlay ids from the official v158.1 content
                // registry (matches mine_result in listener.rs and
                // block_names.tsv): ore-copper 167, ore-lead 168, ore-scrap
                // 169, ore-coal 170, ore-titanium 171, ore-thorium 172,
                // ore-beryllium 173, ore-tungsten 174. (The old 73-80 ids
                // were pre-v8 floor ids and never matched any overlay, so
                // ulocate ore could not find anything.)
                let ore_id: i16 = match spec.ore.as_deref() {
                    Some("copper") => 167,
                    Some("lead") => 168,
                    Some("scrap") => 169,
                    Some("coal") => 170,
                    Some("titanium") => 171,
                    Some("thorium") => 172,
                    Some("beryllium") => 173,
                    Some("tungsten") => 174,
                    _ => 0,
                };
                let width = self.world.width.max(1) as usize;
                for (index, overlay) in self.world.overlays.iter().enumerate() {
                    if ore_id != 0 {
                        if *overlay != ore_id {
                            continue;
                        }
                    } else if *overlay == 0 {
                        continue;
                    }
                    // The overlays array is row-major (y*width + x), not a
                    // packed position; derive tile coords from the index.
                    let tx = (index % width) as f64;
                    let ty = (index / width) as f64;
                    let x = tx * 8.0;
                    let y = ty * 8.0;
                    let distance = ((x - origin_x).hypot(y - origin_y)) as f32;
                    if best.as_ref().map(|(_, b)| distance < *b).unwrap_or(true) {
                        best = Some((index as i32, distance));
                    }
                }
                if let Some((pos, _)) = best {
                    let tx = (pos as usize % width) as f64;
                    let ty = (pos as usize / width) as f64;
                    return Some((LObject::Null, tx * 8.0, ty * 8.0));
                }
            }
            UlocKind::Spawn => {
                for (sx, sy) in &self.world.enemy_spawns {
                    let x = f64::from(*sx) * 8.0;
                    let y = f64::from(*sy) * 8.0;
                    let distance = ((x - origin_x).hypot(y - origin_y)) as f32;
                    if best.as_ref().map(|(_, b)| distance < *b).unwrap_or(true) {
                        best = Some((0, distance));
                    }
                }
                if let Some((_, _)) = best {
                    let (sx, sy) = self.world.enemy_spawns[0];
                    return Some((LObject::Null, f64::from(sx) * 8.0, f64::from(sy) * 8.0));
                }
            }
            UlocKind::Core => {
                let pos = self.world.core_position();
                return Some((
                    LObject::Building(pos),
                    (pos >> 16) as i16 as f64 * 8.0,
                    pos as i16 as f64 * 8.0,
                ));
            }
        }
        // enemy flag ignored for same-team finds in phase 2 (enemy buildings
        // are not simulated as tile buildings in this port).
        let _ = enemy;
        None
    }

    /// setflag: store a global flag value.
    pub fn set_flag(&self, key: &str, value: f64) {
        self.world.logic_flags.insert(key.to_string(), value);
    }

    /// getflag: read a global flag (0 when unset).
    pub fn get_flag(&self, key: &str) -> f64 {
        self.world.logic_flags.get(key).map(|v| *v).unwrap_or(0.0)
    }

    /// spawn: create a unit on the resolved team at logic tile (x, y) with the
    /// given rotation and broadcast the authoritative UNIT_SPAWN. Returns the unit id.
    /// Mirrors v159.7 SpawnUnitI: non-internal type, Units.canCreate, optional effect.
    /// Position: `World.unconv(tile) + Mathf.range(0.01f)` on each axis.
    pub fn spawn_unit(
        &self,
        unit_type: i16,
        logic_x: f64,
        logic_y: f64,
        rotation: f64,
        team: u8,
        effect: bool,
    ) -> Option<i32> {
        let Some(spec) = crate::network::units::enemy_spec(unit_type) else {
            if self
                .world
                .game_state
                .strict_mode
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                tracing::error!(
                    "strict mode: logic spawn of unsupported unit id {} rejected",
                    unit_type
                );
            } else {
                tracing::warn!("logic spawn of unsupported unit id {} skipped", unit_type);
            }
            return None;
        };
        if crate::game::unit_types::unit_type_internal(unit_type) {
            return None;
        }
        if !crate::network::economy::can_create_unit(self.world, team, unit_type) {
            return None;
        }
        let id = self
            .world
            .next_enemy_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (spawn_x, spawn_y) = spawn_world_position(
            logic_x,
            logic_y,
            mathf_range(SPAWN_POSITION_JITTER),
            mathf_range(SPAWN_POSITION_JITTER),
        );
        let mut unit = crate::network::world::EnemyUnit {
            id,
            unit_type,
            entity_class: spec.entity_class,
            team,
            x: spawn_x,
            y: spawn_y,
            rotation: rotation as f32,
            health: spec.health,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 0.0,
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: spec.speed,
            attack_damage: spec.attack_damage,
            attack_reload_time: spec.attack_reload,
            attack_range: spec.attack_range,
            authority: crate::network::world::UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: Default::default(),
        };
        // P0-01: logic-spawned allies get their team's default controller.
        unit.authority = crate::network::units::default_unit_authority(self.world, &unit);
        if effect {
            crate::network::units::StatusContainer::apply_status(&mut unit, 3, 30.0);
            crate::network::units::StatusContainer::apply_status(&mut unit, 21, 60.0);
        }
        self.world.register_unit_group(id);
        self.world.enemies.insert(id, unit.clone());
        if let Ok(payload) =
            crate::network::wire::encode::encode_unit_spawn_payload(self.world, &unit)
        {
            if let Ok(frame) = crate::network::wire::encode::frame_generated_packet(
                crate::network::protocol::UNIT_SPAWN_PACKET_ID,
                &payload,
                false,
            ) {
                self.out.broadcast(frame);
            }
        }
        Some(id)
    }

    /// Official ApplyEffectI: apply or unapply a named status on a unit object.
    /// Duration is seconds (`* 60` ticks). Unknown effects no-op.
    pub fn apply_effect(&self, clear: bool, effect: &str, unit: &LVar, duration_seconds: f64) {
        let LObject::Unit(id) = unit.objval else {
            return;
        };
        let status = crate::network::units::status_effect_id_by_name(effect);
        if status < 0 {
            return;
        }
        let apply_to = |target: &mut dyn crate::network::units::StatusContainer| {
            if clear {
                let (effect_field, duration_field, statuses) = target.status_fields();
                statuses.retain(|entry| entry.effect != status);
                let (legacy, time) = crate::game::status::sync_legacy_view(statuses);
                *effect_field = legacy;
                *duration_field = time;
            } else {
                crate::network::units::StatusContainer::apply_status(
                    target,
                    status,
                    (duration_seconds as f32) * 60.0,
                );
            }
        };
        if let Some(mut enemy) = self.world.enemies.get_mut(&id) {
            apply_to(&mut *enemy);
            return;
        }
        if let Some(mut player) = self.world.players.get_mut(&id) {
            apply_to(&mut *player);
        }
    }

    /// Official SpawnWaveI: `natural` runs `Logic.skipWave()`; otherwise
    /// spawn groups for the current wave at the given TILE coordinates
    /// without incrementing the wave counter.
    pub fn spawn_wave(&self, natural: bool, tile_x: f64, tile_y: f64) {
        if natural {
            crate::network::combat::enemy::spawn_wave(self.world);
            return;
        }
        let wave = self
            .world
            .game_state
            .wave
            .load(std::sync::atomic::Ordering::Relaxed)
            .saturating_sub(1);
        let packed = ((tile_x as i32) << 16) | (tile_y as i32 & 0xffff);
        let spawn_x = (tile_x as f32) * 8.0;
        let spawn_y = (tile_y as f32) * 8.0;
        let groups = {
            let rules = self.world.wave_rules.read();
            if rules.is_default() {
                crate::network::units::initial_official_wave_groups(wave)
            } else {
                crate::network::units::map_wave_spawns(wave, &rules)
                    .into_iter()
                    .filter(|group| group.spawn < 0 || group.spawn == packed)
                    .collect()
            }
        };
        let team = self.world.wave_rules.read().wave_team;
        let mut index = 0u32;
        for group in groups {
            let (health_multiplier, speed_multiplier, damage_multiplier) =
                crate::game::status::status_multipliers(group.status_effect);
            for _ in 0..group.amount {
                let id = self
                    .world
                    .next_enemy_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let spread = (index as f32) * 2.0;
                self.world.enemies.insert(
                    id,
                    crate::network::world::EnemyUnit {
                        id,
                        unit_type: group.spec.unit_type,
                        entity_class: group.spec.entity_class,
                        team,
                        x: spawn_x + spread,
                        y: spawn_y,
                        rotation: -90.0,
                        health: group.spec.health * health_multiplier,
                        shield: group.shield,
                        status_effect: group.status_effect,
                        status_duration: f32::MAX,
                        statuses: if group.status_effect >= 0 {
                            vec![crate::game::status::ActiveStatus::simple(
                                group.status_effect,
                                f32::MAX,
                            )]
                        } else {
                            Vec::new()
                        },
                        velocity_x: 0.0,
                        velocity_y: 0.0,
                        elevation: 0.0,
                        payloads: Vec::new(),
                        flag: 0.0,
                        items: Vec::new(),
                        mine_progress: 0.0,
                        attack_reload: 0.0,
                        secondary_attack_reload: 0.0,
                        tertiary_attack_reload: 0.0,
                        quaternary_attack_reload: 0.0,
                        move_speed: group.spec.speed * speed_multiplier,
                        attack_damage: group.spec.attack_damage * damage_multiplier,
                        attack_reload_time: group.spec.attack_reload,
                        attack_range: group.spec.attack_range,
                        authority: crate::network::world::UnitAuthority::DefaultAi,
                        build_plans: Vec::new(),
                        update_building: true,
                        status_agg: Default::default(),
                    },
                );
                self.world.register_unit_group(id);
                index += 1;
            }
        }
        self.world.game_state.enemies_count.store(
            crate::network::combat::enemy::hostile_unit_count(self.world),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Official SetRuleI: mutate `state.rules` / wave / team multipliers.
    pub fn set_rule(
        &self,
        rule: LogicRule,
        value: &LVar,
        p1: &LVar,
        p2: &LVar,
        p3: &LVar,
        p4: &LVar,
    ) {
        use crate::logic::executor::lvar_bool;
        match rule {
            LogicRule::WaveTimer => self.world.wave_rules.write().wave_timer = lvar_bool(value),
            LogicRule::Wave => {
                let wave = (value.num() as i64).max(1) as u32;
                self.world
                    .game_state
                    .wave
                    .store(wave, std::sync::atomic::Ordering::Relaxed);
            }
            LogicRule::CurrentWaveTime => {
                *self.world.game_state.wave_time.write() = (value.num() as f32 * 60.0).max(0.0);
            }
            LogicRule::Waves => self.world.wave_rules.write().waves_enabled = lvar_bool(value),
            LogicRule::WaveSending => self.world.wave_rules.write().wave_sending = lvar_bool(value),
            LogicRule::WaveSpacing => {
                self.world.wave_rules.write().wave_spacing = value.num() as f32 * 60.0;
            }
            LogicRule::EnemyCoreBuildRadius => {
                self.world.wave_rules.write().enemy_core_build_radius = value.num() as f32 * 8.0;
            }
            LogicRule::UnitCap => {
                self.world.wave_rules.write().unit_cap = (value.num() as i64).max(0) as i32;
            }
            LogicRule::CanGameOver => {
                self.world.wave_rules.write().can_game_over = lvar_bool(value);
            }
            LogicRule::Ban | LogicRule::Unban => {
                let name = match &value.objval {
                    LObject::Str(s) => s.as_str(),
                    _ => return,
                };
                let add = rule == LogicRule::Ban;
                let mut rules = self.world.wave_rules.write();
                if let Some(unit) = crate::network::units::parse_unit_type(name) {
                    if add {
                        if !rules.banned_units.contains(&unit) {
                            rules.banned_units.push(unit);
                        }
                    } else {
                        rules.banned_units.retain(|id| *id != unit);
                    }
                } else if let Some(block) = crate::game::block_names::block_id_from_name(name) {
                    if add {
                        if !rules.banned_blocks.contains(&block) {
                            rules.banned_blocks.push(block);
                        }
                    } else {
                        rules.banned_blocks.retain(|id| *id != block);
                    }
                }
            }
            LogicRule::BuildSpeed
            | LogicRule::UnitHealth
            | LogicRule::UnitBuildSpeed
            | LogicRule::UnitMineSpeed
            | LogicRule::UnitCost
            | LogicRule::UnitDamage
            | LogicRule::BlockHealth
            | LogicRule::BlockDamage
            | LogicRule::RtsMinSquad
            | LogicRule::RtsMinWeight => {
                let Some(team) = crate::logic::executor::lvar_team(p1) else {
                    return;
                };
                let num = value.num() as f32;
                let mut rules = self.world.wave_rules.write();
                let team_rule = rules.team_rules.entry(team).or_default();
                match rule {
                    LogicRule::BuildSpeed => {
                        team_rule.build_speed_multiplier = num.clamp(0.001, 50.0);
                    }
                    LogicRule::UnitHealth => {
                        team_rule.unit_health_multiplier = num.max(0.001);
                    }
                    LogicRule::UnitBuildSpeed => {
                        team_rule.unit_build_speed_multiplier = num.clamp(0.0, 50.0);
                    }
                    LogicRule::UnitMineSpeed => {
                        team_rule.unit_mine_speed_multiplier = num.max(0.0);
                    }
                    LogicRule::UnitCost => {
                        team_rule.unit_cost_multiplier = num.max(0.0);
                    }
                    LogicRule::UnitDamage => {
                        team_rule.unit_damage_multiplier = num.max(0.0);
                    }
                    LogicRule::BlockHealth => {
                        team_rule.block_health_multiplier = num.max(0.001);
                    }
                    LogicRule::BlockDamage => {
                        team_rule.block_damage_multiplier = num.max(0.0);
                    }
                    LogicRule::RtsMinWeight => team_rule.rts_min_weight = num,
                    LogicRule::RtsMinSquad => team_rule.rts_min_squad = num as i32,
                    _ => {}
                }
            }
            LogicRule::MapArea => {
                let _ = (p1, p2, p3, p4);
            }
            LogicRule::AttackMode
            | LogicRule::DropZoneRadius
            | LogicRule::Lighting
            | LogicRule::AmbientLight
            | LogicRule::SolarMultiplier
            | LogicRule::DragMultiplier
            | LogicRule::PauseDisabled
            | LogicRule::MusicVolume => {}
        }
    }

    /// Official logicExplosion damage path (no FX). Coords and radius are
    /// TILE units; converted with World.unconv (`* 8`), radius capped at 100.
    #[allow(clippy::too_many_arguments)]
    pub fn logic_explosion(
        &self,
        team: u8,
        tile_x: f64,
        tile_y: f64,
        tile_radius: f64,
        damage: f64,
        air: bool,
        ground: bool,
        pierce: bool,
    ) {
        let x = tile_x as f32 * 8.0;
        let y = tile_y as f32 * 8.0;
        let radius = tile_radius.min(100.0) as f32 * 8.0;
        let damage = damage as f32;
        let ids: Vec<i32> = self.world.enemies.iter().map(|unit| *unit.key()).collect();
        for id in ids {
            let Some(mut unit) = self.world.enemies.get_mut(&id) else {
                continue;
            };
            if unit.team == team || unit.health <= 0.0 {
                continue;
            }
            let flying = unit.elevation > 0.0;
            if flying && !air || !flying && !ground {
                continue;
            }
            let dist = (unit.x - x).hypot(unit.y - y);
            if dist > radius {
                continue;
            }
            unit.health = (unit.health - explosion_falloff(dist, radius, damage)).max(0.0);
        }
        if !ground {
            return;
        }
        let positions: Vec<i32> = self.world.tiles.iter().map(|tile| *tile.key()).collect();
        for pos in positions {
            let Some(tile) = self.world.tiles.get(&pos) else {
                continue;
            };
            if tile.team == team || tile.block == 0 {
                continue;
            }
            let bx = ((pos >> 16) as i16 as f32) * 8.0;
            let by = (pos as i16 as f32) * 8.0;
            let dist = (bx - x).hypot(by - y);
            if dist > radius {
                continue;
            }
            drop(tile);
            let applied = if pierce {
                damage
            } else {
                explosion_falloff(dist, radius, damage)
            };
            let _ = crate::network::combat::enemy::damage_building(self.world, pos, applied);
        }
    }

    /// Official SetPropI against a unit or building object.
    pub fn set_prop(&self, key: &SetPropKey, of: &LVar, value: &LVar) {
        match of.objval {
            LObject::Unit(id) => self.set_unit_prop(id, key, value),
            LObject::Building(pos) => self.set_building_prop(pos, key, value),
            _ => {}
        }
    }

    fn set_unit_prop(&self, id: i32, key: &SetPropKey, value: &LVar) {
        let Some(mut unit) = self.world.enemies.get_mut(&id) else {
            return;
        };
        match key {
            SetPropKey::Access(LAccess::Health) => {
                let max = crate::network::units::enemy_spec(unit.unit_type)
                    .map(|spec| spec.health)
                    .unwrap_or(unit.health.max(1.0));
                unit.health = (value.num() as f32).clamp(0.0, max);
            }
            SetPropKey::Access(LAccess::Shield) => {
                unit.shield = (value.num() as f32).max(0.0);
            }
            SetPropKey::Access(LAccess::X) => unit.x = value.num() as f32 * 8.0,
            SetPropKey::Access(LAccess::Y) => unit.y = value.num() as f32 * 8.0,
            SetPropKey::Access(LAccess::Team) => {
                if let Some(team) = crate::logic::executor::lvar_team(value) {
                    unit.team = team;
                } else {
                    unit.team = value.num() as u8;
                }
            }
            SetPropKey::Access(LAccess::Flag) => unit.flag = value.num(),
            SetPropKey::Access(LAccess::Rotation) => unit.rotation = value.num() as f32,
            SetPropKey::Item(item) => {
                let amount = (value.num() as i32).max(0);
                if let Some(entry) = unit.items.iter_mut().find(|(id, _)| *id == *item) {
                    entry.1 = amount;
                } else if amount > 0 {
                    unit.items.push((*item, amount));
                }
            }
            _ => {}
        }
    }

    fn set_building_prop(&self, pos: i32, key: &SetPropKey, value: &LVar) {
        match key {
            SetPropKey::Access(LAccess::Health) => {
                let Some(tile) = self.world.tiles.get(&pos) else {
                    return;
                };
                let max = crate::game::content::block_health(tile.block);
                drop(tile);
                let health = (value.num() as f32).clamp(0.0, max);
                if health <= 0.0 {
                    let _ =
                        crate::network::combat::enemy::damage_building(self.world, pos, f32::MAX);
                    return;
                }
                if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
                    tile.health = health;
                }
            }
            SetPropKey::Access(LAccess::Team) => {
                let team = if let Some(team) = crate::logic::executor::lvar_team(value) {
                    team
                } else {
                    value.num() as u8
                };
                crate::network::buildings::placement::change_building_team(self.world, pos, team);
            }
            SetPropKey::Access(LAccess::Enabled) => {
                if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
                    tile.enabled = crate::logic::executor::lvar_bool(value);
                }
            }
            SetPropKey::Item(item) => {
                let amount = (value.num() as i32).max(0);
                if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
                    if let Some(entry) = tile.inventory.iter_mut().find(|(id, _)| *id == *item) {
                        if amount == 0 {
                            tile.inventory.retain(|(id, _)| *id != *item);
                        } else {
                            entry.1 = amount;
                        }
                    } else if amount > 0 {
                        tile.inventory.push((*item, amount));
                    }
                }
            }
            SetPropKey::Liquid(liquid) => {
                let amount = (value.num() as f32).max(0.0);
                if let Some(mut tile) = self.world.tiles.get_mut(&pos) {
                    if let Some(entry) = tile
                        .liquid_inventory
                        .iter_mut()
                        .find(|(id, _)| *id == *liquid)
                    {
                        entry.1 = amount;
                    } else if amount > 0.0 {
                        tile.liquid_inventory.push((*liquid, amount));
                    }
                    tile.stored_liquid = *liquid;
                    tile.liquid_amount = amount;
                }
            }
            _ => {}
        }
    }

    /// Sensor read for a unit object target (LObject::Unit).
    pub fn unit_sensor(&self, unit_id: i32, item: LAccess) -> SensorValue {
        let Some(unit) = self.world.enemies.get(&unit_id) else {
            return SensorValue::Num(0.0);
        };
        match item {
            LAccess::X => SensorValue::Num(f64::from(unit.x)),
            LAccess::Y => SensorValue::Num(f64::from(unit.y)),
            LAccess::Health => SensorValue::Num(f64::from(unit.health)),
            LAccess::Team => SensorValue::Num(f64::from(unit.team)),
            LAccess::Dead => SensorValue::Num(f64::from(unit.health <= 0.0)),
            LAccess::Range => SensorValue::Num(f64::from(unit.attack_range)),
            LAccess::Size => {
                SensorValue::Num(crate::game::content::block_placement(0).size as f64 * 2.0)
            }
            LAccess::TotalItems => {
                let total: i32 = unit.items.iter().map(|(_, amount)| *amount).sum();
                SensorValue::Num(f64::from(total))
            }
            LAccess::Flag => SensorValue::Num(unit.flag),
            LAccess::Shield => SensorValue::Num(f64::from(unit.shield)),
            LAccess::Rotation => SensorValue::Num(f64::from(unit.rotation)),
            LAccess::Flying => {
                // Official UnitComp.java:88: `isFlying() { return elevation >= 0.09f; }`
                SensorValue::Num(if unit.elevation >= 0.09 { 1.0 } else { 0.0 })
            }
            _ => SensorValue::Num(0.0),
        }
    }

    pub fn sensor(&self, target: &LVar, item: LAccess) -> SensorValue {
        use crate::network::buildings::snapshot::dynamic_tile_health;
        match item {
            LAccess::Time => {
                return SensorValue::Num(
                    f64::from(*self.world.game_state.simulation_time.read()) / 60.0,
                );
            }
            LAccess::Tick => {
                return SensorValue::Num(
                    f64::from(*self.world.game_state.simulation_time.read()) / 6.0,
                );
            }
            LAccess::WaveNumber => {
                return SensorValue::Num(
                    self.world
                        .game_state
                        .wave
                        .load(std::sync::atomic::Ordering::Relaxed) as f64,
                );
            }
            LAccess::Second => {
                return SensorValue::Num(
                    f64::from(*self.world.game_state.simulation_time.read()) / 60.0,
                );
            }
            LAccess::Minute => {
                return SensorValue::Num(
                    f64::from(*self.world.game_state.simulation_time.read()) / 3600.0,
                );
            }
            LAccess::This => {
                return SensorValue::Obj(LObject::Building(self.processor_pos));
            }
            LAccess::Unit => return SensorValue::Obj(LObject::Null),
            // Counter, Links and Ipt are executor state (handled in
            // run_instruction); a building sensor never sees them.
            LAccess::Counter | LAccess::Links | LAccess::Ipt => {
                return SensorValue::Num(0.0);
            }
            _ => {}
        }
        let LObject::Building(pos) = target.objval else {
            if let LObject::Unit(unit_id) = target.objval {
                return self.unit_sensor(unit_id, item);
            }
            return SensorValue::Num(0.0);
        };
        let Some(tile) = self.world.tiles.get(&pos) else {
            return SensorValue::Num(0.0);
        };
        match item {
            LAccess::Health => SensorValue::Num(dynamic_tile_health(&tile) as f64),
            LAccess::Team => SensorValue::Num(f64::from(tile.team)),
            LAccess::Block => SensorValue::Num(f64::from(tile.block)),
            LAccess::Enabled => SensorValue::Num(f64::from(tile.enabled)),
            LAccess::TotalItems => {
                let total: i64 = tile
                    .inventory
                    .iter()
                    .map(|(_, amount)| *amount as i64)
                    .sum();
                SensorValue::Num(total as f64)
            }
            LAccess::TotalLiquids => SensorValue::Num(f64::from(tile.liquid_amount)),
            LAccess::X => SensorValue::Num((pos >> 16) as i16 as f64 * 8.0),
            LAccess::Y => SensorValue::Num(pos as i16 as f64 * 8.0),
            LAccess::Size => {
                SensorValue::Num(crate::game::content::block_placement(tile.block).size as f64)
            }
            LAccess::Range => SensorValue::Num(0.0),
            LAccess::Dead => SensorValue::Num(0.0),
            _ => SensorValue::Num(0.0),
        }
    }
}

fn explosion_falloff(dist: f32, radius: f32, damage: f32) -> f32 {
    let falloff = 0.4;
    let scaled = if radius <= 0.00001 {
        1.0
    } else {
        (1.0 - dist / radius) * (1.0 - falloff) + falloff
    };
    damage * scaled
}
