# 0023 — First-class agent and harness crates

Status: proposed (agent-authored; human ratification pending)

## Context

Metron's primary deliverables are `metron-agent` and `metron-harness`. Incubating
them in `products/runtime` with another workspace, lockfile and toolchain makes
the product look like a separate experiment and duplicates development commands.
The source boundaries now express the separation that the nested workspace was
intended to protect.

## Decision

Place the agent, harness and supporting crates directly under `crates/` in
the root workspace. Keep test support as a development dependency, and merge
the architecture checks into `tests/architecture`.
Use one lockfile, one dated Rust 1.100 toolchain and one full-workspace CI check.
Build the inference worker and CUDA feature explicitly when required.

This supersedes ADR 0019's nested-workspace, separate-lockfile and duplicate-CI
layout. It retains the pure cognition boundary, worker-only inference SDK,
admission and recovery semantics, sealed evaluators and laboratory contracts.
No application, model, task, receipt, journal or acceptance contract changes as
part of this relocation.

The [implemented crate and authority diagrams](../architecture/overview.md)
describe the single workspace. The laboratory has no normal dependency on the
agent/harness crates, and the agent/harness crates have none on the laboratory.
Fixtures stay in test support and measurements stay evidence; neither becomes
a production component.

## Compatibility and verification

Seed the merged lock from the pinned inference workspace, preserving its exact
third-party package versions and Git revisions. The laboratory shares those
resolved dependency versions after consolidation. Verify its unchanged smoke
journal heads rather than assuming a shared lock is behaviorally neutral.
Preserve existing per-crate lint policies during the move.

The shared graph enables insertion-ordered JSON maps in a transitive dependency.
Enforce the laboratory's existing canonical map-order contract explicitly while
retaining struct field order; this is a compatibility fix, not a new journal
format. Any changed legacy head rejects the migration.

Keep frozen evaluator bytes, prompt/recipe identities and archived journals
unchanged. Run workspace format, Clippy, tests, laboratory smokes and
read-only legacy/native replay checks. A passing relocation does not establish
model-driven coding acceptance or qualify an inference profile.

Revisit if shared dependency features alter canonical serialization or journal
behavior, or if a proposed dependency crosses the tested trust boundaries.
