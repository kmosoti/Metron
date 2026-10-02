# ADR 0005: Representations connect only through transform contracts

Status: accepted. Date: 2026-10-02.

## Context

The prior-art review identifies explicit transform contracts (what is
lost, what effect class) and the provenance rule (redescriptions of one
observation are not independent evidence) as the design's plausibly novel
methodological contributions. They only matter if the kernel enforces
them.

## Decision

- A `Frame` is a representation system. A `View` is the inquiry's state in
  one frame, with a `Derivation` recording the writing operator, input
  views, directly incorporated observations and the contract.
- A `TransformContract` names `from` frames, a `to` frame, `preserves`,
  `loses`, `assumes` and a `Fidelity` (exact, or approximate with a
  description). It is validated at registration.
- Operators have exactly one kind: observe (writes only the `observations`
  frame), transform (writes only its contract's target frame, from views in
  its source frames), or commit (writes no views). Only observe operators
  may obtain observations.
- The runner enforces the rules on every application, stamps the contract
  onto written views, and journals each write with the view's transitive
  observation set. An answer's evidence lists distinct observations with
  the operator path between them.

## Consequences

- Inductive bias is explicit: the reference `complete-table-by-default`
  operator is approximate and declares "unobserved rows evaluate to false".
- Independence of evidence is computed from observation identities, never
  from the number of evidence records.
- `tests/architecture/tests/invariant_4_transform_contracts.rs` enforces
  this.
