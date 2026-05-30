use perp_adversary::driver::run;
use perp_adversary::oracles::check_observation;
use perp_adversary::runner::{first_violation, OracleFn};
use perp_adversary::scenario::{Action, Scenario};
use proptest::prelude::*;

#[test]
fn synthetic_oracle_flags_the_offending_step() {
    // Synthetic oracle that "fires" on the first Withdraw observation.
    let oracle: OracleFn = |obs| {
        if matches!(obs.action, Action::Withdraw { .. }) {
            Err("synthetic".to_string())
        } else {
            Ok(())
        }
    };
    let s = Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            Action::Deposit {
                account: 0,
                amount: 10,
            },
            Action::Withdraw {
                account: 0,
                amount: 5,
            },
        ],
    };
    let v = first_violation(&s, oracle).expect("should detect the planted violation");
    assert_eq!(v.step, 1);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]
    #[test]
    fn engine_holds_realizability_under_random_lien_campaigns(
        // Source-credit claim (unscaled atoms) seeded on account 0.
        claims in prop::collection::vec(1u128..500, 1..4),
        // Position sizes (in POS_SCALE units) for the lien-drawing trades. Bounded
        // so the resulting initial margin (size * activation_price(100) / POS_SCALE)
        // can be drawn against the seeded claim for most cases — but deliberately
        // also ranges high enough to exercise the cap / rejection path.
        sizes in prop::collection::vec(100_000u128..2_000_000, 1..4),
    ) {
        // Activation price is 100 and POS_SCALE is 1_000_000 (see driver). The
        // market group starts with the activation price, so trading at 100 incurs
        // no target/effective lag and the risk-increasing trade can draw a lien.
        let mut actions = vec![
            Action::Deposit { account: 1, amount: 1_000_000_000 },
        ];
        for (claim, sz) in claims.iter().zip(sizes.iter()) {
            // Each round: top up account 0's source claim, then a risk-increasing
            // open that forces the engine to draw an initial-margin source-credit
            // lien against it.
            actions.push(Action::SeedSourceClaim { account: 0, asset: 0, claim: *claim });
            actions.push(Action::Trade { long: 0, short: 1, asset: 0, size_q: *sz, exec_price: 100, fee_bps: 0 });
        }
        let s = Scenario { n_markets: 1, n_accounts: 2, actions };

        // Sanity: the campaign must actually populate a lien, so the property is
        // not vacuous. (A given random draw may have every trade rejected for
        // insufficient backing; that is fine — it still exercises the engine —
        // but across the whole proptest run the anti_vacuity gate guarantees the
        // lien path is reachable.)
        let trace = run(&s);
        for obs in &trace.observations {
            if let Err(v) = check_observation(obs) {
                // A candidate violation: the engine's own lien failed O1. Persist
                // the scenario for replay and surface it loudly — do NOT weaken.
                let _ = std::fs::create_dir_all("scenarios");
                let path = "scenarios/realizability_candidate.json";
                let _ = std::fs::write(path, serde_json::to_string_pretty(&s).unwrap());
                panic!("REALIZABILITY CANDIDATE at step {}: {} :: saved {} :: scenario={}",
                    obs.step, v.detail, path, serde_json::to_string(&s).unwrap());
            }
        }
    }
}
