//! Quantity-ADL attack-class non-vacuity gate (v0.3-A).
//!
//! The v0.3 ADL work only *means* something if the driver actually reaches an
//! engine-accepted, finalized-close, ADL-eligible state and fires a REAL
//! quantity auto-deleverage — one that closes a non-zero quantity on the
//! profitable counterparty's leg and records a `QuantityAdlOutcomeV16`.
//!
//! This test runs the deliberately ADL-driving campaign and asserts the driver
//! OBSERVES a `QuantityAdlOutcomeV16` where `closed_q > 0` (a real ADL fired)
//! AND the account's `close_progress.quantity_adl_applied_q` advanced by exactly
//! that amount. It mirrors the engine's own ADL entrypoint
//! `apply_quantity_adl_after_residual_for_account_not_atomic` (v16.rs:9479): the
//! profitable counterparty (the side whose `domain_side == opposite_side(
//! bankrupt_side)`) is deleveraged after a bankrupt account's residual close has
//! finalized.
//!
//! If this ever fails, the ADL attack class has gone vacuous and must be treated
//! as a loud regression — NEVER weaken these assertions to make it pass.

use perp_adversary::driver::{adl_campaign, run};

#[test]
fn campaign_fires_a_real_quantity_adl() {
    let s = adl_campaign();
    let trace = run(&s);

    // Find the observation that captured an ADL outcome.
    let found = trace
        .observations
        .iter()
        .find_map(|obs| obs.adl.map(|a| (obs.step, obs.result.clone(), a)));

    let (step, result, adl) = found.unwrap_or_else(|| {
        let mut trail = String::new();
        for obs in &trace.observations {
            trail.push_str(&format!(
                "  step {}: {:?} -> {:?}\n",
                obs.step, obs.action, obs.result
            ));
        }
        panic!(
            "VACUITY: no observation captured an ADL outcome. The driver never \
             drove a real quantity-ADL, so the ADL attack class is not exercised. \
             Campaign step trail:\n{trail}"
        );
    });

    // The ADL step itself must have been accepted by the engine.
    assert!(
        result.is_ok(),
        "ADL step {step} should progress, got {result:?}; observed outcome {adl:?}"
    );

    // Non-vacuity: a REAL ADL fired — it closed a non-zero quantity on the
    // profitable counterparty's leg.
    assert!(
        adl.closed_q > 0,
        "ADL at step {step} must close a non-zero quantity (non-vacuity): {adl:?}"
    );

    // The engine recorded the applied quantity on the account's close-progress
    // ledger; it must equal the outcome's `closed_q`.
    assert_eq!(
        adl.quantity_adl_applied_q, adl.closed_q,
        "close_progress.quantity_adl_applied_q must advance by exactly closed_q: {adl:?}"
    );
}
