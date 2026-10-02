# ADR 0011: Family selection does not realise the measured gap; M3 is closed on this configuration

Status: accepted; decision 2 superseded by ADR 0015. Date: 2026-10-02.

## Context

ADR 0010 passed the M3 gate on the first condition (a positive gap with an
interval excluding zero) and required, before any learned selector, a
hand-authored selector as the control. `metron-app::selector` implements
one: spend a prefix of `k` greedy probes, filter each family's version
space, commit to one family by a rule, run that family's restricted
pipeline. Two rules were measured on
`experiments/manifests/selectors-arity{5,6}.json` (10 seeds, 300 tasks per
arity, the same pools and cost model as ADR 0010):

- `most`: the family with the most surviving members, which is the
  maximum-a-posteriori family under a uniform prior over the pool;
- `fewest`: the family with the fewest surviving members (at least one),
  the negative control.

Reports: `experiments/reports/selectors-arity5-*.md` and
`selectors-arity6-*.md`.

## What was measured

Solved rate and mean cost (probes, failure cost 64 at arity 5 and 128 at
arity 6), against the single best fixed solver `greedy-pool` (100% solved,
8.0 probes):

| Selector | arity 5 solved | arity 5 mean cost | arity 6 solved | arity 6 mean cost |
|---|---:|---:|---:|---:|
| most, k = 0 | 16.7% | 54.2 | 16.7% | 107.6 |
| most, k = 2 | 25.7% | 49.2 | 29.0% | 92.7 |
| most, k = 4 | 33.3% | 44.8 | 38.3% | 81.5 |
| most, k = 6 | 55.0% | 32.7 | 49.0% | 68.8 |
| fewest, k = 3 | 6.3% | 60.2 | 6.0% | 120.6 |
| `greedy-pool` (SBS) | 100% | 8.0 | 100% | 8.0 |

Even at k = 6 the selector that solves a task spends about 7.0 probes on
it, within one probe of the single best solver, and still fails half the
tasks. More prefix only converges on the unrestricted strategy. Under the
work-aware cost model (`probes-plus-work`) the ranking does not change.

## Why

The virtual best solver in ADR 0010 is defined by the target's *family
label*, and a restricted pipeline saves probes only by *assuming* that
label. The label is a property of the generator, not of the function: the
families overlap heavily (monotone functions that are thresholds, shallow
decision trees that are small DNFs), so after any short prefix the family
with the most consistent members is frequently not the one the target was
drawn from. Committing to a family's lone survivor is then a confidently
wrong answer. Verifying the family would mean eliminating the other
families' survivors, which is the unrestricted identification the
selector was trying to avoid.

So the SBS–VBS gap overstates realisable headroom whenever the per-task
best is keyed by information that is not a function of the observable
instance. The gap measured in ADR 0010 is real as an upper bound and
unreachable as a target; the hand-authored control shows it, which is
exactly what the control was for (compare Milli, Lieder and Griffiths
2017: a small fixed repertoire can be bounded-optimal when metareasoning
has real cost).

## Decision

1. M3 is closed as a negative result on this laboratory configuration.
   No learned selector (tabular, LinUCB, Bayesian strategy regression) is
   built for it: the control already establishes that the realisable gap
   is near zero, and a learner cannot recover information the probes do
   not carry.
2. M3 reopens only with a configuration in which the per-task best is
   keyed by *identifiable* structure, verifiable for fewer probes than it
   saves. Candidates: pools large enough that log2 |pool| exceeds the
   cost of a structural test by a wide margin (the affine test costs
   n + 1 probes plus verification; the pools here cost about 8 probes to
   search outright); or families with disjoint, cheaply testable
   signatures. The headroom harness must then be re-run and the gap
   re-measured before any selector work.
3. The selector, the rule parameter and the sweep manifests stay in the
   repository as the standing control for any future routing claim.

## Consequences

- `docs/architecture/milestones.md` records M3 as closed on this
  configuration with the reopening condition.
- Any future "routing pays" claim must show a selector column with a
  gap-closed interval excluding zero on an NPN-clean test split, against
  the fixed strategies and this control.
- The finding feeds the laboratory design: the next task-set
  specification should make family membership an observable property.
