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
use perp_adversary::oracles::check_observation;

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
