use perp_adversary::scenario::{Action, Scenario};

#[test]
fn scenario_roundtrips_through_json() {
    let s = Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            Action::Deposit {
                account: 0,
                amount: 1_000,
            },
            Action::Trade {
                long: 0,
                short: 1,
                asset: 0,
                size_q: 10,
                exec_price: 100,
                fee_bps: 0,
            },
            Action::MovePrice {
                asset: 0,
                now_slot: 1,
                effective_price: 150,
            },
            Action::Withdraw {
                account: 0,
                amount: 500,
            },
        ],
    };
    let j = serde_json::to_string(&s).unwrap();
    let back: Scenario = serde_json::from_str(&j).unwrap();
    assert_eq!(s, back);
}
