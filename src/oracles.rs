//! Pure invariant checkers.
//!
//! # O1 — source-domain realizability
//!
//! This oracle mirrors, field-for-field, the Percolator engine's OWN per-account
//! source-domain validator. The relationships below are NOT invented here: they
//! are exactly the checks the engine itself runs in
//! `SourceCreditLienAggregateProofV16::validate()`
//! (`percolator-ref/src/v16.rs:3060-3100`, SHA `71c9032`), which is invoked on
//! every account by `MarketGroupV16View::validate_source_credit_shape_with_market`
//! (`percolator-ref/src/v16.rs:2143-2253`, call site `:2207`). Those checks read
//! EXACTLY the per-account [`PortfolioSourceDomainV16Account`] fields the driver
//! surfaces in [`DomainObs`], so the oracle needs no engine-side state.
//!
//! Normatively, this enforces spec.md (v16.8.5) Requirement #2 *Source-domain
//! realizability cap* — "positive PnL from a leg is usable only [...] up to that
//! source domain's realizable counterparty backing" (spec.md:35) — together with
//! the well-formedness conditions #6, #16, #17 that make that cap meaningful.
//!
//! ## The exact inequalities (claim/lien field ⟂ reserved/backing field)
//!
//! With `SCALE = BOUND_SCALE = 1e12` (`percolator-ref/src/lib.rs:25`):
//!
//! Let `face       = source_claim_liened_num`            (scaled face claim locked)
//!     `cp_face     = source_claim_counterparty_liened_num`
//!     `ins_face    = source_claim_insurance_liened_num`
//!     `bound       = source_claim_bound_num`
//!     `imp_face    = source_claim_impaired_num`
//!     `eff         = source_lien_effective_reserved`     (unscaled atoms)
//!     `cp_back     = source_lien_counterparty_backing_num` (scaled backing)
//!     `ins_back    = source_lien_insurance_backing_num`
//!     `imp_eff     = source_lien_impaired_effective_reserved`
//!
//! The engine guarantees, and this oracle re-checks:
//!
//! 1. `cp_face + ins_face == face`                         (v16.rs:3061-3067)
//! 2. `face + imp_face <= bound`                           (v16.rs:3068-3074)
//! 3. `eff <= ceil(face / SCALE)`  ── the realizability cap (v16.rs:3075-3079)
//! 4. `cp_back % SCALE == 0 && ins_back % SCALE == 0`      (v16.rs:3080-3084)
//! 5. `cp_back + ins_back == eff * SCALE`                  (v16.rs:3085-3095)
//! 6. `imp_eff != 0  ==>  imp_face != 0`                   (v16.rs:3096-3098)
//!
//! NOT covered here: the outer bound `bound <= positive_claim_bound_num`
//! (`v16.rs:2186`) compares against MARKET-ENGINE `SourceCreditStateV16`
//! (`Market::engine.source_credit_long/short`), which is not part of the
//! per-account observation; it is out of scope for the per-account oracle.
//!
//! ## Fail-closed
//!
//! The oracle is pure and uses checked/wide arithmetic. If a quantity needed to
//! *clear* a check cannot be computed exactly (e.g. `eff * SCALE` overflows
//! u128), the oracle returns the WORST case — a violation — so it can never
//! UNDERSTATE a breach. The companion Kani proof `realizability_is_sound` proves
//! that, within the engine's documented operating range,
//! [`realizability_kind`]`(&d).is_ok()` implies all six exact inequalities hold
//! (no false negatives).
//!
//! ## Allocation-free core + reporting wrapper
//!
//! The arithmetic core [`realizability_kind`] returns a `Copy`
//! [`ViolationKind`] and performs NO heap allocation or string formatting — this
//! is what the Kani proof reasons about (modelling `u128`-to-string `format!`
//! exhausts the model checker's memory). The public [`realizability`] wraps it,
//! constructing a human-readable [`Violation`] only on the error path.

use crate::driver::{DomainObs, Observation};

/// `BOUND_SCALE` / `CREDIT_RATE_SCALE` from `percolator-ref/src/lib.rs:25-26`.
pub const SCALE: u128 = 1_000_000_000_000;

/// Which engine-asserted source-domain relationship was broken. `Copy` and
/// allocation-free so the soundness proof can reason about the check without
/// modelling string formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViolationKind {
    /// (1) `cp_face + ins_face != face` — backing-face decomposition. (R2)
    FaceDecomposition,
    /// (2) `face + imp_face > bound` — locked claim exceeds the claim bound. (R17)
    LockedExceedsBound,
    /// (3) `eff > ceil(face / SCALE)` — the realizability cap. (R2)
    EffectiveExceedsBacking,
    /// (4) backing-num not a multiple of `SCALE`. (R2)
    BackingNotAtomAligned,
    /// (5) `cp_back + ins_back != eff * SCALE` — reservation inexact. (R16)
    ReservationInexact,
    /// (6) `imp_eff != 0 && imp_face == 0` — impaired reserve without face. (R16)
    ImpairedReserveWithoutFace,
}

impl ViolationKind {
    /// The spec requirement id whose realizability guarantee this breaks.
    pub fn requirement(self) -> &'static str {
        match self {
            ViolationKind::FaceDecomposition
            | ViolationKind::EffectiveExceedsBacking
            | ViolationKind::BackingNotAtomAligned => "R2",
            ViolationKind::LockedExceedsBound => "R17",
            ViolationKind::ReservationInexact | ViolationKind::ImpairedReserveWithoutFace => "R16",
        }
    }
}

/// A detected breach of an engine-asserted source-domain relationship.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// The spec requirement id whose realizability guarantee was broken
    /// (e.g. `"R2"`).
    pub requirement: &'static str,
    /// Human-readable detail, including which account/domain when known.
    pub detail: String,
}

/// `ceil(bound_num / SCALE)` — the engine's `V16Core::amount_from_bound_num`
/// (`percolator-ref/src/v16.rs:324-332`). Exact for all u128 (no overflow:
/// `whole < n`, and `whole + 1 <= n` whenever `rem != 0`, since `rem != 0`
/// implies `whole < u128::MAX`).
#[inline]
fn amount_from_bound_num(bound_num: u128) -> u128 {
    let whole = bound_num / SCALE;
    if bound_num.is_multiple_of(SCALE) {
        whole
    } else {
        // whole < n <= u128::MAX so whole + 1 never overflows.
        whole + 1
    }
}

/// O1 arithmetic core: allocation-free, pure, fail-closed. Returns `Ok(())` for
/// a realizable domain, else the first [`ViolationKind`] (checks run in the
/// engine's own order). This is the function the Kani soundness proof verifies.
///
/// Mirrors `SourceCreditLienAggregateProofV16::validate()`
/// (`percolator-ref/src/v16.rs:3060-3100`) over the observed per-account fields.
pub fn realizability_kind(d: &DomainObs) -> Result<(), ViolationKind> {
    let face = d.source_claim_liened_num;
    let cp_face = d.source_claim_counterparty_liened_num;
    let ins_face = d.source_claim_insurance_liened_num;
    let bound = d.source_claim_bound_num;
    let imp_face = d.source_claim_impaired_num;
    let eff = d.source_lien_effective_reserved;
    let cp_back = d.source_lien_counterparty_backing_num;
    let ins_back = d.source_lien_insurance_backing_num;
    let imp_eff = d.source_lien_impaired_effective_reserved;

    // (1) backing-face decomposition: cp_face + ins_face == face. (v16.rs:3061)
    // Fail-closed: an overflow in the sum cannot equal a finite `face`, so it is
    // a violation regardless.
    match cp_face.checked_add(ins_face) {
        Some(backing_face) if backing_face == face => {}
        _ => return Err(ViolationKind::FaceDecomposition),
    }

    // (2) locked + impaired face must fit under the claim bound. (v16.rs:3068)
    // Fail-closed: if the sum overflows it certainly cannot be <= bound.
    match face.checked_add(imp_face) {
        Some(locked_or_impaired) if locked_or_impaired <= bound => {}
        _ => return Err(ViolationKind::LockedExceedsBound),
    }

    // (3) the realizability cap: effective reserved <= ceil(face / SCALE).
    // (v16.rs:3075) — Requirement #2.
    if eff > amount_from_bound_num(face) {
        return Err(ViolationKind::EffectiveExceedsBacking);
    }

    // (4) backing-num atom alignment. (v16.rs:3080)
    if !cp_back.is_multiple_of(SCALE) || !ins_back.is_multiple_of(SCALE) {
        return Err(ViolationKind::BackingNotAtomAligned);
    }

    // (5) reservation exactness: cp_back + ins_back == eff * SCALE. (v16.rs:3085)
    // Fail-closed via checked arithmetic on BOTH sides: if either side cannot be
    // computed exactly in u128 we cannot certify equality, so we report a
    // violation (worst case) rather than risk a false clear.
    let lien_backing_num = match cp_back.checked_add(ins_back) {
        Some(v) => v,
        None => return Err(ViolationKind::ReservationInexact),
    };
    let expected_backing_num = match eff.checked_mul(SCALE) {
        Some(v) => v,
        None => return Err(ViolationKind::ReservationInexact),
    };
    if lien_backing_num != expected_backing_num {
        return Err(ViolationKind::ReservationInexact);
    }

    // (6) impaired reserve well-formedness. (v16.rs:3096)
    if imp_eff != 0 && imp_face == 0 {
        return Err(ViolationKind::ImpairedReserveWithoutFace);
    }

    Ok(())
}

/// O1: check that a single observed source domain is realizable, i.e. its liened
/// positive claim does not exceed its reserved realizable backing, and the lien
/// bookkeeping is well-formed — exactly the engine's own
/// `SourceCreditLienAggregateProofV16::validate()` over the observed fields.
///
/// Pure and fail-closed; wraps [`realizability_kind`], adding a human-readable
/// detail string only on the error path.
pub fn realizability(d: &DomainObs) -> Result<(), Violation> {
    realizability_kind(d).map_err(|kind| Violation {
        requirement: kind.requirement(),
        detail: describe(kind, d),
    })
}

/// Build the human-readable detail for a [`ViolationKind`] over `d`. Only ever
/// called on the error path, so its `format!` allocations stay off the
/// model-checked core.
fn describe(kind: ViolationKind, d: &DomainObs) -> String {
    match kind {
        ViolationKind::FaceDecomposition => format!(
            "counterparty_face({}) + insurance_face({}) != face_locked({})",
            d.source_claim_counterparty_liened_num,
            d.source_claim_insurance_liened_num,
            d.source_claim_liened_num
        ),
        ViolationKind::LockedExceedsBound => format!(
            "face_locked({}) + impaired_face({}) exceeds claim_bound({})",
            d.source_claim_liened_num, d.source_claim_impaired_num, d.source_claim_bound_num
        ),
        ViolationKind::EffectiveExceedsBacking => format!(
            "effective_reserved({}) exceeds realizable backing ceil(face/SCALE)={}",
            d.source_lien_effective_reserved,
            amount_from_bound_num(d.source_claim_liened_num)
        ),
        ViolationKind::BackingNotAtomAligned => format!(
            "backing-num not atom-aligned: counterparty({}), insurance({})",
            d.source_lien_counterparty_backing_num, d.source_lien_insurance_backing_num
        ),
        ViolationKind::ReservationInexact => format!(
            "reserved backing-num(counterparty {} + insurance {}) != effective_reserved({})*SCALE",
            d.source_lien_counterparty_backing_num,
            d.source_lien_insurance_backing_num,
            d.source_lien_effective_reserved
        ),
        ViolationKind::ImpairedReserveWithoutFace => format!(
            "impaired_effective_reserved({}) != 0 but impaired_face == 0",
            d.source_lien_impaired_effective_reserved
        ),
    }
}

/// Run [`realizability`] across every account and every source domain of an
/// [`Observation`], annotating the offending account/domain in `detail`.
pub fn check_observation(obs: &Observation) -> Result<(), Violation> {
    for (ai, acct) in obs.accounts.iter().enumerate() {
        for (di, dom) in acct.domains.iter().enumerate() {
            if let Err(kind) = realizability_kind(dom) {
                return Err(Violation {
                    requirement: kind.requirement(),
                    detail: format!("account {ai}, domain {di}: {}", describe(kind, dom)),
                });
            }
        }
    }
    Ok(())
}
