use perp_adversary::driver::run;
use perp_adversary::scenario::{Action, Scenario};

#[test]
fn trace_records_capital_and_external_flows() {
    let s = Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            Action::Deposit {
                account: 0,
                amount: 1_000,
            },
            Action::Withdraw {
                account: 0,
                amount: 400,
            },
        ],
    };
    let trace = run(&s);
    assert_eq!(trace.observations.len(), 2);
    let last = trace.observations.last().unwrap();
    assert!(
        last.result.is_ok(),
        "withdraw of <= deposit should succeed: {:?}",
        last.result
    );
    assert_eq!(last.accounts[0].capital, 600);
    assert_eq!(trace.external_in[0], 1_000);
    assert_eq!(trace.external_out[0], 400);
}
