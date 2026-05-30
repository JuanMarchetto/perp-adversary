//! Engine adapter. The ONLY module that imports `percolator`.
//!
//! Executes a [`Scenario`] against the real Percolator perps engine and records
//! a [`Trace`] of post-step [`Observation`]s. Account observable state is read
//! only AFTER the engine views (which mutably borrow the accounts) are dropped.
use crate::scenario::{Action, Scenario};
use percolator::v16::{
    v16_domain_count_for_market_slots, EngineAssetSlotV16Account, LiquidationRequestV16, Market,
    MarketGroupV16HeaderAccount, MarketGroupV16ViewMut, PortfolioAccountV16Account,
    PortfolioSourceDomainV16Account, PortfolioV16ViewMut, ProvenanceHeaderV16,
    ProvenanceHeaderV16Account, TradeRequestV16, V16Config, V16Error,
};

/// Per-source-domain observable state, read from a
/// [`PortfolioSourceDomainV16Account`] via its POD `.get()` accessors.
///
/// NOTE: the plan's draft `DomainObs` schema (positive_claim_bound_num,
/// credit_rate_num, fresh_reserved_backing_num, valid_liened_backing_num,
/// spent_backing_num) describes `SourceCreditStateV16`, which lives on the
/// market engine (`Market::engine.source_credit_long/short`), NOT on the
/// per-account source domain. The per-account domain type
/// `PortfolioSourceDomainV16Account` exposes the claim/lien fields mapped below,
/// so `DomainObs` mirrors those instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DomainObs {
    pub source_claim_market_id: u64,
    pub source_claim_bound_num: u128,
    pub source_claim_liened_num: u128,
    pub source_claim_counterparty_liened_num: u128,
    pub source_claim_insurance_liened_num: u128,
    pub source_lien_effective_reserved: u128,
    pub source_lien_counterparty_backing_num: u128,
    pub source_lien_insurance_backing_num: u128,
    pub source_claim_impaired_num: u128,
    pub source_lien_impaired_effective_reserved: u128,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountObs {
    pub capital: u128,
    pub pnl: i128,
    pub fee_credits: i128,
    pub domains: Vec<DomainObs>,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub step: usize,
    pub action: Action,
    pub result: Result<(), String>,
    pub accounts: Vec<AccountObs>,
}

#[derive(Clone, Debug)]
pub struct Trace {
    pub observations: Vec<Observation>,
    pub external_in: Vec<u128>,
    pub external_out: Vec<u128>,
}

/// Owns the real engine state: the market group (header + market slots) and the
/// per-account portfolio + source-domain storage. Views are constructed on
/// demand inside [`apply`] and dropped before [`Engine::observe`] reads state.
struct Engine {
    mh: MarketGroupV16HeaderAccount,
    mk: Vec<Market<u64>>,
    accts: Vec<PortfolioAccountV16Account>,
    domains: Vec<Vec<PortfolioSourceDomainV16Account>>,
}

impl Engine {
    fn new(n_markets: u32, n_accounts: u8) -> Self {
        let slots = n_markets;
        let init_price = 100u64;
        let cfg = V16Config::public_user_fund_with_market_slots(slots as u16, slots, 0, 10);
        let mut mh = MarketGroupV16HeaderAccount::new_dynamic([1u8; 32], cfg, slots, 0).unwrap();
        let mut mk = (0..slots)
            .map(|i| Market::new(i as u64, EngineAssetSlotV16Account::default()))
            .collect::<Vec<_>>();
        {
            let mut v = MarketGroupV16ViewMut::new(&mut mh, &mut mk);
            for i in 0..slots as usize {
                v.activate_empty_market_not_atomic(i as u32, init_price, (i + 1) as u64)
                    .unwrap();
            }
            v.validate_shape().unwrap();
        }

        let domain_count = v16_domain_count_for_market_slots(slots).unwrap();
        let mut accts = Vec::with_capacity(n_accounts as usize);
        let mut domains = Vec::with_capacity(n_accounts as usize);
        for a in 0..n_accounts {
            let seed = a.wrapping_add(1);
            let h = ProvenanceHeaderV16Account::from_runtime(&ProvenanceHeaderV16::new(
                [1u8; 32], [seed; 32], [3u8; 32],
            ));
            accts.push(PortfolioAccountV16Account::try_empty(h).unwrap());
            domains.push(vec![
                PortfolioSourceDomainV16Account::default();
                domain_count
            ]);
        }

        Engine {
            mh,
            mk,
            accts,
            domains,
        }
    }

    /// Read post-step observable account state. MUST be called after all engine
    /// views are dropped, since those views mutably borrow the accounts.
    fn observe(&self) -> Vec<AccountObs> {
        self.accts
            .iter()
            .zip(self.domains.iter())
            .map(|(acct, doms)| AccountObs {
                capital: acct.capital.get(),
                pnl: acct.pnl.get(),
                fee_credits: acct.fee_credits.get(),
                domains: doms.iter().map(read_domain).collect(),
            })
            .collect()
    }
}

/// Map a per-account source domain to its observable POD fields.
fn read_domain(d: &PortfolioSourceDomainV16Account) -> DomainObs {
    DomainObs {
        source_claim_market_id: d.source_claim_market_id.get(),
        source_claim_bound_num: d.source_claim_bound_num.get(),
        source_claim_liened_num: d.source_claim_liened_num.get(),
        source_claim_counterparty_liened_num: d.source_claim_counterparty_liened_num.get(),
        source_claim_insurance_liened_num: d.source_claim_insurance_liened_num.get(),
        source_lien_effective_reserved: d.source_lien_effective_reserved.get(),
        source_lien_counterparty_backing_num: d.source_lien_counterparty_backing_num.get(),
        source_lien_insurance_backing_num: d.source_lien_insurance_backing_num.get(),
        source_claim_impaired_num: d.source_claim_impaired_num.get(),
        source_lien_impaired_effective_reserved: d.source_lien_impaired_effective_reserved.get(),
    }
}

/// Borrow two distinct accounts (and their domain vectors) mutably at once.
/// Asserts the indices are distinct; this is the only aliasing-sensitive code.
fn disjoint_two<'a>(
    accts: &'a mut [PortfolioAccountV16Account],
    domains: &'a mut [Vec<PortfolioSourceDomainV16Account>],
    i: usize,
    j: usize,
) -> (
    &'a mut PortfolioAccountV16Account,
    &'a mut Vec<PortfolioSourceDomainV16Account>,
    &'a mut PortfolioAccountV16Account,
    &'a mut Vec<PortfolioSourceDomainV16Account>,
) {
    assert_ne!(i, j, "trade legs must be distinct accounts");
    let (low, high) = (i.min(j), i.max(j));
    let (a_lo, a_hi) = accts.split_at_mut(high);
    let (d_lo, d_hi) = domains.split_at_mut(high);
    let (acc_low, dom_low) = (&mut a_lo[low], &mut d_lo[low]);
    let (acc_high, dom_high) = (&mut a_hi[0], &mut d_hi[0]);
    if i < j {
        (acc_low, dom_low, acc_high, dom_high)
    } else {
        (acc_high, dom_high, acc_low, dom_low)
    }
}

/// Apply a single action to the engine, threading external-flow tallies.
/// Engine views constructed here are dropped before the caller observes state.
fn apply(
    eng: &mut Engine,
    action: Action,
    ext_in: &mut [u128],
    ext_out: &mut [u128],
) -> Result<(), V16Error> {
    match action {
        Action::Deposit { account, amount } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.deposit_not_atomic(&mut av, amount)?;
            ext_in[account as usize] = ext_in[account as usize].saturating_add(amount);
            Ok(())
        }
        Action::Withdraw { account, amount } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.withdraw_not_atomic(&mut av, amount)?;
            ext_out[account as usize] = ext_out[account as usize].saturating_add(amount);
            Ok(())
        }
        Action::Trade {
            long,
            short,
            asset,
            size_q,
            exec_price,
            fee_bps,
        } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let (la, ld, sa, sd) = disjoint_two(
                &mut eng.accts,
                &mut eng.domains,
                long as usize,
                short as usize,
            );
            let mut lv = PortfolioV16ViewMut::new(la, ld);
            let mut sv = PortfolioV16ViewMut::new(sa, sd);
            let req = TradeRequestV16 {
                asset_index: asset as usize,
                size_q,
                exec_price,
                fee_bps,
            };
            mv.execute_trade_with_fee_in_place_not_atomic(&mut lv, &mut sv, req)?;
            Ok(())
        }
        Action::MovePrice {
            asset,
            now_slot,
            effective_price,
        } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            mv.accrue_asset_to_not_atomic(asset as usize, now_slot, effective_price, 0, false)?;
            Ok(())
        }
        Action::Liquidate { account, asset } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            // LiquidationRequestV16 { asset_index, close_q, fee_bps } (:3105);
            // it does not derive Default, so construct it explicitly. close_q == 0
            // expresses "close the whole position" intent; the engine clamps.
            let req = LiquidationRequestV16 {
                asset_index: asset as usize,
                close_q: 0,
                fee_bps: 0,
            };
            mv.liquidate_account_not_atomic(&mut av, req)?;
            Ok(())
        }
    }
}

/// Execute a scenario, returning a [`Trace`] with one [`Observation`] per action.
pub fn run(s: &Scenario) -> Trace {
    let mut eng = Engine::new(s.n_markets, s.n_accounts);
    let n = s.n_accounts as usize;
    let mut external_in = vec![0u128; n];
    let mut external_out = vec![0u128; n];
    let mut observations = Vec::with_capacity(s.actions.len());

    for (step, action) in s.actions.iter().enumerate() {
        let result = apply(&mut eng, *action, &mut external_in, &mut external_out)
            .map_err(|e| format!("{e:?}"));
        // Views from apply() are out of scope here; safe to read account state.
        let accounts = eng.observe();
        observations.push(Observation {
            step,
            action: *action,
            result,
            accounts,
        });
    }

    Trace {
        observations,
        external_in,
        external_out,
    }
}
