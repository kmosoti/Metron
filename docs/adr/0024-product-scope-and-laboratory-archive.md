# 0024. Product scope and historical laboratory archive

- Status: proposed
- Date: 2026-11-12
- Deciders: Metron maintainers
- Supersedes: the executable Boolean-laboratory product/workspace scope in
  earlier decisions; historical scientific conclusions remain in force for
  their measured workloads

## Context

The current agent, harness, protocol, host, storage and inference crates form
the product graph. Source inspection at
[`898bae3d868fce6db3454a556a2afd91d246d7e1`](https://github.com/kmosoti/Metron/tree/898bae3d868fce6db3454a556a2afd91d246d7e1)
found no product dependencies on the separate six-crate Boolean laboratory.
Its reports and measurements remain useful research evidence, but its runner,
fixtures, manifests, figures and setup guidance do not define agent/harness
behavior. The `metron-testkit` crate remains development support and does not
make its sum fixture a general task architecture.

The trim changes product scope, not the validity of historical results. The
agent/harness runtime still has concrete compatibility work around the `sum-v1`
history and is not asserted to be wholly pure. The frozen evaluator and its
identities remain untouched.

## Decision

Remove the Boolean laboratory's executable crates, fixtures, manifests, result
skeleton and lab-specific required checks from the current product tree. Keep
its report outputs, referenced manifest evidence, useful figures and historical
documentation under `knowledge/archive/laboratory/`, pinned to the source
revision above. Do not copy its executable code into the product or create a
replacement calculator/workflow framework.

Keep product authority and compatibility contracts: typed proposals, host
grants, durable one-shot admission, receipt accounting, unknown-outcome holds
without redispatch, confined/CAS file mutation, scoped memory, independent
artifact verification and journal audit/replay. Keep `metron-testkit`
development-only. Keep the current authority/dependency diagrams free of the
archived laboratory. Alpha.1 remains incomplete until independently accepted
native coding and required resource evidence pass.

## Consequences

- Product workspace and CI describe the current agent/harness graph; removed
  laboratory checks are not replaced by a generic framework.
- Historical ADRs and observations retain their scientific claims and receive
  links to the archive where needed. They are not current product setup
  instructions.
- The archive points to the source revision rather than duplicating executable
  code or fixtures.
- Sum compatibility/isolation and full-context runtime fit remain distinct,
  unresolved product questions and require their own evidence.

## Revisit

Revisit only if a concrete product consumer for laboratory code is identified
or if a new reviewed product requirement needs a task architecture. Old tests,
ADRs or historical results alone do not establish such a consumer.
