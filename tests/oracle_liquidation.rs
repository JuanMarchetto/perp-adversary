//! v0.2 liquidation insurance-domain ISOLATION oracle unit tests (TDD — written
//! before the implementation).
//!
//! These fixtures mirror the cross-step guarantee the engine makes when it
//! liquidates an account: a long-leg bankruptcy may spend insurance ONLY from
//! that leg's asset's bankruptcy insurance domain
//! (`consume_domain_insurance_for_negative_pnl`, `percolator-ref/src/v16.rs:5955`,
//! which charges `insurance_domain_index(asset_index, opposite_side(leg.side))`),
//! and the amount it spends equals the `insurance_used` in the outcome
//! (`v16.rs:5975` increments that one domain's `insurance_domain_spent` by exactly
//! `used`). No OTHER asset's domain may be charged. The engine's own conformance
//! test asserts the no-drain case
//! (`tests/v16_spec_tests.rs:342-348`: `insurance_used == 0`,
//! `insurance_domain_spent_short == 0`), and its Kani proofs bound the funded case
//! (`tests/proofs_v16.rs:2375` budget-caps spend; `:2449` unfunded cannot drain).
//!
//! This is a genuine cross-STEP delta property: it reads the change in each
//! market side's `insurance_domain_spent` between the pre-liquidation observation
//! and the post-liquidation observation. A single-state re-check would be vacuous
//! (every observed state already passed `validate_shape` + `validate_with_market`).
//!
//! Domain pairing: market side `(asset, side)` is the engine's domain
//! `asset*2 + side` (`domain_asset_side`, v16.rs:4754). The oracle keys the
//! liquidated domains off `cur.action`'s `Liquidate { asset, .. }` — both sides of
//! the liquidated asset are the only domains the engine may charge for that
//! liquidation.

use perp_adversary::driver::{
    AccountObs, EngineDomainObs, LiquidationObs, MarketSideObs, Observation,
};
use perp_adversary::oracles::liquidation_insurance_isolation;
use perp_adversary::scenario::Action;

/// A market side carrying only an `insurance_domain_spent` value (the field the
/// isolation oracle reads); other engine fields are irrelevant to it.
fn side(asset: usize, side: u8, insurance_domain_spent: u128) -> MarketSideObs {
    MarketSideObs {
        asset,
        side,
        market_id: (asset as u64) + 1,
        state: EngineDomainObs::default(),
        insurance_domain_spent,
    }
}

/// An observation carrying market sides and (optionally) a liquidation outcome on
/// `liq_asset`. A non-liquidation observation passes `None` for `liq`.
fn obs(market_domains: Vec<MarketSideObs>, liq: Option<(u8, LiquidationObs)>) -> Observation {
    let (action, liquidation) = match liq {
        Some((asset, l)) => (
            Action::Liquidate {
                account: 0,
                asset,
                close_q: 1_000_000,
            },
            Some(l),
        ),
        None => (
            Action::Deposit {
                account: 0,
                amount: 0,
            },
            None,
        ),
    };
    Observation {
        step: 0,
        action,
        result: Ok(()),
        accounts: vec![AccountObs::default()],
        market_domains,
        liquidation,
    }
}

fn liq(insurance_used: u128) -> LiquidationObs {
    LiquidationObs {
        closed_q: 1_000_000,
        insurance_used,
        residual_booked: 0,
        explicit_loss: 0,
        fee_charged: 0,
    }
}

// ---- the non-liquidation case is a no-op (the oracle is delta-only) ----

#[test]
fn no_liquidation_step_is_vacuously_ok() {
    // cur has no liquidation outcome -> the isolation oracle does not apply.
    let prev = obs(vec![side(0, 0, 0), side(0, 1, 0)], None);
    let cur = obs(vec![side(0, 0, 9), side(0, 1, 9)], None);
    assert_eq!(liquidation_insurance_isolation(&prev, &cur), Ok(()));
}

// ---- the no-insurance path (engine books residual, spends nothing) ----

#[test]
fn liquidation_with_zero_insurance_and_no_spend_is_ok() {
    // Mirrors `v16_public_liquidation_on_unfunded_domain_cannot_drain_shared_insurance`:
    // insurance_used == 0 and NO domain's spend moved.
    let prev = obs(vec![side(0, 0, 0), side(0, 1, 0)], None);
    let cur = obs(vec![side(0, 0, 0), side(0, 1, 0)], Some((0, liq(0))));
    assert_eq!(liquidation_insurance_isolation(&prev, &cur), Ok(()));
}

// ---- the funded path: insurance IS spent, ONLY on the liquidated domain ----

#[test]
fn liquidation_spending_insurance_on_liquidated_domain_only_is_ok() {
    // Liquidated asset 0's SHORT-side (the long leg's bankruptcy domain) spend
    // rises 0 -> 5; insurance_used == 5; no other domain moves.
    let prev = obs(vec![side(0, 0, 0), side(0, 1, 0)], None);
    let cur = obs(vec![side(0, 0, 0), side(0, 1, 5)], Some((0, liq(5))));
    assert_eq!(liquidation_insurance_isolation(&prev, &cur), Ok(()));
}

#[test]
fn liquidation_spend_on_long_side_of_liquidated_asset_is_ok() {
    // A short-leg liquidation would charge the asset's LONG-side domain; the
    // oracle keys on the liquidated ASSET, so either side of that asset is allowed.
    let prev = obs(
        vec![side(0, 0, 0), side(0, 1, 0), side(1, 0, 0), side(1, 1, 0)],
        None,
    );
    let cur = obs(
        vec![side(0, 0, 3), side(0, 1, 0), side(1, 0, 0), side(1, 1, 0)],
        Some((0, liq(3))),
    );
    assert_eq!(liquidation_insurance_isolation(&prev, &cur), Ok(()));
}

#[test]
fn pre_existing_spend_on_another_domain_unchanged_is_ok() {
    // Another asset's domain already carries spend from a PRIOR liquidation; as
    // long as it does not INCREASE across THIS step it is not a cross-domain drain.
    let prev = obs(
        vec![side(0, 0, 0), side(0, 1, 0), side(1, 0, 7), side(1, 1, 0)],
        None,
    );
    let cur = obs(
        vec![side(0, 0, 0), side(0, 1, 4), side(1, 0, 7), side(1, 1, 0)],
        Some((0, liq(4))),
    );
    assert_eq!(liquidation_insurance_isolation(&prev, &cur), Ok(()));
}

// ---- violations ----

#[test]
fn cross_domain_drain_is_violation() {
    // Liquidating asset 0, but asset 1's domain spend ALSO increased: a
    // cross-domain insurance drain the engine forbids.
    let prev = obs(
        vec![side(0, 0, 0), side(0, 1, 0), side(1, 0, 0), side(1, 1, 0)],
        None,
    );
    let cur = obs(
        vec![side(0, 0, 0), side(0, 1, 5), side(1, 0, 2), side(1, 1, 0)],
        Some((0, liq(5))),
    );
    let err = liquidation_insurance_isolation(&prev, &cur).unwrap_err();
    assert!(
        err.detail.contains("asset 1"),
        "detail should name the drained foreign domain: {}",
        err.detail
    );
}

#[test]
fn insurance_used_exceeding_liquidated_domain_spend_is_violation() {
    // insurance_used (5) is more than the liquidated asset's domains' spend
    // increase (3): the outcome claims more insurance was spent than any observed
    // domain accounts for.
    let prev = obs(vec![side(0, 0, 0), side(0, 1, 0)], None);
    let cur = obs(vec![side(0, 0, 0), side(0, 1, 3)], Some((0, liq(5))));
    liquidation_insurance_isolation(&prev, &cur).unwrap_err();
}

#[test]
fn insurance_used_less_than_total_spend_is_violation() {
    // The total observed spend increase (5) EXCEEDS the reported insurance_used
    // (3): insurance moved on a domain without being reported in the outcome.
    let prev = obs(vec![side(0, 0, 0), side(0, 1, 0)], None);
    let cur = obs(vec![side(0, 0, 0), side(0, 1, 5)], Some((0, liq(3))));
    liquidation_insurance_isolation(&prev, &cur).unwrap_err();
}

#[test]
fn spend_going_backward_is_violation() {
    // insurance_domain_spent is monotone in the engine; a DECREASE cannot be
    // explained by any liquidation and is fail-closed as a violation.
    let prev = obs(vec![side(0, 0, 0), side(0, 1, 9)], None);
    let cur = obs(vec![side(0, 0, 0), side(0, 1, 4)], Some((0, liq(0))));
    liquidation_insurance_isolation(&prev, &cur).unwrap_err();
}

#[test]
fn missing_prev_side_is_fail_closed() {
    // A market side present in `cur` but absent in `prev` cannot have its delta
    // certified; fail closed rather than silently treat the baseline as zero.
    let prev = obs(vec![side(0, 0, 0)], None);
    let cur = obs(vec![side(0, 0, 0), side(0, 1, 5)], Some((0, liq(5))));
    liquidation_insurance_isolation(&prev, &cur).unwrap_err();
}
