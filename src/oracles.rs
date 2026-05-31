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

use crate::driver::{AccountObs, AdlObs, DomainObs, MarketSideObs, Observation, SystemObs};
use crate::scenario::Action;

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

// ===========================================================================
// v0.2 — liquidation insurance-domain ISOLATION oracle
//
// This is a CROSS-STEP (delta) oracle, distinct from O1 and the v0.1 market
// cross-link, both of which are single-state. It mirrors the engine's guarantee
// about how a liquidation may spend SHARED insurance:
//
//   When `liquidate_account_not_atomic` (`percolator-ref/src/v16.rs:9829`, SHA
//   `71c9032`) settles a bankrupt leg, it calls
//   `consume_domain_insurance_for_negative_pnl(asset_index, leg.side, ..)`
//   (`v16.rs:9923`). That function (`v16.rs:5949`) computes the bankruptcy
//   insurance domain as `insurance_domain_index(asset_index,
//   opposite_side(leg.side))` (`v16.rs:5955`) — a domain of the SAME asset — and
//   increments ONLY that domain's `insurance_domain_spent` by exactly the `used`
//   amount it returns (`v16.rs:5974-5980`), which is the `insurance_used` in the
//   `LiquidationOutcomeV16`. No OTHER domain's `insurance_domain_spent` is ever
//   touched by a liquidation.
//
// So across one liquidation step the engine guarantees, over the per-side
// `insurance_domain_spent` (`EngineAssetSlotV16Account::insurance_domain_spent_*`,
// `v16.rs:3826`) the driver surfaces in `MarketSideObs.insurance_domain_spent`:
//
//   (I1) every side's spend is MONOTONE: `cur.spent >= prev.spent`            ;
//   (I2) ISOLATION: only domains of the LIQUIDATED asset may show an increase
//        (no cross-asset / cross-domain drain)                                ;
//   (I3) FULL ACCOUNTING: the total spend increase across ALL observed domains
//        equals the outcome's `insurance_used`.
//
// (I2)+(I3) together imply the liquidated asset's domain delta == insurance_used
// and every other domain delta == 0 — exactly the engine's per-domain spend
// guarantee, as asserted by its OWN conformance test
// (`tests/v16_spec_tests.rs:342-348`: `insurance_used == 0`,
// `insurance_domain_spent_short == 0`) and bounded by its Kani proofs
// (`tests/proofs_v16.rs:2375` domain budget caps the spend; `:2410` reserved
// insurance cannot be double-spent; `:2449` an unfunded domain cannot drain
// shared insurance).
//
// ## Why this is NON-VACUOUS (the trap this class fell into twice)
//
// After a liquidation the driver runs `validate_shape` + `validate_with_market`,
// so the post-liquidation state has ALREADY passed every single-state validator.
// Re-checking any single state is therefore VACUOUS. The value here is the DELTA:
// `insurance_domain_spent` is a running total; only by comparing its value BEFORE
// and AFTER the liquidation, against the step's `insurance_used`, can isolation be
// observed. No single observed state encodes "this step spent insurance only on
// the liquidated domain". The `funded_liquidation_campaign` reaches an
// engine-accepted state where `insurance_used > 0` and exactly one domain's spend
// rises, so the oracle is exercised non-trivially (see `tests/liquidation.rs`).
//
// ## Fail-closed
//
// Pure, no `unsafe`. If a side observed in `cur` has no matching side in `prev`
// (we cannot certify its delta), or any side's spend went BACKWARD, or the
// accounting does not balance, the oracle returns the WORST case — a violation —
// so it never UNDERSTATES a breach.
// ===========================================================================

/// Which liquidation insurance-isolation relationship was broken. `Copy` and
/// allocation-free so the soundness proof can reason about the check without
/// modelling string formatting (same discipline as [`ViolationKind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolationKind {
    /// (I1) a market side's `insurance_domain_spent` DECREASED across the step,
    /// or a side present in `cur` is absent from `prev` so its delta cannot be
    /// certified (fail-closed). The engine only ever increments spend.
    SpendNotMonotone,
    /// (I2) a domain NOT belonging to the liquidated asset showed a positive
    /// `insurance_domain_spent` increase — a cross-domain insurance drain.
    CrossDomainDrain,
    /// (I3) the total `insurance_domain_spent` increase across all observed
    /// domains does not equal the outcome's `insurance_used` (insurance spent is
    /// unaccounted, or claimed without an observed domain to back it).
    InsuranceSpendUnaccounted,
}

impl IsolationKind {
    /// The engine citation this breaks.
    pub fn requirement(self) -> &'static str {
        match self {
            IsolationKind::SpendNotMonotone => "insurance isolation (v16.rs:5974)",
            IsolationKind::CrossDomainDrain => "insurance isolation (v16.rs:5955)",
            IsolationKind::InsuranceSpendUnaccounted => "insurance isolation (v16.rs:5980)",
        }
    }
}

/// Locate the market side `(asset, side)` in `sides`, returning its
/// `insurance_domain_spent`. The driver emits all observed sides in
/// `Observation.market_domains`; pairing by key (not index) is robust to ordering.
#[inline]
fn spent_of(sides: &[MarketSideObs], asset: usize, side: u8) -> Option<u128> {
    sides
        .iter()
        .find(|m| m.asset == asset && m.side == side)
        .map(|m| m.insurance_domain_spent)
}

/// Liquidation insurance-isolation arithmetic core: allocation-free, pure,
/// fail-closed. Given the pre-step market sides `prev`, the post-step market sides
/// `cur`, the `liquidated_asset` (from the step's `Liquidate` action), and the
/// outcome's `insurance_used`, returns `Ok(())` iff invariants (I1)-(I3) above
/// hold, else the first [`IsolationKind`] together with the offending side's
/// `(asset, side)` for reporting.
///
/// This is the function the Kani soundness proof verifies. It iterates `cur`'s
/// sides (the engine's observable domains) and pairs each against `prev` by key.
#[inline]
pub fn isolation_kind(
    prev: &[MarketSideObs],
    cur: &[MarketSideObs],
    liquidated_asset: usize,
    insurance_used: u128,
) -> Result<(), (IsolationKind, usize, u8)> {
    let mut total_increase: u128 = 0;
    for m in cur {
        // Pair against the pre-step baseline for THIS side. A missing baseline
        // cannot be certified -> fail closed (I1).
        let before = match spent_of(prev, m.asset, m.side) {
            Some(v) => v,
            None => return Err((IsolationKind::SpendNotMonotone, m.asset, m.side)),
        };
        let after = m.insurance_domain_spent;
        // (I1) monotonicity: spend never decreases. A backward move is fail-closed.
        let delta = match after.checked_sub(before) {
            Some(d) => d,
            None => return Err((IsolationKind::SpendNotMonotone, m.asset, m.side)),
        };
        if delta == 0 {
            continue;
        }
        // (I2) isolation: only the liquidated asset's domains may rise.
        if m.asset != liquidated_asset {
            return Err((IsolationKind::CrossDomainDrain, m.asset, m.side));
        }
        // Accumulate the total increase (fail-closed on overflow).
        total_increase = match total_increase.checked_add(delta) {
            Some(v) => v,
            None => return Err((IsolationKind::InsuranceSpendUnaccounted, m.asset, m.side)),
        };
    }
    // (I3) full accounting: total observed spend increase == insurance_used.
    // Because (I2) already confined every increase to the liquidated asset's
    // domains, this equality also pins insurance_used to the liquidated domain's
    // delta — it can be neither MORE (claimed but unobserved) nor LESS (observed
    // but unreported) than what the outcome states.
    if total_increase != insurance_used {
        return Err((
            IsolationKind::InsuranceSpendUnaccounted,
            liquidated_asset,
            0,
        ));
    }
    Ok(())
}

/// v0.2 liquidation insurance-domain ISOLATION oracle: for a step whose
/// `cur.liquidation` is `Some`, check that the liquidation spent SHARED insurance
/// only on the liquidated asset's bankruptcy domain and that the spend is fully
/// accounted by the outcome's `insurance_used` — exactly the engine's own
/// `consume_domain_insurance_for_negative_pnl` guarantee (`v16.rs:5949-5996`),
/// observed as a DELTA across the `(prev, cur)` `insurance_domain_spent` values.
///
/// Steps that are not a `cur.liquidation == Some` liquidation are a no-op (the
/// oracle does not apply). Pure and fail-closed; wraps [`isolation_kind`], adding
/// a human-readable detail string only on the error path.
pub fn liquidation_insurance_isolation(
    prev: &Observation,
    cur: &Observation,
) -> Result<(), Violation> {
    // Only liquidation steps carry an outcome; nothing else can spend insurance
    // this way, so there is no delta to police.
    let Some(liq) = cur.liquidation.as_ref() else {
        return Ok(());
    };
    // The liquidated asset comes from the step's own action. (The engine's
    // bankruptcy domain is `insurance_domain_index(asset, opposite_side(leg.side))`
    // — always a domain of THIS asset; the oracle keys on the asset, the level at
    // which isolation is guaranteed, without needing the unobserved leg side.)
    let liquidated_asset = match cur.action {
        Action::Liquidate { asset, .. } => asset as usize,
        // A liquidation outcome with a non-Liquidate action is impossible from the
        // driver; fail closed if it ever occurs rather than certify blindly.
        _ => {
            return Err(Violation {
                requirement: IsolationKind::CrossDomainDrain.requirement(),
                detail: "liquidation outcome present on a non-Liquidate step; \
                         cannot identify the liquidated asset to certify isolation"
                    .to_string(),
            })
        }
    };

    isolation_kind(
        &prev.market_domains,
        &cur.market_domains,
        liquidated_asset,
        liq.insurance_used,
    )
    .map_err(|(kind, asset, side)| Violation {
        requirement: kind.requirement(),
        detail: describe_isolation(kind, asset, side, liquidated_asset, liq.insurance_used),
    })
}

/// Build the human-readable detail for an [`IsolationKind`]. Only ever called on
/// the error path, so its `format!` allocations stay off the model-checked core.
fn describe_isolation(
    kind: IsolationKind,
    asset: usize,
    side: u8,
    liquidated_asset: usize,
    insurance_used: u128,
) -> String {
    match kind {
        IsolationKind::SpendNotMonotone => format!(
            "asset {asset}, side {side}: insurance_domain_spent moved backward or has no \
             pre-liquidation baseline; cannot certify the spend delta (liquidated asset \
             {liquidated_asset})"
        ),
        IsolationKind::CrossDomainDrain => format!(
            "asset {asset}, side {side}: insurance_domain_spent increased while liquidating \
             asset {liquidated_asset} — a cross-domain insurance drain (v16.rs:5955 charges \
             only the liquidated asset's bankruptcy domain)"
        ),
        IsolationKind::InsuranceSpendUnaccounted => format!(
            "liquidated asset {liquidated_asset}: total insurance_domain_spent increase across \
             observed domains does not equal the outcome's insurance_used({insurance_used}) \
             (v16.rs:5980 increments the liquidated domain by exactly insurance_used)"
        ),
    }
}

/// Run [`liquidation_insurance_isolation`] over a consecutive `(prev, cur)`
/// observation pair. A thin convenience over the pair-wise oracle so a cross-step
/// runner can fold it across a trace's observations.
pub fn check_step_insurance_isolation(
    prev: &Observation,
    cur: &Observation,
) -> Result<(), Violation> {
    liquidation_insurance_isolation(prev, cur)
}

// ===========================================================================
// v0.3 — quantity-ADL EXACT-ACCOUNTING oracle
//
// This is a CROSS-STEP (delta) oracle, the same class as the v0.2 liquidation
// insurance-isolation oracle and distinct from the single-state O1 / v0.1 market
// cross-link. It mirrors the engine's guarantee about how a quantity auto-
// deleverage is booked into the account's close-progress ledger:
//
//   When `apply_quantity_adl_after_residual_for_account_not_atomic`
//   (`percolator-ref/src/v16.rs:9479`, SHA `71c9032`) deleverages a finalized-
//   close account's profitable counterparty, it
//     (1) computes `out.closed_q == close_q` of the leg
//         (`apply_quantity_adl_after_residual_internal`, `v16.rs:9676`), with
//         `close_q > 0` required (`v16.rs:9609` rejects a zero close); then
//     (2) calls `advance_close_progress_quantity_adl(account, out.closed_q)`
//         (`v16.rs:9510`), which FIRST requires the prior
//         `ledger.quantity_adl_applied_q == 0` (`v16.rs:9530`, else `LockActive`)
//         and `out.closed_q != 0` (`v16.rs:9522`, else `NonProgress`), then sets
//         `ledger.quantity_adl_applied_q = out.closed_q` (`v16.rs:9533`).
//
// So across ONE ApplyAdl step the engine guarantees, over the account's
// `close_progress.quantity_adl_applied_q` ledger value
// (`CloseProgressLedgerV16Account::quantity_adl_applied_q`, `v16.rs:11920`) the
// driver surfaces in `AccountObs.quantity_adl_applied_q`:
//
//   (E1) EXACT ACCOUNTING: the ledger's applied-ADL quantity rises by EXACTLY
//        `cur.adl.closed_q` across the step
//        (`cur_applied == prev_applied + closed_q`)                            ;
//   (E2) NON-VACUOUS CLOSE: `closed_q > 0` (a real ADL always closes a non-zero
//        quantity — the engine bound `v16.rs:9609`/`9522`/`9676`)              ;
//   (E3) LEDGER COHERENCE: the outcome's reported post-ledger value
//        (`cur.adl.quantity_adl_applied_q`, read by the driver straight from the
//        same ledger field after the engine advanced it) equals the account's
//        observed `AccountObs.quantity_adl_applied_q` — the two views of the SAME
//        engine field must coincide.
//
// (E1)+(E2) are the exact-accounting / bound guarantee asked for: the ADL is
// booked into the ledger neither MORE (over-credited) nor LESS (under-credited)
// than the quantity it actually closed, and that quantity is strictly positive.
//
// ## Why this is NON-VACUOUS (the trap this class must avoid — third time)
//
// After an ApplyAdl step the driver runs `validate_shape` +
// `validate_with_market`, so the post-ADL state has ALREADY passed every single-
// state validator; the seeded finalized-close ledger's residual equation is
// itself engine-validated. Re-checking any single state — the residual equation,
// the ledger shape, the leg shape — is therefore VACUOUS. The value here is the
// DELTA: `quantity_adl_applied_q` is a running ledger total; only by comparing its
// value BEFORE the ADL (the `SeedFinalizedClose` predecessor, where it is 0) and
// AFTER it, against the step's `closed_q`, can exact accounting be observed. No
// single observed state encodes "this step booked exactly `closed_q` of applied
// ADL into the ledger". The `adl_campaign` reaches an engine-accepted state where
// `closed_q == POS_SCALE` and the ledger advances 0 -> POS_SCALE, so the oracle is
// exercised non-trivially (see `tests/adl.rs`).
//
// ## Fail-closed
//
// Pure, no `unsafe`. If `prev` has no account at the ADL's `account` index (we
// cannot read the baseline ledger value), or the ledger moved BACKWARD, or the
// observed account ledger disagrees with the outcome's reported value, or the
// delta does not equal `closed_q`, or `closed_q == 0`, the oracle returns the
// WORST case — a violation — so it never UNDERSTATES a breach. The companion Kani
// proof `adl_accounting_is_sound` proves a cleared step implies (E1)+(E2).
// ===========================================================================

/// Which ADL exact-accounting relationship was broken. `Copy` and allocation-free
/// so the soundness proof can reason about the check without modelling string
/// formatting (same discipline as [`ViolationKind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdlAccountingKind {
    /// (E2) `closed_q == 0` — the engine never books a zero-quantity ADL
    /// (`v16.rs:9609`/`9522`); an observed outcome claiming one is fail-closed.
    ClosedQZero,
    /// (E3) the account's observed `quantity_adl_applied_q` ledger value disagrees
    /// with the ADL outcome's reported post-ledger value — two views of the SAME
    /// engine field that must coincide (fail-closed).
    LedgerOutcomeMismatch,
    /// (E1) the ledger's `quantity_adl_applied_q` did NOT rise by exactly
    /// `closed_q` across the step (it moved backward, or the delta over/under the
    /// closed quantity): the ADL is mis-accounted in the ledger.
    AppliedDeltaNotClosedQ,
    /// `prev` has no account at the ADL's account index, so the baseline ledger
    /// value cannot be read and the delta cannot be certified (fail-closed).
    MissingPrevAccount,
}

impl AdlAccountingKind {
    /// The engine citation this breaks.
    pub fn requirement(self) -> &'static str {
        match self {
            AdlAccountingKind::ClosedQZero => "ADL accounting (v16.rs:9609)",
            AdlAccountingKind::LedgerOutcomeMismatch => "ADL accounting (v16.rs:9533)",
            AdlAccountingKind::AppliedDeltaNotClosedQ => "ADL accounting (v16.rs:9533)",
            AdlAccountingKind::MissingPrevAccount => "ADL accounting (v16.rs:9530)",
        }
    }
}

/// ADL exact-accounting arithmetic core: allocation-free, pure, fail-closed. Given
/// the pre-step baseline ledger value `prev_applied_q` (the account's
/// `quantity_adl_applied_q` BEFORE the ADL), the post-step account ledger value
/// `cur_applied_q` (AFTER), and the ADL outcome `adl`, returns `Ok(())` iff
/// invariants (E1)-(E3) above hold, else the first [`AdlAccountingKind`].
///
/// This is the function the Kani soundness proof verifies. It reasons only about
/// the three `u128` ledger/outcome quantities — no `format!`, no allocation —
/// exactly as the O1 / cross-link / isolation cores do.
#[inline]
pub fn adl_accounting_kind(
    prev_applied_q: u128,
    cur_applied_q: u128,
    adl: &AdlObs,
) -> Result<(), AdlAccountingKind> {
    // (E2) a real ADL always closes a strictly positive quantity (v16.rs:9609 /
    // 9522 reject a zero close); fail closed on a zero-close outcome.
    if adl.closed_q == 0 {
        return Err(AdlAccountingKind::ClosedQZero);
    }
    // (E3) the account's observed ledger value and the outcome's reported
    // post-ledger value are the SAME engine field (v16.rs:9533 set it, the driver
    // read it twice); they must coincide. Fail closed otherwise.
    if cur_applied_q != adl.quantity_adl_applied_q {
        return Err(AdlAccountingKind::LedgerOutcomeMismatch);
    }
    // (E1) EXACT ACCOUNTING: the ledger rose by EXACTLY closed_q across the step.
    // Fail-closed via checked arithmetic: a backward move (overflow on the
    // subtraction) or a delta != closed_q is a violation. We compare the delta
    // rather than `prev + closed_q` so an overflow in the addition cannot mask a
    // breach.
    match cur_applied_q.checked_sub(prev_applied_q) {
        Some(delta) if delta == adl.closed_q => {}
        _ => return Err(AdlAccountingKind::AppliedDeltaNotClosedQ),
    }
    Ok(())
}

/// v0.3 quantity-ADL EXACT-ACCOUNTING oracle: for a step whose `cur.adl` is `Some`,
/// check that the account's `close_progress.quantity_adl_applied_q` ledger value
/// rose by EXACTLY the outcome's `closed_q` (a strictly positive quantity) across
/// the ApplyAdl step — exactly the engine's own
/// `advance_close_progress_quantity_adl` guarantee (`v16.rs:9510-9536`), observed
/// as a DELTA across the `(prev, cur)` `quantity_adl_applied_q` values.
///
/// The acted-on account comes from the step's own `ApplyAdl` action; its baseline
/// ledger value is read from the matching account in `prev`. Steps that are not a
/// `cur.adl == Some` ADL are a no-op (the oracle does not apply). Pure and
/// fail-closed; wraps [`adl_accounting_kind`], adding a human-readable detail
/// string only on the error path.
pub fn adl_accounting(prev: &Observation, cur: &Observation) -> Result<(), Violation> {
    // Only ApplyAdl steps carry an ADL outcome; nothing else advances the ledger
    // this way, so there is no delta to police.
    let Some(adl) = cur.adl.as_ref() else {
        return Ok(());
    };
    // The acted-on account comes from the step's own action. (An ADL outcome with a
    // non-ApplyAdl action is impossible from the driver; fail closed if it ever
    // occurs rather than certify blindly.)
    let account = match cur.action {
        Action::ApplyAdl { account, .. } => account as usize,
        _ => {
            return Err(Violation {
                requirement: AdlAccountingKind::AppliedDeltaNotClosedQ.requirement(),
                detail: "ADL outcome present on a non-ApplyAdl step; cannot identify the \
                         acted-on account to certify exact accounting"
                    .to_string(),
            })
        }
    };

    // Read the pre-step baseline ledger value for THIS account. A missing baseline
    // cannot be certified -> fail closed.
    let prev_applied_q = match account_ledger(prev, account) {
        Some(v) => v,
        None => {
            return Err(Violation {
                requirement: AdlAccountingKind::MissingPrevAccount.requirement(),
                detail: format!(
                    "account {account}: no pre-ADL observation of its \
                     quantity_adl_applied_q baseline; cannot certify the exact-accounting delta"
                ),
            })
        }
    };
    // The post-step ledger value for the same account; absent -> fail closed too.
    let cur_applied_q = match account_ledger(cur, account) {
        Some(v) => v,
        None => {
            return Err(Violation {
                requirement: AdlAccountingKind::MissingPrevAccount.requirement(),
                detail: format!(
                    "account {account}: no post-ADL observation of its \
                     quantity_adl_applied_q; cannot certify the exact-accounting delta"
                ),
            })
        }
    };

    adl_accounting_kind(prev_applied_q, cur_applied_q, adl).map_err(|kind| Violation {
        requirement: kind.requirement(),
        detail: describe_adl(kind, account, prev_applied_q, cur_applied_q, adl),
    })
}

/// The account's observed `quantity_adl_applied_q` ledger value at `index`, or
/// `None` if no such account exists in the observation (fail-closed signal).
#[inline]
fn account_ledger(obs: &Observation, index: usize) -> Option<u128> {
    obs.accounts
        .get(index)
        .map(|a: &AccountObs| a.quantity_adl_applied_q)
}

/// Build the human-readable detail for an [`AdlAccountingKind`]. Only ever called
/// on the error path, so its `format!` allocations stay off the model-checked core.
fn describe_adl(
    kind: AdlAccountingKind,
    account: usize,
    prev_applied_q: u128,
    cur_applied_q: u128,
    adl: &AdlObs,
) -> String {
    match kind {
        AdlAccountingKind::ClosedQZero => format!(
            "account {account}: ADL outcome reports closed_q == 0, but a real ADL closes a \
             non-zero quantity (v16.rs:9609/9522)"
        ),
        AdlAccountingKind::LedgerOutcomeMismatch => format!(
            "account {account}: observed quantity_adl_applied_q({cur_applied_q}) disagrees with \
             the ADL outcome's reported post-ledger value({}) — two views of the same engine \
             field must coincide (v16.rs:9533)",
            adl.quantity_adl_applied_q
        ),
        AdlAccountingKind::AppliedDeltaNotClosedQ => format!(
            "account {account}: quantity_adl_applied_q moved {prev_applied_q} -> {cur_applied_q} \
             (delta {}), but the ADL closed_q is {} — the ledger must rise by exactly closed_q \
             (v16.rs:9533)",
            cur_applied_q.wrapping_sub(prev_applied_q),
            adl.closed_q
        ),
        AdlAccountingKind::MissingPrevAccount => format!(
            "account {account}: no pre-ADL baseline for quantity_adl_applied_q; cannot certify \
             the exact-accounting delta (v16.rs:9530)"
        ),
    }
}

/// Run [`adl_accounting`] over a consecutive `(prev, cur)` observation pair. A thin
/// convenience over the pair-wise oracle so a cross-step runner can fold it across
/// a trace's observations.
pub fn check_step_adl_accounting(prev: &Observation, cur: &Observation) -> Result<(), Violation> {
    adl_accounting(prev, cur)
}

// =============================================================================
// v0.5 — GLOBAL QUOTE-VALUE CONSERVATION (an EMERGENT, engine-UNCHECKED invariant)
// =============================================================================
//
// THIS ORACLE IS DIFFERENT IN KIND FROM EVERY ORACLE ABOVE.
//
// The O1 / cross-link / isolation / ADL-accounting oracles each MIRROR a
// per-state or per-instruction validator the engine ITSELF runs on every accepted
// state (`SourceCreditLienAggregateProofV16::validate`, the ADL ledger advance,
// etc.). Because the engine rejects any state that would break them, those oracles
// hold on every engine-accepted state by construction — they are CONFORMANCE
// checks, and a green result is (correctly) vacuous as a bug signal.
//
// This oracle checks an EMERGENT property the engine does NOT enforce as a single
// global validator across a whole campaign: total real quote-atom value is
// CONSERVED over an entire multi-instruction campaign, changing only by external
// flows. The engine proves a LOCAL version per instruction
// (`TokenValueFlowProofV16::validate`, `v16.rs:2913-2942`, enforces
// `vault_after - vault_before == external_quote_in - external_quote_out` on each
// value-moving instruction). This oracle composes that across the WHOLE campaign,
// at the HARNESS level, from independently observed post-step `vault` snapshots —
// no engine proof object is consulted. A value LEAK or MINT — quote atoms created
// or destroyed across an internal step beyond the external flow — would be the
// most serious class of DeFi bug. So unlike the conformance oracles, a green
// result here is a genuine (non-vacuous) independent signal, and a RED result is a
// CANDIDATE that must be TRIAGED (real leak vs incomplete value model), never a
// confirmed bug on its own.
//
// ## The value model (spec.md §5.1.1 + `StockReconciliationProofV16`)
//
// Per spec req #13 (`spec.md:46`) quote-value conservation is over QUOTE ATOMS
// ONLY; "encumbrances, source-credit reservations, backing buckets, and liens are
// not value classes". Per req #14 (`spec.md:47`) rounding residue has explicit
// sinks. So PnL, claims, liens, backing, and reservations are ACCOUNTING OF CLAIMS,
// not real value.
//
// The engine's own stock reconciliation (`spec.md:1043`,
// `StockReconciliationProofV16::validate`, `v16.rs:3029-3041`) is the authoritative
// quote-atom value model. It proves the TOTAL real quote-atom balance is the token
// vault, and that vault equals the sum of all PARTITION stock classes:
//
//     token_vault == senior_capital_total    (== C_tot == Σ account capital)
//                  + insurance_capital
//                  + backing_provider_earnings
//                  + settlement_rounding_residue_total
//                  + unallocated_protocol_surplus
//
// Therefore `system_quote_value(state) == vault`. `vault` is the single
// source-of-truth total real quote-atom store; every other quote store is a SLICE
// of it. We deliberately do NOT sum the slices: doing so would DOUBLE-COUNT (e.g.
// `c_tot == Σ capital`, `spec.md:1072`, is a derived aggregate of value already
// inside `vault`).
//
// EXCLUDED from the sum (and WHY):
//   * `pnl`, `pnl_pos_tot`, `pnl_*_bound_*` (`v16.rs:4064-4067`): unrealized/claim
//     accounting, NOT a value class (`spec.md:961-972`).
//   * `payout_snapshot` (`v16.rs:4087`): a SNAPSHOT of payout entitlement, claim
//     accounting, not a separate quote-atom store.
//   * `explicit_unallocated_loss_*` (`v16.rs:3709`): LOSS accounting; the value
//     class `ExplicitBackedLoss` (`spec.md:949`) already lives inside `vault`.
//   * `c_tot`, `insurance`, `backing_provider_earnings`: PARTITIONS of `vault`,
//     so adding them on top of `vault` would double-count. (They are surfaced for
//     the optional partition cross-check below, not added to the measure.)
//
// ## External flow in the harness
//
// The ONLY value that crosses the instance boundary in any campaign is `Deposit`
// (`deposit_not_atomic`, external_in) and `Withdraw` (`withdraw_not_atomic`,
// external_out). No campaign invokes `resolve_market` or
// `claim_resolved_payout_topup` (no such `Action`), so there is no external
// insurance payout or fee leaving the system on any step. The fixture seed
// `seed_underwater_position` injects quote atoms straight into `vault`; the driver
// records that injection as external_in (see `driver.rs`), so it too is an honest
// external flow, not a mint. Consequently, EVERY internal step (trade, price move,
// crank, liquidation, ADL, lien op, finalize-close, ...) MUST net EXACTLY ZERO
// vault change: `Δvault == ext_in_delta − ext_out_delta`.

/// Which conservation invariant was broken across a step.  `Copy` and
/// allocation-free so the soundness proof can reason about the predicate without
/// modelling string formatting (same discipline as [`ViolationKind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConservationKind {
    /// `vault` ROSE across the step by MORE than the net external inflow — quote
    /// atoms appeared from nowhere (a value MINT candidate), or the value model is
    /// missing a store that legitimately grew.
    ValueAppeared,
    /// `vault` FELL across the step by MORE than the net external outflow — quote
    /// atoms vanished (a value LEAK candidate), or the value model is missing a
    /// store that legitimately shrank / a value that legitimately left.
    ValueDisappeared,
    /// The engine's own stock-reconciliation partition is violated in the observed
    /// post-step state: the senior + insurance + backing slices EXCEED `vault`
    /// (`senior <= vault`, `v16.rs:4590`). This means the observed value model is
    /// internally inconsistent and the conservation result cannot be trusted —
    /// fail closed rather than certify a possibly-incomplete measure.
    PartitionExceedsVault,
}

impl ConservationKind {
    /// The spec/engine citation this breaks.
    pub fn requirement(self) -> &'static str {
        match self {
            ConservationKind::ValueAppeared | ConservationKind::ValueDisappeared => {
                "quote-value conservation (spec.md:46 req#13; v16.rs:2913 TokenValueFlowProof)"
            }
            ConservationKind::PartitionExceedsVault => {
                "stock reconciliation partition (spec.md:1043; v16.rs:4590 senior<=vault)"
            }
        }
    }
}

/// GLOBAL QUOTE-VALUE CONSERVATION arithmetic core: allocation-free, pure,
/// fail-closed.  Given the pre-step total quote-atom balance `prev_vault`
/// (`prev.system.vault`), the post-step balance `cur_vault` (`cur.system.vault`),
/// and the per-step external flow deltas `ext_in_delta` / `ext_out_delta`, returns
/// `Ok(())` iff
///
/// ```text
/// cur_vault − prev_vault == ext_in_delta − ext_out_delta
/// ```
///
/// i.e. the TOTAL real quote-atom balance changed by EXACTLY the net external flow.
/// `partition_sum` is the engine's senior+insurance+backing partition of the
/// post-step vault (`c_tot + insurance + backing_provider_earnings`,
/// `v16.rs:4583-4589`); if it EXCEEDS `cur_vault` the value model is internally
/// inconsistent and we fail closed.
///
/// Reasoned about by the companion Kani proof. It compares the two SIGNED-SAFE
/// sides of the equation via `u128` checked arithmetic, never forming an
/// intermediate that could overflow or wrap to mask a breach: it tests the
/// equality `prev_vault + ext_in_delta == cur_vault + ext_out_delta` (every term a
/// non-negative `u128`, the only widening being two checked additions), which is
/// algebraically identical to the displayed equation but cannot underflow.
#[inline]
pub fn value_conservation_kind(
    prev_vault: u128,
    cur_vault: u128,
    ext_in_delta: u128,
    ext_out_delta: u128,
    partition_sum: u128,
) -> Result<(), ConservationKind> {
    // The observed value model must be internally consistent: the engine's own
    // senior+insurance+backing partition can never exceed the total vault
    // (`senior <= vault`, v16.rs:4590). If it does, the measure is untrustworthy;
    // fail closed rather than certify conservation against a broken partition.
    if partition_sum > cur_vault {
        return Err(ConservationKind::PartitionExceedsVault);
    }

    // Balance the conservation equation as `prev_vault + ext_in == cur_vault +
    // ext_out`. Both sides are sums of non-negative u128 atoms; using checked_add
    // means an overflow (impossible within the engine's bounded vault, but proven
    // safe rather than assumed) fails closed instead of wrapping. Splitting the
    // comparison by which side is larger lets us name the breach direction
    // precisely without ever forming a negative intermediate.
    let lhs = prev_vault.checked_add(ext_in_delta);
    let rhs = cur_vault.checked_add(ext_out_delta);
    match (lhs, rhs) {
        (Some(l), Some(r)) if l == r => Ok(()),
        // lhs < rhs  ⇒  cur_vault + ext_out > prev_vault + ext_in
        //           ⇒  Δvault > ext_in − ext_out: value APPEARED beyond the inflow.
        (Some(l), Some(r)) if l < r => Err(ConservationKind::ValueAppeared),
        // lhs > rhs  ⇒  Δvault < ext_in − ext_out: value DISAPPEARED beyond outflow.
        (Some(l), Some(r)) if l > r => Err(ConservationKind::ValueDisappeared),
        // An addition overflowed (cannot happen for a real bounded vault). Fail
        // closed to the worst case: treat as value appearing.
        _ => Err(ConservationKind::ValueAppeared),
    }
}

/// v0.5 GLOBAL QUOTE-VALUE CONSERVATION oracle: for a consecutive `(prev, cur)`
/// observation pair, check that the instance's TOTAL real quote-atom balance
/// (`system.vault`, the engine's `StockReconciliationProofV16.token_vault`) changed
/// by EXACTLY the net external flow that crossed the boundary on this step.
///
/// `ext_in_delta` / `ext_out_delta` are the per-step external inflow / outflow
/// (the runner records them on the observation as `cur.ext_in_step` /
/// `cur.ext_out_step`; the public [`check_step_value_conservation`] wires them in).
/// For an INTERNAL step (no deposit / withdraw) both are 0, so the vault MUST be
/// unchanged.
///
/// This is an EMERGENT invariant the engine does NOT enforce globally (see the
/// module banner above): a green result is a genuine independent conservation
/// signal, and a RED result is a CANDIDATE to be TRIAGED, not a confirmed bug.
/// Pure and fail-closed; wraps [`value_conservation_kind`], adding a human-readable
/// detail only on the error path.
pub fn value_conservation(
    prev: &Observation,
    cur: &Observation,
    ext_in_delta: u128,
    ext_out_delta: u128,
) -> Result<(), Violation> {
    let prev_vault = prev.system.vault;
    let cur_vault = cur.system.vault;
    let partition_sum = partition_sum(&cur.system);
    value_conservation_kind(
        prev_vault,
        cur_vault,
        ext_in_delta,
        ext_out_delta,
        partition_sum,
    )
    .map_err(|kind| Violation {
        requirement: kind.requirement(),
        detail: describe_conservation(
            kind,
            prev_vault,
            cur_vault,
            ext_in_delta,
            ext_out_delta,
            &cur.system,
        ),
    })
}

/// The engine's senior+insurance+backing partition of the vault in the observed
/// post-step state: `c_tot + insurance + backing_provider_earnings`
/// (`v16.rs:4583-4589`). Saturating (the values are real engine quantities and the
/// sum is only used for a `>`-comparison; saturation can only ever make the
/// cross-check STRICTER, never hide a breach).
#[inline]
fn partition_sum(s: &SystemObs) -> u128 {
    s.c_tot
        .saturating_add(s.insurance)
        .saturating_add(s.backing_provider_earnings)
}

/// Build the human-readable detail for a [`ConservationKind`]. Only ever called on
/// the error path, so its `format!` allocations stay off the model-checked core.
fn describe_conservation(
    kind: ConservationKind,
    prev_vault: u128,
    cur_vault: u128,
    ext_in_delta: u128,
    ext_out_delta: u128,
    s: &SystemObs,
) -> String {
    match kind {
        ConservationKind::ValueAppeared => format!(
            "vault {prev_vault} -> {cur_vault} (Δ +{}) but net external flow was \
             +{ext_in_delta}/-{ext_out_delta} (net {}): {} quote atoms APPEARED beyond the \
             external flow. Triage: is this a value store excluded from the model that legitimately \
             grew (c_tot={}, insurance={}, backing_earnings={}), or a genuine MINT?",
            cur_vault.wrapping_sub(prev_vault),
            (ext_in_delta as i128) - (ext_out_delta as i128),
            (cur_vault.wrapping_sub(prev_vault)).wrapping_sub(ext_in_delta.wrapping_sub(ext_out_delta)),
            s.c_tot,
            s.insurance,
            s.backing_provider_earnings,
        ),
        ConservationKind::ValueDisappeared => format!(
            "vault {prev_vault} -> {cur_vault} (Δ -{}) but net external flow was \
             +{ext_in_delta}/-{ext_out_delta} (net {}): quote atoms DISAPPEARED beyond the \
             external flow. Triage: did value legitimately LEAVE (external payout/fee) unaccounted, \
             or is this a genuine LEAK?",
            prev_vault.wrapping_sub(cur_vault),
            (ext_in_delta as i128) - (ext_out_delta as i128),
        ),
        ConservationKind::PartitionExceedsVault => format!(
            "post-step partition c_tot({}) + insurance({}) + backing_earnings({}) = {} EXCEEDS \
             vault({cur_vault}): the engine's own senior<=vault invariant (v16.rs:4590) is \
             violated in the observed state, so the conservation measure is untrustworthy",
            s.c_tot,
            s.insurance,
            s.backing_provider_earnings,
            partition_sum(s),
        ),
    }
}

/// Run [`value_conservation`] over a consecutive `(prev, cur)` observation pair,
/// taking the per-step external flow deltas straight off `cur` (the runner records
/// them there). A thin convenience over the pair-wise oracle so a cross-step runner
/// can fold the v0.5 conservation check across a trace's observations exactly like
/// the other `check_step_*` helpers.
pub fn check_step_value_conservation(
    prev: &Observation,
    cur: &Observation,
) -> Result<(), Violation> {
    value_conservation(prev, cur, cur.ext_in_step, cur.ext_out_step)
}

// =============================================================================
// v0.6 — FUNDING CLAIMABLE-VALUE CONSERVATION (an EMERGENT, engine-UNCHECKED red)
// =============================================================================
//
// THE FINDING (a CANDIDATE red, triaged below): funding settlement is NOT
// value-conservative for a fractional-basis position. Funding is economically a
// ZERO-SUM transfer — what the payer leg loses, the receiver leg gains — so the
// total CLAIMABLE value `M = Σ (account.capital + account.pnl)` over all accounts
// MUST be invariant under any funding settlement (a step with no external flow).
//
// The engine breaks this by a FLOOR/CEIL ASYMMETRY. The two legs of a matched
// position settle their funding K/F deltas in SEPARATE `permissionless_crank`
// instructions (`settle_leg_kf_effects_at_slot`, `v16.rs:7179`):
//   * the RECEIVER leg (`net > 0`, `v16.rs:7194-7197`) credits PnL by
//     `floor_div_signed_conservative_i128(+x) = ⌊x⌋ = q` (truncates down,
//     `wide_math.rs:1435`);
//   * the PAYER leg (`net < 0`, `v16.rs:7198-7208`) debits CAPITAL by
//     `|floor_div_signed_conservative_i128(−x)| = ⌈x⌉ = q+1` (rounds away from
//     zero, `wide_math.rs:1441`) via `reserve_new_capital_backed_loss` (`v16.rs:6998`,
//     `c_tot -= q+1` at `:7028`).
// When the per-leg magnitude `x = funding_delta·|basis| / (a_basis·POS_SCALE)` has a
// nonzero remainder (a FRACTIONAL basis), `⌈x⌉ − ⌊x⌋ = 1`: the payer loses one MORE
// atom than the receiver gains. `M` falls by exactly 1 per settled slot. The vault
// is UNTOUCHED (funding moves no tokens), so the destroyed atom becomes permanent,
// unattributable vault slack.
//
// WHY THE ENGINE NEVER CATCHES IT:
//   * No per-instruction `TokenValueFlowProof` spans BOTH legs (they settle in
//     separate cranks), and each leg's own proof balances against the flat vault.
//   * The destroyed atom is credited to NO sink, violating spec req #14 (`spec.md:47`:
//     conservative-rounding residue MUST be credited to `SettlementRoundingResidue`
//     or `UnallocatedProtocolSurplus`). Those two sink fields HAVE NO STORAGE in the
//     live `MarketGroupV16HeaderAccount` (`v16.rs:4057-4091`) — they exist only as
//     fields of `StockReconciliationProofV16` (`v16.rs:3019-3025`).
//   * `StockReconciliationProofV16` (`v16.rs:3019`/`3028`, the ONLY object binding the
//     partition equality `vault == Σ stock classes`) is DEAD CODE — zero construction
//     sites in `src/` or `tests/`. So nothing reconciles the growing slack.
//   * v0.5 vault conservation stays GREEN (the vault never moves), so this is a
//     genuinely DISTINCT, emergent invariant.
//
// TRIAGE / HONEST LIMITS: the leak is NON-EXTRACTIVE — the rounding is uniformly
// conservative (always FOR the protocol), so the slack accrues to the protocol as
// over-collateralization; no user gains. It is one atom per settled fractional slot,
// but it is PERMANENT, MONOTONE and UNBOUNDED across epochs and fractional-basis
// legs. So a red here is a CANDIDATE to triage (a real conservation/req#14 break vs
// an accepted conservative-rounding policy), never a confirmed exploit. A reviewer
// could argue the conservative direction is intended; the rebuttal is that req #14
// mandates an EXPLICIT sink via a balanced flow proof and `spec.md:28` says
// unattributed rounding residue MUST roll the instruction back — neither happens,
// and the engine cannot even account for the accumulated slack.
//
// LAG vs LEAK (why the demonstration differences two campaign lengths): funding
// settlement lags accrual by one refresh (refresh-then-accrue), so at any snapshot
// one slot of funding is "in flight" (payer debited, receiver not yet credited).
// That produces a BOUNDED, CONSTANT shortfall offset that does NOT grow with the
// campaign length. The LEAK is the part that grows by exactly 1 per added settled
// fractional slot. `tests/funding_conservation.rs` isolates the leak by differencing
// shortfalls across lengths (and clean-vs-fractional bases), so the signal can never
// be confused with the benign lag.

/// Total CLAIMABLE value of an observation: `Σ (capital + pnl)` over all accounts,
/// in quote atoms (i128, since `pnl` is signed). This is the quantity a funding
/// settlement — a pure transfer between accounts — MUST conserve. Saturating-free:
/// real engine magnitudes are far below the i128 range.
pub fn claimable_value(obs: &Observation) -> i128 {
    obs.accounts
        .iter()
        .map(|a| a.capital as i128 + a.pnl)
        .sum()
}

/// Which direction the funding claimable-value conservation broke. `Copy` and
/// allocation-free so the soundness proof can reason about the predicate without
/// modelling string formatting (same discipline as [`ViolationKind`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingConservationKind {
    /// Claimable value (`Σ capital + Σ pnl`) FELL by more than the net external
    /// outflow — value was DESTROYED (the funding floor/ceil leak), or legitimately
    /// left unaccounted.
    ClaimableValueDestroyed,
    /// Claimable value ROSE by more than the net external inflow — value was created
    /// from nowhere (the EXTRACTIVE direction; not expected from conservative funding).
    ClaimableValueCreated,
}

impl FundingConservationKind {
    /// The spec/engine citation this breaks.
    pub fn requirement(self) -> &'static str {
        "funding value conservation (spec.md:47 req#14 rounding-residue sink; \
         floor/ceil asymmetry wide_math.rs:1441 vs :1435; StockReconciliation dead v16.rs:3019)"
    }
}

/// FUNDING CLAIMABLE-VALUE CONSERVATION arithmetic core: allocation-free, pure,
/// fail-closed. Over a window, claimable value must change by EXACTLY the net
/// external flow that crossed the boundary:
///
/// ```text
/// cur_claimable − prev_claimable == ext_in − ext_out
/// ```
///
/// Returns `Ok(())` iff balanced, else the breach direction. Reasoned about by the
/// companion Kani proof. It balances the equation as the two SIGNED sides
/// `prev_claimable + ext_in == cur_claimable + ext_out` via `i128` checked
/// arithmetic, never forming an intermediate that could overflow/wrap to mask a
/// breach (fail-closed to the worst case on overflow, exactly like
/// [`value_conservation_kind`]).
#[inline]
pub fn funding_conservation_kind(
    prev_claimable: i128,
    cur_claimable: i128,
    ext_in: u128,
    ext_out: u128,
) -> Result<(), FundingConservationKind> {
    // External flow magnitudes are small, non-negative quote-atom counts; if either
    // cannot be represented as i128 the measure is untrustworthy — fail closed to
    // "value destroyed" (the conservative worst case for a leak hunt).
    let ext_in_i = match i128::try_from(ext_in) {
        Ok(v) => v,
        Err(_) => return Err(FundingConservationKind::ClaimableValueDestroyed),
    };
    let ext_out_i = match i128::try_from(ext_out) {
        Ok(v) => v,
        Err(_) => return Err(FundingConservationKind::ClaimableValueDestroyed),
    };
    // Compare prev + ext_in  vs  cur + ext_out (algebraically identical to the
    // displayed equation). checked_add fails closed rather than wrapping.
    let lhs = prev_claimable.checked_add(ext_in_i);
    let rhs = cur_claimable.checked_add(ext_out_i);
    match (lhs, rhs) {
        (Some(l), Some(r)) if l == r => Ok(()),
        // lhs > rhs ⇒ prev + ext_in > cur + ext_out ⇒ claimable fell below the net
        // inflow: value DESTROYED (the funding leak).
        (Some(l), Some(r)) if l > r => Err(FundingConservationKind::ClaimableValueDestroyed),
        // lhs < rhs ⇒ claimable rose beyond the net inflow: value CREATED.
        (Some(l), Some(r)) if l < r => Err(FundingConservationKind::ClaimableValueCreated),
        // An addition overflowed (impossible for real bounded magnitudes). Fail
        // closed to the worst case.
        _ => Err(FundingConservationKind::ClaimableValueDestroyed),
    }
}

/// v0.6 FUNDING CLAIMABLE-VALUE CONSERVATION oracle over a window `(prev, cur)`:
/// claimable value (`Σ capital + Σ pnl`) must change by EXACTLY the net external
/// flow `ext_in − ext_out`. Pure and fail-closed; wraps [`funding_conservation_kind`].
///
/// NOTE: funding is zero-sum only across BOTH legs of a position, which settle in
/// SEPARATE cranks, so this is meaningless per single-step pair (it would flag the
/// payer's and receiver's individual cranks, and the benign settlement-lag). Apply
/// it over a WHOLE funding sub-campaign window (`prev` = the empty/baseline state,
/// `cur` = the campaign end) — see [`claimable_shortfall`] and
/// `tests/funding_conservation.rs`, which difference lengths to isolate the
/// permanent leak from the bounded lag.
pub fn funding_value_conservation(
    prev: &Observation,
    cur: &Observation,
    ext_in: u128,
    ext_out: u128,
) -> Result<(), Violation> {
    funding_conservation_kind(claimable_value(prev), claimable_value(cur), ext_in, ext_out).map_err(
        |kind| Violation {
            requirement: kind.requirement(),
            detail: match kind {
                FundingConservationKind::ClaimableValueDestroyed => format!(
                    "claimable value Σ(capital+pnl) {} -> {} but net external flow was +{ext_in}/-{ext_out}: \
                     {} atoms DESTROYED (funding floor/ceil leak to unattributable vault slack; no req#14 sink)",
                    claimable_value(prev),
                    claimable_value(cur),
                    (claimable_value(prev) + ext_in as i128) - (claimable_value(cur) + ext_out as i128),
                ),
                FundingConservationKind::ClaimableValueCreated => format!(
                    "claimable value Σ(capital+pnl) {} -> {} but net external flow was +{ext_in}/-{ext_out}: \
                     value CREATED beyond external inflow (extractive direction)",
                    claimable_value(prev),
                    claimable_value(cur),
                ),
            },
        },
    )
}

/// The claimable-value SHORTFALL over a whole trace: the net external flow that
/// entered the instance minus the claimable value (`Σ capital + Σ pnl`) actually
/// present at the end. The system starts empty (`M == 0`), so under conservation
/// `M_final == Σ ext_in − Σ ext_out`; a POSITIVE shortfall means that many quote
/// atoms of claimable value were DESTROYED (left all accounts without leaving via a
/// withdrawal) — the funding leak plus the bounded in-flight settlement lag.
///
/// `tests/funding_conservation.rs` differences this across campaign lengths and
/// clean-vs-fractional bases to isolate the PERMANENT, length-proportional LEAK from
/// the CONSTANT lag. Returns `(expected_net_external_in, claimable_final, shortfall)`.
pub fn claimable_shortfall(observations: &[Observation]) -> (i128, i128, i128) {
    let mut cum_in: i128 = 0;
    let mut cum_out: i128 = 0;
    for o in observations {
        cum_in += o.ext_in_step as i128;
        cum_out += o.ext_out_step as i128;
    }
    let net_ext = cum_in - cum_out;
    let m_final = observations.last().map(claimable_value).unwrap_or(0);
    (net_ext, m_final, net_ext - m_final)
}
