use perp_adversary::driver::{run, DomainObs};
use perp_adversary::oracles::check_observation;
use perp_adversary::scenario::{Action, Scenario};

/// The JELLY archetype: an account accumulates a large positive source-credit
/// claim on a thin asset, draws an initial-margin source-credit lien against it
/// via a risk-increasing trade, then tries to extract/lever that credit. The
/// engine must keep the liened positive claim bounded by realizable backing at
/// every step — exactly the O1 realizability cap.
///
/// Unlike v0's original JELLY (which used `MovePrice` and, with an open
/// position, never actually moved price — see README "v0 result"), this campaign
/// drives the engine into a genuinely POPULATED source-credit-lien state, so the
/// oracle evaluates the realizability machinery rather than an all-zero default.
fn jelly_scenario() -> Scenario {
    Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            // Counterparty funds the short legs.
            Action::Deposit {
                account: 1,
                amount: 10_000_000,
            },
            // Account 0 accumulates a large source-attributed positive claim on
            // the thin asset (the "JELLY" the engine must not let it over-realize).
            Action::SeedSourceClaim {
                account: 0,
                asset: 0,
                claim: 100,
            },
            // Risk-increasing open by the zero-capital account: the engine draws
            // an initial-margin source-credit lien to meet IM.
            Action::Trade {
                long: 0,
                short: 1,
                asset: 0,
                size_q: 1_000_000,
                exec_price: 100,
                fee_bps: 0,
            },
            // Now lever further: a second risk-increasing add tries to draw more
            // credit against the same claim. The engine must keep the lien capped
            // by realizable backing.
            Action::Trade {
                long: 0,
                short: 1,
                asset: 0,
                size_q: 1_000_000,
                exec_price: 100,
                fee_bps: 0,
            },
        ],
    }
}

#[test]
fn jelly_campaign_never_breaks_realizability() {
    let s = jelly_scenario();
    let trace = run(&s);

    // The campaign must actually populate a source-credit lien, else this test
    // would be vacuous. Require at least one observed non-zero liened claim.
    let any_lien = trace.observations.iter().any(|obs| {
        obs.accounts
            .iter()
            .flat_map(|a| a.domains.iter())
            .any(|d: &DomainObs| d.source_claim_liened_num != 0)
    });
    assert!(
        any_lien,
        "JELLY campaign produced no source-credit lien — it would test O1 \
         vacuously. Step results: {:?}",
        trace
            .observations
            .iter()
            .map(|o| (o.step, &o.result))
            .collect::<Vec<_>>()
    );

    for obs in &trace.observations {
        check_observation(obs)
            .unwrap_or_else(|v| panic!("JELLY broke O1 at step {}: {}", obs.step, v.detail));
    }
}
