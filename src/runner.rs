//! proptest harness + named-scenario runner.
use crate::driver::{run, Observation};
use crate::scenario::Scenario;

pub type OracleFn = fn(&Observation) -> Result<(), String>;

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
