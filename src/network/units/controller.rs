//! Unit-controller predicates, authority lookups and MSAV controller codecs.
//! The listener adapter re-exports these through crate::network::listener::*.

use crate::network::units::*;
use crate::network::world::*;

use crate::network::units::unit_orders::order_target_building_exists;
use crate::network::wire::encode::write_unit_plans_queue;
use crate::network::wire::transfer::enemy_weapon_mount_count;
use crate::state::game_state::GameMode;

pub(crate) fn unit_player_controllable(unit_type: i16) -> bool {
    !matches!(unit_type, 46 | 53 | 55 | 62..=67)
}

/// Whether vanilla `UnitType.controller` creates CommandAI for this unit.
/// In PvP no team is an AI team; in wave/attack/sandbox modes only the rules'
/// default team is player-commandable unless the AI team has `rtsAi` enabled
/// (UnitType.java:281). The `None` fallback is used only by deterministic
/// codec fixtures, whose allied units are team 1.
pub(crate) fn unit_uses_command_ai(world: Option<&DynamicWorld>, unit: &EnemyUnit) -> bool {
    if !unit_player_controllable(unit.unit_type) {
        return false;
    }
    world.map_or(unit.team == 1, |world| {
        let mode = *world.game_state.mode.read();
        if mode == GameMode::Pvp {
            return true;
        }
        let rules = world.wave_rules.read();
        if unit.team == rules.default_team {
            return true;
        }
        // Official UnitType.controller: AI teams use wave AI unless rtsAi.
        if rules.team_is_ai(unit.team, mode, false) {
            return rules.team_rule(unit.team).rts_ai;
        }
        false
    })
}

pub(crate) fn controlling_player_for_unit(world: &DynamicWorld, unit_id: i32) -> Option<i32> {
    controlling_session_for_unit(world, unit_id).map(|session| session.id)
}

pub(crate) fn controlling_session_for_unit(
    world: &DynamicWorld,
    unit_id: i32,
) -> Option<SessionPlayer> {
    world.player_sessions.iter().find_map(|session| {
        (session.controlled_unit.standard_id() == Some(unit_id)).then(|| session.value().clone())
    })
}

pub(crate) fn controlling_player_for_building(world: &DynamicWorld, position: i32) -> Option<i32> {
    controlling_session_for_building(world, position).map(|session| session.id)
}

pub(crate) fn controlling_session_for_building(
    world: &DynamicWorld,
    position: i32,
) -> Option<SessionPlayer> {
    world.player_sessions.iter().find_map(|session| {
        (session.controlled_unit.building_position() == Some(position))
            .then(|| session.value().clone())
    })
}

pub(crate) fn unit_is_player_controlled(world: &DynamicWorld, unit_id: i32) -> bool {
    controlling_player_for_unit(world, unit_id).is_some()
}

/// Exact desktop 158.1 `TypeIO.writeController` layout. Allied factory units
/// use tag 9 (`CommandAI`), including their current attack/position target,
/// command queue and stance bitset. Wave AI remains tag 2, LogicAI is tag 3
/// plus the processor tile, and a possessed unit is tag 0 + Player id.
pub(crate) fn write_unit_controller_sync(
    output: &mut Vec<u8>,
    world: Option<&DynamicWorld>,
    unit: &EnemyUnit,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    if let Some(player_id) = world
        .and_then(|world| crate::network::units::unit_possessed_by(world, unit.id))
        .or(match unit.authority {
            crate::network::world::UnitAuthority::Player { player_id } => Some(player_id),
            _ => None,
        })
    {
        output.write_b(0)?;
        output.write_i(player_id)?;
        return Ok(());
    }
    if let crate::network::world::UnitAuthority::Logic { processor_pos, .. } = unit.authority {
        output.write_b(3)?;
        output.write_i(processor_pos)?;
        return Ok(());
    }
    if !matches!(
        unit.authority,
        crate::network::world::UnitAuthority::Command
    ) && !unit_uses_command_ai(world, unit)
    {
        output.write_b(2)?;
        return Ok(());
    }

    let order = world.and_then(|world| world.unit_orders.get(&unit.id).map(|order| order.clone()));
    // TypeIO.writeController: attackTarget and targetPos are independent flags.
    let has_attack = order
        .as_ref()
        .is_some_and(|order| matches!(order.target_kind, 1 | 2) && order.target_id >= 0);
    let has_pos = order
        .as_ref()
        .is_some_and(|order| order.target_x.is_some() && order.target_y.is_some());

    output.write_b(9)?;
    output.write_bool(has_attack)?;
    output.write_bool(has_pos)?;
    if has_pos {
        let order = order.as_ref().unwrap();
        output.write_f(order.target_x.unwrap())?;
        output.write_f(order.target_y.unwrap())?;
    }
    if has_attack {
        let order = order.as_ref().unwrap();
        // TypeIO: 1 = Building, 0 = Unit.
        output.write_b(u8::from(order.target_kind == 1))?;
        output.write_i(order.target_id)?;
    }
    // TypeIO: null command is -1 on the wire; read maps that to moveCommand.
    let command_byte = order
        .as_ref()
        .map_or(-1, |order| i8::try_from(order.command).unwrap_or(-1));
    output.write_b(command_byte as u8)?;

    let queue = order
        .as_ref()
        .map_or(&[][..], |order| order.queue.as_slice());
    output.write_b(u8::try_from(queue.len()).unwrap_or(u8::MAX))?;
    for target in queue.iter().take(u8::MAX as usize) {
        match target.kind {
            1 => {
                output.write_b(0)?; // Building
                output.write_i(target.id)?;
            }
            2 => {
                output.write_b(1)?; // Unit
                output.write_i(target.id)?;
            }
            _ => {
                output.write_b(2)?; // Vec2
                output.write_f(target.x)?;
                output.write_f(target.y)?;
            }
        }
    }

    let stances = order.as_ref().map_or(0, |order| order.stances);
    let stance_count = (0..30_u8)
        .filter(|stance| stances & (1_u32 << stance) != 0)
        .count();
    output.write_b(u8::try_from(stance_count).unwrap_or(30))?;
    for stance in 0..30_u8 {
        if stances & (1_u32 << stance) != 0 {
            output.write_b(stance)?; // TypeIO.writeStance = content id byte
        }
    }
    Ok(())
}

/// Parsed `TypeIO.readController` payload (tags 0/2/3/4/5/6/7/8/9).
#[derive(Clone, Debug)]
pub(crate) struct ControllerSnapshot {
    pub tag: u8,
    pub authority: crate::network::world::UnitAuthority,
    pub order: Option<crate::network::world::UnitOrder>,
}

/// Exact desktop 158.1 `TypeIO.readController`. Missing buildings/units in
/// the queue are dropped; a missing attack unit id is kept for `afterRead`.
/// Unknown/negative command ids become move (0). Tag 2 is generic AI.
pub(crate) fn read_unit_controller(
    input: &mut impl crate::network::codec::Reads,
    unit_id: i32,
) -> std::io::Result<ControllerSnapshot> {
    use crate::network::codec::Reads;
    use crate::network::world::{UnitAuthority, UnitOrder, UnitOrderTarget};

    let tag = input.read_b()?;
    match tag {
        0 => {
            let player_id = input.read_i()?;
            Ok(ControllerSnapshot {
                tag,
                authority: UnitAuthority::Player { player_id },
                order: None,
            })
        }
        1 => {
            let _ = input.read_i()?;
            Ok(ControllerSnapshot {
                tag,
                authority: UnitAuthority::DefaultAi,
                order: None,
            })
        }
        3 => {
            let processor_pos = input.read_i()?;
            Ok(ControllerSnapshot {
                tag,
                authority: UnitAuthority::Logic {
                    processor_pos,
                    remaining_ticks: 600.0,
                    processor_generation: 0,
                },
                order: None,
            })
        }
        5 => Ok(ControllerSnapshot {
            tag,
            authority: UnitAuthority::DefaultAi,
            order: None,
        }),
        4 | 6 | 7 | 8 | 9 => {
            let has_attack = input.read_bool()?;
            let has_pos = input.read_bool()?;
            let (target_x, target_y) = if has_pos {
                (Some(input.read_f()?), Some(input.read_f()?))
            } else {
                (None, None)
            };
            let (mut target_kind, mut target_id) = (0u8, -1i32);
            if has_attack {
                let entity_type = input.read_b()?;
                target_id = input.read_i()?;
                target_kind = if entity_type == 1 { 1 } else { 2 };
            }
            let command = if matches!(tag, 6..=9) {
                // TypeIO.read.b() is signed; null (-1) and unknown ids
                // become UnitCommand.moveCommand (id 0). Vanilla 158.1
                // registers 10 unit commands (0..=9).
                let id = input.read_b()? as i8;
                if (0..10).contains(&id) {
                    id as u8
                } else {
                    0
                }
            } else {
                0
            };
            let mut queue = Vec::new();
            if matches!(tag, 7..=9) {
                let length = input.read_b()? as usize;
                for _ in 0..length {
                    match input.read_b()? {
                        0 => {
                            let id = input.read_i()?;
                            queue.push(UnitOrderTarget {
                                kind: 1,
                                id,
                                x: 0.0,
                                y: 0.0,
                            });
                        }
                        1 => {
                            let id = input.read_i()?;
                            queue.push(UnitOrderTarget {
                                kind: 2,
                                id,
                                x: 0.0,
                                y: 0.0,
                            });
                        }
                        2 => {
                            let x = input.read_f()?;
                            let y = input.read_f()?;
                            queue.push(UnitOrderTarget {
                                kind: 0,
                                id: -1,
                                x,
                                y,
                            });
                        }
                        _ => {}
                    }
                }
            }
            let mut stances = 0u32;
            if tag == 8 {
                let stance = input.read_b()?;
                if stance < 30 {
                    stances |= 1 << stance;
                }
            } else if tag == 9 {
                let count = input.read_b()? as usize;
                for _ in 0..count {
                    let stance = input.read_b()?;
                    if stance < 30 {
                        stances |= 1 << stance;
                    }
                }
            }
            Ok(ControllerSnapshot {
                tag,
                authority: UnitAuthority::Command,
                order: Some(UnitOrder {
                    unit_id,
                    command,
                    stances,
                    payload_cooldown: 0.0,
                    target_kind,
                    target_id,
                    target_x,
                    target_y,
                    logic_control: 0,
                    queue,
                }),
            })
        }
        _ => Ok(ControllerSnapshot {
            tag,
            authority: UnitAuthority::DefaultAi,
            order: None,
        }),
    }
}

/// Apply a `TypeIO.readController` snapshot. Missing queue buildings/units
/// are dropped (Java skips null `world.build` / `Groups.unit`); a missing
/// attack-unit id is kept for `CommandAI.afterRead`. A Player tag whose id
/// is not in the world leaves the previous controller untouched.
pub(crate) fn apply_controller_snapshot(
    world: &DynamicWorld,
    unit_id: i32,
    snapshot: ControllerSnapshot,
) {
    if snapshot.tag == 0 {
        let crate::network::world::UnitAuthority::Player { player_id } = snapshot.authority else {
            return;
        };
        let known = world
            .player_sessions
            .iter()
            .any(|session| session.id == player_id)
            || world
                .players
                .iter()
                .any(|player| player.player_id == player_id);
        if !known {
            return;
        }
    }
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.authority = snapshot.authority;
    }
    if let Some(mut order) = snapshot.order {
        order.queue.retain(|target| match target.kind {
            1 => order_target_building_exists(world, target.id),
            2 => world
                .enemies
                .get(&target.id)
                .is_some_and(|unit| unit.health > 0.0),
            _ => true,
        });
        world.unit_orders.insert(unit_id, order);
    } else if !matches!(
        snapshot.authority,
        crate::network::world::UnitAuthority::Command
    ) {
        world.unit_orders.remove(&unit_id);
    }
}

/// Desktop 158.1 post-load controller callbacks: `UnitComp.afterReadAll` →
/// `CommandAI.afterRead` and the LogicAI defaults applied when
/// `TypeIO.readController` creates a fresh LogicAI (controlTimer reset,
/// move/aim runtime fields discarded).
pub(crate) fn finalize_controller_after_load(world: &DynamicWorld, unit_id: i32) {
    use crate::network::world::{logic_control, UnitAuthority};

    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        if let UnitAuthority::Logic {
            ref mut remaining_ticks,
            ..
        } = unit.authority
        {
            *remaining_ticks = 600.0;
        }
    }
    // Read attack state without holding a unit_orders guard: nested DashMap
    // access (orders → enemies/tiles) can deadlock under concurrent load.
    let attack = world.unit_orders.get(&unit_id).and_then(|order| {
        matches!(order.target_kind, 1 | 2).then_some((order.target_kind, order.target_id))
    });
    let resolved = attack.is_none_or(|(target_kind, target_id)| match target_kind {
        1 => order_target_building_exists(world, target_id),
        2 => world
            .enemies
            .get(&target_id)
            .is_some_and(|unit| unit.health > 0.0),
        _ => true,
    });
    let Some(mut order) = world.unit_orders.get_mut(&unit_id) else {
        return;
    };
    order.logic_control = logic_control::IDLE;
    if attack.is_some() && !resolved {
        // attackTarget cleared; targetPos coordinates persist on the wire.
        order.target_kind = 0;
        order.target_id = -1;
    }
}

/// Save/load controller round-trip: write → read → apply → afterRead.
pub(crate) fn roundtrip_controller_save(world: &DynamicWorld, unit_id: i32) -> std::io::Result<()> {
    let unit = world
        .enemies
        .get(&unit_id)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("unit {unit_id} missing for controller roundtrip"),
            )
        })?
        .clone();
    let mut encoded = Vec::new();
    write_unit_controller_sync(&mut encoded, Some(world), &unit)?;
    let snapshot = read_unit_controller(&mut std::io::Cursor::new(encoded), unit_id)?;
    apply_controller_snapshot(world, unit_id, snapshot);
    finalize_controller_after_load(world, unit_id);
    Ok(())
}

/// `ItemStack()` initializes its empty item to copper; generated unit
/// destruction dereferences `item()` unconditionally even when amount is zero.
/// Never serialize a null content ID for a unit stack.
pub(crate) fn valid_item_stack(item: i16, amount: i32) -> (i16, i32) {
    if (0..22).contains(&item) && amount > 0 {
        (item, amount)
    } else {
        (0, 0)
    }
}

pub(crate) fn unit_save_revision(entity_class: u8) -> i16 {
    // Save-codec revision per entity class, extracted from desktop.jar 158.1
    // by invoking each unit's `write()` (DumpUnitRevisions.java). The legacy
    // classes (0 alpha, 30 beta, 31 gamma) write revision 5, not the base
    // UnitEntity's 9 — sending 9 would desync any UnitPayload containing one.
    match entity_class {
        2 | 3 | 4 | 20 | 24 => 9, // block, UnitEntity, MechUnit, naval, LegsUnit
        16 | 21 | 23 => 8,        // mono, spiroct, quad
        5 | 17 | 18 | 26 => 7,    // PayloadUnit, nova, poly, oct
        0 | 19 | 29 | 30 | 31 | 32 | 33 => 5, // LegacyAlpha, pulsar, LegacyBeta/Gamma, ...
        36 | 39 => 3,             // missiles (TimedKillUnit / others)
        43 | 45 | 46 => 2,        // TankUnit, ElevationMoveUnit, CrawlUnit
        _ => 9,
    }
}

/// TypeIO UnitPayload uses the entity's save codec (`Unit.write`), not its
/// network sync codec. Keep this byte-for-byte aligned with generated v158.1
/// unit classes so PayloadUnit snapshots remain readable by the desktop client.
pub(crate) fn write_unit_payload(output: &mut Vec<u8>, unit: &EnemyUnit) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    output.write_bool(true)?;
    output.write_b(0)?; // Payload.payloadUnit
    output.write_b(unit.entity_class)?;
    output.write_s(unit_save_revision(unit.entity_class))?;
    output.write_b(0)?; // abilities
    if matches!(unit.entity_class, 4 | 17 | 19 | 32) {
        output.write_f(unit.rotation)?; // mech baseRotation
    }
    write_unit_controller_sync(output, None, unit)?;
    output.write_f(unit.elevation.clamp(0.0, 1.0))?;
    output.write_l(0)?; // flag
    output.write_f(unit.health)?;
    output.write_bool(false)?;
    output.write_i(-1)?; // mining tile
    let mounts = enemy_weapon_mount_count(unit.unit_type);
    output.write_b(mounts)?;
    for _ in 0..mounts {
        output.write_b(0)?;
        output.write_f(unit.x)?;
        output.write_f(unit.y)?;
    }
    if matches!(unit.entity_class, 5 | 23 | 26) {
        output.write_i(i32::try_from(unit.payloads.len()).unwrap_or(i32::MAX))?;
        for carried in &unit.payloads {
            write_carried_payload(output, carried)?;
        }
    }
    write_unit_plans_queue(output, &unit.build_plans, false)?;
    output.write_f(unit.rotation)?;
    output.write_f(unit.shield)?;
    output.write_bool(false)?;
    output.write_s(-1)?; // carried item
    output.write_i(0)?;
    // B14: official UnitEntity.write revision 9 (JAR offsets 147-191)
    // emits the StatusEntry COLLECTION (`i count + [s id + f duration]`),
    // falling back to the legacy single status only when the collection is
    // empty (mirrors the MSAV body in save_io.rs).
    if !unit.statuses.is_empty() {
        output.write_i(i32::try_from(unit.statuses.len()).unwrap_or(i32::MAX))?;
        for entry in &unit.statuses {
            output.write_s(entry.effect)?;
            output.write_f(entry.time.max(0.0))?;
        }
    } else if unit.status_effect >= 0 {
        output.write_i(1)?;
        output.write_s(unit.status_effect)?;
        output.write_f(unit.status_duration.max(0.0))?;
    } else {
        output.write_i(0)?;
    }
    output.write_b(unit.team)?;
    output.write_s(unit.unit_type)?;
    output.write_bool(unit.update_building)?; // updateBuilding
    output.write_f(unit.velocity_x)?;
    output.write_f(unit.velocity_y)?;
    output.write_f(unit.x)?;
    output.write_f(unit.y)?;
    Ok(())
}

pub(crate) fn write_carried_payload(
    output: &mut Vec<u8>,
    payload: &CarriedPayload,
) -> std::io::Result<()> {
    use crate::network::codec::Writes;

    match payload {
        CarriedPayload::Unit(unit) => write_unit_payload(output, unit),
        CarriedPayload::Build(build) => {
            output.write_bool(true)?;
            output.write_b(1)?; // Payload.payloadBlock
            output.write_s(build.tile.block)?;
            output.write_b(build.version)?;
            if build.sync.is_empty() {
                output.extend_from_slice(&crate::engine::save_io::write_building_all_body(
                    &build.tile,
                )?);
            } else {
                output.extend_from_slice(&build.sync);
            }
            Ok(())
        }
    }
}
