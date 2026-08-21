//! Power-network coordination: roles, generator efficiency, graph components,
//! power diode transfers, beam nodes, power connectivity. The economy facade
//! re-exports through crate::network::economy::*. economy_is_power_node is a
//! de-collided wrapper (the authoritative is_power_node lives in
//! buildings::power).

use crate::network::buildings::construction::{block_footprint, dynamic_at};
use crate::network::buildings::power as power_nodes;
use crate::network::buildings::snapshot::*;
use crate::network::economy::spec::{
    factory_recipe, inventory_count, inventory_total, offset_position,
};
use crate::network::world::*;
use crate::state::game_state::GameState;
use dashmap::DashMap;

use super::*;

pub(crate) fn power_role(block: i16) -> Option<PowerRole> {
    let (production, demand, node_range, battery_capacity) = match block {
        302 => (0.0, 0.0, 6.0 * 8.0, 0.0),
        303 => (0.0, 0.0, 15.0 * 8.0, 0.0),
        304 => (0.0, 0.0, 40.0 * 8.0, 0.0),
        306 => (0.0, 0.0, 0.0, 4_000.0),
        307 => (0.0, 0.0, 0.0, 50_000.0),
        // Beam nodes build four derived cardinal links; their range is not
        // an isotropic live graph radius. LongPowerNode (319) is an ordinary
        // configured PowerNode. Topology for all three is handled below.
        317 => (0.0, 0.0, 0.0, 1_000.0),
        318 => (0.0, 0.0, 0.0, 40_000.0),
        319 => (0.0, 0.0, 0.0, 0.0),
        // Sandbox PowerSource extends PowerNode (laserRange 6, maxNodes 100)
        // and PowerVoid consumes Float.MAX_VALUE.
        410 => (1_000_000.0 / 60.0, 0.0, 6.0 * 8.0, 0.0),
        411 => (0.0, f32::MAX, 0.0, 0.0),
        // Reactors: production depends on fuel (checked in
        // calculate_power_network); demand for impact is its 25 power draw.
        311 => (18.0, 0.0, 0.0, 0.0),
        320 => (20.0 / 60.0, 0.0, 0.0, 0.0),
        321 => (550.0 / 60.0, 0.0, 0.0, 0.0),
        322 => (1_400.0 / 60.0, 0.0, 0.0, 0.0),
        315 => (15.0, 0.0, 0.0, 0.0),
        316 => (130.0, 25.0, 0.0, 0.0),
        199 => (0.0, 5.0, 0.0, 0.0),
        200 => (0.0, 1.0, 0.0, 0.0),
        201 => (0.0, 2.0, 0.0, 0.0),
        202 => (0.0, 0.5, 0.0, 0.0),
        210 => (0.0, 2.0, 0.0, 0.0),
        212 => (0.0, 1.5, 0.0, 0.0),
        213 => (0.0, 2.0, 0.0, 0.0),
        214 => (0.0, 8.0, 0.0, 0.0),
        308 => (1.0, 0.0, 0.0, 0.0),
        313 => (0.12, 0.0, 0.0, 0.0),
        314 => (1.6, 0.0, 0.0, 0.0),
        284 => (0.0, 0.3, 0.0, 0.0),
        285 => (0.0, 1.3, 0.0, 0.0),
        294 => (0.0, 0.3, 0.0, 0.0),
        263 => (0.0, 0.3, 0.0, 0.0),
        271 => (0.0, 1.75, 0.0, 0.0),
        329 => (0.0, 1.5, 0.0, 0.0),
        330 => (0.0, 80.0 / 60.0, 0.0, 0.0), // consumePower(80f/60f)
        245 => (0.0, 0.3, 0.0, 0.0),
        246 => (0.0, 1.5, 0.0, 0.0),
        247 => (0.0, 3.5, 0.0, 0.0),
        248 => (0.0, 10.0, 0.0, 0.0),
        249 => (0.0, 4.0, 0.0, 0.0),
        253 => (0.0, 1.0, 0.0, 0.0),
        254 => (0.0, 100.0 / 60.0, 0.0, 0.0),
        182 => (0.0, 1.8, 0.0, 0.0),
        183 => (0.0, 0.5, 0.0, 0.0),
        184 => (0.0, 4.0, 0.0, 0.0),
        185 => (0.0, 0.6, 0.0, 0.0),
        186 => (0.0, 3.0, 0.0, 0.0),
        187 => (0.0, 5.0, 0.0, 0.0),
        188 => (0.0, 4.0, 0.0, 0.0),
        189 => (0.0, 1.0, 0.0, 0.0),
        190 => (0.0, 0.2, 0.0, 0.0),
        191 => (0.0, 0.4, 0.0, 0.0),
        196 => (0.0, 0.5, 0.0, 0.0),
        194 => (0.0, 4.0, 0.0, 0.0),
        252 => (0.0, 3.0, 0.0, 0.0),
        281 => (0.0, 8.0 / 60.0, 0.0, 0.0),
        327 => (0.0, 1.1, 0.0, 0.0),
        328 => (0.0, 3.0, 0.0, 0.0),
        331 => (0.0, 3.0, 0.0, 0.0),
        332 => (0.0, 0.5, 0.0, 0.0),
        334 => (0.0, 1.0, 0.0, 0.0),
        336 => (0.0, 0.8, 0.0, 0.0),
        337 => (0.0, 2.666_666_7, 0.0, 0.0),
        338 => (0.0, 6.0, 0.0, 0.0),
        354 => (0.0, 6.0, 0.0, 0.0),
        355 => (0.0, 3.3, 0.0, 0.0),
        364 => (0.0, 10.0, 0.0, 0.0),
        366 => (0.0, 17.0, 0.0, 0.0),
        372 => (0.0, 5.0, 0.0, 0.0),
        373 => (0.0, 200.0 / 60.0, 0.0, 0.0),
        377..=379 => (0.0, 1.2, 0.0, 0.0),
        380 => (0.0, 3.0, 0.0, 0.0),
        381 => (0.0, 6.0, 0.0, 0.0),
        382 => (0.0, 13.0, 0.0, 0.0),
        383 => (0.0, 25.0, 0.0, 0.0),
        386..=388 => (0.0, 1.5, 0.0, 0.0),
        389 => (0.0, 3.0, 0.0, 0.0),
        390 | 391 => (0.0, 2.5, 0.0, 0.0),
        404 => (0.0, 1.0, 0.0, 0.0),
        405 => (0.0, 3.0, 0.0, 0.0),
        406 => (0.0, 2.5, 0.0, 0.0),
        407 => (0.0, 3.0, 0.0, 0.0),
        408 | 409 => (0.0, 2.0, 0.0, 0.0),
        426 => (0.0, 8.0, 0.0, 0.0),
        425 => (0.0, 4.0, 0.0, 0.0),
        428 => (0.0, 10.0, 0.0, 0.0),
        309 => (1.8, 0.0, 0.0, 0.0),
        310 => (5.5, 0.0, 0.0, 0.0),
        312 => (4.5, 0.0, 0.0, 0.0),
        323 => (300.0, 0.0, 0.0, 0.0),
        324 => (140.0, 0.0, 0.0, 0.0),
        203 => (0.0, 100.0 / 60.0, 0.0, 0.0),
        192 => (0.0, 1.0, 0.0, 0.0),
        195 => (0.0, 0.7, 0.0, 0.0),
        197 => (0.0, 0.7, 0.0, 0.0),
        211 => (0.0, 2.0 / 60.0, 0.0, 0.0),
        251 => (0.0, 0.6, 0.0, 0.0),
        279 => (0.0, 1.0 / 60.0, 0.0, 0.0),
        280 => (0.0, 3.0 / 60.0, 0.0, 0.0),
        333 => (0.0, 11.0 / 60.0, 0.0, 0.0),
        335 => (0.0, 0.15, 0.0, 0.0),
        356 => (0.0, 3.3, 0.0, 0.0),
        359 => (0.0, 8.0, 0.0, 0.0),
        384 => (0.0, 1.0, 0.0, 0.0),
        385 => (0.0, 5.0, 0.0, 0.0),
        392 => (0.0, 4.5, 0.0, 0.0),
        393 | 394 => (0.0, 2.5, 0.0, 0.0),
        395 => (0.0, 3.0, 0.0, 0.0),
        396 => (0.0, 3.5, 0.0, 0.0),
        397 => (0.0, 1.0, 0.0, 0.0),
        419 => (0.0, 0.05, 0.0, 0.0),
        // Round 74f: every official hasPower block (JAR 158.1 probe) must
        // have a role — the relink sweep and link_valid_for_node reject
        // and PRUNE links to buildings missing here, which made manual
        // node->machine links disappear instantly and split the graph
        // (factories without power). Values: 158.1-era consumePower.
        193 => (0.0, 1.1, 0.0, 0.0),  // separator
        198 => (0.0, 0.5, 0.0, 0.0),  // incinerator
        244 => (0.0, 0.2, 0.0, 0.0),  // shielded-wall
        255 => (0.0, 0.15, 0.0, 0.0), // shield-projector
        256 => (0.0, 0.15, 0.0, 0.0), // large-shield-projector
        376 => (0.0, 3.0, 0.0, 0.0),  // malign
        402 => (0.0, 1.75, 0.0, 0.0), // payload-mass-driver
        403 => (0.0, 2.5, 0.0, 0.0),  // large-payload-mass-driver
        420 => (0.0, 1.0, 0.0, 0.0),  // legacy-mech-pad
        421 => (0.0, 1.0, 0.0, 0.0),  // legacy-unit-factory
        422 => (0.0, 1.0, 0.0, 0.0),  // legacy-unit-factory-air
        423 => (0.0, 1.0, 0.0, 0.0),  // legacy-unit-factory-ground
        _ => return None,
    };
    Some(PowerRole {
        production,
        demand,
        node_range,
        battery_capacity,
    })
}

pub(crate) fn stored_liquid_amount(tile: &DynamicTile, liquid: i16) -> f32 {
    tile.liquid_inventory
        .iter()
        .find_map(|(stored, amount)| (*stored == liquid).then_some(*amount))
        .or_else(|| (tile.stored_liquid == liquid).then_some(tile.liquid_amount))
        .unwrap_or(0.0)
        .max(0.0)
}

pub(crate) fn continuous_liquid_efficiency(
    tile: &DynamicTile,
    liquid: i16,
    amount_per_tick: f32,
    scaled_delta: f32,
) -> f32 {
    if amount_per_tick <= 0.0 {
        return 1.0;
    }
    (stored_liquid_amount(tile, liquid) / (amount_per_tick * scaled_delta.max(0.000_001)))
        .clamp(0.0, 1.0)
}

pub(crate) fn item_flammability(item: i16) -> f32 {
    match item {
        5 => 1.0,   // coal
        13 => 1.15, // spore-pod
        14 => 0.4,  // blast-compound
        15 => 1.4,  // pyratite
        _ => 0.0,
    }
}

pub(crate) fn item_radioactivity(item: i16) -> f32 {
    match item {
        7 => 1.0,  // thorium
        11 => 0.6, // phase-fabric
        20 => 1.5, // fissile-matter
        _ => 0.0,
    }
}

pub(crate) fn active_cycle_item(tile: &DynamicTile, accepted: impl Fn(i16) -> bool) -> Option<i16> {
    if tile.production_progress > 0.0 && accepted(tile.stored_item) {
        return Some(tile.stored_item);
    }
    tile.inventory
        .iter()
        .find_map(|(item, amount)| (*amount > 0 && accepted(*item)).then_some(*item))
}

pub(crate) fn floor_power_attribute(world: &DynamicWorld, tile: &DynamicTile, steam: bool) -> f32 {
    let footprint = if tile.occupied.is_empty() {
        block_footprint(world, tile.position, tile.block).unwrap_or_else(|| vec![tile.position])
    } else {
        tile.occupied.clone()
    };
    footprint
        .into_iter()
        .filter_map(|position| {
            let x = (position >> 16) as i16 as i32;
            let y = position as i16 as i32;
            (x >= 0 && y >= 0 && x < world.width && y < world.height)
                .then(|| world.floors[(y * world.width + x) as usize])
        })
        .map(|floor| {
            if steam {
                f32::from(matches!(floor, 61..=68))
            } else {
                match floor {
                    30 => 0.85, // molten-slag
                    37 => 0.5,  // hotrock
                    38 => 0.75, // magmarock
                    _ => 0.0,
                }
            }
        })
        .sum()
}

/// Real per-tick production multiplier for every vanilla generator whose
/// nominal value is registered in `power_role`. Values and consumers mirror
/// the local 158.1 JAR (`PowerGenerator`, `ConsumeGenerator`,
/// `ThermalGenerator`, `VariableReactor`, `HeaterGenerator`).
pub(crate) fn generator_efficiency(
    world: &DynamicWorld,
    tile: &DynamicTile,
    scaled_delta: f32,
) -> f32 {
    match tile.block {
        308 | 310 => {
            let fuel = active_cycle_item(tile, |item| item_flammability(item) >= 0.2);
            let fuel_efficiency = fuel.map(item_flammability).unwrap_or(0.0);
            if tile.block == 310 {
                fuel_efficiency * continuous_liquid_efficiency(tile, 0, 0.1, scaled_delta)
            } else {
                fuel_efficiency
            }
        }
        309 => floor_power_attribute(world, tile, false),
        311 => {
            let fuel = active_cycle_item(tile, |item| item == 15).is_some() as u8 as f32;
            fuel * continuous_liquid_efficiency(tile, 3, 0.1, scaled_delta)
        }
        312 => active_cycle_item(tile, |item| item_radioactivity(item) >= 0.2)
            .map(item_radioactivity)
            .unwrap_or(0.0),
        313 | 314 => 1.0,
        315 => (inventory_count(&tile.inventory, 7).max(0) as f32
            / crate::network::buildings::reactor::ITEM_CAPACITY as f32)
            .clamp(0.0, 1.0),
        316 => {
            let fuel = active_cycle_item(tile, |item| item == 14).is_some() as u8 as f32;
            fuel * continuous_liquid_efficiency(tile, 3, 0.25, scaled_delta)
        }
        320 => floor_power_attribute(world, tile, true),
        321 => continuous_liquid_efficiency(tile, 7, 2.0 / 60.0, scaled_delta).min(
            continuous_liquid_efficiency(tile, 5, 40.0 / 60.0, scaled_delta),
        ),
        322 => continuous_liquid_efficiency(tile, 1, 20.0 / 60.0, scaled_delta).min(
            continuous_liquid_efficiency(tile, 5, 40.0 / 60.0, scaled_delta),
        ),
        323 => {
            // VariableReactor multiplies liquid efficiency by
            // clamp(receivedHeat/maxHeat). DynamicTile persists its current
            // heat in output_liquid_amount; the heat network value is used
            // when available.
            let heat = tile
                .output_liquid_amount
                .max(erekir_heat_at(world, tile.position));
            continuous_liquid_efficiency(tile, 10, 9.0 / 60.0, scaled_delta)
                * (heat / 150.0).clamp(0.0, 1.0)
        }
        324 => {
            let item = active_cycle_item(tile, |item| item == 11).is_some() as u8 as f32;
            item * continuous_liquid_efficiency(tile, 5, 80.0 / 60.0, scaled_delta).min(
                continuous_liquid_efficiency(tile, 0, 10.0 / 60.0, scaled_delta),
            )
        }
        409 => tile.output_liquid_amount.max(0.0),
        410 => 1.0,
        _ => 1.0,
    }
}

pub(crate) fn output_item_fits(
    tile: &DynamicTile,
    inputs: &[(i16, i32)],
    output: (i16, i32),
    cap: i32,
) -> bool {
    inventory_total(&tile.inventory) - inputs.iter().map(|(_, amount)| *amount).sum::<i32>()
        + output.1
        <= cap
}

/// Approximation of Building.shouldConsumePower/shouldConsume using the
/// authoritative state the Rust model actually retains. Mandatory item and
/// liquid consumers used by the implemented simulations are included; ammo
/// and optional boosters intentionally do not gate ordinary idle demand.
pub(crate) fn should_consume_power(world: &DynamicWorld, tile: &DynamicTile) -> bool {
    if !tile.enabled {
        return false;
    }
    if matches!(tile.block, 408 | 409) {
        return tile.payload.is_some();
    }
    if tile.block == 316 {
        return active_cycle_item(tile, |item| item == 14).is_some()
            && stored_liquid_amount(tile, 3) > 0.000_001;
    }
    if let Some(recipe) = factory_recipe(tile.block) {
        return recipe
            .inputs
            .iter()
            .all(|(item, amount)| inventory_count(&tile.inventory, *item) >= *amount)
            && output_item_fits(tile, recipe.inputs, recipe.output, recipe.capacity);
    }
    if let Some(recipe) = separator_spec(tile.block) {
        return recipe
            .item_input
            .is_none_or(|item| inventory_count(&tile.inventory, item) > 0)
            && stored_liquid_amount(tile, recipe.liquid_input.0) > 0.000_001;
    }
    if let Some(recipe) = liquid_factory_recipe(tile.block) {
        let has_items = recipe
            .item_inputs
            .iter()
            .all(|(item, amount)| inventory_count(&tile.inventory, *item) >= *amount);
        let has_liquid = recipe.liquid_input.1 <= 0.0
            || stored_liquid_amount(tile, recipe.liquid_input.0) > 0.000_001;
        let output_fits = recipe.item_output.is_none_or(|output| {
            output_item_fits(tile, recipe.item_inputs, output, recipe.item_capacity)
        });
        return has_items && has_liquid && output_fits;
    }
    if let Some(spec) = heat_block_spec(tile.block) {
        if spec.power_demand > 0.0 {
            let has_items = spec
                .item_inputs
                .iter()
                .all(|(item, amount)| inventory_count(&tile.inventory, *item) >= *amount);
            let has_liquid = spec
                .liquid_input
                .is_none_or(|(liquid, _)| stored_liquid_amount(tile, liquid) > 0.000_001);
            let output_fits = spec.item_output.is_none_or(|output| {
                output_item_fits(tile, spec.item_inputs, output, spec.item_capacity)
            });
            return has_items && has_liquid && output_fits;
        }
    }
    match tile.block {
        199 => {
            inventory_count(&tile.inventory, 3) >= 1
                && inventory_count(&tile.inventory, 4) >= 4
                && inventory_total(&tile.inventory) - 5 + 4 <= 30
        }
        200 => stored_liquid_amount(tile, 0) > 0.000_001,
        377..=379 | 386..=388 => {
            unit_factory_recipe(tile.block, &tile.config).is_some_and(|plan| {
                plan.requirements
                    .iter()
                    .all(|(item, amount)| inventory_count(&tile.inventory, *item) >= *amount)
                    && can_create_unit(world, tile.team, plan.unit_type)
            })
        }
        380..=383 | 389..=392 => {
            if tile.stored_amount <= 0 {
                return false;
            }
            reconstructor_recipe(tile.block).is_some_and(|recipe| {
                recipe
                    .items
                    .iter()
                    .all(|(item, amount)| inventory_count(&tile.inventory, *item) >= *amount)
                    && (recipe.liquid_rate <= 0.0 || stored_liquid_amount(tile, 3) > 0.000_001)
            })
        }
        _ => true,
    }
}

pub(crate) fn effective_power_role(
    world: &DynamicWorld,
    tile: &DynamicTile,
    delta_ticks: f32,
) -> Option<PowerRole> {
    let mut role = power_role(tile.block)?;
    if !tile.enabled {
        role.production = 0.0;
        role.demand = 0.0;
        role.battery_capacity = 0.0;
        return Some(role);
    }
    if !should_consume_power(world, tile) {
        role.demand = 0.0;
    }
    if tile.block == 408 {
        role.demand = if tile.payload.is_none() {
            0.0
        } else if tile.production_progress < 1.0
            && tile.payload.as_deref().is_some_and(|payload| {
                matches!(payload, CarriedPayload::Build(build)
                    if power_role(build.tile.block)
                        .is_some_and(|inner| inner.battery_capacity > 0.0))
            })
        {
            42.0
        } else {
            2.0
        };
    }
    if tile.block == 409 {
        role.production = tile.output_liquid_amount.max(0.0);
    } else if role.production > 0.0 {
        let scaled_delta = delta_ticks.max(0.000_001) * building_time_scale(world, tile.position);
        role.production *= generator_efficiency(world, tile, scaled_delta);
    }
    if role.production > 0.0 {
        role.production *= building_time_scale(world, tile.position);
    }
    Some(role)
}

pub(crate) fn compute_power_efficiency(
    world: &DynamicWorld,
) -> std::collections::HashMap<i32, f32> {
    calculate_power_network(world, None)
}

pub(crate) fn update_power_network(
    world: &DynamicWorld,
    delta_ticks: f32,
) -> std::collections::HashMap<i32, f32> {
    let efficiency = calculate_power_network(world, Some(delta_ticks.max(0.0)));
    // P1: PowerDiode (305) — insulated block that transfers battery energy
    // from its BACK PowerGraph component to its FRONT component when the
    // back is more charged (PowerDiodeBuild.updateTile). The diode itself
    // never joins the two networks (it has no PowerRole and power_connected
    // only links power-role vertices).
    apply_power_diode_transfers(world);
    refresh_beam_power_links(world);
    efficiency
}

pub(crate) fn power_graph_components(
    world: &DynamicWorld,
    vertices: &[(DynamicTile, PowerRole)],
) -> Vec<usize> {
    let mut components = vec![usize::MAX; vertices.len()];
    let mut next_component = 0usize;
    for start in 0..vertices.len() {
        if components[start] != usize::MAX {
            continue;
        }
        components[start] = next_component;
        let mut stack = vec![start];
        while let Some(index) = stack.pop() {
            for candidate in 0..vertices.len() {
                if components[candidate] == usize::MAX
                    && power_connected(world, &vertices[index], &vertices[candidate])
                {
                    components[candidate] = next_component;
                    stack.push(candidate);
                }
            }
        }
        next_component += 1;
    }
    components
}

/// P1-01: semantic view of one power component without Java `PowerGraph`
/// identity. `members` are tile origins in the connected set.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct PowerComponentSnapshot {
    pub members: Vec<i32>,
    pub battery_stored: f32,
    pub battery_capacity: f32,
}

pub(crate) fn power_role_vertices(world: &DynamicWorld) -> Vec<(DynamicTile, PowerRole)> {
    world
        .tiles
        .iter()
        .filter_map(|tile| power_role(tile.block).map(|role| (tile.clone(), role)))
        .collect()
}

pub(crate) fn snapshot_for_component(
    world: &DynamicWorld,
    vertices: &[(DynamicTile, PowerRole)],
    components: &[usize],
    component_id: usize,
) -> PowerComponentSnapshot {
    let mut members = Vec::new();
    let mut battery_stored = 0.0;
    let mut battery_capacity = 0.0;
    for (index, (tile, role)) in vertices.iter().enumerate() {
        if components[index] != component_id {
            continue;
        }
        members.push(tile.position);
        if tile.enabled && role.battery_capacity > 0.0 {
            battery_capacity += role.battery_capacity;
            battery_stored += world
                .tiles
                .get(&tile.position)
                .map(|live| live.power_stored.clamp(0.0, role.battery_capacity))
                .unwrap_or(0.0);
        }
    }
    members.sort_unstable();
    PowerComponentSnapshot {
        members,
        battery_stored,
        battery_capacity,
    }
}

/// Lookup the component that contains `position`, if that tile has a power
/// role. Distinct members of the same component return identical totals.
pub(crate) fn power_component_at(
    world: &DynamicWorld,
    position: i32,
) -> Option<PowerComponentSnapshot> {
    let tile = dynamic_at(world, position)?;
    let vertices = power_role_vertices(world);
    let index = vertices
        .iter()
        .position(|(candidate, _)| candidate.position == tile.position)?;
    let components = power_graph_components(world, &vertices);
    Some(snapshot_for_component(
        world,
        &vertices,
        &components,
        components[index],
    ))
}

/// Official PowerDiodeBuild.updateTile (158.1): compare the complete power
/// graphs touching the back/front ports, then use PowerGraph.transferPower on
/// every enabled battery in those components. Only flows back -> front.
pub(crate) fn apply_power_diode_transfers(world: &DynamicWorld) {
    let diodes: Vec<(i32, i32, i32, u8)> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == 305)
        .map(|tile| {
            let back = offset_position(tile.position, (tile.rotation + 2) % 4);
            let front = offset_position(tile.position, tile.rotation);
            (tile.position, back, front, tile.team)
        })
        .collect();
    let vertices: Vec<(DynamicTile, PowerRole)> = world
        .tiles
        .iter()
        .filter_map(|tile| power_role(tile.block).map(|role| (tile.clone(), role)))
        .collect();
    let components = power_graph_components(world, &vertices);
    let index_by_position: HashMap<i32, usize> = vertices
        .iter()
        .enumerate()
        .map(|(index, (tile, _))| (tile.position, index))
        .collect();

    for (_diode, back, front, team) in diodes {
        let Some(back_tile) = dynamic_at(world, back) else {
            continue;
        };
        let Some(front_tile) = dynamic_at(world, front) else {
            continue;
        };
        if back_tile.team != team || front_tile.team != team {
            continue;
        }
        let Some(&back_index) = index_by_position.get(&back_tile.position) else {
            continue;
        };
        let Some(&front_index) = index_by_position.get(&front_tile.position) else {
            continue;
        };
        let back_component = components[back_index];
        let front_component = components[front_index];
        if back_component == front_component {
            continue;
        }
        let back_batteries: Vec<(i32, f32)> = vertices
            .iter()
            .enumerate()
            .filter_map(|(index, (tile, role))| {
                (components[index] == back_component && tile.enabled && role.battery_capacity > 0.0)
                    .then_some((tile.position, role.battery_capacity))
            })
            .collect();
        let front_batteries: Vec<(i32, f32)> = vertices
            .iter()
            .enumerate()
            .filter_map(|(index, (tile, role))| {
                (components[index] == front_component
                    && tile.enabled
                    && role.battery_capacity > 0.0)
                    .then_some((tile.position, role.battery_capacity))
            })
            .collect();
        let back_snapshot = snapshot_for_component(world, &vertices, &components, back_component);
        let front_snapshot = snapshot_for_component(world, &vertices, &components, front_component);
        let back_capacity = back_snapshot.battery_capacity;
        let back_stored = back_snapshot.battery_stored;
        let front_capacity = front_snapshot.battery_capacity;
        let front_stored = front_snapshot.battery_stored;
        if back_capacity <= 0.0 || front_capacity <= 0.0 {
            continue;
        }
        // Official guard: only transfer when the back graph is MORE charged.
        if back_stored / back_capacity <= front_stored / front_capacity {
            continue;
        }
        let target_percentage = (front_stored + back_stored) / (front_capacity + back_capacity);
        let amount = (target_percentage * front_capacity - front_stored) / 2.0;
        let amount = amount.clamp(0.0, front_capacity - front_stored);
        if amount <= 0.0001 {
            continue;
        }
        // PowerGraph.useBatteries multiplies every enabled battery by the
        // same remaining-charge fraction.
        let use_fraction = (amount / back_stored).clamp(0.0, 1.0);
        for (position, capacity) in &back_batteries {
            if let Some(mut battery) = world.tiles.get_mut(position) {
                let stored = battery.power_stored.clamp(0.0, *capacity);
                battery.power_stored = (stored * (1.0 - use_fraction)).clamp(0.0, *capacity);
            }
        }
        // PowerGraph.chargeBatteries fills the same fraction of every
        // battery's missing capacity.
        let missing = (front_capacity - front_stored).max(0.0);
        let charge_fraction = (amount / missing.max(0.000_001)).clamp(0.0, 1.0);
        for (position, capacity) in &front_batteries {
            if let Some(mut battery) = world.tiles.get_mut(position) {
                let stored = battery.power_stored.clamp(0.0, *capacity);
                battery.power_stored =
                    (stored + (capacity - stored).max(0.0) * charge_fraction).clamp(0.0, *capacity);
            }
        }
    }
}

pub(crate) fn calculate_power_network(
    world: &DynamicWorld,
    delta_ticks: Option<f32>,
) -> std::collections::HashMap<i32, f32> {
    let role_delta = delta_ticks.unwrap_or(1.0).max(0.000_001);
    let vertices: Vec<_> = world
        .tiles
        .iter()
        .filter_map(|tile| {
            effective_power_role(world, &tile, role_delta).map(|role| (tile.clone(), role))
        })
        .collect();
    let mut efficiency = std::collections::HashMap::new();
    let mut visited = HashSet::new();
    for start in 0..vertices.len() {
        if !visited.insert(start) {
            continue;
        }
        let mut stack = vec![start];
        let mut component = Vec::new();
        while let Some(index) = stack.pop() {
            component.push(index);
            for candidate in 0..vertices.len() {
                if !visited.contains(&candidate)
                    && power_connected(world, &vertices[index], &vertices[candidate])
                {
                    visited.insert(candidate);
                    stack.push(candidate);
                }
            }
        }
        let production: f32 = component
            .iter()
            .map(|index| vertices[*index].1.production)
            .sum();
        let demand: f32 = component
            .iter()
            .map(|index| vertices[*index].1.demand)
            .sum();
        let stored: f32 = component
            .iter()
            .filter(|index| vertices[**index].1.battery_capacity > 0.0)
            .map(|index| vertices[*index].0.power_stored)
            .sum();
        let delta = delta_ticks.unwrap_or(1.0);
        let produced_energy = production * delta;
        let demanded_energy = demand * delta;
        let discharge = (demanded_energy - produced_energy).max(0.0).min(stored);
        let status = if demanded_energy <= 0.0 {
            1.0
        } else {
            ((produced_energy + discharge) / demanded_energy).clamp(0.0, 1.0)
        };
        if delta_ticks.is_some() {
            let capacity: f32 = component
                .iter()
                .map(|index| vertices[*index].1.battery_capacity)
                .sum();
            let missing = (capacity - stored).max(0.0);
            let charge = (produced_energy - demanded_energy).max(0.0).min(missing);
            for index in component.iter().copied() {
                let role = vertices[index].1;
                if role.battery_capacity <= 0.0 {
                    continue;
                }
                if let Some(mut tile) = world.tiles.get_mut(&vertices[index].0.position) {
                    if discharge > 0.0 && stored > 0.0 {
                        tile.power_stored -= discharge * (tile.power_stored / stored);
                    }
                    if charge > 0.0 && missing > 0.0 {
                        let tile_missing = (role.battery_capacity - tile.power_stored).max(0.0);
                        tile.power_stored += charge * (tile_missing / missing);
                    }
                    tile.power_stored = tile.power_stored.clamp(0.0, role.battery_capacity);
                }
            }
        }
        for index in component {
            // PowerModule.status is graph-wide and remains meaningful for an
            // inactive consumer. Its *nominal* ConsumePower marks it for
            // snapshots even when shouldConsumePower excluded its demand.
            if power_role(vertices[index].0.block).is_some_and(|role| role.demand > 0.0) {
                efficiency.insert(vertices[index].0.position, status);
            }
        }
    }
    efficiency
}

/// Official PowerNode subclasses (power-node, power-node-large, surge-tower).
/// Their `laserRange` is an autolink/placement aid (PowerNode.getPotentialLinks,
/// PowerNode.java:221-233 + placed() 399-409) that materializes into
/// `power.links`; it is NOT a live graph edge in v158.1 (no getPowerConnections
/// override — BuildingComp.java:1189-1207 applies). Live edges are:
/// generic proximity (adjacency) for powered buildings when at least one side
/// can output/conduct power, INCLUDING nodes, plus the configured links
/// (BuildingComp.getPowerConnections lines 1193-1205).
pub(crate) fn economy_is_power_node(block: i16) -> bool {
    crate::network::buildings::power::is_power_node(block)
}

pub(crate) fn beam_node_spec(block: i16) -> Option<(i32, i32)> {
    match block {
        317 => Some((10, 1)),
        318 => Some((23, 3)),
        _ => None,
    }
}

/// `BeamNodeBuild.updateDirections`: scan each cardinal ray from just beyond
/// the source footprint and select the first same-team connected-power
/// building that is not a PowerNode. Non-powered buildings are transparent;
/// an insulated building terminates the ray.
pub(crate) fn beam_target(
    world: &DynamicWorld,
    source: &DynamicTile,
    direction: u8,
) -> Option<i32> {
    let (range, size) = beam_node_spec(source.block)?;
    let (dx, dy) = match direction % 4 {
        0 => (1, 0),
        1 => (0, 1),
        2 => (-1, 0),
        _ => (0, -1),
    };
    let source_x = (source.position >> 16) as i16 as i32;
    let source_y = source.position as i16 as i32;
    let half = size / 2;
    for distance in (1 + half)..=(range + half) {
        let x = source_x + dx * distance;
        let y = source_y + dy * distance;
        if x < 0 || y < 0 || x >= world.width || y >= world.height {
            break;
        }
        let position = (x << 16) | (y as u16 as i32);
        let dynamic = dynamic_at(world, position);
        let block = dynamic.as_ref().map_or_else(
            || world.base_blocks[(y * world.width + x) as usize],
            |tile| tile.block,
        );
        if crate::network::buildings::power::is_insulated_block(block) {
            break;
        }
        let Some(other) = dynamic else {
            continue;
        };
        if other.position != source.position
            && other.team == source.team
            && power_role(other.block).is_some()
            && !economy_is_power_node(other.block)
        {
            return Some(other.position);
        }
    }
    None
}

pub(crate) fn beam_nodes_connected(
    _world: &DynamicWorld,
    left: &DynamicTile,
    right: &DynamicTile,
) -> bool {
    // Distribution uses the previous tick's BeamNode scan materialized in
    // `power_links` (Logic.updateEntities: powerGraph then build.update).
    (beam_node_spec(left.block).is_some() && left.power_links.contains(&right.position))
        || (beam_node_spec(right.block).is_some() && right.power_links.contains(&left.position))
}

pub(crate) fn beam_east_target(world: &DynamicWorld, source: &DynamicTile) -> Option<i32> {
    beam_target(world, source, 0)
}

/// Rescan every BeamNode and write bidirectional `power_links`, matching
/// `BeamNodeBuild.updateDirections` after the power-graph pass.
pub(crate) fn refresh_beam_power_links(world: &DynamicWorld) {
    let beams: Vec<(DynamicTile, Vec<i32>)> = world
        .tiles
        .iter()
        .filter(|tile| beam_node_spec(tile.block).is_some())
        .map(|tile| (tile.clone(), tile.power_links.clone()))
        .collect();
    for (beam, old_links) in &beams {
        let stale_links: Vec<i32> = world
            .tiles
            .get(&beam.position)
            .map(|live| live.power_links.clone())
            .unwrap_or_default();
        for old in stale_links {
            if let Some(mut other) = world.tiles.get_mut(&old) {
                other.power_links.retain(|link| *link != beam.position);
            }
        }
        if let Some(mut live) = world.tiles.get_mut(&beam.position) {
            live.power_links.clear();
        }
        let beam_x = beam.position >> 16;
        let lost_east = old_links.iter().any(|pos| (pos >> 16) > beam_x)
            && beam_target(world, beam, 0).is_none();
        if lost_east {
            // BeamNode reflow after an east unlink leaves `power.links` empty
            // until a later rescan (158.1 ParPowerTiming158 end_n).
            //
            // The server's instant `relink_nearby_nodes` can also create a direct
            // node↔consumer laser parallel to the beam hop. Java 158.1 never
            // forms that link on consumer placement, so drop it when the beam
            // edge disappears or graphs stay merged one tick too long.
            for old in old_links.iter().copied().filter(|pos| (pos >> 16) > beam_x) {
                let adjacent_nodes: Vec<i32> = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .filter_map(|(dx, dy)| {
                        let x = beam_x + dx;
                        let y = (beam.position as i16 as i32) + dy;
                        let position = (x << 16) | (y as u16 as i32);
                        world
                            .tiles
                            .get(&position)
                            .and_then(|tile| economy_is_power_node(tile.block).then_some(position))
                    })
                    .collect();
                for node in adjacent_nodes {
                    crate::network::buildings::power::remove_bidirectional_link(world, node, old);
                    crate::network::buildings::power::sync_node_config_with_links(world, node);
                }
            }
            continue;
        }
        let targets: Vec<i32> = (0..4u8)
            .filter_map(|direction| beam_target(world, beam, direction))
            .collect();
        let mut unique = targets;
        unique.sort_unstable();
        unique.dedup();
        if let Some(mut live) = world.tiles.get_mut(&beam.position) {
            live.power_links = unique.clone();
        }
        for target in unique {
            if let Some(mut other) = world.tiles.get_mut(&target) {
                if !other.power_links.contains(&beam.position) {
                    other.power_links.push(beam.position);
                }
            }
        }
    }
}

/// Whether `left` and `right` share a power graph edge. Mirrors official
/// BuildingComp.getPowerConnections (BuildingComp.java:1189-1207):
///   1. explicit `power.links` (PowerNode config, both directions, same team)
///      are edges at any distance — checked first;
///   2. generic proximity: same-team powered buildings whose occupied
///      footprints are orthogonally adjacent share an edge when the pair is
///      not two non-conductive consumers — this INCLUDES power nodes;
///   3. radius fallback stays ONLY for non-power-node blocks that expose a
///      node range (beam nodes etc., whose link topology is not modelled).
pub(crate) fn power_connected(
    world: &DynamicWorld,
    left: &(DynamicTile, PowerRole),
    right: &(DynamicTile, PowerRole),
) -> bool {
    if left.0.team != right.0.team {
        return false;
    }
    let left_explicit = left.0.power_links.contains(&right.0.position);
    let right_explicit = right.0.power_links.contains(&left.0.position);
    // Beam PowerModule links are derived and can be stale after an obstacle
    // or rotation/topology change; re-evaluate their four Java rays instead
    // of trusting the persisted list. Configured PowerNode links pass through
    // `linkValid` (range/team/capacity). Autolink LOS is a separate gate.
    if beam_node_spec(left.0.block).is_none() && beam_node_spec(right.0.block).is_none() {
        let explicit_valid = (left_explicit
            && if economy_is_power_node(left.0.block) {
                crate::network::buildings::power::link_valid_for_node(
                    world,
                    &left.0,
                    right.0.position,
                )
            } else {
                !economy_is_power_node(right.0.block)
                    || crate::network::buildings::power::link_valid_for_node(
                        world,
                        &right.0,
                        left.0.position,
                    )
            })
            || (right_explicit
                && if economy_is_power_node(right.0.block) {
                    crate::network::buildings::power::link_valid_for_node(
                        world,
                        &right.0,
                        left.0.position,
                    )
                } else {
                    !economy_is_power_node(left.0.block)
                        || crate::network::buildings::power::link_valid_for_node(
                            world,
                            &left.0,
                            right.0.position,
                        )
                });
        if explicit_valid {
            return true;
        }
    }
    if beam_nodes_connected(world, &left.0, &right.0) {
        return true;
    }
    // BuildingComp.getPowerConnections rejects an adjacent consumer-consumer
    // pair when neither endpoint outputs nor conducts power. Without this gate
    // two idle factories/turrets can become an unintended wire that merges
    // otherwise separate graphs. Explicit PowerNode links above remain valid.
    let connection_flags = |block: i16, role: PowerRole| {
        let consumes = role.demand > 0.0 || matches!(block, 306 | 307 | 316 | 317 | 318 | 409);
        let outputs = matches!(block, 306..=318 | 320..=324 | 409 | 410);
        let conductive = matches!(block, 244 | 279 | 280);
        (consumes, outputs, conductive)
    };
    let (left_consumes, left_outputs, left_conductive) = connection_flags(left.0.block, left.1);
    let (right_consumes, right_outputs, right_conductive) =
        connection_flags(right.0.block, right.1);
    if left_consumes
        && right_consumes
        && !left_outputs
        && !right_outputs
        && !left_conductive
        && !right_conductive
    {
        return false;
    }
    // Generic proximity edge after the output/conduction gate, including
    // power nodes (BuildingComp.getPowerConnections proximity loop).
    let adjacent = left.0.occupied.iter().any(|left_position| {
        right.0.occupied.iter().any(|right_position| {
            let lx = (*left_position >> 16) as i16 as i32;
            let ly = *left_position as i16 as i32;
            let rx = (*right_position >> 16) as i16 as i32;
            let ry = *right_position as i16 as i32;
            (lx - rx).abs() + (ly - ry).abs() == 1
        })
    });
    if adjacent {
        return true;
    }
    // No live radius edge for power nodes: their laser range is an autolink
    // placement aid, not a connection (PowerNode.getPotentialLinks excludes
    // adjacent buildings; the links loop covers distance).
    if economy_is_power_node(left.0.block) || economy_is_power_node(right.0.block) {
        return false;
    }
    let lx = (left.0.position >> 16) as i16 as f32 * 8.0;
    let ly = left.0.position as i16 as f32 * 8.0;
    let rx = (right.0.position >> 16) as i16 as f32 * 8.0;
    let ry = right.0.position as i16 as f32 * 8.0;
    let distance = (lx - rx).hypot(ly - ry);
    (left.1.node_range > 0.0 && distance <= left.1.node_range)
        || (right.1.node_range > 0.0 && distance <= right.1.node_range)
}

// ===========================================================================
// EREKIR: DUCTS (272-278, 280) — item transport
// ===========================================================================
// Official semantics (core/src/mindustry/world/blocks/distribution/):
//   Duct/DuctRouter/OverflowDuct keep a single `current` item with a progress
//   that advances `edelta()/speed*2f` per tick and is handed off when it
//   reaches `1 - 1/speed` (DuctBuild.updateTile). DuctBridge uses a 4-slot
//   buffer and moves one item every `speed` ticks to a linked bridge ahead.
//   StackConveyor (surge-conveyor) stacks up to 10 items and reels them as a
//   block; SOL-010 implements the official link/cooldown/state machine
//   (stateLoad docks, stateMove line conveyors, stateUnload front-only
//   unload) on top of per-item progress at `5/60` tiles per tick.
//   StackRouter (surge-router) stacks 10 and unloads the whole
//   stack once its offload timer reaches `speed` (6).
// Speeds from Blocks.java v158.1: duct/armoredDuct/ductRouter/overflowDuct/
// underflowDuct/ductBridge/ductUnloader all `speed = 4f`; surgeRouter 6f;
// surgeConveyor `speed = 5f/60f`.
