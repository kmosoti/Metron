---
id: O-0002
type: observation
status: historical-snapshot
updated: 2026-11-10
added_by: agent
human_ratified: false
---

# Laboratory results and milestone snapshot

## Source and scope

This record preserves the laboratory findings and milestone summary from the
[main README at 0170c22](https://github.com/kmosoti/Metron/blob/0170c22c7e8de92cc73740f4e34a7bb25f2d4980/README.md).
The findings apply to the Boolean-function laboratories and workloads described
below. Product runtime work has its own proposal and release criteria.

The text and milestone diagram capture that source revision. The linked figures
continue to be generated from committed reports and the priors ledger, with
freshness checked by the architecture suite. The source revision retains the
original figure versions for historical comparison.

## What the laboratory has found

**A perfect router would save probes.** Six structural families, ten
task-set seeds. A clairvoyant per-task router beats the best fixed strategy
by about log2 6 probes: exactly the information in the family label. It
sits below the entropy floor, so no router that has to find the label for
itself could get there.

![Cost ladder: exhaustive, verified affine, greedy over the pool, perfect router](../../docs/figures/headroom.svg)

**But the label it routes on is not in the probes.** A real router has to
learn the family from the same probes it is trying to save. It cannot: more
prefix only walks it back to the single best strategy.

![Solved rate of the most-survivors router against prefix length](../../docs/figures/selectors.svg)

**Where the structure is in the function, routing pays.** In a
laboratory with no published pool, each class (affine, symmetric,
three-variable juntas) is learned by its own representation. A router
spends sixteen structural probes, ranks the classes by posterior, and
tries them in that order. On unseen tasks it closes about half the gap
and beats the best fixed order given the same probes. That took three
pre-registered runs, and learned routers never beat the hand-written
Bayes rule.

![Routers against the single best cascade on the structure-keyed laboratory](../../docs/figures/routing.svg)

**Exact beats hyperdimensional for these objects.** Retrieving every stored
function consistent with a few observed rows, the bitmask index is complete,
smallest and fastest.

![Recall against bytes per entry for exact, Bloom and hyperdimensional indexes](../../docs/figures/retrieval.svg)

**The system proposes; the laboratory decides.** Compositions extracted from
episodes are judged on held-out targets, never on the episodes that produced
them.

![Held-out pass rates of proposed compositions](../../docs/figures/promotion.svg)

**Every prior is a hypothesis.** Whatever the design relies on, whether
recalled, cited or derived, carries a test or a primary source.

![The priors ledger by status](../../docs/figures/priors.svg)

These pictures are drawn from the committed reports by `metron figures`, and
the test suite fails if a report changes without its picture.

## Where it stands

```mermaid
flowchart LR
    M0["M0<br/>the rules are executable"]:::reached
    M1["M1<br/>the lab tells strategies apart"]:::reached
    M2["M2<br/>headroom measured"]:::reached
    M3["M3<br/>routing<br/>reached on structure: the Bayes router pays<br/>learned routing: closed"]:::reached
    M4["M4<br/>candidates judged on held-out targets"]:::reached
    M5["M5<br/>retrieval<br/>hyperdimensional codes dropped"]:::dropped
    M6["M6<br/>an LLM proposes, never decides"]:::reached
    M0 --> M1 --> M2 --> M3
    M1 --> M4
    M1 --> M5
    M0 --> M6
    classDef reached fill:#d3f2e3,stroke:#009E73,color:#0b3d2a
    classDef closed fill:#fde4d6,stroke:#D55E00,color:#5a1e00
    classDef dropped fill:#eceff1,stroke:#8c959f,color:#24292f
```

Milestones are outcomes with exit criteria, never dates. A milestone closed
by a negative result is a result.

## Relations

- supported-by [committed experiment reports](../../experiments/reports/README.md)
- explained-by [architecture decisions](../../docs/adr/README.md)
- contextualized-by [scientific priors](../../docs/research/priors.md)
- tracked-by [laboratory milestone criteria](../../docs/architecture/milestones.md)
- linked-from [project overview](../../README.md)

## Revisit when

New measurements or accepted decisions change a laboratory conclusion. Record
new evidence with its workload and source revision, and link it to this snapshot.
