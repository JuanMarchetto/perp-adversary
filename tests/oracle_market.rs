//! v0.1 market-engine realizability CROSS-LINK oracle unit tests (TDD — written
//! before the implementation).
//!
//! These fixtures mirror, field-for-field, the COMPOSING relationships the engine
//! checks in `MarketGroupV16View::validate_source_credit_shape_with_market`
//! (`percolator-ref/src/v16.rs:2143-2253`, SHA `71c9032`) BETWEEN each per-account
//! source domain `d` and the MARKET-ENGINE `SourceCreditStateV16` of the asset /
//! side that domain maps to (asset = d/2, side = d%2, 0=long, 1=short):
//!
//!   * v16.rs:2177-2179 — `source.source_claim_market_id == asset.market_id`
//!     (else `HiddenLeg`); and
//!   * v16.rs:2186-2188 — `source.source_claim_bound_num
//!                          <= domain_credit.positive_claim_bound_num`
//!     (else `InvalidLeg`). This is the realizability cross-link O1 scoped out.
//!
//! A well-formed pairing (bound <= market positive bound AND market_ids match)
//! must pass `market_cross_link`; any pairing breaking either relationship must be
//! flagged with the matching `Violation`.
//!
//! Scale: `BOUND_SCALE == 1_000_000_000_000` (1e12) — claim/bound "num" fields are
//! amounts scaled by BOUND_SCALE.

use perp_adversary::driver::{AccountObs, DomainObs, EngineDomainObs, MarketSideObs, Observation};
use perp_adversary::oracles::{check_observation_market, market_cross_link};
use perp_adversary::scenario::Action;

const SCALE: u128 = 1_000_000_000_000;

/// A non-empty per-account source domain on `market_id`, with `bound_num` scaled
/// claim bound. (Only the cross-link-relevant fields matter here; the rest are
/// the O1 per-account fields, kept self-consistent but irrelevant to this oracle.)
fn account_domain(market_id: u64, bound_num: u128) -> DomainObs {
    DomainObs {
        source_claim_market_id: market_id,
        source_claim_bound_num: bound_num,
        // a tiny consistent locked face so the domain is NOT numeric-zero.
        source_claim_liened_num: SCALE,
        source_claim_counterparty_liened_num: SCALE,
        source_claim_insurance_liened_num: 0,
        source_lien_effective_reserved: 1,
        source_lien_counterparty_backing_num: SCALE,
        source_lien_insurance_backing_num: 0,
        source_claim_impaired_num: 0,
        source_lien_impaired_effective_reserved: 0,
    }
}

/// A market-engine side state carrying `positive_claim_bound_num`. The asset's
/// `market_id` follows the driver's activation convention `market_id == asset+1`
/// (see `driver::Engine::new`), so a per-account domain built with
/// `account_domain(asset+1, ..)` market-id-matches its asset.
fn market_side(asset: usize, side: u8, positive_claim_bound_num: u128) -> MarketSideObs {
    MarketSideObs {
        asset,
        side,
        market_id: (asset as u64) + 1,
        state: EngineDomainObs {
            positive_claim_bound_num,
            ..EngineDomainObs::default()
        },
        insurance_domain_spent: 0,
    }
}

// ---- single-domain core: market_cross_link over a per-account domain vector ----

#[test]
fn bound_below_market_bound_with_matching_id_is_ok() {
    // domain 0 -> asset 0, long (side 0). market_id of asset 0 == 1.
    let domains = vec![account_domain(1, 3 * SCALE)];
    let market = vec![
        market_side(0, 0, 5 * SCALE), // positive bound 5 >= per-account 3 -> ok
        market_side(0, 1, 0),
    ];
    assert_eq!(market_cross_link(&domains, &market), Ok(()));
}

#[test]
fn bound_equal_to_market_bound_is_ok() {
    // boundary: per-account bound == market positive bound is allowed (<=).
    let domains = vec![account_domain(1, 5 * SCALE)];
    let market = vec![market_side(0, 0, 5 * SCALE), market_side(0, 1, 0)];
    assert_eq!(market_cross_link(&domains, &market), Ok(()));
}

#[test]
fn bound_exceeding_market_bound_is_violation() {
    // mirror InvalidLeg (v16.rs:2186): per-account bound 6 > market positive 5.
    let domains = vec![account_domain(1, 6 * SCALE)];
    let market = vec![market_side(0, 0, 5 * SCALE), market_side(0, 1, 0)];
    let err = market_cross_link(&domains, &market).unwrap_err();
    assert_eq!(err.requirement, "market cross-link (v16.rs:2186)");
    assert!(
        err.detail.contains("6000000000000") && err.detail.contains("5000000000000"),
        "detail should cite the per-account bound and the market bound: {}",
        err.detail
    );
}

#[test]
fn market_id_mismatch_is_violation() {
    // mirror HiddenLeg (v16.rs:2177): per-account market_id 99 != asset.market_id
    // (asset 0's market_id is 1 under the driver convention).
    let domains = vec![account_domain(99, SCALE)];
    let market = vec![market_side(0, 0, 5 * SCALE), market_side(0, 1, 0)];
    let err = market_cross_link(&domains, &market).unwrap_err();
    assert_eq!(err.requirement, "market cross-link (v16.rs:2177)");
}

#[test]
fn short_side_domain_maps_to_side_1() {
    // domain 1 -> asset 0, side 1 (short). Pair against the SHORT market state.
    // long side has a tiny bound that WOULD trip if mis-paired; short has room.
    let domains = vec![
        DomainObs::default(),         // domain 0 (long) empty -> skipped
        account_domain(1, 4 * SCALE), // domain 1 (short)
    ];
    let market = vec![
        market_side(0, 0, SCALE),     // long market bound 1 (would fail if paired)
        market_side(0, 1, 5 * SCALE), // short market bound 5 >= 4 -> ok
    ];
    assert_eq!(market_cross_link(&domains, &market), Ok(()));
}

#[test]
fn second_asset_domain_maps_to_asset_1() {
    // domain 2 -> asset 1, side 0 (long).
    let domains = vec![
        DomainObs::default(),         // d0 asset0 long
        DomainObs::default(),         // d1 asset0 short
        account_domain(2, 7 * SCALE), // d2 asset1 long; asset1 market_id == 2
    ];
    let market = vec![
        market_side(0, 0, 0),
        market_side(0, 1, 0),
        market_side(1, 0, 9 * SCALE), // asset1 long market bound 9 >= 7 -> ok
        market_side(1, 1, 0),
    ];
    assert_eq!(market_cross_link(&domains, &market), Ok(()));

    // and a breach on that asset is still caught.
    let domains_bad = vec![
        DomainObs::default(),
        DomainObs::default(),
        account_domain(2, 10 * SCALE), // 10 > market 9 -> InvalidLeg
    ];
    let err = market_cross_link(&domains_bad, &market).unwrap_err();
    assert_eq!(err.requirement, "market cross-link (v16.rs:2186)");
}

#[test]
fn numeric_zero_domain_is_skipped() {
    // An all-zero per-account domain is the engine's `numeric_zero_source_domain`
    // (v16.rs:2155): the cross-link checks (2177, 2186) are NOT applied to it.
    // Even paired against a zero market bound it must clear.
    let domains = vec![DomainObs::default()];
    let market = vec![market_side(0, 0, 0), market_side(0, 1, 0)];
    assert_eq!(market_cross_link(&domains, &market), Ok(()));
}

#[test]
fn missing_market_side_for_nonempty_domain_is_violation() {
    // Fail-closed: a non-empty per-account domain with NO observable market side
    // to pair against cannot be certified -> violation (mirror HiddenLeg intent:
    // the engine reads `market.markets[asset_index]`, which must exist).
    let domains = vec![account_domain(1, SCALE)];
    let market: Vec<MarketSideObs> = Vec::new();
    assert!(market_cross_link(&domains, &market).is_err());
}

// ---- check_observation_market walks every account ----

fn obs_with(domains: Vec<DomainObs>, market_domains: Vec<MarketSideObs>) -> Observation {
    Observation {
        step: 0,
        action: Action::Deposit {
            account: 0,
            amount: 0,
        },
        result: Ok(()),
        accounts: vec![AccountObs {
            capital: 0,
            pnl: 0,
            fee_credits: 0,
            domains,
        }],
        market_domains,
        liquidation: None,
    }
}

#[test]
fn check_observation_market_passes_when_all_pairings_ok() {
    let obs = obs_with(
        vec![account_domain(1, 3 * SCALE), DomainObs::default()],
        vec![market_side(0, 0, 5 * SCALE), market_side(0, 1, 0)],
    );
    assert_eq!(check_observation_market(&obs), Ok(()));
}

#[test]
fn check_observation_market_flags_offending_account_and_domain() {
    let bad = account_domain(1, 8 * SCALE); // 8 > market 5
    let obs = obs_with(
        vec![bad],
        vec![market_side(0, 0, 5 * SCALE), market_side(0, 1, 0)],
    );
    let err = check_observation_market(&obs).unwrap_err();
    assert!(err.detail.contains("account 0"), "detail: {}", err.detail);
    assert!(err.detail.contains("domain 0"), "detail: {}", err.detail);
}
