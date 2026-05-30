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

#[test]
fn matched_trade_opens_both_legs() {
    // Open a matched 1.0 long/short, then walk the price up via the engine's
    // position-aware crank. A flat fee-free fill has no immediate observable
    // effect (the config forbids trading fees, so a fee delta is unobservable),
    // so we assert the *directional* economic effect the fill creates: once the
    // price rises, the LONG books positive PnL and the SHORT's capital is debited
    // by its realized loss. Neither happens unless both legs actually opened.
    let s = Scenario {
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
                size_q: 1_000_000,
                exec_price: 100,
                fee_bps: 0,
            },
            // Walk the price up; cranks settle then accrue, so PnL books on the
            // second successful crank. Interleave both legs each slot. (The slot-1
            // crank reports NonProgress, which is expected and harmless.)
            Action::Crank {
                account: 0,
                asset: 0,
                now_slot: 1,
                effective_price: 150,
            },
            Action::Crank {
                account: 1,
                asset: 0,
                now_slot: 1,
                effective_price: 150,
            },
            Action::Crank {
                account: 0,
                asset: 0,
                now_slot: 2,
                effective_price: 180,
            },
            Action::Crank {
                account: 1,
                asset: 0,
                now_slot: 2,
                effective_price: 180,
            },
            Action::Crank {
                account: 0,
                asset: 0,
                now_slot: 3,
                effective_price: 200,
            },
            Action::Crank {
                account: 1,
                asset: 0,
                now_slot: 3,
                effective_price: 200,
            },
        ],
    };
    let t = run(&s);

    // The fill itself must succeed.
    let fill = &t.observations[2];
    assert!(
        fill.result.is_ok(),
        "funded matched trade should fill: {:?}",
        fill.result
    );

    // Observable economic effect of the fill: after the price rises, the long
    // (account 0) has STRICTLY POSITIVE PnL — it cannot profit without an open
    // long leg.
    let last = t.observations.last().unwrap();
    assert!(
        last.accounts[0].pnl > 0,
        "long leg should book positive PnL after the price rises (got pnl={}); \
         a fill that opened no position cannot profit",
        last.accounts[0].pnl
    );

    // And the matching short (account 1) had real exposure: its capital was
    // debited below the deposited 1_000_000 as its loss was realized.
    assert!(
        last.accounts[1].capital < 1_000_000,
        "short leg should have capital debited by its realized loss (got \
         capital={}); a fill that opened no position would leave it at 1_000_000",
        last.accounts[1].capital
    );
}
