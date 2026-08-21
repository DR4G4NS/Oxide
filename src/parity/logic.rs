//! Parity differential probes — logic domain (M3 rehearsal split from src/parity.rs).
//! Mechanical movement only; assertions unchanged.

use crate::logic::compile;
use crate::logic::ExecutorState;

use serde_json::Value;

use super::{as_str, as_u64, compare_executor_state, fixture, require_fields, validate_common};

fn run_logic_program(program: &str, ticks: u64) -> ExecutorState {
    let compiled = compile(program).expect("fixture program must compile on the Rust side");
    let mut state = ExecutorState::new(compiled, Vec::new());
    for _ in 0..ticks {
        state.run_tick(None, 1);
    }
    state
}

pub(super) fn compare_logic_fixture(fixture: &Value) -> Result<(), String> {
    let probe = validate_common(fixture)?;
    require_fields(
        fixture,
        &probe,
        &["executions", "program", "counter", "text", "vars"],
    )?;
    let program = as_str(fixture, &probe, "program")?;
    let ticks = as_u64(fixture, &probe, "tick")?;
    let executions = as_u64(fixture, &probe, "executions")?;

    if ticks != executions {
        return Err(format!(
            "parity error: fixture '{probe}' is inconsistent: tick={ticks} but executions={executions}"
        ));
    }

    let state = run_logic_program(program, ticks);
    compare_executor_state(&state, fixture, &probe)
}

// ---------------------------------------------------------------------------
// Power probe (ParPower158): PowerNode link/unlink topology
// ---------------------------------------------------------------------------

#[test]
fn logic_probe_601_matches_java_1581() {
    compare_logic_fixture(&fixture("logic-601.json")).unwrap_or_else(|error| panic!("{error}"));
}
