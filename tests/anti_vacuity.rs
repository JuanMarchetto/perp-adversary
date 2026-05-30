//! Anti-vacuity gate.
//!
//! The O1 realizability oracle (`src/oracles.rs`) only *means* something if the
//! harness actually drives the engine into a state where the source-credit-lien
//! fields it polices are NON-ZERO. If every observed `DomainObs` is
//! `DomainObs::default()` (all zeros), the six realizability relationships hold
//! trivially and a "conformance" result is vacuous.
//!
//! This test runs the deliberately lien-creating campaign and asserts that the
//! engine actually drew a source-credit LIEN (non-zero `source_claim_liened_num`
//! AND `source_lien_effective_reserved`) — not merely a seeded claim bound.
//! That is the precise field the realizability cap (check 3) and the reservation
//! exactness (check 5) bite on. It then runs the real O1 oracle over the
//! populated observation, proving the oracle evaluates a non-trivial domain.
//!
//! If this ever fails, the conformance result has gone vacuous again and must be
//! treated as a loud regression — NEVER weaken these assertions to make it pass.

use perp_adversary::driver::{lien_creating_campaign, run, DomainObs};
use perp_adversary::oracles::{check_observation, check_observation_market, market_cross_link};

/// The lien-creating campaign seeds the MARKET-ENGINE source-credit state on
/// asset 0's long side (domain `asset*2`) via `SeedSourceClaim`, which calls the
/// engine's `add_source_positive_claim_bound_not_atomic` +
/// `add_fresh_counterparty_backing_not_atomic`. That state is a SEPARATE thing
/// from the per-account source domain (`DomainObs`): each market asset carries
/// `source_credit_long`/`source_credit_short` (`SourceCreditStateV16`) on its
/// engine slot. The v0.1 market-engine oracle reads it; this test pins the
/// prerequisite — that the driver actually OBSERVES that non-zero market-engine
/// state, so the future oracle is not vacuous.
#[test]
fn driver_observes_nonzero_market_engine_source_credit_state() {
    let s = lien_creating_campaign();
    let trace = run(&s);

    let found = trace.observations.iter().find_map(|obs| {
        obs.market_domains
            .iter()
            .find(|m| m.state.positive_claim_bound_num != 0)
            .map(|m| (obs.step, *m))
    });

    let (step, m) = found.unwrap_or_else(|| {
        panic!(
            "driver must observe a non-zero market-engine source-credit state; the \
             lien-creating campaign seeds asset 0 long via SeedSourceClaim, so some \
             observation's market_domains must carry a non-zero \
             positive_claim_bound_num"
        );
    });

    // Seeded on asset 0's LONG side (domain asset*2 == 0 -> side 0).
    assert_eq!(m.asset, 0, "seed was on asset 0 (step {step})");
    assert_eq!(m.side, 0, "seed was on the long side (step {step})");
    assert!(
        m.state.positive_claim_bound_num != 0,
        "market-engine positive_claim_bound_num must be non-zero at step {step}: {:?}",
        m.state
    );
}

/// The first observation whose state carries a real, engine-drawn lien
/// (`source_claim_liened_num != 0`), with its location.
fn first_liened_domain(
    trace: &perp_adversary::driver::Trace,
) -> Option<(usize, usize, usize, DomainObs)> {
    for obs in &trace.observations {
        for (ai, acct) in obs.accounts.iter().enumerate() {
            for (di, dom) in acct.domains.iter().enumerate() {
                if dom.source_claim_liened_num != 0 {
                    return Some((obs.step, ai, di, *dom));
                }
            }
        }
    }
    None
}

#[test]
fn campaign_produces_a_nonzero_source_credit_lien() {
    let s = lien_creating_campaign();
    let trace = run(&s);

    let (step, ai, di, dom) = first_liened_domain(&trace).unwrap_or_else(|| {
        let mut trail = String::new();
        for obs in &trace.observations {
            trail.push_str(&format!(
                "  step {}: {:?} -> {:?}\n",
                obs.step, obs.action, obs.result
            ));
        }
        panic!(
            "VACUITY: no observation produced a non-zero source-credit LIEN. The \
             engine never drew an initial-margin source-credit lien, so the O1 \
             oracle never evaluates the realizability machinery and the \
             conformance result is vacuous. Campaign step trail:\n{trail}"
        );
    });

    // The lien must carry both a locked face claim and reserved effective
    // backing — i.e. the realizability cap (check 3) and reservation exactness
    // (check 5) are over genuinely non-zero quantities.
    assert!(
        dom.source_claim_liened_num != 0,
        "lien face claim must be non-zero at step {step}, account {ai}, domain {di}: {dom:?}"
    );
    assert!(
        dom.source_lien_effective_reserved != 0,
        "lien effective reserved must be non-zero at step {step}, account {ai}, domain {di}: {dom:?}"
    );

    // And the O1 oracle must actually run over this populated domain. Whether it
    // CLEARS (engine holds) or FLAGS (candidate) is reported by the JELLY and
    // proptest tests; here we only require that the oracle evaluates a
    // non-trivial domain rather than an all-zero default.
    let populated_obs = &trace.observations[step];
    let oracle_result = check_observation(populated_obs);
    // The engine is expected to HOLD realizability on its own drawn lien.
    assert!(
        oracle_result.is_ok(),
        "O1 oracle flagged the engine's own freshly-drawn lien — this is a \
         CANDIDATE violation, save and report it: {oracle_result:?}"
    );
}

// ===========================================================================
// v0.1 market-engine CROSS-LINK anti-vacuity gate
// ===========================================================================

/// The v0.1 market cross-link oracle (`oracles::market_cross_link`) only *means*
/// something if the harness actually pairs a per-account source domain whose
/// `source_claim_bound_num` is NON-ZERO against a market-engine side whose
/// `positive_claim_bound_num` is also NON-ZERO. If the per-account bound or the
/// market bound were always zero, the inequality `bound <= positive_bound` would
/// hold trivially (`0 <= x`) and the cross-link would be vacuous.
///
/// This test runs the lien-creating campaign and asserts there is an observation
/// in which BOTH bounds are non-zero for a matching (asset, side) pairing — the
/// genuinely non-trivial state the cross-link is meant to police — and that the
/// cross-link oracle evaluates (and, since the engine produced the state, CLEARS)
/// it. NEVER weaken these assertions to make it pass.
#[test]
fn cross_link_evaluates_a_nonzero_over_nonzero_pairing() {
    let s = lien_creating_campaign();
    let trace = run(&s);

    // Find an observation with a per-account domain (asset/side = di/2, di%2) whose
    // source_claim_bound_num != 0 AND whose paired market side carries a non-zero
    // positive_claim_bound_num.
    let mut found: Option<(usize, usize, usize, u128, u128)> = None;
    'outer: for obs in &trace.observations {
        for (ai, acct) in obs.accounts.iter().enumerate() {
            for (di, dom) in acct.domains.iter().enumerate() {
                if dom.source_claim_bound_num == 0 {
                    continue;
                }
                let asset = di / 2;
                let side = (di % 2) as u8;
                if let Some(m) = obs
                    .market_domains
                    .iter()
                    .find(|m| m.asset == asset && m.side == side)
                {
                    if m.state.positive_claim_bound_num != 0 {
                        found = Some((
                            obs.step,
                            ai,
                            di,
                            dom.source_claim_bound_num,
                            m.state.positive_claim_bound_num,
                        ));
                        break 'outer;
                    }
                }
            }
        }
    }

    let (step, ai, di, acct_bound, mkt_bound) = found.unwrap_or_else(|| {
        panic!(
            "VACUITY: no observation paired a NON-ZERO per-account \
             source_claim_bound_num against a NON-ZERO market \
             positive_claim_bound_num, so the v0.1 cross-link (bound <= \
             positive_bound) is only ever checked as 0 <= x — vacuous. The \
             lien-creating campaign must populate both."
        );
    });

    assert!(
        acct_bound != 0 && mkt_bound != 0,
        "both bounds must be non-zero at step {step}, account {ai}, domain {di}: \
         acct_bound={acct_bound}, mkt_bound={mkt_bound}"
    );

    // The cross-link oracle must actually run over this populated pairing, and is
    // expected to HOLD (the engine produced and validated this state). A flag here
    // would be a CANDIDATE violation to save and report — do NOT weaken.
    let populated_obs = &trace.observations[step];
    let acct_result = market_cross_link(
        &populated_obs.accounts[ai].domains,
        &populated_obs.market_domains,
    );
    assert!(
        acct_result.is_ok(),
        "cross-link oracle flagged the engine's own validated state — CANDIDATE \
         violation, save and report it: {acct_result:?}"
    );
    assert!(
        check_observation_market(populated_obs).is_ok(),
        "cross-link oracle flagged the populated observation — CANDIDATE \
         violation, save and report it"
    );
}
