//! Anti-vacuity gate.
//!
//! The O1 realizability oracle (`src/oracles.rs`) only *means* something if the
//! harness actually drives the engine into a state where the source-credit-lien
//! fields it polices are NON-ZERO. If every observed `DomainObs` is
//! `DomainObs::default()` (all zeros), the six realizability relationships hold
//! trivially and a "conformance" result is vacuous.
//!
//! This test builds a deliberately lien-creating campaign and asserts that
//! running it produces at least one `DomainObs` with a non-zero
//! source-credit-lien field. If it ever fails, the conformance result has gone
//! vacuous again and must be treated as a loud regression — NEVER weaken this
//! assertion to make it pass.

use perp_adversary::driver::{lien_creating_campaign, run, DomainObs};

/// True iff this domain carries any non-trivial source-credit-lien state — i.e.
/// the realizability machinery in the oracle has something real to check.
fn domain_is_nonzero(d: &DomainObs) -> bool {
    *d != DomainObs::default()
}

#[test]
fn campaign_produces_a_nonzero_source_credit_domain() {
    let s = lien_creating_campaign();
    let trace = run(&s);

    // Find the first observation/account/domain that carries a real lien.
    let mut found: Option<(usize, usize, usize, DomainObs)> = None;
    for obs in &trace.observations {
        for (ai, acct) in obs.accounts.iter().enumerate() {
            for (di, dom) in acct.domains.iter().enumerate() {
                if domain_is_nonzero(dom) {
                    found = Some((obs.step, ai, di, *dom));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }

    let (step, ai, di, dom) = found.unwrap_or_else(|| {
        // Dump the campaign's step results to make a vacuity regression debuggable.
        let mut trail = String::new();
        for obs in &trace.observations {
            trail.push_str(&format!("  step {}: {:?} -> {:?}\n", obs.step, obs.action, obs.result));
        }
        panic!(
            "VACUITY: no observation produced a non-zero source-credit domain. \
             The oracle never evaluates the realizability machinery, so the \
             conformance result is vacuous. Campaign step trail:\n{trail}"
        );
    });

    // The realizability oracle keys off `source_claim_liened_num` (the locked
    // face claim) and `source_lien_effective_reserved` (the reserved backing).
    // Assert a *source-credit-lien* field specifically is non-zero, not merely
    // any field (e.g. a bare market-id stamp), so the gate proves the lien
    // pipeline actually ran.
    assert!(
        dom.source_claim_liened_num != 0
            || dom.source_lien_effective_reserved != 0
            || dom.source_claim_bound_num != 0,
        "found a non-default domain at step {step}, account {ai}, domain {di}, \
         but no source-credit-lien field is non-zero: {dom:?}"
    );
}
