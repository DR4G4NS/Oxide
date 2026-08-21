//! Support simulation: menders, regen projectors, shockwave towers/mines,
//! oct/quasar/tecta fields, navanax suppression, overdrives, force projectors
//! and projectile-hit handling. Economy facade re-exports through
//! crate::network::economy::*.

use crate::network::buildings::snapshot::dynamic_tile_health;
use crate::network::buildings::snapshot::*;
use crate::network::economy::spec::{inventory_count, inventory_remove};
use crate::network::wire::encode::{
    encode_build_destroyed_frame, encode_build_health_update_frame, frame_generated_packet,
};
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

pub(crate) fn mender_spec(block: i16) -> Option<MenderSpec> {
    match block {
        245 => Some(MenderSpec {
            reload: 200.0,
            range: 40.0,
            heal_percent: 4.0,
            booster_item: 9, // silicon
            phase_boost: 4.0,
            phase_range_boost: 20.0,
            use_time: 400.0,
        }),
        246 => Some(MenderSpec {
            reload: 250.0,
            range: 85.0,
            heal_percent: 11.0,
            booster_item: 11, // phase fabric
            phase_boost: 15.0,
            phase_range_boost: 50.0,
            use_time: 400.0,
        }),
        _ => None,
    }
}

pub(crate) fn lerp_delta(current: f32, target: f32, rate: f32, delta_ticks: f32) -> f32 {
    let alpha = 1.0 - (1.0 - rate).powf(delta_ticks.max(0.0));
    current + (target - current) * alpha
}

pub(crate) fn simulate_menders(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| mender_spec(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(spec) = mender_spec(snapshot.block) else {
            continue;
        };
        let delta_ticks = delta_ticks * building_time_scale(world, key);
        let efficiency = power.get(&key).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let has_booster = inventory_count(&snapshot.inventory, spec.booster_item) > 0;
        let mut should_heal = false;
        let mut phase_heat = 0.0;
        if let Some(mut mender) = world.tiles.get_mut(&key) {
            mender.transport_progress =
                lerp_delta(mender.transport_progress, efficiency, 0.08, delta_ticks)
                    .clamp(0.0, 1.0);
            mender.output_liquid_amount = lerp_delta(
                mender.output_liquid_amount,
                f32::from(has_booster),
                0.1,
                delta_ticks,
            )
            .clamp(0.0, 1.0);
            phase_heat = mender.output_liquid_amount;
            mender.production_progress += mender.transport_progress * delta_ticks.max(0.0);
            if has_booster && efficiency > 0.0 {
                mender.ammo_units += delta_ticks.max(0.0);
                if mender.ammo_units >= spec.use_time {
                    mender.ammo_units %= spec.use_time;
                    let _ = inventory_remove(&mut mender.inventory, spec.booster_item, 1);
                }
            } else {
                mender.ammo_units = 0.0;
            }
            if mender.production_progress >= spec.reload {
                mender.production_progress %= spec.reload;
                should_heal = true;
            }
            changed = true;
        }
        if !should_heal || efficiency <= 0.0 {
            continue;
        }
        let source_x = (snapshot.position >> 16) as i16 as f32 * 8.0;
        let source_y = snapshot.position as i16 as f32 * 8.0;
        let range = spec.range + phase_heat * spec.phase_range_boost;
        let heal_scale = (spec.heal_percent + phase_heat * spec.phase_boost) / 100.0 * efficiency;
        let mut updates = Vec::new();
        let dynamic_targets: Vec<_> = world
            .tiles
            .iter()
            .filter(|tile| tile.block != 0 && tile.team == snapshot.team)
            .filter(|tile| {
                let x = (tile.position >> 16) as i16 as f32 * 8.0;
                let y = tile.position as i16 as f32 * 8.0;
                (x - source_x).hypot(y - source_y) <= range
            })
            .map(|tile| tile.position)
            .collect();
        for position in dynamic_targets {
            if let Some(mut target) = world.tiles.get_mut(&position) {
                let maximum = crate::game::content::block_health(target.block);
                let current = dynamic_tile_health(&target);
                let health = (current + maximum * heal_scale).min(maximum);
                if health > current {
                    target.health = health;
                    updates.push((position, health));
                }
            }
        }
        let base_candidates: Vec<_> = world
            .base_buildings
            .iter()
            .filter(|building| building.team == snapshot.team)
            .filter(|building| {
                let x = (building.position >> 16) as i16 as f32 * 8.0;
                let y = building.position as i16 as f32 * 8.0;
                (x - source_x).hypot(y - source_y) <= range
            })
            .map(|building| building.position)
            .collect();
        // Loaded map buildings have an authoritative DynamicTile at the same
        // position. Keep the base registry as a fallback for legacy/base-only
        // entries, but never process the compatibility copy twice.
        let base_targets: Vec<_> = base_candidates
            .into_iter()
            .filter(|position| !world.tiles.contains_key(position))
            .collect();
        for position in base_targets {
            if let Some(mut target) = world.base_buildings.get_mut(&position) {
                let maximum = crate::game::content::block_health(target.block);
                let current = target.health;
                target.health = (target.health + maximum * heal_scale).min(maximum);
                if target.health > current {
                    updates.push((position, target.health));
                }
            }
        }
        // The mender heals its own team's core (PvP maps carry one core per
        // team; teams without a registered core fall back to the sharded one).
        let (core_x, core_y) = {
            let position = crate::network::world::core_position_for_team(world, snapshot.team);
            (
                (position >> 16) as i16 as f32 * 8.0,
                position as i16 as f32 * 8.0,
            )
        };
        if (core_x - source_x).hypot(core_y - source_y) <= range {
            changed |= crate::network::combat::heal_team_core(
                world,
                snapshot.team,
                crate::network::world::core_max_health_for_team(world, snapshot.team) * heal_scale,
            );
        }
        if !updates.is_empty() {
            if let Ok(frame) = encode_build_health_update_frame(&updates) {
                out.broadcast(frame);
            }
            changed = true;
        }
    }
    changed
}

pub(crate) const REGEN_PROJECTOR_BLOCK: i16 = 253;
pub(crate) const HYDROGEN_LIQUID: i16 = 8;

pub(crate) fn simulate_regen_projectors(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == REGEN_PROJECTOR_BLOCK)
        .map(|tile| *tile.key())
        .collect();
    let mut dynamic_heals = HashMap::<i32, f32>::new();
    let mut base_heals = HashMap::<i32, f32>::new();
    let mut core_heal = HashMap::<u8, f32>::new();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let center_x = (snapshot.position >> 16) as i16 as f32 * 8.0;
        let center_y = snapshot.position as i16 as f32 * 8.0;
        let half_extent = 28.0 * 8.0 / 2.0;
        let in_range = |position: i32| {
            let x = (position >> 16) as i16 as f32 * 8.0;
            let y = position as i16 as f32 * 8.0;
            (x - center_x).abs() <= half_extent && (y - center_y).abs() <= half_extent
        };
        let any_target =
            world.tiles.iter().any(|tile| {
                tile.block != 0
                    && tile.team == snapshot.team
                    && in_range(tile.position)
                    && dynamic_tile_health(&tile)
                        < crate::game::content::block_health(tile.block) - 0.0001
            }) || world.base_buildings.iter().any(|building| {
                building.team == snapshot.team
                    && in_range(building.position)
                    && building.health < crate::game::content::block_health(building.block) - 0.0001
            }) || (in_range(crate::network::world::core_position_for_team(
                world,
                snapshot.team,
            )) && crate::network::world::core_health_for_team(world, snapshot.team)
                < crate::network::world::core_max_health_for_team(world, snapshot.team) - 0.0001);
        let has_hydrogen =
            snapshot.stored_liquid == HYDROGEN_LIQUID && snapshot.liquid_amount > 0.0001;
        let efficiency = power.get(&key).copied().unwrap_or(0.0).clamp(0.0, 1.0)
            * f32::from(has_hydrogen && any_target);
        let scaled_delta = delta_ticks * building_time_scale(world, key);
        let optional = inventory_count(&snapshot.inventory, 11) > 0;
        if let Some(mut projector) = world.tiles.get_mut(&key) {
            let target_warmup = f32::from(efficiency > 0.0);
            let step = scaled_delta / 70.0;
            projector.transport_progress = if projector.transport_progress < target_warmup {
                (projector.transport_progress + step).min(target_warmup)
            } else {
                (projector.transport_progress - step).max(target_warmup)
            };
            projector.ammo_units += projector.transport_progress * scaled_delta;
            if efficiency > 0.0 {
                let consumed = (scaled_delta / 60.0).min(projector.liquid_amount);
                projector.liquid_amount -= consumed;
                if projector.liquid_amount <= 0.0001 {
                    projector.liquid_amount = 0.0;
                    projector.stored_liquid = -1;
                }
                if optional {
                    projector.production_progress += scaled_delta * efficiency;
                    if projector.production_progress >= 480.0 {
                        projector.production_progress = 0.0;
                        let _ = inventory_remove(&mut projector.inventory, 11, 1);
                    }
                }
            }
            changed = true;
        }
        if efficiency <= 0.0 {
            continue;
        }
        let multiplier = if optional { 2.0 } else { 1.0 };
        let heal_fraction = multiplier * (4.0 / 60.0) * scaled_delta * efficiency / 100.0;
        for target in world
            .tiles
            .iter()
            .filter(|tile| tile.block != 0 && tile.team == snapshot.team && in_range(tile.position))
        {
            let maximum = crate::game::content::block_health(target.block);
            if dynamic_tile_health(&target) < maximum - 0.0001 {
                dynamic_heals
                    .entry(target.position)
                    .and_modify(|heal| *heal = heal.max(maximum * heal_fraction))
                    .or_insert(maximum * heal_fraction);
            }
        }
        for target in world
            .base_buildings
            .iter()
            .filter(|building| building.team == snapshot.team && in_range(building.position))
        {
            let maximum = crate::game::content::block_health(target.block);
            if target.health < maximum - 0.0001 {
                base_heals
                    .entry(target.position)
                    .and_modify(|heal| *heal = heal.max(maximum * heal_fraction))
                    .or_insert(maximum * heal_fraction);
            }
        }
        if in_range(crate::network::world::core_position_for_team(
            world,
            snapshot.team,
        )) && crate::network::world::core_health_for_team(world, snapshot.team)
            < crate::network::world::core_max_health_for_team(world, snapshot.team) - 0.0001
        {
            core_heal
                .entry(snapshot.team)
                .and_modify(|heal| {
                    *heal = heal.max(
                        crate::network::world::core_max_health_for_team(world, snapshot.team)
                            * heal_fraction,
                    )
                })
                .or_insert(
                    crate::network::world::core_max_health_for_team(world, snapshot.team)
                        * heal_fraction,
                );
        }
    }

    let mut updates = Vec::new();
    for (position, heal) in dynamic_heals {
        if let Some(mut target) = world.tiles.get_mut(&position) {
            let maximum = crate::game::content::block_health(target.block);
            let current = dynamic_tile_health(&target);
            target.health = (current + heal).min(maximum);
            if target.health > current {
                updates.push((position, target.health));
            }
        }
    }
    for (position, heal) in base_heals {
        if let Some(mut target) = world.base_buildings.get_mut(&position) {
            let maximum = crate::game::content::block_health(target.block);
            let current = target.health;
            target.health = (current + heal).min(maximum);
            if target.health > current {
                updates.push((position, target.health));
            }
        }
    }
    for (team, heal) in core_heal {
        if crate::network::combat::heal_team_core(world, team, heal) {
            changed = true;
        }
    }
    if !updates.is_empty() {
        if let Ok(frame) = encode_build_health_update_frame(&updates) {
            out.broadcast(frame);
        }
        changed = true;
    }
    changed
}

pub(crate) const SHOCKWAVE_TOWER_BLOCK: i16 = 254;
pub(crate) const CYANOGEN_LIQUID: i16 = 10;

pub(crate) fn projectile_position(projectile: &Projectile) -> (f32, f32) {
    let progress = if projectile.total_ticks <= 0.0001 {
        1.0
    } else {
        (1.0 - projectile.remaining_ticks / projectile.total_ticks).clamp(0.0, 1.0)
    };
    (
        projectile.source_x + (projectile.target_x - projectile.source_x) * progress,
        projectile.source_y + (projectile.target_y - projectile.source_y) * progress,
    )
}

pub(crate) fn simulate_shockwave_towers(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == SHOCKWAVE_TOWER_BLOCK)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let has_cyanogen =
            snapshot.stored_liquid == CYANOGEN_LIQUID && snapshot.liquid_amount > 0.0001;
        let efficiency =
            power.get(&key).copied().unwrap_or(0.0).clamp(0.0, 1.0) * f32::from(has_cyanogen);
        let scaled_delta = delta_ticks * building_time_scale(world, key);
        let mut ready = false;
        if let Some(mut tower) = world.tiles.get_mut(&key) {
            tower.transport_progress =
                (tower.transport_progress - scaled_delta / 80.0).clamp(0.0, 1.0);
            if tower.production_progress < 80.0 && efficiency > 0.0 {
                tower.production_progress =
                    (tower.production_progress + scaled_delta * efficiency).min(80.0);
                let consumed = (1.5 / 60.0 * scaled_delta * efficiency).min(tower.liquid_amount);
                tower.liquid_amount -= consumed;
                if tower.liquid_amount <= 0.0001 {
                    tower.liquid_amount = 0.0;
                    tower.stored_liquid = -1;
                }
                changed = true;
            }
            ready = tower.production_progress >= 80.0 && efficiency > 0.0;
        }
        if !ready {
            continue;
        }
        let x = (snapshot.position >> 16) as i16 as f32 * 8.0;
        let y = snapshot.position as i16 as f32 * 8.0;
        let targets: Vec<_> = world
            .projectiles
            .iter()
            .filter(|projectile| projectile.team != snapshot.team)
            .filter(|projectile| {
                let (bullet_x, bullet_y) = projectile_position(projectile);
                (bullet_x - x).abs() <= 170.0 && (bullet_y - y).abs() <= 170.0
            })
            .map(|projectile| *projectile.key())
            .collect();
        if targets.is_empty() {
            continue;
        }
        let wave_damage = 160.0f32.min(160.0 * 20.0 / targets.len() as f32);
        for projectile_id in targets {
            let remove = if let Some(mut projectile) = world.projectiles.get_mut(&projectile_id) {
                if projectile.damage > wave_damage {
                    projectile.damage -= wave_damage;
                    false
                } else {
                    true
                }
            } else {
                false
            };
            if remove {
                world.projectiles.remove(&projectile_id);
            }
        }
        if let Some(mut tower) = world.tiles.get_mut(&key) {
            tower.production_progress = 0.0;
            tower.transport_progress = 1.0;
        }
        changed = true;
    }
    changed
}

pub(crate) const SHOCK_MINE_BLOCK: i16 = 250;

pub(crate) fn simulate_shock_mines(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == SHOCK_MINE_BLOCK)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        if let Some(mut mine) = world.tiles.get_mut(&key) {
            mine.production_progress = (mine.production_progress - delta_ticks).max(0.0);
        }
        if snapshot.production_progress > 0.0 {
            changed = true;
            continue;
        }
        let x = (snapshot.position >> 16) as i16 as f32 * 8.0;
        let y = snapshot.position as i16 as f32 * 8.0;
        let target = world
            .enemies
            .iter()
            .filter(|enemy| enemy.team == 2 && enemy.entity_class != 3)
            .filter_map(|enemy| {
                let distance = (enemy.x - x).hypot(enemy.y - y);
                (distance <= 8.0).then_some((enemy.id, distance))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1))
            .map(|target| target.0);
        let Some(target_id) = target else {
            continue;
        };

        let mut dead = false;
        if let Some(mut enemy) = world.enemies.get_mut(&target_id) {
            for _ in 0..4 {
                enemy.health -= apply_incoming_unit_damage(&enemy, 25.0, 1.0);
            }
            dead = enemy.health <= 0.0;
        }
        for angle in [0.0, 90.0, 180.0, 270.0] {
            if let Ok(payload) =
                encode_create_bullet_payload(2, snapshot.team, x, y, angle, 25.0, 1.0, 1.0)
            {
                if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false)
                {
                    out.broadcast(frame);
                }
            }
        }
        if dead {
            kill_enemy(world, out, target_id);
        }

        let mut destroyed = false;
        let mut health_update = None;
        if let Some(mut mine) = world.tiles.get_mut(&key) {
            mine.production_progress = 80.0;
            let current = dynamic_tile_health(&mine);
            mine.health = (current - 7.0).max(0.0);
            destroyed = mine.health <= 0.0;
            if !destroyed {
                health_update = Some(mine.health);
            }
        }
        if destroyed {
            world.tiles.remove(&key);
            world.navigation_revision.fetch_add(1, Ordering::Relaxed);
            if let Ok(frame) = encode_build_destroyed_frame(key) {
                out.broadcast(frame);
            }
        } else if let Some(health) = health_update {
            if let Ok(frame) = encode_build_health_update_frame(&[(key, health)]) {
                out.broadcast(frame);
            }
        }
        changed = true;
    }
    changed
}

#[derive(Clone, Copy)]
pub(crate) struct OverdriveSpec {
    pub(crate) reload: f32,
    pub(crate) range: f32,
    pub(crate) speed_boost: f32,
    pub(crate) speed_boost_phase: f32,
    pub(crate) phase_range_boost: f32,
    pub(crate) use_time: f32,
    pub(crate) required_items: &'static [(i16, i32)],
    pub(crate) has_boost: bool,
}

pub(crate) fn overdrive_spec(block: i16) -> Option<OverdriveSpec> {
    match block {
        247 => Some(OverdriveSpec {
            reload: 60.0,
            range: 80.0,
            speed_boost: 1.5,
            speed_boost_phase: 0.75,
            phase_range_boost: 20.0,
            use_time: 400.0,
            required_items: &[(11, 1)], // phase fabric
            has_boost: true,
        }),
        248 => Some(OverdriveSpec {
            reload: 60.0,
            range: 200.0,
            speed_boost: 2.5,
            speed_boost_phase: 0.0,
            phase_range_boost: 0.0,
            use_time: 300.0,
            required_items: &[(11, 1), (9, 1)], // phase fabric + silicon
            has_boost: false,
        }),
        _ => None,
    }
}

pub(crate) fn building_time_scale(world: &DynamicWorld, position: i32) -> f32 {
    world
        .overdrive_boosts
        .get(&position)
        .map_or(1.0, |boost| boost.multiplier.max(1.0))
}

/// Official `Block.canOverdrive == false` set (Serpulo) — EMP boosts only
/// buildings that can be overdriven and have a power module.
pub(crate) fn block_can_emp_boost(block: i16) -> bool {
    !matches!(
        block,
        203..=208 // heat producers (electric/slag/phase heaters, redirectors, router)
            | 210 // carbide-crucible (HeatCrafter)
            | 212 // surge-crucible (HeatCrafter)
            | 213 // cyanogen-synthesizer (HeatCrafter)
            | 214 // phase-synthesizer (HeatCrafter)
            | 215 // heat reactor
            | 216..=244 // walls, doors, thruster
            | 247..=248 // overdrive projector / dome
            | 268..=269 // overflow/underflow gates
            | 286..=294 // conduits, liquid router, liquid bridges
            | 302..=304 // power nodes, surge tower
            | 306..=307 // batteries
            | 339..=344 // cores
            | 408..=409 // payload loader / unloader
            | 427 // landing pad
            | 432..=440 // logic processors, memory, displays, canvas
    )
}

pub(crate) fn block_is_suppressable(block: i16) -> bool {
    matches!(block, 246 | 253) // mend-projector, regen-projector
}

pub(crate) fn building_heal_suppressed(world: &DynamicWorld, position: i32, block: i16) -> bool {
    block_is_suppressable(block)
        && world
            .heal_suppression
            .get(&position)
            .is_some_and(|t| *t > 0.0)
}

// Oct ForceFieldAbility(140, 4, 7000, 60*8, 8, 0) values (UnitTypes.java
// line 1533): radius 140, regen 4 HP/tick, max 7000, cooldown 480 ticks.
pub(crate) const OCT_FORCE_FIELD_RADIUS: f32 = 140.0;
pub(crate) const OCT_FORCE_FIELD_REGEN: f32 = 4.0;
pub(crate) const OCT_FORCE_FIELD_MAX: f32 = 7_000.0;
pub(crate) const OCT_FORCE_FIELD_COOLDOWN: f32 = 480.0;

/// Area shield regen/cooldown for every oct in the world. Fields are created
/// at full hp (official ForceFieldAbility.created sets unit.shield = max) and
/// removed when their oct disappears.
pub(crate) fn simulate_oct_force_fields(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let mut changed = false;
    let octs: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 24)
        .map(|unit| unit.id)
        .collect();
    for id in octs {
        let mut entry = world.force_fields.entry(id).or_insert(ForceFieldState {
            hp: OCT_FORCE_FIELD_MAX,
            remaining_ticks: 0.0,
        });
        if entry.remaining_ticks > 0.0 {
            // Broken shield: official cooldown drains cooldown*regen before
            // regen resumes, i.e. 480 ticks of no absorption.
            entry.remaining_ticks = (entry.remaining_ticks - delta_ticks.max(0.0)).max(0.0);
        } else if entry.hp < OCT_FORCE_FIELD_MAX {
            entry.hp =
                (entry.hp + OCT_FORCE_FIELD_REGEN * delta_ticks.max(0.0)).min(OCT_FORCE_FIELD_MAX);
        }
        changed = true;
    }
    let keys: Vec<_> = world
        .force_fields
        .iter()
        .map(|entry| *entry.key())
        .collect();
    for id in keys {
        if !world.enemies.contains_key(&id) {
            world.force_fields.remove(&id);
            changed = true;
        }
    }
    changed
}

/// Absorbs an enemy projectile landing at (x, y) into the nearest oct area
/// shield within radius 140 (projectile team != oct team). Returns true when
/// the projectile was consumed: the oct's shield loses the bullet's damage
/// (BulletType.shieldDamage defaults to damage) and the bullet is removed,
/// so its direct/splash/frag effects never apply. Scope: only projectiles
/// simulated through simulate_projectiles are absorbed (melee/building
/// damage is not shielded).
pub(crate) fn oct_force_field_absorb(
    world: &DynamicWorld,
    projectile_team: u8,
    x: f32,
    y: f32,
    damage: f32,
) -> bool {
    let octs: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 24 && unit.team != projectile_team && unit.health > 0.0)
        .filter_map(|unit| {
            ((unit.x - x).hypot(unit.y - y) <= OCT_FORCE_FIELD_RADIUS).then_some(unit.id)
        })
        .collect();
    if octs.is_empty() {
        return false;
    }
    for id in octs {
        let Some(mut field) = world.force_fields.get_mut(&id) else {
            continue;
        };
        if field.remaining_ticks > 0.0 || field.hp <= 0.0 {
            continue;
        }
        field.hp -= damage;
        if field.hp <= 0.0 {
            field.remaining_ticks = OCT_FORCE_FIELD_COOLDOWN;
        }
        return true;
    }
    false
}

/// Quasar ForceFieldAbility(60, 0.4, 500, 360): absorb bullets inside radius
/// 60 into `unit.shield`. Oct keeps `world.force_fields` and is not rewritten.
pub(crate) const QUASAR_FORCE_FIELD_RADIUS: f32 = 60.0;
pub(crate) const QUASAR_FORCE_FIELD_REGEN: f32 = 0.4;
pub(crate) const QUASAR_FORCE_FIELD_COOLDOWN: f32 = 360.0;

pub(crate) fn quasar_force_field_absorb(
    world: &DynamicWorld,
    projectile_team: u8,
    x: f32,
    y: f32,
    damage: f32,
) -> bool {
    let quasars: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 7 && unit.team != projectile_team && unit.health > 0.0)
        .filter_map(|unit| {
            (unit.shield > 0.0 && (unit.x - x).hypot(unit.y - y) <= QUASAR_FORCE_FIELD_RADIUS)
                .then_some(unit.id)
        })
        .collect();
    for id in quasars {
        let Some(mut unit) = world.enemies.get_mut(&id) else {
            continue;
        };
        if unit.shield <= 0.0 {
            continue;
        }
        unit.shield -= damage;
        if unit.shield <= 0.0 {
            unit.shield -= QUASAR_FORCE_FIELD_COOLDOWN * QUASAR_FORCE_FIELD_REGEN;
        }
        return true;
    }
    false
}

/// Tecta ShieldArcAbility: radius 45, width 8, angle 82, regen 45/60,
/// max 2500, cooldown 480, y = -20, whenShooting = false.
/// `chanceDeflect = 1` is approximated as absorb (bullet consumed).
pub(crate) const TECTA_SHIELD_RADIUS: f32 = 45.0;
pub(crate) const TECTA_SHIELD_WIDTH: f32 = 8.0;
pub(crate) const TECTA_SHIELD_ANGLE: f32 = 82.0;
pub(crate) const TECTA_SHIELD_REGEN: f32 = 45.0 / 60.0;
pub(crate) const TECTA_SHIELD_MAX: f32 = 2_500.0;
pub(crate) const TECTA_SHIELD_COOLDOWN: f32 = 480.0;
const TECTA_SHIELD_OFFSET_Y: f32 = -20.0;

pub(crate) fn tecta_arc_origin(x: f32, y: f32, rotation: f32) -> (f32, f32) {
    let rad = (rotation - 90.0).to_radians();
    let dx = -TECTA_SHIELD_OFFSET_Y * rad.sin();
    let dy = TECTA_SHIELD_OFFSET_Y * rad.cos();
    (x + dx, y + dy)
}

pub(crate) fn angles_within(a: f32, b: f32, max_delta: f32) -> bool {
    let delta = ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs();
    delta <= max_delta
}

pub(crate) fn projectile_in_tecta_arc(
    origin_x: f32,
    origin_y: f32,
    rotation: f32,
    x: f32,
    y: f32,
) -> bool {
    let reach = TECTA_SHIELD_RADIUS + TECTA_SHIELD_WIDTH;
    if (origin_x - x).hypot(origin_y - y) > reach {
        return false;
    }
    let toward = (y - origin_y).atan2(x - origin_x).to_degrees();
    angles_within(toward, rotation, TECTA_SHIELD_ANGLE / 2.0)
}

pub(crate) fn simulate_tecta_shield_arcs(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let mut changed = false;
    let tectas: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 47)
        .map(|unit| unit.id)
        .collect();
    for id in tectas {
        let mut entry = world.force_fields.entry(id).or_insert(ForceFieldState {
            hp: TECTA_SHIELD_MAX,
            remaining_ticks: 0.0,
        });
        if entry.hp < TECTA_SHIELD_MAX {
            entry.hp = (entry.hp + TECTA_SHIELD_REGEN * delta_ticks.max(0.0)).min(TECTA_SHIELD_MAX);
        }
        changed = true;
    }
    changed
}

pub(crate) fn tecta_shield_arc_absorb(
    world: &DynamicWorld,
    projectile_team: u8,
    x: f32,
    y: f32,
    damage: f32,
) -> bool {
    let tectas: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.unit_type == 47 && unit.team != projectile_team && unit.health > 0.0)
        .filter_map(|unit| {
            let (ox, oy) = tecta_arc_origin(unit.x, unit.y, unit.rotation);
            projectile_in_tecta_arc(ox, oy, unit.rotation, x, y).then_some(unit.id)
        })
        .collect();
    for id in tectas {
        let mut field = world.force_fields.entry(id).or_insert(ForceFieldState {
            hp: TECTA_SHIELD_MAX,
            remaining_ticks: 0.0,
        });
        if field.hp <= 0.0 {
            continue;
        }
        if field.hp <= damage {
            field.hp -= TECTA_SHIELD_COOLDOWN * TECTA_SHIELD_REGEN;
        }
        field.hp -= damage;
        return true;
    }
    false
}

pub(crate) fn simulate_navanax_suppression(world: &DynamicWorld, delta_ticks: f32) -> bool {
    let mut changed = false;
    let keys: Vec<_> = world
        .heal_suppression
        .iter()
        .map(|entry| *entry.key())
        .collect();
    for position in keys {
        if let Some(mut remaining) = world.heal_suppression.get_mut(&position) {
            *remaining = (*remaining - delta_ticks.max(0.0)).max(0.0);
            if *remaining <= 0.0 {
                drop(remaining);
                world.heal_suppression.remove(&position);
            }
            changed = true;
        }
    }
    // SuppressionFieldAbility: pulse every maxDelay (90), duration = reload+1.
    // Navanax 34: range 200, reload 90. Quell 52: range 200, reload 480.
    // Disrupt 54: range 320, reload 900 (only the active middle field).
    let sources: Vec<_> = world
        .enemies
        .iter()
        .filter_map(|unit| {
            let (range, duration) = match unit.unit_type {
                34 => (200.0, 91.0),
                52 => (200.0, 481.0),
                54 => (320.0, 901.0),
                _ => return None,
            };
            Some((unit.id, unit.team, unit.x, unit.y, range, duration))
        })
        .collect();
    for (source_id, team, x, y, range, duration) in sources {
        let activations = {
            let Some(mut source) = world.enemies.get_mut(&source_id) else {
                continue;
            };
            source.tertiary_attack_reload += delta_ticks.max(0.0);
            let activations = (source.tertiary_attack_reload / 90.0).floor() as usize;
            source.tertiary_attack_reload %= 90.0;
            activations
        };
        if activations == 0 {
            continue;
        }
        let mut seen = HashSet::new();
        let mut positions: Vec<_> = world
            .tiles
            .iter()
            .filter(|tile| tile.block != 0 && tile.team != team && seen.insert(tile.position))
            .filter_map(|tile| {
                let tile_x = (tile.position >> 16) as i16 as f32 * 8.0;
                let tile_y = tile.position as i16 as f32 * 8.0;
                ((tile_x - x).hypot(tile_y - y) <= range).then_some(tile.position)
            })
            .collect();
        positions.extend(world.base_buildings.iter().filter_map(|building| {
            if building.team == team || !seen.insert(building.position) {
                return None;
            }
            let building_x = (building.position >> 16) as i16 as f32 * 8.0;
            let building_y = building.position as i16 as f32 * 8.0;
            ((building_x - x).hypot(building_y - y) <= range).then_some(building.position)
        }));
        for position in positions {
            let _ = world
                .heal_suppression
                .entry(position)
                .and_modify(|remaining| *remaining = remaining.max(duration))
                .or_insert(duration);
            changed = true;
        }
    }
    changed
}

pub(crate) fn can_receive_overdrive(block: i16) -> bool {
    !matches!(
        block,
        216..=244 // walls, doors and passive defenses
            | 247..=248 // overdrive projectors cannot overdrive each other
            | 264..=265 // item routers
            | 268..=269 // overflow/underflow gates
            | 286..=294 // passive liquid transport/storage
            | 302..=304 // power nodes
            | 306..=307 // batteries
            | 339..=344 // cores
    )
}

pub(crate) fn simulate_overdrives(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let boost_keys: Vec<_> = world
        .overdrive_boosts
        .iter()
        .map(|boost| *boost.key())
        .collect();
    for key in boost_keys {
        if let Some(mut boost) = world.overdrive_boosts.get_mut(&key) {
            boost.remaining_ticks -= delta_ticks.max(0.0);
            if boost.remaining_ticks <= 0.0 {
                drop(boost);
                world.overdrive_boosts.remove(&key);
            }
        }
    }

    let projector_keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| overdrive_spec(tile.block).is_some())
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in projector_keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let Some(spec) = overdrive_spec(snapshot.block) else {
            continue;
        };
        let has_items = spec
            .required_items
            .iter()
            .all(|(item, amount)| inventory_count(&snapshot.inventory, *item) >= *amount);
        let efficiency = power.get(&key).copied().unwrap_or(0.0).clamp(0.0, 1.0)
            * f32::from(spec.has_boost || has_items);
        let optional_efficiency = f32::from(spec.has_boost && has_items);
        let mut pulse = false;
        let mut phase_heat = 0.0;
        if let Some(mut projector) = world.tiles.get_mut(&key) {
            projector.transport_progress = lerp_delta(
                projector.transport_progress,
                f32::from(efficiency > 0.0),
                0.08,
                delta_ticks,
            )
            .clamp(0.0, 1.0);
            projector.output_liquid_amount = if spec.has_boost {
                lerp_delta(
                    projector.output_liquid_amount,
                    optional_efficiency,
                    0.1,
                    delta_ticks,
                )
                .clamp(0.0, 1.0)
            } else {
                0.0
            };
            phase_heat = projector.output_liquid_amount;
            projector.production_progress += projector.transport_progress * delta_ticks.max(0.0);
            if efficiency > 0.0 {
                projector.ammo_units += delta_ticks.max(0.0);
                if projector.ammo_units >= spec.use_time && has_items {
                    projector.ammo_units %= spec.use_time;
                    for (item, amount) in spec.required_items {
                        let _ = inventory_remove(&mut projector.inventory, *item, *amount);
                    }
                }
            }
            if projector.production_progress >= spec.reload {
                projector.production_progress = 0.0;
                pulse = true;
            }
            changed = true;
        }
        if !pulse || efficiency <= 0.0 {
            continue;
        }
        let source_x = (snapshot.position >> 16) as i16 as f32 * 8.0;
        let source_y = snapshot.position as i16 as f32 * 8.0;
        let range = spec.range + phase_heat * spec.phase_range_boost;
        let multiplier =
            ((spec.speed_boost + phase_heat * spec.speed_boost_phase) * efficiency).max(1.0);
        let targets: Vec<_> = world
            .tiles
            .iter()
            .filter(|tile| {
                tile.team == snapshot.team
                    && tile.position != key
                    && can_receive_overdrive(tile.block)
            })
            .filter(|tile| {
                let x = (tile.position >> 16) as i16 as f32 * 8.0;
                let y = tile.position as i16 as f32 * 8.0;
                (x - source_x).hypot(y - source_y) <= range
            })
            .map(|tile| tile.position)
            .collect();
        for target in targets {
            world
                .overdrive_boosts
                .entry(target)
                .and_modify(|boost| {
                    boost.multiplier = boost.multiplier.max(multiplier);
                    boost.remaining_ticks = boost.remaining_ticks.max(spec.reload + 1.0);
                })
                .or_insert(TimedBoost {
                    multiplier,
                    remaining_ticks: spec.reload + 1.0,
                });
        }
    }
    changed
}

pub(crate) const FORCE_PROJECTOR_BLOCK: i16 = 249;
pub(crate) const FORCE_RADIUS: f32 = 101.7;
pub(crate) const FORCE_PHASE_RADIUS_BOOST: f32 = 80.0;
pub(crate) const FORCE_SHIELD_HEALTH: f32 = 750.0;
pub(crate) const FORCE_PHASE_SHIELD_BOOST: f32 = 400.0;
pub(crate) const FORCE_PHASE_USE_TIME: f32 = 350.0;

pub(crate) fn force_broken(tile: &DynamicTile) -> bool {
    tile.config.first() == Some(&1)
}

pub(crate) fn set_force_broken(tile: &mut DynamicTile, broken: bool) {
    if tile.config.is_empty() {
        tile.config.push(u8::from(broken));
    } else {
        tile.config[0] = u8::from(broken);
    }
}

pub(crate) fn simulate_force_projectors(
    world: &DynamicWorld,
    delta_ticks: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == FORCE_PROJECTOR_BLOCK)
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(snapshot) = world.tiles.get(&key).map(|tile| tile.clone()) else {
            continue;
        };
        let efficiency = power.get(&key).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let phase_valid = inventory_count(&snapshot.inventory, 11) > 0;
        let scaled_delta = delta_ticks * building_time_scale(world, key);
        if let Some(mut projector) = world.tiles.get_mut(&key) {
            let mut broken = force_broken(&projector);
            projector.output_liquid_amount = lerp_delta(
                projector.output_liquid_amount,
                f32::from(phase_valid),
                0.1,
                scaled_delta,
            )
            .clamp(0.0, 1.0);
            projector.ammo_units =
                lerp_delta(projector.ammo_units, efficiency, 0.1, scaled_delta).clamp(0.0, 1.0);
            projector.transport_progress = lerp_delta(
                projector.transport_progress,
                if broken { 0.0 } else { projector.ammo_units },
                0.05,
                scaled_delta,
            )
            .clamp(0.0, 1.0);

            if phase_valid && !broken && efficiency > 0.0 {
                projector.mass_driver_rotation += scaled_delta;
                if projector.mass_driver_rotation >= FORCE_PHASE_USE_TIME {
                    projector.mass_driver_rotation %= FORCE_PHASE_USE_TIME;
                    let _ = inventory_remove(&mut projector.inventory, 11, 1);
                }
            }

            if projector.production_progress > 0.0 {
                let mut cooldown = if broken { 0.35 } else { 1.5 };
                if matches!(projector.stored_liquid, 0 | 3) && projector.liquid_amount > 0.0001 {
                    let heat_capacity = if projector.stored_liquid == 3 {
                        0.9
                    } else {
                        0.4
                    };
                    cooldown *= 1.2 * (1.0 + (heat_capacity - 0.4) * 0.9);
                    projector.liquid_amount =
                        (projector.liquid_amount - 0.1 * scaled_delta).max(0.0);
                    if projector.liquid_amount <= 0.0001 {
                        projector.stored_liquid = -1;
                        projector.liquid_amount = 0.0;
                    }
                }
                projector.production_progress =
                    (projector.production_progress - scaled_delta * cooldown).max(0.0);
            }
            if broken && projector.production_progress <= 0.0 {
                broken = false;
                set_force_broken(&mut projector, false);
            }
            let maximum =
                FORCE_SHIELD_HEALTH + FORCE_PHASE_SHIELD_BOOST * projector.output_liquid_amount;
            if !broken && projector.production_progress >= maximum {
                set_force_broken(&mut projector, true);
                projector.production_progress = FORCE_SHIELD_HEALTH;
            }
            changed = true;
        }
    }
    changed
}

pub(crate) fn segment_intersects_circle(
    start_x: f32,
    start_y: f32,
    end_x: f32,
    end_y: f32,
    center_x: f32,
    center_y: f32,
    radius: f32,
) -> bool {
    let dx = end_x - start_x;
    let dy = end_y - start_y;
    let length_sq = dx * dx + dy * dy;
    let t = if length_sq <= f32::EPSILON {
        0.0
    } else {
        (((center_x - start_x) * dx + (center_y - start_y) * dy) / length_sq).clamp(0.0, 1.0)
    };
    let closest_x = start_x + dx * t;
    let closest_y = start_y + dy * t;
    (closest_x - center_x).hypot(closest_y - center_y) <= radius
}

/// Official BulletComp.update -> tileRaycast: a projectile collides with the
/// first solid building whose tile footprint its source->current segment
/// crosses (or lands in), stopping there instead of flying to the original
/// target. Returns the hit building position and the impact point (tile
/// center). `hit_radius` is the bullet hitSize (default 4) plus a margin for
/// the building footprint.
#[allow(clippy::too_many_arguments)]
pub(crate) fn projectile_building_hit(
    world: &crate::network::world::DynamicWorld,
    source_x: f32,
    source_y: f32,
    current_x: f32,
    current_y: f32,
    team: u8,
    bullet_id: i16,
    hit_radius: f32,
) -> Option<(i32, f32, f32)> {
    // Continuous beams (61-64) and the rail (49) use their own ray/damage
    // logic and must not short-circuit here.
    if (61..=64).contains(&bullet_id) || bullet_id == 49 {
        return None;
    }
    let radius = hit_radius.max(1.0);
    let mut best: Option<(i32, f32, f32, f32)> = None; // (pos, dist, x, y)
    for tile in world.tiles.iter() {
        let t = tile.value();
        if t.block == 0 || t.team == team {
            continue;
        }
        let size = crate::game::content::block_size(t.block) as f32;
        // Building corner used by the impact-reached checks elsewhere, plus
        // the footprint center for the collision radius.
        let corner_x = (t.position >> 16) as i16 as f32 * 8.0;
        let corner_y = t.position as i16 as f32 * 8.0;
        let cx = corner_x + size * 4.0;
        let cy = corner_y + size * 4.0;
        let building_radius = size * 8.0 / 2.0 + radius;
        if !segment_intersects_circle(
            source_x,
            source_y,
            current_x,
            current_y,
            cx,
            cy,
            building_radius,
        ) {
            continue;
        }
        // distance from source to the building center along the segment
        let dist = (cx - source_x).hypot(cy - source_y);
        if best.as_ref().is_none_or(|(_, bd, _, _)| dist < *bd) {
            // Return the tile corner so the impact-reached checks elsewhere
            // (which compare against `pos >> 16 * 8`) match exactly.
            best = Some((t.position, dist, corner_x, corner_y));
        }
    }
    best.map(|(pos, _, x, y)| (pos, x, y))
}

pub(crate) fn absorb_enemy_projectile(
    world: &DynamicWorld,
    projectile: &Projectile,
    delta_ticks: f32,
) -> bool {
    if projectile.team == 1 || projectile.damage_interval.is_some() {
        return false;
    }
    let progress = if projectile.total_ticks <= 0.0001 {
        1.0
    } else {
        (1.0 - projectile.remaining_ticks / projectile.total_ticks).clamp(0.0, 1.0)
    };
    let next_remaining = (projectile.remaining_ticks - delta_ticks.max(0.0)).max(0.0);
    let next_progress = if projectile.total_ticks <= 0.0001 {
        1.0
    } else {
        (1.0 - next_remaining / projectile.total_ticks).clamp(0.0, 1.0)
    };
    let start_x = projectile.source_x + (projectile.target_x - projectile.source_x) * progress;
    let start_y = projectile.source_y + (projectile.target_y - projectile.source_y) * progress;
    let end_x = projectile.source_x + (projectile.target_x - projectile.source_x) * next_progress;
    let end_y = projectile.source_y + (projectile.target_y - projectile.source_y) * next_progress;
    let candidates: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block == FORCE_PROJECTOR_BLOCK && !force_broken(tile))
        .filter_map(|tile| {
            let x = (tile.position >> 16) as i16 as f32 * 8.0;
            let y = tile.position as i16 as f32 * 8.0;
            let radius = (FORCE_RADIUS + tile.output_liquid_amount * FORCE_PHASE_RADIUS_BOOST)
                * tile.transport_progress;
            (radius > 0.001
                && segment_intersects_circle(start_x, start_y, end_x, end_y, x, y, radius))
            .then_some((tile.position, (start_x - x).hypot(start_y - y)))
        })
        .collect();
    let Some((position, _)) = candidates
        .into_iter()
        .min_by(|left, right| left.1.total_cmp(&right.1))
    else {
        return false;
    };
    if let Some(mut projector) = world.tiles.get_mut(&position) {
        projector.production_progress += projectile.damage.max(0.0);
        let maximum =
            FORCE_SHIELD_HEALTH + FORCE_PHASE_SHIELD_BOOST * projector.output_liquid_amount;
        if projector.production_progress >= maximum {
            set_force_broken(&mut projector, true);
            projector.production_progress = FORCE_SHIELD_HEALTH;
        }
    }
    true
}

#[derive(Clone, Copy)]
pub(crate) struct PowerRole {
    pub(crate) production: f32,
    pub(crate) demand: f32,
    pub(crate) node_range: f32,
    pub(crate) battery_capacity: f32,
}
