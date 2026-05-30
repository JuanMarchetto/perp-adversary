# perp-adversary

An adversarial economic conformance harness for Solana perpetual-futures risk engines.

**Status: design phase. No implementation code yet.** This repository currently holds the design spec; the build follows it under strict test-first discipline.

## What it does

`perp-adversary` drives a real perps risk engine through **multi-step adversarial sequences** (oracle manipulation, margin withdrawal, liquidation, ADL across multiple accounts and slots) and checks the engine's **economic invariants as runtime oracles** after every step. When a sequence breaks an invariant, the failing campaign is shrunk to a **minimal, reproducible counterexample**.

v0 targets [Percolator](https://github.com/aeyakovenko/percolator). It is an **independent harness that depends on Percolator (Apache-2.0); it does not fork or modify it.** It complements Percolator's existing per-instruction Kani proofs and single-path fuzzing by checking the economic invariant *across a whole campaign*, which bounded per-function model checking does not express.

The oracle itself is formally proven correct with Kani, so a reported finding cannot be a harness bug.

## Design

See [`docs/superpowers/specs/2026-05-30-perp-adversary-design.md`](docs/superpowers/specs/2026-05-30-perp-adversary-design.md) for the full design and roadmap.

## License

Apache-2.0.
