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
