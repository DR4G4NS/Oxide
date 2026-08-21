//! Damage application: player/pierce/rail/splash/EMP, core damage,
//! kill_enemy and unit-death paths.

use crate::network::buildings::construction::dynamic_at;
use crate::network::combat::enemy::{damage_building, hostile_unit_count};
use crate::network::units::mining::heal_building_for_team;
use crate::network::wire::bootstrap::emit_game_over_packet_with_winner;
use crate::network::wire::encode::{
    encode_build_destroyed_frame, encode_build_health_update_frame, encode_enemy_entity_snapshots,
    encode_initial_entity_snapshot, frame_generated_packet,
};
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

pub(crate) fn damage_player(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    player_id: i32,
    damage: f32,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    let Some(mut player) = world.players.get_mut(&player_id) else {
        return false;
    };
    if player.dead {
        return false;
    }
    let absorbed = player.shield.min(damage);
    player.shield -= absorbed;
    player.health = (player.health - (damage - absorbed)).max(0.0);
    if status_effect >= 0 && status_duration > 0.0 {
        // A6: players also carry the StatusEntry collection; the legacy
        // fields mirror the first entry (apply_status).
        crate::network::units::StatusContainer::apply_status(
            &mut *player,
            status_effect,
            status_duration,
        );
    }
    let died = player.health <= 0.0;
    let final_session = if died {
        player.dead = true;
        player.respawn_timer = 60.0;
        // UnitDeath invokes MechUnit.destroy() immediately. Send a reliable
        // final sync with the pair (0, 0) before the RPC; mutating only the
        // server session is insufficient when the last periodic snapshot is
        // still queued or was produced by an older server build.
        world
            .player_sessions
            .get_mut(&player.unit_id)
            .map(|mut session| {
                session.carried_item = -1;
                session.carried_amount = 0;
                session.clone()
            })
    } else {
        None
    };
    let profile = player.clone();
    let uuid = profile.uuid.clone();
    let unit_id = profile.unit_id;
    drop(player);

    if died {
        if let Some(session) = final_session {
            if let Ok(snapshot) = encode_initial_entity_snapshot(&session, Some(&profile)) {
                if let Ok(frame) =
                    frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true)
                {
                    out.broadcast(frame);
                }
            }
        }
        let mut payload = Vec::with_capacity(4);
        use crate::network::codec::Writes;
        if payload.write_i(unit_id).is_ok() {
            if let Ok(frame) = frame_generated_packet(UNIT_DEATH_PACKET_ID, &payload, false) {
                out.broadcast(frame);
            }
        }
    }
    world.player_profiles.insert(uuid, profile);
    world.persistence_dirty.store(true, Ordering::Relaxed);
    true
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_enemy_pierce_player_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    cap: u8,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    let mut targets: Vec<_> = world
        .players
        .iter()
        .filter(|player| !player.dead)
        .filter_map(|player| {
            let (distance, progress) =
                point_segment_distance(player.x, player.y, source_x, source_y, target_x, target_y);
            (distance <= 8.0).then_some((progress, *player.key()))
        })
        .collect();
    targets.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
    targets
        .into_iter()
        .take(usize::from(cap))
        .fold(false, |changed, (_, id)| {
            damage_player(world, out, id, damage, status_effect, status_duration) || changed
        })
}

#[derive(Clone, Copy)]
pub(crate) enum EnemyPierceTarget {
    Player(i32),
    Building(i32),
}

pub(crate) fn point_hits_segment(
    point_x: f32,
    point_y: f32,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    radius: f32,
) -> Option<f32> {
    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= f32::EPSILON {
        return ((point_x - source_x).hypot(point_y - source_y) <= radius).then_some(0.0);
    }
    let progress = ((point_x - source_x) * dx + (point_y - source_y) * dy) / length_squared;
    if !(0.0..=1.0).contains(&progress) {
        return None;
    }
    let closest_x = source_x + dx * progress;
    let closest_y = source_y + dy * progress;
    ((point_x - closest_x).hypot(point_y - closest_y) <= radius).then_some(progress)
}

#[derive(Clone, Copy)]
pub(crate) enum AlliedPierceTarget {
    Unit(i32),
    Building(i32),
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_allied_pierce_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    initial_damage: f32,
    pierce_cap: u8,
    pierce_buildings: bool,
    damage_factor: f32,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    apply_allied_pierce_damage_for_team(
        world,
        out,
        1,
        source_x,
        source_y,
        target_x,
        target_y,
        initial_damage,
        pierce_cap,
        pierce_buildings,
        damage_factor,
        status_effect,
        status_duration,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_allied_pierce_damage_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    initial_damage: f32,
    pierce_cap: u8,
    pierce_buildings: bool,
    damage_factor: f32,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    let mut targets: Vec<(f32, AlliedPierceTarget)> = world
        .enemies
        .iter()
        .filter(|unit| unit.team != team)
        .filter_map(|unit| {
            point_hits_segment(unit.x, unit.y, source_x, source_y, target_x, target_y, 8.0)
                .map(|progress| (progress, AlliedPierceTarget::Unit(unit.id)))
        })
        .collect();
    if pierce_buildings {
        let mut seen = HashSet::new();
        targets.extend(
            world
                .tiles
                .iter()
                .filter(|tile| tile.block != 0 && tile.team != team && seen.insert(tile.position))
                .filter_map(|tile| {
                    let x = (tile.position >> 16) as i16 as f32 * 8.0;
                    let y = tile.position as i16 as f32 * 8.0;
                    point_hits_segment(x, y, source_x, source_y, target_x, target_y, 7.0)
                        .map(|progress| (progress, AlliedPierceTarget::Building(tile.position)))
                }),
        );
        targets.extend(world.base_buildings.iter().filter_map(|building| {
            if building.team == team || !seen.insert(building.position) {
                return None;
            }
            let x = (building.position >> 16) as i16 as f32 * 8.0;
            let y = building.position as i16 as f32 * 8.0;
            point_hits_segment(x, y, source_x, source_y, target_x, target_y, 7.0)
                .map(|progress| (progress, AlliedPierceTarget::Building(building.position)))
        }));
    }
    targets.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));

    let mut damage = initial_damage;
    let mut changed = false;
    for (_, target) in targets.into_iter().take(usize::from(pierce_cap)) {
        match target {
            AlliedPierceTarget::Unit(id) => {
                let mut dead = false;
                if let Some(mut unit) = world.enemies.get_mut(&id) {
                    let dealt = apply_incoming_unit_damage(&unit, damage, 1.0);
                    let absorbed = unit.shield.min(dealt);
                    unit.shield -= absorbed;
                    unit.health = (unit.health - (dealt - absorbed)).max(0.0);
                    if status_effect >= 0
                        && status_duration > 0.0
                        && !unit_immune_to_status(unit.unit_type, status_effect)
                    {
                        // A6: stack into the StatusEntry collection.
                        crate::network::units::StatusContainer::apply_status(
                            &mut *unit,
                            status_effect,
                            status_duration,
                        );
                    }
                    dead = unit.health <= 0.0;
                    changed = true;
                }
                if dead {
                    kill_enemy(world, out, id);
                }
            }
            AlliedPierceTarget::Building(position) => {
                if let Some((destroyed, health)) = damage_building(world, position, damage) {
                    if destroyed {
                        if let Ok(frame) = encode_build_destroyed_frame(position) {
                            out.broadcast(frame);
                        }
                    } else if let Ok(frame) =
                        encode_build_health_update_frame(&[(position, health)])
                    {
                        out.broadcast(frame);
                    }
                    changed = true;
                }
            }
        }
        damage *= damage_factor;
    }
    changed
}

#[derive(Clone, Copy)]
pub(crate) enum EnemyRailTarget {
    Player(i32),
    Building(i32),
    Core,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_enemy_rail_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    target_core: bool,
    initial_damage: f32,
    pierce_damage_factor: f32,
) -> bool {
    let mut targets: Vec<(f32, EnemyRailTarget)> = world
        .players
        .iter()
        .filter(|player| !player.dead)
        .filter_map(|player| {
            let (distance, progress) =
                point_segment_distance(player.x, player.y, source_x, source_y, target_x, target_y);
            (distance <= 8.0).then_some((progress, EnemyRailTarget::Player(*player.key())))
        })
        .collect();
    let mut seen = HashSet::new();
    targets.extend(
        world
            .tiles
            .iter()
            .filter(|tile| {
                tile.block != 0
                    && tile.team == 1
                    && tile.position != world.core_position
                    && seen.insert(tile.position)
            })
            .filter_map(|tile| {
                let x = (tile.position >> 16) as i16 as f32 * 8.0;
                let y = tile.position as i16 as f32 * 8.0;
                let (distance, progress) =
                    point_segment_distance(x, y, source_x, source_y, target_x, target_y);
                (distance <= 7.0).then_some((progress, EnemyRailTarget::Building(tile.position)))
            }),
    );
    targets.extend(world.base_buildings.iter().filter_map(|building| {
        if building.team != 1
            || building.position == world.core_position
            || !seen.insert(building.position)
        {
            return None;
        }
        let x = (building.position >> 16) as i16 as f32 * 8.0;
        let y = building.position as i16 as f32 * 8.0;
        let (distance, progress) =
            point_segment_distance(x, y, source_x, source_y, target_x, target_y);
        (distance <= 7.0).then_some((progress, EnemyRailTarget::Building(building.position)))
    }));
    if target_core {
        targets.push((1.0, EnemyRailTarget::Core));
    }
    targets.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));

    let mut damage = initial_damage;
    let mut changed = false;
    for (_, target) in targets {
        changed |= match target {
            EnemyRailTarget::Player(id) => damage_player(world, out, id, damage, -1, 0.0),
            EnemyRailTarget::Building(position) => {
                apply_enemy_direct_damage(world, out, Some(position), false, damage)
            }
            EnemyRailTarget::Core => apply_enemy_direct_damage(world, out, None, true, damage),
        };
        damage *= pierce_damage_factor;
    }
    changed
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_enemy_shared_pierce_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    cap: u8,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    let mut targets: Vec<(f32, EnemyPierceTarget)> = world
        .players
        .iter()
        .filter(|player| !player.dead)
        .filter_map(|player| {
            let (distance, progress) =
                point_segment_distance(player.x, player.y, source_x, source_y, target_x, target_y);
            (distance <= 8.0).then_some((progress, EnemyPierceTarget::Player(*player.key())))
        })
        .collect();
    let mut seen = HashSet::new();
    targets.extend(
        world
            .tiles
            .iter()
            .filter(|tile| tile.block != 0 && tile.team == 1 && seen.insert(tile.position))
            .filter_map(|tile| {
                let x = (tile.position >> 16) as i16 as f32 * 8.0;
                let y = tile.position as i16 as f32 * 8.0;
                let (distance, progress) =
                    point_segment_distance(x, y, source_x, source_y, target_x, target_y);
                (distance <= 7.0).then_some((progress, EnemyPierceTarget::Building(tile.position)))
            }),
    );
    targets.extend(world.base_buildings.iter().filter_map(|building| {
        if building.team != 1 || !seen.insert(building.position) {
            return None;
        }
        let x = (building.position >> 16) as i16 as f32 * 8.0;
        let y = building.position as i16 as f32 * 8.0;
        let (distance, progress) =
            point_segment_distance(x, y, source_x, source_y, target_x, target_y);
        (distance <= 7.0).then_some((progress, EnemyPierceTarget::Building(building.position)))
    }));
    targets.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
    targets
        .into_iter()
        .take(usize::from(cap))
        .fold(false, |changed, (_, target)| {
            let hit = match target {
                EnemyPierceTarget::Player(id) => {
                    damage_player(world, out, id, damage, status_effect, status_duration)
                }
                EnemyPierceTarget::Building(position) => {
                    apply_enemy_direct_damage(world, out, Some(position), false, damage)
                }
            };
            hit || changed
        })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_enemy_pierce_building_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    cap: u8,
) -> bool {
    let mut seen = HashSet::new();
    let mut targets: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == 1 && seen.insert(tile.position))
        .filter_map(|tile| {
            let x = (tile.position >> 16) as i16 as f32 * 8.0;
            let y = tile.position as i16 as f32 * 8.0;
            let (distance, progress) =
                point_segment_distance(x, y, source_x, source_y, target_x, target_y);
            (distance <= 7.0).then_some((progress, tile.position, x, y))
        })
        .collect();
    targets.extend(world.base_buildings.iter().filter_map(|building| {
        if building.team != 1 || !seen.insert(building.position) {
            return None;
        }
        let x = (building.position >> 16) as i16 as f32 * 8.0;
        let y = building.position as i16 as f32 * 8.0;
        let (distance, progress) =
            point_segment_distance(x, y, source_x, source_y, target_x, target_y);
        (distance <= 7.0).then_some((progress, building.position, x, y))
    }));
    targets.sort_unstable_by(|left, right| left.0.total_cmp(&right.0));
    let mut changed = false;
    for (_, position, _, _) in targets.into_iter().take(usize::from(cap)) {
        changed |= apply_enemy_direct_damage(world, out, Some(position), false, damage);
    }
    changed
}

pub(crate) fn point_segment_distance(
    point_x: f32,
    point_y: f32,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
) -> (f32, f32) {
    let dx = target_x - source_x;
    let dy = target_y - source_y;
    let length_squared = dx * dx + dy * dy;
    let progress = if length_squared <= 0.0001 {
        0.0
    } else {
        (((point_x - source_x) * dx + (point_y - source_y) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let closest_x = source_x + dx * progress;
    let closest_y = source_y + dy * progress;
    ((point_x - closest_x).hypot(point_y - closest_y), progress)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_reign_fragments(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    source_id: i32,
    parent_source_x: f32,
    parent_source_y: f32,
    x: f32,
    y: f32,
) {
    let base_angle = (y - parent_source_y)
        .atan2(x - parent_source_x)
        .to_degrees();
    for (angle_offset, velocity_scale, lifetime_scale) in
        [(-30.0, 0.4, 0.34), (0.0, 0.7, 0.67), (30.0, 1.0, 1.0)]
    {
        let angle = base_angle + angle_offset;
        let radians = angle.to_radians();
        let lifetime = 20.0 * lifetime_scale;
        let distance = 9.0 * velocity_scale * lifetime;
        let target_x = x + radians.cos() * distance;
        let target_y = y + radians.sin() * distance;
        let id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
        world.projectiles.insert(
            id,
            Projectile {
                target_id: source_id,
                shooter_id: source_id,
                team,
                bullet_id: 13,
                damage: 20.0,
                splash_damage: 15.0,
                splash_radius: 10.0,
                status_effect: -1,
                status_duration: 0.0,
                pierce_units: 3,
                pierce_buildings: 3,
                spawn_reign_frags: false,
                homing_range: 0.0,
                enemy_target_position: None,
                enemy_target_core: false,
                apply_direct_on_impact: false,
                armor_multiplier: 1.0,
                remaining_ticks: lifetime,
                total_ticks: lifetime,
                source_x: x,
                source_y: y,
                target_x,
                target_y,
                lifetime_scale,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        if let Ok(payload) = encode_create_bullet_payload(
            13,
            team,
            x,
            y,
            angle,
            20.0,
            velocity_scale,
            lifetime_scale,
        ) {
            if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
                out.broadcast(frame);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_cyerce_fragments(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    source_id: i32,
    parent_source_x: f32,
    parent_source_y: f32,
    x: f32,
    y: f32,
) {
    let base_angle = (y - parent_source_y)
        .atan2(x - parent_source_x)
        .to_degrees();
    for index in 0..7u8 {
        let fraction = f32::from(index) / 6.0;
        let angle = base_angle - 60.0 + fraction * 120.0;
        let velocity_scale = 0.3 + fraction * 0.7;
        let radians = angle.to_radians();
        let lifetime = 60.0;
        let distance = 3.9 * velocity_scale * lifetime;
        let target_x = x + radians.cos() * distance;
        let target_y = y + radians.sin() * distance;
        let id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
        world.projectiles.insert(
            id,
            Projectile {
                target_id: source_id,
                shooter_id: source_id,
                team,
                bullet_id: 57,
                damage: 11.0,
                splash_damage: 13.0,
                splash_radius: 20.0,
                status_effect: -1,
                status_duration: 0.0,
                pierce_units: 0,
                pierce_buildings: 0,
                spawn_reign_frags: false,
                homing_range: 50.0,
                enemy_target_position: None,
                enemy_target_core: false,
                apply_direct_on_impact: true,
                armor_multiplier: 1.0,
                remaining_ticks: lifetime,
                total_ticks: lifetime,
                source_x: x,
                source_y: y,
                target_x,
                target_y,
                lifetime_scale: 1.0,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        if let Ok(payload) =
            encode_create_bullet_payload(57, 2, x, y, angle, 11.0, velocity_scale, 1.0)
        {
            if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
                out.broadcast(frame);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_toxopid_fragments(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    target_id: i32,
    shooter_id: i32,
    parent_source_x: f32,
    parent_source_y: f32,
    x: f32,
    y: f32,
) -> bool {
    // Official toxopid-cannon fragBullet (UnitTypes.java): ArtilleryBulletType
    // (2.3, 30), lifetime 90, splash 40/70, sapped 600; 9 fragments with
    // fragRandomSpread 360, fragVelocityMin/Max 0.2..1.0 and fragLifeMin 0.3.
    // The official per-frag random angle is replaced by a deterministic
    // full-circle spread (documented deviation).
    let base_angle = (y - parent_source_y)
        .atan2(x - parent_source_x)
        .to_degrees();
    for index in 0..9u8 {
        let fraction = f32::from(index) / 9.0;
        let angle = base_angle + fraction * 360.0;
        let velocity_scale = 0.2 + fraction * 0.8;
        let lifetime_scale = 0.3 + fraction * 0.7;
        let radians = angle.to_radians();
        let lifetime = 90.0 * lifetime_scale;
        let distance = 2.3 * velocity_scale * lifetime;
        let target_x = x + radians.cos() * distance;
        let target_y = y + radians.sin() * distance;
        let id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
        world.projectiles.insert(
            id,
            Projectile {
                target_id,
                shooter_id,
                team,
                bullet_id: 29,
                damage: 30.0,
                splash_damage: 40.0,
                splash_radius: 70.0,
                status_effect: 9,
                status_duration: 600.0,
                pierce_units: 0,
                pierce_buildings: 0,
                spawn_reign_frags: false,
                homing_range: 0.0,
                enemy_target_position: None,
                enemy_target_core: false,
                apply_direct_on_impact: true,
                armor_multiplier: 1.0,
                remaining_ticks: lifetime,
                total_ticks: lifetime,
                source_x: x,
                source_y: y,
                target_x,
                target_y,
                lifetime_scale,
                source_position: None,
                damage_interval: None,
                damage_timer: 0.0,
            },
        );
        if let Ok(payload) = encode_create_bullet_payload(
            29,
            team,
            x,
            y,
            angle,
            30.0,
            velocity_scale,
            lifetime_scale,
        ) {
            if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
                out.broadcast(frame);
            }
        }
    }
    true
}

pub(crate) fn building_exists(world: &DynamicWorld, position: i32) -> bool {
    dynamic_at(world, position).is_some_and(|tile| tile.block != 0 && tile.team == 1)
        || world
            .base_buildings
            .get(&position)
            .is_some_and(|building| building.team == 1)
}

pub(crate) fn nearest_player_building_in_range(
    world: &DynamicWorld,
    x: f32,
    y: f32,
    range: f32,
) -> Option<(i32, f32, f32)> {
    let mut nearest: Option<(f32, i32, f32, f32)> = None;
    for tile in world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == 1)
    {
        let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
        let target_y = tile.position as i16 as f32 * 8.0;
        let distance = (target_x - x).hypot(target_y - y);
        if distance <= range && nearest.is_none_or(|current| distance < current.0) {
            nearest = Some((distance, tile.position, target_x, target_y));
        }
    }
    for building in world
        .base_buildings
        .iter()
        .filter(|building| building.team == 1)
    {
        let target_x = (building.position >> 16) as i16 as f32 * 8.0;
        let target_y = building.position as i16 as f32 * 8.0;
        let distance = (target_x - x).hypot(target_y - y);
        if distance <= range && nearest.is_none_or(|current| distance < current.0) {
            nearest = Some((distance, building.position, target_x, target_y));
        }
    }
    nearest.map(|(_, position, x, y)| (position, x, y))
}

/// Applies `damage` to the core of `team` and returns whether this hit
/// destroyed it. The team-1 entry mirrors `GameState.core_health` so the
/// snapshots and economy heal paths keep working unchanged.
///
/// Game over:
/// - survival/attack: destroying team 1 ends the game with winner = waveTeam
///   (crux, 2), exactly like the pre-existing single-core behaviour;
/// - PvP: the destroyed team is eliminated; when at most one player team
///   still has a live core the game ends and that team wins (official
///   Logic.checkGameState "last team standing").
///
/// `Rules.canGameOver=false` suppresses the game-over flag in both modes.
pub(crate) fn damage_team_core(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    damage: f32,
) -> bool {
    if damage <= 0.0 {
        return false;
    }
    let target = crate::network::world::team_core_snapshot(world, team)
        .first()
        .copied();
    let target_position = target
        .map(|core| core.position)
        .unwrap_or(world.core_position);
    let destroyed = if team == 1 {
        let mut health = world.game_state.core_health.write();
        let previous = *health;
        *health = (*health - damage).max(0.0);
        if let Some(mut cores) = world.team_core_lists.get_mut(&1) {
            if let Some(core) = cores
                .iter_mut()
                .find(|core| core.position == target_position)
            {
                core.health = *health;
            }
        }
        if let Some(mut legacy) = world.cores.get_mut(&1) {
            legacy.health = *health;
        } else {
            crate::network::world::register_team_core(
                world,
                1,
                TeamCore {
                    position: target_position,
                    block: 339,
                    health: *health,
                    max_health: world.core_max_health,
                },
            );
        }
        previous > 0.0 && *health <= 0.0
    } else {
        let position = target_position;
        let previous = target
            .map(|core| core.health)
            .unwrap_or_else(|| *world.game_state.core_health.read());
        if let Some(mut cores) = world.team_core_lists.get_mut(&team) {
            if let Some(core) = cores.iter_mut().find(|core| core.position == position) {
                core.health = (core.health - damage).max(0.0);
            }
        } else if let Some(mut core) = world.cores.get_mut(&team) {
            core.health = (core.health - damage).max(0.0);
        } else {
            crate::network::world::register_team_core(
                world,
                team,
                TeamCore {
                    position,
                    block: 339,
                    health: (previous - damage).max(0.0),
                    max_health: world.core_max_health,
                },
            );
        }
        previous > 0.0
            && crate::network::world::team_core_snapshot(world, team)
                .first()
                .is_some_and(|core| core.health <= 0.0)
    };
    // A destroyed core is removed from the ordered topology. Remaining cores
    // keep the team active and retain the shared team inventory.
    if destroyed {
        {
            crate::network::world::unregister_team_core(world, team, target_position);
            if team == 1 {
                *world.game_state.core_health.write() =
                    crate::network::world::team_core_snapshot(world, 1)
                        .first()
                        .map(|core| core.health)
                        .unwrap_or(0.0);
            }
        }
    }
    if !destroyed {
        return false;
    }
    info!("Core of team {} destroyed", team);
    if *world.game_state.mode.read() == crate::state::game_state::GameMode::Sandbox {
        // Round 74d: sandbox has no game over (official Gamemode.sandbox
        // never rotates maps). The legacy port set game_over here, so a
        // sandbox session re-hosted the map every round_wait_ticks (12 s) —
        // builds reset mid-construction, belts restarted, power relinked
        // and the world stream encode stalled the tick (ping spikes).
        return true;
    }
    if *world.game_state.mode.read() == crate::state::game_state::GameMode::Pvp
        && crate::network::world::team_core_snapshot(world, team).is_empty()
    {
        if let Some(winner) = pvp_elimination_winner(world) {
            if world.wave_rules.read().can_game_over {
                world.game_state.game_over.store(true, Ordering::Relaxed);
                emit_game_over_packet_with_winner(world, out, winner);
            }
        }
    } else if !crate::network::world::team_core_snapshot(world, team).is_empty() {
        // Destroying one of several cores is not team elimination and cannot
        // trigger game-over (the remaining cores retain TeamData.active).
        return true;
    } else if team == 1 {
        // Player (sharded) is defeated only after its last core is destroyed.
        if world.wave_rules.read().can_game_over {
            world.game_state.game_over.store(true, Ordering::Relaxed);
        }
        emit_game_over_packet_with_winner(world, out, 2);
    } else if *world.game_state.mode.read() == crate::state::game_state::GameMode::Attack
        && !registered_core_teams(world)
            .into_iter()
            .any(|t| t != 0 && t != 1 && core_health_for_team(world, t) > 0.0)
    {
        // Attack: the last registered enemy core fell -> player victory.
        if world.wave_rules.read().can_game_over {
            world.game_state.game_over.store(true, Ordering::Relaxed);
        }
        emit_game_over_packet_with_winner(world, out, 1);
    } else {
        // Any other non-PvP core destruction (including the shared-core
        // fallback for maps without enemy cores) is the player-side defeat.
        if world.wave_rules.read().can_game_over {
            world.game_state.game_over.store(true, Ordering::Relaxed);
        }
        emit_game_over_packet_with_winner(world, out, 2);
    }
    true
}

/// Heals `team`'s core by `amount` (clamped to its maximum). Mirrors
/// `GameState.core_health` for team 1. Returns whether health changed.
pub(crate) fn heal_team_core(world: &DynamicWorld, team: u8, amount: f32) -> bool {
    if amount <= 0.0 {
        return false;
    }
    if team == 1 {
        let mut health = world.game_state.core_health.write();
        let previous = *health;
        *health = (*health + amount).min(world.core_max_health);
        if let Some(mut entry) = world.cores.get_mut(&1) {
            entry.health = *health;
        }
        *health > previous
    } else if let Some(mut entry) = world.cores.get_mut(&team) {
        let previous = entry.health;
        entry.health = (entry.health + amount).min(entry.max_health);
        entry.health > previous
    } else {
        false
    }
}

/// PvP elimination check after a team's core was destroyed: the game is over
/// when at most one player team still has a live core. The winner is that
/// surviving team; when no player team survives (shared-core fallback) the
/// wave team (2) is reported so every client shows the defeat dialog.
/// Returns `None` while two or more player teams remain alive.
pub(crate) fn pvp_elimination_winner(world: &DynamicWorld) -> Option<u8> {
    let mut teams: HashSet<u8> = world
        .players
        .iter()
        .map(|entry| entry.value().team)
        .filter(|team| *team != 0)
        .collect();
    if teams.is_empty() {
        // No connected players: every registered core team counts (the wave
        // enemy team 2 is excluded — it is not a player team in PvP).
        teams.extend(
            registered_core_teams(world)
                .into_iter()
                .filter(|team| *team != 0 && *team != 2),
        );
    }
    let alive: Vec<u8> = teams
        .iter()
        .copied()
        .filter(|team| core_health_for_team(world, *team) > 0.0)
        .collect();
    if alive.len() > 1 {
        return None;
    }
    alive.first().copied().or(Some(2))
}

pub(crate) fn apply_enemy_direct_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    target_position: Option<i32>,
    target_core: bool,
    damage: f32,
) -> bool {
    if let Some(position) = target_position {
        if let Some((destroyed, health)) = damage_building(world, position, damage) {
            let frame = if destroyed {
                encode_build_destroyed_frame(position)
            } else {
                encode_build_health_update_frame(&[(position, health)])
            };
            if let Ok(frame) = frame {
                out.broadcast(frame);
            }
            return true;
        }
    }
    if target_core {
        // The wave enemy attacks the sharded core (team 1). Per-team core
        // damage + game-over handling live in `damage_team_core`, which also
        // mirrors team-1 health into GameState.core_health.
        damage_team_core(world, out, 1, damage);
        return true;
    }
    false
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_allied_splash_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    x: f32,
    y: f32,
    damage: f32,
    radius: f32,
    unit_damage_scale: f32,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    apply_allied_splash_damage_for_team(
        world,
        out,
        1,
        x,
        y,
        damage,
        radius,
        unit_damage_scale,
        status_effect,
        status_duration,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_allied_splash_damage_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    x: f32,
    y: f32,
    damage: f32,
    radius: f32,
    unit_damage_scale: f32,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    let ids: Vec<_> = world
        .enemies
        .iter()
        .filter(|unit| unit.team != team && (unit.x - x).hypot(unit.y - y) <= radius)
        .map(|unit| unit.id)
        .collect();
    let mut dead = Vec::new();
    let mut changed = false;
    for id in ids {
        if let Some(mut unit) = world.enemies.get_mut(&id) {
            let scaled = damage * unit_damage_scale;
            let dealt = apply_incoming_unit_damage(&unit, scaled, 1.0);
            let absorbed = unit.shield.min(dealt);
            unit.shield -= absorbed;
            unit.health = (unit.health - (dealt - absorbed)).max(0.0);
            if status_effect >= 0
                && status_duration > 0.0
                && !unit_immune_to_status(unit.unit_type, status_effect)
            {
                // A6: stack into the StatusEntry collection.
                crate::network::units::StatusContainer::apply_status(
                    &mut *unit,
                    status_effect,
                    status_duration,
                );
            }
            if unit.health <= 0.0 {
                dead.push(id);
            }
            changed = true;
        }
    }
    for id in dead {
        kill_enemy(world, out, id);
    }
    let mut seen = HashSet::new();
    let mut positions: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team != team && seen.insert(tile.position))
        .filter_map(|tile| {
            let tile_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let tile_y = tile.position as i16 as f32 * 8.0;
            ((tile_x - x).hypot(tile_y - y) <= radius).then_some(tile.position)
        })
        .collect();
    positions.extend(world.base_buildings.iter().filter_map(|building| {
        if building.team == team || !seen.insert(building.position) {
            return None;
        }
        let building_x = (building.position >> 16) as i16 as f32 * 8.0;
        let building_y = building.position as i16 as f32 * 8.0;
        ((building_x - x).hypot(building_y - y) <= radius).then_some(building.position)
    }));
    for position in positions {
        if let Some((destroyed, health)) = damage_building(world, position, damage) {
            if destroyed {
                if let Ok(frame) = encode_build_destroyed_frame(position) {
                    out.broadcast(frame);
                }
            } else if let Ok(frame) = encode_build_health_update_frame(&[(position, health)]) {
                out.broadcast(frame);
            }
            changed = true;
        }
    }
    changed
}

/// Quad compatibility wrapper. BulletType.createSplashDamage heals damaged
/// allied buildings; unit healing is intentionally not part of that method.
pub(crate) fn apply_quad_bomb_heal(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    x: f32,
    y: f32,
    radius: f32,
) -> bool {
    apply_splash_building_heal_for_team(world, out, 1, x, y, radius, 15.0)
}

pub(crate) fn apply_quad_bomb_heal_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    x: f32,
    y: f32,
    radius: f32,
) -> bool {
    apply_splash_building_heal_for_team(world, out, team, x, y, radius, 15.0)
}

pub(crate) fn apply_splash_building_heal_for_team(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    x: f32,
    y: f32,
    radius: f32,
    heal_percent: f32,
) -> bool {
    let mut changed = false;
    let mut seen = HashSet::new();
    let dynamic: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == team && seen.insert(tile.position))
        .filter_map(|tile| {
            let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let target_y = tile.position as i16 as f32 * 8.0;
            ((target_x - x).hypot(target_y - y) <= radius).then_some(tile.position)
        })
        .collect();
    let base: Vec<_> = world
        .base_buildings
        .iter()
        .filter(|building| building.team == team && seen.insert(building.position))
        .filter_map(|building| {
            let target_x = (building.position >> 16) as i16 as f32 * 8.0;
            let target_y = building.position as i16 as f32 * 8.0;
            ((target_x - x).hypot(target_y - y) <= radius).then_some(building.position)
        })
        .collect();
    for position in dynamic.into_iter().chain(base) {
        if let Some(health) = heal_building_for_team(world, position, team, heal_percent, 0.0) {
            if let Ok(frame) = encode_build_health_update_frame(&[(position, health)]) {
                out.broadcast(frame);
            }
            changed = true;
        }
    }
    changed
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_enemy_splash_damage(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    x: f32,
    y: f32,
    damage: f32,
    radius: f32,
    unit_damage_scale: f32,
    status_effect: i16,
    status_duration: f32,
) -> bool {
    let mut targets: HashSet<i32> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && tile.team == 1)
        .filter_map(|tile| {
            let target_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let target_y = tile.position as i16 as f32 * 8.0;
            ((target_x - x).hypot(target_y - y) <= radius).then_some(tile.position)
        })
        .collect();
    targets.extend(world.base_buildings.iter().filter_map(|building| {
        let target_x = (building.position >> 16) as i16 as f32 * 8.0;
        let target_y = building.position as i16 as f32 * 8.0;
        ((target_x - x).hypot(target_y - y) <= radius).then_some(building.position)
    }));
    let mut destroyed = Vec::new();
    let mut health_updates = Vec::new();
    for position in targets {
        if let Some((is_destroyed, health)) = damage_building(world, position, damage) {
            if is_destroyed {
                destroyed.push(position);
            } else {
                health_updates.push((position, health));
            }
        }
    }
    if !health_updates.is_empty() {
        if let Ok(frame) = encode_build_health_update_frame(&health_updates) {
            out.broadcast(frame);
        }
    }
    for position in &destroyed {
        if let Ok(frame) = encode_build_destroyed_frame(*position) {
            out.broadcast(frame);
        }
    }
    let (core_x, core_y) = core_world(world);
    let core_hit = (core_x - x).hypot(core_y - y) <= radius;
    if core_hit {
        // The wave enemy's splash reaches the sharded core (team 1);
        // per-team damage + game over live in damage_team_core.
        damage_team_core(world, out, 1, damage);
    }
    let mut player_hit = false;
    let player_ids: Vec<_> = world
        .players
        .iter()
        .filter(|player| !player.dead && (player.x - x).hypot(player.y - y) <= radius)
        .map(|player| *player.key())
        .collect();
    for player_id in player_ids {
        player_hit |= damage_player(
            world,
            out,
            player_id,
            damage * unit_damage_scale,
            status_effect,
            status_duration,
        );
    }
    core_hit || player_hit || !destroyed.is_empty() || !health_updates.is_empty()
}

// Navanax EmpBulletType (158.1): radius = 100, healPercent = 20,
// timeIncrease = 3, timeDuration = 60 * 20 = 1200, powerDamageScl = 3,
// damage = 60 (so the power strike deals 60 * 3 = 180).
pub(crate) const EMP_HEAL_PERCENT: f32 = 20.0;
pub(crate) const EMP_TIME_INCREASE: f32 = 3.0;
pub(crate) const EMP_TIME_DURATION: f32 = 1200.0;
pub(crate) const EMP_POWER_DAMAGE_SCL: f32 = 3.0;

/// Building-side Navanax EMP effects (bullet_id 60), applied once per impact on
/// top of the regular splash: allied power buildings are healed for
/// healPercent/100 * maxHealth and boosted timeIncrease x for timeDuration ticks,
/// while enemy power buildings take damage * powerDamageScl. Mirrors
/// EmpBulletType.hit; enemy units are already handled by the splash step
/// (unitDamageScl 0.8 + electrified) and are not touched here.
pub(crate) fn apply_emp_bullet_effects(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    team: u8,
    x: f32,
    y: f32,
    radius: f32,
    damage: f32,
) -> bool {
    let mut changed = false;
    let mut seen = HashSet::new();
    let mut positions: Vec<_> = world
        .tiles
        .iter()
        .filter(|tile| tile.block != 0 && seen.insert(tile.position))
        .filter_map(|tile| {
            let tile_x = (tile.position >> 16) as i16 as f32 * 8.0;
            let tile_y = tile.position as i16 as f32 * 8.0;
            ((tile_x - x).hypot(tile_y - y) <= radius).then_some(tile.position)
        })
        .collect();
    positions.extend(world.base_buildings.iter().filter_map(|building| {
        if !seen.insert(building.position) {
            return None;
        }
        let building_x = (building.position >> 16) as i16 as f32 * 8.0;
        let building_y = building.position as i16 as f32 * 8.0;
        ((building_x - x).hypot(building_y - y) <= radius).then_some(building.position)
    }));
    for position in positions {
        let (building_team, block) = world
            .tiles
            .get(&position)
            .map(|tile| (tile.team, tile.block))
            .or_else(|| {
                world
                    .base_buildings
                    .get(&position)
                    .map(|building| (building.team, building.block))
            })
            .unwrap_or((0, 0));
        if block == 0 || power_role(block).is_none() {
            continue;
        }
        if building_team == team {
            if let Some(health) =
                heal_building_for_team(world, position, team, EMP_HEAL_PERCENT, 0.0)
            {
                if let Ok(frame) = encode_build_health_update_frame(&[(position, health)]) {
                    out.broadcast(frame);
                }
                changed = true;
            }
            if block_can_emp_boost(block)
                && building_time_scale(world, position) < EMP_TIME_INCREASE
            {
                world
                    .overdrive_boosts
                    .entry(position)
                    .and_modify(|boost| {
                        boost.multiplier = boost.multiplier.max(EMP_TIME_INCREASE);
                        boost.remaining_ticks = boost.remaining_ticks.max(EMP_TIME_DURATION);
                    })
                    .or_insert(TimedBoost {
                        multiplier: EMP_TIME_INCREASE,
                        remaining_ticks: EMP_TIME_DURATION,
                    });
                changed = true;
            }
        } else if let Some((destroyed, health)) =
            damage_building(world, position, damage * EMP_POWER_DAMAGE_SCL)
        {
            if destroyed {
                if let Ok(frame) = encode_build_destroyed_frame(position) {
                    out.broadcast(frame);
                }
            } else if let Ok(frame) = encode_build_health_update_frame(&[(position, health)]) {
                out.broadcast(frame);
            }
            changed = true;
        }
    }
    changed
}

pub(crate) fn simulate_player_combat(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    delta_ticks: f32,
) -> bool {
    let mut changed = false;
    let burning_players: Vec<_> = world
        .players
        .iter()
        .filter(|player| {
            !player.dead
                && (player
                    .statuses
                    .iter()
                    .any(|entry| entry.effect == 1 && entry.time > 0.0)
                    || (player.statuses.is_empty()
                        && player.status_effect == 1
                        && player.status_duration > 0.0))
        })
        .map(|player| *player.key())
        .collect();
    for id in burning_players {
        // StatusEffects.burning.damage = 0.167 per tick in desktop 158.1.
        changed |= damage_player(world, out, id, 0.167 * delta_ticks.max(0.0), -1, 0.0);
    }
    let ids: Vec<_> = world.players.iter().map(|player| *player.key()).collect();
    for id in ids {
        let Some(mut player) = world.players.get_mut(&id) else {
            continue;
        };
        // A6: tick the StatusEntry collection (expiry + legacy resync)
        // instead of the legacy single status, so stacked statuses expire
        // correctly after the apply_status migration in damage_player.
        if player.status_duration > 0.0 || !player.statuses.is_empty() {
            changed |=
                crate::network::units::StatusContainer::tick_statuses(&mut *player, delta_ticks);
        }
        let mut respawned = false;
        let mut replacement_id = id;
        if player.dead {
            player.respawn_timer = (player.respawn_timer - delta_ticks).max(0.0);
            if player.respawn_timer == 0.0 {
                player.dead = false;
                player.health = 150.0;
                player.shield = 0.0;
                crate::network::units::StatusContainer::clear_statuses(&mut *player);
                // Respawn on the player's OWN team core (PvP/Attack maps
                // carry one core per team; fallback: the sharded core).
                let (core_x, core_y) = core_world_for_team(world, player.team);
                player.x = core_x;
                player.y = core_y;
                replacement_id = world
                    .next_player_unit_id
                    .fetch_add(1, Ordering::Relaxed)
                    .max(2_500_000);
                player.unit_id = replacement_id;
                respawned = true;
            }
            changed = true;
        }
        let profile = player.clone();
        let uuid = profile.uuid.clone();
        let player_id = profile.player_id;
        drop(player);
        if respawned {
            world.players.remove(&id);
            world.players.insert(replacement_id, profile.clone());
            if let Some((_, mut session)) = world.player_sessions.remove(&id) {
                // Death/respawn clears Player.unit before the session is
                // re-keyed to its replacement core avatar.
                crate::network::units::switch_player_unit(world, &mut session, None);
                session.unit_id = replacement_id;
                session.x = profile.x;
                session.y = profile.y;
                session.shooting = false;
                session.boosting = false;
                session.mining_position = None;
                session.mining_progress = 0.0;
                session.carried_item = -1;
                session.carried_amount = 0;
                world.player_sessions.insert(replacement_id, session);
            }
        }
        world.player_profiles.insert(uuid, profile);
        if changed {
            world.persistence_dirty.store(true, Ordering::Relaxed);
        }
        if respawned {
            let position = world.core_position;
            let mut payload = Vec::with_capacity(8);
            use crate::network::codec::Writes;
            if payload.write_i(position).is_ok() && payload.write_i(player_id).is_ok() {
                if let Ok(frame) = frame_generated_packet(PLAYER_SPAWN_PACKET_ID, &payload, false) {
                    out.broadcast(frame);
                }
            }
        }
    }
    changed
}

/// Unit status immunities (UnitTypes.java `immunities`), verified against
/// the desktop.jar 158.1 bytecode. Guarded at every EnemyUnit status
/// application site so immune units never carry the status or take its
/// damage-over-time.
pub(crate) fn unit_immune_to_status(unit_type: i16, status_effect: i16) -> bool {
    // JAR 158.1 immunities (unit id = anonymous class index - 1):
    // - mace (1) burning: UnitTypes$2 offsets 30-39 (`immunities.add(burning)`);
    // - vela (8) burning: UnitTypes$9 offsets 108-122
    //   (`immunities = ObjectSet.with(burning)`);
    // - atrax (11) burning+melting: UnitTypes$12 offsets 47-60;
    // - precept (40) / vanquish (41) / conquer (42) burning+melting:
    //   UnitTypes$41/42/43 offsets 33-77 / 30-72
    //   (`immunities.addAll(burning, melting)`).
    // NOT immune (round-73 A5 corrections): navanax (34) — UnitTypes$35 has
    // no immunities field writes; naval 25-29 — UnitTypes$26..$30 have no
    // immunities (naval units only get wet; burning/melting resistance comes
    // from the liquid conversion, they CAN burn).
    match (unit_type, status_effect) {
        (1, 1) => true,           // mace: burning
        (8, 1) => true,           // vela: burning
        (11, 1 | 8) => true,      // atrax: burning + melting
        (40..=42, 1 | 8) => true, // precept/vanquish/conquer: burning + melting
        _ => false,
    }
}

pub(crate) fn enemy_armor(unit_type: i16) -> f32 {
    match unit_type {
        1 => 4.0,  // Mace
        2 => 9.0,  // Fortress
        3 => 10.0, // Scepter
        4 => 18.0, // Reign
        5 => 1.0,  // Nova
        6 => 4.0,  // Pulsar
        7 => 9.0,  // Quasar
        8 => 9.0,  // Vela
        11 => 3.0, // Atrax
        12 => 5.0, // Spiroct
        16 => 3.0, // Horizon
        17 => 5.0, // Zenith
        18 => 9.0, // Antumbra
        _ => 0.0,
    }
}

pub(crate) fn apply_unit_armor(damage: f32, armor: f32) -> f32 {
    (damage - armor).max(damage * 0.1)
}

/// Official `ShieldComp` armor: `armorOverride >= 0 ? armorOverride : armor`.
pub(crate) fn unit_effective_armor(unit: &EnemyUnit) -> f32 {
    crate::network::units::StatusContainer::status_aggregate(unit)
        .armor_override
        .unwrap_or_else(|| enemy_armor(unit.unit_type))
}

/// Official `ShieldComp.damage`: armor, then divide by `healthMultiplier`.
pub(crate) fn apply_incoming_unit_damage(unit: &EnemyUnit, damage: f32, armor_mult: f32) -> f32 {
    let armored = apply_unit_armor(damage, unit_effective_armor(unit) * armor_mult);
    let health = crate::network::units::StatusContainer::status_aggregate(unit).health;
    if !health.is_finite() {
        0.0
    } else if health.abs() < 1e-12 {
        armored
    } else {
        armored / health
    }
}

pub(crate) fn kill_enemy(
    world: &DynamicWorld,
    out: &dyn crate::network::outbound::FrameEmit,
    target_id: i32,
) {
    if !world.enemies.contains_key(&target_id) {
        return;
    }
    // Snapshot SpawnDeathAbility owners before the unit is removed.
    let spawn_death = world
        .enemies
        .get(&target_id)
        .and_then(|unit| (unit.unit_type == 57).then_some((unit.team, unit.x, unit.y)));
    world.game_state.game_stats.write().enemy_units_destroyed += 1;
    // Keep the final non-null stack invariant ordered before UnitDeath. This makes the
    // client safe even if its most recent periodic state came from an older or
    // partially decoded snapshot: generated destroy() dereferences item() when
    // amount is positive.
    if let Ok(snapshots) = encode_enemy_entity_snapshots(world) {
        for snapshot in snapshots {
            if let Ok(frame) = frame_generated_packet(ENTITY_SNAPSHOT_PACKET_ID, &snapshot, true) {
                out.broadcast(frame);
            }
        }
    }
    if world.enemies.remove(&target_id).is_none() {
        return;
    }
    world.unregister_unit_group(target_id);
    // P0-01: the order and any possession die with the unit; a session left
    // pointing at the dead unit returns to its core avatar.
    crate::network::units::detach_unit_control(world, target_id);
    world
        .game_state
        .enemies_count
        .store(hostile_unit_count(world), Ordering::Relaxed);
    let mut payload = Vec::with_capacity(4);
    use crate::network::codec::Writes;
    if payload.write_i(target_id).is_ok() {
        if let Ok(frame) = frame_generated_packet(UNIT_DEATH_PACKET_ID, &payload, false) {
            out.broadcast(frame);
        }
    }
    // Latum SpawnDeathAbility(renale, 5, 11): five renal, deterministic
    // pentagon at spread 11 (Java uses Mathf.random(spread)).
    if let Some((team, x, y)) = spawn_death {
        for index in 0..5 {
            let angle = index as f32 * 72.0;
            let rad = angle.to_radians();
            let _ = spawn_unit_world(
                world,
                56,
                team,
                x + 11.0 * rad.cos(),
                y + 11.0 * rad.sin(),
                angle,
            );
        }
    }
}
