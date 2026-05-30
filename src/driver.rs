//! Engine adapter. The ONLY module that imports `percolator`.
//!
//! Executes a [`Scenario`] against the real Percolator perps engine and records
//! a [`Trace`] of post-step [`Observation`]s. Account observable state is read
//! only AFTER the engine views (which mutably borrow the accounts) are dropped.
use crate::scenario::{Action, Scenario};
use percolator::v16::{
    v16_domain_count_for_market_slots, AssetStateV16Account, EngineAssetSlotV16Account,
    LiquidationOutcomeV16, LiquidationRequestV16, Market, MarketGroupV16HeaderAccount,
    MarketGroupV16ViewMut, PermissionlessCrankActionV16, PermissionlessCrankRequestV16,
    PortfolioAccountV16Account, PortfolioLegV16, PortfolioLegV16Account,
    PortfolioSourceDomainV16Account, PortfolioV16ViewMut, ProvenanceHeaderV16,
    ProvenanceHeaderV16Account, SideV16, SourceCreditStateV16Account, TradeRequestV16, V16Config,
    V16Error, V16PodI128, V16PodU128, V16PodU64,
};
use percolator::{ADL_ONE, POS_SCALE};

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
    /// The asset's `market_id`, read from `Market::engine.asset.market_id`
    /// (`percolator-ref/src/v16.rs:3597`). This is the value the engine compares
    /// each per-account `source_claim_market_id` against in
    /// `validate_source_credit_shape_with_market` (v16.rs:2177); surfacing it lets
    /// the v0.1 cross-link oracle compare against the REAL engine value rather
    /// than any convention.
    pub market_id: u64,
    pub state: EngineDomainObs,
    /// This side's `insurance_domain_spent_*` on the engine slot
    /// (`EngineAssetSlotV16Account::insurance_domain_spent_long/short`,
    /// `percolator-ref/src/v16.rs:3826`). A liquidation on a properly-funded
    /// domain that draws shared insurance bumps this; a liquidation that books
    /// residual instead leaves it at 0. Surfacing it gives the v0.2-B
    /// insurance-isolation oracle the real engine quantity to police.
    pub insurance_domain_spent: u128,
}

/// Observable result of a single `liquidate_account_not_atomic` call, mirroring
/// the engine's `LiquidationOutcomeV16` (`percolator-ref/src/v16.rs:3113`).
/// Present on an [`Observation`] iff that step was an [`Action::Liquidate`] the
/// engine accepted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LiquidationObs {
    pub closed_q: u128,
    pub insurance_used: u128,
    pub residual_booked: u128,
    pub explicit_loss: u128,
    pub fee_charged: u128,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub step: usize,
    pub action: Action,
    pub result: Result<(), String>,
    pub accounts: Vec<AccountObs>,
    /// Market-engine source-credit state per asset, both sides (long, short).
    pub market_domains: Vec<MarketSideObs>,
    /// The `LiquidationOutcomeV16` captured when this step was an
    /// [`Action::Liquidate`] the engine accepted; `None` otherwise.
    pub liquidation: Option<LiquidationObs>,
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
            // `asset.market_id` is the value the engine cross-checks each
            // per-account `source_claim_market_id` against (v16.rs:2177).
            let market_id = slot.asset.market_id.get();
            out.push(MarketSideObs {
                asset,
                side: 0,
                market_id,
                state: read_source_credit(&slot.source_credit_long),
                insurance_domain_spent: slot.insurance_domain_spent_long.get(),
            });
            out.push(MarketSideObs {
                asset,
                side: 1,
                market_id,
                state: read_source_credit(&slot.source_credit_short),
                insurance_domain_spent: slot.insurance_domain_spent_short.get(),
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
/// Returns the `LiquidationOutcomeV16` (as a [`LiquidationObs`]) when the action
/// is a liquidation the engine accepted, so [`run`] can record it on the step's
/// [`Observation`]; all other actions return `Ok(None)`.
fn apply(
    eng: &mut Engine,
    action: Action,
    ext_in: &mut [u128],
    ext_out: &mut [u128],
) -> Result<Option<LiquidationObs>, V16Error> {
    match action {
        Action::Deposit { account, amount } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.deposit_not_atomic(&mut av, amount)?;
            ext_in[account as usize] = ext_in[account as usize].saturating_add(amount);
            Ok(None)
        }
        Action::Withdraw { account, amount } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.withdraw_not_atomic(&mut av, amount)?;
            ext_out[account as usize] = ext_out[account as usize].saturating_add(amount);
            Ok(None)
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
            Ok(None)
        }
        Action::MovePrice {
            asset,
            now_slot,
            effective_price,
        } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            mv.accrue_asset_to_not_atomic(asset as usize, now_slot, effective_price, 0, false)?;
            Ok(None)
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
            Ok(None)
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
            Ok(None)
        }
        Action::SeedUnderwaterPosition { account, asset } => {
            seed_underwater_position(eng, account, asset, 0)?;
            Ok(None)
        }
        Action::SeedUnderwaterPositionFunded {
            account,
            asset,
            domain_budget,
        } => {
            seed_underwater_position(eng, account, asset, domain_budget)?;
            Ok(None)
        }
        Action::Liquidate {
            account,
            asset,
            close_q,
        } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            // LiquidationRequestV16 { asset_index, close_q, fee_bps } (:3105);
            // it does not derive Default, so construct it explicitly. close_q == 0
            // expresses "close the whole position" intent (the engine clamps); a
            // non-zero close_q against an underwater leg (see
            // `seed_underwater_position`) drives a real liquidation.
            let req = LiquidationRequestV16 {
                asset_index: asset as usize,
                close_q,
                fee_bps: 0,
            };
            let out: LiquidationOutcomeV16 = mv.liquidate_account_not_atomic(&mut av, req)?;
            Ok(Some(LiquidationObs {
                closed_q: out.closed_q,
                insurance_used: out.insurance_used,
                residual_booked: out.residual_booked,
                explicit_loss: out.explicit_loss,
                fee_charged: out.fee_charged,
            }))
        }
    }
}

/// Drive `account` into an engine-accepted UNDERWATER state on `asset`'s long
/// side via direct field writes, mirroring the engine's OWN conformance test
/// `v16_public_liquidation_on_unfunded_domain_cannot_drain_shared_insurance`
/// (`/tmp/percolator-ref/tests/v16_spec_tests.rs:289`).
///
/// The state: a single open long leg of `POS_SCALE`, negative account PnL, and
/// matching open-interest / loss-weight / stored-position-count totals on the
/// asset (two longs and two shorts so the account's leg is not the only OI),
/// with `vault`, `insurance` and `negative_pnl_account_count` on the group
/// header. Liquidating this on an UNFUNDED domain forces the engine to book the
/// unbacked loss as residual rather than draining shared insurance.
///
/// The leg's snapshot fields (`k_snap`, `f_snap`, `epoch_snap`, `b_snap`,
/// `b_epoch_snap`) are taken from the asset's current engine state, exactly as
/// the spec test does, so the leg is consistent with the asset and the engine's
/// `validate_with_market` accepts it. The function then runs `validate_shape` +
/// `validate_with_market`, propagating any rejection as an error so the driver
/// only ever proceeds from an engine-accepted state.
///
/// `domain_budget` (atoms) funds the SHORT-side bankruptcy insurance domain of
/// the long leg (`insurance_domain_budget_short`). With `0` the engine can draw
/// no insurance and books residual (`insurance_used == 0`); with a positive value
/// the engine genuinely SPENDS that domain's insurance on liquidation
/// (`insurance_used > 0`). The seed adds `domain_budget` to both `vault` and
/// `insurance` so that, post-funding, the group still satisfies
/// `validate_shape`'s `senior <= vault` (v16.rs:4590) and
/// `live_domain_budget_remaining_atoms <= insurance` (v16.rs:4594) invariants.
fn seed_underwater_position(
    eng: &mut Engine,
    account: u8,
    asset: u8,
    domain_budget: u128,
) -> Result<(), V16Error> {
    let ai = asset as usize;

    // Group-header bookkeeping the underwater leg requires. The base 50 mirrors
    // the engine's own conformance fixture; the extra `domain_budget` keeps the
    // funded short-side budget within `validate_shape`'s insurance ceiling.
    let extra = 50u128
        .checked_add(domain_budget)
        .ok_or(V16Error::ArithmeticOverflow)?;
    eng.mh.vault = V16PodU128::new(eng.mh.vault.get().saturating_add(extra));
    eng.mh.insurance = V16PodU128::new(eng.mh.insurance.get().saturating_add(extra));
    eng.mh.negative_pnl_account_count =
        V16PodU64::new(eng.mh.negative_pnl_account_count.get().saturating_add(1));

    // Fund the long leg's bankruptcy insurance domain: a long-leg liquidation
    // books its loss against the asset's SHORT-side insurance domain
    // (`consume_domain_insurance_for_negative_pnl` takes `opposite_side(Long)`,
    // v16.rs:5955). Setting `insurance_domain_budget_short` is exactly where the
    // engine's proof `proof_v16_view_domain_budget_caps_bankruptcy_insurance_spend`
    // (tests/proofs_v16.rs:2384) sets it. `spent` stays 0, so
    // `validate_domain_shape_for_view`'s `spent <= budget` (v16.rs:4660) holds.
    if domain_budget != 0 {
        eng.mk[ai].engine.insurance_domain_budget_short = V16PodU128::new(domain_budget);
    }

    // Asset-level open interest / loss-weight / position-count totals: two longs
    // and two shorts of POS_SCALE each (the account contributes one long leg).
    let mut asset_rt = eng.mk[ai].engine.asset.try_to_runtime()?;
    asset_rt.oi_eff_long_q = 2 * POS_SCALE;
    asset_rt.oi_eff_short_q = 2 * POS_SCALE;
    asset_rt.loss_weight_sum_long = 2 * POS_SCALE;
    asset_rt.loss_weight_sum_short = 2 * POS_SCALE;
    asset_rt.stored_pos_count_long = 2;
    asset_rt.stored_pos_count_short = 2;
    eng.mk[ai].engine.asset = AssetStateV16Account::from_runtime(&asset_rt);

    // The account: negative PnL plus one open long leg snapped to the asset.
    let acct = &mut eng.accts[account as usize];
    acct.pnl = V16PodI128::new(acct.pnl.get().saturating_add(-5));
    acct.legs[0] = PortfolioLegV16Account::from_runtime(&PortfolioLegV16 {
        active: true,
        asset_index: ai as u32,
        market_id: asset_rt.market_id,
        side: SideV16::Long,
        basis_pos_q: POS_SCALE as i128,
        a_basis: ADL_ONE,
        k_snap: asset_rt.k_long,
        f_snap: asset_rt.f_long_num,
        epoch_snap: asset_rt.epoch_long,
        loss_weight: POS_SCALE,
        b_snap: asset_rt.b_long_num,
        b_rem: 0,
        b_epoch_snap: asset_rt.epoch_long,
        b_stale: false,
        stale: false,
    });
    acct.active_bitmap[0] = V16PodU64::new(1);

    // Only proceed from a state the engine itself accepts.
    let mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
    mv.validate_shape()?;
    let av = PortfolioV16ViewMut::new(
        &mut eng.accts[account as usize],
        &mut eng.domains[account as usize],
    );
    av.validate_with_market(&mv.as_view())?;
    Ok(())
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

/// A campaign that drives a REAL liquidation: it seeds an engine-accepted
/// underwater position (via [`Action::SeedUnderwaterPosition`], which mirrors the
/// engine's own `v16_public_liquidation_on_unfunded_domain_cannot_drain_shared_insurance`
/// fixture) and then liquidates a non-zero `close_q` of it. The liquidation must
/// progress by booking residual loss (`residual_booked > 0`) without draining
/// shared insurance (`insurance_used == 0`) — the non-vacuity the
/// `tests/liquidation.rs` gate pins.
pub fn liquidation_campaign() -> Scenario {
    use crate::scenario::Action::*;
    Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            // Construct the underwater state the engine accepts.
            SeedUnderwaterPosition {
                account: 0,
                asset: 0,
            },
            // Close the full POS_SCALE long leg: a genuine liquidation that books
            // residual on the unfunded domain.
            Liquidate {
                account: 0,
                asset: 0,
                close_q: POS_SCALE,
            },
        ],
    }
}

/// A campaign that drives a REAL liquidation which GENUINELY SPENDS insurance for
/// the liquidated domain (`insurance_used > 0`). It seeds the same
/// engine-accepted underwater long position as [`liquidation_campaign`], but
/// FUNDS the bankruptcy insurance domain (the asset's short-side
/// `insurance_domain_budget_short`) via [`Action::SeedUnderwaterPositionFunded`].
///
/// On liquidation the engine draws up to that budget from shared insurance for
/// the liquidated long leg's domain — so the resulting `LiquidationObs` has
/// `insurance_used > 0` AND the asset's short-side `insurance_domain_spent`
/// increases by exactly that amount, while NO other domain's spend changes. This
/// is the STRONGER input for the insurance-isolation oracle: insurance IS spent
/// for the correct domain and NONE for any other.
///
/// The 5-atom residual loss seeded by `SeedUnderwaterPositionFunded` is fully
/// within a budget of 5, so the engine spends all 5 from insurance and books no
/// residual; this is the engine's own
/// `proof_v16_view_domain_budget_caps_bankruptcy_insurance_spend` shape
/// (`tests/proofs_v16.rs:2375`) reached through the public liquidation entrypoint.
pub fn funded_liquidation_campaign() -> Scenario {
    use crate::scenario::Action::*;
    Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            // Underwater long leg PLUS a funded short-side insurance domain budget
            // (5 atoms) — enough to cover the seeded 5-atom bankruptcy loss.
            SeedUnderwaterPositionFunded {
                account: 0,
                asset: 0,
                domain_budget: 5,
            },
            // Liquidate the full long leg: the engine spends the funded domain's
            // insurance (insurance_used > 0) for the liquidated domain only.
            Liquidate {
                account: 0,
                asset: 0,
                close_q: POS_SCALE,
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
        // `apply` returns the liquidation outcome on a successful liquidation;
        // split it from the `Result` recorded on the observation.
        let (result, liquidation) =
            match apply(&mut eng, *action, &mut external_in, &mut external_out) {
                Ok(liq) => (Ok(()), liq),
                Err(e) => (Err(format!("{e:?}")), None),
            };
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
            liquidation,
        });
    }

    Trace {
        observations,
        external_in,
        external_out,
    }
}
