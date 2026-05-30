# perp-adversary

An adversarial economic conformance harness for Solana perpetual-futures risk engines.

## What it does

`perp-adversary` drives a real perps risk engine through **multi-step adversarial campaigns** (matched trades, position-aware price cranks, source-credit-lien draws, margin withdrawal, liquidation across multiple accounts and slots) and checks an engine **economic invariant as a runtime oracle after every step**. When a step breaks the invariant, the failing campaign is reported with a one-command repro. Campaigns are driven into genuinely populated source-credit-lien state — not the all-zero default where the invariant holds trivially (see "v0 result").

v0 targets [Percolator](https://github.com/aeyakovenko/percolator) at commit `71c9032`. It is an **independent harness that depends on Percolator (Apache-2.0); it does not fork or modify it.** It complements Percolator's existing per-instruction Kani proofs and single-path fuzzing by checking the invariant *across a whole campaign*, which bounded per-function model checking does not express.

## The O1 oracle (and why it is trustworthy)

v0 checks one invariant: **source-domain realizability** — the spec's core promise that *"the attacker cannot transform uncollectible A paper profit into global B purchasing power"* (spec.md v16.8.5, Requirement #2).

The oracle does not invent this invariant. It re-implements, field for field, Percolator's own per-account validator `SourceCreditLienAggregateProofV16::validate()` (`src/v16.rs:3060-3100`), re-running its six relationships (face decomposition, claim-bound, the `effective ≤ ceil(face/SCALE)` realizability cap, atom alignment, reservation exactness, impaired well-formedness) as an independent oracle after every observed step.

The oracle's arithmetic core is **proven sound with Kani** (`realizability_is_sound`): within the engine's documented operating range, if the oracle clears a domain then all six exact inequalities hold. So a reported finding can never be a harness arithmetic bug. The proof follows the engine's own bounded-proof discipline (small whole-atom magnitudes scaled by `BOUND_SCALE`, plus symbolic sub-scale remainders covering the ceiling and alignment boundaries).

## v0 result

Against the pinned engine (`71c9032`), the engine **held source-domain realizability on every campaign — and, critically, those campaigns now reach genuinely populated source-credit-lien state, so the result is not vacuous.**

Reaching that state took engine-faithful work. The O1 relationships hold trivially over an all-zero `DomainObs`, so a campaign only tests the oracle if it drives the engine to draw a **non-zero source-credit lien**. The engine draws one lien, in one place: `execute_trade`'s `create_initial_margin_source_lien_if_needed` (`src/v16.rs:10260`), and only when a **risk-increasing** trade is made by an account that already holds positive, source-attributed PnL backed by counterparty backing. The engine's design then gets in the way of reaching this through a price walk, *by intent*: `accrue_asset_to_not_atomic` advances the permissionlessly-cranked `effective_price` (`src/v16.rs:7894`) but deliberately does **not** touch the authenticated anchor `raw_oracle_target_price` (written only by the authenticating activation path, `src/v16.rs:4407`). That separation is the safety property — if the permissionless crank could move the authenticated reference, a price walk would self-authenticate. The resulting target/effective lag is a documented, Kani-asserted conservative interlock: it locks **risk-increasing** trades (`validate_trade_position_preflight`, `src/v16.rs:8557`) and is priced into margin as `target_effective_lag_loss_penalty` (`src/v16.rs:905`). So a pure price walk that creates the PnL also, by design, blocks the risk-increasing trade that would draw a lien against it. (We verified this is intended, not a defect, via reproduction + code path + the spec + the engine's own crank Kani proof.)

So the harness establishes the lien precondition **the way the engine's own conformance test does** (`v16_risk_increasing_trade_creates_source_credit_lien_for_im`, `tests/v16_spec_tests.rs:580`): a `SeedSourceClaim` action seeds the account's positive source-attributed PnL and the matching counterparty backing — the market side via the engine's **public** `add_source_positive_claim_bound_not_atomic` / `add_fresh_counterparty_backing_not_atomic` entrypoints, the per-account side exactly as the engine's `account_fixture` seeds it — and then asks the engine to **validate** the resulting state. A real risk-increasing trade at the no-lag activation price then makes the engine run its genuine lien-drawing logic. Nothing in the engine is stubbed or weakened.

On the now-populated campaigns the engine **held O1 at every step**:

- a proptest property over **256 randomized** lien-drawing campaigns (random seeded claims × position sizes, two accounts) — every campaign drew a real lien (observed `source_lien_effective_reserved` spanning ~1–200 atoms across the range, the realizability cap and the rejection path both exercised) — **no violation**;
- a populated **JELLY-archetype** campaign — the account accumulates a source claim, the opening trade draws a lien of 100 atoms, and a second lever attempt is correctly **rejected** (`LockActive`) so the lien stays capped at realizable backing — **no violation at any step**;
- an **anti-vacuity gate** (`tests/anti_vacuity.rs`) that fails loudly if any future change makes campaigns stop producing a non-zero `source_claim_liened_num` — turning vacuity into a permanent test failure.

This is a **conformance signal**, not a proof of correctness. It means: across the adversarial campaigns this harness drove — including states where the engine had actually drawn source-credit liens — the engine never left an accepted observable state that violated its own per-account source-credit lien-aggregate invariant. No candidate violation appeared; if one ever does, the proptest persists its scenario to `scenarios/realizability_candidate.json` and the run fails loudly (the oracle is never weakened to pass).

## v0.2 — the liquidation insurance-isolation oracle

The **liquidation attack class**: a liquidation must settle a bankrupt account's loss against *its own* domain's insurance backstop — it must never let the liquidated account's loss drain a **different** domain's shared insurance. This is an economic isolation guarantee between domains (e.g. liquidating an under-funded JELLY position must not silently spend BTC's insurance).

This is a **cross-step (delta) property**, and that is the whole point. After a liquidation the driver runs `validate_shape` + `validate_with_market`, so the post-liquidation state has already passed every *single-state* validator — re-checking any single state would be **vacuous** (the trap this class fell into twice). The isolation property only exists as a *change*: `insurance_domain_spent` is a running per-domain total, so isolation can only be observed by comparing each domain's value **before vs after** the liquidation against the step's reported `insurance_used`.

The oracle (`liquidation_insurance_isolation`, `src/oracles.rs`) mirrors, line for line, the engine's own `consume_domain_insurance_for_negative_pnl` (`src/v16.rs:5949`): a liquidation of an `asset` leg charges **only** `insurance_domain_index(asset, opposite_side(leg.side))` (`src/v16.rs:5955`) — a domain of *that asset* — and increments exactly that one domain's `insurance_domain_spent` by exactly the `insurance_used` it returns (`src/v16.rs:5974-5980`). Over the `(prev, cur)` observation deltas the oracle enforces, fail-closed: (I1) every domain's spend is **monotone**; (I2) **isolation** — only the liquidated asset's domains may increase (no cross-domain drain); (I3) **full accounting** — the total spend increase across all observed domains equals `insurance_used`. (I2)+(I3) together pin `insurance_used` to the liquidated domain's delta and force every other domain's delta to zero — exactly the engine's per-domain guarantee, as asserted by its own conformance test (`tests/v16_spec_tests.rs:342-348`) and bounded by its Kani proofs (`tests/proofs_v16.rs:2375` budget caps the spend; `:2410` reserved insurance is not double-spent; `:2449` an unfunded domain cannot drain shared insurance).

The oracle's core is **proven sound with Kani** (`liquidation_insurance_isolation_is_sound`, `tests/kani_liquidation.rs`): if it clears a liquidation step then no non-liquidated-asset domain's spend increased and `insurance_used` is fully accounted by the liquidated asset's spend delta. The proof models the engine's four-domain layout (two assets × long/short) with symbolic pre/post spends, a symbolic liquidated asset, and a symbolic `insurance_used` — the same bounded discipline as the O1 / cross-link proofs.

**Non-vacuity, the strong way.** The isolation oracle is meaningful even on the no-insurance path (it asserts *no* domain's insurance was spent). But the stronger test is a liquidation where insurance **is** spent for the correct domain and **none** for any other. The harness reaches that engine-accepted state: `funded_liquidation_campaign` funds the long leg's bankruptcy domain (`insurance_domain_budget_short`) exactly where the engine's own `proof_v16_view_domain_budget_caps_bankruptcy_insurance_spend` (`tests/proofs_v16.rs:2375`) funds it, then liquidates through the **public** entrypoint. Result: `insurance_used = 5`, the asset's short-side `insurance_domain_spent` rises 0 → 5, and *every other domain stays at 0* (`tests/liquidation.rs` gate). Nothing in the engine is stubbed or weakened.

**v0.2 result.** Against the pinned engine (`71c9032`), the engine **held all three oracles across both liquidation campaigns** (the residual/no-insurance path and the funded/insurance-spent path): O1 and the v0.1 cross-link held at every observed step (conformance under liquidation stress), and the v0.2 isolation oracle held on every consecutive step (`tests/liquidation_conformance.rs`). No candidate appeared; if one ever does, the scenario is persisted to `scenarios/liquidation_candidate.json` and the run fails loudly (the oracle is never weakened to pass).

### Scope and honest limits

- **Three oracles.** O1 covers the per-account source-domain relationships (`src/v16.rs:3060`). The **v0.1 market-engine cross-link** oracle (`check_observation_market`) adds the outer realizability link `source_claim_bound_num ≤ positive_claim_bound_num` (per-account vs market-engine `positive_claim_bound_num`) plus the `market_id` binding — mirroring the engine's *composing* validator (`src/v16.rs:2143-2253`), i.e. the one relationship neither single-state validator enforces in isolation. (Re-checking the market-engine *single-state* validator would be vacuous: `try_to_runtime` runs it before the driver can observe the state, so any observable market-engine state has already passed it.) The **v0.2 liquidation insurance-isolation** oracle (`liquidation_insurance_isolation`) adds the *cross-step* domain-isolation guarantee above — the one property no single-state validator can express, because it lives in the delta. All three oracles are Kani-proven sound; the engine held all three across all campaigns.
- **The lien precondition is seeded, not price-walked.** By design the authenticated oracle target is immutable after activation (the permissionless crank moves only `effective_price`), so a sustained price move deliberately locks risk-increase — there is no in-band path to draw a lien through a pure price walk. The harness therefore seeds the same precondition the engine's own conformance test seeds and lets the engine draw the lien; the lien-drawing trade, the lien arithmetic, and the validation are all the engine's real code over a state it validates. (If target re-authentication for a live asset is ever exposed in-engine, the same campaign could be driven end-to-end by `Crank`, which is already implemented.)
- **Two attack classes.** Realizability (v0/v0.1) and liquidation insurance-isolation (v0.2). **ADL** (`apply_quantity_adl_after_residual_for_account_not_atomic`, `src/v16.rs:9479`) is modeled in the engine and reachable but **not yet adversarially explored** — the next attack class.
- **The funded-liquidation domain budget is seeded, not earned.** To make insurance genuinely spent (`insurance_used > 0`) the funded campaign sets `insurance_domain_budget_short` directly — exactly where the engine's own `proof_v16_view_domain_budget_caps_bankruptcy_insurance_spend` sets it — and then liquidates through the public entrypoint over an engine-validated state. The liquidation, the insurance spend, and the validation are all the engine's real code; only the pre-funded budget is seeded (no public budget-funding entrypoint exists to walk to it).
- **In-process, `_not_atomic` driving.** The harness calls the engine's building-block operations directly and observes after each; it does not yet exercise full atomic-instruction compositions or on-chain execution.
- **Bounded Kani proof**, matching the engine's own proof sizing — not an unbounded u128 proof (intractable for the model checker, as it is for the engine's own proofs). The bounded model does not exercise the oracle's overflow fail-closed arms, which only ever add violations, so soundness is unaffected.
- Pinned to engine SHA `71c9032`; the engine moves fast, so re-pin deliberately.

## Run it

```bash
cargo test                                               # full suite (oracles, driver, runner, JELLY, smoke, anti-vacuity, liquidation)
cargo test --test anti_vacuity                           # gates proving campaigns reach non-vacuous lien + cross-link state
cargo test --test liquidation                            # gates proving the liquidation campaigns book residual AND spend insurance (non-vacuity)
cargo test --test liquidation_conformance                # all three oracles over both liquidation campaigns (conformance under stress)
cargo test --test runner                                 # the 256-case adversarial property
cargo kani --tests --harness realizability_is_sound                  # O1 per-account soundness proof (needs `cargo install kani-verifier`)
cargo kani --tests --harness market_cross_link_is_sound              # v0.1 market-engine cross-link soundness proof
cargo kani --tests --harness liquidation_insurance_isolation_is_sound # v0.2 liquidation insurance-isolation soundness proof
cargo run --bin replay -- scenarios/jelly.json
```

## Roadmap

- **v0.1 (done)** — market-engine **cross-link** oracle (`source_claim_bound_num ≤ positive_claim_bound_num` + `market_id` binding), Kani-proven, wired into the property + JELLY; the engine held.
- **v0.2 (done)** — **liquidation insurance-isolation** oracle (cross-step delta over `insurance_domain_spent`: monotone + isolation + full accounting vs `insurance_used`), Kani-proven, run over both a residual and a funded (`insurance_used > 0`) liquidation campaign alongside O1 + the cross-link; the engine held all three.
- **Next** — the **ADL** attack class (`apply_quantity_adl_after_residual_for_account_not_atomic`, `src/v16.rs:9479`), and a deliberate engine re-pin as Percolator advances.

## License

Apache-2.0.
