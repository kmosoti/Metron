# Working in Metron

This file is for coding agents and people alike. Read it before changing
the shape of anything.

## What this repository is

Metron is a laboratory for a resource-bounded system that learns hidden
structure through multiple representations. The repository encodes the
experimental rules in its architecture. The current slice is deliberately
minimal: the research objects, the four invariants, one task family, one
fixed pipeline. See `docs/architecture/overview.md`.

## The four invariants (do not break; extend the tests when you extend the code)

1. `metron-core` is pure: `serde`, `serde_json`, `sha2`, `thiserror` only;
   no I/O, no models, no experiment internals. Inner crates depend on the
   core alone; only `metron-cli` composes them.
2. The laboratory owns ground truth. Hidden targets, the judge and the
   promotion criteria live in `metron-lab`, sealed. The system reaches the
   target only through the `Oracle` port.
3. Every external operation produces a `ResourceReceipt`, issued by the
   port and journaled by the runner. Cost is charged from receipts. Journals
   replay to the same head hash for the same manifest and seed.
4. Representations connect only through validated `TransformContract`s.
   Observe writes `observations`; transform writes its contract's target
   frame; commit writes nothing; only observe obtains observations.

`cargo test --workspace` runs `tests/architecture`, which fails if any of
these is violated. Details and the mechanism for each are in
`docs/architecture/invariants.md`.

## Layout

```
crates/metron-core        pure domain + ports
crates/metron-app         use cases: registry, schedules, episode runner
crates/metron-lab         immutable evaluator: hidden targets, oracle, judge, promotion gate, manifests
crates/metron-operators   reference operators (one complete pipeline)
crates/metron-adapters    effects: clock, files, results
crates/metron-cli         composition root: metron run | verify | explain
experiments/manifests     experiment definitions (lab property)
experiments/fixtures      hidden targets as data (lab property)
experiments/results       generated run directories (not committed)
docs/architecture         overview and invariants
docs/research             archived inputs (proposal, prior-art review)
docs/adr                  decisions
tests/architecture        executable invariants
```

## Rules of the road

- Run before you push: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`. CI runs the same.
- Adding an operator: implement `metron_core::operator::Operator<W>` with
  the narrowest `W` bound you need (`W: Oracle` only if you probe). Pick
  exactly one kind. A transform must carry a validated contract that says
  what it preserves, loses and assumes, and whether it is exact. Declare
  `reads` and `writes`. Add it to `reference_operators` only if it belongs
  to the reference pipeline.
- Adding a task family: it goes in `metron-lab`, sealed like
  `HiddenFunction`, exposing only a `Question` and the `Oracle` port.
  Fixtures are data under `experiments/fixtures/`.
- Changing the cost model (`metron_core::cost::Cost`) or the journal
  format is an ADR-level change.
- Do not add, before their gates in ADR 0006 are met: hyperdimensional
  memory, learned routing, model clients, or SBS/VBS harnesses. The order
  is: mixed Boolean lab → headroom measurement → routing → promotion → HDC
  → models. The prior-art review explains why (headroom first).
- Do not copy the deep-research proposal's architecture. It is an archived
  input, not the design (`docs/research/README.md`).
- Record decisions as ADRs. Do not edit accepted ADRs; supersede them.
- Keep wall-clock time out of decisions and out of hashes.
- Never give the system a way to read `experiments/fixtures/`, a `Verdict`
  during an episode, or `PromotionCriteria` values.

## Vocabulary

Inquiry, Question, View, Frame, TransformContract, Observation, Evidence,
Operator (observe | transform | commit), ResourceReceipt, Episode, Journal,
Verdict, CapabilityCandidate, PromotionGate. Not "capsule", not "ledger",
not "kernel loop". See ADR 0007.
