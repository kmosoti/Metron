# Runtime research and implementation sequence

This collection replaces the working proposal in Metron PR #1 with source-audited
research, explicit proposed contracts and an ordered implementation backlog.
The original proposal revision is
[`4c3781d`](https://github.com/kmosoti/Metron/tree/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime).
The laboratory remains separate. Runtime alpha.0 is released (O-0004); native
inference feasibility and failed operational entry checks are recorded in O-0005.
Alpha.1 coding, confinement and memory require the evidence in D-0005.

Implementation is tracked in [issue #7](https://github.com/kmosoti/Metron/issues/7)
with linked alpha.1 feature issues #21–28. Later releases are separate issues #29–32.

## Read and implement in order

| Order | Record | Role |
| --- | --- | --- |
| 1 | [R-0001 Source audit](R-0001-agent-harness-landscape.md) | Pinned primary sources, competing mechanisms, evidence limits and licensing caveats |
| 2 | [D-0002 Charter and boundary](../decisions/D-0002-runtime-charter.md) | Objective, lab compatibility, component diagram and P0 decision |
| 3 | [R-0002 Grammar](R-0002-runtime-grammar.md) | Shared vocabulary, invariants and architecture alternatives |
| 4 | [R-0003 Agent](R-0003-agent-contracts.md) | Pure cognition, identity, roles, persona, scoped memory and later delegation |
| 5 | [R-0004 Harness](R-0004-harness-contracts.md) | Durable lifecycle, scoped effects, atomic accounting and revocation ordering |
| 6 | [R-0005 Storage and extensions](R-0005-storage-plugins-protocols.md) | SQLite, outbox, projections, plugins, MCP/ACP and licenses |
| 7 | [R-0006 Rust contracts](R-0006-rust-implementation-contracts.md) | Workspace layout, usable token boundary, state/outbox ports and compilation targets |
| 8 | [R-0007 Evaluation](R-0007-runtime-evaluation.md) | Falsifiable hypotheses, matched baselines, holdouts and adaptation experiment |
| 9 | [O-0003 Contract probes](../observations/O-0003-runtime-contract-probes.md) | Executed SQL reservation and Rust privacy probes with reproducible source and limits |
| 10 | [R-0008 Local inference](R-0008-local-inference-resource-profile.md) | Pinned LFM3 default, audited historical size comparison and memory policy |
| 11 | [R-0009 mistral.rs efficiency](R-0009-mistralrs-batching-and-cache-contracts.md) | Runtime pin, shared LFM3 roles, adaptive batching and bounded hybrid caching |
| 12 | [R-0010 Native LFM3 inference](R-0010-lfm3-native-inference.md) | Local GPU feasibility and bounded operational comparison |
| 13 | [R-0011 Shared toolkit](R-0011-shared-reasoning-toolkit.md) | Tool identities, primary sources and qualification obligations |
| 14 | [D-0005 Alpha.1 delivery](../decisions/D-0005-alpha1-delivery.md) | Current ordered features, expanded memory scope and release proof |
| 15 | [D-0003 Implementation sequence](../decisions/D-0003-runtime-implementation-sequence.md) | Ordered PR slices, entry conditions and alpha.0–alpha.5 exit contracts |

Start implementation on `feat/metron-runtime` from the reviewed `develop`
revision containing this collection. P0 establishes a valid isolated Cargo
workspace and compatibility ADR. The first runnable milestone is alpha.0's
scripted, durable, separately usable agent and harness. Later alphas require the
preceding stage's evidence and retain explicit rejection/defer conditions.

## Original proposal coverage

| Original document | Current location |
| --- | --- |
| `README.md` | This index and coverage map |
| `00-charter.md` | [D-0002](../decisions/D-0002-runtime-charter.md) |
| `01-landscape.md` | [R-0001](R-0001-agent-harness-landscape.md), re-audited and pinned |
| `02-grammar-and-selection.md` | [R-0002](R-0002-runtime-grammar.md) |
| `03-agent.md` | [R-0003](R-0003-agent-contracts.md) |
| `04-harness.md` | [R-0004](R-0004-harness-contracts.md), corrected lifecycle/accounting |
| `05-plugins-memory-protocols.md` | [R-0005](R-0005-storage-plugins-protocols.md), checked storage/protocol sources |
| `06-alpha-release-contracts.md` | [D-0003](../decisions/D-0003-runtime-implementation-sequence.md), ordered implementation slices |
| `07-rust-implementation-contract.md` | [R-0006](R-0006-rust-implementation-contracts.md), corrected token ownership/API |
| `08-hypotheses-evaluation.md` | [R-0007](R-0007-runtime-evaluation.md) |

Original wording and sketches were retained where applicable and corrected where
source evidence or [the prior review](../observations/O-0001-runtime-proposal-review.md)
exposed a gap. The archived proposal remains available at its immutable revision.
Working decisions and research are agent-authored with `human_ratified: false`.
A merged knowledge PR does not ratify its hypotheses or implement its APIs.

## Evidence status

Source inspection establishes documented mechanisms. Contract probes establish
limited feasibility. O-0004 records alpha.0 compilation and crash/recovery evidence;
O-0005 records native inference feasibility and unsuccessful entry checks. Sandbox
security, coding quality, cross-task memory and comparative task performance remain
alpha.1 obligations. Do not infer them from the earlier results. Laboratory results apply
to their recorded workloads in [O-0002](../observations/O-0002-laboratory-results-snapshot.md).
