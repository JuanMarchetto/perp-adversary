//! v0.5 GLOBAL QUOTE-VALUE CONSERVATION oracle unit tests (TDD — written before
//! the implementation).
//!
//! Unlike every other oracle test in this crate, these fixtures police an
//! EMERGENT property the engine does NOT enforce as a single global validator:
//! total real quote-atom value (`system.vault`, the engine's
//! `StockReconciliationProofV16.token_vault`, `v16.rs:3020`) is CONSERVED across a
//! step, changing only by the net external flow that crossed the instance
//! boundary. The engine proves a per-instruction version
//! (`TokenValueFlowProofV16::validate`, `v16.rs:2913`: `Δvault == external_in −
//! external_out`); this oracle composes it across a campaign at the harness level
//! from independently observed `vault` snapshots.
//!
//! The value model is `system_quote_value(state) == vault` — the single
//! source-of-truth total real quote-atom balance (spec.md §5.1.1, line 1043). PnL,
//! claims, liens, backing, reservations, and `payout_snapshot` are NOT value
//! classes (spec.md:46/961) and are excluded.
//!
//! These are PURE-DATA fixtures: they build `Observation`s with hand-set `vault`
//! (and the partition fields) and feed explicit external-flow deltas, exercising
//! the oracle's arithmetic directly with no engine in the loop.

use perp_adversary::driver::{Observation, SystemObs};
use perp_adversary::oracles::{value_conservation, value_conservation_kind, ConservationKind};
use perp_adversary::scenario::Action;

/// An observation carrying only the quote-atom stores the oracle reads. The
/// partition fields default to a consistent `c_tot + insurance + backing <= vault`
/// (all zero) unless overridden.
fn obs(vault: u128) -> Observation {
    Observation {
        step: 0,
        action: Action::Deposit {
            account: 0,
            amount: 0,
        },
        result: Ok(()),
        accounts: vec![],
        market_domains: vec![],
        liquidation: None,
        adl: None,
        system: SystemObs {
            vault,
            ..SystemObs::default()
        },
        ext_in_step: 0,
        ext_out_step: 0,
    }
}

/// An observation with an explicit partition (c_tot + insurance + backing) under a
/// given vault, to exercise the partition cross-check.
fn obs_part(vault: u128, c_tot: u128, insurance: u128, backing: u128) -> Observation {
    Observation {
        system: SystemObs {
            vault,
            c_tot,
            insurance,
            backing_provider_earnings: backing,
            ..SystemObs::default()
        },
        ..obs(vault)
    }
}

// ---------------------------------------------------------------------------
// OK cases
// ---------------------------------------------------------------------------

#[test]
fn internal_step_with_no_external_flow_conserves() {
    // An internal step (trade / liquidation / ADL): vault unchanged, no external
    // flow. This is the central guarantee — internal value movements net ZERO.
    let prev = obs(1_000_000);
    let cur = obs(1_000_000);
    assert!(value_conservation(&prev, &cur, 0, 0).is_ok());
}

#[test]
fn deposit_raises_vault_by_exactly_the_inflow() {
    // A deposit of 500 raises the vault by exactly 500.
    let prev = obs(1_000_000);
    let cur = obs(1_000_500);
    assert!(value_conservation(&prev, &cur, 500, 0).is_ok());
}

#[test]
fn withdrawal_lowers_vault_by_exactly_the_outflow() {
    // A withdrawal of 500 lowers the vault by exactly 500.
    let prev = obs(1_000_000);
    let cur = obs(999_500);
    assert!(value_conservation(&prev, &cur, 0, 500).is_ok());
}

#[test]
fn simultaneous_in_and_out_net_to_the_vault_delta() {
    // Net external flow of +300 (in 800, out 500) raises the vault by exactly 300.
    let prev = obs(1_000_000);
    let cur = obs(1_000_300);
    assert!(value_conservation(&prev, &cur, 800, 500).is_ok());
}

#[test]
fn zero_vault_zero_flow_conserves() {
    let prev = obs(0);
    let cur = obs(0);
    assert!(value_conservation(&prev, &cur, 0, 0).is_ok());
}

#[test]
fn partition_under_vault_is_accepted() {
    // c_tot + insurance + backing = 700 <= vault 1000: a consistent value model.
    let prev = obs_part(1000, 400, 200, 100);
    let cur = obs_part(1000, 400, 200, 100);
    assert!(value_conservation(&prev, &cur, 0, 0).is_ok());
}

// ---------------------------------------------------------------------------
// Violation cases — value APPEARED (mint candidate)
// ---------------------------------------------------------------------------

#[test]
fn value_appearing_on_an_internal_step_is_a_violation() {
    // Vault rose by 1 with NO external inflow: a mint candidate.
    let prev = obs(1_000_000);
    let cur = obs(1_000_001);
    let v = value_conservation(&prev, &cur, 0, 0);
    assert!(v.is_err());
    assert_eq!(
        value_conservation_kind(1_000_000, 1_000_001, 0, 0, 0),
        Err(ConservationKind::ValueAppeared)
    );
}

#[test]
fn deposit_that_overcredits_the_vault_is_a_violation() {
    // Deposit of 500 but vault rose by 600: 100 atoms appeared from nowhere.
    let prev = obs(1_000_000);
    let cur = obs(1_000_600);
    assert_eq!(
        value_conservation_kind(1_000_000, 1_000_600, 500, 0, 0),
        Err(ConservationKind::ValueAppeared)
    );
    assert!(value_conservation(&prev, &cur, 500, 0).is_err());
}

// ---------------------------------------------------------------------------
// Violation cases — value DISAPPEARED (leak candidate)
// ---------------------------------------------------------------------------

#[test]
fn value_disappearing_on_an_internal_step_is_a_violation() {
    // Vault fell by 1 with NO external outflow: a leak candidate.
    let prev = obs(1_000_000);
    let cur = obs(999_999);
    let v = value_conservation(&prev, &cur, 0, 0);
    assert!(v.is_err());
    assert_eq!(
        value_conservation_kind(1_000_000, 999_999, 0, 0, 0),
        Err(ConservationKind::ValueDisappeared)
    );
}

#[test]
fn withdrawal_that_overdebits_the_vault_is_a_violation() {
    // Withdrawal of 500 but vault fell by 600: 100 atoms vanished.
    let prev = obs(1_000_000);
    let cur = obs(999_400);
    assert_eq!(
        value_conservation_kind(1_000_000, 999_400, 0, 500, 0),
        Err(ConservationKind::ValueDisappeared)
    );
    assert!(value_conservation(&prev, &cur, 0, 500).is_err());
}

// ---------------------------------------------------------------------------
// Partition cross-check — fail closed when the value model is inconsistent
// ---------------------------------------------------------------------------

#[test]
fn partition_exceeding_vault_fails_closed() {
    // c_tot + insurance + backing = 1200 > vault 1000: the engine's own
    // senior<=vault invariant is violated in the observed state, so the measure is
    // untrustworthy. The oracle must fail closed even though Δvault == net flow.
    let prev = obs_part(1000, 1000, 0, 0);
    let cur = obs_part(1000, 800, 300, 100); // 1200 > 1000
    assert_eq!(
        value_conservation_kind(1000, 1000, 0, 0, 1200),
        Err(ConservationKind::PartitionExceedsVault)
    );
    assert!(value_conservation(&prev, &cur, 0, 0).is_err());
}

// ---------------------------------------------------------------------------
// Core arithmetic — overflow safety / fail-closed
// ---------------------------------------------------------------------------

#[test]
fn core_balances_the_equation_exactly() {
    // prev_vault + ext_in == cur_vault + ext_out  is the exact predicate.
    assert!(value_conservation_kind(100, 130, 50, 20, 0).is_ok()); // 100+50 == 130+20
    assert_eq!(
        value_conservation_kind(100, 131, 50, 20, 0),
        Err(ConservationKind::ValueAppeared)
    );
    assert_eq!(
        value_conservation_kind(100, 129, 50, 20, 0),
        Err(ConservationKind::ValueDisappeared)
    );
}

#[test]
fn core_addition_overflow_fails_closed() {
    // prev_vault + ext_in overflows u128: must NOT wrap to a spurious pass.
    let r = value_conservation_kind(u128::MAX, u128::MAX, 1, 1, 0);
    assert!(r.is_err());
}
