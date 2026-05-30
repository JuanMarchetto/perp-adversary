//! O1 realizability oracle unit tests (TDD — written before the implementation).
//!
//! Fixtures mirror the engine's OWN per-account source-domain validator
//! `SourceCreditLienAggregateProofV16::validate()`
//! (`percolator-ref/src/v16.rs:3060-3100`). A well-formed (engine-realizable)
//! domain must pass `realizability`; any domain that breaks one of the engine's
//! asserted relationships must be flagged.
//!
//! Scale: `BOUND_SCALE == 1_000_000_000_000` (1e12) — claim/backing "num" fields
//! are amounts scaled by BOUND_SCALE; `*_effective_reserved` are unscaled atoms.

use perp_adversary::driver::{AccountObs, DomainObs, Observation};
use perp_adversary::oracles::{check_observation, realizability};
use perp_adversary::scenario::Action;

const SCALE: u128 = 1_000_000_000_000;

/// A fully-backed, well-formed holding: effective reserved == liened amount,
/// backing-num == effective * SCALE, face decomposes into counterparty side.
fn fully_backed(effective: u128) -> DomainObs {
    let backing_num = effective * SCALE;
    let face_num = backing_num; // credit_rate == 1.0 case: face == backing
    DomainObs {
        source_claim_market_id: 1,
        source_claim_bound_num: face_num,
        source_claim_liened_num: face_num,
        source_claim_counterparty_liened_num: face_num,
        source_claim_insurance_liened_num: 0,
        source_lien_effective_reserved: effective,
        source_lien_counterparty_backing_num: backing_num,
        source_lien_insurance_backing_num: 0,
        source_claim_impaired_num: 0,
        source_lien_impaired_effective_reserved: 0,
    }
}

#[test]
fn empty_domain_is_realizable() {
    assert_eq!(realizability(&DomainObs::default()), Ok(()));
}

#[test]
fn fully_backed_domain_is_realizable() {
    assert_eq!(realizability(&fully_backed(5)), Ok(()));
}

#[test]
fn haircut_domain_with_face_above_backing_is_realizable() {
    // credit_rate < 1.0: face claim locked exceeds backing-num, but effective
    // reserved (ceil(face/SCALE)) is still backed by the reserved backing-num.
    // effective = ceil(7*SCALE / SCALE) = 7; backing_num = 7*SCALE.
    let face_num = 7 * SCALE;
    let effective = 7; // == amount_from_bound_num(face_num)
    let d = DomainObs {
        source_claim_market_id: 1,
        source_claim_bound_num: 9 * SCALE,
        source_claim_liened_num: face_num,
        source_claim_counterparty_liened_num: face_num,
        source_claim_insurance_liened_num: 0,
        source_lien_effective_reserved: effective,
        source_lien_counterparty_backing_num: effective * SCALE,
        source_lien_insurance_backing_num: 0,
        source_claim_impaired_num: 0,
        source_lien_impaired_effective_reserved: 0,
    };
    assert_eq!(realizability(&d), Ok(()));
}

#[test]
fn insurance_split_is_realizable() {
    // counterparty + insurance face must equal total face; backing-num split too.
    let cp_face = 3 * SCALE;
    let ins_face = 2 * SCALE;
    let d = DomainObs {
        source_claim_market_id: 1,
        source_claim_bound_num: cp_face + ins_face,
        source_claim_liened_num: cp_face + ins_face,
        source_claim_counterparty_liened_num: cp_face,
        source_claim_insurance_liened_num: ins_face,
        source_lien_effective_reserved: 5,
        source_lien_counterparty_backing_num: 3 * SCALE,
        source_lien_insurance_backing_num: 2 * SCALE,
        source_claim_impaired_num: 0,
        source_lien_impaired_effective_reserved: 0,
    };
    assert_eq!(realizability(&d), Ok(()));
}

#[test]
fn impaired_with_reserve_is_realizable() {
    // impaired_effective_reserved != 0 is allowed iff impaired_face != 0.
    let mut d = fully_backed(4);
    d.source_claim_bound_num = 6 * SCALE; // room for impaired face under the bound
    d.source_claim_impaired_num = 2 * SCALE;
    d.source_lien_impaired_effective_reserved = 2;
    assert_eq!(realizability(&d), Ok(()));
}

// ---- violation cases: each breaks ONE engine-asserted relationship ----

#[test]
fn effective_reserved_exceeding_realizable_backing_is_violation() {
    // Core Req#2 cap: effective_credit_reserved > ceil(face_locked / SCALE).
    // face = 5*SCALE -> max effective = 5; set effective = 6.
    let mut d = fully_backed(5);
    d.source_lien_effective_reserved = 6;
    d.source_lien_counterparty_backing_num = 6 * SCALE; // keep reservation-exact
    let err = realizability(&d).unwrap_err();
    assert_eq!(err.requirement, "R2");
}

#[test]
fn face_decomposition_mismatch_is_violation() {
    // counterparty_face + insurance_face != face_locked.
    let mut d = fully_backed(5);
    d.source_claim_counterparty_liened_num = 4 * SCALE; // should be 5*SCALE
    realizability(&d).unwrap_err();
}

#[test]
fn locked_plus_impaired_exceeding_bound_is_violation() {
    // face_locked + impaired_face > source_claim_bound_num.
    let mut d = fully_backed(5); // face_locked = 5*SCALE, bound = 5*SCALE
    d.source_claim_impaired_num = SCALE; // 5+1 > 5
    realizability(&d).unwrap_err();
}

#[test]
fn backing_num_not_matching_effective_is_violation() {
    // counterparty_backing + insurance_backing != effective * SCALE.
    let mut d = fully_backed(5);
    d.source_lien_counterparty_backing_num = 4 * SCALE; // should be 5*SCALE
    realizability(&d).unwrap_err();
}

#[test]
fn backing_num_not_atom_aligned_is_violation() {
    // counterparty_backing_reserved_num % SCALE != 0.
    let mut d = fully_backed(5);
    d.source_lien_counterparty_backing_num = 5 * SCALE + 1;
    realizability(&d).unwrap_err();
}

#[test]
fn impaired_reserve_without_impaired_face_is_violation() {
    let mut d = fully_backed(5);
    d.source_lien_impaired_effective_reserved = 1; // but impaired_face == 0
    realizability(&d).unwrap_err();
}

// ---- check_observation walks every account/domain ----

fn obs_with(domains: Vec<DomainObs>) -> Observation {
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
        market_domains: Vec::new(),
        liquidation: None,
    }
}

#[test]
fn check_observation_passes_when_all_domains_realizable() {
    let obs = obs_with(vec![fully_backed(5), DomainObs::default(), fully_backed(2)]);
    assert_eq!(check_observation(&obs), Ok(()));
}

#[test]
fn check_observation_flags_offending_domain_with_location() {
    let mut bad = fully_backed(5);
    bad.source_lien_effective_reserved = 99; // over-reserved
    let obs = obs_with(vec![fully_backed(2), bad]);
    let err = check_observation(&obs).unwrap_err();
    // detail must locate account 0, domain 1.
    assert!(err.detail.contains("account 0"), "detail: {}", err.detail);
    assert!(err.detail.contains("domain 1"), "detail: {}", err.detail);
}
