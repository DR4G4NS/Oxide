//! Player combat helpers for the session pass. Session facade re-exports
//! through crate::network::session::*.

use crate::network::listener::*;
use crate::network::world::*;

use super::*;

pub fn update_player_combat(
    player: &mut SessionPlayer,
    world: &DynamicWorld,
    connections: &DashMap<i32, PendingConnection>,
) -> std::io::Result<()> {
    // Alpha's official weapon reload is 17 ticks and its bullet deals 11 damage.
    if !player.shooting
        || player.last_shot.elapsed() < std::time::Duration::from_secs_f32(17.0 / 60.0)
    {
        return Ok(());
    }
    let aim_x = player.mouse_x - player.x;
    let aim_y = player.mouse_y - player.y;
    let aim_length = aim_x.hypot(aim_y);
    if aim_length <= 0.001 {
        return Ok(());
    }
    let direction_x = aim_x / aim_length;
    let direction_y = aim_y / aim_length;
    // Team of the firing player: 1 in survival/attack, the assigned PvP team
    // otherwise (NetServer.assignTeam). Fallback to the profile team so a
    // session snapshot is never used before the combat state exists.
    let player_team = world
        .players
        .get(&player.unit_id)
        .map(|combat| combat.team)
        .unwrap_or_else(|| {
            world
                .player_profiles
                .get(&player.uuid)
                .map(|profile| profile.team)
                .unwrap_or(1)
        });
    let pvp = *world.game_state.mode.read() == GameMode::Pvp;
    let mut target = None;
    for enemy in world.enemies.iter() {
        if enemy.team != world.wave_rules.read().wave_team {
            continue;
        }
        let relative_x = enemy.x - player.x;
        let relative_y = enemy.y - player.y;
        let along = relative_x * direction_x + relative_y * direction_y;
        if !(0.0..=150.0).contains(&along) {
            continue;
        }
        let perpendicular = (relative_x * direction_y - relative_y * direction_x).abs();
        if perpendicular <= 6.0
            && target.is_none_or(|(_, nearest, _, _): (i32, f32, f32, f32)| along < nearest)
        {
            target = Some((enemy.id, along, enemy.x, enemy.y));
        }
    }
    // PvP: players of other teams are valid targets (BulletType.collidesTeam).
    if pvp {
        for other in world.players.iter() {
            let other = other.value();
            if other.dead || other.unit_id == player.unit_id || other.team == player_team {
                continue;
            }
            let relative_x = other.x - player.x;
            let relative_y = other.y - player.y;
            let along = relative_x * direction_x + relative_y * direction_y;
            if !(0.0..=150.0).contains(&along) {
                continue;
            }
            let perpendicular = (relative_x * direction_y - relative_y * direction_x).abs();
            if perpendicular <= 6.0
                && target.is_none_or(|(_, nearest, _, _): (i32, f32, f32, f32)| along < nearest)
            {
                target = Some((other.unit_id, along, other.x, other.y));
            }
        }
    }
    player.last_shot = std::time::Instant::now();
    let Some((target_id, distance, target_x, target_y)) = target else {
        return Ok(());
    };
    if pvp && player_team != 1 {
        // Projectile carries the shooter's team so the PvP damage pass and
        // the client's CreateBullet rendering use the real team.
        spawn_team_projectile(
            world,
            connections,
            None,
            target_id,
            65, // alpha bullet
            player.x,
            player.y,
            target_x,
            target_y,
            11.0,
            2.5,
            distance,
            1.0,
            player_team,
        );
    } else {
        spawn_projectile(
            world,
            connections,
            None,
            target_id,
            65, // alpha bullet
            player.x,
            player.y,
            target_x,
            target_y,
            11.0,
            2.5,
            distance,
            1.0,
        );
    }
    Ok(())
}

/// Mirrors combat.rs `spawn_projectile` but tags the projectile (and the
/// broadcast `CreateBulletCallPacket`) with the shooter's real team instead of
/// the hardcoded sharded team 1. Required for PvP so opposing players can see
/// and take damage from the correct team's bullets.
#[allow(clippy::too_many_arguments)]
pub fn spawn_team_projectile(
    world: &DynamicWorld,
    connections: &DashMap<i32, PendingConnection>,
    source_position: Option<i32>,
    target_id: i32,
    bullet_id: i16,
    source_x: f32,
    source_y: f32,
    target_x: f32,
    target_y: f32,
    damage: f32,
    speed: f32,
    distance: f32,
    lifetime_scale: f32,
    team: u8,
) -> i32 {
    let angle = (target_y - source_y)
        .atan2(target_x - source_x)
        .to_degrees();
    let total_ticks = if speed <= 0.0 { 0.0 } else { distance / speed };
    let projectile_id = world.next_projectile_id.fetch_add(1, Ordering::Relaxed);
    world.projectiles.insert(
        projectile_id,
        Projectile {
            target_id,
            shooter_id: -1,
            team,
            bullet_id,
            damage,
            splash_damage: 0.0,
            splash_radius: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            pierce_units: 0,
            pierce_buildings: 0,
            spawn_reign_frags: false,
            homing_range: 0.0,
            enemy_target_position: None,
            enemy_target_core: false,
            apply_direct_on_impact: false,
            armor_multiplier: projectile_armor_multiplier(bullet_id),
            remaining_ticks: total_ticks,
            total_ticks,
            source_x,
            source_y,
            target_x,
            target_y,
            lifetime_scale,
            source_position,
            damage_interval: None,
            damage_timer: 0.0,
        },
    );
    if let Ok(payload) = encode_create_bullet_payload(
        bullet_id,
        team,
        source_x,
        source_y,
        angle,
        damage,
        1.0,
        lifetime_scale,
    ) {
        if let Ok(frame) = frame_generated_packet(CREATE_BULLET_PACKET_ID, &payload, false) {
            broadcast(connections, frame);
        }
    }
    projectile_id
}
