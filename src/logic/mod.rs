//! Mindustry logic execution — phase 1 subset of the official LExecutor.
//!
//! The official client ships the processor program as SOURCE CODE inside the
//! compressed config container (`LogicBlock.compress`, verified in
//! target/audit/logic_bytecode_158.md); the server compiles it locally with
//! `LAssembler` + `LParser` and runs `LExecutor` every tick. This module ports
//! that pipeline for the statements that matter most in real factories and
//! defenses:
//!
//!   set, op, read, write, jump, wait, getlink, sensor, print, printchar,
//!   draw/drawflush, printflush, control(enabled), setrate, end, stop,
//!   packcolor, unpackcolor,
//!   label
//!
//! Every other statement compiles to a `NoOp` that preserves instruction
//! indices (so jump targets stay aligned) and warns once. Object values are
//! buildings and units: `ubind` binds units round-robin from the processor
//! team's candidates (P0-02, official UnitBindI semantics), and unit objects
//! flow through radar/fetch/sensor/ucontrol.
//!
//! Numeric semantics mirror LogicOp.java / ConditionOp.java exactly (verified
//! against the v158.1 source): double math, `equal` with 0.000001 epsilon,
//! strictEqual on object identity, unary ops, degrees for trig.

mod compiler;
mod container;
mod executor;
mod ops;
mod view;

// Re-export the whole `logic` API under `crate::logic::*` exactly as it was
// when `logic` was a single file. Several items are only referenced from
// `#[cfg(test)]` or internally, so silence the unused-import lint on the wall.
#[allow(unused_imports)]
pub use compiler::{
    compile, compile_report, parse_links, source_from_config, Assembler, Expr, Program,
};
#[allow(unused_imports)]
pub use container::{
    parse_logic_container, LogicContainer, LOGIC_MAX_COMPRESSED, LOGIC_MAX_LINKS,
    LOGIC_MAX_NAME_LENGTH, LOGIC_MAX_UNCOMPRESSED,
};
#[allow(unused_imports)]
pub use executor::{
    format_number, lvar_bool, lvar_invalid, lvar_team, lvar_unit_type, lvar_unit_type_object,
    run_instruction, ApplyStatusSpec, DrawSpec, DrawType, ExecutorState, Instr, LObject, LVar,
    LogicRule, RadarSort, RadarSpec, RadarTarget, SetPropKey, SetPropSpec, SetRuleSpec, SpawnSpec,
    UcOp, UlocGroup, UlocKind, UlocSpec,
};
#[allow(unused_imports)]
pub use ops::{
    item_id_from_name, item_name_from_id, liquid_name_from_id, ore_item_id, unit_name_from_id,
    Cond, LAccess, LookupKind, Op,
};
#[allow(unused_imports)]
pub use view::{SensorValue, WorldView};

#[cfg(test)]
mod tests;
