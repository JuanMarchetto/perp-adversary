//! Pure campaign vocabulary. No engine calls. Deterministic and serializable.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Long = 0,
    Short = 1,
}

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
