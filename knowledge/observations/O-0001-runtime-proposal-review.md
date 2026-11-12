---
id: O-0001
type: observation
status: reviewed-with-open-findings
updated: 2026-11-10
added_by: agent
human_ratified: false
---

# Runtime proposal review

## Scope and observed state

Review of [Metron PR #1](https://github.com/kmosoti/Metron/pull/1) at
`4c3781daa7211efd39bcdb95dcb15491be1abe39` against laboratory main
`3822fb1`. GitHub returned no separate issues on 2026-11-10. The PR adds ten
proposal documents. Product crates and product tests remain proposed.

## Findings

### High priority — budget accounting lacks an atomic reservation contract

`PolicyKernel::authorize` accepts immutable limits and returns a permission
reference. The composition loop authorizes and dispatches each committed intent
without a specified shared usage reservation. Two outstanding effects can each
fit the remaining budget while exceeding it together. Define reserve, settle and
release operations with task/principal scope, concurrent-worker semantics and
recovery of reservations associated with uncertain dispatches. Test competing
workers at the final available budget unit and crash recovery of a reservation.
This is a missing implementation contract. No executable overspend was observed.
[Source](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/07-rust-implementation-contract.md#L216-L284)

### Medium priority — effect lifecycle order disagrees across documents

The harness lifecycle puts authorization before persisted intent. The composition
algorithm commits the outbox before authorization. Specify one durable lifecycle,
including denied and suspended intents, approval expiry and revocation at dispatch.
Verify restart at each transition and denial after grants change while queued.
[Harness lifecycle](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/04-harness.md#L30-L52)
[Composition loop](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/07-rust-implementation-contract.md#L265-L284)

### Medium priority — authorization token has no specified usable boundary

`AuthorizedEffect` has private fields in `effect.rs`, while `PolicyKernel` in the
sibling `policy.rs` returns a string reference. External drivers receive the token
by value but have no scoped accessors for the approved operation. Rust module
privacy requires an explicit minting path. Define the trusted constructor,
validated operation accessors, binding to principal/task/action digest/grant
revision, and dispatch-time revocation check. A minimal compiled fixture should
prove an external driver can consume authorized work and cannot forge its token.
The current snippets explicitly disclaim compilation.
[Source](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/07-rust-implementation-contract.md#L188-L237)

## Interpretation and remaining uncertainty

Pure cognition, independent effect execution, durable receipts and staged release
contracts fit the stated product objective. P0 should resolve compatibility with
the laboratory before implementation. The findings above require specification
refinement before implementing effect dispatch and spending enforcement.

This review inspected repository contracts and proposal text. External landscape
claims and licensing assertions were not independently re-audited. Product Rust
sketches were not compiled. Proposal test names are planned acceptance checks.

## Follow-up research

The revised [harness contract](../research/R-0004-harness-contracts.md) specifies
persisted-intent-first ordering, atomic budget reservations and dispatch-time
admission. The [Rust contract](../research/R-0006-rust-implementation-contracts.md)
places token construction in its policy module and exposes read-only driver
access. [O-0003](O-0003-runtime-contract-probes.md) records limited feasibility
probes. Product tests for these findings remain outstanding in the
[implementation sequence](../decisions/D-0003-runtime-implementation-sequence.md).

## Relations

- contextualizes [workflow adoption](../decisions/D-0001-development-workflow.md)
- informed-by [historical laboratory invariants](../archive/laboratory/docs/invariants.md)

## Revisit when

The proposal changes revision, P0 is accepted, or compiled product contracts and
crash/concurrency evidence become available.
