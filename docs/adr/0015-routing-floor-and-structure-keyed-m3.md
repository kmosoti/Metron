# ADR 0015: The routing floor; M3 reopens on a structure-keyed task set

Status: accepted. Date: 2026-10-02. Supersedes decision 2 of ADR 0011
(the reopening candidates); the rest of ADR 0011 stands.

## Context

ADR 0011 closed M3 on the mixed-family configuration: the hand-authored
selector never beat the single best fixed strategy, because the family
label that defines the per-task best is not in the probes. It named two
ways to reopen: bigger pools, or families with disjoint, cheaply testable
signatures. Before building anything for either, one question had not been
asked: how much of a measured gap can any router reach at all?

## The floor

For targets drawn from a known distribution `P`, a strategy that always
answers correctly is a query tree whose leaves form a prefix code, so its
expected number of probes is at least the entropy `H(P)` (Kraft's
inequality). A router that is not told the label is such a strategy.
Hence:

- every label-free strategy averages at least `H(P)` probes on correct
  answers, and a wrong answer costs more than finishing the identification
  whenever the failure cost exceeds the probe cap plus `log2` of the
  support, which holds for every pre-registered cost model here;
- a router can save at most `SBS − H(P)` against the single best solver.
  The virtual best solver can sit below `H(P)` only by spending
  information the strategies were never given.

`bounds::optimal_expected_depth` computes the exact optimum on small
explicit classes, and `expected_queries_are_bounded_below_by_entropy`
checks the bound on 80 class-and-weighting cases (mean slack 0.066 bits).
`TaskSet::entropy_floor` computes `H(P)` for a task set, and every headroom
report now carries it.

On the committed reports the floor is `log2 240 = 7.907` at both arities.
The single best solver (`greedy-pool`) sits 0.126 above it at arity 5 and
0.070 at arity 6, against gaps of 2.64 and 2.62: at most 4.8% and 2.7% of
the gap was ever reachable. The virtual best sits 2.5 probes *below* the
floor. ADR 0011's negative result follows from one line of arithmetic per
task set.

## Consequences for reopening

1. **Bigger pools cannot reopen routing.** Whenever the target's support is
   published and a near-Bayes-optimal search over it is affordable, that
   search stays within a fraction of a probe of the floor (ledger entry 4)
   and routing headroom in probes is that fraction. ADR 0011's first
   candidate is withdrawn (ledger entry 22).
2. **Routing can pay in probes only where no fixed strategy can search the
   support.** That is the case when the support is defined by structure
   rather than published as a list, and each representation learns its
   own class cheaply but cannot learn the others. Then the best fixed
   strategy is a cascade of representation-specific learners, and the
   waste of trying the wrong one first is headroom a router could recover.
3. **Headroom is measured over label-free strategies.** A strategy that is
   correct only when told the family (the restricted searches of ADR 0010,
   a learner without verification) stays in reports as a reference column
   but is not a candidate for the single best or the virtual best solver.
   Its virtual best measures the value of the label, not routing headroom.
4. **Every headroom report states the floor**, and a routing claim states
   how much of the realisable bound `SBS − H` it closes, not only how much
   of the gap.
5. **Compute is the other place routing can pay** (ADR 0010, decision 3c;
   the review: "only compute and prior can differ"). A union search can
   reach the floor in probes at a cost in work that a resource-bounded
   system may not afford. That question is deferred; M3 first asks the
   probe question where it is not ruled out.

## M3 v2, pre-registered

Fixed before any measurement on the seeds below.

**Laboratory.** Arity 8 (256 rows). Three structural classes, disjoint by
construction, each a property of the function rather than a generator
label:

| Class | Definition | Members |
|---|---|---:|
| `affine` | `a·x ⊕ b` with `a ≠ 0` (parity and its complement included) | 510 |
| `symmetric` | depends only on the number of ones; not constant, not affine | 508 |
| `junta` | exactly three relevant variables; not affine | 12,096 |

Targets are drawn uniformly over the classes, then uniformly within the
class. No hypothesis pool is published. The question publishes the
promise instead: the classes, their definitions and sizes, and the prior.
The floor is `log2 3 + (log2 510 + log2 508 + log2 12096) / 3 ≈ 12.1`
probes. Ten targets per class per seed, ten seeds, NPN-clean splits with
fractions 0.5 / 0.2 / 0.3. Probe cap 256; failure cost 512 (twice the cap,
as in ADR 0010).

**Representations** (system side, one learner per class, each verified
against every observation before it may commit):

- affine: probe the zero row and the unit vectors, solve over GF(2);
- symmetric: probe one row of each Hamming weight, read off the weight
  profile;
- junta: find relevant variables by binary search between disagreeing
  rows, then probe the table on them;
- the truth table, probed row by row, as the final fallback.

A learner commits only after `v` further probes at random unobserved rows
agree with its hypothesis. `v` is chosen from {4, 8, 12, 16} on pilot
seeds 900 to 904, disjoint from the measurement seeds, as the value that
minimises the best cascade's mean cost; the pilot is reported.

**Fixed strategies (label-free):** the six cascades (every order of the
three learners, each followed by the next on refutation and by the truth
table last) and the truth table alone. **References (label-assuming, not
SBS or VBS candidates):** each learner without verification or fallback.

**Routers.** A router spends a prefix, reads features, and picks one of
the six cascades.

- Prefix: the affine anchors (the zero row and the unit vectors, nine
  probes). Secondary: the anchors plus one row of every weight.
- Features: the exact posterior over the three classes after the prefix,
  computed by a transform from per-class counts of consistent members
  (GF(2) rank for affine, weight consistency for symmetric, an exact count
  over relevant-variable sets for juntas) under the published prior.
- Hand-authored control: order the cascade by posterior, highest first.
  This is the survivor-count rule of ADR 0011 generalised from counting a
  pool to counting a class. Negative control: lowest first.
- Learned: a tabular router (bucket by the most probable class and its
  posterior, choose the cascade with the lowest mean train cost) and a
  ridge regression of each cascade's cost on the features (choose the
  lowest prediction). Fitted on the train split, chosen on validation,
  reported once on test.

**Success criteria.** Routing pays if a router's gap-closed 95% interval
on the test split (stratified bootstrap over classes) excludes zero
against the label-free single best solver. Learning pays if, in addition,
a learned router beats the hand-authored control with an interval on the
paired difference that excludes zero. Either can fail; the result is
recorded in an ADR whichever way it goes.

**Predictions** (from the analysis in this ADR, to be checked):

1. The label-free gap is positive and its interval excludes zero.
2. The floor sits far below the single best solver, so routing is not
   ruled out by arithmetic this time.
3. The hand-authored control closes at least a third of the label-free
   gap.
4. The learned routers do not beat the control: the posterior is close to
   a sufficient statistic for the class.

**Threats to validity.** The union of the three classes has about 13,000
members, so an unbounded system could search it implicitly and stay near
the floor; M3 v2 asks the resource-bounded question, and the floor is
reported as the unbounded reference. The learners and the verification
rule are designed by the same hand that designed the routers; the pilot
seeds keep the choice of `v` off the measurement seeds. Generator
uniformity is tested, not assumed.
