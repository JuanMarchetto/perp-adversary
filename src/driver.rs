//! Engine adapter. The ONLY module that imports `percolator`.
//!
//! Executes a [`Scenario`] against the real Percolator perps engine and records
//! a [`Trace`] of post-step [`Observation`]s. Account observable state is read
//! only AFTER the engine views (which mutably borrow the accounts) are dropped.
use crate::scenario::{Action, Scenario};
use percolator::v16::{
    v16_domain_count_for_market_slots, AssetStateV16Account, CloseProgressLedgerV16,
    CloseProgressLedgerV16Account, EngineAssetSlotV16Account, LiquidationOutcomeV16,
    LiquidationRequestV16, Market, MarketGroupV16HeaderAccount, MarketGroupV16ViewMut,
    PermissionlessCrankActionV16, PermissionlessCrankRequestV16, PortfolioAccountV16Account,
    PortfolioLegV16, PortfolioLegV16Account, PortfolioSourceDomainV16Account, PortfolioV16ViewMut,
    ProvenanceHeaderV16, ProvenanceHeaderV16Account, QuantityAdlOutcomeV16, SideV16,
    SourceCreditStateV16Account, TradeRequestV16, V16Config, V16Error, V16PodI128, V16PodU128,
    V16PodU64,
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
    /// The account's `close_progress.quantity_adl_applied_q` ledger value
    /// (`CloseProgressLedgerV16Account::quantity_adl_applied_q`,
    /// `percolator-ref/src/v16.rs:11920`), read straight from the engine slot via
    /// `.get()` exactly as the engine reads it (`v16.rs:2134`). The engine's ADL
    /// entrypoint advances this to the closed quantity inside
    /// `advance_close_progress_quantity_adl` (`v16.rs:9533`). Surfacing it on EVERY
    /// observation (not just the ADL step) lets the v0.3-B accounting oracle
    /// compare the value BEFORE and AFTER an `ApplyAdl` step — the cross-step delta
    /// it polices. On a non-ADL state this is `0`.
    pub quantity_adl_applied_q: u128,
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

/// Observable result of a single
/// `apply_quantity_adl_after_residual_for_account_not_atomic` call, mirroring the
/// engine's `QuantityAdlOutcomeV16` (`percolator-ref/src/v16.rs:3178`). Present on
/// an [`Observation`] iff that step was an [`Action::ApplyAdl`] the engine
/// accepted.
///
/// `quantity_adl_applied_q` is the account's post-ADL
/// `close_progress.quantity_adl_applied_q` (read via
/// `CloseProgressLedgerV16Account::try_to_runtime`): the engine advances this to
/// the closed quantity inside `advance_close_progress_quantity_adl`
/// (`v16.rs:9517`), so the v0.3-B accounting oracle can police that the ledger's
/// applied quantity equals the outcome's `closed_q`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdlObs {
    pub closed_q: u128,
    pub opposite_a_after: u128,
    pub reset_started: bool,
    pub quantity_adl_applied_q: u128,
}

/// GROUP-HEADER quote-atom stores, read from the `MarketGroupV16HeaderAccount`
/// (`percolator-ref/src/v16.rs:4057`) AFTER all engine views are dropped. These
/// are the fields the v0.5 GLOBAL QUOTE-VALUE CONSERVATION oracle reasons over.
///
/// The authoritative quote-atom value model is the engine's own stock
/// reconciliation (`spec.md` §5.1.1, line 1043; `StockReconciliationProofV16`,
/// `v16.rs:3019`): the TOTAL real quote-atom balance held by the instance is the
/// token vault, and it equals the sum of all partition stock classes
/// (`token_vault == senior_capital_total + insurance_capital +
/// backing_provider_earnings + settlement_rounding_residue_total +
/// unallocated_protocol_surplus`, `v16.rs:3029-3041`). So `vault` IS
/// `system_quote_value` — a single source-of-truth total, summed without any
/// double-count. The remaining fields are surfaced for the oracle's partition
/// CROSS-CHECK and for triage, not added on top of `vault`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemObs {
    /// `header.vault` (`v16.rs:4061`) — the TOTAL real quote-atom balance of the
    /// instance (the engine's `StockReconciliationProofV16.token_vault`,
    /// `v16.rs:3020`). This is `system_quote_value(state)`: every other quote
    /// store is a PARTITION of this total, so summing the parts would
    /// double-count. The engine's `TokenValueFlowProof::validate` (`v16.rs:2913`)
    /// proves `vault` changes by EXACTLY net external flow on every value-moving
    /// instruction; the v0.5 oracle checks the EMERGENT composition of that across
    /// a whole campaign at the harness level.
    pub vault: u128,
    /// `header.insurance` (`v16.rs:4062`) — `InsuranceCapital`, a partition class
    /// of `vault` (`v16.rs:3022`). Surfaced for the partition cross-check, NOT
    /// added on top of `vault`.
    pub insurance: u128,
    /// `header.c_tot` (`v16.rs:4063`) — `C_tot == sum(C_i)` (`spec.md:1072`): a
    /// DERIVED aggregate of every account's `capital`, i.e. `SeniorCapital`, a
    /// partition class of `vault`. It is NOT an independent store and is NOT added
    /// on top of `vault` (doing so would double-count the per-account capital that
    /// already sits inside `vault`).
    pub c_tot: u128,
    /// `header.payout_snapshot` (`v16.rs:4087`) — a SNAPSHOT of resolved-payout
    /// entitlement captured at resolution, NOT a separate quote-atom store. It is
    /// claim accounting (a record of what is owed), so it is observed for triage
    /// but DELIBERATELY EXCLUDED from `system_quote_value`.
    pub payout_snapshot: u128,
    /// Sum over all asset slots of
    /// `EngineAssetSlotV16Account::explicit_unallocated_loss_long/short`
    /// (`v16.rs:3709`). This is LOSS ACCOUNTING — a record of loss that could not
    /// be allocated — not a quote-atom store; the corresponding value class is
    /// `ExplicitBackedLoss` (`spec.md:949`), tracked by `explicit_backed_loss_-
    /// reserve_total`, which this simplified header does not separately carry.
    /// Booking such loss does NOT move `vault`. Observed for triage, EXCLUDED from
    /// the sum.
    pub explicit_unallocated_loss_total: u128,
    /// Sum over all asset slots of both sides'
    /// `BackingDomainV16Account::utilization_fee_earnings` — the engine's
    /// `backing_provider_earnings` stock class (`v16.rs:4508-4517`,
    /// `StockReconciliationProofV16.backing_provider_earnings`, `v16.rs:3023`). A
    /// partition class of `vault`. Surfaced for the partition cross-check only.
    pub backing_provider_earnings: u128,
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
    /// The `QuantityAdlOutcomeV16` captured when this step was an
    /// [`Action::ApplyAdl`] the engine accepted; `None` otherwise.
    pub adl: Option<AdlObs>,
    /// GROUP-HEADER quote-atom stores for this post-step state (see [`SystemObs`]).
    /// The v0.5 global quote-value conservation oracle reads `system.vault` here.
    pub system: SystemObs,
    /// The TOTAL external quote that flowed IN to the instance ON THIS STEP
    /// (`deposit_not_atomic`, `v16.rs:11451`), i.e. the per-step delta of the
    /// runner's cumulative `external_in`. `0` for any non-deposit step.
    pub ext_in_step: u128,
    /// The TOTAL external quote that flowed OUT of the instance ON THIS STEP
    /// (`withdraw_not_atomic`, `v16.rs:11174`, and resolved-payout claims), i.e.
    /// the per-step delta of the runner's cumulative `external_out`. `0` for any
    /// non-withdraw step.
    pub ext_out_step: u128,
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

/// The market's activation/target price (see [`Engine::new`]). Earned-state
/// funding cranks MUST keep `effective_price == ACTIVATION_PRICE` so there is no
/// target/effective lag to block the risk-increasing lien-drawing trade.
pub const ACTIVATION_PRICE: u64 = 100;

/// Build the market-group config. The default is the canonical public-user-fund
/// config (`public_user_fund_with_market_slots`, funding OFF —
/// `max_abs_funding_e9_per_slot == 0`), used by every seeded campaign.
///
/// `funding_enabled` switches to a config that PERMITS funding accrual, required
/// by the v0.4 earned-lien campaign: the earned positive PnL is settled from a
/// funding K/F delta, which `accrue_asset_to_not_atomic` rejects
/// (`InvalidConfig`, `v16.rs:7833`) unless `max_abs_funding_e9_per_slot > 0`. The
/// funding-enabled config keeps `maintenance_margin_bps == initial_margin_bps ==
/// 10_000` and shifts the price-move budget down by one bps so the engine's exact
/// solvency envelope (`validate_exact_solvency_envelope`, `v16.rs:1430`) still
/// accepts it on its branch-2 path (`loss_budget_bps_ceil == 10_000`,
/// `v16.rs:1486`): with `max_accrual_dt_slots == 1` and
/// `max_abs_funding_e9_per_slot == 10_000`, the funding budget rounds up to 1 bps,
/// so `max_price_move_bps_per_slot == 9_999` keeps the total loss budget at exactly
/// 10_000 bps. The resulting config passes `validate_public_user_fund`
/// (`v16.rs:1676`), so `MarketGroupV16HeaderAccount::new_dynamic` accepts it — it
/// is a legitimate funded market, not a weakened one.
fn engine_config(slots: u32, funding_enabled: bool) -> V16Config {
    let mut cfg = V16Config::public_user_fund_with_market_slots(slots as u16, slots, 0, 10);
    if funding_enabled {
        cfg.max_abs_funding_e9_per_slot = 10_000;
        cfg.max_accrual_dt_slots = 1;
        cfg.min_funding_lifetime_slots = 1;
        cfg.max_price_move_bps_per_slot = 9_999;
    }
    cfg
}

impl Engine {
    fn new(n_markets: u32, n_accounts: u8, funding_enabled: bool) -> Self {
        let slots = n_markets;
        let init_price = ACTIVATION_PRICE;
        let cfg = engine_config(slots, funding_enabled);
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
                // The close-progress ledger's applied-ADL quantity, read directly
                // via `.get()` as the engine does (v16.rs:2134). 0 until an ADL
                // advances it (v16.rs:9533).
                quantity_adl_applied_q: acct.close_progress.quantity_adl_applied_q.get(),
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

    /// Read the GROUP-HEADER quote-atom stores (see [`SystemObs`]). MUST be called
    /// after all engine views are dropped (same discipline as [`Engine::observe`]).
    /// `vault`/`insurance`/`c_tot`/`payout_snapshot` come straight off the header
    /// (`v16.rs:4061-4087`); `backing_provider_earnings` and
    /// `explicit_unallocated_loss_total` are summed over the engine asset slots
    /// exactly as the engine's stock reconciliation does (`v16.rs:4508-4517`).
    fn observe_system(&self) -> SystemObs {
        let mut backing_provider_earnings = 0u128;
        let mut explicit_unallocated_loss_total = 0u128;
        for market in self.mk.iter() {
            let slot = &market.engine;
            backing_provider_earnings = backing_provider_earnings
                .saturating_add(slot.backing_long.utilization_fee_earnings.get())
                .saturating_add(slot.backing_short.utilization_fee_earnings.get());
            explicit_unallocated_loss_total = explicit_unallocated_loss_total
                .saturating_add(slot.asset.explicit_unallocated_loss_long.get())
                .saturating_add(slot.asset.explicit_unallocated_loss_short.get());
        }
        SystemObs {
            vault: self.mh.vault.get(),
            insurance: self.mh.insurance.get(),
            c_tot: self.mh.c_tot.get(),
            payout_snapshot: self.mh.payout_snapshot.get(),
            explicit_unallocated_loss_total,
            backing_provider_earnings,
        }
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

/// Per-step engine outcomes captured for the step's [`Observation`]. Both fields
/// are `None` for actions that produce neither a liquidation nor an ADL.
#[derive(Clone, Copy, Debug, Default)]
struct StepOutcome {
    liquidation: Option<LiquidationObs>,
    adl: Option<AdlObs>,
}

/// Apply a single action to the engine, threading external-flow tallies.
/// Engine views constructed here are dropped before the caller observes state.
/// Returns a [`StepOutcome`]: the `LiquidationOutcomeV16` (as a [`LiquidationObs`])
/// when the action is a liquidation the engine accepted, or the
/// `QuantityAdlOutcomeV16` (as an [`AdlObs`]) when the action is an ADL the engine
/// accepted, so [`run`] can record it on the step's [`Observation`]; all other
/// actions return the default (both `None`).
fn apply(
    eng: &mut Engine,
    action: Action,
    ext_in: &mut [u128],
    ext_out: &mut [u128],
) -> Result<StepOutcome, V16Error> {
    match action {
        Action::Deposit { account, amount } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.deposit_not_atomic(&mut av, amount)?;
            ext_in[account as usize] = ext_in[account as usize].saturating_add(amount);
            Ok(StepOutcome::default())
        }
        Action::Withdraw { account, amount } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.withdraw_not_atomic(&mut av, amount)?;
            ext_out[account as usize] = ext_out[account as usize].saturating_add(amount);
            Ok(StepOutcome::default())
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
            Ok(StepOutcome::default())
        }
        Action::MovePrice {
            asset,
            now_slot,
            effective_price,
        } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            mv.accrue_asset_to_not_atomic(asset as usize, now_slot, effective_price, 0, false)?;
            Ok(StepOutcome::default())
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
            Ok(StepOutcome::default())
        }
        Action::ProviderPostClaim { asset, side, claim } => {
            // v0.4 EARNED-STATE: a backing provider posts a positive source-credit
            // claim bound PLUS matching fresh counterparty backing on the chosen
            // domain, through the engine's PUBLIC provider entrypoints. This is the
            // ONLY privileged-but-public op the earned campaign keeps; it writes no
            // per-account field and no group-header total (contrast
            // `SeedSourceClaim`). The per-account claim/PnL is EARNED via funding.
            let claim_num = claim
                .checked_mul(BOUND_SCALE)
                .ok_or(V16Error::ArithmeticOverflow)?;
            let domain = (asset as usize) * 2 + (side as usize);
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            mv.add_source_positive_claim_bound_not_atomic(domain, claim_num, claim_num)?;
            // expiry must exceed current_slot; use u64::MAX so the backing never
            // expires across the campaign's funding cranks.
            mv.add_fresh_counterparty_backing_not_atomic(domain, claim_num, u64::MAX)?;
            Ok(StepOutcome::default())
        }
        Action::AccrueFunding {
            account,
            asset,
            now_slot,
            effective_price,
            funding_rate_e9,
        } => {
            // v0.4 EARNED-STATE: a Refresh crank threading a NON-ZERO
            // `funding_rate_e9`. Refresh first certifies the account (settling any
            // now-stale favorable funding K/F delta into source-attributed positive
            // PnL via `apply_signed_kf_delta_to_pnl`, `v16.rs:7197`), then accrues
            // the asset's funding at `funding_rate_e9` for `now_slot`. With
            // `effective_price` held at the activation/target price there is NO
            // price walk and hence no target/effective lag.
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            let req = PermissionlessCrankRequestV16 {
                now_slot,
                asset_index: asset as usize,
                effective_price,
                funding_rate_e9,
                action: PermissionlessCrankActionV16::Refresh,
            };
            mv.permissionless_crank_not_atomic(&mut av, req)?;
            Ok(StepOutcome::default())
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
            Ok(StepOutcome::default())
        }
        Action::SeedUnderwaterPosition { account, asset } => {
            // The seed injects `extra` quote atoms straight into the vault; record
            // it as EXTERNAL INFLOW so the v0.5 conservation oracle treats this
            // fixture deposit as an external flow, not a value mint.
            let extra = seed_underwater_position(eng, account, asset, 0)?;
            ext_in[account as usize] = ext_in[account as usize].saturating_add(extra);
            Ok(StepOutcome::default())
        }
        Action::SeedUnderwaterPositionFunded {
            account,
            asset,
            domain_budget,
        } => {
            let extra = seed_underwater_position(eng, account, asset, domain_budget)?;
            ext_in[account as usize] = ext_in[account as usize].saturating_add(extra);
            Ok(StepOutcome::default())
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
            Ok(StepOutcome {
                liquidation: Some(LiquidationObs {
                    closed_q: out.closed_q,
                    insurance_used: out.insurance_used,
                    residual_booked: out.residual_booked,
                    explicit_loss: out.explicit_loss,
                    fee_charged: out.fee_charged,
                }),
                adl: None,
            })
        }
        Action::SeedFinalizedClose {
            account,
            asset,
            bankrupt_side,
        } => {
            seed_finalized_close(eng, account, asset, side_from_u8(bankrupt_side))?;
            Ok(StepOutcome::default())
        }
        Action::ApplyAdl {
            account,
            asset,
            bankrupt_side,
            close_q,
        } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            let out: QuantityAdlOutcomeV16 = mv
                .apply_quantity_adl_after_residual_for_account_not_atomic(
                    &mut av,
                    asset as usize,
                    side_from_u8(bankrupt_side),
                    close_q,
                )?;
            // The engine advanced the account's close-progress ledger; surface its
            // `quantity_adl_applied_q` for the v0.3-B accounting oracle. The view
            // `av` still borrows the account here, so read through it before drop.
            let quantity_adl_applied_q = av
                .header
                .close_progress
                .try_to_runtime()?
                .quantity_adl_applied_q;
            Ok(StepOutcome {
                liquidation: None,
                adl: Some(AdlObs {
                    closed_q: out.closed_q,
                    opposite_a_after: out.opposite_a_after,
                    reset_started: out.reset_started,
                    quantity_adl_applied_q,
                }),
            })
        }
        Action::ConvertReleasedPnl { account } => {
            // v0.7: realize released positive PnL into withdrawable capital through
            // the engine's PUBLIC entrypoint. The engine does `capital += converted`,
            // `pnl -= face_burn` with the vault flat (the `support_to_account_capital`
            // flow proof, `v16.rs:10808`), runs the backing firewall
            // (`create_and_consume_..._for_effective`, `v16.rs:6185`, fail-closed
            // `LockActive` at `:6249` if support cannot be covered), and validates the
            // result (`v16.rs:10828`). A mint (`converted > face_burn`) would raise
            // claimable value with no external flow — caught by the v0.6 oracle.
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.convert_released_pnl_to_capital_not_atomic(&mut av)?;
            Ok(StepOutcome::default())
        }
        Action::ReleaseLiens { account } => {
            // v0.7: release no-longer-needed source-credit liens so a lien drawn
            // while a position was open can be cleared after the position is flat —
            // the setup for converting against the residual liened-effective seam.
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let mut av = PortfolioV16ViewMut::new(
                &mut eng.accts[account as usize],
                &mut eng.domains[account as usize],
            );
            mv.release_account_source_credit_liens_if_unneeded_not_atomic(&mut av)?;
            Ok(StepOutcome::default())
        }
    }
}

/// Map a scenario `bankrupt_side` byte (`0 == Long`, `1 == Short`) to the engine's
/// [`SideV16`]. [`Scenario::validate`](crate::scenario::Scenario::validate) rejects
/// any other value at the trust boundary, so a non-`{0,1}` byte can only reach here
/// from internal driver code; treat it as `Short`.
fn side_from_u8(side: u8) -> SideV16 {
    if side == 0 {
        SideV16::Long
    } else {
        SideV16::Short
    }
}

/// The engine's private `opposite_side` (`v16.rs:12443`): `Long <-> Short`.
/// Inlined here because the engine does not export it.
fn opposite_side(side: SideV16) -> SideV16 {
    match side {
        SideV16::Long => SideV16::Short,
        SideV16::Short => SideV16::Long,
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
///
/// Returns the quote-atom amount `extra` injected into `vault` (and `insurance`).
/// This is an EXTERNAL INFLOW of value into the instance — the seed deposits
/// `extra` quote atoms straight into the vault via a direct field write rather
/// than through `deposit_not_atomic`, but it is value crossing the instance
/// boundary all the same. The call site MUST record it in the runner's external
/// inflow ledger so the v0.5 conservation oracle does not mistake this fixture
/// deposit for a value mint.
fn seed_underwater_position(
    eng: &mut Engine,
    account: u8,
    asset: u8,
    domain_budget: u128,
) -> Result<u128, V16Error> {
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
    Ok(extra)
}

/// Drive `account` into an engine-accepted, FINALIZED-CLOSE, ADL-eligible state on
/// `asset` for the given `bankrupt_side`, so [`Action::ApplyAdl`] can fire a real
/// quantity auto-deleverage.
///
/// The engine's ADL entrypoint
/// `apply_quantity_adl_after_residual_for_account_not_atomic` (`v16.rs:9479`) gates
/// on TWO things, which this seed satisfies exactly:
///
/// 1. The account's `close_progress` ledger is `active && finalized &&
///    residual_remaining == 0`, with `asset_index == asset`, `market_id` equal to
///    the asset's `market_id`, and `domain_side == opposite_side(bankrupt_side)`.
///    The ledger body satisfies the engine-validated residual equation
///    (`total_loss == gross + drift`; `progress == support + insurance + b_loss +
///    explicit`; `residual == total_loss - progress`) from the engine's own
///    `proof_v16_close_progress_ledger_residual_equation_is_enforced`
///    (`tests/proofs_v16.rs:1716`): here `gross = support = junior_face_burned = 4`,
///    everything else 0, so `total_loss == progress == 4` and `residual == 0`
///    (finalized). `support_consumed <= junior_face_burned` and
///    `drift_reference_slot <= max_close_slot` (the other ledger-shape invariants,
///    `v16.rs:2297`) also hold.
///
/// 2. An ACTIVE, non-stale leg on `asset` whose `side == bankrupt_side` and whose
///    `basis_pos_q.unsigned_abs()` the ADL `close_q` must equal. The leg mirrors the
///    snapshot-bound shape of [`seed_underwater_position`]'s leg (a_basis `ADL_ONE`,
///    `loss_weight == POS_SCALE`, `basis_pos_q == POS_SCALE`, snapshots taken from
///    the asset), so `validate_active_leg` + `leg_snapshots_bound_to_asset_side`
///    accept it. With `domain_side == opposite_side(bankrupt_side) ==
///    opposite_side(leg.side)`, the ledger/leg cross-check at `v16.rs:2332` also
///    passes.
///
/// Open interest / loss-weight / position-count totals are set to `2 * POS_SCALE`
/// (balanced long == short, required in `Live` mode by `v16.rs:4636`) with
/// `stored_pos_count == 2` each, so an ADL `close_q == POS_SCALE` reduces both
/// sides to `POS_SCALE` (still > 0, so neither side hits a full-drain reset) and
/// `opposite_a_after == ADL_ONE / 2`, which stays above `MIN_A_SIDE`. The function
/// then runs `validate_shape` + `validate_with_market`, propagating any rejection,
/// so the driver only ever proceeds from a state the engine itself accepts.
fn seed_finalized_close(
    eng: &mut Engine,
    account: u8,
    asset: u8,
    bankrupt_side: SideV16,
) -> Result<(), V16Error> {
    let ai = asset as usize;
    let domain_side = opposite_side(bankrupt_side);

    // Balanced open interest / loss-weight / position-count totals: two legs each
    // side of POS_SCALE (this account contributes one `bankrupt_side` leg).
    let mut asset_rt = eng.mk[ai].engine.asset.try_to_runtime()?;
    asset_rt.oi_eff_long_q = 2 * POS_SCALE;
    asset_rt.oi_eff_short_q = 2 * POS_SCALE;
    asset_rt.loss_weight_sum_long = 2 * POS_SCALE;
    asset_rt.loss_weight_sum_short = 2 * POS_SCALE;
    asset_rt.stored_pos_count_long = 2;
    asset_rt.stored_pos_count_short = 2;
    eng.mk[ai].engine.asset = AssetStateV16Account::from_runtime(&asset_rt);

    // The leg on `bankrupt_side`, snapshot-bound to the asset's side state.
    let (epoch_snap, k_snap, f_snap, b_snap) = match bankrupt_side {
        SideV16::Long => (
            asset_rt.epoch_long,
            asset_rt.k_long,
            asset_rt.f_long_num,
            asset_rt.b_long_num,
        ),
        SideV16::Short => (
            asset_rt.epoch_short,
            asset_rt.k_short,
            asset_rt.f_short_num,
            asset_rt.b_short_num,
        ),
    };
    let acct = &mut eng.accts[account as usize];
    acct.legs[0] = PortfolioLegV16Account::from_runtime(&PortfolioLegV16 {
        active: true,
        asset_index: ai as u32,
        market_id: asset_rt.market_id,
        side: bankrupt_side,
        basis_pos_q: POS_SCALE as i128,
        a_basis: ADL_ONE,
        k_snap,
        f_snap,
        epoch_snap,
        loss_weight: POS_SCALE,
        b_snap,
        b_rem: 0,
        b_epoch_snap: epoch_snap,
        b_stale: false,
        stale: false,
    });
    acct.active_bitmap[0] = V16PodU64::new(1);

    // The finalized close-progress ledger: residual 0, `domain_side ==
    // opposite_side(bankrupt_side)`. gross = support = junior_face_burned = 4 ⇒
    // total_loss == progress == 4, residual == 0, finalized.
    //
    // `drift_reference_slot` is set to the group's `current_slot` (the activation
    // slot) so the ADL entrypoint's snapshot-recovery gate
    // (`ensure_open_close_snapshot_current_or_recovery`, v16.rs:9088) sees
    // `current_slot <= drift_reference_slot` and does NOT declare permissionless
    // recovery; `max_close_slot` stays at or above `current_slot` so the
    // not-expired gate (v16.rs:9064) also passes, with `drift_reference_slot <=
    // max_close_slot` (v16.rs:2301) preserved.
    let current_slot = eng.mh.current_slot.get();
    let max_close_slot = current_slot.saturating_add(10);
    let ledger = CloseProgressLedgerV16 {
        active: true,
        finalized: true,
        canceled: false,
        close_id: 1,
        asset_index: ai as u32,
        market_id: asset_rt.market_id,
        domain_side,
        gross_loss_at_close_start: 4,
        drift_reference_slot: current_slot,
        max_close_slot,
        support_consumed: 4,
        junior_face_burned: 4,
        insurance_spent: 0,
        b_loss_booked: 0,
        explicit_loss_assigned: 0,
        quantity_adl_applied_q: 0,
        drift_consumed: 0,
        residual_remaining: 0,
    };
    acct.close_progress = CloseProgressLedgerV16Account::from_runtime(&ledger);

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

/// v0.4 EARNED-STATE campaign: reach the source-credit-lien precondition through
/// REAL engine operations rather than the direct field writes
/// [`lien_creating_campaign`] uses via [`Action::SeedSourceClaim`].
///
/// What is EARNED (driven by engine logic, no direct writes):
///   * the OPEN LEG — a real matched position from `execute_trade`
///     ([`Action::Trade`]), not a hand-built `PortfolioLegV16Account`;
///   * the source-attributed POSITIVE PnL — settled by the engine from a favorable
///     FUNDING K/F delta. Each [`Action::AccrueFunding`] crank refreshes (settling
///     any now-stale favorable delta into the long's account as source-attributed
///     positive PnL via `apply_signed_kf_delta_to_pnl` -> `set_account_pnl_with_source`,
///     `v16.rs:7197`/`6627`) then accrues more funding. `set_account_pnl_inner`
///     (`v16.rs:6673`) is what raises BOTH `account.pnl` AND the per-account
///     `source_claim_bound_num` (and the market `positive_claim_bound_num`) — all
///     through engine logic;
///   * the LIEN — drawn by `execute_trade`'s
///     `create_initial_margin_source_lien_if_needed` (`v16.rs:10261`) on the final
///     risk-increasing add.
///
/// What is still PROVIDER-POSTED (a legitimate public op, kept per the plan):
///   * the MARKET-side source-credit claim + counterparty backing, via the public
///     provider entrypoints ([`Action::ProviderPostClaim`]). It writes no
///     per-account field; it is the credit a backing provider posts on-chain.
///
/// Domain choice: account 0 holds a LONG leg, whose favorable funding the engine
/// attributes to the SHORT source-credit domain (`opposite_side(Long)`,
/// `v16.rs:7196`), domain `asset*2+1`. So the provider backs the SHORT domain and
/// the lien is drawn there.
///
/// Economics (price held at [`ACTIVATION_PRICE`] == 100, NO price walk so no
/// target/effective lag): account 0 deposits exactly leg-1's IM (10_000 for a
/// 100.0 position at 100), so its capital is fully committed and the later add has
/// no free equity beyond the earned credit. A NEGATIVE `funding_rate_e9` makes the
/// long gain ~100 PnL per settled slot; after the funding cranks the long holds
/// several hundred atoms of source-attributed positive PnL. The final add (a 1.0
/// position, IM 100) needs 100 of incremental IM the capital cannot cover, so the
/// engine draws a 100-atom effective lien against the EARNED claim.
pub fn earned_lien_campaign() -> Scenario {
    use crate::scenario::Action::*;
    // A LONG leg's favorable funding is attributed to the SHORT domain.
    let short_side: u8 = 1;
    let p = ACTIVATION_PRICE;
    // Negative rate => f_long rises => the long books a funding GAIN (v16.rs:7892).
    let funding_rate_e9: i128 = -10_000;
    // Refresh-then-accrue means funding settles one crank AFTER it accrues, so a
    // burst of slots is needed to accumulate the earned claim. Interleave both
    // legs each slot (both must stay current for the asset to keep accruing).
    let mut actions = vec![
        // Counterparty funds the short legs generously (it absorbs the funding
        // loss and the trades' short side).
        Deposit {
            account: 1,
            amount: 100_000_000,
        },
        // The long deposits EXACTLY leg-1's IM (notional 100.0 * price 100 = 10_000
        // at 100% IM), so its capital is fully committed by the open.
        Deposit {
            account: 0,
            amount: 10_000,
        },
        // Open the real EARN position: long 100.0 vs short, at the activation price.
        Trade {
            long: 0,
            short: 1,
            asset: 0,
            size_q: 100 * POS_SCALE,
            exec_price: p,
            fee_bps: 0,
        },
        // Provider posts the market-side claim + backing on the SHORT domain (the
        // domain the long's funding gain is attributed to). 1_000 atoms backs a
        // lien far larger than the 100 the add needs.
        ProviderPostClaim {
            asset: 0,
            side: short_side,
            claim: 1_000,
        },
    ];
    // Funding cranks: accrue at a negative rate with effective == target (no lag).
    // Six slots earn ~400 atoms of source-attributed positive PnL for the long
    // (the first one or two slots settle nothing — refresh-then-accrue).
    for slot in 1..=6u64 {
        for account in [0u8, 1u8] {
            actions.push(AccrueFunding {
                account,
                asset: 0,
                now_slot: slot,
                effective_price: p,
                funding_rate_e9,
            });
        }
    }
    // Risk-increasing add by the long: its capital is fully committed, so the
    // incremental IM (100) must be met by a source-credit lien drawn against the
    // EARNED positive PnL. effective == target throughout, so the trade is not
    // blocked by a target/effective lag.
    actions.push(Trade {
        long: 0,
        short: 1,
        asset: 0,
        size_q: POS_SCALE,
        exec_price: p,
        fee_bps: 0,
    });
    Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions,
    }
}

/// v0.6 — a funding-only campaign that drives the engine through its
/// NON-VALUE-CONSERVATIVE funding settlement. Two well-capitalized accounts open a
/// matched position of `size_q` and then run `slots` epochs, each cranking BOTH
/// legs so each settles its own funding K/F delta in a SEPARATE
/// `permissionless_crank` (`settle_leg_kf_effects_at_slot`, `v16.rs:7179`).
///
/// When `size_q` is NOT a whole multiple of `POS_SCALE` (a fractional basis), the
/// per-leg settlement magnitude `funding_delta * |basis| / (a_basis * POS_SCALE)`
/// has a nonzero remainder, so `floor_div_signed_conservative_i128`
/// (`wide_math.rs:1433-1447`) truncates the RECEIVER's gain to `q` (`net > 0` branch,
/// `v16.rs:7194-7197`) while rounding the PAYER's loss to `q+1` (`net < 0` branch,
/// `v16.rs:7198-7208`, whose `reserve_new_capital_backed_loss` debits capital by the
/// rounded magnitude, `v16.rs:6998`/`7028`). Because the two legs settle in separate
/// instructions, no per-instruction `TokenValueFlowProof` spans both, so the
/// asymmetry is never caught: each settled slot permanently destroys exactly one
/// quote atom of claimable value (`Σ capital + Σ pnl`) with the vault flat and the
/// dust credited to NO sink — violating spec req #14 (the
/// `SettlementRoundingResidue`/`UnallocatedProtocolSurplus` sink fields do not exist
/// in the live `MarketGroupV16HeaderAccount`, `v16.rs:4057-4091`) and undetectable at
/// runtime (`StockReconciliationProofV16`, `v16.rs:3019`, is never constructed).
///
/// A whole-multiple `size_q` (no remainder) conserves claimable value exactly — the
/// `tests/funding_conservation.rs` causation arm pins clean-vs-fractional. The
/// per-slot leak is permanent and monotone, so it scales with `slots` (a constant
/// settlement-lag offset does not) — the discriminator the oracle relies on.
///
/// The campaign is REACHABLE with no field-writes: real `Deposit` + `Trade` + a
/// `ProviderPostClaim` (the engine's PUBLIC backing-provider entrypoint, no internal
/// callers — `v16.rs:4909`) + `AccrueFunding` cranks. The provider claim backs the
/// receiver domain (a long leg's funding is attributed to the SHORT domain,
/// `opposite_side(Long)`, `v16.rs:7196`) so the long books its full floored gain
/// every slot and the leak grows WITHOUT BOUND instead of capping at the receiver's
/// unbacked source-realizability support. The provider op injects no quote value
/// into the conservation measure (net external flow stays exactly the deposits).
pub fn funding_leak_campaign(size_q: u128, slots: u64) -> Scenario {
    use crate::scenario::Action::*;
    let p = ACTIVATION_PRICE;
    // Negative rate => f_long rises => the long is the funding RECEIVER (gains),
    // the short is the PAYER (loses). Both legs are cranked each slot so both settle.
    let funding_rate_e9: i128 = -9_999;
    let big = 1_000_000_000u128; // both accounts over-funded: no haircut/bankruptcy branch.
    let mut actions = vec![
        Deposit { account: 0, amount: big },
        Deposit { account: 1, amount: big },
        Trade {
            long: 0,
            short: 1,
            asset: 0,
            size_q,
            exec_price: p,
            fee_bps: 0,
        },
        // Back the RECEIVER domain (a long leg's funding is attributed to the SHORT
        // domain, opposite_side(Long)) so the long can book its full floored gain
        // indefinitely (no source-realizability haircut) — the leak then grows
        // without bound instead of capping at the receiver's unbacked support.
        ProviderPostClaim { asset: 0, side: 1, claim: 1_000_000_000 },
    ];
    for slot in 1..=slots {
        for account in [0u8, 1u8] {
            actions.push(AccrueFunding {
                account,
                asset: 0,
                now_slot: slot,
                effective_price: p,
                funding_rate_e9,
            });
        }
    }
    Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions,
    }
}

/// v0.7 — drive the engine's PnL→capital REALIZATION firewall end-to-end. A long
/// earns funding PnL (as in [`funding_leak_campaign`]), then FLATTENS its leg (a
/// closing trade) so it carries released positive PnL with NO active exposure,
/// releases any liens, re-certifies, and calls
/// [`Action::ConvertReleasedPnl`] — the engine's public
/// `convert_released_pnl_to_capital_not_atomic` (`v16.rs:10821`), the ONLY path that
/// turns PnL into withdrawable `capital`. Finally it `Withdraw`s part of the
/// realized capital.
///
/// This exercises the backing firewall
/// (`create_and_consume_account_source_credit_for_effective_not_atomic`,
/// `v16.rs:6185`) that every "winner overdraws realizable PnL" extraction theory
/// gated on but the harness never reached. Convert does `capital += converted`,
/// `pnl -= face_burn` with the vault flat, so the v0.6 claimable oracle catches any
/// MINT (`converted > face_burn` ⇒ `ClaimableValueCreated`, the extractive
/// direction). In the pinned engine the convert conserves claimable value EXACTLY
/// (`converted == face_burn`), so the firewall holds — now TESTED, not argued.
pub fn convert_realization_campaign() -> Scenario {
    use crate::scenario::Action::*;
    let p = ACTIVATION_PRICE;
    let size_q = 100 * POS_SCALE + 1; // fractional basis: also carries the v0.6 leak
    let big = 1_000_000_000u128;
    let mut actions = vec![
        Deposit { account: 0, amount: big },
        Deposit { account: 1, amount: big },
        Trade {
            long: 0,
            short: 1,
            asset: 0,
            size_q,
            exec_price: p,
            fee_bps: 0,
        },
        // Back the receiver (short) domain so the long books its full funding gain.
        ProviderPostClaim {
            asset: 0,
            side: 1,
            claim: big,
        },
    ];
    for slot in 1..=6u64 {
        for account in [0u8, 1u8] {
            actions.push(AccrueFunding {
                account,
                asset: 0,
                now_slot: slot,
                effective_price: p,
                funding_rate_e9: -9_999,
            });
        }
    }
    // Flatten the long's leg (it sells the same size back), release liens, re-certify
    // at a fresh slot, then REALIZE the released PnL into capital and withdraw part.
    actions.push(Trade {
        long: 1,
        short: 0,
        asset: 0,
        size_q,
        exec_price: p,
        fee_bps: 0,
    });
    actions.push(ReleaseLiens { account: 0 });
    actions.push(Crank {
        account: 0,
        asset: 0,
        now_slot: 7,
        effective_price: p,
    });
    actions.push(ConvertReleasedPnl { account: 0 });
    actions.push(Withdraw {
        account: 0,
        amount: 300,
    });
    Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions,
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

/// A campaign that drives a REAL quantity-ADL: it seeds an engine-accepted,
/// finalized-close, ADL-eligible state (via [`Action::SeedFinalizedClose`], which
/// constructs the engine-validated finalized close-progress ledger plus an active
/// `bankrupt_side` leg) and then applies a quantity-ADL of `POS_SCALE` against that
/// leg. The ADL must close a non-zero quantity (`closed_q > 0`) on the profitable
/// counterparty's open interest and advance the account's
/// `close_progress.quantity_adl_applied_q` to that quantity — the non-vacuity the
/// `tests/adl.rs` gate pins.
///
/// `bankrupt_side` is `Long` (`0`), so the deleveraged (profitable-counterparty)
/// side is `Short`; `close_q == POS_SCALE` equals the seeded leg's
/// `basis_pos_q.unsigned_abs()` and is below each side's `2 * POS_SCALE` open
/// interest, so the ADL halves the opposite side's `a` (to `ADL_ONE / 2`) without
/// driving either side to a full-drain reset.
pub fn adl_campaign() -> Scenario {
    use crate::scenario::Action::*;
    Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            // Construct the finalized-close, ADL-eligible state (bankrupt = long).
            SeedFinalizedClose {
                account: 0,
                asset: 0,
                bankrupt_side: 0,
            },
            // Apply the quantity-ADL: close the full POS_SCALE long leg, deleveraging
            // the profitable short counterparty.
            ApplyAdl {
                account: 0,
                asset: 0,
                bankrupt_side: 0,
                close_q: POS_SCALE,
            },
        ],
    }
}

/// True iff the scenario accrues funding (an [`Action::AccrueFunding`] with a
/// non-zero rate). Such a scenario needs the funding-enabled market config (see
/// [`engine_config`]); every seeded campaign leaves this `false` and runs on the
/// canonical funding-off public config.
fn scenario_needs_funding(s: &Scenario) -> bool {
    s.actions.iter().any(|a| {
        matches!(
            a,
            Action::AccrueFunding {
                funding_rate_e9, ..
            } if *funding_rate_e9 != 0
        )
    })
}

/// Execute a scenario, returning a [`Trace`] with one [`Observation`] per action.
pub fn run(s: &Scenario) -> Trace {
    let funding_enabled = scenario_needs_funding(s);
    let mut eng = Engine::new(s.n_markets, s.n_accounts, funding_enabled);
    let n = s.n_accounts as usize;
    let mut external_in = vec![0u128; n];
    let mut external_out = vec![0u128; n];
    let mut observations = Vec::with_capacity(s.actions.len());

    for (step, action) in s.actions.iter().enumerate() {
        // Snapshot the cumulative external-flow totals BEFORE the step so the
        // per-step delta (the only external value that crosses the instance
        // boundary this step) can be recovered for the conservation oracle.
        let in_before: u128 = external_in.iter().copied().sum();
        let out_before: u128 = external_out.iter().copied().sum();
        // `apply` returns the liquidation and/or ADL outcome on a successful step;
        // split them from the `Result` recorded on the observation.
        let (result, outcome) = match apply(&mut eng, *action, &mut external_in, &mut external_out)
        {
            Ok(out) => (Ok(()), out),
            Err(e) => (Err(format!("{e:?}")), StepOutcome::default()),
        };
        // Views from apply() are out of scope here; safe to read account,
        // market-engine, and group-header state.
        let accounts = eng.observe();
        let market_domains = eng.observe_markets();
        let system = eng.observe_system();
        let in_after: u128 = external_in.iter().copied().sum();
        let out_after: u128 = external_out.iter().copied().sum();
        observations.push(Observation {
            step,
            action: *action,
            result,
            accounts,
            market_domains,
            liquidation: outcome.liquidation,
            adl: outcome.adl,
            system,
            ext_in_step: in_after.saturating_sub(in_before),
            ext_out_step: out_after.saturating_sub(out_before),
        });
    }

    Trace {
        observations,
        external_in,
        external_out,
    }
}
