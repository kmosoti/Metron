# Linked project knowledge

Dated records retain decisions, research and observations with source links and
explicit uncertainty. Read relevant records before repeating an investigation.

## Record protocol

Use `decisions/D-xxxx-*.md`, `research/R-xxxx-*.md` or
`observations/O-xxxx-*.md`, with stable IDs and YAML front matter containing
`id`, `type`, `status`, `updated`, `added_by` (`agent`, `human`, `mixed`) and
`human_ratified` (`true`, `false`). Ratified records also require `ratified_by`
and `ratified_on`. Use explicit human confirmation to set ratification.

Agents create and use unratified records immediately. Human ratification is
asynchronous. PR merge alone does not ratify a record. Obtain explicit human
approval before overturning a ratified decision. Record contrary evidence and
link superseding decisions so readers can follow the history.

Distinguish observed facts, deductions, hypotheses and unknowns when they affect
a decision. Include typed relations and a condition for revisiting the record.

Stable architectural contracts remain in [docs/](../docs/architecture/overview.md).
Architectural decisions continue in [ADRs](../docs/adr/README.md), scientific
assumptions in the [priors ledger](../docs/research/priors.md), and experimental
measurements in [reports](../experiments/reports/README.md). Link supporting
knowledge to those sources.

## Records

| Record | Added by | Human ratified |
| --- | --- | --- |
| [D-0001 Development workflow](decisions/D-0001-development-workflow.md) | agent | false |
| [O-0001 Runtime proposal review](observations/O-0001-runtime-proposal-review.md) | agent | false |
| [O-0002 Laboratory results and milestone snapshot](observations/O-0002-laboratory-results-snapshot.md) | agent | false |
| [D-0002 Runtime charter and boundary](decisions/D-0002-runtime-charter.md) | agent | false |
| [D-0003 Runtime implementation sequence](decisions/D-0003-runtime-implementation-sequence.md) | agent | false |
| [D-0004 Runtime compatibility](decisions/D-0004-runtime-compatibility.md) | agent | false |
| [D-0005 Alpha.1 delivery](decisions/D-0005-alpha1-delivery.md) | agent | false |
| [R-0001 Agent/harness source audit](research/R-0001-agent-harness-landscape.md) | agent | false |
| [R-0002 Runtime grammar](research/R-0002-runtime-grammar.md) | agent | false |
| [R-0003 Agent contracts](research/R-0003-agent-contracts.md) | agent | false |
| [R-0004 Harness contracts](research/R-0004-harness-contracts.md) | agent | false |
| [R-0005 Storage, plugins and protocols](research/R-0005-storage-plugins-protocols.md) | agent | false |
| [R-0006 Rust implementation contracts](research/R-0006-rust-implementation-contracts.md) | agent | false |
| [R-0007 Runtime evaluation](research/R-0007-runtime-evaluation.md) | agent | false |
| [R-0008 Local inference and resource profile](research/R-0008-local-inference-resource-profile.md) | agent | false |
| [R-0009 mistral.rs batching and cache contracts](research/R-0009-mistralrs-batching-and-cache-contracts.md) | agent | false |
| [R-0010 Native LFM3 inference](research/R-0010-lfm3-native-inference.md) | agent | false |
| [R-0011 Shared reasoning toolkit](research/R-0011-shared-reasoning-toolkit.md) | agent | false |
| [O-0003 Runtime contract probes](observations/O-0003-runtime-contract-probes.md) | agent | false |
| [O-0004 Scripted runtime alpha.0 evidence](observations/O-0004-runtime-alpha0-evidence.md) | agent | false |
| [O-0005 Native LFM3 CUDA execution](observations/O-0005-native-lfm3-cuda.md) | agent | false |
| [O-0006 Calculator contract mismatch](observations/O-0006-calculator-contract-mismatch.md) | agent | false |
| [O-0007 Toolkit and task foundation](observations/O-0007-toolkit-and-task-foundation.md) | agent | false |
| [O-0008 Coding loop and confinement](observations/O-0008-coding-loop-and-confinement.md) | agent | false |
| [O-0009 Known proposal feedback](observations/O-0009-known-proposal-feedback.md) | agent | false |

## Runtime implementation

Start with the [runtime research index](research/README.md), then follow the
[ordered implementation sequence](decisions/D-0003-runtime-implementation-sequence.md).
The index maps all ten original proposal documents to their working records.
