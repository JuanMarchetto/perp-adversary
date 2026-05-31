//! v0.6 — funding claimable-value conservation: the CANDIDATE red.
//!
//! Funding is economically a zero-sum transfer, so total claimable value
//! `M = Σ (capital + pnl)` over all accounts MUST be invariant under any funding
//! settlement (no external flow). The engine breaks this for a FRACTIONAL-basis
//! position: the receiver leg credits PnL by `⌊x⌋` while the payer leg debits
//! capital by `⌈x⌉` (the floor/ceil asymmetry of `floor_div_signed_conservative_i128`,
//! the two legs settling in SEPARATE cranks), destroying exactly one quote atom of
//! claimable value per settled slot — to unattributable vault slack, with no req#14
//! sink, undetected (`StockReconciliationProofV16` is dead). See `oracles.rs` v0.6.
//!
//! These tests (a) confirm the campaign is NON-VACUOUS (funding really settles and
//! the vault stays flat, so v0.5 vault conservation cannot see it), and (b) ISOLATE
//! the permanent, length-proportional LEAK from the benign, constant settlement-lag
//! by differencing the claimable shortfall across campaign lengths and across
//! clean-vs-fractional bases. The leak is the signal; the lag is not. The receiver
//! domain is backed (a public provider op) so the long books its full floored gain
//! every slot and the leak grows WITHOUT BOUND.
use perp_adversary::driver::{funding_leak_campaign, run};
use perp_adversary::oracles::{
    claimable_shortfall, claimable_value, funding_conservation_kind, FundingConservationKind,
};
use perp_adversary::scenario::Action;

const POS_SCALE: u128 = 1_000_000;
const CLEAN: u128 = 100 * POS_SCALE; // whole-multiple basis: no per-leg remainder
const FRAC: u128 = 100 * POS_SCALE + 1; // fractional basis: nonzero remainder => leak

/// Shortfall = net external value that entered minus claimable value present at the
/// end (system starts empty, so conservation ⇒ shortfall == 0). Positive = atoms of
/// claimable value destroyed (the permanent leak plus the bounded in-flight lag).
fn shortfall(size_q: u128, slots: u64) -> i128 {
    let trace = run(&funding_leak_campaign(size_q, slots));
    claimable_shortfall(&trace.observations).2
}

/// How many slots actually settled funding into the long (the receiver gained PnL).
fn settled_slots(size_q: u128, slots: u64) -> i128 {
    let trace = run(&funding_leak_campaign(size_q, slots));
    let mut prev = 0i128;
    let mut n = 0i128;
    for o in &trace.observations {
        if matches!(o.action, Action::AccrueFunding { account: 0, .. }) {
            let pnl0 = o.accounts[0].pnl;
            if pnl0 > prev {
                n += 1;
            }
            prev = pnl0;
        }
    }
    n
}

#[test]
fn funding_campaign_is_non_vacuous() {
    // The fractional campaign must REACH a real funding settlement: the long must
    // gain strictly positive PnL on many slots, every crank must succeed (the backed
    // receiver never haircuts/locks), and the vault must stay FLAT — otherwise the
    // claimable-conservation check would be vacuous or confounded.
    let trace = run(&funding_leak_campaign(FRAC, 30));
    let end = trace.observations.last().unwrap();
    assert!(end.accounts[0].pnl > 0, "long must earn funding PnL; got {}", end.accounts[0].pnl);
    assert!(
        settled_slots(FRAC, 30) >= 25,
        "campaign must settle funding on most slots (backed, non-vacuous)"
    );
    assert!(
        trace.observations.iter().all(|o| o.result.is_ok()),
        "every step must succeed (no LockActive/haircut regime) so the signal is clean"
    );
    let post_trade = trace
        .observations
        .iter()
        .find(|o| matches!(o.action, Action::Trade { .. }))
        .unwrap();
    assert_eq!(
        post_trade.system.vault, end.system.vault,
        "vault must stay flat across funding: the leak hides in claim accounting, NOT the vault \
         (so v0.5 vault conservation stays green — this is a genuinely distinct invariant)"
    );
}

#[test]
fn clean_basis_conserves_claimable_value() {
    // A whole-multiple basis has NO per-leg rounding remainder, so claimable value is
    // conserved up to the CONSTANT in-flight settlement lag — which does NOT grow with
    // campaign length. shortfall(short) == shortfall(long).
    let (n1, n2) = (10u64, 30u64);
    let (s1, s2) = (shortfall(CLEAN, n1), shortfall(CLEAN, n2));
    println!("clean: shortfall({n1})={s1}  shortfall({n2})={s2}  (constant lag => equal, no leak)");
    assert_eq!(
        s1, s2,
        "clean basis: claimable shortfall must be a CONSTANT lag independent of length (no leak)"
    );
}

#[test]
fn fractional_basis_leaks_one_atom_per_settled_slot() {
    // A fractional basis destroys exactly 1 atom of claimable value per settled slot.
    // Differencing two lengths cancels the constant lag and exposes the pure leak.
    let (n1, n2) = (10u64, 30u64);
    let (s1, s2) = (shortfall(FRAC, n1), shortfall(FRAC, n2));
    let extra_settled = settled_slots(FRAC, n2) - settled_slots(FRAC, n1);
    let leaked = s2 - s1;
    println!("fractional: shortfall({n1})={s1}  shortfall({n2})={s2}  leaked over {extra_settled} extra settled slots = {leaked}");
    assert!(
        leaked > 0,
        "fractional basis: claimable value must LEAK (shortfall must grow with length): {leaked}"
    );
    assert_eq!(
        leaked, extra_settled,
        "leak must be exactly ONE atom per added settled slot (the floor/ceil remainder)"
    );
    // The leak is unbounded: the backed receiver settles every added slot.
    assert!(extra_settled >= 18, "the backed receiver must settle every added slot (unbounded growth)");
}

#[test]
fn oracle_flags_the_fractional_leak_as_destroyed() {
    // The pure oracle core, applied over the whole-campaign window (empty system ->
    // end), must classify the fractional campaign as claimable value DESTROYED.
    let frac = run(&funding_leak_campaign(FRAC, 30));
    let (net_ext, m_final, s) = claimable_shortfall(&frac.observations);
    assert!(s > 0, "fractional campaign must show a positive shortfall, got {s}");
    // System starts empty (M_initial == 0); conservation requires M_final == net_ext.
    let verdict = funding_conservation_kind(0, m_final, net_ext as u128, 0);
    assert_eq!(
        verdict,
        Err(FundingConservationKind::ClaimableValueDestroyed),
        "oracle must flag the fractional funding campaign as claimable-value DESTROYED \
         (net_ext={net_ext}, m_final={m_final}, shortfall={s})"
    );
    assert_eq!(claimable_value(frac.observations.last().unwrap()), m_final);

    // And the engine NEVER detects it: across the INTERNAL funding steps (no external
    // flow) the vault is constant, so v0.5 vault conservation holds on every funding
    // step — this leak is invisible to the only live global value check. (Deposits
    // legitimately raise the vault as external inflow; we check the funding steps.)
    let post_trade_vault = frac
        .observations
        .iter()
        .find(|o| matches!(o.action, Action::Trade { .. }))
        .unwrap()
        .system
        .vault;
    assert!(
        frac.observations
            .iter()
            .filter(|o| matches!(o.action, Action::AccrueFunding { .. }))
            .all(|o| o.system.vault == post_trade_vault && o.ext_in_step == 0 && o.ext_out_step == 0),
        "vault is flat with zero external flow on every funding step — the leak is invisible to \
         v0.5 vault conservation"
    );
}
