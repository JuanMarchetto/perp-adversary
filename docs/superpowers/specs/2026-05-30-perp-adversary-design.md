# perp-adversary — Design Spec

**Status:** approved design, source of truth for implementation.
**Date:** 2026-05-30.
**Owner:** JuanMarchetto.
**License:** Apache-2.0.

---

## 1. Purpose & success criteria

`perp-adversary` is an adversarial economic conformance harness for Solana perpetual-futures risk engines. v0 targets [Percolator](https://github.com/aeyakovenko/percolator) (`aeyakovenko/percolator`, Apache-2.0).

It drives the **real** Percolator engine through **multi-step adversarial sequences** and checks the spec's **economic invariants as runtime oracles** after every step. When a sequence violates an invariant, proptest shrinks it to a **minimal, reproducible counterexample** that is emitted as a structured finding.

**Why this is novel / where it sits.** Percolator already ships per-instruction Kani proofs (`tests/proofs_v16.rs`, `kani-list.json`) and single-path proptest fuzzing (`tests/v16_fuzzing.rs`, behind the `fuzz` feature). Those check *one instruction* at a time. `perp-adversary` checks the *economic* invariant **across a campaign** — multiple accounts, multiple slots, oracle manipulation, margin withdrawal, liquidation, ADL — which bounded per-function model checking does not express.

**Success = one of:**
- a minimal reproducible sequence where the engine lets an economic invariant break (a real, PR-worthy finding), **or**
- a strong negative result: under heavy adversarial search the engine holds, with the search space and oracles documented (a credible conformance signal).

Either outcome is valuable. The bar: rigorous enough that the Percolator/Kani community trusts it, including a **formally-proven-correct oracle** (see §7) so a reported finding can never be a harness bug.

## 2. Grounding facts (verified 2026-05-30)

- `percolator` is a single `#![no_std]`, `#![deny(unsafe_code)]` Rust lib crate (edition 2021, Apache-2.0). Sole dep `bytemuck` (zero-copy). dev-dep `proptest 1.4`.
- Engine lives in `src/v16.rs` (~482 KB) + `src/wide_math.rs` (≥256-bit arithmetic). `src/lib.rs` re-exports `v16::*` and the scaled constants.
- Normative spec: `spec.md` **v16.8.5**, 37 non-negotiable requirements. Core invariant (verbatim):
  ```
  usable_positive_credit_from_source_domain
      <= realizable_counterparty_backing_reserved_for_that_domain
  ```
  Headline threat (verbatim): *"the attacker cannot transform uncollectible A paper profit into global B purchasing power."*
- Engine driving surface (from `tests/v16_spec_tests.rs`): construct `MarketGroupV16HeaderAccount` + `Vec<Market<u64>>`, build a `MarketGroupV16ViewMut::new(&mut header, &mut markets)`; per account build `PortfolioAccountV16Account` + `Vec<PortfolioSourceDomainV16Account>` and a `PortfolioV16ViewMut::new(...)`. Instructions are methods such as `activate_empty_market_not_atomic`, `deposit_not_atomic`, withdraw/trade/liquidate paths, the `PermissionlessCrank*` family; `validate_shape()` checks structural invariants; errors are `V16Error`. The `_not_atomic` suffix marks the building-block ops; atomicity = call + revalidate + rollback-on-error.
- `Cargo.toml` reserves a feature `stress = "Reserved for stress/audit harnesses"` — an intended hook for exactly this kind of tool.
- Toolchain present: Rust 1.94, surfpool 1.0-rc1, cargo-fuzz 0.13. **Kani is NOT installed** → setup step: `cargo install kani-verifier && cargo kani setup`.

## 3. Non-goals (YAGNI for v0)

- No on-chain / surfpool execution. In-process driving of the real engine is deterministic and faster; surfpool integration is deferred behind the `driver` interface.
- No Drift/Pacifica adapters. Venue-neutrality is preserved by the `driver` boundary, not built yet.
- No production/venue layer (that is a separate repo).
- No UI/dashboard.
- No second attack class. v0 is **one** class: realizability / oracle-walk (the JELLY archetype).

## 4. Architecture

A new crate that **depends on `percolator` as a pinned git dependency** (a specific commit SHA) and never forks or modifies it. Five isolated components, each with one purpose and a well-defined interface:

```
proptest strategy ──▶ Scenario (pure data)
                          │
                          ▼
                       driver  ──drives──▶  real `percolator` engine
                          │
                          ▼
                    Trace<Observation>
                          │
                          ▼
                       oracles  ──pure──▶  Ok | Violation(req#)
                          │ (on Violation)
                          ▼
                    proptest shrink ──▶ minimal Scenario
                          │
                          ▼
                        report  ──▶ structured finding + 1-line repro
```

Determinism is mandatory end to end: no wall clock, no RNG inside `driver`/`oracles` (proptest owns seeding); slots are explicit integers in the `Scenario`; the engine is deterministic.

## 5. Components

### 5.1 `scenario` (pure)
A serializable, deterministic campaign description.
- `Action` enum: `Deposit{account, amount}`, `Trade{account, asset, side, size_q}`, `MoveOraclePrice{asset, new_price}` (bounded by the engine's `max_price_move_bps_per_slot`), `Withdraw{account, amount}`, `Crank{...}`, `Liquidate{liquidatee, ...}`, `AdvanceSlot{by}`.
- `Scenario { config: V16ConfigSeed, n_markets, n_accounts, actions: Vec<Action> }`.
- Pure: no engine calls. Owns its own `proptest::Arbitrary`-style strategy and a stable serde form for repro files.
- **Interface:** construct, serialize/deserialize, iterate actions. Depends on nothing but the action vocabulary.

### 5.2 `driver` (the only `percolator`-coupled component)
- `fn run(scenario: &Scenario) -> Trace` — builds the real engine views, executes each `Action` by mapping it to the real instruction call, captures a post-step `Observation`.
- `Observation` records, per step: which action, the resulting `Result<(), V16Error>`, and the **observable economic state** needed by oracles (per-account: principal/quote balance, per-asset net leg, realized vs unrealized PnL, source-domain credit + realizable backing fields, extracted-value running totals). Exact `V16` field mapping is the first implementation task (TDD: a test pins each mapped field against engine behavior before the mapping is written).
- **Interface:** `Scenario -> Trace`. All Percolator types stay behind this module. If the engine API changes, only this file changes.

### 5.3 `oracles` (pure)
- Each oracle: `fn check(prev: &Observation, cur: &Observation) -> Result<(), Violation>`, mapped to a numbered spec requirement.
- v0 oracle set:
  - **O1 — Realizability cap (reqs #2, #6, #15):** no step may let an account extract quote value (withdrawal, fee-from-PnL) or back new risk using positive PnL beyond the realizable source-domain backing. Operational form: maintain an extraction ledger; `extracted_quote(account) <= deposited_principal(account) + realized_backed_pnl(account)` at all times, and any new-risk approval must be covered by principal + *realizable* (non-stale, backed) credit, never by raw unrealized PnL above its source-domain backing.
  - **O2 — Quote-value conservation (req #13):** total quote atoms are conserved across the vault per step (deposits/withdrawals/fees/insurance balance); no value is minted by rounding (req #14 sink).
  - **O3 — Hints discovery-only / stale fails closed (reqs #32, #16):** omitting or staleness of a position/backing never *improves* an account's health or usable credit.
- Pure functions, independently unit- + property-tested, and Kani-proven sound (§7).

### 5.4 `runner`
- Ties `proptest` strategy → `driver::run` → all oracles on every step → on first violation, return it so proptest shrinks the `Scenario`.
- Also runs **named, hand-seeded scenarios** (the JELLY campaign) deterministically, independent of proptest, for regression and documentation.
- **Interface:** `run_property(config)`, `run_named(scenario)`.

### 5.5 `report`
- `fn render(violation: &Violation, minimal: &Scenario, trace: &Trace) -> Finding` — structured: violated requirement number + text, the minimal action sequence, engine state before/after the offending step, and a one-command repro (`cargo run -p perp-adversary --bin replay -- <scenario.json>`).
- Output both machine (JSON) and human (markdown) forms.

## 6. The v0 attack class: realizability / oracle-walk (JELLY archetype)

A hand-seeded campaign, then its fuzzed generalization:
1. Attacker account opens a position in a thin asset A.
2. Walk A's oracle price within the engine's allowed per-slot bound across several `AdvanceSlot` steps, accruing positive unrealized PnL on A.
3. Attempt extraction/leverage: (a) withdraw quote against the A PnL, (b) pay fees from it, (c) open new risk in asset B using the A PnL as support.
4. Optionally bankrupt the A counterparty side so the source-domain backing collapses.
5. Oracle O1 asserts the engine never let unbacked A paper-profit become withdrawn quote / fee payment / B-risk support beyond realizable A backing.

The fuzzed form lets proptest choose assets, sizes, price-walk magnitudes, slot counts, account counts, and interleavings; the seeded narrative is the known-interesting starting point and a permanent regression.

## 7. Kani's role (prove the checker, not the engine)

Kani cannot model-check campaign-length sequences against the 482 KB engine (path explosion). Instead Kani is applied to **our own pure code** to make findings trustworthy:
- **Oracle soundness:** prove that if an oracle returns `Ok`, the underlying spec inequality actually holds for the observed values (no false negatives within bounded value ranges).
- **Shrinker preservation:** prove the minimization step preserves the violating property (a shrunk scenario still violates).
- Arithmetic in oracles uses checked/wide ops mirroring `wide_math` so the proofs are about real integer behavior, not wrapped values.

This is the differentiator: a fuzzer whose oracle is formally proven correct. CI runs these Kani proofs alongside the proptest suite.

## 8. Error handling

`V16Error` from the engine is **expected behavior**, recorded in the `Trace`, never a violation (a rejected withdrawal is the engine working). A **Violation** is strictly: the engine *accepted* an action that left an economic invariant false. Oracles fail **closed**: if an oracle cannot evaluate (missing/stale observation), it reports suspicion, not `Ok` (mirrors reqs #16/#32).

## 9. Testing strategy — furious TDD (hard rule)

No production line is written without a failing test that specifies its behavior first. Per component:
- `scenario`, `oracles`, `report` (pure): unit tests + proptest properties + Kani proofs, written before implementation.
- `driver`: each mapped observation field is pinned by a test against real engine behavior before the mapping code exists; a rejected-vs-accepted action is asserted to land in the right `Trace` slot.
- `runner`: first wired against a **synthetic oracle known to fire** to prove shrink + repro work, before the real oracle is connected.
- Red → green → refactor on every unit.

## 10. Toly + Kani review gate (after every step)

A step is not done until it passes this checklist:
- **Kani optics:** deterministic and total (no panics on valid input; explicit `Result`); zero `unsafe`; no unbounded loops in pure code; invariants stated as checkable predicates; checked/wide arithmetic (no silent overflow); fail-closed.
- **Toly optics (the spec's own ethos):** account-local / bounded work (no full-market scans in our hot path either); no reliance on self-trade/identity detection; conservative (never understate a violation); `no_std`-friendly where it touches engine types; terse, high-signal code; and **never weaken, stub, or special-case the engine to produce a result.**
- Mechanical gates: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, and the Kani proofs all green.

## 11. Repo & build

- Repo `perp-adversary` on `JuanMarchetto`, Apache-2.0. Cargo workspace.
- Dependency: `percolator = { git = "https://github.com/aeyakovenko/percolator", rev = "<pinned SHA>" }`. Pin a reviewed commit; bump deliberately (the engine moves fast).
- Toolchain: Rust 1.94; `proptest`; `kani-verifier` (install + `cargo kani setup`); `cargo-fuzz` optional.
- CI: build, `cargo test`, `clippy -D warnings`, `fmt --check`, Kani proofs on the pure crates. (`stress`/audit features of Percolator may be enabled for richer observation if needed.)

## 12. Milestone shape for v0 (detail comes from the implementation plan)

1. Scaffold workspace + CI + pinned `percolator` dep; one trivial driver smoke test (deposit/withdraw round-trip) green.
2. `scenario` vocabulary + strategies (TDD).
3. `driver` observation mapping (TDD, field by field, pinned to real engine).
4. `oracles` O1–O3 (TDD) + Kani soundness proofs.
5. `runner` with synthetic-oracle shrink/repro proof, then real wiring.
6. Seeded JELLY campaign + fuzzed generalization; `report` artifacts.
7. README + a written-up first result (finding or conformance signal).

## 13. Open questions / risks

- Exact `V16` field names for "realizable backing", "source-domain credit", and "extracted value" must be mapped from the engine in step 3 (treated as a TDD task, not guessed here).
- Engine churns fast (daily pushes); the pinned-SHA discipline contains this. Re-pin on a cadence.
- Kani is not installed locally; setup is a plan step and a CI dependency.
- If O1 needs internal engine state not exposed publicly, the driver may need Percolator's `stress`/`audit-scan` features or a thin read-only accessor (prefer the public surface; escalate only if blocked).
