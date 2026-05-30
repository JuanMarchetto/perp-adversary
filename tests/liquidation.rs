//! Liquidation attack-class non-vacuity gate (v0.2-A).
//!
//! The v0.2 liquidation work only *means* something if the driver actually
//! reaches an engine-accepted underwater state and fires a REAL liquidation —
//! one that closes a non-zero quantity and books residual loss. The prior
//! `Action::Liquidate` used `close_q: 0` against an account with no underwater
//! position: a no-op that exercises none of the liquidation path.
//!
//! This test runs the deliberately liquidation-driving campaign and asserts the
//! driver OBSERVES a `LiquidationOutcomeV16` where `residual_booked > 0` (a real
//! liquidation fired — the engine booked unbacked loss as residual rather than
//! draining shared insurance). It mirrors the engine's own conformance test
//! `v16_public_liquidation_on_unfunded_domain_cannot_drain_shared_insurance`,
//! whose central assertion is `out.residual_booked > 0`.
//!
//! If this ever fails, the liquidation attack class has gone vacuous and must be
//! treated as a loud regression — NEVER weaken these assertions to make it pass.

use perp_adversary::driver::{liquidation_campaign, run};

#[test]
fn campaign_fires_a_real_liquidation_that_books_residual() {
    let s = liquidation_campaign();
    let trace = run(&s);

    // Find the observation that captured a liquidation outcome.
    let found = trace
        .observations
        .iter()
        .find_map(|obs| obs.liquidation.map(|l| (obs.step, obs.result.clone(), l)));

    let (step, result, liq) = found.unwrap_or_else(|| {
        let mut trail = String::new();
        for obs in &trace.observations {
            trail.push_str(&format!(
                "  step {}: {:?} -> {:?}\n",
                obs.step, obs.action, obs.result
            ));
        }
        panic!(
            "VACUITY: no observation captured a liquidation outcome. The driver \
             never drove a real liquidation, so the liquidation attack class is \
             not exercised. Campaign step trail:\n{trail}"
        );
    });

    // The liquidation step itself must have been accepted by the engine.
    assert!(
        result.is_ok(),
        "liquidation step {step} should progress, got {result:?}; observed outcome {liq:?}"
    );

    // Non-vacuity: a REAL liquidation fired — it closed a non-zero quantity and
    // booked residual loss. `residual_booked > 0` is exactly the engine's own
    // conformance assertion for this state.
    assert!(
        liq.closed_q > 0,
        "liquidation at step {step} must close a non-zero quantity: {liq:?}"
    );
    assert!(
        liq.residual_booked > 0,
        "liquidation at step {step} must book residual loss (non-vacuity): {liq:?}"
    );
    // It must NOT have drained shared insurance — the unfunded-domain invariant.
    assert_eq!(
        liq.insurance_used, 0,
        "liquidation at step {step} must not drain shared insurance: {liq:?}"
    );
}
