# ADR 0006: Defer HDC, learned routing, model integration and the mixed Boolean lab

Status: accepted. Date: 2026-10-02.

## Context

The deep-research proposal (`docs/research/proposal-deep-research-2026-10.md`)
makes hyperdimensional memory foundational, plans learned routing (LinUCB,
eligibility traces, Hebbian updates) and model calls early, and benchmarks
on a Boolean lab. The prior-art review (`docs/research/prior-art-review-2026-10.md`)
shows that:

- at n ≤ 4 with 16 queries the lab is trivial and cannot test the routing
  or representation hypotheses; it must use n = 5–8, mixed structural
  families, NPN-deduplicated splits and binding enumeration budgets;
- adaptive routing can at most close the single-best-solver to
  virtual-best-solver gap, so headroom must be measured first, otherwise
  one trains a selector over strategies that do not meaningfully differ;
- HDC cannot win on recall per byte for the lab's native objects and
  should be a replaceable retrieval experiment with exact-scan and
  Bloom-filter baselines, not the substrate;
- results should be reported rliable-style over ≥ 10 seeds.

## Decision

The first slice establishes only the objects and the four invariants.
The following are explicitly deferred, in this order, each gated on the
previous:

1. **Mixed Boolean lab** in `metron-lab`: families (parity/affine, monotone,
   read-once, threshold, k-term DNF, decision trees) at n = 5–8, NPN
   canonicalisation for split hygiene, a greedy-splitting baseline, the
   Hegedűs bracket as the query ruler, an oracle-optimal query tree for
   small classes.
2. **Headroom measurement**: SBS and VBS over fixed schedules; "gap closed"
   reporting. If VBS − SBS is small, routing work stops here.
3. **Routing policies** in `metron-app`: only if headroom exists, with
   tabular, LinUCB and Bayesian strategy-selection controls, and the
   Hebbian rule described honestly as REINFORCE with baseline plus traces.
4. **Capability promotion**: candidates judged by the lab's gate on
   held-out targets with matched compute and measured reuse frequency.
5. **HDC retrieval** as an experiment in `metron-adapters` or a dedicated
   crate, against exact bitmask scan and Bloom-filter baselines, measured
   as recall@k per byte and per nanosecond.
6. **Model clients** in `metron-adapters`, as receipted external calls.

## Consequences

- There is no `metron-memory` crate and no `RoutingPolicy` trait; the
  `Scheduler` trait has one implementation, `FixedSchedule`.
- The core carries no model types.
- The `Protocol`, `Verdict` and `PromotionGate` are in place so that steps
  1–4 extend the lab instead of restructuring it.
