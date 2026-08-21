#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

static BLOCK_REQUIREMENTS: OnceLock<HashMap<i16, Vec<(usize, i32)>>> = OnceLock::new();
static BLOCK_SIZES: OnceLock<HashMap<i16, u8>> = OnceLock::new();
static BLOCK_BUILD_TIMES: OnceLock<HashMap<i16, f32>> = OnceLock::new();
static BLOCK_COMBAT: OnceLock<HashMap<i16, (f32, f32)>> = OnceLock::new();
static BLOCK_NAVIGATION: OnceLock<HashMap<i16, BlockNavigation>> = OnceLock::new();
static BLOCK_PATHING: OnceLock<HashMap<i16, BlockPathing>> = OnceLock::new();
static BLOCK_PLACEMENT: OnceLock<HashMap<i16, BlockPlacement>> = OnceLock::new();
static UNIT_MOVEMENT: OnceLock<HashMap<i16, UnitMovement>> = OnceLock::new();
static FLOOR_STATUS: OnceLock<HashMap<i16, (i16, f32)>> = OnceLock::new();
static UNIT_HOVERING: OnceLock<HashMap<i16, ()>> = OnceLock::new();
type UnitRequirement = (i16, i32);
type UnitRecipe = (f32, Vec<UnitRequirement>);
static UNIT_REQUIREMENTS: OnceLock<HashMap<i16, UnitRecipe>> = OnceLock::new();
static UNIT_WEAPONS: OnceLock<HashMap<i16, Vec<UnitWeapon>>> = OnceLock::new();
static UNIT_INVENTORY: OnceLock<HashMap<i16, UnitInventoryEntry>> = OnceLock::new();

#[derive(Clone, Copy, Default)]
pub struct BlockNavigation {
    pub solid: bool,
    pub team_passable: bool,
    pub floor: bool,
    pub deep: bool,
    pub damages: bool,
}

#[derive(Clone, Copy, Default)]
pub struct BlockPathing {
    pub synthetic: bool,
    pub fills_tile: bool,
    pub force_dark: bool,
}

pub fn block_pathing(block: i16) -> BlockPathing {
    *BLOCK_PATHING
        .get_or_init(|| {
            include_str!("block_pathing.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let id = fields.next().unwrap().parse().unwrap();
                    let synthetic = fields.next().unwrap().parse().unwrap();
                    let fills_tile = fields.next().unwrap().parse().unwrap();
                    let force_dark = fields.next().unwrap().parse().unwrap();
                    (
                        id,
                        BlockPathing {
                            synthetic,
                            fills_tile,
                            force_dark,
                        },
                    )
                })
                .collect()
        })
        .get(&block)
        .unwrap_or(&BlockPathing::default())
}

#[derive(Clone, Copy, Default)]
pub struct BlockPlacement {
    pub size: u8,
    pub group: u8,
    pub replaceable: bool,
    pub always_replace: bool,
    pub rotate: bool,
    pub quick_rotate: bool,
    pub privileged: bool,
}

pub fn block_placement(block: i16) -> BlockPlacement {
    *BLOCK_PLACEMENT
        .get_or_init(|| {
            include_str!("block_placement.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let id = fields.next().unwrap().parse().unwrap();
                    let size = fields.next().unwrap().parse().unwrap();
                    let group = match fields.next().unwrap() {
                        "none" => 0,
                        "walls" => 1,
                        "projectors" => 2,
                        "turrets" => 3,
                        "transportation" => 4,
                        "power" => 5,
                        "liquids" => 6,
                        "drills" => 7,
                        "units" => 8,
                        "logic" => 9,
                        "payloads" => 10,
                        "heat" => 11,
                        other => panic!("unknown official block group {other}"),
                    };
                    let replaceable = fields.next().unwrap().parse().unwrap();
                    let always_replace = fields.next().unwrap().parse().unwrap();
                    let rotate = fields.next().unwrap().parse().unwrap();
                    let quick_rotate = fields.next().unwrap().parse().unwrap();
                    let privileged = fields.next().unwrap().parse().unwrap();
                    (
                        id,
                        BlockPlacement {
                            size,
                            group,
                            replaceable,
                            always_replace,
                            rotate,
                            quick_rotate,
                            privileged,
                        },
                    )
                })
                .collect()
        })
        .get(&block)
        .unwrap_or(&BlockPlacement::default())
}

fn group_any_replace(group: u8) -> bool {
    matches!(group, 1 | 2 | 3 | 4 | 6 | 9 | 10 | 11)
}

/// Exact data-driven equivalent of `Block.canReplace` for vanilla 158.1.
pub fn block_can_replace(new_block: i16, existing_block: i16) -> bool {
    let new = block_placement(new_block);
    let existing = block_placement(existing_block);
    if existing.always_replace {
        return true;
    }
    if existing.privileged || !existing.replaceable {
        return false;
    }
    let same_or_group =
        new_block == existing_block || (new.group != 0 && new.group == existing.group);
    same_or_group
        && (new.size == existing.size
            || (new.size >= existing.size && group_any_replace(new.group)))
}

#[derive(Clone, Copy, Default)]
pub struct UnitMovement {
    pub hit_size: f32,
    pub physics: bool,
    pub allow_leg_step: bool,
    pub leg_physics_layer: bool,
    pub flying: bool,
    pub naval: bool,
}

/// One player-triggerable weapon mount from the official v158.1 unit table.
///
/// Repair beams, point-defense mounts and the builder beam use non-projectile
/// services (healing/interception/construction) and therefore stay out of
/// this list. Every ordinary projectile row is driven by `Unit.isShooting`,
/// including each independently reloading side mount.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitWeapon {
    pub reload: f32,
    pub shots: u8,
    pub bullet_id: i16,
    pub speed: f32,
    pub damage: f32,
    pub lifetime: f32,
    pub splash_damage: f32,
    pub splash_radius: f32,
    pub pierce_units: bool,
    pub pierce_buildings: bool,
    pub status_effect: i16,
    pub status_duration: f32,
}

fn autonomous_unit_weapon(bullet_id: i16) -> bool {
    matches!(
        bullet_id,
        // repair beams
        19 | 50 | 55 |
        // point-defense mounts
        54 | 58 | 59 | 61 | 62 | 63 | 64 | 91 |
        // Manually controllable RepairBeamWeapon core tools use a healing
        // ray service, not Projectile/Bullet damage.
        108 | 109 | 111 |
        // BuildWeapon (incite)
        110
    )
}

/// Player-triggerable mounts for `unit`, in the same stable order as
/// `UnitType.weapons`. The TSV is generated from the desktop 158.1 content
/// registry, so new content rows become authoritative without duplicating a
/// second hand-written unit-id switch in the simulation.
pub fn unit_weapons(unit: i16) -> &'static [UnitWeapon] {
    UNIT_WEAPONS
        .get_or_init(|| {
            let mut by_unit: HashMap<i16, Vec<UnitWeapon>> = HashMap::new();
            for line in include_str!("unit_weapons.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
            {
                let mut fields = line.split('\t');
                let unit: i16 = fields.next().unwrap().parse().unwrap();
                let _unit_name = fields.next().unwrap();
                let _weapon_name = fields.next().unwrap();
                let reload = fields.next().unwrap().parse().unwrap();
                let shots = fields.next().unwrap().parse().unwrap();
                let bullet_id = fields.next().unwrap().parse().unwrap();
                let speed = fields.next().unwrap().parse().unwrap();
                let damage = fields.next().unwrap().parse().unwrap();
                let lifetime = fields.next().unwrap().parse().unwrap();
                let splash_damage = fields.next().unwrap().parse().unwrap();
                let splash_radius = fields.next().unwrap().parse().unwrap();
                let pierce_units = fields.next().unwrap().parse().unwrap();
                let pierce_buildings = fields.next().unwrap().parse().unwrap();
                let raw_status: i16 = fields.next().unwrap().parse().unwrap();
                let status_duration = fields.next().unwrap().parse().unwrap();
                if autonomous_unit_weapon(bullet_id) {
                    continue;
                }
                by_unit.entry(unit).or_default().push(UnitWeapon {
                    reload,
                    shots,
                    bullet_id,
                    speed,
                    damage,
                    lifetime,
                    splash_damage,
                    splash_radius,
                    pierce_units,
                    pierce_buildings,
                    // Status id 0 in the exported table is StatusEffects.none.
                    status_effect: if raw_status == 0 { -1 } else { raw_status },
                    status_duration: if raw_status == 0 {
                        0.0
                    } else {
                        status_duration
                    },
                });
            }
            by_unit
        })
        .get(&unit)
        .map_or(&[], Vec::as_slice)
}

pub fn unit_movement(unit: i16) -> UnitMovement {
    *UNIT_MOVEMENT
        .get_or_init(|| {
            include_str!("unit_movement.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let id = fields.next().unwrap().parse().unwrap();
                    let hit_size = fields.next().unwrap().parse().unwrap();
                    let physics = fields.next().unwrap().parse().unwrap();
                    let allow_leg_step = fields.next().unwrap().parse().unwrap();
                    let leg_physics_layer = fields.next().unwrap().parse().unwrap();
                    let flying = fields.next().unwrap().parse().unwrap();
                    let naval = fields.next().unwrap().parse().unwrap();
                    (
                        id,
                        UnitMovement {
                            hit_size,
                            physics,
                            allow_leg_step,
                            leg_physics_layer,
                            flying,
                            naval,
                        },
                    )
                })
                .collect()
        })
        .get(&unit)
        .unwrap_or(&UnitMovement::default())
}

/// Official `Floor.status` / `statusDuration` for environment floors.
pub fn floor_status(floor: i16) -> (i16, f32) {
    FLOOR_STATUS
        .get_or_init(|| {
            include_str!("floor_status.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let block = fields.next().unwrap().parse().unwrap();
                    let _name = fields.next().unwrap();
                    let status: i16 = fields.next().unwrap().parse().unwrap();
                    let duration = fields.next().unwrap().parse().unwrap();
                    (block, (status, duration))
                })
                .collect()
        })
        .get(&floor)
        .copied()
        .unwrap_or((-1, 0.0))
}

/// Official `UnitType.hovering` (158.1).
pub fn unit_hovering(unit: i16) -> bool {
    UNIT_HOVERING
        .get_or_init(|| {
            include_str!("unit_hovering.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let id = fields.next().unwrap().parse().unwrap();
                    (id, ())
                })
                .collect()
        })
        .contains_key(&unit)
}

/// P2-B1: one row from `unit_inventory.tsv` classifying vanilla AI support.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitInventoryEntry {
    pub id: i16,
    pub name: &'static str,
    pub spawnable: &'static str,
    pub default_controller: &'static str,
    pub movement: &'static str,
    pub target_selection: &'static str,
    pub weapons: &'static str,
    pub payload: &'static str,
    pub mine_build: &'static str,
    pub path_cost: &'static str,
    pub rust_status: &'static str,
}

impl UnitInventoryEntry {
    pub fn simulation_supported(&self) -> bool {
        matches!(self.rust_status, "FULL" | "PARTIAL")
    }

    pub fn strict_rejected(&self) -> bool {
        self.rust_status == "REJECTED"
    }
}

/// Inventory row for a vanilla unit id, if registered (0..=68).
pub fn unit_inventory(unit: i16) -> Option<&'static UnitInventoryEntry> {
    UNIT_INVENTORY
        .get_or_init(|| {
            include_str!("unit_inventory.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let id: i16 = fields.next().unwrap().parse().unwrap();
                    let entry = UnitInventoryEntry {
                        id,
                        name: fields.next().unwrap(),
                        spawnable: fields.next().unwrap(),
                        default_controller: fields.next().unwrap(),
                        movement: fields.next().unwrap(),
                        target_selection: fields.next().unwrap(),
                        weapons: fields.next().unwrap(),
                        payload: fields.next().unwrap(),
                        mine_build: fields.next().unwrap(),
                        path_cost: fields.next().unwrap(),
                        rust_status: fields.next().unwrap(),
                    };
                    (id, entry)
                })
                .collect()
        })
        .get(&unit)
}

/// Whether the server can simulate this vanilla unit (has `enemy_spec`).
pub fn unit_simulation_supported(unit: i16) -> bool {
    unit_inventory(unit).is_some_and(|entry| entry.simulation_supported())
}

pub fn block_requirements(block: i16) -> &'static [(usize, i32)] {
    BLOCK_REQUIREMENTS
        .get_or_init(|| {
            include_str!("block_requirements.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let block = fields.next().unwrap().parse().unwrap();
                    let requirements = fields
                        .map(|field| {
                            let (item, amount) = field.split_once(':').unwrap();
                            (item.parse().unwrap(), amount.parse().unwrap())
                        })
                        .collect();
                    (block, requirements)
                })
                .collect()
        })
        .get(&block)
        .map_or(&[], Vec::as_slice)
}

pub fn is_player_buildable(block: i16, sandbox: bool) -> bool {
    !block_requirements(block).is_empty() || (sandbox && (410..=418).contains(&block))
}

pub fn block_size(block: i16) -> u8 {
    *BLOCK_SIZES
        .get_or_init(|| {
            include_str!("block_sizes.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let (block, size) = line.split_once('\t').unwrap();
                    (block.parse().unwrap(), size.parse().unwrap())
                })
                .collect()
        })
        .get(&block)
        .unwrap_or(&1)
}

pub fn block_build_time(block: i16) -> f32 {
    *BLOCK_BUILD_TIMES
        .get_or_init(|| {
            include_str!("block_build_times.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let (block, time) = line.split_once('\t').unwrap();
                    (block.parse().unwrap(), time.parse().unwrap())
                })
                .collect()
        })
        .get(&block)
        .unwrap_or(&20.0)
}

pub fn unit_requirements(unit: i16) -> Option<(f32, &'static [(i16, i32)])> {
    UNIT_REQUIREMENTS
        .get_or_init(|| {
            include_str!("unit_requirements.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let unit = fields.next().unwrap().parse().unwrap();
                    let build_time = fields.next().unwrap().parse().unwrap();
                    let requirements = fields
                        .map(|field| {
                            let (item, amount) = field.split_once(':').unwrap();
                            (item.parse().unwrap(), amount.parse().unwrap())
                        })
                        .collect();
                    (unit, (build_time, requirements))
                })
                .collect()
        })
        .get(&unit)
        .map(|(time, requirements)| (*time, requirements.as_slice()))
}

fn block_combat(block: i16) -> (f32, f32) {
    *BLOCK_COMBAT
        .get_or_init(|| {
            include_str!("block_combat.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let block = fields.next().unwrap().parse().unwrap();
                    let health = fields.next().unwrap().parse().unwrap();
                    let armor = fields.next().unwrap().parse().unwrap();
                    (block, (health, armor))
                })
                .collect()
        })
        .get(&block)
        .unwrap_or(&(1.0, 0.0))
}

/// Maximum building health after official base-content initialization.
pub fn block_health(block: i16) -> f32 {
    block_combat(block).0
}

/// Flat building armor used by `Damage.applyArmor`.
pub fn block_armor(block: i16) -> f32 {
    block_combat(block).1
}

pub fn block_navigation(block: i16) -> BlockNavigation {
    *BLOCK_NAVIGATION
        .get_or_init(|| {
            include_str!("block_navigation.tsv")
                .lines()
                .filter(|line| !line.starts_with('#') && !line.is_empty())
                .map(|line| {
                    let mut fields = line.split('\t');
                    let block = fields.next().unwrap().parse().unwrap();
                    let parse_bool = |field: Option<&str>| field == Some("true");
                    (
                        block,
                        BlockNavigation {
                            solid: parse_bool(fields.next()),
                            team_passable: parse_bool(fields.next()),
                            floor: parse_bool(fields.next()),
                            deep: parse_bool(fields.next()),
                            damages: parse_bool(fields.next()),
                        },
                    )
                })
                .collect()
        })
        .get(&block)
        .unwrap_or(&BlockNavigation::default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Item {
    Copper = 0,
    Lead = 1,
    Metaglass = 2,
    Graphite = 3,
    Sand = 4,
    Coal = 5,
    Titanium = 6,
    Thorium = 7,
    Scrap = 8,
    Silicon = 9,
    Plastanium = 10,
    PhaseFabric = 11,
    SurgeAlloy = 12,
    SporePod = 13,
    BlastCompound = 14,
    Pyratite = 15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Liquid {
    Water = 0,
    Slag = 1,
    Oil = 2,
    Cryofluid = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum BlockType {
    Conveyor = 0,
    Router = 1,
    Junction = 2,
    Turret = 3,
    CoreShard = 4,
    CoreFoundation = 5,
    CoreNucleus = 6,
    PowerNode = 7,
    SolarPanel = 8,
    Battery = 9,
    Wall = 10,
    Factory = 11,
}

pub struct ContentRegistry {
    pub items: HashMap<u16, Item>,
    pub liquids: HashMap<u16, Liquid>,
    pub blocks: HashMap<u16, BlockType>,
}

impl Default for ContentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentRegistry {
    pub fn new() -> Self {
        let mut items = HashMap::new();
        items.insert(Item::Copper as u16, Item::Copper);
        items.insert(Item::Lead as u16, Item::Lead);
        items.insert(Item::Metaglass as u16, Item::Metaglass);
        items.insert(Item::Graphite as u16, Item::Graphite);
        items.insert(Item::Sand as u16, Item::Sand);
        items.insert(Item::Coal as u16, Item::Coal);
        items.insert(Item::Titanium as u16, Item::Titanium);
        items.insert(Item::Thorium as u16, Item::Thorium);
        items.insert(Item::Scrap as u16, Item::Scrap);
        items.insert(Item::Silicon as u16, Item::Silicon);
        items.insert(Item::Plastanium as u16, Item::Plastanium);
        items.insert(Item::PhaseFabric as u16, Item::PhaseFabric);
        items.insert(Item::SurgeAlloy as u16, Item::SurgeAlloy);
        items.insert(Item::SporePod as u16, Item::SporePod);
        items.insert(Item::BlastCompound as u16, Item::BlastCompound);
        items.insert(Item::Pyratite as u16, Item::Pyratite);

        let mut liquids = HashMap::new();
        liquids.insert(Liquid::Water as u16, Liquid::Water);
        liquids.insert(Liquid::Slag as u16, Liquid::Slag);
        liquids.insert(Liquid::Oil as u16, Liquid::Oil);
        liquids.insert(Liquid::Cryofluid as u16, Liquid::Cryofluid);

        let mut blocks = HashMap::new();
        blocks.insert(BlockType::Conveyor as u16, BlockType::Conveyor);
        blocks.insert(BlockType::Router as u16, BlockType::Router);
        blocks.insert(BlockType::Junction as u16, BlockType::Junction);
        blocks.insert(BlockType::Turret as u16, BlockType::Turret);
        blocks.insert(BlockType::CoreShard as u16, BlockType::CoreShard);
        blocks.insert(BlockType::CoreFoundation as u16, BlockType::CoreFoundation);
        blocks.insert(BlockType::CoreNucleus as u16, BlockType::CoreNucleus);
        blocks.insert(BlockType::PowerNode as u16, BlockType::PowerNode);
        blocks.insert(BlockType::SolarPanel as u16, BlockType::SolarPanel);
        blocks.insert(BlockType::Battery as u16, BlockType::Battery);
        blocks.insert(BlockType::Wall as u16, BlockType::Wall);
        blocks.insert(BlockType::Factory as u16, BlockType::Factory);

        Self {
            items,
            liquids,
            blocks,
        }
    }

    pub fn get_item(&self, id: u16) -> Option<Item> {
        self.items.get(&id).copied()
    }

    pub fn get_liquid(&self, id: u16) -> Option<Liquid> {
        self.liquids.get(&id).copied()
    }

    pub fn get_block(&self, id: u16) -> Option<BlockType> {
        self.blocks.get(&id).copied()
    }
}

#[cfg(test)]
mod unit_weapon_tests {
    use super::*;

    const NON_GENERIC_BULLETS: &[i16] = &[
        19, 50, 54, 55, 58, 59, 61, 62, 63, 64, 91, 108, 109, 110, 111,
    ];

    #[derive(Debug)]
    struct RawWeapon {
        unit: i16,
        unit_name: &'static str,
        weapon_name: &'static str,
        weapon: UnitWeapon,
    }

    fn raw_weapons() -> Vec<RawWeapon> {
        include_str!("unit_weapons.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .map(|line| {
                let mut fields = line.split('\t');
                let unit = fields.next().unwrap().parse().unwrap();
                let unit_name = fields.next().unwrap();
                let weapon_name = fields.next().unwrap();
                let reload = fields.next().unwrap().parse().unwrap();
                let shots = fields.next().unwrap().parse().unwrap();
                let bullet_id = fields.next().unwrap().parse().unwrap();
                let speed = fields.next().unwrap().parse().unwrap();
                let damage = fields.next().unwrap().parse().unwrap();
                let lifetime = fields.next().unwrap().parse().unwrap();
                let splash_damage = fields.next().unwrap().parse().unwrap();
                let splash_radius = fields.next().unwrap().parse().unwrap();
                let pierce_units = fields.next().unwrap().parse().unwrap();
                let pierce_buildings = fields.next().unwrap().parse().unwrap();
                let raw_status: i16 = fields.next().unwrap().parse().unwrap();
                let raw_status_duration = fields.next().unwrap().parse().unwrap();
                assert!(fields.next().is_none(), "unexpected TSV field in {line}");
                RawWeapon {
                    unit,
                    unit_name,
                    weapon_name,
                    weapon: UnitWeapon {
                        reload,
                        shots,
                        bullet_id,
                        speed,
                        damage,
                        lifetime,
                        splash_damage,
                        splash_radius,
                        pierce_units,
                        pierce_buildings,
                        status_effect: if raw_status == 0 { -1 } else { raw_status },
                        status_duration: if raw_status == 0 {
                            0.0
                        } else {
                            raw_status_duration
                        },
                    },
                }
            })
            .collect()
    }

    fn player_controllable(unit: i16) -> bool {
        // MissileUnitType plus manifold/assembly-drone/scathe missiles have
        // `playerControllable=false` in the official v158.1 registry.
        !matches!(unit, 46 | 53 | 55 | 62..=67)
    }

    #[test]
    fn unit_weapon_registry_matches_every_offensive_tsv_field_and_order() {
        let raw = raw_weapons();
        assert_eq!(raw.len(), 91, "the v158.1 export has 91 mount rows");

        let mut expected: HashMap<i16, Vec<UnitWeapon>> = HashMap::new();
        for row in &raw {
            assert_eq!(
                crate::game::unit_types::unit_name_from_id(row.unit),
                Some(row.unit_name),
                "unit id/name mismatch in TSV"
            );
            assert!(row.weapon.reload > 0.0 && row.weapon.reload.is_finite());
            assert!(row.weapon.shots > 0);
            assert!(row.weapon.bullet_id >= 0);
            assert!(row.weapon.speed.is_finite() && row.weapon.speed >= 0.0);
            assert!(row.weapon.damage.is_finite() && row.weapon.damage >= 0.0);
            assert!(row.weapon.lifetime.is_finite() && row.weapon.lifetime > 0.0);
            assert!(row.weapon.splash_damage.is_finite());
            assert!(row.weapon.splash_radius.is_finite());
            assert!(row.weapon.status_duration.is_finite());
            if !NON_GENERIC_BULLETS.contains(&row.weapon.bullet_id) {
                expected.entry(row.unit).or_default().push(row.weapon);
            }
        }

        assert_eq!(expected.values().map(Vec::len).sum::<usize>(), 76);
        for unit in 0..=68 {
            assert_eq!(
                unit_weapons(unit),
                expected.get(&unit).map_or(&[][..], Vec::as_slice),
                "unit {unit} registry must preserve every retained TSV field and mount order"
            );
        }
        assert!(unit_weapons(-1).is_empty());
        assert!(unit_weapons(69).is_empty());
    }

    #[test]
    fn repair_point_defense_and_build_mount_exclusion_is_exact() {
        let raw = raw_weapons();
        let excluded: Vec<_> = raw
            .iter()
            .filter(|row| NON_GENERIC_BULLETS.contains(&row.weapon.bullet_id))
            .map(|row| (row.unit, row.weapon_name, row.weapon.bullet_id))
            .collect();
        assert_eq!(
            excluded,
            vec![
                (8, "repair-beam-weapon-center-large", 19),
                (30, "repair-beam-weapon-center", 50),
                (31, "point-defense-mount", 54),
                (32, "repair-beam-weapon-center", 55),
                (33, "point-defense-mount", 58),
                (33, "point-defense-mount", 59),
                // Navanax plasma mounts keep autoTarget=true and
                // controllable=false; the simulation owns them as one
                // synchronized autonomous group while the unit is possessed.
                (34, "plasma-laser-mount", 61),
                (34, "plasma-laser-mount", 62),
                (34, "plasma-laser-mount", 63),
                (34, "plasma-laser-mount", 64),
                (44, "cleroi-point-defense", 91),
                // These player-controlled Erekir core RepairBeamWeapon mounts
                // are handled by simulation's dedicated repair-beam path, not
                // by generic projectile spawning.
                (58, "", 108),
                (59, "", 109),
                (59, "build-weapon", 110),
                (60, "", 111),
            ]
        );

        for bullet in 0..=195 {
            assert_eq!(
                autonomous_unit_weapon(bullet),
                NON_GENERIC_BULLETS.contains(&bullet),
                "generic-projectile exclusion changed for bullet {bullet}"
            );
        }
        for (unit, _, bullet) in excluded {
            assert!(
                unit_weapons(unit)
                    .iter()
                    .all(|weapon| weapon.bullet_id != bullet),
                "non-generic bullet {bullet} leaked into unit {unit} projectile controls"
            );
        }
        assert!(
            unit_weapons(33).is_empty(),
            "Aegires has only point defense"
        );
        for unit in [58, 59, 60] {
            assert!(
                unit_weapons(unit).is_empty(),
                "Erekir core unit {unit} must use the dedicated RepairBeamWeapon path"
            );
        }
    }

    #[test]
    fn every_player_controllable_unit_with_offensive_rows_is_registered() {
        const EXPECTED: &[(i16, usize)] = &[
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 3),
            (4, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (8, 1),
            (9, 1),
            (10, 1),
            (11, 1),
            (12, 2),
            (13, 4),
            (14, 2),
            (15, 1),
            (16, 1),
            (17, 1),
            (18, 3),
            (19, 3),
            (21, 1),
            (22, 2),
            (23, 1),
            (25, 2),
            (26, 2),
            (27, 2),
            (28, 2),
            (29, 1),
            (30, 2),
            (31, 1),
            (32, 1),
            (34, 1),
            (35, 1),
            (36, 1),
            (37, 1),
            (38, 1),
            (39, 1),
            (40, 1),
            (41, 3),
            (42, 1),
            (43, 1),
            (44, 1),
            (45, 1),
            (47, 1),
            (48, 1),
            (49, 1),
            (50, 1),
            (51, 1),
            (52, 1),
            (54, 1),
        ];

        let actual: Vec<_> = (0..=68)
            .filter(|unit| player_controllable(*unit))
            .filter_map(|unit| {
                let count = unit_weapons(unit).len();
                (count > 0).then_some((unit, count))
            })
            .collect();
        assert_eq!(actual, EXPECTED);
    }

    #[test]
    fn independently_reloading_mount_groups_fit_runtime_slots_except_navanax() {
        let multiple: Vec<_> = (0..=68)
            .filter_map(|unit| {
                let count = unit_weapons(unit).len();
                (count > 1).then_some((unit, count))
            })
            .collect();
        assert_eq!(
            multiple,
            vec![
                (3, 3),
                (12, 2),
                (13, 4),
                (14, 2),
                (18, 3),
                (19, 3),
                (22, 2),
                (25, 2),
                (26, 2),
                (27, 2),
                (28, 2),
                (30, 2),
                (41, 3),
            ]
        );
        let oversized: Vec<_> = (0..=68)
            .filter_map(|unit| {
                let count = unit_weapons(unit).len();
                (count > 4).then_some((unit, count))
            })
            .collect();
        assert!(oversized.is_empty());
        assert_eq!(
            unit_weapons(34)
                .iter()
                .map(|weapon| weapon.bullet_id)
                .collect::<Vec<_>>(),
            vec![60]
        );
    }

    #[test]
    fn representative_weapon_fields_cover_none_status_splash_and_piercing() {
        assert_eq!(
            unit_weapons(3)[0],
            UnitWeapon {
                reload: 45.0,
                shots: 3,
                bullet_id: 10,
                speed: 8.0,
                damage: 70.0,
                lifetime: 27.0,
                splash_damage: 0.0,
                splash_radius: -1.0,
                pierce_units: false,
                pierce_buildings: false,
                status_effect: -1,
                status_duration: 0.0,
            }
        );
        assert_eq!(
            unit_weapons(11)[0],
            UnitWeapon {
                reload: 9.0,
                shots: 1,
                bullet_id: 22,
                speed: 2.5,
                damage: 13.0,
                lifetime: 57.0,
                splash_damage: 0.0,
                splash_radius: -1.0,
                pierce_units: false,
                pierce_buildings: false,
                status_effect: 8,
                status_duration: 120.0,
            }
        );
        assert_eq!(
            unit_weapons(40)[0],
            UnitWeapon {
                reload: 80.0,
                shots: 1,
                bullet_id: 70,
                speed: 7.0,
                damage: 90.0,
                lifetime: 28.0,
                splash_damage: 50.0,
                splash_radius: 20.0,
                pierce_units: true,
                pierce_buildings: true,
                status_effect: -1,
                status_duration: 0.0,
            }
        );
    }
}
