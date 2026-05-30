use perp_adversary::driver::run;
use perp_adversary::oracles::check_observation;
use perp_adversary::scenario::{Action, Scenario};

/// The JELLY archetype: open on a thin asset, walk its price up across slots,
/// then try to extract the unrealized PnL. The engine must keep usable credit
/// bounded by realizable backing at every step.
fn jelly_scenario() -> Scenario {
    Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            Action::Deposit {
                account: 0,
                amount: 1_000_000,
            },
            Action::Deposit {
                account: 1,
                amount: 1_000_000,
            },
            Action::Trade {
                long: 0,
                short: 1,
                asset: 0,
                size_q: 5_000,
                exec_price: 100,
                fee_bps: 0,
            },
            Action::MovePrice {
                asset: 0,
                now_slot: 1,
                effective_price: 140,
            },
            Action::MovePrice {
                asset: 0,
                now_slot: 2,
                effective_price: 180,
            },
            Action::Withdraw {
                account: 0,
                amount: 900_000,
            },
        ],
    }
}

#[test]
fn jelly_campaign_never_breaks_realizability() {
    let s = jelly_scenario();
    let trace = run(&s);
    for obs in &trace.observations {
        check_observation(obs)
            .unwrap_or_else(|v| panic!("JELLY broke O1 at step {}: {}", obs.step, v.detail));
    }
}
