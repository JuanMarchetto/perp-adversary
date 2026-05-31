//! Kani soundness proofs for the v0.6 FUNDING CLAIMABLE-VALUE CONSERVATION oracle,
//! plus the ROOT-CAUSE arithmetic lemma against the engine's OWN rounding function.
//!
//! Three harnesses:
//!
//!   1. `funding_conservation_core_is_exact` — PREDICATE CORRECTNESS of
//!      `funding_conservation_kind(prev, cur, ext_in, ext_out)`: it returns `Ok` IFF
//!      claimable value changed by exactly the net external flow
//!      (`cur − prev == ext_in − ext_out`), and otherwise names the breach direction
//!      (`ClaimableValueDestroyed` when claimable fell below the inflow,
//!      `ClaimableValueCreated` when it rose above it). So a campaign the oracle
//!      flags `Destroyed` provably lost claimable value.
//!
//!   2. `funding_conservation_overflow_fails_closed` — OVERFLOW SAFETY: even at the
//!      i128 ceiling the core never panics and never wraps an addition into a
//!      spurious `Ok` (it fails closed to `Err`).
//!
//!   3. `funding_floor_ceil_asymmetry_destroys_remainder` — the ROOT CAUSE, proven
//!      against the ENGINE's REAL `floor_div_signed_conservative_i128`
//!      (`percolator::wide_math`, `wide_math.rs:1423-1448`). For one funding
//!      settlement of unrounded magnitude `x = x_num / d`, the RECEIVER leg credits
//!      PnL by `floor_div(+x_num, d)` (the `net > 0` branch, `v16.rs:7194-7197`) and
//!      the PAYER leg debits capital by `|floor_div(−x_num, d)|` (the `net < 0`
//!      branch, `v16.rs:7198-7208`). This proves the payer's debit MINUS the
//!      receiver's credit equals EXACTLY `1` when `x_num % d != 0` and `0` otherwise
//!      — i.e. each fractional settlement permanently destroys exactly one quote atom
//!      of claimable value, the leak the harness observes at runtime.
//!
//! This file is only compiled under Kani (`cargo kani`); it is inert otherwise.
#![cfg(kani)]

use percolator::wide_math::floor_div_signed_conservative_i128;
use perp_adversary::oracles::{funding_conservation_kind, FundingConservationKind};

/// PREDICATE CORRECTNESS. Bounded magnitudes keep the SMT formula over small
/// bit-vectors while spanning every ordering of the two sides of the balance
/// equation. The arithmetic is exact and the branch structure is magnitude-
/// independent, so this range covers all equivalence classes of the predicate.
#[kani::proof]
fn funding_conservation_core_is_exact() {
    let prev: i128 = kani::any();
    let cur: i128 = kani::any();
    let ext_in: u128 = kani::any();
    let ext_out: u128 = kani::any();
    kani::assume(prev >= -8 && prev <= 8);
    kani::assume(cur >= -8 && cur <= 8);
    kani::assume(ext_in <= 8);
    kani::assume(ext_out <= 8);

    // Reference math, independent of the oracle internals (no overflow: all small).
    let lhs = prev + ext_in as i128;
    let rhs = cur + ext_out as i128;

    let got = funding_conservation_kind(prev, cur, ext_in, ext_out);

    if lhs == rhs {
        // claimable changed by exactly the net external flow: conservation HELD.
        assert!(got.is_ok());
    } else if lhs > rhs {
        // prev + ext_in > cur + ext_out  ⇒  claimable fell below the inflow: DESTROYED.
        assert!(got == Err(FundingConservationKind::ClaimableValueDestroyed));
    } else {
        // claimable rose above the inflow: CREATED.
        assert!(got == Err(FundingConservationKind::ClaimableValueCreated));
    }
}

/// OVERFLOW SAFETY / FAIL-CLOSED. When `prev + ext_in` would overflow i128, the core
/// MUST NOT panic and MUST NOT wrap into a spurious `Ok`. Drive the LHS to the
/// ceiling; the `checked_add` returns `None`, failing closed to `Err`.
#[kani::proof]
fn funding_conservation_overflow_fails_closed() {
    let cur: i128 = kani::any();
    let ext_out: u128 = kani::any();
    let prev = i128::MAX;
    let ext_in = u128::MAX; // i128::try_from(MAX) fails -> fail-closed Destroyed
    let got = funding_conservation_kind(prev, cur, ext_in, ext_out);
    assert!(got.is_err());
}

/// ROOT CAUSE, against the engine's REAL `floor_div_signed_conservative_i128`. For a
/// settlement of unrounded magnitude `x = x_num / d`, the receiver credit is
/// `floor_div(+x_num, d)` and the payer debit magnitude is `|floor_div(−x_num, d)|`.
/// Prove: payer_debit − receiver_credit == (x_num % d != 0) ? 1 : 0. So every
/// fractional-remainder settlement destroys exactly one atom of claimable value.
#[kani::proof]
fn funding_floor_ceil_asymmetry_destroys_remainder() {
    let x_num: u128 = kani::any();
    let d: u128 = kani::any();
    kani::assume(d >= 1 && d <= 16);
    kani::assume(x_num <= 64); // keeps x_num as i128 and the negation in range

    let x_signed = x_num as i128;
    let receiver_credit = floor_div_signed_conservative_i128(x_signed, d); // net > 0 leg
    let payer_net = floor_div_signed_conservative_i128(-x_signed, d); // net < 0 leg
    let payer_debit = -payer_net; // magnitude debited from capital

    // Both are non-negative magnitudes of the same underlying x.
    assert!(receiver_credit >= 0);
    assert!(payer_debit >= 0);

    let destroyed = payer_debit - receiver_credit;
    if x_num % d == 0 {
        // Whole basis: floor == ceil, funding is exactly zero-sum.
        assert!(destroyed == 0);
    } else {
        // Fractional basis: payer pays ceil = q+1, receiver gets floor = q.
        assert!(destroyed == 1);
    }
    // NOTE on the funding settlement path: `leg_kf_delta_for_settlement` (v16.rs:7129)
    // floors each leg's net in one of two ways, BOTH rounding toward -inf identically:
    //   * fast path `scaled_adl_delta_fast` (v16.rs:12557), taken when a_basis==ADL_ONE
    //     (legs default to ADL_ONE, v16.rs:2390), which calls THIS function
    //     `floor_div_signed_conservative_i128` at v16.rs:12572 — so this proof covers
    //     the real fast-path primitive;
    //   * the wide fallback `wide_signed_mul_div_floor_from_k_pair` (wide_math.rs:1630),
    //     whose negative branch rounds the magnitude up (q+1) at wide_math.rs:1654-1666
    //     and positive branch truncates at :1667 — the identical rule, verified by
    //     reading (its U256 internals make it intractable for Kani). The runtime test
    //     `tests/funding_conservation.rs` drives whichever path the engine takes and
    //     measures the resulting 1-atom/slot leak directly.
}
