//! RTS/command orders, logic authority and processor leases. Units facade
//! re-exports through crate::network::units::*.

use crate::network::economy::default_unit_command;
use crate::network::units::controller::unit_uses_command_ai;
use crate::network::world::*;
use dashmap::DashMap;

use super::*;

pub(crate) fn unit_order_has_active_rts_target(order: &UnitOrder) -> bool {
    match order.target_kind {
        0 => order.target_x.is_some(),
        1 | 2 => order.target_id >= 0,
        _ => false,
    }
}

/// Whether the unit currently follows an active RTS command, independent of
/// who owns the controller.
pub(crate) fn unit_has_active_rts_command(world: &DynamicWorld, unit_id: i32) -> bool {
    world
        .unit_orders
        .get(&unit_id)
        .is_some_and(|order| unit_order_has_active_rts_target(&order))
}

/// `CommandAI.maxCommandQueueSize` (CommandAI.java:19: `maxCommandQueueSize
/// = 50`). The queue never holds more entries; the wire codec writes the
/// count as one byte so 50 always fits.
pub(crate) const MAX_COMMAND_QUEUE_SIZE: usize = 50;

/// Java equality for queued command targets — what
/// `CommandAI.commandQueue`'s `!commandQueue.contains(location)`
/// (CommandAI.java:500) compares with. Arc `Seq.contains` uses `.equals()`
/// (Arc/arc-core/src/arc/struct/Seq.java:574-595), and:
///
/// - `Vec2.equals` compares x and y by exact `Float.floatToIntBits`
///   (Arc/arc-core/src/arc/math/geom/Vec2.java:611-618), so two DISTINCT
///   Vec2 instances with identical coordinates are duplicates;
/// - `Building` and `Unit` do not override `equals`, so entity equality is
///   instance identity — the same building tile / same unit id. A Building
///   is never equal to a Unit or a Vec2 (class check), hence the kind match.
pub(crate) fn targets_equal(a: &UnitOrderTarget, b: &UnitOrderTarget) -> bool {
    match (a.kind, b.kind) {
        (1, 1) | (2, 2) => a.id == b.id,
        (0, 0) => a.x.to_bits() == b.x.to_bits() && a.y.to_bits() == b.y.to_bits(),
        _ => false,
    }
}

/// Makes `target` the order's ACTIVE command — `CommandAI.commandTarget`
/// (`attackTarget = moveTo`, CommandAI.java:590-593) for kinds 1-2 and
/// `CommandAI.commandPosition` (`targetPos = pos; attackTarget = null`,
/// CommandAI.java:567-580) for positions.
///
/// Java keeps a one-update transient for kinds 1-2 (`attackTarget` set,
/// `targetPos` still null, `hasCommand()` false) until `defaultBehavior`
/// materializes `targetPos` (CommandAI.java:256-261). Logic never reads that
/// transient: `Logic.updateEntities` runs unit updates before building
/// updates, so `isLogicControllable()` at the LogicBlock phase always sees
/// the post-update state (ParCommandTiming158). The Rust order therefore
/// materializes coordinates eagerly here — no separate transient slot.
pub(crate) fn set_order_active_target(order: &mut UnitOrder, target: UnitOrderTarget) {
    order.target_kind = target.kind;
    order.target_id = target.id;
    order.target_x = Some(target.x);
    order.target_y = Some(target.y);
}

/// Clears the ACTIVE target only (`targetPos = null; attackTarget = null`)
/// without touching the queue — the invalidation tail of `defaultBehavior`
/// (CommandAI.java:244-247) and `clearCommands` minus its queue clear
/// (CommandAI.java:177-181).
pub(crate) fn clear_order_active_target(order: &mut UnitOrder) {
    order.target_kind = 0;
    order.target_id = -1;
    order.target_x = None;
    order.target_y = None;
}

/// `CommandAI.commandQueue(Position)` (CommandAI.java:493-503), the single
/// queued-order entry point `InputHandler.commandUnits` drives
/// (InputHandler.java:343-351). Exact 158.1 semantics:
///
/// 1. NO active target (`targetPos == null && attackTarget == null`) — the
///    target is consumed IMMEDIATELY as the active command, never queued
///    (first-queued-target promotion);
/// 2. otherwise it is appended only when the queue holds fewer than
///    [`MAX_COMMAND_QUEUE_SIZE`] (50) entries AND no equal target is
///    already queued ([`targets_equal`]).
///
/// Returns whether anything changed (a duplicate, or the 51st unique
/// target, leaves the order untouched).
pub(crate) fn queue_unit_target(order: &mut UnitOrder, target: UnitOrderTarget) -> bool {
    if !unit_order_has_active_rts_target(order) {
        set_order_active_target(order, target);
        true
    } else if order.queue.len() < MAX_COMMAND_QUEUE_SIZE
        && !order
            .queue
            .iter()
            .any(|queued| targets_equal(queued, &target))
    {
        order.queue.push(target);
        true
    } else {
        false
    }
}

/// Whether a logic binding holds the unit: Logic authority, or a transient
/// logic order (kinds 6-9) issued by `ucontrol` before LogicAI authority is
/// modeled. RTS orders never set this.
pub(crate) fn unit_bound_to_logic(world: &DynamicWorld, unit_id: i32) -> bool {
    if world
        .enemies
        .get(&unit_id)
        .is_some_and(|unit| matches!(unit.authority, UnitAuthority::Logic { .. }))
    {
        return true;
    }
    world
        .unit_orders
        .get(&unit_id)
        .is_some_and(|order| order.target_kind >= 6)
}

/// Rust counterpart of the `UnitController.isLogicControllable()`
/// dispatch: Player => false, CommandAI => `!hasCommand()`, everything
/// else (wave AI, LogicAI) => true. This is the authority-driven single
/// source for logic-control eligibility; callers must not re-derive it
/// from `unit_orders` key existence.
pub(crate) fn unit_is_logic_controllable(world: &DynamicWorld, unit_id: i32) -> bool {
    let authority = world.enemies.get(&unit_id).map(|unit| unit.authority);
    match authority {
        None | Some(UnitAuthority::Player { .. }) => false,
        Some(UnitAuthority::Command) => !unit_has_active_rts_command(world, unit_id),
        Some(UnitAuthority::DefaultAi | UnitAuthority::Logic { .. }) => true,
    }
}

/// The authority `Unit.resetController()` (`type.createController`)
/// installs: CommandAI for player-commandable teams, plain AI otherwise.
pub(crate) fn default_unit_authority(world: &DynamicWorld, unit: &EnemyUnit) -> UnitAuthority {
    if unit_uses_command_ai(Some(world), unit) {
        UnitAuthority::Command
    } else {
        UnitAuthority::DefaultAi
    }
}

/// Returns the unit's controller to its default (`Unit.resetController()`).
/// For teams whose units are player-commandable that is Command authority
/// WITHOUT an active target — `hasCommand()` stays false.
pub(crate) fn reset_unit_authority(world: &DynamicWorld, unit_id: i32) {
    // dashmap-guard: allow DM900 reason="default_unit_authority reads wave rules and game state only; it does not access world.enemies"
    let authority = world
        .enemies
        .get(&unit_id)
        .map(|unit| default_unit_authority(world, &unit));
    if let (Some(mut unit), Some(authority)) = (world.enemies.get_mut(&unit_id), authority) {
        unit.authority = authority;
    }
}

/// Binds the unit to a logic processor — the Rust counterpart of
/// `LExecutor.checkLogicAI` installing a LogicAI controller, including its
/// `isLogicControllable()` gate. Returns false (and changes nothing) when
/// another controller currently owns the unit.
pub(crate) fn acquire_logic_control(
    world: &DynamicWorld,
    unit_id: i32,
    processor_pos: i32,
    remaining_ticks: f32,
) -> bool {
    if !unit_is_logic_controllable(world, unit_id) {
        return false;
    }
    let processor_generation = world
        .tiles
        .get(&processor_pos)
        .map(|tile| {
            crate::network::world::note_building_generation(tile.generation);
            tile.generation
        })
        .unwrap_or(0);
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.authority = UnitAuthority::Logic {
            processor_pos,
            remaining_ticks,
            processor_generation,
        };
        true
    } else {
        false
    }
}

/// `LogicAI.logicControlTimeout` (LogicAI.java:18: `60f * 10f`) expressed in
/// TICKS. Arc's `Time.delta` is `getDeltaTime() * 60f` on the official
/// server (ServerControl.java:197), i.e. exactly 1.0 per game tick at 60
/// TPS, so the 600-unit timeout (`controlTimer -= Time.delta`,
/// LogicAI.java:59-64) expires after 600 ticks (10 seconds) and every valid
/// `ucontrol`/`ulocate` resets it (`ai.controlTimer =
/// LogicAI.logicControlTimeout`, LExecutor.java:238/351).
pub(crate) const LOGIC_CONTROL_TIMEOUT_TICKS: f32 = 60.0 * 10.0;

/// Whether the block may hold a unit's Logic lease: the processor family
/// micro 431, logic 432, hyper 433 and the (privileged) world processor
/// 442. A lease holder whose tile is gone or was replaced by any other
/// building is invalid the way Java's `Building.isValid()`
/// (`tile.build == this && !dead`, Building.class 158.1) is.
pub(crate) fn block_is_logic_processor(block: i16) -> bool {
    matches!(block, 431..=433 | 442)
}

/// Whether the lease still points at the same processor instance Java would
/// keep as `LogicAI.controller`: the tile exists, holds a processor block,
/// and the generation matches the Building that acquired the lease
/// (`tile.build == this && !dead`).
pub(crate) fn processor_lease_valid(
    world: &DynamicWorld,
    processor_pos: i32,
    processor_generation: u64,
) -> bool {
    world.tiles.get(&processor_pos).is_some_and(|tile| {
        crate::network::world::note_building_generation(tile.generation);
        let identity = tile.identity();
        block_is_logic_processor(tile.block)
            && identity.position == processor_pos
            && identity.generation == processor_generation
    })
}

/// Clears the transient logic order state — kinds 6-9 (logic mine, aim+fire,
/// aim, logic build) — plus the unit's logic mine progress. This is the
/// port's observable equivalent of dropping a `LogicAI` controller: the
/// official controller object carried all of that state (`control`/moveX/
/// moveY, `mineTile`, the build `plan`, aiming), so both the "clear old
/// state" block of `checkLogicAI` (first takeover) and every
/// `resetController()` path (`ucontrol unbind`, lease expiry) map to this
/// cleanup.
///
/// Does **not** touch [`EnemyUnit::build_plans`] (`BuilderComp.plans`):
/// Java's `resetController()` leaves the placement queue alone. First
/// LogicAI acquisition clears it separately via [`clear_builder_plans`].
pub(crate) fn clear_transient_logic_orders(world: &DynamicWorld, unit_id: i32) {
    if let Some(mut order) = world.unit_orders.get_mut(&unit_id) {
        if order.target_kind >= 6 {
            order.target_kind = 0;
            order.target_x = None;
            order.target_y = None;
            order.target_id = -1;
        }
    }
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.mine_progress = 0.0;
    }
}

/// Official `unit.clearBuilding()` (`BuilderComp.plans.clear()`, desktop
/// 158.1 BuilderComp.java:267-269). First LogicAI takeover is the only
/// `checkLogicAI` path that runs it; a later processor refresh keeps the
/// same LogicAI object and does not empty the queue.
pub(crate) fn clear_builder_plans(world: &DynamicWorld, unit_id: i32) {
    if let Some(mut unit) = world.enemies.get_mut(&unit_id) {
        unit.build_plans.clear();
    }
}

/// First-acquisition cleanup of `UnitControlI.checkLogicAI` when the unit
/// is not already under a LogicAI (desktop 158.1 LExecutor.java:328-336,
/// verified with `javap` on desktop.jar 158.1): `unit.mineTile = null`,
/// `unit.clearBuilding()`, and a brand-new LogicAI (empty control/plan/
/// aim). Maps to dropping kinds 6-9, `mine_progress`, and `build_plans`.
pub(crate) fn clear_on_first_logic_takeover(world: &DynamicWorld, unit_id: i32) {
    clear_transient_logic_orders(world, unit_id);
    clear_builder_plans(world, unit_id);
}

/// Rust port of `UnitControlI.checkLogicAI` (desktop 158.1
/// LExecutor.java:322-338) followed by the lease refresh every executed
/// `ucontrol`/`ulocate` performs on the passing unit
/// (LExecutor.java:238/351). Conditions, all evaluated before anything
/// changes (a failed gate is a complete no-op):
///
/// - `unitObj instanceof Unit unit && unit.isValid()` — a live bound unit;
/// - `exec.unit.obj() == unit` — trivially holds, the caller passes the
///   executor's own `bound_unit`;
/// - `unit.team == exec.team || exec.privileged`;
/// - `unit.controller().isLogicControllable()` — the P0-01 gate
///   ([`unit_is_logic_controllable`]).
///
/// On pass: a unit not yet under Logic control is taken over with the
/// official "clear old state" cleanup (`unit.mineTile = null` and
/// `unit.clearBuilding()` via [`clear_on_first_logic_takeover`]). A unit
/// already under LogicAI only has its controller pointer / lease refreshed
/// — Java does **not** repeat the mining or building wipe. The lease is
/// re-pointed at THIS processor in both branches (Java assigns
/// `la.controller = exec.thisv.building()`), so a second processor legally
/// steals control with its own position, and the timer is set to
/// [`LOGIC_CONTROL_TIMEOUT_TICKS`].
pub(crate) fn refresh_logic_control(
    world: &DynamicWorld,
    bound_unit: Option<i32>,
    processor_pos: i32,
    processor_team: u8,
    privileged: bool,
) -> bool {
    let Some(unit_id) = bound_unit else {
        return false;
    };
    let Some(unit) = world.enemies.get(&unit_id) else {
        return false;
    };
    // unit.isValid(): dead units (health <= 0) are never controlled.
    if unit.health <= 0.0 {
        return false;
    }
    let unit_team = unit.team;
    let was_logic = matches!(unit.authority, UnitAuthority::Logic { .. });
    drop(unit);
    if unit_team != processor_team && !privileged {
        return false;
    }
    if !unit_is_logic_controllable(world, unit_id) {
        return false;
    }
    if !was_logic {
        // First takeover — official "clear old state" (LExecutor.java:331-334).
        clear_on_first_logic_takeover(world, unit_id);
    }
    acquire_logic_control(world, unit_id, processor_pos, LOGIC_CONTROL_TIMEOUT_TICKS)
}

/// Rust port of `unit.resetController()` restricted to logic control: drops
/// the Logic authority (back to the team's default controller) together with
/// the transient logic order state that represented the LogicAI object.
///
/// `resetController()` installs a BRAND-NEW controller
/// (`controller(type.createController(self()))`, UnitComp.java:465-467), so
/// nothing the old object held survives. For the default CommandAI that
/// means an empty `commandQueue`, `targetPos`/`attackTarget` null, empty
/// `stances` and `command = type.defaultCommand` (`CommandAI.init`,
/// CommandAI.java:98-104). The port keeps all of that in the single
/// `UnitOrder` entry, so the release must clear the active target slot too:
/// while the unit was logic-controlled that slot held the LogicAI's
/// `moveX/moveY` destination, NOT a CommandAI command. Leaving it behind
/// would make the freshly defaulted controller report `hasCommand()` and
/// permanently lock the unit out of `isLogicControllable()`.
pub(crate) fn release_logic_control(world: &DynamicWorld, unit_id: i32) {
    clear_transient_logic_orders(world, unit_id);
    let default_command = world
        .enemies
        .get(&unit_id)
        .map(|unit| crate::network::economy::default_unit_command(unit.unit_type));
    if let (Some(mut order), Some(default_command)) =
        (world.unit_orders.get_mut(&unit_id), default_command)
    {
        clear_order_active_target(&mut order);
        order.queue.clear();
        order.stances = 0;
        order.logic_control = crate::network::world::logic_control::IDLE;
        order.payload_cooldown = 0.0;
        order.command = default_command;
    }
    reset_unit_authority(world, unit_id);
}

/// Whether a command packet may still reach this unit's order state — the
/// Rust counterpart of `unit.controller() instanceof CommandAI`
/// (InputHandler.java:333 commandUnits, :422 setUnitCommand, :456
/// setUnitStance). A POSSESSED unit's controller is the Player and a
/// logic-bound unit's is a LogicAI, so both are skipped entirely: commands
/// never touch the order of a unit a player drives (P0-05 possession
/// lifecycle). Team/type commandability itself stays answered by the
/// actor-team gate the callers already apply.
///
/// Safe to call while holding an `enemies` read guard: neither branch takes
/// a conflicting write lock.
pub(crate) fn unit_command_ai_reachable(world: &DynamicWorld, unit: &EnemyUnit) -> bool {
    !matches!(unit.authority, UnitAuthority::Player { .. }) && !unit_bound_to_logic(world, unit.id)
}

pub(crate) fn fresh_command_order(unit_id: i32, command: u8) -> UnitOrder {
    UnitOrder {
        unit_id,
        command,
        stances: 0,
        payload_cooldown: 0.0,
        target_kind: 0,
        target_id: -1,
        target_x: None,
        target_y: None,
        logic_control: crate::network::world::logic_control::IDLE,
        queue: Vec::new(),
    }
}
