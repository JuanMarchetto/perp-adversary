//! v0.7 — the PnL→capital REALIZATION firewall, now exercised end-to-end.
//!
//! `convert_released_pnl_to_capital_not_atomic` (`v16.rs:10821`) is the only path
//! that turns positive PnL (claim accounting) into withdrawable `capital`. It was
//! the engine's unwired surface — the gate behind which every "winner overdraws
//! realizable PnL" extraction theory dead-ended (the extractive hunt could not
//! reach it, so it could only *argue* the backing firewall was safe). v0.7 wires it
//! (`Action::ConvertReleasedPnl` + `Action::ReleaseLiens`) and drives it end-to-end:
//! a long earns funding PnL, flattens, releases liens, re-certifies, converts the
//! released PnL into capital, and withdraws part of it.
//!
//! The security property under test: convert does `capital += converted`,
//! `pnl -= face_burn` with the vault flat, so a MINT (`converted > face_burn`) would
//! raise claimable value with no external flow — the EXTRACTIVE direction the v0.6
//! oracle flags as `ClaimableValueCreated`. These tests confirm the firewall HOLDS
//! (convert conserves claimable value exactly) and that the realized withdrawal is
//! fully backed (global vault conservation holds on every step). Turns "argued
//! safe" into "tested against the mint detector".
use perp_adversary::driver::{convert_realization_campaign, run};
use perp_adversary::oracles::{
    check_step_value_conservation, claimable_value, funding_conservation_kind,
    FundingConservationKind,
};
use perp_adversary::scenario::Action;

/// Index of the (single) `ConvertReleasedPnl` observation in the trace.
fn convert_step(obs: &[perp_adversary::driver::Observation]) -> usize {
    obs.iter()
        .position(|o| matches!(o.action, Action::ConvertReleasedPnl { .. }))
        .expect("campaign must contain a ConvertReleasedPnl step")
}

#[test]
fn convert_realization_firewall_is_exercised_and_conserves() {
    let trace = run(&convert_realization_campaign());
    let obs = &trace.observations;
    let ci = convert_step(obs);
    let (prev, cur) = (&obs[ci - 1], &obs[ci]);

    // NON-VACUOUS: the convert must SUCCEED and actually realize PnL — capital rises
    // by the converted amount and PnL falls. Otherwise the firewall was never run.
    assert!(cur.result.is_ok(), "convert must succeed (firewall reached): {:?}", cur.result);
    let converted = cur.accounts[0].capital as i128 - prev.accounts[0].capital as i128;
    let pnl_burned = prev.accounts[0].pnl - cur.accounts[0].pnl;
    assert!(converted > 0, "convert must realize positive capital (converted={converted})");
    assert!(pnl_burned > 0, "convert must burn positive PnL (pnl_burned={pnl_burned})");

    // THE FIREWALL: convert must NOT MINT. Claimable value (Σ capital + Σ pnl) must
    // not rise across the convert step (no external flow): converted <= face_burn.
    // A mint would be `ClaimableValueCreated` (the extractive direction).
    let verdict = funding_conservation_kind(claimable_value(prev), claimable_value(cur), 0, 0);
    assert_ne!(
        verdict,
        Err(FundingConservationKind::ClaimableValueCreated),
        "convert MINTED claimable value (converted={converted} > face_burn={pnl_burned}) — extractive!"
    );
    // In the pinned engine it conserves EXACTLY (converted == face_burn): firewall held.
    assert_eq!(
        converted, pnl_burned,
        "pinned-engine convert conserves claimable value exactly (converted == face_burn)"
    );
    assert!(verdict.is_ok(), "claimable conserved across convert: {verdict:?}");
}

#[test]
fn realized_withdrawal_is_fully_backed_value_conserves_every_step() {
    // The whole earn -> flatten -> convert -> withdraw chain must conserve global
    // quote value: vault changes ONLY by net external flow on EVERY step (v0.5).
    // This proves the winner's realized withdrawal is backed by real value
    // transferred from the counterparty, not minted.
    let trace = run(&convert_realization_campaign());
    for w in trace.observations.windows(2) {
        let (prev, cur) = (&w[0], &w[1]);
        assert!(
            check_step_value_conservation(prev, cur).is_ok(),
            "vault conservation must hold at step {} ({:?}): {:?}",
            cur.step,
            cur.action,
            check_step_value_conservation(prev, cur)
        );
    }
    // And a real withdrawal of realized capital actually occurred (non-vacuous).
    let withdrew = trace
        .observations
        .iter()
        .any(|o| matches!(o.action, Action::Withdraw { .. }) && o.ext_out_step > 0 && o.result.is_ok());
    assert!(withdrew, "campaign must end with a real withdrawal of realized capital");
}
