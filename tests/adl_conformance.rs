//! Conformance under quantity-ADL stress (v0.3-B).
//!
//! Runs FOUR oracles over the ADL campaign and asserts the engine HELD all four
//! across the auto-deleverage:
//!
//!   * O1 — per-account source-domain realizability (`check_observation`,
//!     `src/oracles.rs`), a single-state oracle, on EVERY observation; and
//!   * the v0.1 market-engine realizability CROSS-LINK (`check_observation_market`),
//!     a single-state oracle, on EVERY observation; and
//!   * the v0.2 liquidation insurance-domain ISOLATION oracle
//!     (`liquidation_insurance_isolation`), a CROSS-STEP delta oracle, on every
//!     consecutive `(prev, cur)` pair (a no-op on non-liquidation steps); and
//!   * the v0.3 quantity-ADL EXACT-ACCOUNTING oracle (`adl_accounting`), a
//!     CROSS-STEP delta oracle, on every consecutive `(prev, cur)` pair (the new
//!     property — fires on the ApplyAdl step).
//!
//! The first three are conformance-under-stress (the engine's existing invariants
//! must still hold across an ADL campaign); the fourth is the new cross-step
//! exact-accounting property. If any oracle ever fires, the offending scenario is
//! persisted to `scenarios/adl_candidate.json` and the test fails loudly — NEVER
//! weaken the oracle to make this pass.

use perp_adversary::driver::{adl_campaign, run};
use perp_adversary::oracles::{
    adl_accounting, check_observation, check_observation_market, liquidation_insurance_isolation,
};
use perp_adversary::runner::first_violation_delta;
use perp_adversary::scenario::Scenario;

/// Run all four oracles over `s`; on the first breach, persist the scenario and
/// return a loud message. Returns `Ok(())` if the engine held all four.
fn run_all_oracles(label: &str, s: &Scenario) -> Result<(), String> {
    let trace = run(s);

    // Single-state oracles on every observation (conformance under stress).
    for obs in &trace.observations {
        if let Err(v) = check_observation(obs) {
            return Err(persist_candidate(
                label,
                s,
                obs.step,
                "O1 realizability",
                &v.detail,
            ));
        }
        if let Err(v) = check_observation_market(obs) {
            return Err(persist_candidate(
                label,
                s,
                obs.step,
                "v0.1 market cross-link",
                &v.detail,
            ));
        }
    }

    // Cross-step oracles on every consecutive (prev, cur) pair.
    for pair in trace.observations.windows(2) {
        let (prev, cur) = (&pair[0], &pair[1]);
        if let Err(v) = liquidation_insurance_isolation(prev, cur) {
            return Err(persist_candidate(
                label,
                s,
                cur.step,
                "v0.2 insurance isolation",
                &v.detail,
            ));
        }
        if let Err(v) = adl_accounting(prev, cur) {
            return Err(persist_candidate(
                label,
                s,
                cur.step,
                "v0.3 ADL accounting",
                &v.detail,
            ));
        }
    }

    Ok(())
}

fn persist_candidate(label: &str, s: &Scenario, step: usize, oracle: &str, detail: &str) -> String {
    let _ = std::fs::create_dir_all("scenarios");
    let path = "scenarios/adl_candidate.json";
    let _ = std::fs::write(path, serde_json::to_string_pretty(s).unwrap());
    format!(
        "ADL CANDIDATE [{label}] at step {step}: {oracle} :: {detail} :: \
         saved {path} :: scenario={}",
        serde_json::to_string(s).unwrap()
    )
}

#[test]
fn engine_holds_all_oracles_on_adl_campaign() {
    // The ADL campaign: seed a finalized-close, ADL-eligible state, then apply a
    // real quantity-ADL that closes POS_SCALE and advances the close-progress
    // ledger 0 -> POS_SCALE — the non-vacuous input for the accounting oracle.
    let s = adl_campaign();
    if let Err(msg) = run_all_oracles("adl", &s) {
        panic!("{msg}");
    }
}

#[test]
fn adl_accounting_holds_via_first_violation_delta() {
    // Wire the v0.3 ADL accounting oracle through the cross-step runner exactly as
    // a delta oracle is meant to be folded over a trace. The runner's
    // `DeltaOracleFn` returns `Result<(), String>`, so adapt the `Violation`.
    let s = adl_campaign();
    let v = first_violation_delta(&s, |prev, cur| {
        adl_accounting(prev, cur).map_err(|e| e.detail)
    });
    assert!(
        v.is_none(),
        "engine must hold ADL exact-accounting across the campaign; got {v:?}"
    );
}
