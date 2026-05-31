//! v0.8 — the liened-effective vs unliened-consume SEAM, now executed (holds).
//!
//! The extractive hunt's residual doubt: converting released PnL while a source-credit
//! lien is still RESERVED. `account_source_realizable_support` (`v16.rs:5728`) counts
//! liened-effective support 1:1, but the realization consume
//! (`create_and_consume_account_source_credit_for_effective_not_atomic`, `v16.rs:6204`)
//! walks only `source_claim_unliened_num`. If converting could mint capital the
//! consume can't back, a winner could realize more than entitlement (extractive). The
//! hunt argued it fails closed (`LockActive`, `v16.rs:6249`) but never executed it —
//! the harness couldn't convert. v0.7 wired convert; v0.8 drives the seam.
//!
//! Result: converting a FLAT account that still carries a reserved lien fails closed
//! (`LockActive`) with NO mint; releasing the lien first lets the convert succeed and
//! conserve claimable value EXACTLY. The seam holds — tested, not argued.
use perp_adversary::driver::{liened_seam_campaign, run, Observation};
use perp_adversary::oracles::claimable_value;
use perp_adversary::scenario::Action;

fn lien_effective(o: &Observation) -> u128 {
    o.accounts
        .iter()
        .flat_map(|a| a.domains.iter())
        .map(|d| d.source_lien_effective_reserved)
        .sum()
}

fn convert_window(obs: &[Observation]) -> (&Observation, &Observation) {
    let ci = obs
        .iter()
        .position(|o| matches!(o.action, Action::ConvertReleasedPnl { .. }))
        .expect("campaign must contain a ConvertReleasedPnl step");
    (&obs[ci - 1], &obs[ci])
}

#[test]
fn convert_with_reserved_lien_fails_closed_no_mint() {
    let trace = run(&liened_seam_campaign(false)); // do NOT release the lien
    let (before, after) = convert_window(&trace.observations);

    // Non-vacuous: a real lien must be reserved on the flat account at convert time
    // (otherwise we are not testing the seam).
    assert!(
        lien_effective(before) > 0,
        "the account must carry a reserved lien when convert is attempted (got {})",
        lien_effective(before)
    );
    // The seam fails CLOSED: converting while a lien is reserved is rejected...
    assert!(
        after.result.is_err(),
        "convert with a reserved lien must fail closed (LockActive), got {:?}",
        after.result
    );
    // ...and crucially mints NOTHING — claimable value is unchanged across the step.
    assert_eq!(
        claimable_value(after),
        claimable_value(before),
        "a rejected convert must not change claimable value (no partial mint)"
    );
}

#[test]
fn releasing_lien_then_convert_succeeds_and_conserves() {
    // Control: the ONLY difference is releasing the lien first. The convert then
    // succeeds (proving the account was flat enough to reach the realizable path and
    // the reserved lien was the blocker) and conserves claimable value exactly — it
    // never mints. So the firewall holds in BOTH the blocked and the allowed case.
    let trace = run(&liened_seam_campaign(true)); // release the lien first
    let (before, after) = convert_window(&trace.observations);

    assert_eq!(lien_effective(before), 0, "lien must be released before convert");
    assert!(after.result.is_ok(), "convert after release must succeed: {:?}", after.result);
    let converted = after.accounts[0].capital as i128 - before.accounts[0].capital as i128;
    assert!(converted > 0, "convert must realize positive capital (converted={converted})");
    assert_eq!(
        claimable_value(after),
        claimable_value(before),
        "convert must conserve claimable value exactly (no mint): converted == face_burn"
    );
}
