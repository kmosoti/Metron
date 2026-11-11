---
id: D-0003
type: decision
status: proposed
updated: 2026-11-10
added_by: agent
human_ratified: false
source_proposal: 4c3781daa7211efd39bcdb95dcb15491be1abe39
---

# 06 — Product alpha release vector and success contracts

These are **proposed product releases**, not the existing laboratory's M0–M6. Nothing below is currently claimed as implemented. The labels are semantic slices, not deadlines. A milestone can be closed as negative and that is a useful result.

Implementation is tracked in [issue #7](https://github.com/kmosoti/Metron/issues/7)
on the new `feat/metron-runtime` branch.

## Ordered implementation slices

The implementation issue tracks P0 and alpha.0 as its first runnable milestone,
then progresses through the later release contracts below. Use independently
reviewable feature PRs from `develop`, following the repository's stack workflow.
The implementation branch is `feat/metron-runtime`.

```mermaid
flowchart LR
    Research[Source audit and corrected contracts] --> P0[P0 boundary ADR and Cargo checks]
    P0 --> Protocol[Versioned protocol and scripted fixtures]
    Protocol --> Store[Journal outbox budget reservations]
    Store --> Dispatch[Authorization tokens leases and dispatch]
    Dispatch --> Loop[Pure cognition standalone harness CLI replay]
    Loop --> A0[alpha.0 evidence gate]
    A0 --> A1[alpha.1 bounded coding]
    A1 --> A2[alpha.2 persistent memory]
    A2 --> A3[alpha.3 static plugins and MCP]
    A3 --> A4[alpha.4 browser computer use]
    A4 --> A5[alpha.5 controlled adaptation]
```

| Slice | Prerequisite | Deliverable | Required evidence |
| --- | --- | --- | --- |
| P0 | This source audit and existing laboratory contracts | New compatibility ADR, minimal buildable nested workspace, pinned compiler/lockfile, threat boundary and protocol outline | Root workspace member/dependency comparison, nested `cargo metadata`, root suite, executable product dependency and fixture-access checks |
| 1 | P0 merged through normal review | Typed task/identity/event/intent/receipt/outcome IDs and schemas, explicit deterministic inputs, scripted fixtures | Schema round trips, invalid-version rejection, deterministic bytes, no I/O in protocol/cognition |
| 2 | Slice 1 | SQLite event/state/outbox transaction, expected revisions, unique intent IDs, task/principal resource reservations | Conflict/retry recovery, duplicate admission and settlement, two workers racing for final budget unit, integer overflow, provider overrun receipts and breached-account admission denial, backup and configured durability checks |
| 3 | Slice 2 | Host-only token minting, durable denial/approval, lease epochs and atomic dispatch admission | External driver can read approved request, external token construction fails, grant/approval mismatch denies, stale worker is fenced, uncertain effect retains reservation and prevents blind redispatch |
| 4 | Slice 3 | One pure cognitive strategy, direct standalone harness entry point, synthetic host CLI and independent verifier | Same event/config produces same replay hash, task ends with verifiable artifact, agent/harness compile independently, restart and real process-kill fixtures at durable boundaries |
| 5 | Slices 1–4 | alpha.0 evidence report and supported CLI workflow | Exact source/fixture/config hashes, complete check commands/results, accounting reconciliation, scoped negative controls and explicit limits |

Atomic budget reservations, persisted-intent-first ordering and a usable opaque
authorization token are part of the first dispatch implementation. The
[original review findings](../observations/O-0001-runtime-proposal-review.md)
are addressed in the revised contracts. Their product behavior remains unverified
until the acceptance tests above execute against the implementation.

A real filesystem driver begins with alpha.1's tested containment boundary.
Alpha.0's bounded read operation uses fixture data through a scripted driver,
so its authority and replay tests can run without claiming OS containment.

## Release framework

- **Entry gate**: dependencies and evidence required to begin.
- **Deliverable**: smallest externally visible capability.
- **Success contract**: precise automated tests, reproduced artifacts and safety conditions.
- **Rejection gate**: conditions that close or defer the release rather than accumulating complexity.
- **Evidence file**: a committed test/run report with source revision, toolchain, fixtures and checksums.
- **Backward compatibility**: events, schema and plugin ABI versions require explicit migration or rejection.

### Siloed workstreams

| Silo | Responsibilities | Must not own |
|---|---|---|
| S0 Protocol | IDs, events, intents, task/receipt/outcome schemas, pure transition types | files, Tokio, net, model clients |
| S1 Agent | identity, goals, role, persona, strategies, beliefs, learning proposals | external effect execution or grants |
| S2 Harness | model/tool drivers, ACI, sandboxes, budgets, approval and receipts | task purpose or belief authority |
| S3 Persistence | SQLite journal, outbox, memory queries, projections | cognitive decisions or permissions |
| S4 Extensibility | static traits, registry and manifests, later MCP/Wasm/process | permission kernel replacement |
| S5 Evaluation | deterministic tests, independent graders, attack cases, matched experiments | mutable candidate self-grading |
| S6 Interfaces | CLI, later ACP, browser UI/desktop adapters | source of authority |

**Dependency invariant:** S1 knows S0 and injected ports. S2 knows S0 and host policy. S3 implements S0 ports. S5 observes outcomes via stable interfaces, never shares hidden fixtures with S1/S2. S6 is a composition root. No S1 ↔ S2 direct library coupling.

## Gate P0 — Establish an adoptable product boundary (spec phase)

**Entry:** current `AGENTS.md`, root workspace crate graph, ADRs 0007/0008/0009/0012/0014 and architecture tests reviewed.

**Deliverable:** accepted *new* ADR proposing `products/runtime/` as a separate Cargo workspace (or a reasoned alternative), with no root-lab behavior change; stable glossary, threat model, versioned protocol outline, license policy and synthetic task fixture.

**Success:**
- a tree/dependency graph shows lab crates unchanged;
- source and priors ledger records key assumptions and their tests;
- no direct model API client added to lab contrary to ADR 0009;
- the independent reviewer records acceptance or rejection of the new compatibility ADR through the repository workflow, with human promotion to `main` using its existing environment gate.

**Stop:** if the current lab's executable architecture cannot coexist with a new workspace, split product into a separate repository and link research artifacts. Do not relax the lab invariant solely for convenience.

## v0.1.0-alpha.0 — Closed-loop protocol slice

**Silos:** S0 + minimal S1/S2/S3/S5/S6. **Entry:** P0 accepted.

**Deliverable:**
- independent `metron-agent` library (one identity, one static role/persona, one pure deterministic strategy);
- independent `metron-harness` library (scripted model/tool drivers, one bounded fixture-read operation through a scripted workspace interface);
- a composition CLI that runs a synthetic task end-to-end;
- SQLite event/outbox journal with transactional resource reservations, receipts and deterministic replay projection;
- one independent verifier and `Uncertain` effect state.

**Success:**
1. A host-supplied task is accepted, emitted as an intent, permitted, journaled, executed and verified, with task and receipt IDs linked.
2. Two runs from identical event/seed/config sequences yield the same replay hash excluding volatile fields.
3. Invalid capability and unauthorized write are rejected before effect dispatch.
   Concurrent workers cannot reserve the same remaining budget unit. Duplicate
   admission and settlement preserve accounting, and overflow is rejected.
4. Agent core can be compiled/tested with only a scripted harness port; harness can be invoked directly with a task and no agent.
5. Killing between outbox commit and dispatch allows safe resume; killing after dispatch admission leaves an explicit uncertain result and preserves its reservation until reconciliation.
6. All core/adapter tests, fmt and clippy pass under pinned stable toolchain.

**Explicitly excluded:** live model HTTP, real shell mutation, GraphLite, hot plug-in reload, multiple agents, OS desktop control, automatic prompt rewriting.

**Stop:** if the Sans-IO state machine cannot remain pure without reintroducing hidden effects, refactor the contracts before expanding tools.

## v0.1.0-alpha.1 — Real bounded coding harness

**Silos:** S2/S5/S6. **Entry:** alpha.0 replay/authority tests.

**Deliverable:** workspace-scoped file read, structured edit/diff and test/command execution in a verified OS sandbox; a local inference plugin behind the model adapter (pinned LFM3-1.7B Q4_K_M through mistral.rs; one shared weight set for multiple roles); CLI with bounded event streaming. Tool output includes truncation and evidence refs.

**Success:**
1. Complete a known synthetic Rust coding task: read fixture, apply edit, run checks, inspect diff, produce artifact, independently verify.
2. Deny out-of-root read/write, symlink race attempts, child process escape, alternate shell/network route and approval spoofing.
3. On interrupt, terminate or explicitly flag lingering process tree; never report successful cleanup without evidence.
4. Run both scripted and real-model adapters against the same tool/receipt contracts.
5. Compare against a minimal static baseline, measuring solved cases, cost, latency, intervention and blocked unauthorized effects.
6. Meet [R-0008 local inference acceptance](../research/R-0008-local-inference-resource-profile.md#proposed-plugin-boundary): pinned offline artifacts, bounded workers/context/caches, actual peak-memory measurements, typed resource denial and replay from recorded responses. Meet [R-0009 batching and cache gates](../research/R-0009-mistralrs-batching-and-cache-contracts.md#acceptance-and-decision-criteria): adaptive continuous batching, chunked prefill, bounded task-scoped hybrid prefix reuse and per-request accounting. The 2 GiB VRAM target requires measurement and is not an assumed hard allocation cap.

**Stop:** if sandbox guarantees fail under adversarial fixtures, do not expose terminal or file mutation in default profile. If the selected model fails coding acceptance or its resource profile does not fit, record that outcome and defer the affected live-model capability. Do not substitute a hosted backend silently.

## v0.1.0-alpha.2 — Persistent autonomous agent

**Silos:** S1/S3/S5. **Entry:** alpha.1 bounded effects and stable schema.

**Deliverable:** durable goals, versioned identity/role/persona/skill references, source-tagged observations/claims/beliefs, wakeup scheduling, memory query interface and restart recovery. SQLite remains canonical.

**Success:**
1. Goal resumes correctly after process restart without hallucinated completion.
2. Contradictory source claims remain distinguishable and a prior belief is explicitly revised.
3. Persona/role changes leave authorization decisions bit-identical.
4. Scoped task memory cannot leak private cross-tenant evidence.
5. Query freshness and truncation are explicit, and a simple SQLite node/edge view supports a bounded relationship traversal.

**Stop:** no GraphLite promotion unless a measured graph-heavy workload dominates SQLite in required correctness/latency/storage dimensions.

## v0.1.0-alpha.3 — Composable trusted plugins and external standards

**Silos:** S4/S2/S6. **Entry:** stable alpha.2 interfaces.

**Deliverable:** statically linked Rust extension registry, dependency resolution, scoped lifecycle and one MCP adapter. A typed declarative preset composes role/persona/model/tool configurations while host policy grants authority separately.

**Success:**
1. Dependency cycles, duplicate services, version incompatibility and illegal requested grants fail closed.
2. Failed plugin startup rolls back registrations, tools and resource handles.
3. An MCP tool call traverses the same authorization and receipt path as a native tool.
4. A paused task retains its previous implementation/version or enters an explicit migration state.
5. Reference plugin source and license inventory are reproducible.

**Stop:** no dynamic Wasm ABI if an additional trust/sandbox layer cannot pass confinement tests; native static plugin API remains valid.

## v0.1.0-alpha.4 — Computer-use adapters (experimental)

**Silos:** S2/S5/S6. **Entry:** documented threat model and alpha.3 host policy.

**Deliverable:** isolated browser DOM adapter, optional screenshot/click driver behind explicit capability and observed pre/post conditions. Desktop-wide OS control remains outside the default install.

**Success:**
1. Navigation/click observations carry source, scope, URL/window identity, screenshots when needed and result uncertainty.
2. Test prompt injection, hidden overlays, stale UI coordinates, changed DOM and cross-origin data disclosure.
3. Human emergency stop works during a long-running interaction.
4. Never treat a click, a screenshot match, or a model summary as a completed external transaction.
5. No outbound action beyond a permitted test domain in sandbox mode.

**Stop:** if UI observability is too weak for safe verification, retain browser read-only inspection and defer mutation. Full desktop driver requires its own ADR and threat test.

## v0.1.0-alpha.5 — Controlled adaptation / Harness² hypothesis

**Silos:** S1/S5 with versioned S2 configuration. **Entry:** stable reproducible baseline, sealed evaluation split, adapter budgets.

**Deliverable:** candidate configuration snapshots, Propose–Probe–Compare experiment, task-local improvement and a *manual* promote/rollback action. Cross-task improvement library is experimental only.

**Success:**
1. A static baseline A, task-local adaptive B and persistent-library C run on matched tasks/models/budgets.
2. Test split/ground truth/promotion criteria remain inaccessible to candidate execution.
3. Every candidate has parent hash, changed surface, hypothesis, probe results, cost and negative controls.
4. Candidate that increases unauthorized behavior or regression fails promotion, even if headline task accuracy rises.
5. Promotion and rollback change an authorized version pointer, not hidden mutable code.
6. No statistically credible improvement means the adaptive module remains experimental and is not default-enabled.

**Stop:** if adaptation costs more than it saves or fails transfer tests, preserve the static harness and reject recursive edits for this domain.

## After alpha.5 — Conditional, not promised

- **GraphLite projection** (only if canonical SQLite graph is measurably inferior at representative scale).
- **Wasm Component plugins** (only if ABI stability and host-call scope are verified).
- **Multiple delegated agents** (only if constrained delegation outperforms a single-agent baseline without unauthorized escalation).
- **Intelligent provider routing** (only if it improves task-verified goodput/cost at matched quality).
- **Weight-space and adaptive-symbolic cognition** (separate sealed experiments; no production dependency).
- **Remote/distributed harness workers**, desktop-wide control, mobile UI and multi-tenant hosting (separate risk reviews and release contracts).

## Cross-release quality gates

Every release candidate publishes:
- scope and architecture dependency diff;
- pinned toolchain/lockfile + Apache-2.0 notice/dependency audit;
- threat model and adversarial fixtures;
- deterministic tests and reproducible receipts;
- benchmark tasks, held-out rules, compute budgets, seeds and confidence intervals where relevant;
- known deviations, explicitly unsupported platforms and rollback path;
- decision: `REACHED` / `CLOSED_NEGATIVE` / `DEFERRED` with source-linked rationale.

No milestone is “done” because documentation exists or a model said tests would pass. The released binary and executed evidence determine status.

## Provenance and relations

- derived-from [original 06-alpha-release-contracts.md](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/06-alpha-release-contracts.md)
- indexed-by [runtime research and sequence](../research/README.md)
- reviewed-against [original proposal findings](../observations/O-0001-runtime-proposal-review.md)

## Revisit when

Implementation evidence, a changed workload, a source revision or a failed acceptance
criterion contradicts the proposed contract. Record the new evidence and update
linked decisions before expanding scope.
