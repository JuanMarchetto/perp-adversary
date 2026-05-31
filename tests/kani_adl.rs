//! Kani soundness proof for the v0.3 quantity-ADL EXACT-ACCOUNTING oracle.
//!
//! Goal: prove the oracle has NO FALSE NEGATIVES. Concretely, if
//! [`adl_accounting_kind`]`(prev_applied_q, cur_applied_q, &adl).is_ok()`, then the
//! engine's cross-step ADL accounting guarantees hold for that ApplyAdl step:
//!
//!   (E1) EXACT ACCOUNTING: the account's `quantity_adl_applied_q` ledger value
//!        rose by EXACTLY `adl.closed_q` across the step
//!        (`cur_applied_q == prev_applied_q + adl.closed_q`) — mirrors
//!        `advance_close_progress_quantity_adl` setting
//!        `ledger.quantity_adl_applied_q = out.closed_q` from a prior value of 0
//!        (`percolator-ref/src/v16.rs:9510,9530,9533`); and
//!   (E2) NON-VACUOUS CLOSE: `adl.closed_q > 0` — mirrors the engine rejecting a
//!        zero-quantity ADL (`v16.rs:9609`/`9522`); and
//!   (E3) LEDGER COHERENCE: the outcome's reported post-ledger value equals the
//!        account's observed ledger value (the two views of the same engine field
//!        coincide, `v16.rs:9533`).
//!
//! If the oracle clears an ApplyAdl step, the engine's exact-accounting invariant
//! provably holds for it — so a reported finding can never be a harness arithmetic
//! bug (e.g. a wrong-direction subtraction or an off-by-one in the delta).
//!
//! ## Input model (mirrors the `tests/kani_liquidation.rs` / O1 proof discipline)
//!
//! The oracle core reads exactly three `u128` quantities: the pre-step baseline
//! ledger value, the post-step ledger value, and the ADL outcome (`closed_q` plus
//! the reported post-ledger value; `opposite_a_after` / `reset_started` do not
//! participate). Each is drawn symbolically from a tiny range so the SMT formula
//! stays over small bit-vectors (CBMC-tractable) while still spanning increase /
//! no-change / decrease and every (im)balance between the delta and `closed_q`,
//! including the zero-close case. The magnitudes stay far under the engine's
//! reachable `POS_SCALE`-class quantities (`percolator-ref/src/lib.rs`), i.e.
//! inside its operating range; the accounting law is scale-invariant.
//!
//! This file is only compiled under Kani (`cargo kani`); it is inert otherwise.
#![cfg(kani)]

use perp_adversary::driver::AdlObs;
use perp_adversary::oracles::adl_accounting_kind;

/// A tiny symbolic ledger / close quantity in `0..=4` — enough to span increase,
/// no-change, and decrease against another symbolic value of the same range, and
/// to cover `closed_q == 0`, while keeping the SMT formula over small bit-vectors.
fn small_q() -> u128 {
    let v: u128 = kani::any();
    kani::assume(v <= 4);
    v
}

#[kani::proof]
#[kani::unwind(4)]
fn adl_accounting_is_sound() {
    // Independent symbolic pre/post ledger values and an ADL outcome whose
    // `closed_q` and reported post-ledger value are each symbolic over the tiny
    // range. `opposite_a_after` / `reset_started` are irrelevant to the core.
    let prev_applied_q = small_q();
    let cur_applied_q = small_q();
    let adl = AdlObs {
        closed_q: small_q(),
        opposite_a_after: 1,
        reset_started: false,
        quantity_adl_applied_q: small_q(),
    };

    // The proof reasons about the allocation-free core (no `format!`), exactly as
    // the O1 / cross-link / isolation proofs do.
    if adl_accounting_kind(prev_applied_q, cur_applied_q, &adl).is_ok() {
        // (E2) NON-VACUOUS CLOSE: a cleared ADL always closed a positive quantity.
        assert!(adl.closed_q > 0);

        // (E3) LEDGER COHERENCE: the observed account ledger value matches the
        //      outcome's reported post-ledger value.
        assert!(cur_applied_q == adl.quantity_adl_applied_q);

        // (E1) EXACT ACCOUNTING: the ledger rose by EXACTLY closed_q. In
        //      particular it did NOT move backward (cur >= prev) and the delta is
        //      precisely the closed quantity — neither over- nor under-credited.
        assert!(cur_applied_q >= prev_applied_q);
        assert!(cur_applied_q - prev_applied_q == adl.closed_q);
    }
}
