//! Logic processors in the simulation pass: config hash, leases, simulate_logic
//! and ucontrol-mining/fire/build autopilot.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::logic::*;
use crate::network::buildings::construction::{block_footprint, dynamic_at};
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::valid_logic_config;
use crate::network::buildings::snapshot::*;
use crate::network::combat::enemy::base_building_at;
use crate::network::combat::unit_combat::{collect_allied_weapon_fire, spawn_allied_weapon_fire};
use crate::network::combat::*;
use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::unit_orders::{clear_logic_build_order, place_logic_building};
use crate::network::units::*;
use crate::network::wire::encode::encode_block_snapshot;
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;
use tracing::{debug, error, info, warn};

use super::*;

pub(crate) fn logic_config_hash(config: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config.hash(&mut hasher);
    hasher.finish()
}

/// P0-03: the authoritative LogicAI lease clock — the Rust counterpart of
/// `LogicAI.updateMovement`'s timeout block (desktop 158.1 LogicAI.java:
/// 59-64):
///
/// ```java
/// if(controlTimer > 0 && controller != null && controller.isValid()){
///     controlTimer -= Time.delta;
/// }else{
///     unit.resetController();
///     return;
/// }
/// ```
///
/// Every unit under [`UnitAuthority::Logic`] decrements
/// `remaining_ticks` by the authoritative world delta (Arc's `Time.delta`
/// is 1.0 per tick at 60 TPS on the official server, ServerControl.java:197,
/// so a refreshed lease of 600 expires after 600 ticks). Expiry
/// (`remaining <= 0` on the pre-decrement check, exactly Java's
/// `controlTimer > 0`) or an invalid processor instance (tile gone,
/// replaced by a non-processor, or a new processor on the same tile —
/// Java's `controller.isValid()` is `tile.build == this && !dead`)
/// releases the unit back to its default controller and drops the
/// transient logic order state (kinds 6-9).
///
/// This runs in the world tick — NOT inside `simulate_logic` — because the
/// official unit controller updates once per game tick regardless of any
/// processor's instructions-per-tick budget. Units update before buildings
/// in Java's entity-group order, so the pass runs before `simulate_logic`
/// within a tick: a lease that reached zero is released before that tick's
/// processor step could refresh it (a later `ucontrol` then re-acquires
/// with the first-takeover cleanup, exactly like Java).
pub fn simulate_logic_control_leases(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let logic_units: Vec<i32> = world
        .enemies
        .iter()
        .filter(|unit| matches!(unit.authority, UnitAuthority::Logic { .. }))
        .map(|unit| unit.id)
        .collect();
    let mut changed = false;
    for unit_id in logic_units {
        let lease = world
            .enemies
            .get(&unit_id)
            .and_then(|unit| match unit.authority {
                UnitAuthority::Logic {
                    processor_pos,
                    remaining_ticks,
                    processor_generation,
                } => Some((processor_pos, remaining_ticks, processor_generation)),
                _ => None,
            });
        let Some((processor_pos, remaining_ticks, processor_generation)) = lease else {
            continue;
        };
        let processor_valid = crate::network::units::processor_lease_valid(
            world,
            processor_pos,
            processor_generation,
        );
        if processor_valid && remaining_ticks > 0.0 {
            if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
                if let UnitAuthority::Logic {
                    processor_pos,
                    processor_generation,
                    ..
                } = unit.authority
                {
                    unit.authority = UnitAuthority::Logic {
                        processor_pos,
                        remaining_ticks: remaining_ticks - delta_ticks,
                        processor_generation,
                    };
                }
            }
        } else {
            crate::network::units::release_logic_control(world, unit_id);
            changed = true;
        }
    }
    changed
}

/// Runs the live logic processors (micro 431, logic 432, hyper 433, and
/// privileged world processor 442) for one
/// simulation tick. Compiles each processor's config once and recompiles when
/// the program changes; executes up to the block's instructions-per-tick and
/// applies side effects (memory cells, message blocks, control enabled).
pub fn simulate_logic(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    // Snapshot processor tiles before touching the world (anti-deadlock).
    let processors: Vec<(i32, i16, Vec<u8>)> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 431..=433 | 442))
        .filter_map(|tile| {
            building_config::logic_payload(&tile.config)
                .map(|payload| (tile.position, tile.block, payload.to_vec()))
        })
        .collect();
    if processors.is_empty() {
        return false;
    }
    let mut changed = false;
    let strict = world.game_state.strict_mode.load(Ordering::Relaxed);
    for (pos, block, config) in processors {
        if !valid_logic_config(&config) {
            // P0-7: malformed logic configs are not silently dropped in
            // strict mode; the processor is rejected with a located error.
            if strict {
                let tile_x = (pos >> 16) as i16;
                let tile_y = pos as i16;
                let mut entry = world.logic_executors.entry(pos).or_insert_with(|| {
                    let mut state =
                        crate::logic::ExecutorState::new(empty_logic_program(), Vec::new());
                    state.rejected = true;
                    state
                });
                if !entry.rejected {
                    entry.rejected = true;
                }
                error!(
                    "strict mode: logic processor at ({tile_x},{tile_y}) has a malformed config; program rejected"
                );
                continue;
            }
            continue;
        }
        let hash = logic_config_hash(&config);
        // P0-7: compile with diagnostics; strict mode rejects programs that
        // degrade statements to NoOp instead of running them partially.
        let source = crate::logic::source_from_config(&config).unwrap_or_default();
        let (compiled, diagnostics) = crate::logic::compile_report(&source);
        if strict && !diagnostics.is_empty() {
            let tile_x = (pos >> 16) as i16;
            let tile_y = pos as i16;
            let mut entry = world.logic_executors.entry(pos).or_insert_with(|| {
                let mut state = crate::logic::ExecutorState::new(empty_logic_program(), Vec::new());
                state.rejected = true;
                state
            });
            if !entry.rejected {
                entry.rejected = true;
            }
            error!(
                "strict mode: logic processor at ({tile_x},{tile_y}) rejected: {}",
                diagnostics.join("; ")
            );
            continue;
        }
        let program = compiled.unwrap_or_else(empty_logic_program);
        let mut entry = world.logic_executors.entry(pos).or_insert_with(|| {
            let tile_x = (pos >> 16) as i16;
            let tile_y = pos as i16;
            let links = crate::logic::parse_links(&config)
                .into_iter()
                .map(|(x, y)| (((tile_x + x) as i32) << 16) | (tile_y + y) as i32)
                .collect();
            let mut state = crate::logic::ExecutorState::new(program.clone(), links);
            state.config_hash = hash;
            state.privileged = block == 442;
            state
        });
        if entry.config_hash != hash {
            let tile_x = (pos >> 16) as i16;
            let tile_y = pos as i16;
            let links = crate::logic::parse_links(&config)
                .into_iter()
                .map(|(x, y)| (((tile_x + x) as i32) << 16) | (tile_y + y) as i32)
                .collect();
            let mut state = crate::logic::ExecutorState::new(program, links);
            state.config_hash = hash;
            state.privileged = block == 442;
            *entry = state;
        }
        // LogicBlock instructionsPerTick from Blocks.java: micro=2,
        // logic=8, hyper=25; world processors are privileged but start at 8
        // and may raise their rate through `setrate` up to 1000.
        entry.privileged = block == 442;
        let budget = match block {
            431 => 2,
            432 | 442 => 8,
            _ => 25,
        };
        let ipt_var = entry.program.ipt_var;
        if let Some(v) = entry.vars.get_mut(ipt_var) {
            v.numval = budget as f64;
            v.isobj = false;
        }
        let view = crate::logic::WorldView {
            world,
            processor_pos: pos,
            out,
        };
        // The official LogicBuild.updateLogic runs every GAME tick (60fps);
        // our simulation tick is 100ms = 6 game ticks.
        let steps = delta_ticks.round().max(1.0) as usize;
        for _ in 0..steps {
            entry.run_tick(Some(&view), budget);
        }
        changed = true;
    }
    changed
}

pub(crate) fn empty_logic_program() -> Arc<crate::logic::Program> {
    // A program with no instructions; run_tick is a no-op.
    crate::logic::compile("").unwrap_or_else(|| {
        // Unreachable: compile("") always succeeds.
        let program = crate::logic::Program {
            vars: Vec::new(),
            instructions: Vec::new(),
            counter_var: 0,
            this_var: 0,
            unit_var: 0,
            links_var: 0,
            ipt_var: 0,
        };
        Arc::new(program)
    })
}

/// Game ticks simulated per world-loop step for a target TPS (SOL-005).
/// Official server runs at 60 TPS with delta 1.0; the legacy port default of
/// 10 TPS (100 ms step) keeps delta 6.0, preserving the exact same
/// accumulated simulation time.
pub fn simulate_logic_mining(world: &DynamicWorld, snapshot: &EnemyUnit, delta_ticks: f32) {
    let Some(order) = world
        .unit_orders
        .get(&snapshot.id)
        .map(|order| order.clone())
    else {
        return;
    };
    let target_x = order.target_x.unwrap_or(snapshot.x);
    let target_y = order.target_y.unwrap_or(snapshot.y);
    let dx = target_x - snapshot.x;
    let dy = target_y - snapshot.y;
    let distance = dx.hypot(dy);
    if distance > 6.0 {
        if let Some(mut unit) = world.enemies.get_mut(&snapshot.id) {
            unit.x += dx / distance * unit.move_speed * delta_ticks;
            unit.y += dy / distance * unit.move_speed * delta_ticks;
        }
        return;
    }
    let tile_x = (f64::from(target_x) / 8.0).floor() as i16;
    let tile_y = (f64::from(target_y) / 8.0).floor() as i16;
    let pos = ((tile_x as i32) << 16) | tile_y as i32;
    let Some(item) = world
        .overlays
        .get(pos as usize)
        .and_then(|overlay| crate::logic::ore_item_id(*overlay))
    else {
        return;
    };
    let carried: i32 = snapshot.items.iter().map(|(_, amount)| *amount).sum();
    if carried >= 2 {
        return;
    }
    if let Some(mut unit) = world.enemies.get_mut(&snapshot.id) {
        unit.mine_progress += delta_ticks;
        if unit.mine_progress >= 60.0 {
            unit.mine_progress = 0.0;
            if let Some(entry) = unit.items.iter_mut().find(|(i, _)| *i == item) {
                entry.1 += 1;
            } else {
                unit.items.push((item, 1));
            }
        }
    }
}

pub fn simulate_logic_fire(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    snapshot: &EnemyUnit,
    delta_ticks: f32,
) {
    let Some(order) = world
        .unit_orders
        .get(&snapshot.id)
        .map(|order| order.clone())
    else {
        return;
    };
    let target_x = order.target_x.unwrap_or(snapshot.x);
    let target_y = order.target_y.unwrap_or(snapshot.y);
    let angle = (target_x - snapshot.x)
        .atan2(target_y - snapshot.y)
        .to_degrees();
    let mut fires = None;
    if let Some(mut unit) = world.enemies.get_mut(&snapshot.id) {
        unit.velocity_x = 0.0;
        unit.velocity_y = 0.0;
        unit.rotation = angle;
        if order.target_kind == 7 {
            let distance = (target_x - unit.x).hypot(target_y - unit.y);
            fires = collect_allied_weapon_fire(&mut unit, delta_ticks, distance);
        }
    }
    if let Some(fires) = fires {
        spawn_allied_weapon_fire(
            world,
            out,
            &fires,
            snapshot.id,
            -1,
            None,
            snapshot.x,
            snapshot.y,
            target_x,
            target_y,
        );
    }
}

pub fn simulate_logic_build(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    snapshot: &EnemyUnit,
    delta_ticks: f32,
) {
    let Some(order) = world
        .unit_orders
        .get(&snapshot.id)
        .map(|order| order.clone())
    else {
        return;
    };
    let block = (order.target_id & 0xffff) as i16;
    let rotation = ((order.target_id >> 16) & 3) as u8;
    let target_x = order.target_x.unwrap_or(snapshot.x);
    let target_y = order.target_y.unwrap_or(snapshot.y);
    let dx = target_x - snapshot.x;
    let dy = target_y - snapshot.y;
    let distance = dx.hypot(dy);
    if distance > 8.0 {
        if let Some(mut unit) = world.enemies.get_mut(&snapshot.id) {
            unit.x += dx / distance * unit.move_speed * delta_ticks;
            unit.y += dy / distance * unit.move_speed * delta_ticks;
        }
        return;
    }
    let tile_x = (f64::from(target_x) / 8.0).floor() as i16;
    let tile_y = (f64::from(target_y) / 8.0).floor() as i16;
    let position = ((tile_x as i32) << 16) | tile_y as i32;
    // Site must be empty; otherwise cancel the order.
    let Some(occupied) = block_footprint(world, position, block) else {
        clear_logic_build_order(world, snapshot.id);
        return;
    };
    if occupied.iter().any(|position| {
        dynamic_at(world, *position).is_some() || base_building_at(world, *position).is_some()
    }) {
        clear_logic_build_order(world, snapshot.id);
        return;
    }
    let should_place = if let Some(mut unit) = world.enemies.get_mut(&snapshot.id) {
        unit.mine_progress += delta_ticks;
        if unit.mine_progress >= 60.0 {
            unit.mine_progress = 0.0;
            true
        } else {
            false
        }
    } else {
        false
    };
    // Release the enemies write guard before helpers that transitively read
    // `world.enemies` (place_logic_building → encode_block_snapshot).
    if should_place {
        place_logic_building(world, out, position, block, rotation, occupied);
        clear_logic_build_order(world, snapshot.id);
    }
}
