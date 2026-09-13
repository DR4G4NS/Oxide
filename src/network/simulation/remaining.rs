//! Remaining audit-register simulation: fog, weather, map objectives,
//! map-area clamp, air-use-spawns placement, payload source/void,
//! tractor/point-defense turrets 356/359, ForceProjector explosion absorb.

use crate::game::content::{block_build_time, block_size, unit_movement};
use crate::network::outbound::FrameEmit;
use crate::network::world::{
    CarriedBuildPayload, CarriedPayload, ControlledUnit, DynamicTile, DynamicWorld, PendingBuild,
    SessionPlayer,
};
use crate::state::game_state::{MapObjectiveKind, WorldExtras};

pub(crate) const FORCE_PROJECTOR_BLOCK: i16 = 249;
pub(crate) const PAYLOAD_SOURCE: i16 = 416;
pub(crate) const PAYLOAD_VOID: i16 = 417;
pub(crate) const PARALLAX: i16 = 356;
pub(crate) const SEGMENT: i16 = 359;

/// Regular hexagon occupancy (ForceProjector.sides = 6).
pub(crate) fn in_regular_hexagon(
    cx: f32,
    cy: f32,
    radius: f32,
    rotation_deg: f32,
    px: f32,
    py: f32,
) -> bool {
    if radius <= 0.0 {
        return false;
    }
    let rot = rotation_deg.to_radians();
    let dx = px - cx;
    let dy = py - cy;
    let x = dx * rot.cos() + dy * rot.sin();
    let y = -dx * rot.sin() + dy * rot.cos();
    let qx = x.abs();
    let qy = y.abs();
    qx * 0.866_025_4 + qy * 0.5 <= radius && qy <= radius * 0.866_025_4 + 1e-3
}

pub(crate) fn force_projector_absorbs_explosion(
    world: &DynamicWorld,
    x: f32,
    y: f32,
    damage: f32,
) -> bool {
    let mut hit = None;
    for tile in world.tiles.iter() {
        if tile.block != FORCE_PROJECTOR_BLOCK {
            continue;
        }
        if crate::network::economy::force_broken(&tile) {
            continue;
        }
        let cx = (tile.position >> 16) as i16 as f32 * 8.0;
        let cy = tile.position as i16 as f32 * 8.0;
        let radius = 101.7 + tile.output_liquid_amount * 80.0;
        if in_regular_hexagon(cx, cy, radius, 0.0, x, y) {
            hit = Some(tile.position);
            break;
        }
    }
    let Some(position) = hit else {
        return false;
    };
    if let Some(mut projector) = world.tiles.get_mut(&position) {
        projector.production_progress += damage * 2.0;
    }
    true
}

pub(crate) fn flyer_spawn_world(
    world: &DynamicWorld,
    tile_x: i16,
    tile_y: i16,
    unit_type: i16,
) -> (f32, f32) {
    let flying = unit_movement(unit_type).flying;
    let air_use = world.wave_rules.read().air_use_spawns;
    if !flying || air_use {
        return (f32::from(tile_x) * 8.0, f32::from(tile_y) * 8.0);
    }
    let cx = world.width as f32 * 4.0;
    let cy = world.height as f32 * 4.0;
    let sx = f32::from(tile_x) * 8.0;
    let sy = f32::from(tile_y) * 8.0;
    let dx = sx - cx;
    let dy = sy - cy;
    let len = dx.hypot(dy).max(1.0);
    let span = world.width.max(world.height) as f32 * 8.0 * std::f32::consts::SQRT_2;
    // WaveSpawner.margin = 0: clamp to the map edge, not ±80 (ASTRA W03).
    let max_x = world.width as f32 * 8.0;
    let max_y = world.height as f32 * 8.0;
    let x = (cx + dx / len * span).clamp(0.0, max_x);
    let y = (cy + dy / len * span).clamp(0.0, max_y);
    (x, y)
}

pub(crate) fn simulate_remaining(
    world: &DynamicWorld,
    out: &dyn FrameEmit,
    delta: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let mut changed = false;
    changed |= tick_weather(world, delta);
    changed |= tick_fog(world);
    changed |= tick_objectives(world, out, delta);
    changed |= clamp_map_area(world);
    changed |= simulate_payload_source_void(world, delta);
    changed |= simulate_parallax_and_segment(world, delta, power);
    changed |= simulate_campaign_pads(world, delta, power);
    for (x, y, team) in world.game_state.extras.take_wall_lightning() {
        let mut best: Option<(i32, f32)> = None;
        for enemy in world.enemies.iter() {
            if enemy.team == team {
                continue;
            }
            let dist = (enemy.x - x).hypot(enemy.y - y);
            if dist <= 136.0 && best.is_none_or(|(_, d)| dist < d) {
                best = Some((enemy.id, dist));
            }
        }
        let Some((id, _)) = best else {
            continue;
        };
        let mut died = false;
        if let Some(mut enemy) = world.enemies.get_mut(&id) {
            enemy.health -= 20.0;
            died = enemy.health <= 0.0;
        }
        if died {
            crate::network::combat::kill_enemy(world, out, id);
            changed = true;
        }
    }
    for id in world.game_state.extras.take_unit_kills() {
        crate::network::combat::kill_enemy(world, out, id);
        changed = true;
    }
    for frame in world.game_state.extras.take_calls() {
        out.broadcast(frame);
        changed = true;
    }
    changed |= tick_ground_items(world, delta);
    changed
}

fn tick_ground_items(world: &DynamicWorld, delta: f32) -> bool {
    let drops = std::mem::take(&mut *world.game_state.extras.ground_items.lock());
    if drops.is_empty() {
        return false;
    }
    let units: Vec<(i32, f32, f32, f32)> = world
        .enemies
        .iter()
        .map(|unit| {
            let reach = crate::network::combat::unit_hit_size(unit.unit_type) + 4.0;
            (*unit.key(), unit.x, unit.y, reach)
        })
        .collect();
    let players: Vec<(i32, f32, f32)> = world
        .player_sessions
        .iter()
        .map(|session| (*session.key(), session.x, session.y))
        .collect();
    let mut changed = false;
    let mut kept = Vec::with_capacity(drops.len());
    for mut drop in drops {
        drop.life -= delta;
        if drop.life <= 0.0 {
            changed = true;
            continue;
        }
        let mut best_unit: Option<(i32, f32)> = None;
        for &(id, x, y, reach) in &units {
            let dist = (x - drop.x).hypot(y - drop.y);
            if dist <= reach && best_unit.is_none_or(|(_, d)| dist < d) {
                best_unit = Some((id, dist));
            }
        }
        let mut best_player: Option<(i32, f32)> = None;
        for &(id, x, y) in &players {
            let dist = (x - drop.x).hypot(y - drop.y);
            if dist <= 16.0 && best_player.is_none_or(|(_, d)| dist < d) {
                best_player = Some((id, dist));
            }
        }
        let picked = match (best_unit, best_player) {
            (Some((uid, ud)), Some((pid, pd))) if pd <= ud => {
                player_take_item(world, pid, drop.item, drop.amount)
                    || unit_take_item(world, uid, drop.item, drop.amount)
            }
            (Some((uid, _)), _) => unit_take_item(world, uid, drop.item, drop.amount),
            (_, Some((pid, _))) => player_take_item(world, pid, drop.item, drop.amount),
            _ => false,
        };
        if picked {
            changed = true;
        } else {
            kept.push(drop);
        }
    }
    world
        .game_state
        .extras
        .ground_items
        .lock()
        .append(&mut kept);
    changed
}

fn unit_take_item(world: &DynamicWorld, id: i32, item: i16, amount: i32) -> bool {
    let Some(mut unit) = world.enemies.get_mut(&id) else {
        return false;
    };
    if let Some(entry) = unit.items.iter_mut().find(|(id, _)| *id == item) {
        entry.1 = entry.1.saturating_add(amount);
    } else {
        unit.items.push((item, amount));
    }
    true
}

fn player_take_item(world: &DynamicWorld, id: i32, item: i16, amount: i32) -> bool {
    let Some(mut session) = world.player_sessions.get_mut(&id) else {
        return false;
    };
    if session.carried_item >= 0 && session.carried_item != item {
        return false;
    }
    session.carried_item = item;
    session.carried_amount = session.carried_amount.saturating_add(amount);
    true
}

fn tick_weather(world: &DynamicWorld, delta: f32) -> bool {
    let mut weather = world.game_state.extras.weather.write();
    if weather.is_empty() {
        return false;
    }
    for entry in weather.iter_mut() {
        entry.remaining = (entry.remaining - delta).max(0.0);
    }
    let before = weather.len();
    weather.retain(|entry| entry.remaining > 0.0);
    before != weather.len()
}

fn tick_fog(world: &DynamicWorld) -> bool {
    if !world.wave_rules.read().fog {
        return false;
    }
    let extras = &world.game_state.extras;
    extras.fog_visible.clear();
    for unit in world.enemies.iter() {
        reveal_around(world, extras, unit.team, unit.x, unit.y, 80.0);
    }
    for player in world.player_sessions.iter() {
        let team = world
            .player_profiles
            .get(&player.uuid)
            .map(|profile| profile.team)
            .unwrap_or(1);
        reveal_around(world, extras, team, player.x, player.y, 120.0);
    }
    true
}

fn reveal_around(world: &DynamicWorld, extras: &WorldExtras, team: u8, x: f32, y: f32, range: f32) {
    let tile_r = (range / 8.0).ceil() as i32;
    let tx = (x / 8.0).floor() as i32;
    let ty = (y / 8.0).floor() as i32;
    for dy in -tile_r..=tile_r {
        for dx in -tile_r..=tile_r {
            let nx = tx + dx;
            let ny = ty + dy;
            if nx < 0 || ny < 0 || nx >= world.width || ny >= world.height {
                continue;
            }
            if dx * dx + dy * dy > tile_r * tile_r {
                continue;
            }
            extras.fog_visible.insert((team, (nx << 16) | ny), ());
        }
    }
}

fn tick_objectives(world: &DynamicWorld, out: &dyn FrameEmit, delta: f32) -> bool {
    let extras = &world.game_state.extras;
    let wave = world
        .game_state
        .wave
        .load(std::sync::atomic::Ordering::Relaxed) as i32;
    let default_team = world.wave_rules.read().default_team;
    let timer_scale = world.wave_rules.read().objective_timer_multiplier.max(0.0);
    let mut objectives = extras.objectives.write();
    let mut changed = false;
    for (index, objective) in objectives.iter_mut().enumerate() {
        if objective.complete {
            continue;
        }
        if matches!(objective.kind, MapObjectiveKind::Timer { .. }) {
            objective.progress += delta * timer_scale;
        }
        let done = match objective.kind {
            MapObjectiveKind::WinWave(need) => wave >= need,
            MapObjectiveKind::DestroyCores => world
                .cores
                .iter()
                .filter(|core| *core.key() != default_team)
                .all(|core| core.health <= 0.0),
            MapObjectiveKind::Flag(hash) => extras.markers_json.read().contains(&hash.to_string()),
            MapObjectiveKind::Item { item, amount } => {
                team_item_count(world, default_team, item) >= amount
            }
            MapObjectiveKind::CoreItem { item, amount } => {
                world
                    .game_state
                    .game_stats
                    .read()
                    .core_item_count
                    .iter()
                    .find(|(id, _)| *id == item)
                    .map(|(_, count)| *count as i32)
                    .unwrap_or(0)
                    >= amount
            }
            MapObjectiveKind::BuildCount { block, count } => {
                world
                    .game_state
                    .game_stats
                    .read()
                    .placed_block_count
                    .iter()
                    .find(|(id, _)| *id == block)
                    .map(|(_, n)| *n as i32)
                    .unwrap_or(0)
                    >= count
            }
            MapObjectiveKind::UnitCount { unit, count } => {
                world
                    .enemies
                    .iter()
                    .filter(|enemy| enemy.team == default_team && enemy.unit_type == unit)
                    .count() as i32
                    >= count
            }
            MapObjectiveKind::DestroyUnits { count } => {
                world.game_state.game_stats.read().enemy_units_destroyed as i32 >= count
            }
            MapObjectiveKind::Timer { duration } => objective.progress >= duration,
            MapObjectiveKind::DestroyBlock { x, y, team, block } => {
                let pos = (i32::from(x) << 16) | (i32::from(y as u16));
                world.tiles.get(&pos).is_none_or(|tile| {
                    tile.block != block || tile.team != team || tile.health <= 0.0
                })
            }
            MapObjectiveKind::CommandMode => true,
        };
        if done {
            objective.complete = true;
            changed = true;
            if let Ok(frame) =
                crate::network::wire::calls::encode_complete_objective_frame(index as i32)
            {
                out.broadcast(frame);
            }
        }
    }
    if !objectives.is_empty() && objectives.iter().all(|o| o.complete) {
        if let Ok(frame) = crate::network::wire::calls::encode_sector_capture_frame() {
            out.broadcast(frame);
        }
    }
    changed
}

fn team_item_count(world: &DynamicWorld, team: u8, item: i16) -> i32 {
    if team == 1 {
        world
            .game_state
            .core_items
            .read()
            .get(item as usize)
            .copied()
            .unwrap_or(0)
    } else {
        world
            .game_state
            .team_items
            .get(&team)
            .and_then(|items| items.get(item as usize).copied())
            .unwrap_or(0)
    }
}

/// Parse `Rules.objectives` (MapObjectives JSON) from the map rules blob.
pub(crate) fn parse_map_objectives(
    rules_json: &str,
) -> Vec<crate::state::game_state::MapObjective> {
    use crate::state::game_state::MapObjective;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(rules_json) else {
        return Vec::new();
    };
    let Some(entries) = value
        .get("objectives")
        .or_else(|| value.get("mapObjectives"))
        .and_then(|node| node.get("all").or(Some(node)))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(parse_one_objective)
        .map(|kind| MapObjective {
            kind,
            complete: false,
            progress: 0.0,
        })
        .collect()
}

fn parse_one_objective(value: &serde_json::Value) -> Option<MapObjectiveKind> {
    let type_name = value
        .get("type")
        .or_else(|| value.get("class"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .rsplit(['.', '$'])
        .next()
        .unwrap_or("")
        .trim();
    let key = type_name.trim_end_matches("Objective").to_ascii_lowercase();
    match key.as_str() {
        "destroycore" | "destroycores" => Some(MapObjectiveKind::DestroyCores),
        "commandmode" | "command" => Some(MapObjectiveKind::CommandMode),
        "winwave" => Some(MapObjectiveKind::WinWave(
            value.get("wave").and_then(serde_json::Value::as_i64)? as i32,
        )),
        "flag" => {
            let flag = value.get("flag").and_then(serde_json::Value::as_str)?;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(flag, &mut hasher);
            Some(MapObjectiveKind::Flag(std::hash::Hasher::finish(&hasher)))
        }
        "item" => Some(MapObjectiveKind::Item {
            item: crate::logic::item_id_from_name(
                value.get("item").and_then(serde_json::Value::as_str)?,
            ),
            amount: value.get("amount").and_then(serde_json::Value::as_i64)? as i32,
        }),
        "coreitem" => Some(MapObjectiveKind::CoreItem {
            item: crate::logic::item_id_from_name(
                value.get("item").and_then(serde_json::Value::as_str)?,
            ),
            amount: value.get("amount").and_then(serde_json::Value::as_i64)? as i32,
        }),
        "buildcount" => Some(MapObjectiveKind::BuildCount {
            block: crate::game::block_names::block_id_from_name(
                value.get("block").and_then(serde_json::Value::as_str)?,
            )?,
            count: value.get("count").and_then(serde_json::Value::as_i64)? as i32,
        }),
        "unitcount" => Some(MapObjectiveKind::UnitCount {
            unit: crate::game::unit_types::unit_id_from_name(
                value.get("unit").and_then(serde_json::Value::as_str)?,
            )?,
            count: value.get("count").and_then(serde_json::Value::as_i64)? as i32,
        }),
        "destroyunits" => Some(MapObjectiveKind::DestroyUnits {
            count: value.get("count").and_then(serde_json::Value::as_i64)? as i32,
        }),
        "timer" => Some(MapObjectiveKind::Timer {
            duration: value
                .get("duration")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0) as f32,
        }),
        "destroyblock" => Some(MapObjectiveKind::DestroyBlock {
            x: value.get("x").and_then(serde_json::Value::as_i64)? as i16,
            y: value.get("y").and_then(serde_json::Value::as_i64)? as i16,
            team: u8::try_from(
                value
                    .get("team")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(2),
            )
            .unwrap_or(2),
            block: crate::game::block_names::block_id_from_name(
                value.get("block").and_then(serde_json::Value::as_str)?,
            )?,
        }),
        _ => None,
    }
}

fn clamp_map_area(world: &DynamicWorld) -> bool {
    let rules = world.wave_rules.read();
    if !rules.limit_map_area || rules.limit_width <= 0 || rules.limit_height <= 0 {
        return false;
    }
    let min_x = rules.limit_x as f32 * 8.0;
    let min_y = rules.limit_y as f32 * 8.0;
    let max_x = (rules.limit_x + rules.limit_width) as f32 * 8.0;
    let max_y = (rules.limit_y + rules.limit_height) as f32 * 8.0;
    drop(rules);
    let mut changed = false;
    for mut unit in world.enemies.iter_mut() {
        let nx = unit.x.clamp(min_x, max_x);
        let ny = unit.y.clamp(min_y, max_y);
        if (nx - unit.x).abs() + (ny - unit.y).abs() > 0.01 {
            unit.x = nx;
            unit.y = ny;
            changed = true;
        }
    }
    changed
}

fn simulate_payload_source_void(world: &DynamicWorld, delta: f32) -> bool {
    let keys: Vec<i32> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, PAYLOAD_SOURCE | PAYLOAD_VOID))
        .map(|tile| *tile.key())
        .collect();
    let mut changed = false;
    for key in keys {
        let Some(mut tile) = world.tiles.get_mut(&key) else {
            continue;
        };
        if tile.block == PAYLOAD_VOID {
            if tile.payload.take().is_some() || !tile.payload_inventory.is_empty() {
                tile.payload_inventory.clear();
                changed = true;
            }
            continue;
        }
        if !tile.enabled {
            continue;
        }
        tile.production_progress += delta;
        if tile.production_progress < 60.0 || tile.payload.is_some() {
            continue;
        }
        tile.production_progress = 0.0;
        let configured = configured_payload_block(&tile.config);
        if configured <= 0 {
            continue;
        }
        let mut ghost = ghost_tile(key, configured, tile.team);
        ghost.rotation = tile.rotation;
        tile.payload = Some(Box::new(CarriedPayload::Build(CarriedBuildPayload {
            tile: ghost,
            version: 0,
            sync: Vec::new(),
        })));
        changed = true;
    }
    changed
}

fn configured_payload_block(config: &[u8]) -> i16 {
    if config.len() >= 3 && config[0] == 6 {
        i16::from_be_bytes([config[1], config[2]])
    } else if config.len() >= 2 {
        i16::from_be_bytes([config[0], config[1]])
    } else {
        0
    }
}

fn ghost_tile(position: i32, block: i16, team: u8) -> DynamicTile {
    serde_json::from_value(serde_json::json!({
        "position": position,
        "block": block,
        "team": team,
        "rotation": 0,
        "config": []
    }))
    .expect("DynamicTile serde defaults")
}

fn simulate_parallax_and_segment(
    world: &DynamicWorld,
    delta: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<(i32, i16, u8)> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, PARALLAX | SEGMENT) && tile.enabled)
        .map(|tile| (*tile.key(), tile.block, tile.team))
        .collect();
    let mut changed = false;
    for (key, block, team) in keys {
        let efficiency = power.get(&key).copied().unwrap_or(0.0);
        if efficiency < 0.01 {
            continue;
        }
        let tx = (key >> 16) as i16 as f32 * 8.0;
        let ty = key as i16 as f32 * 8.0;
        if block == PARALLAX {
            let range = 300.0;
            let target = world
                .enemies
                .iter()
                .filter(|unit| unit.team != team && unit_movement(unit.unit_type).flying)
                .filter_map(|unit| {
                    let dist = (unit.x - tx).hypot(unit.y - ty);
                    (dist <= range && dist > 1.0).then_some((unit.id, dist, unit.x, unit.y))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1));
            if let Some((id, dist, ux, uy)) = target {
                let pull = (16.0 + 9.0 * (1.0 - dist / range)) * delta * efficiency;
                if let Some(mut unit) = world.enemies.get_mut(&id) {
                    unit.x += (tx - ux) / dist * pull;
                    unit.y += (ty - uy) / dist * pull;
                    unit.health = (unit.health - 0.5 * delta * efficiency).max(0.0);
                    changed = true;
                }
            }
            continue;
        }
        let range = 180.0;
        let mut best: Option<(i32, f32)> = None;
        for projectile in world.projectiles.iter() {
            if projectile.team == team {
                continue;
            }
            let px = (projectile.source_x + projectile.target_x) * 0.5;
            let py = (projectile.source_y + projectile.target_y) * 0.5;
            let dist = (px - tx).hypot(py - ty);
            if dist <= range && best.is_none_or(|(_, best_d)| dist < best_d) {
                best = Some((*projectile.key(), dist));
            }
        }
        let Some((id, _)) = best else {
            continue;
        };
        if let Some(mut turret) = world.tiles.get_mut(&key) {
            turret.production_progress += delta * efficiency;
            if turret.production_progress >= 8.0 {
                turret.production_progress = 0.0;
                drop(turret);
                if let Some(mut shot) = world.projectiles.get_mut(&id) {
                    shot.damage = (shot.damage - 30.0).max(0.0);
                    if shot.damage <= 0.0 {
                        shot.remaining_ticks = 0.0;
                    }
                    changed = true;
                }
            }
        }
    }
    changed
}

const LAUNCH_PAD: i16 = 425;
const ADVANCED_LAUNCH_PAD: i16 = 426;
const LANDING_PAD: i16 = 427;

fn simulate_campaign_pads(
    world: &DynamicWorld,
    delta: f32,
    power: &std::collections::HashMap<i32, f32>,
) -> bool {
    let keys: Vec<(i32, i16, u8)> = world
        .tiles
        .iter()
        .filter(|tile| matches!(tile.block, LAUNCH_PAD | ADVANCED_LAUNCH_PAD | LANDING_PAD))
        .map(|tile| (*tile.key(), tile.block, tile.team))
        .collect();
    let mut changed = false;
    let mut launched: Vec<(u8, Vec<(i16, i32)>)> = Vec::new();
    for (key, block, team) in &keys {
        let efficiency = power.get(key).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let Some(mut tile) = world.tiles.get_mut(key) else {
            continue;
        };
        match *block {
            LAUNCH_PAD | ADVANCED_LAUNCH_PAD => {
                if efficiency <= 0.01 {
                    continue;
                }
                let launch_time = if *block == LAUNCH_PAD { 1200.0 } else { 1800.0 };
                tile.production_progress =
                    (tile.production_progress + delta * efficiency).min(launch_time);
                let full = crate::network::economy::spec::inventory_total(&tile.inventory) >= 100;
                if tile.production_progress >= launch_time && full {
                    launched.push((*team, tile.inventory.clone()));
                    tile.inventory.clear();
                    tile.production_progress = 0.0;
                    changed = true;
                }
            }
            LANDING_PAD if tile.liquid_amount > 0.0 => {
                tile.production_progress = (tile.production_progress + delta).min(180.0);
                changed = true;
            }
            _ => {}
        }
    }
    for (team, cargo) in launched {
        let Some(pad) = keys
            .iter()
            .find(|(_, block, pad_team)| *block == LANDING_PAD && *pad_team == team)
            .map(|(position, _, _)| *position)
        else {
            continue;
        };
        if let Some(mut tile) = world.tiles.get_mut(&pad) {
            for (item, amount) in cargo {
                crate::network::economy::spec::inventory_add(&mut tile.inventory, item, amount);
            }
            tile.production_progress = 0.0;
            changed = true;
        }
    }
    changed
}

pub(crate) fn restore_pending_from_construct(world: &DynamicWorld) {
    let constructs: Vec<DynamicTile> = world
        .tiles
        .iter()
        .filter(|tile| (5..=20).contains(&tile.block))
        .map(|tile| tile.clone())
        .collect();
    for tile in constructs {
        // ConstructBuild.write: progress, previous.id, current.id.
        // DynamicTile: stored_item = previous, stored_amount = current.
        let current = i16::try_from(tile.stored_amount).unwrap_or(0);
        if current <= 0 {
            continue;
        }
        let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
        let build_time = (block_build_time(current) * cost_mult / 0.5).max(1.0);
        let remaining = (1.0 - tile.production_progress.clamp(0.0, 1.0)) * build_time;
        world.pending_builds.insert(
            tile.position,
            PendingBuild {
                position: tile.position,
                block: current,
                previous_block: tile.stored_item.max(0),
                rotation: tile.rotation,
                config: tile.config.clone(),
                occupied: tile.occupied.clone(),
                team: tile.team,
                builder: synthetic_builder(),
                last_seen: std::time::Instant::now(),
                assist_progress: 0.0,
                remaining_ticks: remaining,
                applied_assist: 0.0,
            },
        );
    }
}

pub(crate) fn inject_construct_tiles_from_pending(world: &DynamicWorld) {
    let pendings: Vec<_> = world
        .pending_builds
        .iter()
        .map(|pending| pending.value().clone())
        .collect();
    for pending in pendings {
        if world.tiles.contains_key(&pending.position) {
            continue;
        }
        let size = block_size(pending.block).max(1);
        let construct = 4i16 + i16::from(size).clamp(1, 16);
        let mut tile = ghost_tile(pending.position, construct, pending.team);
        tile.rotation = pending.rotation;
        tile.config = pending.config.clone();
        tile.occupied = pending.occupied.clone();
        tile.stored_item = pending.previous_block.max(0);
        tile.stored_amount = i32::from(pending.block);
        let cost_mult = world.wave_rules.read().build_cost_multiplier.max(0.0);
        let build_time = (block_build_time(pending.block) * cost_mult / 0.5).max(1.0);
        tile.production_progress = (1.0 - pending.remaining_ticks / build_time).clamp(0.0, 1.0);
        world.tiles.insert(pending.position, tile);
        for cell in &pending.occupied {
            world.tile_footprint.insert(*cell, pending.position);
        }
    }
}

fn synthetic_builder() -> SessionPlayer {
    SessionPlayer {
        id: -1,
        controlled_unit: ControlledUnit::Core,
        unit_id: -1,
        uuid: String::new(),
        name: String::new(),
        color: 0,
        last_snapshot: 0,
        x: 0.0,
        y: 0.0,
        mouse_x: 0.0,
        mouse_y: 0.0,
        rotation: 0.0,
        velocity_x: 0.0,
        velocity_y: 0.0,
        boosting: false,
        shooting: false,
        building: false,
        last_command: None,
        docked_type: None,
        active_plans: std::collections::HashSet::new(),
        mining_position: None,
        mining_progress: 0.0,
        mining_updated: std::time::Instant::now(),
        carried_item: -1,
        carried_amount: 0,
        preview_plan_group: -1,
        preview_plans: Vec::new(),
        last_shot: std::time::Instant::now(),
        admin: false,
        chat_rate: crate::network::wire::ChatRateLimiter::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexagon_contains_center_and_rejects_corner() {
        assert!(in_regular_hexagon(0.0, 0.0, 100.0, 0.0, 0.0, 0.0));
        assert!(in_regular_hexagon(0.0, 0.0, 100.0, 0.0, 50.0, 0.0));
        assert!(!in_regular_hexagon(0.0, 0.0, 100.0, 0.0, 200.0, 0.0));
    }

    #[test]
    fn parse_map_objectives_reads_destroy_core_item_and_timer() {
        let parsed = parse_map_objectives(
            r#"{"objectives":{"all":[
                {"type":"destroyCore"},
                {"type":"item","item":"copper","amount":50},
                {"type":"timer","duration":120}
            ]}}"#,
        );
        assert_eq!(parsed.len(), 3);
        assert!(matches!(parsed[0].kind, MapObjectiveKind::DestroyCores));
        assert!(matches!(
            parsed[1].kind,
            MapObjectiveKind::Item {
                item: 0,
                amount: 50
            }
        ));
        assert!(matches!(
            parsed[2].kind,
            MapObjectiveKind::Timer { duration } if (duration - 120.0).abs() < f32::EPSILON
        ));
    }
}
