---
id: D-0004
type: decision
status: accepted
updated: 2026-11-10
added_by: agent
human_ratified: false
---

# Runtime compatibility boundary

[ADR 0019](../../docs/adr/0019-runtime-compatibility.md) establishes the
isolated runtime workspace, dated beta compiler pin, and alpha.0 synchronous
ports and accounting refinements. It preserves the laboratory's dependencies
and evaluation boundary. The workspace checks provide executable evidence for
membership and declared dependencies; source scans are guardrails, not proof of
native containment.

**Relations:** implements [D-0003](D-0003-runtime-implementation-sequence.md)
P0; refines proposed sketches in [R-0006](../research/R-0006-rust-implementation-contracts.md).
Revisit when stable 1.100 is qualified or compatibility evidence changes.
