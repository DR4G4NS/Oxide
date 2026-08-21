//! Unit spawn / despawn and enemy-spec resolution. Units facade re-exports
//! through crate::network::units::*.

use crate::network::combat::enemy::hostile_unit_count;
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

pub(crate) fn parse_unit_type(name: &str) -> Option<i16> {
    // P1: delegate to the single unit content registry (desktop 158.1 jar
    // dump, 69 ids) instead of keeping a second name table here. Numeric
    // ids keep working for console convenience; unregistered names return
    // None so callers fail explicitly (strict mode) instead of guessing.
    crate::game::unit_types::unit_id_from_name(name).or_else(|| name.trim().parse::<i16>().ok())
}

/// Nearest enemy spawn overlay to the core; fallback for `spawn` without x/y.
pub(crate) fn nearest_enemy_spawn(world: &DynamicWorld) -> (i16, i16) {
    let (core_x, core_y) = core_world(world);
    world
        .enemy_spawns
        .iter()
        .copied()
        .min_by(|left, right| {
            let dl = (f32::from(left.0) * 8.0 - core_x).powi(2)
                + (f32::from(left.1) * 8.0 - core_y).powi(2);
            let dr = (f32::from(right.0) * 8.0 - core_x).powi(2)
                + (f32::from(right.1) * 8.0 - core_y).powi(2);
            dl.total_cmp(&dr)
        })
        .unwrap_or(world.enemy_spawns[0])
}

/// Console `spawn` implementation: inserts `count` enemy (team 2) units built
/// from the enemy spec table, at the given tile position or at the nearest
/// enemy spawn overlay. Returns the number of units actually inserted.
pub(crate) fn spawn_enemy_units(
    world: &DynamicWorld,
    unit_type: i16,
    count: u32,
    x: Option<i16>,
    y: Option<i16>,
) -> usize {
    let Some(spec) = enemy_spec(unit_type) else {
        return 0;
    };
    if count == 0 {
        return 0;
    }
    let (base_x, base_y) = match (x, y) {
        (Some(tile_x), Some(tile_y)) => (f32::from(tile_x) * 8.0, f32::from(tile_y) * 8.0),
        _ => {
            if world.enemy_spawns.is_empty() {
                return 0;
            }
            let (tile_x, tile_y) = nearest_enemy_spawn(world);
            (f32::from(tile_x) * 8.0, f32::from(tile_y) * 8.0)
        }
    };
    let mut spawned = 0usize;
    for index in 0..count {
        let id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
        // Small cluster so a multi-unit spawn does not stack on one tile.
        let spread = (index % 3) as f32 * 6.0;
        world.enemies.insert(
            id,
            EnemyUnit {
                id,
                unit_type: spec.unit_type,
                entity_class: spec.entity_class,
                team: 2,
                x: base_x + spread,
                y: base_y,
                rotation: -90.0,
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
                authority: UnitAuthority::DefaultAi,
                build_plans: Vec::new(),
                update_building: true,
                status_agg: Default::default(),
            },
        );
        world.register_unit_group(id);
        spawned += 1;
    }
    world
        .game_state
        .enemies_count
        .store(hostile_unit_count(world), Ordering::Relaxed);
    world.persistence_dirty.store(true, Ordering::Relaxed);
    spawned
}

/// Spawn one unit at world-pixel coordinates (used by death abilities).
pub(crate) fn spawn_unit_world(
    world: &DynamicWorld,
    unit_type: i16,
    team: u8,
    x: f32,
    y: f32,
    rotation: f32,
) -> Option<i32> {
    let spec = enemy_spec(unit_type)?;
    let id = world.next_enemy_id.fetch_add(1, Ordering::Relaxed);
    world.enemies.insert(
        id,
        EnemyUnit {
            id,
            unit_type: spec.unit_type,
            entity_class: spec.entity_class,
            team,
            x,
            y,
            rotation,
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
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: Default::default(),
        },
    );
    world.register_unit_group(id);
    world
        .game_state
        .enemies_count
        .store(hostile_unit_count(world), Ordering::Relaxed);
    world.persistence_dirty.store(true, Ordering::Relaxed);
    Some(id)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct EnemySpec {
    pub(crate) unit_type: i16,
    pub(crate) entity_class: u8,
    pub(crate) health: f32,
    pub(crate) speed: f32,
    pub(crate) attack_damage: f32,
    pub(crate) attack_reload: f32,
    pub(crate) attack_range: f32,
}

pub(crate) const DAGGER: EnemySpec = EnemySpec {
    unit_type: 0,
    entity_class: 4,
    health: 150.0,
    speed: 0.5,
    attack_damage: 9.0,
    attack_reload: 13.0,
    attack_range: 145.0,
};
pub(crate) const MACE: EnemySpec = EnemySpec {
    unit_type: 1,
    entity_class: 4,
    health: 550.0,
    speed: 0.5,
    attack_damage: 74.0,
    attack_reload: 22.0,
    attack_range: 50.0,
};
pub(crate) const CRAWLER: EnemySpec = EnemySpec {
    unit_type: 10,
    entity_class: 4,
    health: 150.0,
    speed: 1.0,
    attack_damage: 81.0,
    attack_reload: 24.0,
    attack_range: 28.0,
};

pub(crate) const FORTRESS: EnemySpec = EnemySpec {
    unit_type: 2,
    entity_class: 4,
    health: 900.0,
    speed: 0.43,
    attack_damage: 20.0,
    attack_reload: 60.0,
    attack_range: 240.0,
};
pub(crate) const SCEPTER: EnemySpec = EnemySpec {
    unit_type: 3,
    entity_class: 4,
    health: 9_000.0,
    speed: 0.36,
    attack_damage: 210.0,
    attack_reload: 45.0,
    attack_range: 216.0,
};
pub(crate) const REIGN: EnemySpec = EnemySpec {
    unit_type: 4,
    entity_class: 4,
    health: 24_000.0,
    speed: 0.4,
    attack_damage: 80.0,
    attack_reload: 9.0,
    attack_range: 195.0,
};
pub(crate) const NOVA: EnemySpec = EnemySpec {
    unit_type: 5,
    entity_class: 17,
    health: 120.0,
    speed: 0.55,
    attack_damage: 13.0,
    attack_reload: 24.0,
    attack_range: 156.0,
};
pub(crate) const PULSAR: EnemySpec = EnemySpec {
    unit_type: 6,
    entity_class: 19,
    health: 320.0,
    speed: 0.7,
    attack_damage: 45.0,
    attack_reload: 36.0,
    attack_range: 80.0,
};
pub(crate) const QUASAR: EnemySpec = EnemySpec {
    unit_type: 7,
    entity_class: 32,
    health: 640.0,
    speed: 0.5,
    attack_damage: 45.0,
    attack_reload: 55.0,
    attack_range: 160.0,
};
pub(crate) const VELA: EnemySpec = EnemySpec {
    unit_type: 8,
    entity_class: 4,
    health: 8_200.0,
    speed: 0.44,
    attack_damage: 35.0,
    attack_reload: 155.0,
    attack_range: 180.0,
};
pub(crate) const ATRAX: EnemySpec = EnemySpec {
    unit_type: 11,
    entity_class: 24,
    health: 600.0,
    speed: 0.6,
    attack_damage: 13.0,
    attack_reload: 9.0,
    attack_range: 100.0,
};
pub(crate) const SPIROCT: EnemySpec = EnemySpec {
    unit_type: 12,
    entity_class: 21,
    health: 1_000.0,
    speed: 0.54,
    attack_damage: 46.0,
    attack_reload: 14.0,
    attack_range: 75.0,
};
pub(crate) const FLARE: EnemySpec = EnemySpec {
    unit_type: 15,
    entity_class: 3,
    health: 70.0,
    speed: 2.7,
    attack_damage: 27.0,
    attack_reload: 80.0,
    attack_range: 150.0,
};
pub(crate) const MONO: EnemySpec = EnemySpec {
    unit_type: 20,
    entity_class: 16,
    health: 100.0,
    speed: 1.5,
    attack_damage: 0.0,
    attack_reload: 1.0,
    attack_range: 50.0,
};
pub(crate) const RISSO: EnemySpec = EnemySpec {
    unit_type: 25,
    entity_class: 20,
    health: 280.0,
    speed: 1.1,
    attack_damage: 9.0,
    attack_reload: 13.0,
    attack_range: 175.5,
};
pub(crate) const RETUSA: EnemySpec = EnemySpec {
    unit_type: 30,
    entity_class: 20,
    health: 270.0,
    speed: 0.9,
    attack_damage: 12.0,
    attack_reload: 22.0,
    attack_range: 100.0,
};
pub(crate) const HORIZON: EnemySpec = EnemySpec {
    unit_type: 16,
    entity_class: 3,
    health: 340.0,
    speed: 1.65,
    attack_damage: 27.0,
    attack_reload: 24.0,
    attack_range: 140.0,
};
pub(crate) const ZENITH: EnemySpec = EnemySpec {
    unit_type: 17,
    entity_class: 3,
    health: 700.0,
    speed: 1.7,
    attack_damage: 14.0,
    attack_reload: 40.0,
    attack_range: 140.0,
};
pub(crate) const ANTUMBRA: EnemySpec = EnemySpec {
    unit_type: 18,
    entity_class: 3,
    health: 7_200.0,
    speed: 0.8,
    attack_damage: 110.0,
    attack_reload: 20.0,
    attack_range: 200.0,
};

pub(crate) fn enemy_spec(unit_type: i16) -> Option<EnemySpec> {
    match unit_type {
        0 => Some(DAGGER),
        1 => Some(MACE),
        2 => Some(FORTRESS),
        3 => Some(SCEPTER),
        4 => Some(REIGN),
        5 => Some(NOVA),
        6 => Some(PULSAR),
        7 => Some(QUASAR),
        8 => Some(VELA),
        10 => Some(CRAWLER),
        11 => Some(ATRAX),
        12 => Some(SPIROCT),
        15 => Some(FLARE),
        16 => Some(HORIZON),
        17 => Some(ZENITH),
        18 => Some(ANTUMBRA),
        20 => Some(MONO),
        21 => Some(EnemySpec {
            unit_type: 21,
            entity_class: 18,
            health: 400.0,
            speed: 2.6,
            attack_damage: 12.0,
            attack_reload: 30.0,
            attack_range: 130.0,
        }),
        22 => Some(EnemySpec {
            unit_type: 22,
            entity_class: 5,
            health: 460.0,
            speed: 2.5,
            attack_damage: 10.0,
            attack_reload: 24.0,
            attack_range: 180.0,
        }),
        23 => Some(EnemySpec {
            unit_type: 23,
            entity_class: 23,
            health: 6_000.0,
            speed: 1.2,
            attack_damage: 374.0,
            attack_reload: 55.0,
            attack_range: 140.0,
        }),
        24 => Some(EnemySpec {
            unit_type: 24,
            entity_class: 26,
            health: 24_000.0,
            speed: 0.8,
            attack_damage: 0.0,
            attack_reload: 1.0,
            attack_range: 200.0,
        }),
        25 => Some(RISSO),
        26 => Some(EnemySpec {
            unit_type: 26,
            entity_class: 20,
            health: 600.0,
            speed: 0.9,
            attack_damage: 43.5,
            attack_reload: 10.0,
            attack_range: 220.0,
        }),
        27 => Some(EnemySpec {
            unit_type: 27,
            entity_class: 20,
            health: 910.0,
            speed: 0.85,
            attack_damage: 85.0,
            attack_reload: 65.0,
            attack_range: 268.0,
        }),
        28 => Some(EnemySpec {
            unit_type: 28,
            entity_class: 20,
            health: 11_000.0,
            speed: 0.73,
            attack_damage: 87.0,
            attack_reload: 45.0,
            attack_range: 260.0,
        }),
        29 => Some(EnemySpec {
            unit_type: 29,
            entity_class: 20,
            health: 22_000.0,
            speed: 0.62,
            attack_damage: 1_250.0,
            attack_reload: 110.0,
            attack_range: 500.0,
        }),
        30 => Some(RETUSA),
        31 => Some(EnemySpec {
            unit_type: 31,
            entity_class: 20,
            health: 560.0,
            speed: 0.83,
            attack_damage: 23.0,
            attack_reload: 5.0,
            attack_range: 100.0,
        }),
        32 => Some(EnemySpec {
            unit_type: 32,
            entity_class: 20,
            health: 870.0,
            speed: 0.86,
            attack_damage: 25.0,
            attack_reload: 60.0,
            attack_range: 200.0,
        }),
        33 => Some(EnemySpec {
            unit_type: 33,
            entity_class: 20,
            health: 12_000.0,
            speed: 0.7,
            attack_damage: 80.0,
            attack_reload: 30.0,
            attack_range: 180.0,
        }),
        34 => Some(EnemySpec {
            unit_type: 34,
            entity_class: 20,
            health: 20_000.0,
            speed: 0.65,
            attack_damage: 60.0,
            attack_reload: 65.0,
            attack_range: 300.0,
        }),
        // Campaign mechs (UnitTypes.java v158.1: alpha 150hp/3.0 speed,
        // beta 170hp/3.3, gamma 220hp/3.55). Speeds converted to the port's
        // tile-scale (dagger 0.5). Used by late Serpulo spawn groups.
        35 => Some(EnemySpec {
            unit_type: 35,
            entity_class: 4,
            health: 150.0,
            speed: 0.5,
            attack_damage: 9.0,
            attack_reload: 13.0,
            attack_range: 145.0,
        }),
        36 => Some(EnemySpec {
            unit_type: 36,
            entity_class: 4,
            health: 170.0,
            speed: 0.55,
            attack_damage: 11.0,
            attack_reload: 13.0,
            attack_range: 150.0,
        }),
        37 => Some(EnemySpec {
            unit_type: 37,
            entity_class: 4,
            health: 220.0,
            speed: 0.6,
            attack_damage: 14.0,
            attack_reload: 13.0,
            attack_range: 160.0,
        }),
        9 => Some(EnemySpec {
            unit_type: 9,
            entity_class: 24,
            health: 18_000.0,
            speed: 0.3,
            attack_damage: 560.0,
            attack_reload: 350.0,
            attack_range: 460.0,
        }),
        13 => Some(EnemySpec {
            unit_type: 13,
            entity_class: 29,
            health: 8_000.0,
            speed: 0.62,
            attack_damage: 40.0,
            attack_reload: 9.0,
            attack_range: 180.0,
        }),
        14 => Some(EnemySpec {
            unit_type: 14,
            entity_class: 33,
            health: 22_000.0,
            speed: 0.5,
            attack_damage: 220.0,
            attack_reload: 30.0,
            attack_range: 180.0,
        }),
        19 => Some(EnemySpec {
            unit_type: 19,
            entity_class: 3,
            health: 22_000.0,
            speed: 0.54,
            attack_damage: 115.0,
            attack_reload: 45.0,
            attack_range: 300.0,
        }),
        // ---- Erekir units (UnitTypes.java v158.1 + unit_weapons.tsv) ----
        38 => Some(EnemySpec {
            unit_type: 38,
            entity_class: 43,
            health: 850.0,
            speed: 0.75,
            attack_damage: 30.0,
            attack_reload: 50.0,
            attack_range: 160.0,
        }),
        39 => Some(EnemySpec {
            unit_type: 39,
            entity_class: 43,
            health: 2_100.0,
            speed: 0.7,
            attack_damage: 36.0,
            attack_reload: 18.0,
            attack_range: 130.0,
        }),
        40 => Some(EnemySpec {
            unit_type: 40,
            entity_class: 43,
            health: 5_000.0,
            speed: 0.64,
            attack_damage: 90.0,
            attack_reload: 80.0,
            attack_range: 196.0,
        }),
        41 => Some(EnemySpec {
            unit_type: 41,
            entity_class: 43,
            health: 11_000.0,
            speed: 0.63,
            attack_damage: 112.5,
            attack_reload: 80.0,
            attack_range: 180.0,
        }),
        42 => Some(EnemySpec {
            unit_type: 42,
            entity_class: 43,
            health: 22_000.0,
            speed: 0.48,
            attack_damage: 270.0,
            attack_reload: 100.0,
            attack_range: 280.0,
        }),
        43 => Some(EnemySpec {
            unit_type: 43,
            entity_class: 24,
            health: 680.0,
            speed: 0.72,
            attack_damage: 30.0,
            attack_reload: 63.0,
            attack_range: 138.0,
        }),
        44 => Some(EnemySpec {
            unit_type: 44,
            entity_class: 24,
            health: 1_100.0,
            speed: 0.6,
            attack_damage: 22.5,
            attack_reload: 33.0,
            attack_range: 140.0,
        }),
        47 => Some(EnemySpec {
            unit_type: 47,
            entity_class: 24,
            health: 6_500.0,
            speed: 0.6,
            attack_damage: 38.25,
            attack_reload: 40.0,
            attack_range: 231.0,
        }),
        48 => Some(EnemySpec {
            unit_type: 48,
            entity_class: 24,
            health: 18_000.0,
            speed: 1.1,
            attack_damage: 195.0,
            attack_reload: 130.0,
            attack_range: 330.0,
        }),
        49 => Some(EnemySpec {
            unit_type: 49,
            entity_class: 45,
            health: 600.0,
            speed: 1.8,
            attack_damage: 12.0,
            attack_reload: 40.0,
            attack_range: 150.0,
        }),
        50 => Some(EnemySpec {
            unit_type: 50,
            entity_class: 3,
            health: 1_100.0,
            speed: 2.0,
            attack_damage: 22.0,
            attack_reload: 35.0,
            attack_range: 90.0,
        }),
        51 => Some(EnemySpec {
            unit_type: 51,
            entity_class: 3,
            health: 2_300.0,
            speed: 1.8,
            attack_damage: 56.25,
            attack_reload: 140.0,
            attack_range: 180.0,
        }),
        52 => Some(EnemySpec {
            unit_type: 52,
            entity_class: 5,
            health: 6_000.0,
            speed: 1.1,
            attack_damage: 52.5,
            attack_reload: 55.0,
            attack_range: 128.0,
        }),
        54 => Some(EnemySpec {
            unit_type: 54,
            entity_class: 5,
            health: 12_000.0,
            speed: 1.0,
            attack_damage: 2.25,
            attack_reload: 70.0,
            attack_range: 100.0,
        }),
        // Neoplasm crawlers (UnitTypes.java 158.1). Latum's SpawnDeathAbility
        // needs a live spec so death can spawn renal without a factory.
        56 => Some(EnemySpec {
            unit_type: 56,
            entity_class: 46,
            health: 500.0,
            speed: 1.2,
            attack_damage: 0.0,
            attack_reload: 1.0,
            attack_range: 8.0,
        }),
        57 => Some(EnemySpec {
            unit_type: 57,
            entity_class: 46,
            health: 20_000.0,
            speed: 1.0,
            attack_damage: 0.0,
            attack_reload: 1.0,
            attack_range: 8.0,
        }),
        _ => None,
    }
}
