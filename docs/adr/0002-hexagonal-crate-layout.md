# ADR 0002: Hexagonal crate layout with a single composition root

Status: accepted. Date: 2026-10-02.

## Context

The system under study, the laboratory that evaluates it, and the effects
that connect them to the world must be separable, so that the laboratory
can be trusted and the system can be swapped.

## Decision

Six crates plus a test package:

- `metron-core`: pure domain and ports. Depends on `serde`, `serde_json`,
  `sha2`, `thiserror` only.
- `metron-app`: use cases (registry, schedules, episode runner). Depends on
  the core only.
- `metron-lab`: the immutable evaluator. Depends on the core only.
- `metron-operators`: reference operators. Depends on the core only.
- `metron-adapters`: persistence, clock, later models. Depends on the core
  only.
- `metron-cli`: the composition root; the only crate that depends on all of
  the above.
- `tests/architecture`: executable invariants over the workspace.

Dependencies point inward to the core. No inner crate depends on another
inner crate. The crates are prefixed `metron-` because a crate named `core`
would shadow the standard library's.

## Consequences

- The laboratory cannot be reached from operators or the app even by
  accident, because they cannot name its types.
- Adding a world other than the laboratory (a different oracle) means
  implementing the core's `Oracle` port and wiring it in the CLI.
- `tests/architecture/tests/invariant_1_core_is_pure.rs` enforces the
  layout.
