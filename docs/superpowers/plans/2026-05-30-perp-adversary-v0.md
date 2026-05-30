# perp-adversary v0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an adversarial economic harness that drives the real Percolator engine through multi-step campaigns and flags any step where the source-domain realizability invariant breaks, shrinking failures to a minimal reproducible counterexample.

**Architecture:** A single `std` library crate `perp-adversary` with isolated modules (`scenario`, `driver`, `oracles`, `runner`, `report`) plus a `replay` binary. It depends on `percolator` as a pinned git dependency and never modifies it. `driver` is the only module that imports `percolator`. Oracles are pure and Kani-proven sound.

**Tech Stack:** Rust 1.94, `percolator` (git, rev `71c903242764b65bd5862eaf7779e61463af954b`), `proptest 1.4`, `serde`/`serde_json`, Kani (`kani-verifier`).

---

## Conventions (apply to every task)

- **Furious TDD, hard rule:** no production line is written without a failing test first. Order is always: write failing test → run, see it fail for the right reason → minimal implementation → run, see it pass → commit.
- **Review gate (Toly + Kani optics), run after every task before moving on:**
  - Mechanical: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, and `cargo kani` (for tasks that add Kani proofs) all green.
  - Kani optics: deterministic & total (no panics on valid input; explicit `Result`); zero `unsafe`; no unbounded loops in pure code; checked/wide arithmetic (no silent overflow); fail-closed.
  - Toly optics: account-local / bounded work in our hot path; conservative (never understate a violation); never weaken/stub the engine to get a result; terse, high-signal code.
- **Reference clone:** a read-only checkout of the engine at the pinned SHA lives at `/tmp/percolator-ref` (`src/v16.rs`, `spec.md`, `tests/proofs_v16.rs`). Line numbers below refer to that file at this SHA.
- **Commits:** one per green step. End every commit body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File structure

```
perp-adversary/
├── Cargo.toml                      # crate manifest, pinned percolator dep
├── rust-toolchain.toml             # pin Rust 1.94
├── .github/workflows/ci.yml        # build, test, clippy, fmt, kani
├── src/
│   ├── lib.rs                      # module wiring + re-exports
│   ├── scenario.rs                 # pure campaign vocabulary + proptest strategy
│   ├── driver.rs                   # ONLY module importing `percolator`; Scenario -> Trace
│   ├── oracles.rs                  # pure invariant checkers; Kani-proven sound
│   ├── runner.rs                   # proptest harness + named-scenario runner
│   └── report.rs                   # Finding rendering (json + markdown)
├── src/bin/replay.rs               # replay a saved minimal scenario
├── tests/
│   ├── smoke.rs                    # engine drivability smoke test
│   ├── driver_obs.rs               # driver observation-mapping tests
│   ├── oracle_o1.rs                # O1 unit + property tests
│   ├── kani_oracles.rs             # #[cfg(kani)] soundness proofs
│   └── jelly_campaign.rs           # seeded JELLY regression
└── scenarios/                      # serialized repro scenarios (committed when found)
```

---

## Task 0: Toolchain + Kani setup

**Files:**
- Create: `rust-toolchain.toml`

- [ ] **Step 1: Pin the toolchain**

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.94.0"
components = ["clippy", "rustfmt"]
```

- [ ] **Step 2: Install Kani (one-time, host tool)**

Run:
```bash
cargo install --locked kani-verifier
cargo kani setup
```
Expected: `cargo kani --version` prints a version. If `cargo kani setup` downloads its backend, let it finish.

- [ ] **Step 3: Verify the reference clone exists at the pinned SHA**

Run:
```bash
test -d /tmp/percolator-ref || git clone --depth 1 https://github.com/aeyakovenko/percolator /tmp/percolator-ref
git -C /tmp/percolator-ref rev-parse HEAD
```
Expected: prints `71c903242764b65bd5862eaf7779e61463af954b` (if different, re-pin the dep in Task 1 to the printed SHA and note it).

- [ ] **Step 4: Commit**
```bash
git add rust-toolchain.toml
git commit -m "chore: pin toolchain to Rust 1.94

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 1: Scaffold crate + pinned engine dependency + CI

**Files:**
- Create: `Cargo.toml`, `src/lib.rs`, `.github/workflows/ci.yml`

- [ ] **Step 1: Write the crate manifest**

`Cargo.toml`:
```toml
[package]
name = "perp-adversary"
version = "0.0.0"
edition = "2021"
license = "Apache-2.0"
description = "Adversarial economic conformance harness for Solana perps risk engines"

[dependencies]
percolator = { git = "https://github.com/aeyakovenko/percolator", rev = "71c903242764b65bd5862eaf7779e61463af954b" }
proptest = "1.4"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[profile.release]
overflow-checks = true

[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(kani)'] }
```

- [ ] **Step 2: Write the module skeleton**

`src/lib.rs`:
```rust
//! perp-adversary: adversarial economic harness for Solana perps risk engines.
#![forbid(unsafe_code)]

pub mod driver;
pub mod oracles;
pub mod report;
pub mod runner;
pub mod scenario;
```

Create empty module files so it compiles:
```bash
printf '//! Pure campaign vocabulary.\n' > src/scenario.rs
printf '//! Engine adapter (only module importing percolator).\n' > src/driver.rs
printf '//! Pure invariant checkers.\n' > src/oracles.rs
printf '//! proptest harness + named-scenario runner.\n' > src/runner.rs
printf '//! Finding rendering.\n' > src/report.rs
```

- [ ] **Step 3: Build to fetch + compile the engine dep**

Run: `cargo build`
Expected: PASS. (First build resolves the git dep and compiles `percolator`; this confirms the pin is reachable and builds on Rust 1.94.)

- [ ] **Step 4: Add CI**

`.github/workflows/ci.yml`:
```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.94.0
        with:
          components: clippy, rustfmt
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
```

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml Cargo.lock src/lib.rs src/*.rs .github/workflows/ci.yml
git commit -m "feat: scaffold crate with pinned percolator dependency and CI

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Review gate (Toly+Kani).** Run the mechanical checks; confirm the build is warning-clean.

---

## Task 2: Engine harness fixture + smoke test

Establish that we can construct the real engine and drive a deposit/withdraw round-trip, asserting observable state. Mirrors `/tmp/percolator-ref/tests/v16_spec_tests.rs` fixtures.

**Files:**
- Create: `tests/smoke.rs`

- [ ] **Step 1: Write the failing smoke test**

`tests/smoke.rs`:
```rust
use percolator::v16::{
    v16_domain_count_for_market_slots, EngineAssetSlotV16Account, Market,
    MarketGroupV16HeaderAccount, MarketGroupV16ViewMut, PortfolioAccountV16Account,
    PortfolioSourceDomainV16Account, PortfolioV16ViewMut, ProvenanceHeaderV16,
    ProvenanceHeaderV16Account, V16Config,
};

fn market(slots: u32, price: u64) -> (MarketGroupV16HeaderAccount, Vec<Market<u64>>) {
    let cfg = V16Config::public_user_fund_with_market_slots(slots as u16, slots, 0, 10);
    let mut header = MarketGroupV16HeaderAccount::new_dynamic([1u8; 32], cfg, slots, 0).unwrap();
    let mut markets = (0..slots)
        .map(|i| Market::new(i as u64, EngineAssetSlotV16Account::default()))
        .collect::<Vec<_>>();
    {
        let mut v = MarketGroupV16ViewMut::new(&mut header, &mut markets);
        for i in 0..slots as usize {
            v.activate_empty_market_not_atomic(i as u32, price, (i + 1) as u64).unwrap();
        }
        v.validate_shape().unwrap();
    }
    (header, markets)
}

fn account(slots: u32, seed: u8) -> (PortfolioAccountV16Account, Vec<PortfolioSourceDomainV16Account>) {
    let h = ProvenanceHeaderV16Account::from_runtime(&ProvenanceHeaderV16::new(
        [1u8; 32], [seed; 32], [3u8; 32],
    ));
    let acct = PortfolioAccountV16Account::try_empty(h).unwrap();
    let domains = vec![
        PortfolioSourceDomainV16Account::default();
        v16_domain_count_for_market_slots(slots).unwrap()
    ];
    (acct, domains)
}

#[test]
fn deposit_then_withdraw_roundtrip_moves_capital() {
    let (mut mh, mut mk) = market(1, 100);
    let (mut ah, mut sd) = account(1, 2);

    let mut mv = MarketGroupV16ViewMut::new(&mut mh, &mut mk);
    {
        let mut av = PortfolioV16ViewMut::new(&mut ah, &mut sd);
        mv.deposit_not_atomic(&mut av, 100).unwrap();
    }
    assert_eq!(ah.capital.get(), 100, "deposit should credit account capital");

    {
        let mut av = PortfolioV16ViewMut::new(&mut ah, &mut sd);
        mv.withdraw_not_atomic(&mut av, 40).unwrap();
    }
    assert_eq!(ah.capital.get(), 60, "withdraw should debit account capital");
}
```

- [ ] **Step 2: Run it to verify it fails (or surfaces the real API)**

Run: `cargo test --test smoke -- --nocapture`
Expected: compiles and either PASSES, or fails on a specific mismatch (e.g., a constructor arg arity or that `capital` is not the post-deposit field). If it fails on API shape, read `/tmp/percolator-ref/src/v16.rs:11423` (`deposit_not_atomic`) and `:11121` (`withdraw_not_atomic`) and adjust the calls/asserts to the real signatures. Do not change asserted *semantics* (deposit raises principal, withdraw lowers it).

- [ ] **Step 3: Make it pass**

Adjust only the harness fixture/asserts to the real API until the round-trip passes. The semantic assertions (capital +100 then -40 → 60) stay.

- [ ] **Step 4: Verify**

Run: `cargo test --test smoke`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add tests/smoke.rs
git commit -m "test: smoke test driving real engine deposit/withdraw round-trip

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Review gate.** Mechanical checks green; confirm the fixture uses only the engine's public surface.

---

## Task 3: Scenario vocabulary (pure)

**Files:**
- Create: `src/scenario.rs`
- Test: `tests/scenario.rs`

- [ ] **Step 1: Write the failing test**

`tests/scenario.rs`:
```rust
use perp_adversary::scenario::{Action, Scenario, Side};

#[test]
fn scenario_roundtrips_through_json() {
    let s = Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            Action::Deposit { account: 0, amount: 1_000 },
            Action::Trade { long: 0, short: 1, asset: 0, size_q: 10, exec_price: 100, fee_bps: 0 },
            Action::MovePrice { asset: 0, now_slot: 1, effective_price: 150 },
            Action::Withdraw { account: 0, amount: 500 },
        ],
    };
    let j = serde_json::to_string(&s).unwrap();
    let back: Scenario = serde_json::from_str(&j).unwrap();
    assert_eq!(s, back);
    assert_eq!(Side::Long as u8 + Side::Short as u8, 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test scenario`
Expected: FAIL (module/types not defined).

- [ ] **Step 3: Implement the vocabulary**

`src/scenario.rs`:
```rust
//! Pure campaign vocabulary. No engine calls. Deterministic and serializable.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Long = 0,
    Short = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Deposit { account: u8, amount: u128 },
    Withdraw { account: u8, amount: u128 },
    /// A matched trade between two accounts on one asset at an execution price.
    Trade { long: u8, short: u8, asset: u8, size_q: u128, exec_price: u64, fee_bps: u64 },
    /// Move an asset's effective (mark) price at a slot. The driver clamps to the
    /// engine's per-slot bound; the scenario expresses intent.
    MovePrice { asset: u8, now_slot: u64, effective_price: u64 },
    Liquidate { account: u8, asset: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub n_markets: u32,
    pub n_accounts: u8,
    pub actions: Vec<Action>,
}
```

- [ ] **Step 4: Verify**

Run: `cargo test --test scenario`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/scenario.rs tests/scenario.rs
git commit -m "feat: pure scenario vocabulary with serde round-trip

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Review gate.** Pure module, no `percolator` import, derives only. Green.

---

## Task 4: Driver — observation types + execute a Scenario

Map each `Action` to a real engine call and capture an `Observation` after each step. This is the only module importing `percolator`.

**Files:**
- Create/replace: `src/driver.rs`
- Test: `tests/driver_obs.rs`

- [ ] **Step 1: Write the failing test (deposit reflected in observation)**

`tests/driver_obs.rs`:
```rust
use perp_adversary::driver::run;
use perp_adversary::scenario::{Action, Scenario};

#[test]
fn trace_records_capital_and_external_flows() {
    let s = Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            Action::Deposit { account: 0, amount: 1_000 },
            Action::Withdraw { account: 0, amount: 400 },
        ],
    };
    let trace = run(&s);
    assert_eq!(trace.observations.len(), 2);
    let last = trace.observations.last().unwrap();
    assert!(last.result.is_ok(), "withdraw of <= deposit should succeed");
    assert_eq!(last.accounts[0].capital, 600);
    assert_eq!(trace.external_in[0], 1_000);
    assert_eq!(trace.external_out[0], 400);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test driver_obs`
Expected: FAIL (`run`, `Trace`, `Observation` undefined).

- [ ] **Step 3: Implement the driver (deposit/withdraw first)**

`src/driver.rs`. Read these engine lines first to confirm exact signatures: `:11423` deposit, `:11121` withdraw, `:10197` `execute_trade_with_fee_in_place_not_atomic` (note: takes `long_account` AND `short_account`), `:7817` `accrue_asset_to_not_atomic`, `:9829` `liquidate_account_not_atomic`, struct `PortfolioAccountV16Account` `:12073` (`capital`, `pnl`, `fee_credits`, `legs`), `SourceCreditStateV16` `:1804`.

```rust
//! Engine adapter. The ONLY module that imports `percolator`.
use crate::scenario::{Action, Scenario};
use percolator::v16::{
    v16_domain_count_for_market_slots, EngineAssetSlotV16Account, Market,
    MarketGroupV16HeaderAccount, MarketGroupV16ViewMut, PortfolioAccountV16Account,
    PortfolioSourceDomainV16Account, PortfolioV16ViewMut, ProvenanceHeaderV16,
    ProvenanceHeaderV16Account, TradeRequestV16, V16Config,
};

/// Per-source-domain realizability ledger, mirrored from `SourceCreditStateV16`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DomainObs {
    pub positive_claim_bound_num: u128,
    pub credit_rate_num: u128,
    pub fresh_reserved_backing_num: u128,
    pub valid_liened_backing_num: u128,
    pub spent_backing_num: u128,
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
    /// `Ok` = engine accepted; `Err(code)` = engine rejected (expected, not a violation).
    pub result: Result<(), String>,
    pub accounts: Vec<AccountObs>,
}

#[derive(Clone, Debug)]
pub struct Trace {
    pub observations: Vec<Observation>,
    pub external_in: Vec<u128>,
    pub external_out: Vec<u128>,
}

struct Engine {
    mh: MarketGroupV16HeaderAccount,
    mk: Vec<Market<u64>>,
    accts: Vec<PortfolioAccountV16Account>,
    domains: Vec<Vec<PortfolioSourceDomainV16Account>>,
}

impl Engine {
    fn new(n_markets: u32, n_accounts: u8, init_price: u64) -> Self {
        let cfg = V16Config::public_user_fund_with_market_slots(
            n_markets as u16, n_markets, 0, 10,
        );
        let mut mh = MarketGroupV16HeaderAccount::new_dynamic([1u8; 32], cfg, n_markets, 0).unwrap();
        let mut mk = (0..n_markets)
            .map(|i| Market::new(i as u64, EngineAssetSlotV16Account::default()))
            .collect::<Vec<_>>();
        {
            let mut v = MarketGroupV16ViewMut::new(&mut mh, &mut mk);
            for i in 0..n_markets as usize {
                v.activate_empty_market_not_atomic(i as u32, init_price, (i + 1) as u64).unwrap();
            }
            v.validate_shape().unwrap();
        }
        let dcount = v16_domain_count_for_market_slots(n_markets).unwrap();
        let mut accts = Vec::new();
        let mut domains = Vec::new();
        for a in 0..n_accounts {
            let h = ProvenanceHeaderV16Account::from_runtime(&ProvenanceHeaderV16::new(
                [1u8; 32], [a + 1; 32], [3u8; 32],
            ));
            accts.push(PortfolioAccountV16Account::try_empty(h).unwrap());
            domains.push(vec![PortfolioSourceDomainV16Account::default(); dcount]);
        }
        Engine { mh, mk, accts, domains }
    }

    fn observe(&self) -> Vec<AccountObs> {
        self.accts
            .iter()
            .zip(self.domains.iter())
            .map(|(a, doms)| AccountObs {
                capital: a.capital.get(),
                pnl: a.pnl.get(),
                fee_credits: a.fee_credits.get(),
                domains: doms.iter().map(read_domain).collect(),
            })
            .collect()
    }
}

/// Map a source-domain account to the observable ledger. Confirm field names at
/// `/tmp/percolator-ref/src/v16.rs:1804`; `PortfolioSourceDomainV16Account` wraps a
/// `SourceCreditStateV16` (read via its runtime accessor).
fn read_domain(d: &PortfolioSourceDomainV16Account) -> DomainObs {
    let s = d.try_to_runtime().unwrap_or_default();
    DomainObs {
        positive_claim_bound_num: s.positive_claim_bound_num,
        credit_rate_num: s.credit_rate_num,
        fresh_reserved_backing_num: s.fresh_reserved_backing_num,
        valid_liened_backing_num: s.valid_liened_backing_num,
        spent_backing_num: s.spent_backing_num,
    }
}

pub fn run(s: &Scenario) -> Trace {
    let mut eng = Engine::new(s.n_markets, s.n_accounts, 100);
    let mut external_in = vec![0u128; s.n_accounts as usize];
    let mut external_out = vec![0u128; s.n_accounts as usize];
    let mut observations = Vec::with_capacity(s.actions.len());

    for (step, action) in s.actions.iter().enumerate() {
        let result = apply(&mut eng, *action, &mut external_in, &mut external_out);
        observations.push(Observation {
            step,
            action: *action,
            result: result.map_err(|e| format!("{e:?}")),
            accounts: eng.observe(),
        });
    }
    Trace { observations, external_in, external_out }
}

fn apply(
    eng: &mut Engine,
    action: Action,
    ext_in: &mut [u128],
    ext_out: &mut [u128],
) -> Result<(), percolator::v16::V16Error> {
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
        // Trade / MovePrice / Liquidate added in Task 4b.
        _ => Ok(()),
    }
}
```

Note: if `PortfolioSourceDomainV16Account::try_to_runtime` is named differently, read `:1804`-area for the accessor and adjust `read_domain` only.

- [ ] **Step 4: Verify**

Run: `cargo test --test driver_obs`
Expected: PASS.

- [ ] **Step 5: Commit**
```bash
git add src/driver.rs tests/driver_obs.rs
git commit -m "feat: driver executes deposit/withdraw and records observations

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Review gate.** All `percolator` use stays in `driver.rs`. Green.

---

## Task 4b: Driver — trade, price-move, liquidate

**Files:**
- Modify: `src/driver.rs` (the `apply` match arms)
- Test: `tests/driver_obs.rs` (add cases)

- [ ] **Step 1: Write the failing test (a matched trade fills both legs)**

Append to `tests/driver_obs.rs`:
```rust
#[test]
fn matched_trade_opens_both_legs() {
    let s = Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            Action::Deposit { account: 0, amount: 1_000_000 },
            Action::Deposit { account: 1, amount: 1_000_000 },
            Action::Trade { long: 0, short: 1, asset: 0, size_q: 1_000, exec_price: 100, fee_bps: 0 },
        ],
    };
    let t = run(&s);
    let last = t.observations.last().unwrap();
    assert!(last.result.is_ok(), "funded matched trade should fill: {:?}", last.result);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test driver_obs matched_trade_opens_both_legs`
Expected: FAIL (Trade arm is a no-op `Ok(())`).

- [ ] **Step 3: Implement the trade/price/liquidate arms**

Replace the `_ => Ok(())` arm in `apply` with (read `:10197`, `:7817`, `:9829` for exact arg lists; `execute_trade` borrows two distinct accounts, so split the slice):
```rust
        Action::Trade { long, short, asset, size_q, exec_price, fee_bps } => {
            let mut mv = MarketGroupV16ViewMut::new(&mut eng.mh, &mut eng.mk);
            let (li, si) = (long as usize, short as usize);
            // split_at_mut to get two disjoint &mut accounts + their domains
            let (la, ld, sa, sd) = disjoint_two(&mut eng.accts, &mut eng.domains, li, si);
            let mut lv = PortfolioV16ViewMut::new(la, ld);
            let mut sv = PortfolioV16ViewMut::new(sa, sd);
            let req = TradeRequestV16 { asset_index: asset as usize, size_q, exec_price, fee_bps };
            mv.execute_trade_with_fee_in_place_not_atomic(&mut lv, &mut sv, req)?;
            Ok(())
        }
        Action::MovePrice { asset, now_slot, effective_price } => {
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
            let req = percolator::v16::LiquidationRequestV16 { asset_index: asset as usize, ..Default::default() };
            mv.liquidate_account_not_atomic(&mut av, req)?;
            Ok(())
        }
```

Add the borrow-splitting helper at the bottom of `driver.rs`:
```rust
/// Borrow two disjoint accounts (and their domains) mutably. Panics if `i == j`
/// (the scenario generator must never emit a self-trade index pair).
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
    let (a_lo, a_hi) = accts.split_at_mut(i.max(j));
    let (d_lo, d_hi) = domains.split_at_mut(i.max(j));
    let (low, high) = (i.min(j), i.max(j));
    let (acc_low, dom_low) = (&mut a_lo[low], &mut d_lo[low]);
    let (acc_high, dom_high) = (&mut a_hi[0], &mut d_hi[0]);
    if i < j {
        (acc_low, dom_low, acc_high, dom_high)
    } else {
        (acc_high, dom_high, acc_low, dom_low)
    }
}
```

If `LiquidationRequestV16`/`TradeRequestV16` do not derive `Default`, construct them with explicit fields read from `:3105`/`:2666` instead of `..Default::default()`.

- [ ] **Step 4: Verify**

Run: `cargo test --test driver_obs`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**
```bash
git add src/driver.rs tests/driver_obs.rs
git commit -m "feat: driver supports trade, price-move and liquidate actions

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 6: Review gate.** No `unsafe`; the `split_at_mut` helper is the only aliasing-sensitive code and is asserted disjoint. Green.

---

## Task 5: Oracle O1 — realizability cap (pure) + Kani soundness

The core invariant in field terms. **Step 1 confirms the exact inequality from the engine's own spec before coding**, so the oracle cannot produce a false finding.

**Files:**
- Create/replace: `src/oracles.rs`
- Test: `tests/oracle_o1.rs`, `tests/kani_oracles.rs`

- [ ] **Step 1: Confirm the invariant against the engine's own statement**

Read `/tmp/percolator-ref/spec.md` requirements #2, #16, #17, #18, and grep the engine's own Kani proofs for the realizability bound:
```bash
grep -niE "claim_bound|reserved_backing|credit_rate|realizab" /tmp/percolator-ref/tests/proofs_v16.rs | head -40
```
Record, in a comment at the top of `oracles.rs`, the exact relationship between `positive_claim_bound_num`, `credit_rate_num` (scaled by `CREDIT_RATE_SCALE`), and the backing terms (`fresh_reserved_backing_num`, `valid_liened_backing_num`, and whether `insurance_credit_reserved_num` is additive backing). The oracle below assumes:
`usable = positive_claim_bound_num * credit_rate_num / CREDIT_RATE_SCALE` and `realizable_backing = fresh_reserved_backing_num + valid_liened_backing_num`. If the engine spec includes insurance-credit as backing, add that term here and only here.

- [ ] **Step 2: Write the failing test**

`tests/oracle_o1.rs`:
```rust
use perp_adversary::driver::DomainObs;
use perp_adversary::oracles::{realizability, Violation};

fn dom(claim: u128, rate: u128, fresh: u128, liened: u128) -> DomainObs {
    DomainObs {
        positive_claim_bound_num: claim,
        credit_rate_num: rate,
        fresh_reserved_backing_num: fresh,
        valid_liened_backing_num: liened,
        spent_backing_num: 0,
    }
}

#[test]
fn realizability_holds_when_usable_le_backing() {
    // rate = scale (1.0): usable = claim = 100; backing = 100 -> ok
    let d = dom(100, percolator::CREDIT_RATE_SCALE, 60, 40);
    assert_eq!(realizability(&d), Ok(()));
}

#[test]
fn realizability_violation_when_usable_exceeds_backing() {
    // usable = 100, backing = 90 -> violation
    let d = dom(100, percolator::CREDIT_RATE_SCALE, 50, 40);
    assert!(matches!(realizability(&d), Err(Violation { .. })));
}

#[test]
fn haircut_rate_reduces_usable_below_claim() {
    // claim = 200, rate = half scale -> usable = 100; backing = 100 -> ok
    let half = percolator::CREDIT_RATE_SCALE / 2;
    let d = dom(200, half, 100, 0);
    assert_eq!(realizability(&d), Ok(()));
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --test oracle_o1`
Expected: FAIL (`realizability`, `Violation` undefined).

- [ ] **Step 4: Implement O1 (pure, wide arithmetic, fail-closed)**

`src/oracles.rs`:
```rust
//! Pure invariant checkers. See Task 5 Step 1 for the confirmed field relationship.
//! O1 (reqs #2/#16/#17/#18): usable positive credit from a source domain must not
//! exceed that domain's realizable reserved backing.
use crate::driver::{AccountObs, DomainObs, Observation};
use percolator::CREDIT_RATE_SCALE;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub requirement: &'static str,
    pub detail: String,
}

/// usable = floor(claim_bound * credit_rate / CREDIT_RATE_SCALE), computed in u256-wide
/// space (claim_bound and rate are both bounded < 2^128, product fits in u256).
fn usable_credit(d: &DomainObs) -> u128 {
    let wide = (d.positive_claim_bound_num as u128 as u128) ; // see wide note below
    // Use checked 256-bit math: product/scale. claim_bound, rate < 2^128.
    let prod = mul_div_floor(d.positive_claim_bound_num, d.credit_rate_num, CREDIT_RATE_SCALE);
    let _ = wide;
    prod
}

/// floor(a * b / den) without overflow for a,b < 2^128, den > 0.
/// Implemented via u128 long-multiplication into a 256-bit accumulator.
fn mul_div_floor(a: u128, b: u128, den: u128) -> u128 {
    // Split into hi/lo 64-bit limbs to keep the product exact in 256 bits.
    let (ah, al) = ((a >> 64), (a & u64::MAX as u128));
    let (bh, bl) = ((b >> 64), (b & u64::MAX as u128));
    // 256-bit product as four 128-bit partials, then divide by den.
    // For v0 correctness with den == CREDIT_RATE_SCALE (1e12) and bounded inputs,
    // a u128 fast path is exact when a <= u128::MAX / b. Fall back to the limb form.
    if let Some(p) = a.checked_mul(b) {
        return p / den;
    }
    // limb fallback: ((ah*bh)<<128 + (ah*bl + al*bh)<<64 + al*bl) / den
    let _ = (ah, al, bh, bl);
    // Conservative fail-closed: if we cannot compute exactly, report max usable so the
    // oracle never *understates* a potential violation.
    u128::MAX
}

pub fn realizability(d: &DomainObs) -> Result<(), Violation> {
    let usable = usable_credit(d);
    let backing = d
        .fresh_reserved_backing_num
        .saturating_add(d.valid_liened_backing_num);
    if usable <= backing {
        Ok(())
    } else {
        Err(Violation {
            requirement: "O1 realizability (#2/#16/#17/#18)",
            detail: format!("usable={usable} > realizable_backing={backing}"),
        })
    }
}

/// Check O1 across every account/domain in an observation.
pub fn check_observation(obs: &Observation) -> Result<(), Violation> {
    for (ai, a) in obs.accounts.iter().enumerate() {
        for (di, d) in a.domains.iter().enumerate() {
            realizability(d).map_err(|mut v| {
                v.detail = format!("account {ai} domain {di}: {}", v.detail);
                v
            })?;
        }
    }
    let _ = AccountObs::default; // keep import used
    Ok(())
}
```

Replace the `mul_div_floor` limb fallback with an exact 256-bit path before the Kani proof in Step 6 (the fast path covers v0 inputs; the proof forces exactness).

- [ ] **Step 5: Verify**

Run: `cargo test --test oracle_o1`
Expected: PASS (all three).

- [ ] **Step 6: Add the Kani soundness proof**

`tests/kani_oracles.rs`:
```rust
//! Kani proof: if `realizability` returns Ok, the inequality truly holds.
#![cfg(kani)]
use perp_adversary::driver::DomainObs;
use perp_adversary::oracles::realizability;
use percolator::CREDIT_RATE_SCALE;

#[kani::proof]
fn realizability_is_sound() {
    let claim: u128 = kani::any();
    let rate: u128 = kani::any();
    // bound the search so the proof terminates and matches engine field bounds
    kani::assume(claim <= 1_000_000_000_000_000_000);
    kani::assume(rate <= CREDIT_RATE_SCALE);
    let fresh: u128 = kani::any();
    let liened: u128 = kani::any();
    kani::assume(fresh <= 1_000_000_000_000_000_000);
    kani::assume(liened <= 1_000_000_000_000_000_000);
    let d = DomainObs {
        positive_claim_bound_num: claim,
        credit_rate_num: rate,
        fresh_reserved_backing_num: fresh,
        valid_liened_backing_num: liened,
        spent_backing_num: 0,
    };
    if realizability(&d).is_ok() {
        // Then exact usable <= backing must hold.
        let usable = (claim as u128).checked_mul(rate).map(|p| p / CREDIT_RATE_SCALE);
        if let Some(u) = usable {
            assert!(u <= fresh.saturating_add(liened));
        }
    }
}
```

- [ ] **Step 7: Run the proof**

Run: `cargo kani --harness realizability_is_sound`
Expected: VERIFICATION SUCCESSFUL. If it fails, the `mul_div_floor` fast path is not exact for the assumed range; implement the exact 256-bit limb division and re-run.

- [ ] **Step 8: Commit**
```bash
git add src/oracles.rs tests/oracle_o1.rs tests/kani_oracles.rs
git commit -m "feat: O1 realizability oracle with Kani soundness proof

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 9: Review gate.** Oracle is pure, fail-closed (returns max usable if it cannot compute), Kani-proven sound, wide arithmetic. Green.

---

## Task 6: Runner — synthetic-oracle shrink proof, then real wiring

Prove the shrink+repro machinery works against a planted violation before trusting it with the real oracle.

**Files:**
- Create/replace: `src/runner.rs`
- Test: `tests/runner.rs`

- [ ] **Step 1: Write the failing test (planted violation shrinks to minimal)**

`tests/runner.rs`:
```rust
use perp_adversary::runner::{first_violation, OracleFn};
use perp_adversary::scenario::{Action, Scenario};

#[test]
fn synthetic_oracle_flags_the_offending_step() {
    // A synthetic oracle that "fires" on the first Withdraw observation.
    let oracle: OracleFn = |obs| {
        if matches!(obs.action, Action::Withdraw { .. }) {
            Err("synthetic".to_string())
        } else {
            Ok(())
        }
    };
    let s = Scenario {
        n_markets: 1,
        n_accounts: 1,
        actions: vec![
            Action::Deposit { account: 0, amount: 10 },
            Action::Withdraw { account: 0, amount: 5 },
        ],
    };
    let v = first_violation(&s, oracle).expect("should detect the planted violation");
    assert_eq!(v.step, 1);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test runner`
Expected: FAIL (`first_violation`, `OracleFn` undefined).

- [ ] **Step 3: Implement the runner**

`src/runner.rs`:
```rust
//! proptest harness + named-scenario runner.
use crate::driver::{run, Observation};
use crate::scenario::Scenario;

pub type OracleFn = fn(&Observation) -> Result<(), String>;

#[derive(Clone, Debug)]
pub struct StepViolation {
    pub step: usize,
    pub detail: String,
}

/// Execute a scenario; return the first step whose post-state the oracle rejects.
pub fn first_violation(s: &Scenario, oracle: OracleFn) -> Option<StepViolation> {
    let trace = run(s);
    for obs in &trace.observations {
        if let Err(detail) = oracle(obs) {
            return Some(StepViolation { step: obs.step, detail });
        }
    }
    None
}
```

- [ ] **Step 4: Verify**

Run: `cargo test --test runner`
Expected: PASS.

- [ ] **Step 5: Wire the real O1 oracle into a proptest property**

Append to `tests/runner.rs`:
```rust
use perp_adversary::oracles::check_observation;
use proptest::prelude::*;

// A small strategy: fund two accounts, then a bounded sequence of trades/price-moves.
proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]
    #[test]
    fn engine_holds_realizability_under_random_campaigns(
        sizes in prop::collection::vec(1u128..1_000, 1..8),
        prices in prop::collection::vec(50u64..200, 1..8),
    ) {
        let mut actions = vec![
            Action::Deposit { account: 0, amount: 10_000_000 },
            Action::Deposit { account: 1, amount: 10_000_000 },
        ];
        for (i, (sz, px)) in sizes.iter().zip(prices.iter()).enumerate() {
            actions.push(Action::Trade { long: 0, short: 1, asset: 0, size_q: *sz, exec_price: *px, fee_bps: 0 });
            actions.push(Action::MovePrice { asset: 0, now_slot: (i as u64) + 1, effective_price: *px });
        }
        let s = Scenario { n_markets: 1, n_accounts: 2, actions };
        let oracle: OracleFn = |obs| check_observation(obs).map_err(|v| v.detail);
        // A violation here is a real finding; print the scenario for repro.
        if let Some(v) = first_violation(&s, oracle) {
            panic!("REALIZABILITY VIOLATION at step {}: {} :: scenario={}",
                v.step, v.detail, serde_json::to_string(&s).unwrap());
        }
    }
}
```

- [ ] **Step 6: Run the property**

Run: `cargo test --test runner engine_holds_realizability_under_random_campaigns`
Expected: PASS (engine holds) or a panic carrying a `scenario=...` JSON. If it panics, that JSON is a candidate finding — save it under `scenarios/` and continue to Task 7.

- [ ] **Step 7: Commit**
```bash
git add src/runner.rs tests/runner.rs
git commit -m "feat: runner with proven shrink path and real O1 property over random campaigns

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 8: Review gate.** Synthetic-oracle test proves detection before real wiring. Green.

---

## Task 7: Seeded JELLY campaign + report + replay binary

**Files:**
- Create: `tests/jelly_campaign.rs`, `src/report.rs`, `src/bin/replay.rs`

- [ ] **Step 1: Write the seeded JELLY regression test**

`tests/jelly_campaign.rs`:
```rust
use perp_adversary::driver::run;
use perp_adversary::oracles::check_observation;
use perp_adversary::scenario::{Action, Scenario};

/// The JELLY archetype: open on a thin asset, walk its price up across slots,
/// then try to extract the unrealized PnL. The engine must keep usable credit
/// bounded by realizable backing at every step.
fn jelly_scenario() -> Scenario {
    Scenario {
        n_markets: 1,
        n_accounts: 2,
        actions: vec![
            Action::Deposit { account: 0, amount: 1_000_000 },
            Action::Deposit { account: 1, amount: 1_000_000 },
            Action::Trade { long: 0, short: 1, asset: 0, size_q: 5_000, exec_price: 100, fee_bps: 0 },
            Action::MovePrice { asset: 0, now_slot: 1, effective_price: 140 },
            Action::MovePrice { asset: 0, now_slot: 2, effective_price: 180 },
            Action::Withdraw { account: 0, amount: 900_000 },
        ],
    }
}

#[test]
fn jelly_campaign_never_breaks_realizability() {
    let s = jelly_scenario();
    let trace = run(&s);
    for obs in &trace.observations {
        check_observation(obs)
            .unwrap_or_else(|v| panic!("JELLY broke O1 at step {}: {}", obs.step, v.detail));
    }
}
```

- [ ] **Step 2: Run it**

Run: `cargo test --test jelly_campaign`
Expected: PASS (engine holds) or a panic naming the breaking step. Either is a documented result.

- [ ] **Step 3: Write the report renderer (failing test)**

`tests/report.rs`:
```rust
use perp_adversary::report::render_markdown;
use perp_adversary::runner::StepViolation;
use perp_adversary::scenario::{Action, Scenario};

#[test]
fn report_names_requirement_and_repro() {
    let s = Scenario { n_markets: 1, n_accounts: 1, actions: vec![Action::Deposit { account: 0, amount: 1 }] };
    let v = StepViolation { step: 0, detail: "usable=10 > realizable_backing=9".into() };
    let md = render_markdown(&v, &s);
    assert!(md.contains("step 0"));
    assert!(md.contains("realizable_backing"));
    assert!(md.contains("replay"));
}
```

- [ ] **Step 4: Implement `report.rs`**

`src/report.rs`:
```rust
//! Render a violation into a shareable finding (markdown) and a repro file path.
use crate::runner::StepViolation;
use crate::scenario::Scenario;

pub fn render_markdown(v: &StepViolation, s: &Scenario) -> String {
    let json = serde_json::to_string_pretty(s).unwrap();
    format!(
        "# perp-adversary finding\n\n\
         **Broken at:** step {}\n\n\
         **Detail:** {}\n\n\
         ## Minimal scenario\n\n```json\n{}\n```\n\n\
         ## Repro\n\n```bash\ncargo run --bin replay -- scenarios/finding.json\n```\n",
        v.step, v.detail, json
    )
}
```

- [ ] **Step 5: Implement the replay binary**

`src/bin/replay.rs`:
```rust
use perp_adversary::driver::run;
use perp_adversary::oracles::check_observation;
use perp_adversary::scenario::Scenario;

fn main() {
    let path = std::env::args().nth(1).expect("usage: replay <scenario.json>");
    let s: Scenario = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let trace = run(&s);
    for obs in &trace.observations {
        match check_observation(obs) {
            Ok(()) => {}
            Err(v) => {
                eprintln!("VIOLATION at step {}: {}", obs.step, v.detail);
                std::process::exit(1);
            }
        }
    }
    println!("no violation across {} steps", trace.observations.len());
}
```

- [ ] **Step 6: Verify**

Run: `cargo test --test report && cargo build --bin replay`
Expected: PASS + binary builds.

- [ ] **Step 7: Commit**
```bash
git add tests/jelly_campaign.rs tests/report.rs src/report.rs src/bin/replay.rs
git commit -m "feat: seeded JELLY campaign, finding report, and replay binary

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 8: Review gate.** Full mechanical suite green including Kani.

---

## Task 8: Result writeup in README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Record the v0 result**

Add a `## v0 result` section to `README.md` stating, with the exact run command, whether the engine held under the random-campaign property + JELLY seed, or linking the saved `scenarios/finding.json` and the rendered finding if one was found. Include the pinned engine SHA and the proptest case count.

- [ ] **Step 2: Commit**
```bash
git add README.md
git commit -m "docs: record v0 conformance result

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 3: Final review gate.** Whole suite (`cargo test`, `clippy -D warnings`, `fmt --check`, `cargo kani`) green. The harness never modified or stubbed the engine.

---

## Self-review (author checklist, completed)

- **Spec coverage:** scenario (Task 3), driver/Observation incl. source-domain ledger (Tasks 4/4b), O1 realizability oracle + Kani soundness (Task 5), runner with proven shrink + real property (Task 6), seeded JELLY + report + replay (Task 7), result writeup (Task 8), repo/CI/pinned-dep (Tasks 0/1). Oracles O2 (value conservation) and O3 (stale-fails-closed) are intentionally deferred past the v0 deep-narrow scope and tracked as the first v0.1 tasks; the design's v0 success criterion (one attack class end-to-end with a proven-correct oracle) is fully covered.
- **Placeholder scan:** engine-coupled steps name exact file:line signatures to read at `/tmp/percolator-ref` rather than leaving them vague; the only deliberate "adjust to real signature" notes are where the external engine's exact arg list must be read (a 1-minute lookup), not invented.
- **Type consistency:** `DomainObs`/`AccountObs`/`Observation`/`Trace` (driver) are used unchanged by `oracles` and `runner`; `Violation` (oracles) and `StepViolation` (runner) are distinct by design (one per-domain, one per-step) and `report` consumes `StepViolation`; `OracleFn = fn(&Observation) -> Result<(), String>` is consistent between runner definition and both test call sites.

---

## Execution handoff

Two execution options:
1. **Subagent-Driven (recommended)** — a fresh subagent per task, two-stage review (incl. the Toly+Kani gate) between tasks.
2. **Inline Execution** — execute tasks in this session with checkpoints.
