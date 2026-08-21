//! Factory simulation: separators, unit factories, reconstructors, liquid
//! factories, unit caps. The economy facade re-exports through
//! crate::network::economy::*.

use crate::network::buildings::snapshot::*;
use crate::network::combat::unit_combat::effective_unit_speed;
use crate::network::economy::spec::{
    accept_logistics_item_from, factory_recipe, inventory_add, inventory_count, inventory_remove,
    inventory_total, offset_position,
};
use crate::network::units::*;
use crate::network::wire::encode::{encode_unit_spawn_payload, frame_generated_packet};
use crate::network::wire::tile_config::{configured_unit_command, unit_factory_plan};
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

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
                let crafted = if let Some(mut factory) = world.tiles.get_mut(&key) {
                    factory.production_progress +=
                        delta_ticks * building_time_scale(world, key) * efficiency;
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

pub(crate) fn unit_factory_recipe(block: i16, config: &[u8]) -> Option<UnitFactoryPlan> {
    // Erekir fabricators (386-388) are configurable=false with one fixed plan
    // (Blocks.java: tankFabricator -> stell, shipFabricator -> elude,
    // mechFabricator -> merui).
    let (plan, unit_type) = match block {
        386..=388 => (0i16, [38, 49, 43][(block - 386) as usize]),
        _ => unit_factory_plan(block, config).or_else(|| {
            // Official UnitFactoryBuild.created() selects the first unlocked plan.
            unit_factory_plan(block, &[1, 0, 0, 0, 0])
        })?,
    };
    let recipe = match block {
        377 => [
            UnitFactoryPlan {
                unit_type: 0,
                requirements: &[(9, 10), (1, 10)],
                build_time: 900.0,
            },
            UnitFactoryPlan {
                unit_type: 10,
                requirements: &[(9, 8), (5, 10)],
                build_time: 600.0,
            },
            UnitFactoryPlan {
                unit_type: 5,
                requirements: &[(9, 30), (1, 20), (6, 20)],
                build_time: 2_400.0,
            },
        ]
        .get(usize::try_from(plan).ok()?)
        .copied(),
        378 => [
            UnitFactoryPlan {
                unit_type: 15,
                requirements: &[(9, 15)],
                build_time: 900.0,
            },
            UnitFactoryPlan {
                unit_type: 20,
                requirements: &[(9, 30), (1, 15)],
                build_time: 2_100.0,
            },
        ]
        .get(usize::try_from(plan).ok()?)
        .copied(),
        379 => [
            UnitFactoryPlan {
                unit_type: 25,
                requirements: &[(9, 20), (3, 35)],
                build_time: 2_700.0,
            },
            UnitFactoryPlan {
                unit_type: 30,
                requirements: &[(9, 15), (6, 20)],
                build_time: 2_100.0,
            },
        ]
        .get(usize::try_from(plan).ok()?)
        .copied(),
        386 => [UnitFactoryPlan {
            unit_type: 38, // stell
            requirements: &[(16, 40), (9, 50)],
            build_time: 2_100.0, // 60f * 35f
        }]
        .get(usize::try_from(plan).ok()?)
        .copied(),
        387 => [UnitFactoryPlan {
            unit_type: 49, // elude
            requirements: &[(3, 50), (9, 70)],
            build_time: 2_400.0, // 60f * 40f
        }]
        .get(usize::try_from(plan).ok()?)
        .copied(),
        388 => [UnitFactoryPlan {
            unit_type: 43, // merui
            requirements: &[(16, 50), (9, 70)],
            build_time: 2_400.0, // 60f * 40f
        }]
        .get(usize::try_from(plan).ok()?)
        .copied(),
        _ => None,
    }?;
    (recipe.unit_type == unit_type).then_some(recipe)
}

pub(crate) fn unit_factory_item_capacity(block: i16, item: i16) -> i32 {
    match (block, item) {
        (377, 9) => 60,
        (377, 1 | 6) => 40,
        (377, 5) => 20,
        (378, 9) => 60,
        (378, 1) => 30,
        (379, 9 | 6) => 40,
        (379, 3) => 70,
        // Erekir fabricators: per-item capacity = amount * 2 (initCapacities).
        (386, 16) => 80,
        (386, 9) => 100,
        (387, 3) => 100,
        (387, 9) => 140,
        (388, 16) => 100,
        (388, 9) => 140,
        _ => 0,
    }
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
        let Some(plan) = unit_factory_recipe(snapshot.block, &snapshot.config) else {
            continue;
        };
        let rules = world.wave_rules.read();
        let tick = *world.game_state.simulation_time.read();
        if !rules.activate_unit_factories(snapshot.team, tick) {
            continue;
        }
        // A7: official UnitFactory consumes `Math.round(amount *
        // Rules.unitCost(team))` (ConsumeItems.trigger JAR offsets 23-52;
        // Rules.unitCost offsets 0-16 = unitCostMultiplier * TeamRule
        // .unitCostMultiplier; UnitFactory.lambda$initCapacities$6 wires it
        // as the Consume multiplier). The port models the TeamRule part; the
        // global Rules.unitCostMultiplier is not parsed (see report).
        let cost_multiplier = rules.team_rule(snapshot.team).unit_cost_multiplier;
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
        if !can_create_unit(world, snapshot.team, plan.unit_type) {
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
        // The port models the TeamRule part; the global
        // Rules.unitBuildSpeedMultiplier is not parsed (see report).
        let build_speed_multiplier = rules.team_rule(snapshot.team).unit_build_speed_multiplier;
        let completed = if let Some(mut factory) = world.tiles.get_mut(&key) {
            factory.production_progress += delta_ticks
                * building_time_scale(world, key)
                * efficiency
                * build_speed_multiplier.max(0.0);
            if factory.production_progress >= plan.build_time {
                factory.production_progress %= plan.build_time;
                for (item, amount) in requirements {
                    let removed = inventory_remove(&mut factory.inventory, item, amount);
                    debug_assert!(removed, "validated unit-factory inputs disappeared");
                }
                true
            } else {
                false
            }
        } else {
            false
        };
        changed = true;
        if completed {
            spawn_factory_unit(world, out, &snapshot, plan.unit_type);
        }
    }
    changed
}

pub(crate) fn spawn_factory_unit(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    factory: &DynamicTile,
    unit_type: i16,
) {
    let Some(spec) = enemy_spec(unit_type) else {
        return;
    };
    world.game_state.game_stats.write().units_created += 1;
    let id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
    let center_x = (factory.position >> 16) as i16 as f32 * 8.0;
    let center_y = factory.position as i16 as f32 * 8.0;
    let angle = f32::from(factory.rotation) * 90.0;
    let radians = angle.to_radians();
    let mut unit = EnemyUnit {
        id,
        unit_type,
        entity_class: spec.entity_class,
        team: factory.team,
        x: center_x + radians.cos() * 20.0,
        y: center_y + radians.sin() * 20.0,
        rotation: angle,
        health: spec.health
            * world.wave_rules.read().unit_health_multiplier
            * world
                .wave_rules
                .read()
                .team_rule(factory.team)
                .unit_health_multiplier,
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
    // P0-01: factory allies are born with their team's default controller
    // (CommandAI for player-commandable teams).
    unit.authority = crate::network::units::default_unit_authority(world, &unit);
    world.register_unit_group(id);
    world.enemies.insert(id, unit.clone());
    let command =
        configured_unit_command(factory).unwrap_or_else(|| default_unit_command(unit_type));
    let target = world.building_commands.get(&factory.position);
    world.unit_orders.insert(
        id,
        UnitOrder {
            unit_id: id,
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
    // dashmap-guard: allow DM900 reason="encode_unit_spawn_payload reads rules and payload fields only; it does not access building_commands"
    if let Ok(payload) = encode_unit_spawn_payload(world, &unit) {
        if let Ok(frame) = frame_generated_packet(UNIT_SPAWN_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
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
        .filter(|unit| unit.team == team && unit.unit_type == unit_type)
        .count();
    count < usize::try_from(cap.max(0)).unwrap_or(usize::MAX)
        && !world.wave_rules.read().unit_banned(unit_type)
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
    let legacy = crate::network::world::team_core_snapshot(world, 1)
        .iter()
        .map(|core| core_unit_modifier(core.block))
        .sum::<i32>();
    let base = world.sharded_unit_cap.saturating_sub(legacy).max(0);
    let own = crate::network::world::team_core_snapshot(world, team)
        .iter()
        .map(|core| core_unit_modifier(core.block))
        .sum::<i32>();
    base.saturating_add(own).max(0)
}

pub(crate) fn sharded_unit_cap(
    rules: &str,
    buildings: &[crate::engine::world_stream::NetworkBuilding],
) -> i32 {
    let parsed = serde_json::from_str::<serde_json::Value>(rules).unwrap_or_default();
    if parsed
        .get("disableUnitCap")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return i32::MAX;
    }
    let rule_cap = parsed
        .get("unitCap")
        .and_then(serde_json::Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(0)
        .max(0);
    if !parsed
        .get("unitCapVariable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
    {
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
    pub(crate) liquid_rate: f32,
    pub(crate) build_time: f32,
}

pub(crate) fn reconstructor_recipe(block: i16) -> Option<ReconstructorRecipe> {
    match block {
        380 => Some(ReconstructorRecipe {
            items: &[(9, 40), (4, 40)],
            liquid_rate: 0.0,
            build_time: 600.0,
        }),
        381 => Some(ReconstructorRecipe {
            items: &[(9, 130), (6, 80), (3, 40)],
            liquid_rate: 0.0,
            build_time: 1_800.0,
        }),
        382 => Some(ReconstructorRecipe {
            items: &[(9, 850), (6, 750), (10, 650)],
            liquid_rate: 1.0,
            build_time: 5_400.0,
        }),
        383 => Some(ReconstructorRecipe {
            items: &[(9, 1_000), (10, 600), (12, 500), (11, 350)],
            liquid_rate: 3.0,
            build_time: 14_400.0,
        }),
        _ => None,
    }
}

pub(crate) fn reconstructor_upgrade(block: i16, input: i16) -> Option<i16> {
    let output = match (block, input) {
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
/// acceptors. Unlike the old proximity shortcut, a unit is only consumed after
/// it has been explicitly ordered onto the matching reconstructor.
pub(crate) fn simulate_unit_payload_entries(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let candidates: Vec<_> = world
        .unit_orders
        .iter()
        .filter(|order| order.command == 5 && order.target_kind == 1)
        .map(|order| (order.unit_id, order.target_id))
        .collect();
    let mut changed = false;

    for (unit_id, position) in candidates {
        let Some(unit) = world.enemies.get(&unit_id).map(|unit| unit.clone()) else {
            continue;
        };
        let Some(reconstructor) = world.tiles.get(&position).map(|tile| tile.clone()) else {
            continue;
        };
        if unit.team != reconstructor.team
            || reconstructor_recipe(reconstructor.block).is_none()
            || reconstructor.stored_amount != 0
            || reconstructor_upgrade(reconstructor.block, unit.unit_type).is_none()
        {
            continue;
        }

        let target_x = (position >> 16) as i16 as f32 * 8.0;
        let target_y = position as i16 as f32 * 8.0;
        let dx = target_x - unit.x;
        let dy = target_y - unit.y;
        let distance = dx.hypot(dy);
        let acceptance_distance =
            f32::from(crate::game::content::block_size(reconstructor.block)) * 4.0;
        if distance > acceptance_distance.max(1.0) {
            if let Some(mut live) = world.enemies.get_mut(&unit_id) {
                let speed = effective_unit_speed(&live);
                let step = (speed * delta_ticks.max(0.0)).min(distance);
                live.velocity_x = dx / distance * speed;
                live.velocity_y = dy / distance * speed;
                live.x += dx / distance * step;
                live.y += dy / distance * step;
                live.rotation = dy.atan2(dx).to_degrees();
                changed = true;
            }
            continue;
        }

        if let Ok(frame) = encode_unit_entered_payload_frame(unit_id, position) {
            out.broadcast(frame);
        }
        world.enemies.remove(&unit_id);
        // P0-01: control associations die with the unit-as-payload.
        crate::network::units::detach_unit_control(world, unit_id);
        if let Some(mut live) = world.tiles.get_mut(&position) {
            live.stored_amount = i32::from(unit.unit_type) + 1;
            live.production_progress = 0.0;
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
        let Some(recipe) = reconstructor_recipe(snapshot.block) else {
            continue;
        };
        let rules = world.wave_rules.read();
        let tick = *world.game_state.simulation_time.read();
        if !rules.activate_unit_factories(snapshot.team, tick) {
            continue;
        }
        if snapshot.stored_amount == 0 {
            continue;
        }
        let input_type = i16::try_from(snapshot.stored_amount - 1).unwrap_or(-1);
        let Some(output_type) = reconstructor_upgrade(snapshot.block, input_type) else {
            continue;
        };
        if !can_create_unit(world, snapshot.team, output_type) {
            continue;
        }
        let has_items = recipe
            .items
            .iter()
            .all(|(item, amount)| inventory_count(&snapshot.inventory, *item) >= *amount);
        let has_liquid = recipe.liquid_rate <= 0.0
            || (snapshot.stored_liquid == 3
                && snapshot.liquid_amount + 0.0001 >= recipe.liquid_rate * delta_ticks.max(0.0));
        let efficiency = power.get(&key).copied().unwrap_or(0.0);
        if !has_items || !has_liquid || efficiency <= 0.0 {
            continue;
        }
        let complete = if let Some(mut reconstructor) = world.tiles.get_mut(&key) {
            let scaled_delta = delta_ticks * building_time_scale(world, key) * efficiency;
            reconstructor.production_progress += scaled_delta;
            if recipe.liquid_rate > 0.0 {
                reconstructor.liquid_amount =
                    (reconstructor.liquid_amount - recipe.liquid_rate * scaled_delta).max(0.0);
                if reconstructor.liquid_amount <= 0.0001 {
                    reconstructor.liquid_amount = 0.0;
                    reconstructor.stored_liquid = -1;
                }
            }
            if reconstructor.production_progress >= recipe.build_time {
                for (item, amount) in recipe.items {
                    let removed = inventory_remove(&mut reconstructor.inventory, *item, *amount);
                    debug_assert!(removed, "validated reconstructor inputs disappeared");
                }
                reconstructor.production_progress = 0.0;
                reconstructor.stored_amount = 0;
                true
            } else {
                false
            }
        } else {
            false
        };
        changed = true;
        if complete {
            spawn_factory_unit(world, out, &snapshot, output_type);
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
        let crafted = if let Some(mut factory) = world.tiles.get_mut(&key) {
            factory.production_progress +=
                delta_ticks * building_time_scale(world, key) * efficiency;
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
