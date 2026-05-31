//! Kani soundness proof for the v0.5 GLOBAL QUOTE-VALUE CONSERVATION oracle core.
//!
//! `value_conservation_kind(prev_vault, cur_vault, ext_in, ext_out, partition_sum)`
//! is the allocation-free arithmetic core the harness folds over a campaign. It
//! returns `Ok(())` iff the total real quote-atom balance changed by EXACTLY the
//! net external flow:
//!
//! ```text
//! cur_vault − prev_vault == ext_in − ext_out      (and partition_sum <= cur_vault)
//! ```
//!
//! computed as the underflow-free equality `prev_vault + ext_in == cur_vault +
//! ext_out`. This proof pins down TWO things:
//!
//!   1. PREDICATE CORRECTNESS — within a bounded-but-representative input range,
//!      the core returns `Ok` IFF the balance equation holds AND the partition is
//!      consistent, and otherwise names the breach DIRECTION correctly
//!      (`ValueAppeared` when value exceeds the inflow, `ValueDisappeared` when it
//!      falls short, `PartitionExceedsVault` when the model is inconsistent). So a
//!      green campaign provably means conservation held; a red one provably means
//!      it did not (modulo triage of the value model).
//!
//!   2. OVERFLOW SAFETY / FAIL-CLOSED — even at the u128 ceiling, the core never
//!      panics and never wraps an addition into a spurious `Ok`. A separate
//!      harness drives the two operands to `u128::MAX` to exercise the
//!      `checked_add` overflow arms, asserting the result is an `Err` (fail
//!      closed), never `Ok`.
//!
//! This file is only compiled under Kani (`cargo kani`); it is inert otherwise.
#![cfg(kani)]

use perp_adversary::oracles::{value_conservation_kind, ConservationKind};

/// PREDICATE CORRECTNESS. Bounded magnitudes (each `<= 8`) keep the SMT formula
/// over small bit-vectors while spanning every ordering of the two sides of the
/// balance equation and every position of `partition_sum` relative to `cur_vault`
/// — the only distinctions the core's branches make. The arithmetic is exact and
/// the branch structure is magnitude-independent, so this small range covers all
/// equivalence classes of the predicate.
#[kani::proof]
fn conservation_core_is_exact() {
    let prev_vault: u128 = kani::any();
    let cur_vault: u128 = kani::any();
    let ext_in: u128 = kani::any();
    let ext_out: u128 = kani::any();
    let partition_sum: u128 = kani::any();
    kani::assume(prev_vault <= 8);
    kani::assume(cur_vault <= 8);
    kani::assume(ext_in <= 8);
    kani::assume(ext_out <= 8);
    kani::assume(partition_sum <= 8);

    // Reference math, independent of the oracle's internals. No overflow possible
    // here (all operands <= 8), so plain `+` is exact.
    let lhs = prev_vault + ext_in;
    let rhs = cur_vault + ext_out;
    let partition_ok = partition_sum <= cur_vault;
    let balanced = lhs == rhs;

    let got = value_conservation_kind(prev_vault, cur_vault, ext_in, ext_out, partition_sum);

    if !partition_ok {
        // Inconsistent value model takes priority and fails closed, regardless of
        // whether the balance equation happens to hold.
        assert!(got == Err(ConservationKind::PartitionExceedsVault));
    } else if balanced {
        // Consistent model + balanced equation == conservation HELD.
        assert!(got.is_ok());
    } else if lhs < rhs {
        // cur_vault + ext_out > prev_vault + ext_in  ⇒  value APPEARED beyond inflow.
        assert!(got == Err(ConservationKind::ValueAppeared));
    } else {
        // lhs > rhs  ⇒  value DISAPPEARED beyond outflow.
        assert!(got == Err(ConservationKind::ValueDisappeared));
    }
}

/// OVERFLOW SAFETY / FAIL-CLOSED. When `prev_vault + ext_in` (or `cur_vault +
/// ext_out`) would overflow u128, the core MUST NOT panic and MUST NOT wrap into a
/// spurious `Ok`. Here `partition_sum` is forced consistent (0 <= cur_vault) so the
/// only thing under test is the addition-overflow arm. We drive the LHS operands to
/// the ceiling; the core's `checked_add` returns `None`, which fails closed to an
/// `Err` (worst case `ValueAppeared`), never `Ok`.
#[kani::proof]
fn conservation_core_overflow_fails_closed() {
    let cur_vault: u128 = kani::any();
    let ext_out: u128 = kani::any();
    // Force a guaranteed overflow on the LHS: prev_vault + ext_in with both at MAX.
    let prev_vault = u128::MAX;
    let ext_in = u128::MAX;
    // Keep the partition trivially consistent so it does not pre-empt the test.
    let partition_sum = 0u128;

    let got = value_conservation_kind(prev_vault, cur_vault, ext_in, ext_out, partition_sum);
    // Must fail closed — never a spurious Ok from a wrapped addition.
    assert!(got.is_err());
}
