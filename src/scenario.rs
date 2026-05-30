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
    MovePrice {
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
