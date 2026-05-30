//! Engine adapter. The ONLY module that imports `percolator`.
//!
//! Executes a [`Scenario`] against the real Percolator perps engine and records
//! a [`Trace`] of post-step [`Observation`]s. Account observable state is read
//! only AFTER the engine views (which mutably borrow the accounts) are dropped.
use crate::scenario::{Action, Scenario};
use percolator::v16::{
    v16_domain_count_for_market_slots, EngineAssetSlotV16Account, LiquidationRequestV16, Market,
    MarketGroupV16HeaderAccount, MarketGroupV16ViewMut, PermissionlessCrankActionV16,
    PermissionlessCrankRequestV16, PortfolioAccountV16Account, PortfolioSourceDomainV16Account,
    PortfolioV16ViewMut, ProvenanceHeaderV16, ProvenanceHeaderV16Account,
    SourceCreditStateV16Account, TradeRequestV16, V16Config, V16Error, V16PodI128, V16PodU128,
    V16PodU64,
};

/// `BOUND_SCALE` from `percolator-ref/src/lib.rs:25`. Claim/backing "num" fields
/// are amounts scaled by this; `*_effective_reserved` are unscaled atoms.
const BOUND_SCALE: u128 = 1_000_000_000_000;

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

/// Observable MARKET-ENGINE source-credit state, mirroring `SourceCreditStateV16`
/// (`percolator-ref/src/v16.rs:1804`). Distinct from the per-account
/// [`DomainObs`]: this lives on a market asset's engine slot
/// (`Market::engine.source_credit_long/short`), read via
/// `SourceCreditStateV16Account::try_to_runtime`. The v0.1 market-engine oracle
/// is defined over these fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineDomainObs {
    pub positive_claim_bound_num: u128,
    pub exact_positive_claim_num: u128,
    pub fresh_reserved_backing_num: u128,
    pub spent_backing_num: u128,
    pub provider_receivable_num: u128,
    pub valid_liened_backing_num: u128,
    pub impaired_liened_backing_num: u128,
    pub insurance_credit_reserved_num: u128,
    pub valid_liened_insurance_num: u128,
    pub impaired_liened_insurance_num: u128,
    pub credit_rate_num: u128,
    pub credit_epoch: u64,
}

/// One market-engine source-credit observation: the state of `asset`'s engine
/// slot on a given `side` (long = 0, short = 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarketSideObs {
    pub asset: usize,
    pub side: u8,
    pub state: EngineDomainObs,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub step: usize,
    pub action: Action,
    pub result: Result<(), String>,
    pub accounts: Vec<AccountObs>,
    /// Market-engine source-credit state per asset, both sides (long, short).
    pub market_domains: Vec<MarketSideObs>,
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

    /// Read post-step MARKET-ENGINE source-credit state, per asset and per side
    /// (long = 0, short = 1). Each market's engine slot
    /// (`Market::engine`, the same `EngineAssetSlotV16Account` the engine reaches
    /// via `engine_slot()`) carries `source_credit_long`/`source_credit_short`;
    /// each is decoded with `SourceCreditStateV16Account::try_to_runtime`. MUST be
    /// called after all engine views are dropped (same discipline as
    /// [`Engine::observe`]).
    fn observe_markets(&self) -> Vec<MarketSideObs> {
        let mut out = Vec::with_capacity(self.mk.len() * 2);
        for (asset, market) in self.mk.iter().enumerate() {
            // `market.engine` is the engine slot (`engine_slot()` returns
            // `&self.engine`); both fields are public on the engine slot.
            let slot = &market.engine;
            out.push(MarketSideObs {
                asset,
                side: 0,
                state: read_source_credit(&slot.source_credit_long),
            });
            out.push(MarketSideObs {
                asset,
                side: 1,
                state: read_source_credit(&slot.source_credit_short),
            });
        }
        out
    }
}

/// Decode a market-engine `SourceCreditStateV16Account` into its observable
/// fields. If the engine's static validator rejects the decoded state (which it
/// should never do for a state the engine itself produced), fall back to the
/// zero state — observation must never panic.
fn read_source_credit(s: &SourceCreditStateV16Account) -> EngineDomainObs {
    match s.try_to_runtime() {
        Ok(v) => EngineDomainObs {
            positive_claim_bound_num: v.positive_claim_bound_num,
            exact_positive_claim_num: v.exact_positive_claim_num,
            fresh_reserved_backing_num: v.fresh_reserved_backing_num,
            spent_backing_num: v.spent_backing_num,
            provider_receivable_num: v.provider_receivable_num,
            valid_liened_backing_num: v.valid_liened_backing_num,
            impaired_liened_backing_num: v.impaired_liened_backing_num,
            insurance_credit_reserved_num: v.insurance_credit_reserved_num,
            valid_liened_insurance_num: v.valid_liened_insurance_num,
            impaired_liened_insurance_num: v.impaired_liened_insurance_num,
            credit_rate_num: v.credit_rate_num,
            credit_epoch: v.credit_epoch,
        },
        Err(_) => EngineDomainObs::default(),
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
        Action::Crank {
            account,
            asset,
            now_slot,
            effective_price,
        } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            // The engine's position-aware price-progression path: Refresh first
            // certifies the account (booking the open leg's favourable K-delta as
            // source-attributed positive PnL), then `permissionless_crank` computes
            // `protective_progress` and accrues the asset. This is exactly what the
            // engine's own fuzz/spec tests do (v16.rs:7974-8021).
            let req = PermissionlessCrankRequestV16 {
                now_slot,
                asset_index: asset as usize,
                effective_price,
                funding_rate_e9: 0,
                action: PermissionlessCrankActionV16::Refresh,
            };
            mv.permissionless_crank_not_atomic(&mut av, req)?;
            Ok(())
        }
        Action::SeedSourceClaim {
            account,
            asset,
            claim,
        } => {
            let claim_num = claim
                .checked_mul(BOUND_SCALE)
                .ok_or(V16Error::ArithmeticOverflow)?;
            let domain = (asset as usize) * 2; // long side of `asset`
                                               // 1. Market-side source-credit claim + backing, via the engine's own
                                               //    PUBLIC entrypoints (these recompute the credit rate and run the
                                               //    reservation-encumbrance + shape proofs internally).
            {
                let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
                mv.add_source_positive_claim_bound_not_atomic(domain, claim_num, claim_num)?;
                // expiry must exceed current_slot; current_slot is 0 here.
                mv.add_fresh_counterparty_backing_not_atomic(domain, claim_num, u64::MAX)?;
            }
            // 2. Per-account positive, source-attributed PnL + claim. This is the
            //    only field-level seed (no public account-claim setter exists);
            //    it mirrors the engine's own `account_fixture` seeding in
            //    `v16_risk_increasing_trade_creates_source_credit_lien_for_im`.
            let acct = &mut eng.accts[account as usize];
            let claim_i128 = i128::try_from(claim).map_err(|_| V16Error::ArithmeticOverflow)?;
            acct.pnl = V16PodI128::new(acct.pnl.get().saturating_add(claim_i128));
            let market_id = eng.mk[asset as usize].engine.asset.market_id.get();
            let dom = &mut eng.domains[account as usize][domain];
            dom.source_claim_market_id = V16PodU64::new(market_id);
            dom.source_claim_bound_num =
                V16PodU128::new(dom.source_claim_bound_num.get().saturating_add(claim_num));
            // 3. Group-level positive-PnL totals kept consistent with the claim.
            eng.mh.pnl_pos_tot = V16PodU128::new(eng.mh.pnl_pos_tot.get().saturating_add(claim));
            eng.mh.pnl_pos_bound_tot_num =
                V16PodU128::new(eng.mh.pnl_pos_bound_tot_num.get().saturating_add(claim_num));
            eng.mh.pnl_pos_bound_tot =
                V16PodU128::new(eng.mh.pnl_pos_bound_tot.get().saturating_add(claim));
            // 4. Ask the engine to VALIDATE the resulting account/market state, so
            //    we only ever observe a state the engine itself accepts.
            {
                let mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
                mv.validate_shape()?;
                let av = PortfolioV16ViewMut::new(
                    &mut eng.accts[account as usize],
                    &mut eng.domains[account as usize],
                );
                av.validate_with_market(&mv.as_view())?;
            }
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

/// A campaign that drives the engine into a state with a NON-ZERO
/// source-credit lien, so the O1 oracle's realizability machinery is actually
/// exercised (see `tests/anti_vacuity.rs`).
///
/// The engine creates a source-credit lien ONLY inside `execute_trade`'s
/// `create_initial_margin_source_lien_if_needed` (`v16.rs:10260`), and only when
/// a *risk-increasing* trade is performed by an account that already holds
/// positive, source-attributed PnL backed by counterparty backing — AND there
/// is no asset target/effective price lag (`v16.rs:8557` rejects every
/// risk-increasing trade while a lag exists). In engine rev `71c9032` accrue
/// never re-authenticates `raw_oracle_target_price`, so a pure price walk that
/// creates the PnL also creates a permanent lag that blocks the lien-drawing
/// trade. The legitimate, engine-accepted way to reach the lien state is
/// therefore to establish the same precondition the engine's OWN conformance
/// test builds (see [`Action::SeedSourceClaim`]) and then trade at the
/// no-lag activation price — exactly what
/// `v16_risk_increasing_trade_creates_source_credit_lien_for_im` does.
pub fn lien_creating_campaign() -> Scenario {
    use crate::scenario::Action::*;
    // Market activates at price 100 (see `Engine::new`); trade at 100 ⇒ no lag.
    // Long opens 1.0 position (notional 100, IM 100 at 100% IM) with ZERO
    // capital, so the whole IM must be drawn from the seeded source credit ⇒ a
    // lien of 100 atoms. The seeded claim (100) exactly backs it.
    Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            // Counterparty funds the short leg of the opening trade.
            Deposit {
                account: 1,
                amount: 1_000_000,
            },
            // Establish account 0's positive source-attributed PnL + the
            // matching counterparty backing on asset 0's long domain.
            SeedSourceClaim {
                account: 0,
                asset: 0,
                claim: 100,
            },
            // Risk-increasing open by the (zero-capital) long: the engine must
            // draw an initial-margin source-credit lien to meet IM.
            Trade {
                long: 0,
                short: 1,
                asset: 0,
                size_q: 1_000_000,
                exec_price: 100,
                fee_bps: 0,
            },
        ],
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
        // Views from apply() are out of scope here; safe to read account and
        // market-engine state.
        let accounts = eng.observe();
        let market_domains = eng.observe_markets();
        observations.push(Observation {
            step,
            action: *action,
            result,
            accounts,
            market_domains,
        });
    }

    Trace {
        observations,
        external_in,
        external_out,
    }
}
