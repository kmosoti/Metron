---
id: D-0005
type: decision
status: proposed
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Alpha.1 delivery and evidence matrix

## Objective

Close [#7](https://github.com/kmosoti/Metron/issues/7) only after bounded local
coding, the shared toolkit and scoped cross-task memory reach alpha.1 on main.
Require deterministic harness proofs and independently accepted coding by the
actual LFM3 Q4 CUDA backend. Human release approval remains mandatory.

Cross-task memory is pulled forward from alpha.2. Persistent autonomous goals,
identity lifecycle and wakeups remain [#29](https://github.com/kmosoti/Metron/issues/29).
Plugins/MCP, computer use and adaptation follow separately in #30, #31 and #32.

## Baseline and limits

- Observed: alpha.0 is released on main; O-0004 scopes its evidence to sum-v1.
- Observed: O-0005 establishes one Q4 CUDA generation; all twelve operational
  profiles failed entry and no production profile was selected.
- Observed: O-0006 records the calculator request mismatch.
- Unknown: current queued GitHub Actions runs expose no job or queue cause.
- Hypothesis: the bounded toolkit and model loop can complete coding tasks.
  Source support and DTO compilation cannot establish this behavior.

## Ordered feature obligations

| Issue | Feature | Required evidence |
| --- | --- | --- |
| [#21](https://github.com/kmosoti/Metron/issues/21) | Baseline/toolkit | Inventory and pinned identities; positive/negative tool probes; R-0011 |
| [#22](https://github.com/kmosoti/Metron/issues/22) | Integrate PR #20 | Required exact-HEAD CI, resolved review, independent rebase merge |
| [#23](https://github.com/kmosoti/Metron/issues/23) | Explicit task contracts | Unsupported tasks reject before effects; acceptance binding; legacy replay |
| [#24](https://github.com/kmosoti/Metron/issues/24) | Sandboxed tools | Controlled tool runs; path/race/process/network/approval negative controls |
| [#25](https://github.com/kmosoti/Metron/issues/25) | Memory/context/personas | Cross-task restart/retrieval; scope/revocation/injection; rebuildable index |
| [#26](https://github.com/kmosoti/Metron/issues/26) | Live coding loop | Agent and standalone entry points, shared admitted effects, actual receipts |
| [#27](https://github.com/kmosoti/Metron/issues/27) | Efficiency qualification | Frozen matched 1/2/4 workloads, hybrid cache correctness, bounded resources |
| [#28](https://github.com/kmosoti/Metron/issues/28) | Acceptance/release | Actual model-written calculator and repair; independent grader; all gates |

Each issue records governing sources, hypothesis, smallest discriminating probe,
decision, implementation, exact revision/build inputs and independent checks.
Record failures and unknowns, not only passing commands. Update linked research,
observations and priors when measurements contradict assumptions. Never revise
a frozen evaluator after seeing holdout results without a new evaluation identity.

## Defaults and implementation constraints

- First-party product code remains Rust 1.100; Z3/Lean are external toolchains.
- LFM3 Q4_K_M is fixed for local iteration. No paid inference fallback or model
  search is required for this program. Missing coding evidence blocks release.
- Start at 2,048 total context/512 output tokens; qualify 4K then 8K only when
  smaller profiles fail the frozen workload. Keep per-request grants/receipts.
- Treat 2 GiB device memory as a measured target, distinct from the 4 GiB
  operational envelope; neither sampled monitoring nor KV limits prove a cap.
- New coding DTOs do not add file/process/model dependencies to pure crates.
- Lexical paths do not establish containment. Expose mutation only after the
  selected sandbox passes adversarial fixtures on its declared environment.
- Cross-task memory is principal/project/collection scoped. It cannot expose
  sealed evaluator data or turn model-authored claims into authority.
- Static personas do not implement alpha.2 autonomy or create extra agents.

## Completion

Calculator: dependency-free CLI `<lhs> <operator> <rhs>` using checked i64
arithmetic for +, -, *, /. Integer division truncates toward zero. Invalid
arguments, zero division and overflow fail without panic. Metron generates
source, tests and instructions; the independent evaluator checks immutable
artifacts and edge cases outside the candidate's workspace.

Also complete one separately frozen Rust repair task. Exercise agent and
standalone harness modes. Run all D-0003 alpha.1 requirements, both workspaces'
checks, unchanged laboratory smoke/journal verification and old runtime replay.
Full release is REACHED only when required gates pass; partial, negative or
deferred results never imply issue #7 is complete.

**Relations:** extends D-0003 alpha.1 scope; implements ADR 0022; informed by
R-0003–R-0011 and O-0004–O-0006. Revisit on failed coding, isolation, resource or
recovery acceptance. Later architectural changes require a new decision.
