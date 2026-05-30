//! Kani soundness proof for the v0.1 market-engine realizability CROSS-LINK
//! oracle.
//!
//! Goal: prove the cross-link oracle has NO FALSE NEGATIVES. Concretely, if
//! [`cross_link_kind`]`(&d, market_id, positive_claim_bound_num).is_ok()`, then
//! BOTH composing relationships the engine asserts in
//! `MarketGroupV16View::validate_source_credit_shape_with_market`
//! (`percolator-ref/src/v16.rs:2143-2253`, SHA `71c9032`) hold for that pairing:
//!
//!   (a) `d.source_claim_market_id == market_id`                 (v16.rs:2177)
//!   (b) `d.source_claim_bound_num <= positive_claim_bound_num`  (v16.rs:2186)
//!
//! If the oracle clears a paired (per-account domain, market side), the engine's
//! cross-link invariant provably holds for it — so a reported finding can never be
//! a harness arithmetic bug.
//!
//! ## Input model (matches the engine's / O1 proof discipline)
//!
//! Like `tests/kani_oracles.rs`, scaled "num" fields are small whole-atom
//! multiples of `SCALE` (`BOUND_SCALE`, `percolator-ref/src/lib.rs:25`) plus an
//! independent symbolic sub-`SCALE` remainder, so the proof covers the exact
//! `>` boundary of check (b) both at, just below, and just above equality —
//! including non-atom-aligned values. `market_id` and `source_claim_market_id`
//! range over a tiny symbolic set so the proof covers both the matching and
//! mismatching cases of check (a). The composed magnitudes stay far under
//! `MAX_ACCOUNT_NOTIONAL == 1e20` (`percolator-ref/src/lib.rs:29`), i.e. inside
//! the engine's reachable state.
//!
//! This file is only compiled under Kani (`cargo kani`); it is inert otherwise.
#![cfg(kani)]

use perp_adversary::driver::DomainObs;
use perp_adversary::oracles::{cross_link_kind, SCALE};

/// Build a scaled "num" value `whole * SCALE + rem` with a symbolic whole-atom
/// multiplier (`<= 6`) and a symbolic strictly-sub-`SCALE` remainder (`<= 3`).
/// The cross-link's only num comparison is `bound > positive_bound` (check b),
/// whose truth depends only on the ordering of the two composed values; the
/// `{0,1,2,3}` remainder set covers the at-/above-/below-equality boundary while
/// keeping the SMT formula over small bit-vectors (CBMC-tractable). Mirrors the
/// O1 proof's `scaled_num` (`tests/kani_oracles.rs:52`).
fn scaled_num() -> u128 {
    let whole: u8 = kani::any();
    let rem: u128 = kani::any();
    kani::assume(whole <= 6);
    kani::assume(rem <= 3);
    whole as u128 * SCALE + rem
}

/// A tiny symbolic market id in `{0,1,2}` — enough to span "matches" and
/// "mismatches" of the per-account `source_claim_market_id` (check a).
fn small_market_id() -> u64 {
    let v: u64 = kani::any();
    kani::assume(v <= 2);
    v
}

#[kani::proof]
fn market_cross_link_is_sound() {
    // The market side the engine reads (its asset's market_id + that side's
    // positive_claim_bound_num). market_id is symbolic so check (a) is exercised
    // both directions; the bound is a symbolic scaled num.
    let market_id = small_market_id();
    let positive_claim_bound_num = scaled_num();

    // Only the two cross-link-relevant per-account fields participate; the rest do
    // not affect `cross_link_kind`, so leave them at default. `source_claim_*`
    // are symbolic so both checks range over matching and breaching inputs.
    let d = DomainObs {
        source_claim_market_id: small_market_id(),
        source_claim_bound_num: scaled_num(),
        ..DomainObs::default()
    };

    // If the oracle clears the pairing, both engine cross-link relationships hold.
    if cross_link_kind(&d, market_id, positive_claim_bound_num).is_ok() {
        // (a) market id binding (v16.rs:2177).
        assert!(d.source_claim_market_id == market_id);
        // (b) the realizability cross-link: per-account bound <= market bound
        //     (v16.rs:2186).
        assert!(d.source_claim_bound_num <= positive_claim_bound_num);
    }
}
