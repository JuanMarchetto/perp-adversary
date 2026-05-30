//! proptest harness + named-scenario runner.
use crate::driver::{run, Observation};
use crate::scenario::Scenario;

pub type OracleFn = fn(&Observation) -> Result<(), String>;

/// A cross-step (delta) oracle over a consecutive `(prev, cur)` observation pair,
/// e.g. the v0.2 liquidation insurance-isolation oracle. Single-state oracles use
/// [`OracleFn`]; this is for invariants that only exist as a CHANGE between steps.
pub type DeltaOracleFn = fn(&Observation, &Observation) -> Result<(), String>;

#[derive(Clone, Debug)]
pub struct StepViolation {
    pub step: usize,
    pub detail: String,
}

/// Execute a scenario; return the first step whose post-state the oracle rejects.
pub fn first_violation(s: &Scenario, oracle: OracleFn) -> Option<StepViolation> {
    let trace = run(s);
    for obs in &trace.observations {
        if let Err(detail) = oracle(obs) {
            return Some(StepViolation {
                step: obs.step,
                detail,
            });
        }
    }
    None
}

/// Execute a scenario; return the first CONSECUTIVE `(prev, cur)` pair the
/// cross-step oracle rejects. The reported `step` is `cur`'s step (the step whose
/// transition broke the invariant). The first observation has no predecessor, so
/// the scan starts at the second.
pub fn first_violation_delta(s: &Scenario, oracle: DeltaOracleFn) -> Option<StepViolation> {
    let trace = run(s);
    for pair in trace.observations.windows(2) {
        let (prev, cur) = (&pair[0], &pair[1]);
        if let Err(detail) = oracle(prev, cur) {
            return Some(StepViolation {
                step: cur.step,
                detail,
            });
        }
    }
    None
}
