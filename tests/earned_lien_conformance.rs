//! Conformance over the v0.4 EARNED-STATE lien campaign.
//!
//! Runs ALL FOUR oracles over the earned-lien campaign and asserts the engine
//! HELD all four on the EARNED transitions:
//!
//!   * O1 — per-account source-domain realizability (`check_observation`,
//!     `src/oracles.rs`), a single-state oracle, on EVERY observation; and
//!   * the v0.1 market-engine realizability CROSS-LINK (`check_observation_market`),
//!     a single-state oracle, on EVERY observation; and
//!   * the v0.2 liquidation insurance-domain ISOLATION oracle
//!     (`liquidation_insurance_isolation`), a CROSS-STEP delta oracle, on every
//!     consecutive `(prev, cur)` pair (a no-op on the earned campaign's
//!     non-liquidation steps); and
//!   * the v0.3 quantity-ADL EXACT-ACCOUNTING oracle (`adl_accounting`), a
//!     CROSS-STEP delta oracle, on every consecutive `(prev, cur)` pair (a no-op
//!     on the earned campaign's non-ADL steps).
//!
//! The earned campaign is the v0.4 thesis in action: the lien precondition (a
//! source-attributed positive PnL with a per-account claim bound) is REACHED by
//! engine logic — a real matched trade opens the leg, funding settlement raises the
//! PnL and the claim, a provider posts the public backing — rather than seeded by
//! direct field writes. The O1 and cross-link oracles therefore evaluate a lien
//! the engine actually DREW on an earned transition.
//!
//! If any oracle fires, the offending scenario is persisted to
//! `scenarios/earned_lien_candidate.json` and the test fails loudly. A candidate
//! is NOT automatically a bug — it may be a real finding OR a harness/observation
//! artifact — so it is captured for adversarial verification. NEVER weaken an
//! oracle to make this pass.

use perp_adversary::driver::{earned_lien_campaign, run, DomainObs};
use perp_adversary::oracles::{
    adl_accounting, check_observation, check_observation_market, liquidation_insurance_isolation,
};
use perp_adversary::runner::{first_violation, first_violation_delta};
use perp_adversary::scenario::Scenario;

/// Run all four oracles over `s`; on the first breach, persist the scenario and
/// return a loud message with the exact step/oracle/detail. Returns `Ok(())` if
/// the engine held all four.
fn run_all_oracles(label: &str, s: &Scenario) -> Result<(), String> {
    let trace = run(s);

    // Single-state oracles on every observation (the earned lien is policed here).
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

    // Cross-step delta oracles on every consecutive (prev, cur) pair. They are
    // no-ops on the earned campaign's steps (no liquidation, no ADL), but running
    // them keeps the four-oracle coverage explicit and would fire if a future
    // earned campaign reached those transitions.
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
    let path = "scenarios/earned_lien_candidate.json";
    let _ = std::fs::write(path, serde_json::to_string_pretty(s).unwrap());
    format!(
        "EARNED-LIEN CANDIDATE [{label}] at step {step}: {oracle} :: {detail} :: \
         saved {path} :: scenario={}",
        serde_json::to_string(s).unwrap()
    )
}

#[test]
fn engine_holds_all_four_oracles_on_earned_lien_campaign() {
    let s = earned_lien_campaign();
    if let Err(msg) = run_all_oracles("earned", &s) {
        panic!("{msg}");
    }
}

/// Non-vacuity guard for the four-oracle gate: the earned campaign must actually
/// reach a populated, engine-drawn lien, else the conformance result above is
/// vacuous (the oracles would clear an all-zero default). This asserts the O1 /
/// cross-link oracles above evaluated a non-trivial EARNED domain.
#[test]
fn earned_four_oracle_gate_is_non_vacuous() {
    let s = earned_lien_campaign();
    let trace = run(&s);

    let liened = trace.observations.iter().any(|obs| {
        obs.accounts
            .iter()
            .flat_map(|a| a.domains.iter())
            .any(|d: &DomainObs| {
                d.source_claim_liened_num != 0 && d.source_lien_effective_reserved != 0
            })
    });
    assert!(
        liened,
        "earned campaign produced no populated source-credit lien — the four-oracle \
         gate would be vacuous. The earned funding/lien path must reach a non-zero \
         source_claim_liened_num AND source_lien_effective_reserved."
    );
}

/// Fold the single-state O1 oracle over the earned campaign via the runner's
/// `first_violation`, and the two CROSS-STEP delta oracles via
/// `first_violation_delta` — exactly the wiring the plan asks for. Each must report
/// `None` (no violating step) on the earned transitions.
#[test]
fn earned_oracles_hold_via_runner() {
    let s = earned_lien_campaign();

    // O1 realizability, single-state, folded by `first_violation`.
    let o1 = first_violation(&s, |obs| check_observation(obs).map_err(|e| e.detail));
    assert!(
        o1.is_none(),
        "engine must hold O1 realizability across the earned campaign; got {o1:?}"
    );

    // Market cross-link, single-state, folded by `first_violation`.
    let xlink = first_violation(&s, |obs| {
        check_observation_market(obs).map_err(|e| e.detail)
    });
    assert!(
        xlink.is_none(),
        "engine must hold the market cross-link across the earned campaign; got {xlink:?}"
    );

    // v0.2 insurance isolation, cross-step, folded by `first_violation_delta`.
    let iso = first_violation_delta(&s, |prev, cur| {
        liquidation_insurance_isolation(prev, cur).map_err(|e| e.detail)
    });
    assert!(
        iso.is_none(),
        "engine must hold insurance isolation across the earned campaign; got {iso:?}"
    );

    // v0.3 ADL exact-accounting, cross-step, folded by `first_violation_delta`.
    let adl = first_violation_delta(&s, |prev, cur| {
        adl_accounting(prev, cur).map_err(|e| e.detail)
    });
    assert!(
        adl.is_none(),
        "engine must hold ADL exact-accounting across the earned campaign; got {adl:?}"
    );
}
