//! Persistence: world snapshot/persist, PersistenceWorker, load_tiles, TypeIO
//! envelope codecs and construct-finish frames. The listener adapter
//! re-exports these through crate::network::listener::*.

use crate::network::codec::Writes;
use crate::network::combat::unit_combat::unit_hit_size;
use crate::network::world::*;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Error, ErrorKind};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::network::economy::*;
use crate::network::protocol::*;
use crate::network::units::*;
use crate::network::world::*;
use crate::state::game_state::{GameMode, GameState};
use dashmap::DashMap;
use tracing::{debug, info, warn};

use crate::network::buildings::construction::block_footprint_in;
use crate::network::decoders::{read_typeio_object_raw, BuildPlan};
pub(crate) fn apply_loaded_team_cores(world: &DynamicWorld, loaded: &LoadedWorld) {
    for core in &loaded.team_cores {
        crate::network::world::register_team_core(
            world,
            core.team,
            TeamCore {
                position: core.position,
                block: core.block,
                health: core.health,
                max_health: core.max_health,
            },
        );
    }
    if let Some(health) = loaded.core_health {
        if let Some(mut entry) = world.cores.get_mut(&1) {
            entry.health = health;
        } else {
            // Map without a sharded core (custom): synthesize the legacy one.
            crate::network::world::register_team_core(
                world,
                1,
                TeamCore {
                    position: world.core_position,
                    block: 339,
                    health,
                    max_health: world.core_max_health,
                },
            );
        }
    }
    if let Some(entry) = world.cores.get(&1) {
        *world.game_state.core_health.write() = entry.health;
    }
}

/// Applies the persisted per-team item inventories onto a world loaded from
/// a save: team 1 was already restored into `GameState.core_items` (the
/// legacy field), so only the OTHER teams are inserted into
/// `GameState.team_items` (a fresh save from a v<=10 file has none).
pub(crate) fn apply_loaded_team_items(world: &DynamicWorld, loaded: &LoadedWorld) {
    world.game_state.team_items.clear();
    for entry in &loaded.team_items {
        world
            .game_state
            .team_items
            .insert(entry.team, entry.items.clone());
    }
}

pub fn load_tiles(path: &Path, map_size: Option<(i32, i32)>) -> std::io::Result<LoadedWorld> {
    let (map_width, map_height) = map_size.unwrap_or((MAP_WIDTH, MAP_HEIGHT));
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(LoadedWorld {
                tiles: DashMap::new(),
                core_items: None,
                wave: None,
                wave_time: None,
                core_health: None,
                enemies: Vec::new(),
                base_building_health: Vec::new(),
                players: Vec::new(),
                building_commands: Vec::new(),
                unit_orders: Vec::new(),
                map_name: None,
                team_build_plans: crate::engine::typeio::TeamBlocks::default(),
                team_cores: Vec::new(),
                team_items: Vec::new(),
                simulation_time: None,
                logic_flags: Vec::new(),
                game_stats: crate::state::game_state::GameStats::default(),
                puddles: Vec::new(),
            })
        }
        Err(err) => return Err(err),
    };
    let saved: PersistedWorldCompat =
        serde_json::from_slice(&bytes).map_err(|err| Error::new(ErrorKind::InvalidData, err))?;
    let (
        saved,
        core_items,
        wave,
        wave_time,
        core_health,
        saved_enemies,
        base_building_health,
        saved_players,
        saved_building_commands,
        saved_unit_orders,
        map_name,
        team_build_plans,
        saved_team_cores,
        saved_team_items,
        saved_simulation_time,
        saved_logic_flags,
        saved_game_stats,
        saved_puddles,
    ) = match saved {
        PersistedWorldCompat::Current(mut saved) => {
            if !matches!(saved.version, 1..=14)
                || saved.core_items.len() > 256
                || saved.core_items.iter().any(|amount| *amount < 0)
                || saved.wave == 0
                || !saved.wave_time.is_finite()
                || saved.wave_time < 0.0
                || !saved.core_health.is_finite()
                || !(0.0..=6000.0).contains(&saved.core_health)
                || saved.base_building_health.len() > 4096
                || saved
                    .base_building_health
                    .iter()
                    .any(|entry| !entry.health.is_finite() || entry.health <= 0.0)
                || !saved.simulation_time.is_finite()
                || saved.simulation_time < 0.0
                || saved.logic_flags.len() > 4096
                || saved
                    .logic_flags
                    .iter()
                    .any(|(name, value)| name.len() > 64 || !value.is_finite())
                || saved.puddles.len() > 65536
                || saved.puddles.iter().any(|puddle| {
                    !puddle.amount.is_finite()
                        || !(0.0..=70.0).contains(&puddle.amount)
                        || !(0..12).contains(&puddle.liquid)
                        || puddle.entity_id < 0
                })
            {
                return Err(Error::new(
                    ErrorKind::InvalidData,
                    "invalid persisted world state",
                ));
            }
            // Versions 1..=7 used a temporary 10,000-of-every-item bootstrap.
            // Reset only unmistakable scaffold saves; preserve normal games.
            let legacy_bootstrap = saved.version <= 7
                && saved.core_items.len() == 20
                && saved.core_health <= 0.0
                && saved
                    .core_items
                    .iter()
                    .skip(1)
                    .all(|amount| *amount >= 9_000);
            if legacy_bootstrap {
                saved.core_items = GameState::initial_core_items();
                saved.wave = 1;
                // First wave at the default initial spacing (60 s), matching
                // the fresh-map policy instead of the old 3-second spawn.
                saved.wave_time = crate::network::units::DEFAULT_INITIAL_WAVE_SPACING;
                saved.core_health = 6_000.0;
                saved.enemies.clear();
                saved.players.clear();
            }
            // P0-10: migrate old checkpoints that smuggled the unit-factory
            // command as a `[254, command]` suffix inside `config` into the
            // typed `factory_command` field, so `config` is a pure TypeIO
            // object from here on and serializers never emit the marker.
            for tile in &mut saved.tiles {
                if matches!(tile.block, 377..=379) && tile.factory_command.is_none() {
                    if let Some(index) = tile
                        .config
                        .iter()
                        .position(|byte| *byte == FACTORY_COMMAND_MARKER)
                    {
                        let command = tile.config.get(index + 1).copied().filter(|c| *c <= 9);
                        tile.config.truncate(index);
                        tile.factory_command = command;
                    }
                }
            }
            (
                saved.tiles,
                Some(saved.core_items),
                Some(saved.wave),
                Some(saved.wave_time),
                Some(saved.core_health),
                saved.enemies,
                saved.base_building_health,
                saved.players,
                saved.building_commands,
                saved.unit_orders,
                (!saved.map_name.is_empty()).then_some(saved.map_name),
                saved.team_build_plans,
                saved.team_cores,
                saved.team_items,
                Some(saved.simulation_time),
                saved.logic_flags,
                saved.game_stats,
                saved.puddles,
            )
        }
        PersistedWorldCompat::Legacy(tiles) => (
            tiles,
            None,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            crate::engine::typeio::TeamBlocks::default(),
            Vec::new(),
            Vec::new(),
            None,
            Vec::new(),
            crate::state::game_state::GameStats::default(),
            Vec::new(),
        ),
    };
    let tiles = DashMap::new();
    for mut tile in saved {
        let x = (tile.position >> 16) as i16 as i32;
        let y = tile.position as i16 as i32;
        let payload_valid = tile.payload.as_deref_mut().is_none_or(|payload| {
            let accepted = if matches!(tile.block, 406 | 407) {
                matches!(payload, CarriedPayload::Build(build)
                    if decode_constructor_recipe(tile.block, &tile.config)
                        == Some(build.tile.block))
            } else {
                payload_block_limit(tile.block)
                    .is_some_and(|limit| payload_fits_limit(payload, limit))
                    && payload_block_accepts(tile.block, payload)
            };
            accepted && sanitize_standalone_payload(payload, tile.team)
        });
        let payload_accum_valid = if matches!(tile.block, 404 | 405) {
            tile.payload.as_deref().map_or_else(
                || tile.payload_accum.is_empty(),
                |payload| {
                    carried_payload_requirements(payload).is_some_and(|(_, requirements)| {
                        (tile.payload_accum.is_empty()
                            || tile.payload_accum.len() == requirements.len())
                            && tile
                                .payload_accum
                                .iter()
                                .all(|value| value.is_finite() && (0.0..1.0002).contains(value))
                    })
                },
            )
        } else {
            tile.payload_accum.is_empty()
        };
        let payload_progress_limit = match tile.block {
            398 | 399 => 45.0,
            400 | 401 => 35.0,
            402 => 220.0,
            403 => 230.0,
            404 | 405 => 1.0,
            408 | 409 => 1.0,
            _ => 0.0,
        };
        if (0..map_width).contains(&x)
            && (0..map_height).contains(&y)
            && (0..446).contains(&tile.block)
            && tile.rotation < 4
            && (-1..22).contains(&tile.stored_item)
            && (0..=1_000_000).contains(&tile.stored_amount)
            && tile.production_progress.is_finite()
            && tile.production_progress >= 0.0
            && tile.transport_progress.is_finite()
            && tile.transport_progress >= 0.0
            && tile.ammo_units.is_finite()
            && (0.0..=1000.0).contains(&tile.ammo_units)
            && tile.inventory.len() <= 64
            && tile
                .inventory
                .iter()
                .all(|(item, amount)| (0..22).contains(item) && (0..=1_000_000).contains(amount))
            && storage_capacity(tile.block)
                .is_none_or(|capacity| tile.inventory.iter().all(|(_, amount)| *amount <= capacity))
            && tile.power_stored.is_finite()
            && (0.0..=50_000.0).contains(&tile.power_stored)
            && (-1..12).contains(&tile.stored_liquid)
            && tile.liquid_amount.is_finite()
            && (0.0..=3_000.0).contains(&tile.liquid_amount)
            && tile.output_liquid_amount.is_finite()
            && (0.0..=3_000.0).contains(&tile.output_liquid_amount)
            && tile.junction_items.len() <= 24
            && tile
                .junction_items
                .iter()
                .all(|(direction, item, remaining)| {
                    *direction < 4
                        && (0..22).contains(item)
                        && remaining.is_finite()
                        && (0.0..=26.0).contains(remaining)
                })
            && tile.mass_driver_incoming.len() <= 64
            && tile
                .mass_driver_incoming
                .iter()
                .all(|(source, item, amount, remaining)| {
                    *source >= 0
                        && (0..22).contains(item)
                        && (1..=120).contains(amount)
                        && remaining.is_finite()
                        && (0.0..=200.0).contains(remaining)
                })
            && (tile.block == 271 || tile.mass_driver_incoming.is_empty())
            && tile.mass_driver_rotation.is_finite()
            && (0.0..360.0).contains(&tile.mass_driver_rotation)
            && tile.mass_driver_waiting.len() <= 64
            && tile
                .mass_driver_waiting
                .iter()
                .all(|position| *position >= 0)
            && (tile.block == 271 || tile.mass_driver_waiting.is_empty())
            && tile.payload_progress.is_finite()
            && (0.0..=payload_progress_limit).contains(&tile.payload_progress)
            && tile.payload_rotation.is_finite()
            && payload_valid
            && payload_accum_valid
            && (matches!(tile.block, 398..=409)
                || (tile.payload.is_none() && tile.payload_progress == 0.0))
            && tile.health.is_finite()
            && (tile.health == 0.0
                || (0.0..=crate::game::content::block_health(tile.block)).contains(&tile.health))
        {
            if tile.health == 0.0 {
                tile.health = crate::game::content::block_health(tile.block);
            }
            // Round 74: legacy saves can hold hundreds of items in one stack
            // conveyor stretch (the unbounded batch append before the
            // capacity cap). Truncate to the official itemCapacity so a
            // loaded world starts bounded and the client never renders a
            // 300-item stack.
            if matches!(tile.block, 259 | 279) && tile.conveyor_items.len() > 10 {
                tile.conveyor_items.truncate(10);
                tile.stored_amount = i32::try_from(tile.conveyor_items.len()).unwrap_or(i32::MAX);
            }
            if tile.stored_amount == 0 {
                tile.stored_item = -1;
            }
            if tile.liquid_amount <= 0.0001 {
                tile.stored_liquid = -1;
                tile.liquid_amount = 0.0;
            }
            if tile.occupied.is_empty() {
                tile.occupied =
                    block_footprint_in(map_width, map_height, tile.position, tile.block)
                        .unwrap_or_else(|| vec![tile.position]);
            }
            tiles.insert(tile.position, tile);
        } else {
            warn!("Ignored invalid persisted tile at {},{}", x, y);
        }
    }
    info!("Loaded {} persisted dynamic tiles", tiles.len());
    if saved_enemies.len() > 4096 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "too many persisted enemies",
        ));
    }
    let mut enemy_ids = HashSet::new();
    let mut enemies = Vec::with_capacity(saved_enemies.len());
    for mut enemy in saved_enemies {
        let Some(spec) = enemy_spec(enemy.unit_type) else {
            continue;
        };
        let (health_multiplier, speed_multiplier, damage_multiplier) =
            status_multipliers_composite(enemy.status_effect, &enemy.statuses);
        let mut candidate_ids = enemy_ids.clone();
        let valid = enemy.id > 0
            && candidate_ids.insert(enemy.id)
            && enemy.x.is_finite()
            && enemy.y.is_finite()
            && (-128.0..=(map_width as f32 * 8.0 + 128.0)).contains(&enemy.x)
            && (-128.0..=(map_height as f32 * 8.0 + 128.0)).contains(&enemy.y)
            && enemy.rotation.is_finite()
            && enemy.health.is_finite()
            && (0.0..=spec.health * health_multiplier).contains(&enemy.health)
            && enemy.shield.is_finite()
            && (0.0..=1_000_000.0).contains(&enemy.shield)
            && enemy.elevation.is_finite()
            && (0.0..=1.0).contains(&enemy.elevation)
            && enemy.attack_reload.is_finite()
            && enemy.attack_reload >= 0.0
            && enemy.secondary_attack_reload.is_finite()
            && enemy.secondary_attack_reload >= 0.0
            && enemy.tertiary_attack_reload.is_finite()
            && enemy.tertiary_attack_reload >= 0.0
            && matches!(enemy.status_effect, -1 | 1 | 10 | 13..=15 | 18)
            && enemy.status_duration.is_finite()
            && enemy.status_duration >= 0.0
            && sanitize_unit_payloads(&mut enemy, &mut candidate_ids, 0);
        if !valid {
            warn!("Ignored invalid persisted enemy {}", enemy.id);
            continue;
        }
        enemy_ids = candidate_ids;
        enemy.entity_class = spec.entity_class;
        enemy.velocity_x = 0.0;
        enemy.velocity_y = 0.0;
        enemy.move_speed = spec.speed * speed_multiplier;
        enemy.attack_damage = spec.attack_damage * damage_multiplier;
        enemy.attack_reload_time = spec.attack_reload;
        enemy.attack_range = spec.attack_range;
        enemies.push(enemy);
    }
    info!("Loaded {} persisted enemies", enemies.len());
    if saved_players.len() > 1024 {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "too many persisted player profiles",
        ));
    }
    let mut player_uuids = HashSet::new();
    let mut players = Vec::with_capacity(saved_players.len());
    for mut player in saved_players {
        let valid = !player.uuid.is_empty()
            && player.uuid.len() <= 256
            && player_uuids.insert(player.uuid.clone())
            && player.x.is_finite()
            && player.y.is_finite()
            && (-128.0..=(map_width as f32 * 8.0 + 128.0)).contains(&player.x)
            && (-128.0..=(map_height as f32 * 8.0 + 128.0)).contains(&player.y)
            && player.health.is_finite()
            && (0.0..=150.0).contains(&player.health)
            && player.shield.is_finite()
            && (0.0..=10_000.0).contains(&player.shield)
            && matches!(player.status_effect, -1 | 1 | 8 | 9 | 10 | 14 | 18)
            && player.status_duration.is_finite()
            && (0.0..=360_000.0).contains(&player.status_duration)
            && player.respawn_timer.is_finite()
            && (0.0..=3600.0).contains(&player.respawn_timer);
        if !valid {
            warn!("Ignored invalid persisted player profile");
            continue;
        }
        if player.health <= 0.0 {
            player.dead = true;
        }
        if player.status_duration == 0.0 {
            player.status_effect = -1;
            player.statuses.clear();
            crate::network::units::StatusContainer::clear_statuses(&mut player);
        }
        players.push(player);
    }
    info!("Loaded {} persisted player profiles", players.len());
    let building_commands = saved_building_commands
        .into_iter()
        .filter(|command| {
            let x = (command.position >> 16) as i16 as i32;
            let y = command.position as i16 as i32;
            (0..map_width).contains(&x)
                && (0..map_height).contains(&y)
                && command.target_x.is_finite()
                && command.target_y.is_finite()
        })
        .collect();
    let unit_orders = saved_unit_orders
        .into_iter()
        .filter(|order| {
            order.unit_id > 0
                && enemy_ids.contains(&order.unit_id)
                && order.command <= 9
                && order.stances & !((1_u32 << 30) - 1) == 0
                && order.payload_cooldown.is_finite()
                && (0.0..=60.0).contains(&order.payload_cooldown)
                && order.target_kind <= 2
                && order.queue.len() <= 100
                && order.queue.iter().all(|target| {
                    target.kind <= 2
                        && target.id >= -1
                        && target.x.is_finite()
                        && target.y.is_finite()
                })
                && match (order.target_x, order.target_y) {
                    (None, None) => true,
                    (Some(x), Some(y)) => x.is_finite() && y.is_finite(),
                    _ => false,
                }
        })
        .collect();
    // Persisted per-team cores: valid teams (1..=250, 0/2 excluded), packed
    // positions inside the map and finite health within the block maximum.
    let team_cores: Vec<PersistedTeamCore> = saved_team_cores
        .into_iter()
        .filter(|core| {
            (1..=250).contains(&core.team)
                && core.team != 2
                && (core.position >> 16) as i16 as i32 >= 0
                && core.position as i16 as i32 >= 0
                && ((core.position >> 16) as i16 as i32) < map_width
                && (core.position as i16 as i32) < map_height
                && (339..=344).contains(&core.block)
                && core.health.is_finite()
                && (0.0..=core.max_health).contains(&core.health)
                && core.max_health.is_finite()
                && core.max_health > 0.0
        })
        .collect();
    // Persisted per-team item inventories: valid teams (1..=250, 0/2
    // excluded — team 1 lives in `core_items`), sane lengths and amounts.
    let team_items: Vec<PersistedTeamItems> = saved_team_items
        .into_iter()
        .filter(|entry| {
            (1..=250).contains(&entry.team)
                && entry.team != 2
                && entry.items.len() <= 256
                && entry.items.iter().all(|amount| *amount >= 0)
        })
        .collect();
    Ok(LoadedWorld {
        tiles,
        core_items,
        wave,
        wave_time,
        core_health,
        enemies,
        base_building_health,
        players,
        building_commands,
        unit_orders,
        map_name,
        team_build_plans,
        team_cores,
        team_items,
        simulation_time: saved_simulation_time,
        logic_flags: saved_logic_flags,
        game_stats: saved_game_stats,
        puddles: saved_puddles,
    })
}

pub(crate) fn sanitize_standalone_payload(payload: &mut CarriedPayload, team: u8) -> bool {
    match payload {
        CarriedPayload::Unit(unit) => {
            let Some(spec) = enemy_spec(unit.unit_type) else {
                return false;
            };
            let mut ids = HashSet::new();
            if unit.id <= 0
                || !ids.insert(unit.id)
                || unit.team != team
                || !unit.health.is_finite()
                || !(0.0..=spec.health).contains(&unit.health)
                || !sanitize_unit_payloads(unit, &mut ids, 1)
            {
                return false;
            }
            unit.entity_class = spec.entity_class;
            unit.move_speed = spec.speed;
            unit.attack_damage = spec.attack_damage;
            unit.attack_reload_time = spec.attack_reload;
            unit.attack_range = spec.attack_range;
            true
        }
        CarriedPayload::Build(build) => {
            build.tile.team == team
                && build.tile.block > 0
                && build.sync.len() <= 32 * 1024
                && build.version <= 32
        }
    }
}

pub(crate) fn sanitize_unit_payloads(
    unit: &mut EnemyUnit,
    ids: &mut HashSet<i32>,
    depth: u8,
) -> bool {
    if depth >= 4 || unit.payloads.len() > 32 {
        return unit.payloads.is_empty();
    }
    if !unit.payloads.is_empty() && payload_capacity(unit.unit_type) <= 0.0 {
        return false;
    }
    let mut used = 0.0;
    for payload in &mut unit.payloads {
        let CarriedPayload::Unit(payload) = payload else {
            let CarriedPayload::Build(build) = payload else {
                unreachable!();
            };
            let size = crate::game::content::block_size(build.tile.block);
            used += f32::from(size * 8).powi(2);
            if build.tile.team != unit.team
                || build.tile.block <= 0
                || build.sync.len() > 32 * 1024
                || build.version > 32
            {
                return false;
            }
            continue;
        };
        let Some(spec) = enemy_spec(payload.unit_type) else {
            return false;
        };
        used += unit_hit_size(payload.unit_type).powi(2);
        if payload.id <= 0
            || !ids.insert(payload.id)
            || payload.team != unit.team
            || !payload.x.is_finite()
            || !payload.y.is_finite()
            || !payload.rotation.is_finite()
            || !payload.health.is_finite()
            || !(0.0..=spec.health).contains(&payload.health)
            || !payload.shield.is_finite()
            || !(0.0..=1_000_000.0).contains(&payload.shield)
            || !payload.elevation.is_finite()
            || !(0.0..=1.0).contains(&payload.elevation)
            || !sanitize_unit_payloads(payload, ids, depth + 1)
        {
            return false;
        }
        payload.entity_class = spec.entity_class;
        payload.move_speed = spec.speed;
        payload.attack_damage = spec.attack_damage;
        payload.attack_reload_time = spec.attack_reload;
        payload.attack_range = spec.attack_range;
    }
    used <= payload_capacity(unit.unit_type) + 0.001
}

pub trait CorePersistenceSource {
    fn snapshot(&self) -> Vec<PersistedTeamCore>;
}

impl CorePersistenceSource for &DashMap<u8, TeamCore> {
    fn snapshot(&self) -> Vec<PersistedTeamCore> {
        self.iter()
            .map(|entry| PersistedTeamCore {
                team: *entry.key(),
                position: entry.value().position,
                block: entry.value().block,
                health: entry.value().health,
                max_health: entry.value().max_health,
            })
            .collect()
    }
}

impl CorePersistenceSource for (&DashMap<u8, TeamCore>, &DashMap<u8, Vec<TeamCore>>) {
    fn snapshot(&self) -> Vec<PersistedTeamCore> {
        let mut result = Vec::new();
        for entry in self.1.iter() {
            result.extend(entry.value().iter().map(|core| PersistedTeamCore {
                team: *entry.key(),
                position: core.position,
                block: core.block,
                health: core.health,
                max_health: core.max_health,
            }));
        }
        // Legacy-only worlds may not have initialized the list map.
        if result.is_empty() {
            result.extend(self.0.snapshot());
        }
        result
    }
}

/// P0-8: snapshots the live world into a serializable `PersistedWorld`
/// WITHOUT doing any I/O. The snapshot is consistent (callers hold the
/// world lock) and cheap enough to build on the tick; the actual serialize/
/// write/fsync runs on the persistence worker thread.
#[allow(clippy::too_many_arguments)]
pub fn snapshot_persisted_world(
    tiles: &DashMap<i32, DynamicTile>,
    state: &GameState,
    enemies: &DashMap<i32, EnemyUnit>,
    base_buildings: &DashMap<i32, BaseBuildingState>,
    player_profiles: &DashMap<String, PlayerCombatState>,
    building_commands: &DashMap<i32, BuildingCommand>,
    unit_orders: &DashMap<i32, UnitOrder>,
    team_build_plans: &crate::engine::typeio::TeamBlocks,
    cores: impl CorePersistenceSource,
    logic_flags: &DashMap<String, f64>,
    puddles: &crate::network::buildings::puddles::PuddleSystem,
) -> PersistedWorld {
    let mut snapshot: Vec<_> = tiles.iter().map(|tile| tile.value().clone()).collect();
    snapshot.sort_unstable_by_key(|tile| tile.position);
    let mut enemy_snapshot: Vec<_> = enemies.iter().map(|enemy| enemy.value().clone()).collect();
    enemy_snapshot.sort_unstable_by_key(|enemy| enemy.id);
    let mut base_building_health: Vec<_> = base_buildings
        .iter()
        .filter_map(|building| {
            let maximum = crate::game::content::block_health(building.block);
            (building.health < maximum).then_some(PersistedBaseBuildingHealth {
                position: building.position,
                health: building.health,
            })
        })
        .collect();
    base_building_health.sort_unstable_by_key(|building| building.position);
    let mut players: Vec<_> = player_profiles
        .iter()
        .map(|player| player.value().clone())
        .collect();
    players.sort_unstable_by(|left, right| left.uuid.cmp(&right.uuid));
    let mut building_command_snapshot: Vec<_> = building_commands
        .iter()
        .map(|command| command.value().clone())
        .collect();
    building_command_snapshot.sort_unstable_by_key(|command| command.position);
    let mut unit_order_snapshot: Vec<_> = unit_orders
        .iter()
        .map(|order| order.value().clone())
        .collect();
    unit_order_snapshot.sort_unstable_by_key(|order| order.unit_id);
    let mut team_cores = cores.snapshot();
    team_cores.sort_unstable_by_key(|core| core.team);
    // Per-team item inventories (revision 11): team 1 stays in the legacy
    // `core_items` field above; every other team is snapshotted here. Sorted
    // snapshot first, no DashMap guards held on the serialized data.
    let mut team_items: Vec<PersistedTeamItems> = state
        .team_items
        .iter()
        .filter(|entry| *entry.key() != 1)
        .map(|entry| PersistedTeamItems {
            team: *entry.key(),
            items: entry.value().clone(),
        })
        .collect();
    team_items.sort_unstable_by_key(|entry| entry.team);
    let mut logic_flags: Vec<_> = logic_flags
        .iter()
        .map(|entry| (entry.key().clone(), *entry.value()))
        .collect();
    logic_flags.sort_by(|left, right| left.0.cmp(&right.0));
    // Authoritative puddles (revision 14, round 73): sorted snapshot, no
    // DashMap guards held on the serialized data.
    let mut puddle_snapshot: Vec<PersistedPuddle> = puddles
        .puddles
        .iter()
        .map(|entry| PersistedPuddle {
            position: *entry.key(),
            liquid: entry.value().liquid,
            amount: entry.value().amount,
            entity_id: entry.value().entity_id,
        })
        .collect();
    puddle_snapshot.sort_unstable_by_key(|puddle| puddle.position);
    let saved = PersistedWorld {
        version: 14,
        map_name: state.map_name.read().clone(),
        tiles: snapshot,
        core_items: state.core_items.read().clone(),
        wave: state.wave.load(Ordering::Relaxed),
        wave_time: *state.wave_time.read(),
        core_health: *state.core_health.read(),
        // Game-over is intentionally NOT persisted (ephemeral runtime
        // state, official server behavior): a restart boots a live game.
        enemies: enemy_snapshot,
        base_building_health,
        players,
        building_commands: building_command_snapshot,
        unit_orders: unit_order_snapshot,
        team_build_plans: team_build_plans.clone(),
        team_cores,
        team_items,
        simulation_time: *state.simulation_time.read(),
        logic_flags,
        game_stats: state.game_stats.read().clone(),
        puddles: puddle_snapshot,
    };
    saved
}

/// P0-8: durable atomic persistence of a world snapshot: serialize to a
/// unique temp file, `fsync` the file, atomically rename it over the target
/// and `fsync` the parent directory so the rename itself survives a crash.
/// Returns the number of bytes written (diagnostics/tests).
pub fn persist_world_sync(path: &Path, saved: &PersistedWorld) -> std::io::Result<u64> {
    let bytes = serde_json::to_vec_pretty(saved).map_err(Error::other)?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    // Unique temp name per call (process + atomic counter): parallel tests
    // and concurrent saves never collide on a shared .tmp file, so the
    // rename stays atomic (persist_tiles is called without the world lock
    // from finish_pending_build/finish_pending_break).
    static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = format!(
        "{}.tmp{}-{}",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = path.with_file_name(unique);
    std::fs::write(&temporary, &bytes)?;
    // Durable atomicity: flush the file contents before the rename...
    {
        let file = std::fs::File::open(&temporary)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    // ...and flush the directory entry after it (Unix). Best-effort on
    // platforms without directory fsync.
    #[cfg(unix)]
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    Ok(bytes.len() as u64)
}

/// P0-8: one persistence job handed to the worker thread: a fully
/// snapshotted world plus the destination path.
pub struct PersistJob {
    pub path: PathBuf,
    pub world: PersistedWorld,
}

/// P0-8: dedicated persistence worker that keeps serialization and file I/O
/// OFF the world tick. Snapshots are built on the tick (consistent, under
/// the world lock) and submitted here; the single worker serializes writes
/// so the last submitted snapshot always wins and the target file is never
/// written concurrently. The worker is detached: a failed save logs a
/// warning and the next periodic save retries.
pub struct PersistenceWorker {
    sender: std::sync::mpsc::Sender<PersistJob>,
}

impl PersistenceWorker {
    pub fn spawn() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<PersistJob>();
        std::thread::Builder::new()
            .name("persistence-worker".into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    if let Err(err) = persist_world_sync(&job.path, &job.world) {
                        warn!(
                            "Persistence worker could not save {}: {}",
                            job.path.display(),
                            err
                        );
                    }
                }
            })
            .expect("persistence worker thread spawns");
        PersistenceWorker { sender }
    }

    /// Submits a snapshot for durable persistence. Returns false when the
    /// worker has stopped (channel closed); the caller logs and retries.
    pub fn submit(&self, job: PersistJob) -> bool {
        self.sender.send(job).is_ok()
    }
}

/// P0-8: synchronous convenience API (tests + runtime commands): snapshot
/// the world and durably persist it on the calling thread.
#[allow(clippy::too_many_arguments)]
pub fn persist_tiles(
    path: &Path,
    tiles: &DashMap<i32, DynamicTile>,
    state: &GameState,
    enemies: &DashMap<i32, EnemyUnit>,
    base_buildings: &DashMap<i32, BaseBuildingState>,
    player_profiles: &DashMap<String, PlayerCombatState>,
    building_commands: &DashMap<i32, BuildingCommand>,
    unit_orders: &DashMap<i32, UnitOrder>,
    team_build_plans: &crate::engine::typeio::TeamBlocks,
    cores: impl CorePersistenceSource,
    logic_flags: &DashMap<String, f64>,
    puddles: &crate::network::buildings::puddles::PuddleSystem,
) -> std::io::Result<()> {
    let saved = snapshot_persisted_world(
        tiles,
        state,
        enemies,
        base_buildings,
        player_profiles,
        building_commands,
        unit_orders,
        team_build_plans,
        cores,
        logic_flags,
        puddles,
    );
    persist_world_sync(path, &saved).map(|_| ())
}

pub(crate) fn valid_build_position(world: &DynamicWorld, position: i32) -> bool {
    // Round 74e: the official NetServer.clientSnapshot accepts the WHOLE
    // synced plan queue with NO distance validation (the client can place
    // anywhere it can see). The legacy BUILD_RANGE gate (220 u = 27.5
    // tiles from the snapshot position) silently rejected far plans, so a
    // long conveyor line placed across the map left the far end as ghosts
    // forever — "a veces sí a veces no" depending on distance. Precedent:
    // round 72j removed the same gate from tileConfig/rotateBlock.
    let x = (position >> 16) as i16 as i32;
    let y = position as i16 as i32;
    x >= 0 && y >= 0 && x < world.width && y < world.height
}

pub(crate) fn encode_construct_finish(
    player: &SessionPlayer,
    plan: &BuildPlan,
    rotation: u8,
    team: u8,
) -> std::io::Result<Vec<u8>> {
    encode_construct_finish_for_unit(
        player.unit_id,
        plan.position,
        plan.block,
        rotation,
        team,
        &plan.config,
    )
}

pub(crate) fn encode_construct_finish_for_unit(
    unit_id: i32,
    position: i32,
    block: i16,
    rotation: u8,
    team: u8,
    config: &[u8],
) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let mut payload = Vec::new();
    payload.write_i(position)?;
    payload.write_s(block)?;
    payload.write_b(2)?; // normal unit reference
    payload.write_i(unit_id)?;
    payload.write_b(rotation)?;
    payload.write_b(team)?;
    payload.extend_from_slice(&outbound_typeio_object(config));
    Ok(payload)
}

/// Returns exactly one structurally valid `TypeIO.writeObject` value for an
/// outbound packet field.
///
/// Runtime `DynamicTile::config` predates the strict TypeIO codec and can also
/// contain a decoded building tail or a private suffix used by the simulation.
/// Forwarding that blob verbatim lets the desktop interpret its first byte as
/// an object tag; e.g. a map tail beginning with 127 crashed build 158.1 in
/// `ConstructFinishCallPacket.handled`. Keep the first validated object (which
/// strips private suffixes) and encode null for empty/legacy-invalid state.
/// This compatibility boundary must never emit an unvalidated object tag.
pub(crate) fn outbound_typeio_object(config: &[u8]) -> Vec<u8> {
    let mut input = std::io::Cursor::new(config);
    read_typeio_object_raw(&mut input).unwrap_or_else(|_| vec![0])
}

pub fn decode_typeio_string(payload: &[u8]) -> std::io::Result<String> {
    use crate::network::codec::Reads;
    let mut cursor = std::io::Cursor::new(payload);
    cursor
        .read_typeio_string()?
        .ok_or_else(|| Error::new(ErrorKind::InvalidData, "null chat message"))
}

pub fn encode_typeio_string(value: &str) -> std::io::Result<Vec<u8>> {
    use crate::network::codec::Writes;
    let mut payload = Vec::new();
    payload.write_typeio_string(Some(value))?;
    Ok(payload)
}
