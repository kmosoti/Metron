# Metron agent/harness product proposal

**Status: proposed, not an accepted architecture.** Reviewed November 10, 2026. This proposal does **not** change the current Hexagonal Cognitive Kernel laboratory or its accepted ADRs.

Start with [the charter](00-charter.md), then follow these independently reviewable documents:

| Document | Purpose |
|---|---|
| [01 — Research landscape](01-landscape.md) | Serious and experimental agents, harnesses, papers, first-party sources, caveats |
| [02 — Grammar and logic](02-grammar-and-selection.md) | Vocabulary, formal well-formedness laws, architecture reduction pipeline |
| [03 — Agent](03-agent.md) | Identity, role, persona, cognition, skills, beliefs, memory and delegation |
| [04 — Harness](04-harness.md) | Computer use, effects, sandboxing, authorization, receipts and verification |
| [05 — Plugins/memory/protocols](05-plugins-memory-protocols.md) | Composable services, SQLite/GraphLite, MCP/ACP/Wasm, FOSS boundaries |
| [06 — Alpha releases](06-alpha-release-contracts.md) | Outcome-gated alpha.0–alpha.5, success/failure contracts |
| [07 — Rust-shaped code](07-rust-implementation-contract.md) | Expected file paths, traits, source style, compile and adversarial test templates |
| [08 — Hypotheses and evals](08-hypotheses-evaluation.md) | Priors, negative controls, Harness² A/B/C experiments and red-team tests |

**The first implementation action is NOT a rewrite.** Propose a compatibility ADR for a separate Rust product workspace that preserves [the current invariants](../../architecture/invariants.md), [the laboratory milestone contract](../../architecture/milestones.md), and [ADR 0009](../../adr/0009-llm-inference-through-claude-code.md).

### One-line design law

`Agent chooses and maintains beliefs; harness executes authorized effects; independent evidence determines outcomes; learning proposes reversible changes.`

### Proposed first alpha

One scripted, replayable, independently verified task through a pure Rust Sans-IO cognitive transition and a capability-enforced harness. No graph engine, unsafe zero-copy buffer lifetime tricks, live desktop control, or unreviewed self-modification at alpha.0.

The first-party source target is Apache-2.0, with compatible FOSS dependencies and an open local reference configuration. External proprietary model providers, when optionally supported, must not be misrepresented as FOSS.

**Implementation status:** documentation only. These Rust code examples have **not** been compiled, and no product runtime tests have been run. Existing repository lab tests remain separate.
