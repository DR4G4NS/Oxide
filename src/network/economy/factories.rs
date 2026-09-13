//! Factory simulation: separators, unit factories, reconstructors, liquid
//! factories, unit caps. The economy facade re-exports through
//! crate::network::economy::*.

use crate::game::content::block_size;
use crate::network::buildings::construction::{block_footprint, dynamic_at};
use crate::network::buildings::snapshot::*;
use crate::network::economy::spec::{
    accept_logistics_item_from, factory_recipe, inventory_add, inventory_count, inventory_remove,
    inventory_total, offset_position,
};
use crate::network::units::*;
use crate::network::wire::encode::{encode_unit_spawn_payload, frame_generated_packet};
use crate::network::wire::tile_config::configured_unit_command;
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

/// Official `PayloadBlock` ctor (159.7): `payloadSpeed = 0.7`, `payloadRotateSpeed = 5`.
const PAYLOAD_SPEED: f32 = 0.7;
const PAYLOAD_ROTATE_SPEED: f32 = 5.0;
const PAYLOAD_ARRIVED: f32 = 0.01;
const PAYLOAD_AT_DEST: f32 = 0.001;

fn unit_block_half(block: i16) -> f32 {
    f32::from(block_size(block)) * 4.0
}

fn set_pay_vector(tile: &mut DynamicTile, x: f32, y: f32) {
    if x == 0.0 && y == 0.0 {
        tile.payload_accum.clear();
    } else {
        tile.payload_accum = vec![x, y];
    }
}

/// Initialize every newly received UnitPayload consistently, whether it
/// arrives from another unit block, a conveyor, or a carrying unit.
pub(crate) fn initialize_received_unit_payload(
    tile: &mut DynamicTile,
    source_x: f32,
    source_y: f32,
    rotation: f32,
) {
    let Some(CarriedPayload::Unit(unit)) = tile.payload.as_deref() else {
        return;
    };
    let unit_type = unit.unit_type;
    let (cx, cy) = building_center(tile.position, tile.block);
    let half = unit_block_half(tile.block);
    set_pay_vector(
        tile,
        (source_x - cx).clamp(-half, half),
        (source_y - cy).clamp(-half, half),
    );
    tile.payload_rotation = rotation;
    tile.production_progress = 0.0;
    tile.stored_amount = i32::from(unit_type) + 1;
}

fn approach_vec(current: (f32, f32), dest: (f32, f32), step: f32) -> (f32, f32) {
    let dx = dest.0 - current.0;
    let dy = dest.1 - current.1;
    let dist = dx.hypot(dy);
    if dist <= step {
        dest
    } else {
        (current.0 + dx / dist * step, current.1 + dy / dist * step)
    }
}

fn output_dest(tile: &DynamicTile) -> (f32, f32) {
    let half = unit_block_half(tile.block);
    let radians = (f32::from(tile.rotation) * 90.0).to_radians();
    (radians.cos() * half, radians.sin() * half)
}

fn has_arrived(tile: &DynamicTile) -> bool {
    let (x, y) = unit_block_pay_vector(tile);
    x.hypot(y) <= PAYLOAD_ARRIVED
}

fn front_position(tile: &DynamicTile) -> i32 {
    // Official Building.front: nearby(d4(rotation) * (size/2 + 1)) from the
    // origin tile (odd-sized UnitBlock origins are the centre).
    let trns = i32::from(block_size(tile.block)) / 2 + 1;
    offset_position_by(tile.position, tile.rotation, trns)
}

fn apply_move_in(tile: &mut DynamicTile, delta: f32) -> bool {
    let dest_rot = f32::from(tile.rotation) * 90.0;
    tile.payload_rotation = move_toward_angle(
        tile.payload_rotation,
        dest_rot,
        PAYLOAD_ROTATE_SPEED * delta,
    );
    let next = approach_vec(
        unit_block_pay_vector(tile),
        (0.0, 0.0),
        PAYLOAD_SPEED * delta,
    );
    set_pay_vector(tile, next.0, next.1);
    has_arrived(tile)
}

fn apply_move_out_slide(tile: &mut DynamicTile, delta: f32) -> (f32, f32) {
    let dest = output_dest(tile);
    let dest_rot = f32::from(tile.rotation) * 90.0;
    tile.payload_rotation = move_toward_angle(
        tile.payload_rotation,
        dest_rot,
        PAYLOAD_ROTATE_SPEED * delta,
    );
    let next = approach_vec(unit_block_pay_vector(tile), dest, PAYLOAD_SPEED * delta);
    set_pay_vector(tile, next.0, next.1);
    dest
}

pub(crate) fn unit_allowed_in_payloads(unit_type: i16) -> bool {
    // Official UnitType.allowedInPayloads is true for combat units and false
    // for missiles / the internal `block` unit / assembly drones.
    !matches!(unit_type, 46 | 53 | 55 | 61..=67)
}

pub(crate) fn unit_spawned_by_core(unit_type: i16) -> bool {
    // Official core ships spawn with spawnedByCore=true: Serpulo
    // alpha/beta/gamma and Erekir evoke/incite/emanate.
    matches!(unit_type, 35..=37 | 58..=60)
}

fn unit_tile_position(unit: &EnemyUnit) -> i32 {
    let x = crate::network::combat::enemy::world_to_tile(unit.x);
    let y = crate::network::combat::enemy::world_to_tile(unit.y);
    (x << 16) | (y as u16 as i32)
}

fn unit_on_building_footprint(
    world: &DynamicWorld,
    unit: &EnemyUnit,
    building: &DynamicTile,
) -> bool {
    let tile = unit_tile_position(unit);
    if tile == building.position || building.occupied.contains(&tile) {
        return true;
    }
    block_footprint(world, building.position, building.block)
        .is_some_and(|footprint| footprint.contains(&tile))
}

pub(crate) fn front_accepts_payload(
    world: &DynamicWorld,
    front: &DynamicTile,
    payload: &CarriedPayload,
) -> bool {
    if !front.enabled || front.payload.is_some() {
        return false;
    }
    if let Some(limit) = payload_block_limit(front.block) {
        if matches!(front.block, 398..=409)
            && payload_fits_limit(payload, limit)
            && payload_block_accepts(front.block, payload)
        {
            return true;
        }
    }
    if let CarriedPayload::Unit(unit) = payload {
        if reconstructor_recipe(front.block).is_some() {
            let Some(output) = reconstructor_upgrade(front.block, unit.unit_type) else {
                return false;
            };
            return !world.wave_rules.read().unit_banned(output);
        }
    }
    false
}

/// Official `PayloadBlockBuild.moveOutPayload` for UnitBlock descendants.
/// Snapshots the tile, slides `payVector`, then either hands the payload to
/// the front acceptor or dumps into the world. Never holds a tile guard
/// while querying another tile.
fn move_out_unit_payload(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    key: i32,
    delta_ticks: f32,
) -> bool {
    let dest = if let Some(mut tile) = world.tiles.get_mut(&key) {
        if tile.payload.is_none() {
            return false;
        }
        apply_move_out_slide(&mut tile, delta_ticks.max(0.0))
    } else {
        return false;
    };
    let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
        return false;
    };
    let (px, py) = unit_block_pay_vector(&snapshot);
    if (px - dest.0).hypot(py - dest.1) > PAYLOAD_AT_DEST {
        return true;
    }
    let front = dynamic_at(world, front_position(&snapshot));
    let front_accepts =
        front
            .as_ref()
            .zip(snapshot.payload.as_deref())
            .is_some_and(|(front, payload)| {
                front.team == snapshot.team && front_accepts_payload(world, front, payload)
            });
    if front_accepts {
        if let Some(front) = front {
            let payload = world
                .tiles
                .get_mut(&key)
                .and_then(|mut tile| tile.payload.take());
            if let Some(payload) = payload {
                if let Some(mut receiver) = world.tiles.get_mut(&front.position) {
                    let (sx, sy) = building_center(snapshot.position, snapshot.block);
                    let (rx, ry) = building_center(receiver.position, receiver.block);
                    let half = unit_block_half(receiver.block);
                    receiver.payload = Some(payload);
                    set_pay_vector(
                        &mut receiver,
                        (sx - rx).clamp(-half, half),
                        (sy - ry).clamp(-half, half),
                    );
                    receiver.payload_rotation = snapshot.payload_rotation;
                    if reconstructor_recipe(receiver.block).is_some() {
                        if let Some(CarriedPayload::Unit(unit)) = receiver.payload.as_deref() {
                            receiver.stored_amount = i32::from(unit.unit_type) + 1;
                            receiver.production_progress = 0.0;
                        }
                    }
                }
                if let Some(mut sender) = world.tiles.get_mut(&key) {
                    sender.payload = None;
                    sender.payload_accum.clear();
                    sender.stored_amount = 0;
                    sender.production_progress = 0.0;
                }
            }
        }
        return true;
    }
    let can_dump = front.as_ref().is_none_or(|front| {
        !crate::game::content::building_check_solid(front.block, front.door_open)
            || front.position == snapshot.position
    });
    // Empty or non-solid front (conveyor, open door) → dump. Solid wall → hold.
    if front.is_some() && !can_dump {
        return true;
    }
    let Some(CarriedPayload::Unit(unit)) = snapshot.payload.as_deref().cloned() else {
        return true;
    };
    let (dump_x, dump_y) = {
        let (center_x, center_y) = building_center(snapshot.position, snapshot.block);
        let (px, py) = unit_block_pay_vector(&snapshot);
        (center_x + px, center_y + py)
    };
    if !can_create_unit(world, unit.team, unit.unit_type)
        || !payload_dump_world_clear(world, &unit, dump_x, dump_y)
    {
        return true;
    }
    if let Some(mut live) = world.tiles.get_mut(&key) {
        live.payload = None;
        live.payload_accum.clear();
        live.stored_amount = 0;
        live.production_progress = 0.0;
    }
    release_held_unit(world, out, &snapshot, unit);
    true
}

fn attribute_crafter_boost(world: &DynamicWorld, block: i16, position: i32) -> f32 {
    use crate::game::content::FloorAttribute;
    let (attribute, scale, size, max_boost) = match block {
        184 => (FloorAttribute::Heat, 0.15, 3, 1.0),
        330 => (FloorAttribute::Spores, 1.0, 2, 2.0),
        _ => return 1.0,
    };
    let tile_x = (position >> 16) as i16 as i32;
    let tile_y = position as i16 as i32;
    let mut sum = 0.0;
    for dy in 0..size {
        for dx in 0..size {
            sum += crate::game::content::floor_attribute(
                crate::network::combat::floor_at_tile(world, tile_x + dx, tile_y + dy),
                attribute,
            );
        }
    }
    (1.0 + (sum * scale).min(max_boost)).max(0.0)
}

pub(crate) fn simulate_factories(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| factory_recipe(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(recipe) = factory_recipe(snapshot.block) else {
            continue;
        };
        let has_inputs = recipe
            .inputs
            .iter()
            .all(|(item, amount)| inventory_count(&snapshot.inventory, *item) >= *amount);
        let final_total = inventory_total(&snapshot.inventory)
            - recipe.inputs.iter().map(|(_, amount)| *amount).sum::<i32>()
            + recipe.output.1;
        if has_inputs && final_total <= recipe.capacity {
            let efficiency = power.get(&key).copied().unwrap_or(1.0);
            if efficiency > 0.0 {
                let boost = attribute_crafter_boost(world, snapshot.block, snapshot.position);
                let time_scale = building_time_scale(world, key);
                let crafted = if let Some(mut factory) = world.tiles.get_mut(&key) {
                    factory.production_progress += delta_ticks * time_scale * efficiency * boost;
                    if factory.production_progress >= recipe.craft_time {
                        factory.production_progress %= recipe.craft_time;
                        for (item, amount) in recipe.inputs {
                            let removed = inventory_remove(&mut factory.inventory, *item, *amount);
                            debug_assert!(removed);
                        }
                        inventory_add(&mut factory.inventory, recipe.output.0, recipe.output.1);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                changed = true;
                if crafted {
                    changed |= dump_factory_output(world, key, recipe.output.0);
                }
            }
        } else {
            changed |= dump_factory_output(world, key, recipe.output.0);
        }
    }
    changed
}

/// Official Separator (Blocks.java v158.1): separator 193 and disassembler
/// 194. On each craft (progress >= craftTime) it consumes its inputs (slag
/// for 193; scrap + slag for 194), picks ONE weighted result at random with a
/// deterministic per-tile seed (SeparatorBuild.updateTile: Mathf.randomSeed(
/// seed++, 0, sum-1)), and offloads it if there is room. Progress advances
/// with getProgressIncrease(craftTime) = edelta()/craftTime (edelta == 1.0 on
/// the headless server, so per-craft liquid amounts are rate*tick * craftTime).
pub(crate) struct SeparatorSpec {
    pub(crate) craft_time: f32,
    pub(crate) results: &'static [(i16, i32)],
    pub(crate) liquid_input: (i16, f32),
    pub(crate) item_input: Option<i16>,
    pub(crate) item_capacity: i32,
}

pub(crate) fn separator_spec(block: i16) -> Option<SeparatorSpec> {
    match block {
        // consumeLiquid(slag, 4/60) per tick * 35t craft = 4/60*35 ≈ 2.3333.
        193 => Some(SeparatorSpec {
            craft_time: 35.0,
            results: &[(0, 5), (1, 3), (3, 2), (6, 2)], // copper, lead, graphite, titanium
            liquid_input: (1, (4.0 / 60.0) * 35.0),     // slag
            item_input: None,
            item_capacity: 10,
        }),
        // consumeLiquid(slag, 0.12) per tick * 15t craft = 1.8; consumeItem scrap.
        194 => Some(SeparatorSpec {
            craft_time: 15.0,
            results: &[(4, 2), (3, 1), (6, 1), (7, 1)], // sand, graphite, titanium, thorium
            liquid_input: (1, 0.12 * 15.0),             // slag
            item_input: Some(8),                        // scrap
            item_capacity: 20,
        }),
        _ => None,
    }
}

pub(crate) fn simulate_separators(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| separator_spec(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(spec) = separator_spec(snapshot.block) else {
            continue;
        };
        let has_liquid = snapshot.liquid_amount + 0.0001 >= spec.liquid_input.1
            && snapshot.stored_liquid == spec.liquid_input.0;
        let has_item = spec
            .item_input
            .is_none_or(|item| inventory_count(&snapshot.inventory, item) >= 1);
        if !has_liquid || !has_item {
            continue;
        }
        let efficiency = power.get(&key).copied().unwrap_or(0.0);
        if efficiency <= 0.0 {
            continue;
        }
        let crafted = if let Some(mut sep) = world.tiles.get_mut(&key) {
            sep.production_progress += delta_ticks * building_time_scale(world, key) * efficiency;
            if sep.production_progress >= spec.craft_time {
                sep.production_progress %= spec.craft_time;
                // consume slag (and scrap for the disassembler)
                sep.liquid_amount = (sep.liquid_amount - spec.liquid_input.1).max(0.0);
                if sep.liquid_amount <= 0.0001 {
                    sep.liquid_amount = 0.0;
                    sep.stored_liquid = -1;
                }
                if let Some(item) = spec.item_input {
                    let _ = inventory_remove(&mut sep.inventory, item, 1);
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        if crafted {
            // Weighted random pick with the official per-tile seed sequence.
            let sum: i32 = spec.results.iter().map(|(_, amount)| *amount).sum();
            let seed = world
                .tiles
                .get(&key)
                .map(|tile| tile.transport_progress as i64)
                .unwrap_or(0) as u64;
            let pick =
                ((seed.wrapping_mul(1103515245).wrapping_add(12345)) >> 16) % sum.max(1) as u64;
            let mut count = 0i32;
            let mut chosen: Option<i16> = None;
            for &(item, amount) in spec.results {
                if pick >= count as u64 && pick < (count + amount) as u64 {
                    chosen = Some(item);
                    break;
                }
                count += amount;
            }
            if let Some(item) = chosen {
                if let Some(mut sep) = world.tiles.get_mut(&key) {
                    sep.transport_progress = ((sep.transport_progress as i32) + 1) as f32;
                    if inventory_count(&sep.inventory, item) < spec.item_capacity {
                        inventory_add(&mut sep.inventory, item, 1);
                    }
                }
            }
            changed = true;
        }
    }
    changed
}

#[derive(Clone, Copy)]
pub(crate) struct UnitFactoryPlan {
    pub(crate) unit_type: i16,
    pub(crate) requirements: &'static [(i16, i32)],
    pub(crate) build_time: f32,
}

struct ProductionRecipe {
    block: i16,
    plan: i16,
    unit_type: i16,
    build_time: f32,
    liquid_id: i16,
    liquid_rate: f32,
    items: Vec<(i16, i32)>,
}

fn production_recipes() -> &'static [ProductionRecipe] {
    static RECIPES: std::sync::LazyLock<Vec<ProductionRecipe>> = std::sync::LazyLock::new(|| {
        include_str!("../../game/unit_production.tsv")
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let mut fields = line.split('\t');
                ProductionRecipe {
                    block: fields.next().unwrap().parse().unwrap(),
                    plan: fields.next().unwrap().parse().unwrap(),
                    unit_type: fields.next().unwrap().parse().unwrap(),
                    build_time: fields.next().unwrap().parse().unwrap(),
                    liquid_id: fields.next().unwrap().parse().unwrap(),
                    liquid_rate: fields.next().unwrap().parse().unwrap(),
                    items: fields
                        .map(|field| {
                            let (item, amount) = field.split_once(':').unwrap();
                            (item.parse().unwrap(), amount.parse().unwrap())
                        })
                        .collect(),
                }
            })
            .collect()
    });
    &RECIPES
}

pub(crate) fn unit_factory_recipe(block: i16, config: &[u8]) -> Option<UnitFactoryPlan> {
    let (plan, unit_type) =
        crate::network::wire::tile_config::resolved_unit_factory_plan(block, config)?;
    let recipe = production_recipes().iter().find(|recipe| {
        recipe.block == block && recipe.plan == plan && recipe.unit_type == unit_type
    })?;
    Some(UnitFactoryPlan {
        unit_type,
        requirements: &recipe.items,
        build_time: recipe.build_time,
    })
}

pub(crate) fn unit_factory_item_capacity(block: i16, item: i16) -> i32 {
    production_recipes()
        .iter()
        .filter(|recipe| recipe.block == block && recipe.plan >= 0)
        .flat_map(|recipe| &recipe.items)
        .filter(|(candidate, _)| *candidate == item)
        .map(|(_, amount)| amount.saturating_mul(2))
        .max()
        .unwrap_or(0)
}

/// Shared consumption gate for the power graph and client prediction. The
/// held payload is authoritative; `stored_amount` is only a legacy marker.
pub(crate) fn unit_block_consumption_ready(
    tile: &DynamicTile,
    rules: &WaveRules,
    tick: f32,
) -> bool {
    if !tile.enabled || !rules.activate_unit_factories(tile.team, tick) {
        return false;
    }
    let cost = rules.unit_cost_multiplier.max(0.0)
        * rules.team_rule(tile.team).unit_cost_multiplier.max(0.0);
    let has_items = |items: &[(i16, i32)]| {
        items.iter().all(|(item, amount)| {
            inventory_count(&tile.inventory, *item) >= (*amount as f32 * cost).round() as i32
        })
    };
    if let Some(recipe) = reconstructor_recipe(tile.block) {
        let Some(CarriedPayload::Unit(unit)) = tile.payload.as_deref() else {
            return false;
        };
        return reconstructor_upgrade(tile.block, unit.unit_type)
            .is_some_and(|output| !rules.unit_banned(output))
            && has_items(recipe.items)
            && (recipe.liquid_rate <= 0.0
                || cost == 0.0
                || (tile.stored_liquid == recipe.liquid_id && tile.liquid_amount > 0.000_001));
    }
    tile.payload.is_none()
        && unit_factory_recipe(tile.block, &tile.config)
            .is_some_and(|plan| !rules.unit_banned(plan.unit_type) && has_items(plan.requirements))
}

pub(crate) fn simulate_unit_factories(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, 377..=379 | 386..=388))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if !snapshot.enabled {
            continue;
        }
        if snapshot.payload.is_some() {
            // Official UnitFactoryBuild.updateTile: `shouldConsume` is false
            // while a payload is held, `progress = 0`, and `moveOutPayload`
            // runs every tick. Completion happens *after* moveOut, so a
            // payload created this tick does not slide until the next tick.
            if let Some(mut factory) = world.tiles.get_mut(&key) {
                factory.production_progress = 0.0;
            }
            changed |= move_out_unit_payload(world, out, key, delta_ticks);
            continue;
        }
        let Some(plan) = unit_factory_recipe(snapshot.block, &snapshot.config) else {
            continue;
        };
        let rules = world.wave_rules.read();
        let tick = *world.game_state.simulation_time.read();
        if !rules.activate_unit_factories(snapshot.team, tick) {
            continue;
        }
        if rules.unit_banned(plan.unit_type) {
            continue;
        }
        // A7: official UnitFactory consumes `Math.round(amount *
        // Rules.unitCost(team))` (ConsumeItems.trigger JAR offsets 23-52;
        // Rules.unitCost offsets 0-16 = unitCostMultiplier * TeamRule
        // .unitCostMultiplier; UnitFactory.lambda$initCapacities$6 wires it
        // as the Consume multiplier). The port models the TeamRule part; the
        // global Rules.unitCostMultiplier is not parsed (see report).
        let cost_multiplier = rules.unit_cost_multiplier.max(0.0)
            * rules.team_rule(snapshot.team).unit_cost_multiplier;
        let requirements: Vec<(i16, i32)> = plan
            .requirements
            .iter()
            .map(|(item, amount)| {
                (
                    *item,
                    (*amount as f32 * cost_multiplier.max(0.0)).round().max(0.0) as i32,
                )
            })
            .collect();
        if !requirements
            .iter()
            .all(|(item, amount)| inventory_count(&snapshot.inventory, *item) >= *amount)
        {
            continue;
        }
        let efficiency = power.get(&key).copied().unwrap_or(0.0);
        if efficiency <= 0.0 {
            continue;
        }
        // A7: official progress advances by `edelta() *
        // Rules.unitBuildSpeed(team)` (UnitFactory$UnitFactoryBuild.updateTile
        // JAR offsets 93-117; Rules.unitBuildSpeed offsets 0-16 =
        // unitBuildSpeedMultiplier * TeamRule.unitBuildSpeedMultiplier).
        let build_speed_multiplier = rules.unit_build_speed_multiplier.max(0.0)
            * rules.team_rule(snapshot.team).unit_build_speed_multiplier;
        // DashMap: snapshot overdrive before the factory get_mut.
        let time_scale = building_time_scale(world, key);
        changed = true;
        if let Some(mut factory) = world.tiles.get_mut(&key) {
            factory.production_progress +=
                delta_ticks * time_scale * efficiency * build_speed_multiplier.max(0.0);
            if factory.production_progress >= plan.build_time {
                // Official: `progress %= 1f`, then `payload = new UnitPayload`,
                // `payVector.setZero()`, consume. Cap is checked at dump,
                // not at create (UnitPayload.dump / Units.canCreate).
                factory.production_progress %= 1.0;
                for (item, amount) in &requirements {
                    if *amount <= 0 {
                        continue;
                    }
                    let removed = inventory_remove(&mut factory.inventory, *item, *amount);
                    debug_assert!(removed, "validated unit-factory inputs disappeared");
                }
                factory.payload_accum.clear();
                factory.production_progress =
                    factory.production_progress.clamp(0.0, plan.build_time);
                drop(factory);
                if let Some(unit) = create_unit_for_block(world, &snapshot, plan.unit_type) {
                    bind_factory_spawn_order(world, &snapshot, &unit);
                    if let Some(mut factory) = world.tiles.get_mut(&key) {
                        factory.payload = Some(Box::new(CarriedPayload::Unit(unit)));
                    }
                }
            }
        }
    }
    changed
}

fn create_unit_for_block(
    world: &DynamicWorld,
    factory: &DynamicTile,
    unit_type: i16,
) -> Option<EnemyUnit> {
    let spec = enemy_spec(unit_type)?;
    world.game_state.game_stats.write().units_created += 1;
    let id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
    let (center_x, center_y) = building_center(factory.position, factory.block);
    let angle = f32::from(factory.rotation) * 90.0;
    let mut unit = EnemyUnit {
        id,
        unit_type,
        entity_class: spec.entity_class,
        team: factory.team,
        x: center_x,
        y: center_y,
        health: spec.health,
        rotation: angle,
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
        missile_time: 0.0,
        status_agg: Default::default(),
        drown_progress: 0.0,
    };
    // P0-01: factory allies are born with their team's default controller
    // (CommandAI for player-commandable teams).
    unit.authority = crate::network::units::default_unit_authority(world, &unit);
    if crate::game::content::unit_movement(unit_type).flying {
        unit.elevation = 1.0;
    }
    Some(unit)
}

fn bind_factory_spawn_order(world: &DynamicWorld, factory: &DynamicTile, unit: &EnemyUnit) {
    if world.unit_orders.contains_key(&unit.id) {
        return;
    }
    let requested =
        configured_unit_command(factory).unwrap_or_else(|| default_unit_command(unit.unit_type));
    let command = if crate::game::unit_types::unit_type_allows_command(unit.unit_type, requested) {
        requested
    } else {
        default_unit_command(unit.unit_type)
    };
    let target = world.building_commands.get(&factory.position);
    world.unit_orders.insert(
        unit.id,
        UnitOrder {
            unit_id: unit.id,
            command,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 0,
            target_id: -1,
            target_x: target.as_ref().map(|target| target.target_x),
            target_y: target.as_ref().map(|target| target.target_y),
            logic_control: 0,
            queue: Vec::new(),
        },
    );
}

fn insert_released_unit(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    factory: &DynamicTile,
    unit: EnemyUnit,
) {
    bind_factory_spawn_order(world, factory, &unit);
    world.register_unit_group(unit.id);
    world.enemies.insert(unit.id, unit.clone());
    // dashmap-guard: allow DM900 reason="encode_unit_spawn_payload reads rules and payload fields only; it does not access building_commands"
    if let Ok(payload) = encode_unit_spawn_payload(world, &unit) {
        if let Ok(frame) = frame_generated_packet(UNIT_SPAWN_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
}

fn release_held_unit(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    factory: &DynamicTile,
    mut unit: EnemyUnit,
) {
    let (center_x, center_y) = building_center(factory.position, factory.block);
    let (px, py) = unit_block_pay_vector(factory);
    unit.x = center_x + px;
    unit.y = center_y + py;
    unit.rotation = factory.payload_rotation;
    insert_released_unit(world, out, factory, unit);
    // Official UnitBlock.dumpPayload: Call.unitBlockSpawn(tile) after a
    // successful dump. TypeIO.writeTile is the packed i32 position.
    if let Ok(frame) = encode_unit_block_spawn_frame(factory.position) {
        out.broadcast(frame);
    }
}

pub(crate) fn spawn_factory_unit(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    factory: &DynamicTile,
    unit_type: i16,
) {
    let Some(mut unit) = create_unit_for_block(world, factory, unit_type) else {
        return;
    };
    let (center_x, center_y) = building_center(factory.position, factory.block);
    let radians = (f32::from(factory.rotation) * 90.0).to_radians();
    unit.x = center_x + radians.cos() * 20.0;
    unit.y = center_y + radians.sin() * 20.0;
    insert_released_unit(world, out, factory, unit);
}

pub(crate) fn default_unit_command(unit_type: i16) -> u8 {
    match unit_type {
        20 => 4, // Mono: mine
        21 => 2, // Poly: rebuild
        22 => 1, // Mega: repair
        _ => 0,  // CommandAI treats null as move.
    }
}

/// TeamData.unitCap is the rules cap plus the sum of every live core's
/// modifier. Unlike the old port this applies equally to every team.
pub(crate) fn can_create_unit(world: &DynamicWorld, team: u8, unit_type: i16) -> bool {
    // Exact v159.7 Units.canCreate: `!type.useUnitCap ||
    // (countType(type) < getCap(team) && !type.isBanned())`. The short
    // circuit intentionally bypasses both cap and ban checks for types whose
    // canonical UnitType metadata has useUnitCap=false.
    if !crate::game::unit_types::unit_type_use_unit_cap(unit_type) {
        return true;
    }
    let cap = team_unit_cap(world, team);
    // DynamicWorld.enemies is the authoritative collection for simulated
    // non-player units. PlayerCombatState is player lifecycle state (its
    // status adapter reports core Alpha, type 35), not a per-unit TeamData
    // collection used by factory/spawn counts in this server model.
    let count = world
        .enemies
        .iter()
        .map(|unit| count_unit_type_with_payloads(&unit, team, unit_type))
        .sum::<usize>();
    count < usize::try_from(cap.max(0)).unwrap_or(usize::MAX)
        && !world.wave_rules.read().unit_banned(unit_type)
}

fn count_unit_type_with_payloads(unit: &EnemyUnit, team: u8, unit_type: i16) -> usize {
    // Teams.updateTeamStats counts units carried by live units recursively.
    // Building payloads remain outside the count until released.
    usize::from(unit.team == team && unit.unit_type == unit_type)
        + unit
            .payloads
            .iter()
            .map(|payload| match payload {
                CarriedPayload::Unit(carried) => {
                    count_unit_type_with_payloads(carried, team, unit_type)
                }
                CarriedPayload::Build(_) => 0,
            })
            .sum::<usize>()
}

pub(crate) fn core_unit_modifier(block: i16) -> i32 {
    match block {
        339 => 8,
        340 => 16,
        341 => 24,
        342..=344 => 15,
        _ => 0,
    }
}

pub(crate) fn team_unit_cap(world: &DynamicWorld, team: u8) -> i32 {
    if world.sharded_unit_cap == i32::MAX {
        return i32::MAX;
    }
    let rules = world.wave_rules.read();
    if rules.disable_unit_cap {
        return i32::MAX;
    }
    // Official Units.getCap: wave team is uncapped outside PvP.
    if team == rules.wave_team
        && *world.game_state.mode.read() != crate::state::game_state::GameMode::Pvp
    {
        return i32::MAX;
    }
    // Official Units.getCap: `unitCapVariable ? unitCap + team.unitCap : unitCap`.
    // Live cores of THIS team, not a cached load-time total (ASTRA F01).
    if !rules.unit_cap_variable {
        return rules.unit_cap.max(0);
    }
    let own = crate::network::world::team_core_snapshot(world, team)
        .iter()
        .map(|core| core_unit_modifier(core.block))
        .sum::<i32>();
    rules.unit_cap.saturating_add(own).max(0)
}

pub(crate) fn sharded_unit_cap(
    rules: &str,
    buildings: &[crate::engine::world_stream::NetworkBuilding],
) -> i32 {
    let parsed = crate::network::units::parse_wave_rules(rules);
    if parsed.disable_unit_cap {
        return i32::MAX;
    }
    let rule_cap = parsed.unit_cap.max(0);
    if !parsed.unit_cap_variable {
        return rule_cap;
    }
    let core_modifier = buildings
        .iter()
        .filter(|building| building.team == 1)
        .map(|building| match building.block {
            339 => 8,
            340 => 16,
            341 => 24,
            342..=344 => 15,
            _ => 0,
        })
        .sum::<i32>();
    rule_cap.saturating_add(core_modifier).max(0)
}

#[derive(Clone, Copy)]
pub(crate) struct ReconstructorRecipe {
    pub(crate) items: &'static [(i16, i32)],
    /// Consumed liquid id (Liquids.java load order: water 0, slag 1, oil 2,
    /// cryofluid 3, neoplasm 4, arkycite 5, gallium 6, ozone 7, hydrogen 8,
    /// nitrogen 9, cyanogen 10). Negative when the block consumes no liquid.
    pub(crate) liquid_id: i16,
    pub(crate) liquid_rate: f32,
    pub(crate) build_time: f32,
}

pub(crate) fn reconstructor_recipe(block: i16) -> Option<ReconstructorRecipe> {
    let recipe = production_recipes()
        .iter()
        .find(|recipe| recipe.block == block && recipe.plan == -1)?;
    Some(ReconstructorRecipe {
        items: &recipe.items,
        liquid_id: recipe.liquid_id,
        liquid_rate: recipe.liquid_rate,
        build_time: recipe.build_time,
    })
}

pub(crate) fn reconstructor_upgrade(block: i16, input: i16) -> Option<i16> {
    // Erekir refabricator upgrades (Blocks.java v158.1 `upgrades` rows).
    // prime-refabricator carries three upgrade pairs.
    let output = match (block, input) {
        (389, 38) => 39, // stell -> locus
        (390, 49) => 50, // elude -> avert
        (391, 43) => 44, // merui -> cleroi
        (392, 39) => 40, // locus -> precept
        (392, 44) => 45, // cleroi -> anthicus
        (392, 50) => 51, // avert -> obviate
        (380, 5) => 6,
        (380, 0) => 1,
        (380, 10) => 11,
        (380, 15) => 16,
        (380, 20) => 21,
        (380, 25) => 26,
        (380, 30) => 31,
        (381, 16) => 17,
        (381, 1) => 2,
        (381, 21) => 22,
        (381, 26) => 27,
        (381, 6) => 7,
        (381, 11) => 12,
        (381, 31) => 32,
        (382, 17) => 18,
        (382, 12) => 13,
        (382, 2) => 3,
        (382, 27) => 28,
        (382, 22) => 23,
        (382, 7) => 8,
        (382, 32) => 33,
        (383, 18) => 19,
        (383, 13) => 14,
        (383, 3) => 4,
        (383, 28) => 29,
        (383, 23) => 24,
        (383, 8) => 9,
        (383, 33) => 34,
        _ => return None,
    };
    Some(output)
}

pub(crate) fn reconstructor_item_capacity(block: i16, item: i16) -> i32 {
    reconstructor_recipe(block)
        .and_then(|recipe| {
            recipe
                .items
                .iter()
                .find(|(candidate, _)| *candidate == item)
                .map(|(_, amount)| amount.saturating_mul(2))
        })
        .unwrap_or(0)
}

/// Executes `UnitCommand.enterPayload` for the currently supported unit-payload
/// acceptors. Official CommandAI absorbs only when the unit is standing on the
/// building (`buildOn`) with command 5; movement is the ordinary order path.
/// `PayloadBlock.acceptPayload(self, …)` uses `relativeTo(self) == -1`, so the
/// unit-on-footprint path has no extra face check — `relativeTo` applies to
/// building-to-building dumps (`src/network/economy/transport.rs`).
pub(crate) fn simulate_unit_payload_entries(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    _delta_ticks: f32,
) -> bool {
    let candidates: Vec<_> = world
        .unit_orders
        .iter()
        .filter(|order| order.command == 5)
        .map(|order| {
            let target = if order.target_kind == 1 && order.target_id >= 0 {
                Some(order.target_id)
            } else if let (Some(x), Some(y)) = (order.target_x, order.target_y) {
                Some((((x / 8.0).floor() as i32) << 16) | ((y / 8.0).floor() as i32 & 0xffff))
            } else {
                None
            };
            (order.unit_id, target)
        })
        .collect();
    let mut changed = false;

    for (unit_id, position) in candidates {
        let Some(unit) = world.enemies.get(&unit_id).map(|unit| unit.clone()) else {
            continue;
        };
        let position = position.or_else(|| {
            let tile =
                ((unit.x / 8.0).floor() as i32) << 16 | ((unit.y / 8.0).floor() as i32 & 0xffff);
            Some(tile)
        });
        let Some(position) = position else {
            continue;
        };
        let Some(reconstructor) = dynamic_at(world, position)
            .and_then(|tile| world.tiles.get(&tile.position).map(|t| t.clone()))
        else {
            continue;
        };
        if unit.team != reconstructor.team
            || !reconstructor.enabled
            || reconstructor.payload.is_some()
            || reconstructor_recipe(reconstructor.block).is_none()
            || reconstructor_upgrade(reconstructor.block, unit.unit_type)
                .is_none_or(|output| world.wave_rules.read().unit_banned(output))
            || !unit_allowed_in_payloads(unit.unit_type)
            || unit_spawned_by_core(unit.unit_type)
            || !unit_on_building_footprint(world, &unit, &reconstructor)
        {
            continue;
        }

        if let Ok(frame) = encode_unit_entered_payload_frame(unit_id, reconstructor.position) {
            out.broadcast(frame);
        }
        world.enemies.remove(&unit_id);
        // Official `unitEnteredPayload` calls `unit.remove()`, which drops the
        // unit from `Groups.unit` too; every other despawn path pairs the two.
        // Leaving the id behind kept it in the snapshot order and in the unit
        // cap after it had become a payload.
        world.unregister_unit_group(unit_id);
        // P0-01: control associations die with the unit-as-payload.
        crate::network::units::detach_unit_control(world, unit_id);
        if let Some(mut live) = world.tiles.get_mut(&reconstructor.position) {
            let (cx, cy) = building_center(reconstructor.position, live.block);
            let half = unit_block_half(live.block);
            live.stored_amount = i32::from(unit.unit_type) + 1;
            live.production_progress = 0.0;
            live.payload_rotation = unit.rotation;
            set_pay_vector(
                &mut live,
                (unit.x - cx).clamp(-half, half),
                (unit.y - cy).clamp(-half, half),
            );
            live.payload = Some(Box::new(CarriedPayload::Unit(unit)));
        }
        changed = true;
    }
    changed
}

pub(crate) fn encode_unit_entered_payload_frame(
    unit_id: i32,
    position: i32,
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(9);
    payload.write_b(2)?; // TypeIO Unit: ordinary synced unit
    payload.write_i(unit_id)?;
    payload.write_i(position)?;
    frame_generated_packet(UNIT_ENTERED_PAYLOAD_PACKET_ID, &payload, false)
}

/// `UnitBlockSpawnCallPacket` (id 146): TypeIO.writeTile — packed i32 position.
/// Official `Call.unitBlockSpawn(tile)` after `UnitBlock.dumpPayload`.
pub(crate) fn encode_unit_block_spawn_frame(position: i32) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(4);
    payload.write_i(position)?;
    frame_generated_packet(UNIT_BLOCK_SPAWN_PACKET_ID, &payload, false)
}

pub(crate) fn simulate_reconstructors(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| reconstructor_recipe(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if !snapshot.enabled {
            continue;
        }
        let Some(recipe) = reconstructor_recipe(snapshot.block) else {
            continue;
        };
        let rules = world.wave_rules.read();
        let tick = *world.game_state.simulation_time.read();
        if !rules.activate_unit_factories(snapshot.team, tick) {
            continue;
        }
        let Some(CarriedPayload::Unit(held)) = snapshot.payload.as_deref() else {
            continue;
        };
        let input_type = held.unit_type;
        let Some(output_type) = reconstructor_upgrade(snapshot.block, input_type) else {
            // Official: no remaining upgrade → `moveOutPayload` every tick.
            changed |= move_out_unit_payload(world, out, key, delta_ticks);
            continue;
        };
        if rules.unit_banned(output_type) {
            changed |= move_out_unit_payload(world, out, key, delta_ticks);
            continue;
        }
        let cost_multiplier = rules.unit_cost_multiplier.max(0.0)
            * rules.team_rule(snapshot.team).unit_cost_multiplier;
        let requirements: Vec<(i16, i32)> = recipe
            .items
            .iter()
            .map(|(item, amount)| {
                (
                    *item,
                    (*amount as f32 * cost_multiplier.max(0.0)).round().max(0.0) as i32,
                )
            })
            .collect();
        let has_items = requirements
            .iter()
            .all(|(item, amount)| inventory_count(&snapshot.inventory, *item) >= *amount);
        let efficiency = power.get(&key).copied().unwrap_or(0.0);
        let build_speed_multiplier = rules.unit_build_speed_for(snapshot.team).max(0.0);
        let time_scale = building_time_scale(world, key);
        let edelta = delta_ticks.max(0.0) * time_scale * efficiency.max(0.0);
        let consume_scale = edelta * cost_multiplier.max(0.0);
        let have_liquid = if recipe.liquid_rate <= 0.0 {
            f32::MAX
        } else if snapshot.stored_liquid == recipe.liquid_id {
            snapshot.liquid_amount
        } else {
            0.0
        };
        let liquid_need = recipe.liquid_rate * consume_scale;
        // ASTRA F07: ConsumeLiquid.efficiency is the available fraction, not a
        // boolean "enough for a full tick".
        let liquid_frac = if recipe.liquid_rate <= 0.0 || liquid_need <= 1e-9 {
            1.0
        } else {
            (have_liquid / liquid_need).clamp(0.0, 1.0)
        };
        let arrived = if let Some(mut reconstructor) = world.tiles.get_mut(&key) {
            apply_move_in(&mut reconstructor, delta_ticks.max(0.0))
        } else {
            false
        };
        changed = true;
        if !arrived {
            continue;
        }
        if !has_items || liquid_frac <= 0.0 || efficiency <= 0.0 {
            continue;
        }
        if let Some(mut reconstructor) = world.tiles.get_mut(&key) {
            let progress_delta =
                delta_ticks * time_scale * efficiency * liquid_frac * build_speed_multiplier;
            reconstructor.production_progress += progress_delta;
            if recipe.liquid_rate > 0.0 {
                reconstructor.liquid_amount = (reconstructor.liquid_amount
                    - recipe.liquid_rate * consume_scale * liquid_frac)
                    .max(0.0);
                if reconstructor.liquid_amount <= 0.0001 {
                    reconstructor.liquid_amount = 0.0;
                    reconstructor.stored_liquid = -1;
                }
            }
            if reconstructor.production_progress >= recipe.build_time {
                // Official: replace `payload.unit`, `progress %= 1f`, consume.
                // The upgraded type has no further upgrade, so later ticks
                // take the moveOut branch. Cap is checked at dump.
                reconstructor.production_progress %= 1.0;
                for (item, amount) in &requirements {
                    let removed = inventory_remove(&mut reconstructor.inventory, *item, *amount);
                    debug_assert!(removed, "validated reconstructor inputs disappeared");
                }
                drop(reconstructor);
                if let Some(unit) = create_unit_for_block(world, &snapshot, output_type) {
                    bind_factory_spawn_order(world, &snapshot, &unit);
                    if let Some(mut reconstructor) = world.tiles.get_mut(&key) {
                        reconstructor.payload = Some(Box::new(CarriedPayload::Unit(unit)));
                        reconstructor.stored_amount = i32::from(output_type) + 1;
                    }
                }
            }
        }
    }
    changed
}

pub(crate) fn encode_unit_despawn_frame_legacy(unit_id: i32) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;

    let mut payload = Vec::with_capacity(5);
    payload.write_b(2)?;
    payload.write_i(unit_id)?;
    frame_generated_packet(UNIT_DESPAWN_PACKET_ID, &payload, false)
}

#[derive(Clone, Copy)]
pub(crate) struct LiquidFactoryRecipe {
    pub(crate) item_inputs: &'static [(i16, i32)],
    pub(crate) item_output: Option<(i16, i32)>,
    pub(crate) liquid_input: (i16, f32),
    pub(crate) liquid_output: Option<(i16, f32)>,
    pub(crate) craft_time: f32,
    pub(crate) item_capacity: i32,
}

pub(crate) fn liquid_factory_recipe(block: i16) -> Option<LiquidFactoryRecipe> {
    match block {
        182 => Some(LiquidFactoryRecipe {
            item_inputs: &[(5, 3)],
            item_output: Some((3, 2)),
            liquid_input: (0, 3.0),
            liquid_output: None,
            craft_time: 30.0,
            item_capacity: 20,
        }),
        186 => Some(LiquidFactoryRecipe {
            item_inputs: &[(6, 2)],
            item_output: Some((10, 1)),
            liquid_input: (2, 15.0),
            liquid_output: None,
            craft_time: 60.0,
            item_capacity: 10,
        }),
        189 => Some(LiquidFactoryRecipe {
            item_inputs: &[(6, 1)],
            item_output: None,
            liquid_input: (0, 24.0),
            liquid_output: Some((3, 24.0)),
            craft_time: 120.0,
            item_capacity: 10,
        }),
        330 => Some(LiquidFactoryRecipe {
            item_inputs: &[],
            item_output: Some((13, 1)),
            // consumeLiquid(water, 18/60) = 0.3/tick continuous * 100t craft.
            liquid_input: (0, (18.0 / 60.0) * 100.0),
            liquid_output: None,
            craft_time: 100.0,
            item_capacity: 10,
        }),
        // Melter: scrap -> slag. Official outputLiquid = 12/60 per tick
        // CONTINUOUS (GenericCrafterBuild.updateTile: handleLiquid(amount*inc)
        // every tick), so the per-craft amount is rate*tick * craftTime =
        // (12/60)*10 = 2.0, not 0.2.
        192 => Some(LiquidFactoryRecipe {
            item_inputs: &[(8, 1)],
            item_output: None,
            liquid_input: (0, 0.0),
            liquid_output: Some((1, (12.0 / 60.0) * 10.0)),
            craft_time: 10.0,
            item_capacity: 10,
        }),
        // Spore press: spore pod -> oil (18/60 per tick * 20t = 6.0 per craft).
        195 => Some(LiquidFactoryRecipe {
            item_inputs: &[(13, 1)],
            item_output: None,
            liquid_input: (0, 0.0),
            liquid_output: Some((2, (18.0 / 60.0) * 20.0)),
            craft_time: 20.0,
            item_capacity: 10,
        }),
        // Coal centrifuge: oil -> coal. consumeLiquid(oil, 0.1) consumes
        // 0.1/tick CONTINUOUS = 0.1*30 = 3.0 per 30-tick craft.
        197 => Some(LiquidFactoryRecipe {
            item_inputs: &[],
            item_output: Some((5, 1)),
            liquid_input: (2, 0.1 * 30.0),
            liquid_output: None,
            craft_time: 30.0,
            item_capacity: 10,
        }),
        // Slag centrifuge: sand + slag -> gallium. consumeLiquid(slag, 40/60)
        // = 40/60 per tick * 120t = 80.0; outputLiquid(gallium, 1/60) =
        // 1/60 per tick * 120t = 2.0.
        211 => Some(LiquidFactoryRecipe {
            item_inputs: &[(4, 1)],
            item_output: None,
            liquid_input: (1, (40.0 / 60.0) * 120.0),
            liquid_output: Some((5, (1.0 / 60.0) * 120.0)),
            craft_time: 120.0,
            item_capacity: 10,
        }),
        // Oil extractor (Fracker): sand + water -> oil. Official pumpAmount
        // 0.25/tick continuous, itemUseTime 60 (1 sand per 60 ticks),
        // consumeLiquid water 0.15. Per-craft (60s): oil 0.25*60 = 15.0,
        // water 0.15*60 = 9.0.
        331 => Some(LiquidFactoryRecipe {
            item_inputs: &[(4, 1)],
            item_output: None,
            liquid_input: (0, 0.15 * 60.0),
            liquid_output: Some((2, 0.25 * 60.0)),
            craft_time: 60.0,
            item_capacity: 10,
        }),
        _ => None,
    }
}

pub(crate) fn simulate_liquid_factories(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| liquid_factory_recipe(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(recipe) = liquid_factory_recipe(snapshot.block) else {
            continue;
        };
        let has_items = recipe
            .item_inputs
            .iter()
            .all(|(item, amount)| inventory_count(&snapshot.inventory, *item) >= *amount);
        let has_liquid = recipe.liquid_input.1 <= 0.0
            || (snapshot.stored_liquid == recipe.liquid_input.0
                && snapshot.liquid_amount + 0.0001 >= recipe.liquid_input.1);
        let output_fits = recipe.item_output.is_none_or(|(_, amount)| {
            inventory_total(&snapshot.inventory)
                - recipe
                    .item_inputs
                    .iter()
                    .map(|(_, amount)| *amount)
                    .sum::<i32>()
                + amount
                <= recipe.item_capacity
        }) && recipe.liquid_output.is_none_or(|(_, amount)| {
            snapshot.output_liquid_amount + amount <= liquid_capacity(snapshot.block).unwrap_or(0.0)
        });
        if !has_items || !has_liquid || !output_fits {
            if let Some((item, _)) = recipe.item_output {
                changed |= dump_factory_output(world, key, item);
            }
            continue;
        }
        let efficiency = power.get(&key).copied().unwrap_or(1.0);
        if efficiency <= 0.0 {
            continue;
        }
        let boost = attribute_crafter_boost(world, snapshot.block, snapshot.position);
        let time_scale = building_time_scale(world, key);
        let crafted = if let Some(mut factory) = world.tiles.get_mut(&key) {
            factory.production_progress += delta_ticks * time_scale * efficiency * boost;
            if factory.production_progress >= recipe.craft_time {
                factory.production_progress %= recipe.craft_time;
                for (item, amount) in recipe.item_inputs {
                    let removed = inventory_remove(&mut factory.inventory, *item, *amount);
                    debug_assert!(removed);
                }
                factory.liquid_amount = (factory.liquid_amount - recipe.liquid_input.1).max(0.0);
                if factory.liquid_amount <= 0.0001 {
                    factory.liquid_amount = 0.0;
                    factory.stored_liquid = -1;
                }
                if let Some((item, amount)) = recipe.item_output {
                    inventory_add(&mut factory.inventory, item, amount);
                }
                if let Some((_, amount)) = recipe.liquid_output {
                    factory.output_liquid_amount += amount;
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        changed = true;
        if crafted {
            if let Some((item, _)) = recipe.item_output {
                changed |= dump_factory_output(world, key, item);
            }
        }
    }
    changed
}

pub(crate) fn dump_factory_output(world: &DynamicWorld, key: i32, item: i16) -> bool {
    let Some(factory) = world.tiles.get(&key).map(|tile| tile.clone()) else {
        return false;
    };
    if inventory_count(&factory.inventory, item) <= 0 {
        return false;
    }
    let mut targets = Vec::new();
    for position in &factory.occupied {
        for rotation in 0..4 {
            let target = offset_position(*position, rotation);
            if !factory.occupied.contains(&target) && !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    if !targets
        .into_iter()
        .any(|target| accept_logistics_item_from(world, target, item, Some(key), 0))
    {
        return false;
    }
    world
        .tiles
        .get_mut(&key)
        .is_some_and(|mut tile| inventory_remove(&mut tile.inventory, item, 1))
}

#[derive(Clone, Copy)]
pub(crate) struct MenderSpec {
    pub(crate) reload: f32,
    pub(crate) range: f32,
    pub(crate) heal_percent: f32,
    pub(crate) booster_item: i16,
    pub(crate) phase_boost: f32,
    pub(crate) phase_range_boost: f32,
    pub(crate) use_time: f32,
}
