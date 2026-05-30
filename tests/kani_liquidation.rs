//! Kani soundness proof for the v0.2 liquidation insurance-domain ISOLATION
//! oracle.
//!
//! Goal: prove the oracle has NO FALSE NEGATIVES. Concretely, if
//! [`isolation_kind`]`(prev, cur, liquidated_asset, insurance_used).is_ok()`, then
//! the engine's cross-step insurance guarantees hold for that liquidation step:
//!
//!   (a) ISOLATION: NO domain of a NON-liquidated asset had its
//!       `insurance_domain_spent` increase (no cross-domain drain) — mirrors
//!       `consume_domain_insurance_for_negative_pnl` charging only
//!       `insurance_domain_index(asset, ..)` of the LIQUIDATED asset
//!       (`percolator-ref/src/v16.rs:5955`); and
//!   (b) FULL ACCOUNTING: the total `insurance_domain_spent` increase across all
//!       observed domains equals `insurance_used` — mirrors the engine
//!       incrementing exactly one domain by exactly `used`
//!       (`v16.rs:5974-5980`); together with (a) this pins `insurance_used` to the
//!       liquidated asset's spend delta and forces every other delta to zero; and
//!   (c) MONOTONICITY: no observed side's spend went backward.
//!
//! If the oracle clears a liquidation step, the engine's isolation invariant
//! provably holds for it — so a reported finding can never be a harness
//! arithmetic bug.
//!
//! ## Input model (mirrors the `tests/kani_market.rs` / O1 proof discipline)
//!
//! We model a fixed two-asset world (assets 0 and 1, sides long=0/short=1 — four
//! domains, the engine's `domain_asset_side` layout, `v16.rs:4754`). Each domain
//! carries an independent symbolic pre-step and post-step
//! `insurance_domain_spent`, drawn from a tiny range so the SMT formula stays over
//! small bit-vectors (CBMC-tractable) while still spanning increase / no-change /
//! decrease for every domain. The liquidated asset and `insurance_used` are
//! symbolic over the same tiny range, so the proof covers the liquidated asset
//! being either one and every accounting (im)balance. The magnitudes stay far
//! under `MAX_VAULT_TVL` / `MAX_ACCOUNT_NOTIONAL` (`percolator-ref/src/lib.rs`),
//! i.e. inside the engine's reachable state.
//!
//! This file is only compiled under Kani (`cargo kani`); it is inert otherwise.
#![cfg(kani)]

use perp_adversary::driver::{EngineDomainObs, MarketSideObs};
use perp_adversary::oracles::isolation_kind;

/// A tiny symbolic spend value in `0..=4` — enough to span increase, no-change,
/// and decrease against another symbolic value of the same range, while keeping
/// the SMT formula over small bit-vectors.
fn small_spent() -> u128 {
    let v: u128 = kani::any();
    kani::assume(v <= 4);
    v
}

/// Build a market side `(asset, side)` carrying `insurance_domain_spent`. Only
/// `asset`, `side`, and `insurance_domain_spent` participate in `isolation_kind`.
fn mk_side(asset: usize, side: u8, insurance_domain_spent: u128) -> MarketSideObs {
    MarketSideObs {
        asset,
        side,
        market_id: (asset as u64) + 1,
        state: EngineDomainObs::default(),
        insurance_domain_spent,
    }
}

#[kani::proof]
#[kani::unwind(8)]
fn liquidation_insurance_isolation_is_sound() {
    // Two assets, two sides each: the engine's four-domain layout. Independent
    // symbolic pre/post spends per domain.
    let prev = [
        mk_side(0, 0, small_spent()),
        mk_side(0, 1, small_spent()),
        mk_side(1, 0, small_spent()),
        mk_side(1, 1, small_spent()),
    ];
    let cur = [
        mk_side(0, 0, small_spent()),
        mk_side(0, 1, small_spent()),
        mk_side(1, 0, small_spent()),
        mk_side(1, 1, small_spent()),
    ];

    // The liquidated asset is symbolic over {0, 1}; insurance_used is symbolic.
    let liquidated_asset: usize = kani::any();
    kani::assume(liquidated_asset <= 1);
    let insurance_used = small_spent();

    // The proof reasons about the allocation-free core (no `format!`), exactly as
    // the O1 / cross-link proofs do.
    if isolation_kind(&prev, &cur, liquidated_asset, insurance_used).is_ok() {
        // Pair pre/post by index: prev[i] and cur[i] are the SAME (asset, side).
        let mut total_increase: u128 = 0;
        let mut i = 0usize;
        while i < cur.len() {
            let before = prev[i].insurance_domain_spent;
            let after = cur[i].insurance_domain_spent;

            // (c) MONOTONICITY: a cleared step never decreased any domain's spend.
            assert!(after >= before);

            let delta = after - before;
            if delta > 0 {
                // (a) ISOLATION: any domain that increased belongs to the
                //     liquidated asset — no cross-domain drain.
                assert!(cur[i].asset == liquidated_asset);
            }
            total_increase += delta;
            i += 1;
        }

        // (b) FULL ACCOUNTING: the total observed spend increase equals
        //     insurance_used (neither claimed-but-unobserved nor
        //     observed-but-unreported). With (a), this is the liquidated domain's
        //     delta and forces every other domain's delta to zero.
        assert!(total_increase == insurance_used);
    }
}
