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
