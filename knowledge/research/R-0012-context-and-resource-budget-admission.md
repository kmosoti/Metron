---
id: R-0012
type: research
status: proposed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# Context quality and resident-worker resource admission

## Question and scope

What should Metron infer when planning larger context allocations or concurrent
planner/executor roles, and how should the local inference adapter account for
device capacity? This record informs the bounded implementation in
[`context_budget`](../../docs/architecture/context-budget.md). It does not
qualify a larger context, a multi-agent coding workflow, or a physical GPU cap.

## Evidence

### Context and task quality

Liu et al. find position-dependent performance in long-context retrieval and
multi-document QA experiments, including degradation when relevant information
falls in the middle. This is evidence against treating advertised context length
as uniform effective capacity. It is not a measured threshold for Metron's
pinned LFM3 model or coding workload. (Primary source: [Lost in the Middle](https://arxiv.org/abs/2307.03172).)

Agentless reports that a simple staged approach can be competitive with more
elaborate agents on the SWE-bench tasks it evaluates. Anthropic's multi-agent
research system reports gains on its research workload and substantially higher
token use. These results concern different models and tasks. Together they
support workload-specific comparison, not an optimal agent count or a predicted
gain for LFM3 coding. (Primary sources: [Agentless](https://arxiv.org/abs/2407.01489); [Anthropic multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system).)

The Google Research analysis of multi-agent scaling likewise reports that
benefits depend on task structure: parallelizable work can benefit, while
sequential tasks can degrade. It is not an LFM3 result and does not establish a
universal threshold. ([Towards a Science of Scaling Agent Systems](https://research.google/blog/towards-a-science-of-scaling-agent-systems-when-and-why-agent-systems-work/),
[paper](https://arxiv.org/abs/2512.08296).)

### Serving capacity

PagedAttention introduces block-managed KV allocation and scheduling to improve
serving efficiency. This is a runtime allocation mechanism, not a physical
memory guarantee. One must account for weights, cache pools, recurrent state,
workspaces, load/transient peaks, and allocator overhead. (Primary source:
[PagedAttention](https://arxiv.org/abs/2309.06180).) The pinned mistral.rs
adapter/backend behavior and existing Metron measurements are documented in
[R-0009](R-0009-mistralrs-batching-and-cache-contracts.md),
[R-0010](R-0010-lfm3-native-inference.md), [O-0015](../observations/O-0015-native-coding-v10-and-context.md)
and [O-0017](../observations/O-0017-paged-unpaged-resource-diagnostic.md).
In particular, O-0017's 513-token relative RMS difference is 4.115% for that
probe only; it is not a generally qualified tolerance.

For a hypothetical 2-byte-per-scalar paged BF16 calculation, a 32K planner
plus four 8K executors has a conditional KV payload estimate of
1,477,115,904 bytes (1.376 GiB) when simultaneous, versus 738,918,400 bytes
(0.688 GiB) for phased operation only if inactive role state is actually
released. Five 8K contexts estimate 923,467,776 bytes (0.860 GiB). These are
formula-derived payload estimates, not observed allocation or total-device
fit; they exclude weights, recurrent state and runtime overhead. The budgeted
admission route uses a conservative `paged_scalar4_upper_bound` to cover
runtime Auto dtype through four bytes per scalar, so its cache reservation is
higher. These 2-byte estimates exceed
the current 256 MiB experimental KV planning target, and the 32K planner also
exceeds the current 8192-token runtime profile limit. None is executable under
the current profile.

## Design implications

* Context capability, task quality, and resource fit are separate claims. A
  trained/runtime context ceiling does not establish positional robustness or
  physical fit.
* A role-allocation calculator should use checked arithmetic, explicit
  component bounds and provenance, and distinguish missing evidence from a
  measured capacity rejection. A missing upper bound is not zero.
* A larger-planner plus four 8K-executor allocation can be planned
  arithmetically, but the current 8192-token profile ceiling and same-profile
  adapter do not make it executable. Do not silently shrink requested executor
  budgets or imply five-agent support.
* For budgeted execution, reserve the full maximum resident-worker envelope
  atomically across drivers that share one coordinator, including the profile's
  maximum sequences and context. Keep the lease while the worker remains
  resident, including cancellation/unknown outcomes; release only after
  confirmed process exit. Per-cohort validation must run before any frame even
  for warm workers. This complements durable effect admission; it does not
  replace it.
* Coordinator scope is one host process. External budget documents do not
  become durable task or journal identity. Reapply policy on resume. Legacy
  unbudgeted execution retains compatibility but carries no resource-fit
  guarantee. Require the supplied policy path itself to be canonical and
  symlink-free outside the workspace, so a workspace-controlled alias cannot
  select a different external policy on a later resume. The host resolves and
  reads the path once per invocation; an operator may update the external
  regular file between invocations. This path rule does not create a general
  filesystem sandbox. Strict allocator/process memory caps remain unsupported.
* Select context size and concurrency using frozen, representative coding tasks
  and matched total resource limits. Compare one executor, four bounded
  executors, and phased versus simultaneous planning. Report accepted artifact
  rate, rendered input/output tokens, time to accepted artifact, observed
  resource peaks, and cancellation/failure behavior. Arithmetic fairness and
  KV estimates do not establish quality optimality.
* Anthropic's context-engineering guidance advocates bounded, task-relevant
  context and compact summaries. It informs assembly practice, not a numerical
  capacity or coding-quality claim for Metron. ([Effective context engineering
  for AI agents](https://www.anthropic.com/research/effective-context-engineering-for-ai-agents).)

## Proposed hypotheses and revisit conditions

H1: On a frozen Metron coding suite, some tasks benefit from a planner context
larger than the current profile, but the benefit is task-dependent. Revisit
after a matched coding evaluation with positional variants and accepted-patch
criteria.

H2: Concurrent executor contexts may improve wall time for sufficiently
parallelizable coding work when total resource limits are held constant.
Revisit only after the same suite compares sequential and concurrent runs and
measures accepted artifacts, latency, tokens, and resource peaks.

H3: A complete conservative resident-worker envelope can provide a useful
conditional admission guarantee within one host process and an operationally
exclusive device scope. Revisit if a supported shared host-wide coordinator or
backend-enforced hard cap is introduced.

## Relations

**constrains** [R-0008](R-0008-local-inference-resource-profile.md) and
[R-0009](R-0009-mistralrs-batching-and-cache-contracts.md);
**extends** [ADR 0021](../../docs/adr/0021-local-inference.md) and
[ADR 0025](../../docs/adr/0025-file-backed-inference-session-authority.md);
**proposes** [ADR 0026](../../docs/adr/0026-resource-budget-admission.md);
**supported by** [O-0015](../observations/O-0015-native-coding-v10-and-context.md)
and [O-0017](../observations/O-0017-paged-unpaged-resource-diagnostic.md).
