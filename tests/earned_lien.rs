//! Earned-state non-vacuity gate (v0.4).
//!
//! The v0.4 thesis: drive the engine through REAL operation sequences so the
//! source-credit-lien precondition is REACHED by engine logic, not seeded by
//! direct field writes. Where the legacy `lien_creating_campaign` (via
//! `SeedSourceClaim`) writes `account.pnl`, the per-account
//! `source_claim_bound_num`, and the group pnl_pos totals DIRECTLY, the
//! `earned_lien_campaign` instead:
//!
//!   (a) has a backing provider POST claim+backing through the PUBLIC entrypoints
//!       (`add_source_positive_claim_bound_not_atomic` /
//!       `add_fresh_counterparty_backing_not_atomic`) — the one genuinely
//!       privileged-but-public operation the plan keeps;
//!   (b) opens a REAL matched position via `execute_trade` (no hand-built leg);
//!   (c) accrues POSITIVE FUNDING (effective == target, so NO target/effective
//!       lag) and refreshes, so the engine SETTLES a source-attributed positive
//!       PnL into the long's account via `apply_signed_kf_delta_to_pnl` ->
//!       `set_account_pnl_with_source` — raising `account.pnl` AND the per-account
//!       `source_claim_bound_num` THROUGH ENGINE LOGIC; then
//!   (d) a risk-increasing add draws the initial-margin source-credit lien.
//!
//! This test pins the NON-VACUITY: the lien fields the O1 realizability oracle
//! polices (`source_claim_liened_num`, `source_lien_effective_reserved`) become
//! non-zero, and — critically — the positive PnL and the claim bound that back the
//! lien were REACHED WITHOUT the direct `account.pnl` / `source_claim_bound_num`
//! writes the legacy `SeedSourceClaim` used.
//!
//! If this ever fails, the earned path has gone vacuous (no real lien earned) and
//! must be treated as a loud regression — NEVER weaken these assertions to pass.

use perp_adversary::driver::{earned_lien_campaign, run, AccountObs, DomainObs};
use perp_adversary::oracles::{check_observation, check_observation_market, market_cross_link};

/// The first observation/account/domain whose state carries a real engine-drawn
/// lien (`source_claim_liened_num != 0`), with its location.
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
fn earned_campaign_reaches_positive_attributed_pnl_through_funding() {
    let s = earned_lien_campaign();
    let trace = run(&s);

    // Every step the engine accepted must be Ok (the campaign is conformant).
    for obs in &trace.observations {
        assert!(
            obs.result.is_ok(),
            "earned campaign step {} ({:?}) failed: {:?}",
            obs.step,
            obs.action,
            obs.result
        );
    }

    // The long (account 0) must reach STRICTLY POSITIVE PnL through funding — the
    // engine settled a source-attributed favorable K/F delta into its account. A
    // campaign that opened no position, or whose funding never settled, leaves
    // pnl == 0; this is the earned analogue of the seeded `account.pnl` write.
    let long_pnl_peak = trace
        .observations
        .iter()
        .map(|o| o.accounts[0].pnl)
        .max()
        .unwrap_or(0);
    assert!(
        long_pnl_peak > 0,
        "long account must EARN positive PnL via funding (peaked at {long_pnl_peak}); \
         the favorable funding K/F delta must settle into account.pnl through \
         apply_signed_kf_delta_to_pnl, not a direct write"
    );

    // And that positive PnL must be ATTRIBUTED to a source domain: the per-account
    // `source_claim_bound_num` on some domain must be non-zero, set by the engine's
    // `set_account_pnl_with_source` -> `ensure_account_source_claim_market_id`
    // path. With the seed removed, the ONLY way this becomes non-zero is the
    // earned funding attribution.
    let earned_claim = trace.observations.iter().any(|o| {
        o.accounts[0]
            .domains
            .iter()
            .any(|d| d.source_claim_bound_num != 0)
    });
    assert!(
        earned_claim,
        "long account must EARN a source-attributed claim bound (non-zero \
         per-account source_claim_bound_num) through funding attribution, not a \
         direct seed"
    );
}

#[test]
fn earned_campaign_draws_a_nonzero_source_credit_lien() {
    let s = earned_lien_campaign();
    let trace = run(&s);

    let (step, ai, di, dom) = first_liened_domain(&trace).unwrap_or_else(|| {
        let mut trail = String::new();
        for obs in &trace.observations {
            trail.push_str(&format!(
                "  step {}: {:?} -> {:?} | acct0 pnl={} domains={:?}\n",
                obs.step,
                obs.action,
                obs.result,
                obs.accounts
                    .first()
                    .map(|a: &AccountObs| a.pnl)
                    .unwrap_or(0),
                obs.accounts
                    .first()
                    .map(|a| a
                        .domains
                        .iter()
                        .map(|d| (d.source_claim_bound_num, d.source_claim_liened_num))
                        .collect::<Vec<_>>())
                    .unwrap_or_default(),
            ));
        }
        panic!(
            "EARNED-VACUITY: the earned campaign drew no source-credit LIEN. The \
             engine never drew an initial-margin source-credit lien on the EARNED \
             positive PnL, so the v0.4 earned-state thesis is unproven. Step \
             trail:\n{trail}"
        );
    });

    // A real lien: both a locked face claim and reserved effective backing.
    assert!(
        dom.source_claim_liened_num != 0,
        "earned lien face claim must be non-zero at step {step}, account {ai}, domain {di}: {dom:?}"
    );
    assert!(
        dom.source_lien_effective_reserved != 0,
        "earned lien effective reserved must be non-zero at step {step}, account {ai}, domain {di}: {dom:?}"
    );

    // The lien must sit on a domain whose per-account claim bound was EARNED
    // (non-zero), i.e. the lien is drawn against funding-attributed positive PnL,
    // not a directly-seeded claim.
    assert!(
        dom.source_claim_bound_num >= dom.source_claim_liened_num,
        "the liened face claim ({}) must be within the earned claim bound ({}) at \
         step {step}, account {ai}, domain {di}",
        dom.source_claim_liened_num,
        dom.source_claim_bound_num
    );

    // The O1 oracle and the market cross-link must both EVALUATE this populated,
    // earned domain and HOLD (the engine produced and validated it). A flag here is
    // a CANDIDATE — save and report, never weaken.
    let obs = &trace.observations[step];
    assert!(
        check_observation(obs).is_ok(),
        "O1 oracle flagged the engine's own EARNED lien — CANDIDATE violation, save \
         and report: {:?}",
        check_observation(obs)
    );
    assert!(
        market_cross_link(&obs.accounts[ai].domains, &obs.market_domains).is_ok(),
        "cross-link oracle flagged the engine's own EARNED state — CANDIDATE \
         violation, save and report"
    );
    assert!(
        check_observation_market(obs).is_ok(),
        "market cross-link flagged the earned observation — CANDIDATE violation, \
         save and report"
    );
}
