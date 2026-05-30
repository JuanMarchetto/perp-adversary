use perp_adversary::report::render_markdown;
use perp_adversary::runner::StepViolation;
use perp_adversary::scenario::{Action, Scenario};

#[test]
fn report_names_step_and_repro() {
    let s = Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![Action::Deposit {
            account: 0,
            amount: 1,
        }],
    };
    let v = StepViolation {
        step: 0,
        detail: "effective_reserved(10) exceeds realizable backing".into(),
    };
    let md = render_markdown(&v, &s);
    assert!(md.contains("step 0"));
    assert!(md.contains("realizable backing"));
    assert!(md.contains("replay"));
}
