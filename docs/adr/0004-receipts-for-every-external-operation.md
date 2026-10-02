# ADR 0004: Every external operation produces a receipt

Status: accepted. Date: 2026-10-02.

## Context

Matched-budget comparisons and replay require that every resource spent
outside the system is accounted for, by something the system cannot
influence.

## Decision

- Ports that execute external operations issue a `ResourceReceipt` at the
  moment of execution: id, operation kind, operator, inquiry, request and
  response hashes, cost, observations produced, and an informational
  wall-clock reading that is excluded from the receipt's hash.
- The runner drains receipts after every application, before reading the
  operator's result, journals them, and charges their cost to the inquiry.
- The journal is a hash chain over a replay projection of each event
  (wall-clock readings zeroed). Two runs with the same manifest and seed
  produce the same head hash.
- The cost model is pre-registered in `metron_core::cost::Cost`: operator
  calls, work units, oracle probes, external calls. Wall-clock time is never
  a decision input.

## Consequences

- An operator cannot hide a probe: not by failing, not by discarding the
  observation, not by being a transform.
- `tests/architecture/tests/invariant_3_receipts.rs` and
  `replay_and_manifests.rs` enforce this.
- Model calls, when added (ADR 0006), will be `OperationKind::ExternalCall`
  receipts issued by the adapter, with token counts in `Cost`.
