---
id: R-0011
type: research
status: active
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Shared development and Metron reasoning toolkit

Use the same statically wired capabilities for development and model-proposed
actions, with runtime execution subject to host admission and receipts. Installed
tools and source-documented behavior are not qualified runtime adapters.

## Primary sources and local observations

| Tool | Source / observed baseline | Proposed use and qualification |
| --- | --- | --- |
| jj | [Working copy](https://github.com/jj-vcs/jj/blob/main/docs/working-copy.md); installed 0.46.0 | Stacks/snapshots/diffs. Ordinary commands may snapshot; treat this as mutation. Read-only inspection selects an explicit revision with `--ignore-working-copy`. Never expose shared host VCS metadata. |
| ripgrep | [Official source](https://github.com/BurntSushi/ripgrep); installed 15.2.0 e89fff89ac | Bounded workspace search with structured output, explicit truncation and provenance. Distinguish no matches from execution failure. |
| Z3 | [SMT-LIB2 interface](https://github.com/Z3Prover/z3#interfaces); installed 4.16.0 | SMT constraints/counterexamples. Separate sat/unsat/unknown/timeout; bind exact formulas and assumptions. A model-generated formula can encode the wrong claim. |
| Lean | [4.35.0 release](https://github.com/leanprover/lean4/releases/tag/v4.35.0), [proof validation](https://lean-lang.org/doc/reference/latest/ValidatingProofs/) | Installed 4.24.0 is inventory only; select the released 4.35.0 toolchain and pin its complete archive digest. Check explicit statements, permitted axioms and clean proofs. Do not accept candidate-supplied compiled caches as trusted. |
| SQLite/FTS5 | [FTS5 reference](https://www.sqlite.org/fts5.html); runtime already uses bundled rusqlite 0.37.0 | Canonical scoped memory plus a rebuildable lexical index. Verify the actual bundled library's FTS5 support and query isolation. |
| Context assembly | R-0003, R-0008/R-0009 | Bound source excerpts and complete rendered token counts; record source hashes, selection, provenance and truncation. Test stale sources and scope changes. |
| Rust tools | Existing beta-2026-11-07 pin; workspace minimum 1.100 | Offline coding checks in a qualified sandbox, using read-only toolchains and SSD scratch. |

The GitHub release API reports Lean 4.35.0 published 2026-10-12. Its x86-64
Linux archive is 594,481,375 bytes with SHA-256
`7289597a44543246c9227b8cae501767e095ce17f6d448b95fc4a8dc97268f6d`.
This identifies the proposed distribution, not an executed proof or installation.

The local host has bubblewrap 0.12.0, cgroup v2 and a user systemd manager. A
basic namespaced `/usr/bin/true` invocation returned zero. This does not establish
filesystem/network/process-tree confinement or resource enforcement.

## First discriminating checks

1. Record full tool identity/version; reject a missing or mismatched required tool.
   The inventory uses safe rustix 1.1.6 wrappers to open regular objects without
   FIFO waits, copy executable bytes into sealed Linux memfd images, hash those
   images and execute the same held-open objects. Source path replacement cannot
   substitute another executable. Interpreters/shared libraries are still trusted
   dependencies, so this is not full distribution or runtime qualification.
2. Check controlled reservation arithmetic and monotone grants with SMT/Lean;
   reject a deliberately weakened condition or false theorem. Cross-check Rust
   checked-integer boundaries and actual concurrent admissions separately.
3. Test search framing/no-matches/limits, explicit jj snapshot/diff behavior,
   sandbox timeout/tree cleanup, and FTS5 scope/restart/rebuild.
4. Compare costs only on identical accepted work. Record cold/warm startup,
   memory, output/context size and failures. Prefer on-demand processes and
   reuse pinned caches; no always-on tool server is required.

No claim of a globally fastest toolkit, verified whole harness, useful coding,
or safe memory isolation follows from this source audit. Formal checks cover
their models; executable adapters, containment and independent task acceptance
remain separate obligations.

**Relations:** informs ADR 0022 and D-0005; extends R-0003/R-0005/R-0009.
Revisit after tool/backend updates, failed source assumptions, or a matched
comparison showing a simpler mechanism meets the same accepted-work envelope.
