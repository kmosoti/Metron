# 0022 — Explicit coding contracts and scoped cross-task memory

Status: proposed (agent-authored; human ratification pending)

## Context

Alpha.0 implements one sum fixture, independently of its descriptive objective.
A calculator objective submitted to that family produced a valid sum artifact,
not a calculator. Native CUDA generation is established separately in O-0005;
its operational comparison failed entry and selected no production profile.

Alpha.1 must connect task purpose, effects, artifacts and independent acceptance.
Cross-task memory is included in this release's scope; the remaining autonomous
agent lifecycle stays alpha.2. This decision extends ADRs 0019–0021 without
altering their accepted laboratory or legacy replay contracts.

## Decision

New submissions select their task family explicitly. Missing or unsupported
families reject before run creation or model loading. Legacy fixture runs use
`--task-family sum-v1`; their verifier certifies only that fixture. Persisted
v1/v2 runs continue to resume/replay without changing serialized identities.

Coding uses a separate version-3 task envelope binding principal, project,
objective, tool catalog, model profile, memory collection and operator-owned
acceptance digest. Typed actions are proposals, never capabilities. A finish
proposal requests independent acceptance of immutable final source artifacts;
neither model assertions nor candidate-written tests can set the verdict.

The following diagram is the alpha.1 target. Components marked proposed are
not implemented by accepting this document or by compiling their DTOs.

```mermaid
flowchart TB
    Operator[Operator] --> Host[Composition host]
    Host --> Agent[Pure agent strategy - coding proposed]
    Host --> Caller[Standalone harness caller - coding proposed]
    Agent -->|proposals| Harness[Admission budgets and durable effects]
    Caller -->|proposals| Harness
    Harness --> Worker[Existing native inference worker]
    Worker -->|observations only| Host
    Harness --> Tools[Proposed sandboxed Rust tool adapters]
    Tools --> Workspace[Isolated workspace and SSD scratch]
    Harness --> Memory[Proposed principal/project/collection SQLite memory]
    Memory --> Context[Proposed bounded provenance-bearing context]
    Context --> Host
    Harness --> Verifier[Proposed independent coding verifier]
    Verifier -->|artifact-bound outcome| Harness
```

Use Fedora bubblewrap namespaces and cgroup-v2 process/resource controls for
real effects. Bind only required tools, an isolated workspace and private SSD
scratch; deny ambient credentials and network. Path validation supplements
directory-capability operations and OS confinement. No unsandboxed fallback.
Interrupted process trees must be terminated or explicitly remain uncertain.

Statically wire workspace, jj, ripgrep, Rust, Z3 and Lean adapters behind the
normal admission/receipt path. No alpha.3 registry is required. jj snapshots are
mutations, including automatic snapshots during ordinary inspection. Confine
runtime Git/jj metadata to independent task repositories. SMT/Lean results are
scoped to their statements and assumptions, not global software certificates.

Cross-task memory uses canonical SQLite records plus a rebuildable FTS5 index.
Host grants scope every query/mutation to principal, project and collection.
Store immutable provenance, distinguish unverified claims from evidence, retain
contradictions and identify stale sources. Model text cannot promote itself to
trusted evidence. Private evaluator material is excluded from memory/context.
Memory mutations use durable identities and receipts; retrieval results and
context manifests are recorded so replay does not rerun retrieval.

Static planner/coder/reviewer personas affect context and suggested behavior,
never principals or authority. No autonomous goals, wakeup scheduler, delegation,
MCP, browser or desktop adapter is added under this release.

## Consequences and release gates

The independent agent and harness libraries remain separated by protocol ports;
all I/O and process/model dependencies stay at effect/composition boundaries.
Keep Rust 1.100's dated toolchain pin and existing laboratory golden journals.

LFM3-1.7B Q4_K_M stays the local iteration backend. Reuse the verified CUDA build
and full artifact identities; do not silently substitute a hosted service.
Version changed prompts/constraints/corpora without rewriting failed evidence.
Qualify batching, hybrid cache state and resource envelopes under R-0008/R-0009.
Sampled telemetry is not a hard allocator cap. Compilation and inference have
separate resource budgets; build scratch/targets stay on SSD.

Release requires controlled harness proofs and actual LFM3 CUDA coding through
Metron: an empty-workspace checked-i64 calculator and a frozen Rust repair task,
with independent acceptance. A coordinating agent must not supply the calculator
implementation. All mandatory confinement, recovery, memory, inference and
efficiency gates must have passing evidence. Failures keep issue #7 open.

See D-0005 for the implementation/evidence matrix and R-0011 for tool sources.
