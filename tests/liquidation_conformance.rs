//! Conformance under liquidation stress (v0.2-B).
//!
//! Runs THREE oracles over the liquidation campaigns and asserts the engine HELD
//! all three across the liquidation:
//!
//!   * O1 — per-account source-domain realizability (`check_observation`,
//!     `src/oracles.rs`), a single-state oracle, on EVERY observation; and
//!   * the v0.1 market-engine realizability CROSS-LINK (`check_observation_market`),
//!     a single-state oracle, on EVERY observation; and
//!   * the v0.2 liquidation insurance-domain ISOLATION oracle
//!     (`liquidation_insurance_isolation`), a CROSS-STEP delta oracle, on every
//!     consecutive `(prev, cur)` pair.
//!
//! The first two are conformance-under-stress (the engine's single-state
//! invariants must still hold across a liquidation campaign); the third is the new
//! cross-step isolation property. If any oracle ever fires, the offending scenario
//! is persisted to `scenarios/liquidation_candidate.json` and the test fails
//! loudly — NEVER weaken the oracle to make this pass.

use perp_adversary::driver::{funded_liquidation_campaign, liquidation_campaign, run};
use perp_adversary::oracles::{
    check_observation, check_observation_market, liquidation_insurance_isolation,
};
use perp_adversary::scenario::Scenario;

/// Run all three oracles over `s`; on the first breach, persist the scenario and
/// return a loud message. Returns `Ok(())` if the engine held all three.
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

    // Cross-step isolation oracle on every consecutive (prev, cur) pair.
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
    }

    Ok(())
}

fn persist_candidate(label: &str, s: &Scenario, step: usize, oracle: &str, detail: &str) -> String {
    let _ = std::fs::create_dir_all("scenarios");
    let path = "scenarios/liquidation_candidate.json";
    let _ = std::fs::write(path, serde_json::to_string_pretty(s).unwrap());
    format!(
        "LIQUIDATION CANDIDATE [{label}] at step {step}: {oracle} :: {detail} :: \
         saved {path} :: scenario={}",
        serde_json::to_string(s).unwrap()
    )
}

#[test]
fn engine_holds_all_oracles_on_residual_liquidation_campaign() {
    // The no-insurance path: the engine books residual, spends no insurance.
    let s = liquidation_campaign();
    if let Err(msg) = run_all_oracles("residual", &s) {
        panic!("{msg}");
    }
}

#[test]
fn engine_holds_all_oracles_on_funded_liquidation_campaign() {
    // The funded path: the engine genuinely spends insurance for the liquidated
    // domain — the stronger input for the isolation oracle.
    let s = funded_liquidation_campaign();
    if let Err(msg) = run_all_oracles("funded", &s) {
        panic!("{msg}");
    }
}
