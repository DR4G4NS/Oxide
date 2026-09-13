//! Unit content registry (SOL-AUDIT P1: unified UnitTypes registry).
//!
//! This module is the single owner of the unit content table for the
//! 158.1 baseline. The id<->name pairs were dumped from `desktop.jar` 158.1
//! by enumerating `Vars.content.units()` (69 UnitTypes, including Erekir
//! units and internal missiles) — IDs are never inferred from source order.
//!
//! Per ARCHITECTURE.md, domain modules (logic, network/units, save_io)
//! delegate here instead of keeping their own unit tables.

/// Official v158.1 unit content ids and names (jar dump, 69 entries).
pub const UNIT_NAMES: &[(i16, &str)] = &[
    (0, "dagger"),
    (1, "mace"),
    (2, "fortress"),
    (3, "scepter"),
    (4, "reign"),
    (5, "nova"),
    (6, "pulsar"),
    (7, "quasar"),
    (8, "vela"),
    (9, "corvus"),
    (10, "crawler"),
    (11, "atrax"),
    (12, "spiroct"),
    (13, "arkyid"),
    (14, "toxopid"),
    (15, "flare"),
    (16, "horizon"),
    (17, "zenith"),
    (18, "antumbra"),
    (19, "eclipse"),
    (20, "mono"),
    (21, "poly"),
    (22, "mega"),
    (23, "quad"),
    (24, "oct"),
    (25, "risso"),
    (26, "minke"),
    (27, "bryde"),
    (28, "sei"),
    (29, "omura"),
    (30, "retusa"),
    (31, "oxynoe"),
    (32, "cyerce"),
    (33, "aegires"),
    (34, "navanax"),
    (35, "alpha"),
    (36, "beta"),
    (37, "gamma"),
    (38, "stell"),
    (39, "locus"),
    (40, "precept"),
    (41, "vanquish"),
    (42, "conquer"),
    (43, "merui"),
    (44, "cleroi"),
    (45, "anthicus"),
    (46, "anthicus-missile"),
    (47, "tecta"),
    (48, "collaris"),
    (49, "elude"),
    (50, "avert"),
    (51, "obviate"),
    (52, "quell"),
    (53, "quell-missile"),
    (54, "disrupt"),
    (55, "disrupt-missile"),
    (56, "renale"),
    (57, "latum"),
    (58, "evoke"),
    (59, "incite"),
    (60, "emanate"),
    (61, "block"),
    (62, "manifold"),
    (63, "assembly-drone"),
    (64, "scathe-missile"),
    (65, "scathe-missile-phase"),
    (66, "scathe-missile-surge"),
    (67, "scathe-missile-surge-split"),
    (68, "turret-unit-build-tower"),
];

/// Number of registered unit content ids in the 158.1 baseline.
pub const UNIT_COUNT: usize = UNIT_NAMES.len();

/// Official `Vars.defaultEnv` (v159.7): terrestrial + spores + groundOil +
/// groundWater + oxygen.
pub const RULES_ENV_DEFAULT: i32 = 1 | 8 | 32 | 64 | 128;

/// Exact v159.7 `UnitType.commands` membership used by CommandAI.
/// Move (0) is universal. Mine (4) is mining units. Payload loop commands
/// (6-9) require payloadCapacity. enterPayload (5) is issued to reconstructors
/// by every player-commandable type.
pub fn unit_type_allows_command(id: i16, command: u8) -> bool {
    if !unit_player_controllable_like(id) {
        return false;
    }
    match command {
        0 | 5 => true,
        1 => matches!(id, 22 | 31),
        2 => matches!(id, 21 | 22 | 23 | 24 | 35..=37),
        3 => matches!(id, 21 | 22),
        4 => matches!(id, 6 | 7 | 20 | 21),
        6..=9 => matches!(id, 22..=24),
        _ => false,
    }
}

fn unit_player_controllable_like(id: i16) -> bool {
    !matches!(id, 46 | 53 | 55 | 62..=67)
}

/// Official v159.7 internal UnitTypes: hidden `block` plus the generated
/// `turret-unit-build-tower` created by `BuildTurret.init()`.
pub fn unit_type_internal(id: i16) -> bool {
    // `block` plus the generated turret-unit-build-tower content type.
    matches!(id, 61 | 68)
}

/// Exact v159.7 `UnitType.useUnitCap` metadata.
///
/// `UnitType` defaults this field to true. `MissileUnitType` sets it false
/// for all seven vanilla missile content types, and `UnitTypes.assemblyDrone`
/// overrides it to false. Unknown IDs retain the default true value.
pub fn unit_type_use_unit_cap(id: i16) -> bool {
    !matches!(id, 46 | 53 | 55 | 63 | 64..=67)
}

/// Official `UnitType.isEnemy` (desktop 159.7 dump). Default is true.
/// `hostile_unit_count` / `waitEnemies` / wave victory use this, not team
/// membership alone (ASTRA W06): mono, mega and core units are not enemies.
pub fn unit_type_is_enemy(id: i16) -> bool {
    !matches!(
        id,
        20 | 22 | 35 | 36 | 37 | 46 | 53 | 55 | 58 | 59 | 60 | 62 | 63 | 64 | 65 | 66 | 67
    )
}

/// Exact v159.7 `MissileUnitType.lifetime` values, emitted as
/// `TimedKillUnit.lifetime`/`.time` in the unit sync stream. The
/// `MissileUnitType` constructor defaults the field to `60f * 1.7f`;
/// entries only list types that override it or rely on the default.
pub fn unit_missile_lifetime(id: i16) -> Option<f32> {
    match id {
        46 => Some(99.6),  // anthicus-missile: 60f * 1.66f
        53 => Some(54.24), // quell-missile: 60f * (1.4f - 0.496f)
        55 => Some(102.0), // disrupt-missile: constructor default
        64 => Some(330.0), // scathe-missile: 60f * 5.5f
        65 => Some(586.2), // scathe-missile-phase: 60f * 9.77f
        66 => Some(84.0),  // scathe-missile-surge: 60f * 1.4f
        67 => Some(222.0), // scathe-missile-surge-split: 60f * 3.7f
        _ => None,
    }
}

/// CoreBlock.unitType for vanilla core blocks (Blocks.java v159.7).
pub fn core_block_unit_type(block: i16) -> Option<i16> {
    match block {
        339 => Some(35),
        340 => Some(36),
        341 => Some(37),
        342 => Some(58),
        343 => Some(59),
        344 => Some(60),
        _ => None,
    }
}

/// Official `UnitType.coreUnitDock`. Only Erekir core ships dock in place
/// on `InputHandler.unitClear`; Serpulo alpha/beta/gamma fall through to
/// `playerSpawn` at the best core (ASTRA C07).
pub fn core_unit_dock(unit_type: i16) -> bool {
    matches!(unit_type, 58..=60)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitEnvFlags {
    pub enabled: i32,
    pub disabled: i32,
    pub required: i32,
}

/// Exact v159.7 `UnitType.supportsEnv` flag triple for vanilla core units.
///
/// Verified against tag `v159.7` (`c9686eb5…`):
/// - `UnitType` defaults: `envEnabled=terrestrial(1)`, `envDisabled=scorching(16)`,
///   `envRequired=0`; `init()` adds `Env.space` when `flying`.
/// - `ErekirUnitType` sets `envDisabled=space`, then evoke/incite/emanate override
///   `envDisabled=0`.
/// - Serpulo cores alpha/beta/gamma (35–37): flying → enabled=`terrestrial|space`,
///   disabled=`scorching`.
/// - Erekir cores evoke/incite/emanate (58–60): flying → enabled=`terrestrial|space`,
///   disabled=`0`.
pub fn unit_env_flags(id: i16) -> UnitEnvFlags {
    match id {
        // evoke / incite / emanate
        58..=60 => UnitEnvFlags {
            enabled: 1 | 2,
            disabled: 0,
            required: 0,
        },
        // alpha / beta / gamma
        35..=37 => UnitEnvFlags {
            enabled: 1 | 2,
            disabled: 16,
            required: 0,
        },
        // Non-core callers: UnitType flying defaults after init().
        _ => UnitEnvFlags {
            enabled: 1 | 2,
            disabled: 16,
            required: 0,
        },
    }
}

pub fn unit_supports_env(id: i16, env: i32) -> bool {
    let flags = unit_env_flags(id);
    (flags.enabled & env) != 0
        && (flags.disabled & env) == 0
        && (flags.required == 0 || (flags.required & env) == flags.required)
}

/// Official v158.1 `UnitType.logicControllable` (desktop 158.1): true unless
/// explicitly disabled. False for exactly nine content ids:
/// - `manifold` (62) and `assembly-drone` (63) — UnitTypes.java 4572/4613;
/// - every `MissileUnitType` (MissileUnitType.java 18 sets the flag false):
///   anthicus-missile (46), quell-missile (53), disrupt-missile (55)
///   (UnitTypes.java 3451/4101/4222) and the scathe missiles (64-67)
///   (Blocks.java 5224/5312/5417/5473).
///
/// `ubind` refuses these types (`type.logicControllable` gate in
/// LExecutor.UnitBindI) — the type never reaches the round-robin cache.
pub fn unit_type_logic_controllable(id: i16) -> bool {
    !matches!(id, 46 | 53 | 55 | 62 | 63 | 64 | 65 | 66 | 67)
}

/// Official unit content name by id (None for unregistered ids).
pub fn unit_name_from_id(id: i16) -> Option<&'static str> {
    UNIT_NAMES
        .iter()
        .find(|(registered, _)| *registered == id)
        .map(|(_, name)| *name)
}

/// Official unit content id by name (case-insensitive; `arklyid` is accepted
/// as the historical community alias of `arkyid`). Unregistered names return
/// None so callers can fail explicitly instead of guessing.
pub fn unit_id_from_name(name: &str) -> Option<i16> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized == "arklyid" {
        return Some(13);
    }
    UNIT_NAMES
        .iter()
        .find(|(_, registered)| **registered == normalized)
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_registry_matches_desktop_158_dump() {
        // The 69 id/name pairs were dumped from desktop.jar 158.1
        // (Vars.content.units()); ids are contiguous 0..=68.
        assert_eq!(UNIT_COUNT, 69);
        for (index, (id, name)) in UNIT_NAMES.iter().enumerate() {
            assert_eq!(*id as usize, index, "ids are contiguous 0..=68");
            assert!(!name.is_empty());
            assert_eq!(unit_name_from_id(*id), Some(*name));
            assert_eq!(unit_id_from_name(name), Some(*id));
        }
        // Round-trip and explicit rejection.
        assert_eq!(unit_id_from_name("DAGGER"), Some(0));
        assert_eq!(unit_id_from_name("stell"), Some(38));
        assert_eq!(unit_name_from_id(68), Some("turret-unit-build-tower"));
        assert_eq!(unit_name_from_id(-1), None);
        assert_eq!(unit_name_from_id(69), None);
        assert_eq!(unit_id_from_name("not-a-unit"), None);
        assert_eq!(unit_id_from_name(""), None);
        // Historical alias.
        assert_eq!(unit_id_from_name("arklyid"), Some(13));
    }

    #[test]
    fn unit_type_internal_matches_v1597() {
        assert!(unit_type_internal(61));
        assert!(unit_type_internal(68));
        assert!(!unit_type_internal(0));
        assert!(!unit_type_internal(46));
    }

    #[test]
    fn unit_type_use_unit_cap_matches_v1597_metadata() {
        // MissileUnitType.java:20 and UnitTypes.java:4612.
        for id in [46, 53, 55, 64, 65, 66, 67, 63] {
            assert!(!unit_type_use_unit_cap(id), "id {id} must ignore unit cap");
        }
        assert!(unit_type_use_unit_cap(0));
        assert!(
            unit_type_use_unit_cap(62),
            "manifold retains UnitType default"
        );
        assert!(
            unit_type_use_unit_cap(69),
            "unknown IDs retain UnitType default"
        );
    }

    #[test]
    fn unit_type_is_enemy_matches_v1597_dump() {
        assert!(unit_type_is_enemy(0), "dagger");
        assert!(unit_type_is_enemy(21), "poly");
        assert!(!unit_type_is_enemy(20), "mono");
        assert!(!unit_type_is_enemy(22), "mega");
        assert!(!unit_type_is_enemy(35), "alpha");
        assert!(!unit_type_is_enemy(36), "beta");
        assert!(!unit_type_is_enemy(37), "gamma");
        assert!(unit_type_is_enemy(61), "block keeps UnitType default");
    }

    #[test]
    fn core_env_support_matches_official_planet_defaults() {
        let erekir_env = 16 | 1;
        assert!(unit_supports_env(58, erekir_env));
        assert!(!unit_supports_env(36, erekir_env));
        let default_env = RULES_ENV_DEFAULT;
        assert!(unit_supports_env(36, default_env));
        assert!(unit_supports_env(58, default_env));
    }

    #[test]
    fn logic_controllable_flag_matches_desktop_1581_sources() {
        // Every regular unit is logic-controllable...
        for (id, _) in UNIT_NAMES {
            let name = unit_name_from_id(*id).unwrap();
            match *id {
                46 | 53 | 55 | 62 | 63 | 64 | 65 | 66 | 67 => {
                    assert!(
                        !unit_type_logic_controllable(*id),
                        "{name} must not be logic-controllable"
                    );
                }
                _ => assert!(
                    unit_type_logic_controllable(*id),
                    "{name} must be logic-controllable"
                ),
            }
        }
        // Unknown ids (outside the 0..=68 registry) keep the permissive
        // default: only the nine known 158.1 non-controllable types are
        // refused, and they must not panic.
        assert!(unit_type_logic_controllable(-1));
        assert!(unit_type_logic_controllable(69));
    }
}
