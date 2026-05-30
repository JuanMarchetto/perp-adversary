# perp-adversary

An adversarial economic conformance harness for Solana perpetual-futures risk engines.

## What it does

`perp-adversary` drives a real perps risk engine through **multi-step adversarial campaigns** (matched trades, oracle price walks, margin withdrawal, liquidation across multiple accounts and slots) and checks an engine **economic invariant as a runtime oracle after every step**. When a step breaks the invariant, the failing campaign is reported with a one-command repro.

v0 targets [Percolator](https://github.com/aeyakovenko/percolator) at commit `71c9032`. It is an **independent harness that depends on Percolator (Apache-2.0); it does not fork or modify it.** It complements Percolator's existing per-instruction Kani proofs and single-path fuzzing by checking the invariant *across a whole campaign*, which bounded per-function model checking does not express.

## The O1 oracle (and why it is trustworthy)

v0 checks one invariant: **source-domain realizability** — the spec's core promise that *"the attacker cannot transform uncollectible A paper profit into global B purchasing power"* (spec.md v16.8.5, Requirement #2).

The oracle does not invent this invariant. It re-implements, field for field, Percolator's own per-account validator `SourceCreditLienAggregateProofV16::validate()` (`src/v16.rs:3060-3100`), re-running its six relationships (face decomposition, claim-bound, the `effective ≤ ceil(face/SCALE)` realizability cap, atom alignment, reservation exactness, impaired well-formedness) as an independent oracle after every observed step.

The oracle's arithmetic core is **proven sound with Kani** (`realizability_is_sound`): within the engine's documented operating range, if the oracle clears a domain then all six exact inequalities hold. So a reported finding can never be a harness arithmetic bug. The proof follows the engine's own bounded-proof discipline (small whole-atom magnitudes scaled by `BOUND_SCALE`, plus symbolic sub-scale remainders covering the ceiling and alignment boundaries).

## v0 result

Against the pinned engine (`71c9032`), the engine **held source-domain realizability across every campaign tested**:

- a proptest property over **64 randomized** deposit / matched-trade / oracle-price-walk campaigns (two accounts, multiple slots) — no violation;
- a hand-seeded **JELLY-archetype** campaign (open a position, walk the price up across slots, attempt an oversized extraction) — no violation at any step.

This is a **conformance signal**, not a proof of correctness. It means: across the adversarial campaigns this harness drove, the engine never left an accepted observable state that violated its own per-account source-credit lien-aggregate invariant.

### Scope and honest limits

- **One invariant, per-account scope.** O1 covers the per-account source-domain relationships. It does **not** check the outer link `source_claim_bound_num ≤ positive_claim_bound_num`, which compares against market-engine `SourceCreditStateV16` state the per-account observation does not carry (documented in `src/oracles.rs`). A market-engine oracle is the natural next step.
- **One attack class.** Realizability / oracle-walk only. Liquidation- and ADL-driven campaigns are modeled but not yet adversarially explored.
- **In-process, `_not_atomic` driving.** The harness calls the engine's building-block operations directly and observes after each; it does not yet exercise full atomic-instruction compositions or on-chain execution.
- **Bounded Kani proof**, matching the engine's own proof sizing — not an unbounded u128 proof (intractable for the model checker, as it is for the engine's own proofs).
- Pinned to engine SHA `71c9032`; the engine moves fast, so re-pin deliberately.

## Run it

```bash
cargo test                                   # 21 tests: oracle, driver, runner, JELLY, smoke
cargo test --test runner                     # the 64-case adversarial property
cargo kani --harness realizability_is_sound  # the oracle soundness proof (needs `cargo install kani-verifier`)
cargo run --bin replay -- scenarios/jelly.json
```

## Design & roadmap

- Design spec: [`docs/superpowers/specs/2026-05-30-perp-adversary-design.md`](docs/superpowers/specs/2026-05-30-perp-adversary-design.md)
- Implementation plan: [`docs/superpowers/plans/2026-05-30-perp-adversary-v0.md`](docs/superpowers/plans/2026-05-30-perp-adversary-v0.md)

## License

Apache-2.0.
