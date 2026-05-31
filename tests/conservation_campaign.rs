//! v0.5 GLOBAL QUOTE-VALUE CONSERVATION over EVERY campaign.
//!
//! This is the PIVOT oracle of the project. Every other oracle mirrors a
//! per-state validator the engine itself runs, so a green result is vacuous as a
//! bug signal. THIS oracle checks an EMERGENT property the engine does NOT enforce
//! as a single global validator across a whole campaign: total real quote-atom
//! value (`system.vault`, the engine's `StockReconciliationProofV16.token_vault`,
//! `v16.rs:3020`) is CONSERVED — it changes only by the net external flow that
//! crossed the instance boundary on each step.
//!
//! The value model is `system_quote_value(state) == vault` (spec.md §5.1.1, line
//! 1043). Every other quote store (`c_tot == Σ capital`, `insurance`,
//! `backing_provider_earnings`, `settlement_rounding_residue_total`,
//! `unallocated_protocol_surplus`) is a PARTITION of `vault`; PnL, claims, liens,
//! backing, reservations and `payout_snapshot` are NOT value classes (spec.md:46).
//!
//! We fold `check_step_value_conservation` over EVERY consecutive `(prev, cur)`
//! pair of EVERY campaign the harness has, taking the per-step external flow
//! deltas the runner records on each observation. A green result across all
//! campaigns is a strong INDEPENDENT conservation signal. If a discrepancy ever
//! appears, the offending scenario + the exact field-by-field before/after
//! breakdown is persisted to `scenarios/conservation_candidate.json` and the test
//! fails loudly — it is a CANDIDATE to triage, NEVER a confirmed bug, and the
//! oracle is NEVER weakened to make this pass.

use perp_adversary::driver::{
    adl_campaign, earned_lien_campaign, funded_liquidation_campaign, lien_creating_campaign,
    liquidation_campaign, run, Observation,
};
use perp_adversary::oracles::check_step_value_conservation;
use perp_adversary::scenario::{Action, Scenario};

/// Fold the conservation oracle over a campaign trace. On the first discrepancy,
/// persist the scenario with a full field-by-field before/after breakdown and
/// return a loud message. `Ok(())` means conservation HELD across the campaign.
fn conservation_holds(label: &str, s: &Scenario) -> Result<(), String> {
    let trace = run(s);
    for pair in trace.observations.windows(2) {
        let (prev, cur) = (&pair[0], &pair[1]);
        if let Err(v) = check_step_value_conservation(prev, cur) {
            return Err(persist_candidate(label, s, prev, cur, &v.detail));
        }
    }
    Ok(())
}

/// Persist a conservation CANDIDATE with the exact step and field-by-field
/// before/after breakdown so it can be triaged (real leak vs incomplete model).
fn persist_candidate(
    label: &str,
    s: &Scenario,
    prev: &Observation,
    cur: &Observation,
    detail: &str,
) -> String {
    let _ = std::fs::create_dir_all("scenarios");
    let path = "scenarios/conservation_candidate.json";
    let breakdown = serde_json::json!({
        "label": label,
        "step": cur.step,
        "action": format!("{:?}", cur.action),
        "ext_in_step": cur.ext_in_step.to_string(),
        "ext_out_step": cur.ext_out_step.to_string(),
        "before": {
            "vault": prev.system.vault.to_string(),
            "c_tot": prev.system.c_tot.to_string(),
            "insurance": prev.system.insurance.to_string(),
            "backing_provider_earnings": prev.system.backing_provider_earnings.to_string(),
            "payout_snapshot": prev.system.payout_snapshot.to_string(),
            "explicit_unallocated_loss_total":
                prev.system.explicit_unallocated_loss_total.to_string(),
        },
        "after": {
            "vault": cur.system.vault.to_string(),
            "c_tot": cur.system.c_tot.to_string(),
            "insurance": cur.system.insurance.to_string(),
            "backing_provider_earnings": cur.system.backing_provider_earnings.to_string(),
            "payout_snapshot": cur.system.payout_snapshot.to_string(),
            "explicit_unallocated_loss_total":
                cur.system.explicit_unallocated_loss_total.to_string(),
        },
        "detail": detail,
        "scenario": s,
    });
    let _ = std::fs::write(path, serde_json::to_string_pretty(&breakdown).unwrap());
    format!(
        "CONSERVATION CANDIDATE [{label}] at step {}: {detail} :: saved {path} :: \
         TRIAGE before claiming a bug — real leak/mint vs incomplete value model.",
        cur.step
    )
}

#[test]
fn conservation_holds_on_lien_creating_campaign() {
    if let Err(msg) = conservation_holds("lien_creating", &lien_creating_campaign()) {
        panic!("{msg}");
    }
}

#[test]
fn conservation_holds_on_earned_lien_campaign() {
    if let Err(msg) = conservation_holds("earned_lien", &earned_lien_campaign()) {
        panic!("{msg}");
    }
}

#[test]
fn conservation_holds_on_liquidation_campaign() {
    if let Err(msg) = conservation_holds("liquidation", &liquidation_campaign()) {
        panic!("{msg}");
    }
}

#[test]
fn conservation_holds_on_funded_liquidation_campaign() {
    if let Err(msg) = conservation_holds("funded_liquidation", &funded_liquidation_campaign()) {
        panic!("{msg}");
    }
}

#[test]
fn conservation_holds_on_adl_campaign() {
    if let Err(msg) = conservation_holds("adl", &adl_campaign()) {
        panic!("{msg}");
    }
}

#[test]
fn conservation_holds_on_jelly_campaign() {
    // The JELLY archetype (mirrors tests/jelly_campaign.rs): a zero-capital account
    // draws and levers a source-credit lien. Pure internal value movement after the
    // initial deposit — every internal step must net ZERO vault change.
    let s = Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            Action::Deposit {
                account: 1,
                amount: 10_000_000,
            },
            Action::SeedSourceClaim {
                account: 0,
                asset: 0,
                claim: 100,
            },
            Action::Trade {
                long: 0,
                short: 1,
                asset: 0,
                size_q: 1_000_000,
                exec_price: 100,
                fee_bps: 0,
            },
            Action::Trade {
                long: 0,
                short: 1,
                asset: 0,
                size_q: 1_000_000,
                exec_price: 100,
                fee_bps: 0,
            },
        ],
    };
    if let Err(msg) = conservation_holds("jelly", &s) {
        panic!("{msg}");
    }
}

/// A deposit-then-withdraw campaign: the external flow path itself. The deposit
/// raises the vault by exactly the inflow; the withdraw lowers it by exactly the
/// outflow; conservation must hold at every step.
#[test]
fn conservation_holds_on_external_flow_campaign() {
    let s = Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            Action::Deposit {
                account: 0,
                amount: 1_000_000,
            },
            Action::Withdraw {
                account: 0,
                amount: 400_000,
            },
            Action::Deposit {
                account: 0,
                amount: 250_000,
            },
            Action::Withdraw {
                account: 0,
                amount: 850_000,
            },
        ],
    };
    if let Err(msg) = conservation_holds("external_flow", &s) {
        panic!("{msg}");
    }
}

/// NEGATIVE CONTROL — proves the oracle is NOT vacuous. Run a real campaign, then
/// PLANT a 1-atom value mint into one observed post-step `vault` and confirm the
/// conservation oracle CATCHES it on the spliced pair. If this ever passes, the
/// oracle would be silently inert and the "held" results above would be worthless.
#[test]
fn planted_leak_is_caught_proving_the_oracle_is_live() {
    let trace = run(&liquidation_campaign());
    // Find an internal step (no external flow) to corrupt: there the vault MUST be
    // unchanged, so a planted +1 is an unambiguous mint.
    let mut found_internal = false;
    for pair in trace.observations.windows(2) {
        let (prev, cur) = (&pair[0], &pair[1]);
        if cur.ext_in_step == 0 && cur.ext_out_step == 0 {
            found_internal = true;
            // Honest pair conserves.
            assert!(
                check_step_value_conservation(prev, cur).is_ok(),
                "honest internal step should conserve"
            );
            // Plant a 1-atom mint into the post-step vault.
            let mut tampered = cur.clone();
            tampered.system.vault = cur.system.vault.wrapping_add(1);
            assert!(
                check_step_value_conservation(prev, &tampered).is_err(),
                "oracle FAILED to catch a planted 1-atom value mint — it is vacuous!"
            );
        }
    }
    assert!(
        found_internal,
        "liquidation campaign had no internal (zero-external-flow) step to test the \
         oracle against — the negative control could not run"
    );
}
