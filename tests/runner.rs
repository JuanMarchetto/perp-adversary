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
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]
    #[test]
    fn engine_holds_realizability_under_random_campaigns(
        sizes in prop::collection::vec(1u128..1_000, 1..8),
        prices in prop::collection::vec(50u64..200, 1..8),
    ) {
        let mut actions = vec![
            Action::Deposit { account: 0, amount: 10_000_000 },
            Action::Deposit { account: 1, amount: 10_000_000 },
        ];
        for (i, (sz, px)) in sizes.iter().zip(prices.iter()).enumerate() {
            actions.push(Action::Trade { long: 0, short: 1, asset: 0, size_q: *sz, exec_price: *px, fee_bps: 0 });
            actions.push(Action::MovePrice { asset: 0, now_slot: (i as u64) + 1, effective_price: *px });
        }
        let s = Scenario { n_markets: 1, n_accounts: 2, actions };
        let oracle: OracleFn = |obs| check_observation(obs).map_err(|v| v.detail);
        if let Some(v) = first_violation(&s, oracle) {
            panic!("REALIZABILITY CANDIDATE at step {}: {} :: scenario={}",
                v.step, v.detail, serde_json::to_string(&s).unwrap());
        }
    }
}
