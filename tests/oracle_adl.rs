//! v0.3 ADL exact-accounting oracle unit tests (TDD — written before the
//! implementation).
//!
//! These fixtures mirror the cross-step guarantee the engine makes when it
//! applies a quantity-ADL to a finalized-close account
//! (`apply_quantity_adl_after_residual_for_account_not_atomic`,
//! `percolator-ref/src/v16.rs:9479`). The engine's accounting is exact:
//!
//!   * The ADL closes `out.closed_q == close_q` of the leg (`v16.rs:9676`), and
//!     `close_q > 0` is required (`v16.rs:9609`/`9522` reject a zero close), so a
//!     real ADL ALWAYS closes a non-zero quantity.
//!   * `advance_close_progress_quantity_adl(account, out.closed_q)` (`v16.rs:9510`)
//!     sets `ledger.quantity_adl_applied_q = out.closed_q` (`v16.rs:9533`) and
//!     FIRST requires the prior `ledger.quantity_adl_applied_q == 0`
//!     (`v16.rs:9530`, else `LockActive`). So across one ApplyAdl step the
//!     account's `close_progress.quantity_adl_applied_q` rises from 0 by EXACTLY
//!     `closed_q`.
//!
//! This is a genuine cross-STEP delta property: it reads the change in the
//! account's `quantity_adl_applied_q` ledger value between the pre-ADL observation
//! and the post-ADL observation, and pins that delta to the outcome's `closed_q`.
//! A single-state re-check would be VACUOUS — every observed state already passed
//! `validate_shape` + `validate_with_market`, and the seeded finalized-close
//! ledger's residual equation is itself engine-validated. Only the BEFORE/AFTER
//! comparison of the running `quantity_adl_applied_q` total against `closed_q`
//! exposes that the ADL was exactly accounted in the ledger.

use perp_adversary::driver::{AccountObs, AdlObs, Observation};
use perp_adversary::scenario::Action;

/// An account carrying only the `quantity_adl_applied_q` ledger value the oracle
/// reads (other fields are irrelevant to ADL accounting).
fn acct(quantity_adl_applied_q: u128) -> AccountObs {
    AccountObs {
        quantity_adl_applied_q,
        ..AccountObs::default()
    }
}

/// A pre-ADL observation: a `SeedFinalizedClose` step with `accounts`, no ADL
/// outcome.
fn prev_obs(accounts: Vec<AccountObs>) -> Observation {
    Observation {
        step: 0,
        action: Action::SeedFinalizedClose {
            account: 0,
            asset: 0,
            bankrupt_side: 0,
        },
        result: Ok(()),
        accounts,
        market_domains: vec![],
        liquidation: None,
        adl: None,
    }
}

/// A post-ADL observation: an `ApplyAdl` step with `accounts` and an ADL outcome
/// of `closed_q` whose ledger landed at `applied_q`.
fn cur_obs(accounts: Vec<AccountObs>, closed_q: u128, applied_q: u128) -> Observation {
    Observation {
        step: 1,
        action: Action::ApplyAdl {
            account: 0,
            asset: 0,
            bankrupt_side: 0,
            close_q: closed_q,
        },
        result: Ok(()),
        accounts,
        market_domains: vec![],
        liquidation: None,
        adl: Some(AdlObs {
            closed_q,
            opposite_a_after: 1,
            reset_started: false,
            quantity_adl_applied_q: applied_q,
        }),
    }
}

// ---- the non-ADL case is a no-op (the oracle is delta-only) ----

#[test]
fn non_adl_step_is_vacuously_ok() {
    use perp_adversary::oracles::adl_accounting;
    // cur has no ADL outcome -> the accounting oracle does not apply.
    let prev = prev_obs(vec![acct(0)]);
    let mut cur = prev_obs(vec![acct(7)]); // a wild ledger jump, but no ADL outcome
    cur.step = 1;
    assert_eq!(adl_accounting(&prev, &cur), Ok(()));
}

// ---- the happy path: ledger rose by EXACTLY closed_q, from 0 ----

#[test]
fn ledger_rises_by_exactly_closed_q_is_ok() {
    use perp_adversary::oracles::adl_accounting;
    // prev ledger 0; ADL closes 1_000; post-ledger 1_000 == 0 + 1_000.
    let prev = prev_obs(vec![acct(0)]);
    let cur = cur_obs(vec![acct(1_000)], 1_000, 1_000);
    assert_eq!(adl_accounting(&prev, &cur), Ok(()));
}

#[test]
fn ledger_rises_by_exactly_closed_q_from_nonzero_prev_is_ok() {
    use perp_adversary::oracles::adl_accounting;
    // The oracle pins the DELTA, not the absolute value: if for any reason a prior
    // applied quantity were already present, a clean ADL still raises the ledger by
    // exactly closed_q. (The engine forbids prev != 0 at v16.rs:9530, but the
    // oracle's delta law is the faithful general statement and stays correct here.)
    let prev = prev_obs(vec![acct(500)]);
    let cur = cur_obs(vec![acct(1_500)], 1_000, 1_500);
    assert_eq!(adl_accounting(&prev, &cur), Ok(()));
}

// ---- violations ----

#[test]
fn ledger_delta_less_than_closed_q_is_violation() {
    use perp_adversary::oracles::adl_accounting;
    // ADL claims to close 1_000 but the ledger only advanced by 600: the ADL is
    // UNDER-accounted in the ledger.
    let prev = prev_obs(vec![acct(0)]);
    let cur = cur_obs(vec![acct(600)], 1_000, 600);
    adl_accounting(&prev, &cur).unwrap_err();
}

#[test]
fn ledger_delta_more_than_closed_q_is_violation() {
    use perp_adversary::oracles::adl_accounting;
    // The ledger advanced by 1_400 but the outcome only closed 1_000: more applied
    // quantity was booked than the ADL actually closed.
    let prev = prev_obs(vec![acct(0)]);
    let cur = cur_obs(vec![acct(1_400)], 1_000, 1_400);
    adl_accounting(&prev, &cur).unwrap_err();
}

#[test]
fn zero_closed_q_is_violation() {
    use perp_adversary::oracles::adl_accounting;
    // The engine never produces an ADL outcome with closed_q == 0 (v16.rs:9609 /
    // 9522 reject it); an observed ADL claiming a zero close is fail-closed.
    let prev = prev_obs(vec![acct(0)]);
    let cur = cur_obs(vec![acct(0)], 0, 0);
    adl_accounting(&prev, &cur).unwrap_err();
}

#[test]
fn ledger_went_backward_is_violation() {
    use perp_adversary::oracles::adl_accounting;
    // quantity_adl_applied_q can only advance in the engine; a backward move cannot
    // be explained by any ADL and is fail-closed.
    let prev = prev_obs(vec![acct(900)]);
    let cur = cur_obs(vec![acct(400)], 1_000, 400);
    adl_accounting(&prev, &cur).unwrap_err();
}

#[test]
fn adl_outcome_applied_q_inconsistent_with_account_is_violation() {
    use perp_adversary::oracles::adl_accounting;
    // The outcome's reported post-ledger value (1_000) disagrees with the account's
    // actual observed ledger value (1_001): the two views of the SAME ledger field
    // must coincide, else accounting is incoherent. Fail closed.
    let prev = prev_obs(vec![acct(0)]);
    let cur = cur_obs(vec![acct(1_001)], 1_000, 1_000);
    adl_accounting(&prev, &cur).unwrap_err();
}

#[test]
fn missing_prev_account_is_fail_closed() {
    use perp_adversary::oracles::adl_accounting;
    // The ADL acted on account 0, but `prev` has NO account 0 to read the baseline
    // ledger value from; the delta cannot be certified. Fail closed.
    let prev = prev_obs(vec![]);
    let cur = cur_obs(vec![acct(1_000)], 1_000, 1_000);
    adl_accounting(&prev, &cur).unwrap_err();
}
