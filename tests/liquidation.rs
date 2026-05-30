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

use perp_adversary::driver::{funded_liquidation_campaign, liquidation_campaign, run};

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

/// Anti-vacuity for the v0.2-B insurance-ISOLATION oracle: the isolation oracle is
/// strongest when some insurance IS actually spent for the liquidated domain (and
/// NONE for any other). This gate proves the FUNDED campaign reaches an
/// engine-accepted state where `insurance_used > 0` AND exactly the liquidated
/// asset's short-side (the long leg's bankruptcy domain) `insurance_domain_spent`
/// rises by that amount, while every other domain stays put. It mirrors the
/// engine's own `proof_v16_view_domain_budget_caps_bankruptcy_insurance_spend`
/// (`tests/proofs_v16.rs:2375`) reached through the public liquidation entrypoint.
///
/// If this ever fails, the isolation oracle has lost its stronger (insurance-IS-
/// spent) input and falls back to the weak no-spend path — treat it as a loud
/// regression; NEVER weaken these assertions to make it pass.
#[test]
fn funded_campaign_spends_insurance_only_on_the_liquidated_domain() {
    let s = funded_liquidation_campaign();
    let trace = run(&s);

    // Locate the pre- and post-liquidation observations.
    let liq_step = trace
        .observations
        .iter()
        .position(|o| o.liquidation.is_some())
        .expect("funded campaign must fire a liquidation");
    assert!(
        liq_step >= 1,
        "liquidation must have a predecessor observation to delta against"
    );
    let prev = &trace.observations[liq_step - 1];
    let cur = &trace.observations[liq_step];
    let liq = cur.liquidation.unwrap();

    assert!(
        cur.result.is_ok(),
        "funded liquidation step should progress, got {:?}; outcome {liq:?}",
        cur.result
    );

    // Non-vacuity: insurance WAS genuinely spent.
    assert!(
        liq.insurance_used > 0,
        "funded liquidation must spend insurance (insurance_used > 0): {liq:?}"
    );

    // The spend is concentrated on asset 0's SHORT side (the long leg's
    // bankruptcy domain = opposite_side(Long)); every other domain is unchanged.
    let spent = |obs: &perp_adversary::driver::Observation, asset: usize, side: u8| -> u128 {
        obs.market_domains
            .iter()
            .find(|m| m.asset == asset && m.side == side)
            .map(|m| m.insurance_domain_spent)
            .unwrap_or_else(|| panic!("missing market side ({asset}, {side})"))
    };
    let short_delta = spent(cur, 0, 1) - spent(prev, 0, 1);
    let long_delta = spent(cur, 0, 0) - spent(prev, 0, 0);
    assert_eq!(
        short_delta, liq.insurance_used,
        "liquidated domain (asset 0, short) spend must rise by exactly insurance_used: {liq:?}"
    );
    assert_eq!(
        long_delta, 0,
        "the non-bankruptcy side of the liquidated asset must not be charged: {liq:?}"
    );
}
