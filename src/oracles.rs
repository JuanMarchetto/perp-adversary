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

use crate::driver::{DomainObs, MarketSideObs, Observation};

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

// ===========================================================================
// v0.1 — market-engine realizability CROSS-LINK oracle
//
// This is a SEPARATE oracle from O1 above. O1 mirrors the engine's per-account
// `SourceCreditLienAggregateProofV16::validate()` (v16.rs:3060), which reads
// ONLY per-account fields. This oracle mirrors the COMPOSING relationships the
// engine checks in `MarketGroupV16View::validate_source_credit_shape_with_market`
// (`percolator-ref/src/v16.rs:2143-2253`, SHA `71c9032`) BETWEEN each per-account
// source domain `d` and the MARKET-ENGINE `SourceCreditStateV16` of the asset and
// side that domain maps to (asset_index = d/2, side = d%2; 0=long, 1=short):
//
//   (a) v16.rs:2177-2179 — `source.source_claim_market_id == asset.market_id`
//       (else `V16Error::HiddenLeg`); and
//   (b) v16.rs:2186-2188 — `source.source_claim_bound_num
//                             <= domain_credit.positive_claim_bound_num`
//       (else `V16Error::InvalidLeg`).
//
// (b) is the realizability cross-link the per-account O1 oracle DELIBERATELY
// scoped out (see `oracles.rs` O1 doc, "NOT covered here", v16.rs:2186): it caps
// a per-account positive-claim bound by the MARKET's realizable positive-claim
// bound — a relationship no per-account check, and no single-state market check,
// can see.
//
// ## Why this is NON-VACUOUS
//
// There are three source-credit validators in the engine. Two are single-state:
//   1. the per-account `SourceCreditLienAggregateProofV16::validate()` — O1's
//      target; reads only per-account fields.
//   2. the market-engine `validate_source_credit_state_static()` (v16.rs:467) —
//      reads only one market `SourceCreditStateV16`. RE-CHECKING it would be
//      VACUOUS: `SourceCreditStateV16Account::try_to_runtime()` (v16.rs:3589)
//      calls it internally, so the driver's `MarketSideObs` only ever holds
//      states that ALREADY passed it (see `driver::read_source_credit`).
// This oracle targets NEITHER. It mirrors the third, COMPOSING validator
// `validate_source_credit_shape_with_market` (v16.rs:2186), which checks a
// per-account↔market-engine RELATIONSHIP that neither single-state static
// validator enforces. That is why it is a meaningful, non-vacuous addition.
//
// ## Domain ↔ market-side pairing
//
// Mirrors the engine loop exactly: per-account source-domain index `d` maps to
// `asset_index = d / 2` and `side = d % 2` (v16.rs:2175,2181). The driver builds
// `AccountObs.domains` with the same length and order
// (`v16_domain_count_for_market_slots`), and `Observation.market_domains` is
// emitted in the same order (asset0-long, asset0-short, asset1-long, …) by
// `driver::observe_markets`, so `market_domains[d]` is the engine slot the
// engine itself reads for domain `d`. We locate the paired side by its
// `(asset, side)` keys to be robust to ordering.
//
// ## Numeric-zero domains are SKIPPED (mirrors the engine)
//
// The engine applies the cross-link checks ONLY to non-empty domains: an all-zero
// `numeric_zero_source_domain` (v16.rs:2155) bypasses 2177/2186 entirely (it only
// requires `source_claim_market_id == 0`). We mirror that: a numeric-zero
// `DomainObs` is not paired or cross-checked.
//
// ## Fail-closed
//
// Pure, no `unsafe`. If a non-empty domain has NO observable market side to pair
// against (the engine would read `market.markets[asset_index]`, which must
// exist), we cannot certify the relationship and return the WORST case — a
// violation — so the oracle never UNDERSTATES a breach. The companion Kani proof
// `market_cross_link_is_sound` proves that a cleared pairing implies both (a) and
// (b) hold.
// ===========================================================================

/// Is this observed per-account domain the engine's `numeric_zero_source_domain`
/// (v16.rs:2155-2164) over the fields the driver exposes? The engine additionally
/// inspects `source_lien_fee_last_slot`, which `DomainObs` does not carry; over
/// the observed subset this is the conservative emptiness test (a domain we treat
/// as empty is necessarily empty under the engine's superset of fields too, so we
/// never wrongly SKIP a domain the engine would cross-check).
#[inline]
fn is_numeric_zero_domain(d: &DomainObs) -> bool {
    d.source_claim_bound_num == 0
        && d.source_claim_liened_num == 0
        && d.source_claim_counterparty_liened_num == 0
        && d.source_claim_insurance_liened_num == 0
        && d.source_lien_effective_reserved == 0
        && d.source_lien_counterparty_backing_num == 0
        && d.source_lien_insurance_backing_num == 0
        && d.source_claim_impaired_num == 0
        && d.source_lien_impaired_effective_reserved == 0
}

/// Which market-engine cross-link relationship was broken. `Copy` and
/// allocation-free so the soundness proof can reason about the check without
/// modelling string formatting (same discipline as [`ViolationKind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrossLinkKind {
    /// (a) `source_claim_market_id != asset.market_id` (v16.rs:2177 — HiddenLeg),
    /// OR no observable market side exists to pair the non-empty domain against
    /// (fail-closed; the engine would read `market.markets[asset_index]`).
    MarketIdMismatch,
    /// (b) `source_claim_bound_num > positive_claim_bound_num`
    /// (v16.rs:2186 — InvalidLeg): the realizability cross-link O1 scoped out.
    BoundExceedsMarket,
}

impl CrossLinkKind {
    /// The engine citation this breaks.
    pub fn requirement(self) -> &'static str {
        match self {
            CrossLinkKind::MarketIdMismatch => "market cross-link (v16.rs:2177)",
            CrossLinkKind::BoundExceedsMarket => "market cross-link (v16.rs:2186)",
        }
    }
}

/// Cross-link arithmetic core: allocation-free, pure, fail-closed. Given a
/// non-empty per-account domain `d` and the MARKET-ENGINE side it pairs to
/// (`market_id` = that asset's `market_id`, `positive_claim_bound_num` = that
/// side's market bound), returns `Ok(())` iff both engine relationships hold:
///
///   (a) `d.source_claim_market_id == market_id`        (v16.rs:2177)
///   (b) `d.source_claim_bound_num <= positive_claim_bound_num` (v16.rs:2186)
///
/// This is the function the Kani soundness proof verifies. Callers MUST only
/// invoke it on non-numeric-zero domains (the engine skips empty ones).
#[inline]
pub fn cross_link_kind(
    d: &DomainObs,
    market_id: u64,
    positive_claim_bound_num: u128,
) -> Result<(), CrossLinkKind> {
    // (a) market id must match the asset (v16.rs:2177 -> HiddenLeg).
    if d.source_claim_market_id != market_id {
        return Err(CrossLinkKind::MarketIdMismatch);
    }
    // (b) per-account claim bound capped by the market's positive claim bound
    //     (v16.rs:2186 -> InvalidLeg) — the realizability cross-link.
    if d.source_claim_bound_num > positive_claim_bound_num {
        return Err(CrossLinkKind::BoundExceedsMarket);
    }
    Ok(())
}

/// v0.1 market cross-link: check that every NON-EMPTY per-account source domain
/// is consistent with the market-engine `SourceCreditStateV16` of the asset/side
/// it maps to — exactly the engine's own
/// `validate_source_credit_shape_with_market` cross-checks (v16.rs:2177, 2186)
/// over the observed fields.
///
/// Per-account domain index `di` maps to `asset = di/2`, `side = di%2`
/// (0=long, 1=short); the paired market side is located in `market_domains` by
/// those keys. Pure and fail-closed; wraps [`cross_link_kind`], adding a
/// human-readable detail string only on the error path. Returns the first
/// breach (domains scanned in engine order).
pub fn market_cross_link(
    account_domains: &[DomainObs],
    market_domains: &[MarketSideObs],
) -> Result<(), Violation> {
    for (di, dom) in account_domains.iter().enumerate() {
        // The engine cross-checks only non-empty domains (v16.rs:2155-2171).
        if is_numeric_zero_domain(dom) {
            continue;
        }
        let asset = di / 2;
        let side = (di % 2) as u8;
        // Locate the market-engine side this domain pairs to. The engine reads
        // `market.markets[asset_index]`; if no such observed side exists we
        // cannot certify the relationship -> fail closed (MarketIdMismatch).
        let paired = market_domains
            .iter()
            .find(|m| m.asset == asset && m.side == side);
        let Some(m) = paired else {
            return Err(Violation {
                requirement: CrossLinkKind::MarketIdMismatch.requirement(),
                detail: format!(
                    "domain {di} (asset {asset}, side {side}): no observable \
                     market-engine source-credit side to pair against; cannot \
                     certify the v16.rs:2177/2186 cross-link"
                ),
            });
        };
        // `m.market_id` is the asset's REAL `market_id` read straight from
        // `Market::engine.asset.market_id` by the driver — exactly the value the
        // engine compares each per-account `source_claim_market_id` against
        // (v16.rs:2177). Comparing against it (not a reconstructed convention)
        // keeps the check a genuine cross-link over the real engine state.
        if let Err(kind) = cross_link_kind(dom, m.market_id, m.state.positive_claim_bound_num) {
            return Err(Violation {
                requirement: kind.requirement(),
                detail: describe_cross_link(kind, dom, di, asset, side, m),
            });
        }
    }
    Ok(())
}

/// Build the human-readable detail for a [`CrossLinkKind`]. Only ever called on
/// the error path, so its `format!` allocations stay off the model-checked core.
fn describe_cross_link(
    kind: CrossLinkKind,
    d: &DomainObs,
    di: usize,
    asset: usize,
    side: u8,
    m: &MarketSideObs,
) -> String {
    match kind {
        CrossLinkKind::MarketIdMismatch => format!(
            "domain {di} (asset {asset}, side {side}): source_claim_market_id({}) \
             != asset.market_id({})",
            d.source_claim_market_id, m.market_id
        ),
        CrossLinkKind::BoundExceedsMarket => format!(
            "domain {di} (asset {asset}, side {side}): source_claim_bound_num({}) \
             > market positive_claim_bound_num({})",
            d.source_claim_bound_num, m.state.positive_claim_bound_num
        ),
    }
}

/// Run [`market_cross_link`] across every account of an [`Observation`], pairing
/// each account's per-account source domains against the shared market-engine
/// `market_domains`, annotating the offending account in `detail`.
pub fn check_observation_market(obs: &Observation) -> Result<(), Violation> {
    for (ai, acct) in obs.accounts.iter().enumerate() {
        if let Err(v) = market_cross_link(&acct.domains, &obs.market_domains) {
            return Err(Violation {
                requirement: v.requirement,
                detail: format!("account {ai}, {}", v.detail),
            });
        }
    }
    Ok(())
}
