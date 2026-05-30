//! Pure campaign vocabulary. No engine calls. Deterministic and serializable.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Deposit {
        account: u8,
        amount: u128,
    },
    Withdraw {
        account: u8,
        amount: u128,
    },
    /// A matched trade between two accounts on one asset at an execution price.
    Trade {
        long: u8,
        short: u8,
        asset: u8,
        size_q: u128,
        exec_price: u64,
        fee_bps: u64,
    },
    /// Move an asset's effective (mark) price at a slot. The driver clamps to the
    /// engine's per-slot bound; the scenario expresses intent.
    ///
    /// NOTE: with an open position the engine's bare price-accrual entrypoint
    /// returns `NonProgress` (it refuses to mutate equity-active state without
    /// committed protective progress). Use [`Action::Crank`] to legitimately
    /// progress price for an account that holds a position — that is the path
    /// that books PnL and can create source-credit liens. `MovePrice` remains
    /// only for the no-open-position case.
    MovePrice {
        asset: u8,
        now_slot: u64,
        effective_price: u64,
    },
    /// Permissionless crank (`Refresh`) for one account on one asset. Routes
    /// through the engine's `permissionless_crank_not_atomic` + `Refresh`
    /// action: it first refreshes/certifies the account (settling the open
    /// leg's favourable K-delta into source-attributed positive PnL), then
    /// computes `protective_progress` and accrues the asset to the new price.
    /// This is the public, position-aware price-progression path the engine's
    /// own tests use (`v16.rs:7974-8021`), and the one that ultimately lets the
    /// engine create source-credit liens.
    Crank {
        account: u8,
        asset: u8,
        now_slot: u64,
        effective_price: u64,
    },
    /// Seed `account`'s positive, source-attributed PnL of `claim` (unscaled
    /// atoms) on `asset`'s long source-credit domain, plus the matching
    /// counterparty backing in the market — establishing the precondition the
    /// engine requires before a risk-increasing trade can draw an
    /// initial-margin source-credit lien.
    ///
    /// This mirrors, step for step, the precondition built by the engine's OWN
    /// conformance test
    /// `v16_risk_increasing_trade_creates_source_credit_lien_for_im`
    /// (`tests/v16_spec_tests.rs:580`): the market-side source-credit/backing
    /// state is established through the engine's PUBLIC entrypoints
    /// (`add_source_positive_claim_bound_not_atomic`,
    /// `add_fresh_counterparty_backing_not_atomic`) and the per-account
    /// PnL/claim is set exactly as the engine's `account_fixture` seeds it. The
    /// driver then asks the engine to VALIDATE the resulting state, so this is a
    /// state the engine itself accepts — not a stub.
    ///
    /// Why a seed and not a pure price walk: in engine rev `71c9032`,
    /// `accrue_asset_to_not_atomic` moves `effective_price` but never updates
    /// `raw_oracle_target_price`, so any price progression leaves a permanent
    /// target/effective lag that makes `validate_trade_position_preflight`
    /// reject every risk-increasing trade (`v16.rs:8557`) — the ONLY path that
    /// creates a lien. The price walk that would create the PnL therefore also
    /// blocks the lien. See README "v0 result" for the full finding.
    SeedSourceClaim {
        account: u8,
        asset: u8,
        claim: u128,
    },
    /// Seed `account` into an engine-accepted UNDERWATER state on `asset`'s long
    /// side, so a subsequent [`Action::Liquidate`] with a real `close_q` fires a
    /// genuine liquidation that books residual loss.
    ///
    /// This mirrors, field for field, the underwater state the engine's OWN
    /// conformance test
    /// `v16_public_liquidation_on_unfunded_domain_cannot_drain_shared_insurance`
    /// (`tests/v16_spec_tests.rs:289`) constructs: a single open long leg at
    /// `POS_SCALE`, negative account PnL, and matching open-interest /
    /// loss-weight / position-count totals on the asset, with `vault`,
    /// `insurance` and `negative_pnl_account_count` set on the group header. The
    /// driver then asks the engine to VALIDATE the resulting state
    /// (`validate_shape` + `validate_with_market`), so this is a state the engine
    /// itself accepts — not a stub. Liquidating it on an unfunded domain forces
    /// the engine to book the unbacked loss as RESIDUAL rather than draining
    /// shared insurance.
    SeedUnderwaterPosition {
        account: u8,
        asset: u8,
    },
    /// Like [`Action::SeedUnderwaterPosition`], but ALSO funds the bankruptcy
    /// insurance domain of the liquidated long leg — i.e. the asset's SHORT-side
    /// `insurance_domain_budget_short` — with `domain_budget` atoms.
    ///
    /// A long-leg liquidation books its bankruptcy loss against the
    /// `opposite_side(Long) == Short` insurance domain
    /// (`consume_domain_insurance_for_negative_pnl`, `v16.rs:5955`). With a ZERO
    /// budget (the plain `SeedUnderwaterPosition` case) the engine can draw NO
    /// insurance and must book residual, so `insurance_used == 0` and the
    /// isolation oracle's anti-vacuity precondition is weak. Funding the
    /// short-side budget lets the engine genuinely SPEND insurance for the
    /// liquidated domain (`insurance_used > 0`), giving the isolation oracle the
    /// stronger test: some insurance IS spent for the correct domain and NONE for
    /// any other.
    ///
    /// The budget is set on the asset's engine slot exactly where the engine's own
    /// proof `proof_v16_view_domain_budget_caps_bankruptcy_insurance_spend`
    /// (`tests/proofs_v16.rs:2384`) sets it, and the resulting state is run through
    /// the engine's `validate_shape` + `validate_with_market` before the driver
    /// proceeds (the same discipline as [`Action::SeedUnderwaterPosition`]), so it
    /// remains a state the engine itself accepts. `domain_budget` must keep the
    /// group's total live domain-budget-remaining at or below `header.insurance`
    /// (`validate_shape`, `v16.rs:4594`); the seed funds `insurance`/`vault` to
    /// accommodate it.
    SeedUnderwaterPositionFunded {
        account: u8,
        asset: u8,
        domain_budget: u128,
    },
    /// Liquidate `close_q` of `account`'s position on `asset` via the engine's
    /// `liquidate_account_not_atomic`. `close_q == 0` expresses "close the whole
    /// position" intent (the engine clamps); a non-zero `close_q` against an
    /// underwater leg (see [`Action::SeedUnderwaterPosition`]) drives a real
    /// liquidation whose `LiquidationOutcomeV16` is captured into the
    /// [`crate::driver::Observation`].
    Liquidate {
        account: u8,
        asset: u8,
        close_q: u128,
    },
    /// Seed `account` into an engine-accepted, FINALIZED-CLOSE, ADL-eligible state
    /// on `asset`, so a subsequent [`Action::ApplyAdl`] with a real `close_q`
    /// fires a genuine quantity auto-deleverage.
    ///
    /// The engine's ADL entrypoint
    /// `apply_quantity_adl_after_residual_for_account_not_atomic` (`v16.rs:9479`)
    /// gates on the account's `close_progress` ledger being
    /// `active && finalized && residual_remaining == 0`, with
    /// `domain_side == opposite_side(bankrupt_side)`, plus an ACTIVE, non-stale leg
    /// on `asset` whose `side == bankrupt_side`. This arm constructs exactly that:
    /// a residual-equation-satisfying finalized ledger (mirroring the engine's own
    /// `proof_v16_close_progress_ledger_residual_equation_is_enforced`,
    /// `tests/proofs_v16.rs:1716`) plus an active `bankrupt_side` leg on `asset`,
    /// with balanced open-interest / loss-weight / position-count totals so the
    /// later ADL leaves both sides solvent. The driver then asks the engine to
    /// VALIDATE the resulting state (`validate_shape` + `validate_with_market`), so
    /// this is a state the engine itself accepts — not a stub.
    ///
    /// `bankrupt_side` is `0 == Long` / `1 == Short`; the deleveraged
    /// (profitable-counterparty) side is `opposite_side(bankrupt_side)`.
    SeedFinalizedClose {
        account: u8,
        asset: u8,
        bankrupt_side: u8,
    },
    /// Apply a quantity-ADL of `close_q` against `account`'s `bankrupt_side` leg on
    /// `asset` via the engine's
    /// `apply_quantity_adl_after_residual_for_account_not_atomic`. Requires the
    /// state seeded by [`Action::SeedFinalizedClose`]; `close_q` must equal the
    /// leg's `basis_pos_q.unsigned_abs()` and not exceed either side's
    /// open interest. The resulting `QuantityAdlOutcomeV16` is captured into the
    /// [`crate::driver::Observation`].
    ///
    /// `bankrupt_side` is `0 == Long` / `1 == Short`.
    ApplyAdl {
        account: u8,
        asset: u8,
        bankrupt_side: u8,
        close_q: u128,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub n_markets: u32,
    pub n_accounts: u8,
    pub actions: Vec<Action>,
}

/// A structural problem with an externally-supplied [`Scenario`] (e.g. an
/// out-of-range account/asset index). Returned by [`Scenario::validate`] so the
/// trust boundary for untrusted JSON is explicit and callers can reject rather
/// than panic deep inside the engine adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioError {
    pub action_index: Option<usize>,
    pub detail: String,
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.action_index {
            Some(i) => write!(f, "action {i}: {}", self.detail),
            None => write!(f, "{}", self.detail),
        }
    }
}

impl std::error::Error for ScenarioError {}

impl Scenario {
    /// Reject structurally-invalid scenarios up front: every account index must
    /// be `< n_accounts`, every asset index `< n_markets`, trade legs must be
    /// distinct, and the dimensions must be non-degenerate. The driver indexes
    /// its account/market vectors directly, so an out-of-range index from
    /// untrusted JSON would otherwise panic. Validating here keeps that a
    /// recoverable error at the trust boundary.
    pub fn validate(&self) -> Result<(), ScenarioError> {
        if self.n_accounts == 0 {
            return Err(ScenarioError {
                action_index: None,
                detail: "n_accounts must be >= 1".into(),
            });
        }
        if self.n_markets == 0 {
            return Err(ScenarioError {
                action_index: None,
                detail: "n_markets must be >= 1".into(),
            });
        }
        let n_acc = self.n_accounts;
        let n_mkt = self.n_markets;
        let check_acc = |i: usize, a: u8, what: &str| -> Result<(), ScenarioError> {
            if a as u16 >= n_acc as u16 {
                Err(ScenarioError {
                    action_index: Some(i),
                    detail: format!("{what} account index {a} >= n_accounts {n_acc}"),
                })
            } else {
                Ok(())
            }
        };
        let check_asset = |i: usize, a: u8, what: &str| -> Result<(), ScenarioError> {
            if (a as u32) >= n_mkt {
                Err(ScenarioError {
                    action_index: Some(i),
                    detail: format!("{what} asset index {a} >= n_markets {n_mkt}"),
                })
            } else {
                Ok(())
            }
        };
        // `bankrupt_side` maps 0 -> Long, 1 -> Short; any other value is invalid.
        let check_side = |i: usize, s: u8| -> Result<(), ScenarioError> {
            if s > 1 {
                Err(ScenarioError {
                    action_index: Some(i),
                    detail: format!("bankrupt_side {s} must be 0 (long) or 1 (short)"),
                })
            } else {
                Ok(())
            }
        };
        for (i, action) in self.actions.iter().enumerate() {
            match *action {
                Action::Deposit { account, .. } | Action::Withdraw { account, .. } => {
                    check_acc(i, account, "")?;
                }
                Action::Trade {
                    long, short, asset, ..
                } => {
                    check_acc(i, long, "long")?;
                    check_acc(i, short, "short")?;
                    check_asset(i, asset, "")?;
                    if long == short {
                        return Err(ScenarioError {
                            action_index: Some(i),
                            detail: format!("trade legs must be distinct accounts (both {long})"),
                        });
                    }
                }
                Action::MovePrice { asset, .. } => check_asset(i, asset, "")?,
                Action::Crank { account, asset, .. } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                }
                Action::SeedSourceClaim { account, asset, .. } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                }
                Action::SeedUnderwaterPosition { account, asset } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                }
                Action::SeedUnderwaterPositionFunded { account, asset, .. } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                }
                Action::Liquidate { account, asset, .. } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                }
                Action::SeedFinalizedClose {
                    account,
                    asset,
                    bankrupt_side,
                } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                    check_side(i, bankrupt_side)?;
                }
                Action::ApplyAdl {
                    account,
                    asset,
                    bankrupt_side,
                    ..
                } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                    check_side(i, bankrupt_side)?;
                }
            }
        }
        Ok(())
    }
}
