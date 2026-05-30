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
    Liquidate {
        account: u8,
        asset: u8,
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
                Action::Liquidate { account, asset } => {
                    check_acc(i, account, "")?;
                    check_asset(i, asset, "")?;
                }
            }
        }
        Ok(())
    }
}
