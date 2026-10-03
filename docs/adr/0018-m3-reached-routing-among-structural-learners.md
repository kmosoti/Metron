# ADR 0018: M3 is reached; the hand-authored Bayes router pays, learned routing does not

Status: accepted. Date: 2026-10-03.

## Context

ADR 0017 confirmed that a router (the anchors and one row per weight,
then the Bayes ranking of classes) closes about half the label-free gap
on fresh seeds. Its negative control showed that part of that gain was
the prefix: its rows double as verification, and no fixed strategy had
been offered them. It pre-registered one more test before running it.
On thirty fresh seeds (501 to 530) the test compares the control with
the best fixed order run after the same prefix, chosen on train.

## The ordering result

`experiments/reports/structure-ordering-arity8-0cd8b732427e-route.md`
covers 900 tasks: 420 train, 132 validation, 348 test, with pooled
hygiene verified. All 696 replays matched their simulated choice and
score.

| Test split, prefix: anchors and weights | Test cost | Solved |
|---|---:|---:|
| Single best fixed strategy, chosen on train (junta → symmetric → affine) | 40.5 | |
| Best fixed order after the same prefix, chosen on train (same order) | 32.9 | |
| Bayes ranking (control) | 29.6 | 99.7% |
| Tabular router (learned) | 30.4 | 98.9% |
| Ridge router (learned) | 31.4 | 99.7% |
| Reverse ranking (negative control) | 39.9 | 98.6% |
| Best router possible after the prefix | 23.1 | |
| Virtual best | 17.1 | |
| Entropy floor | 12.1 | |

- **Primary comparison:** the control's test cost minus that of the best
  fixed order after the same prefix is −3.39 probes per task (95% CI
  −4.84 to −0.59). The interval lies below zero: the choice pays beyond
  its prefix. ADR 0017's prediction holds.
- The control closes 0.47 of the gap against the single best fixed
  strategy (95% CI 0.21 to 0.64), the third run in a row whose interval
  excludes zero.
- The negative control now closes 0.03 (95% CI −0.36 to 0.31). The
  control beats it by 10.3 probes (95% CI 3.1 to 17.9).
- No learned router beats the control. Ridge is worse by 1.9 (95% CI 1.6
  to 2.0).

## The three runs together

| Run | Seeds | Prefix | Control's gap closed (95% CI) | Control minus fixed order after the prefix (95% CI) |
|---|---|---|---:|---:|
| exploratory | 301 to 310 | anchors (pre-registered) | 0.18 (−0.73 to 0.71) | 0.31 (−9.70 to 14.32), post hoc |
| exploratory | 301 to 310 | anchors and weights (secondary) | 0.50 (0.09 to 0.74) | −4.59 (−4.77 to −4.44), post hoc |
| confirmatory | 401 to 410 | anchors and weights | 0.55 (0.23 to 0.75), primary | −11.0 (−28.4 to 3.3), post hoc |
| ordering | 501 to 530 | anchors and weights | 0.47 (0.21 to 0.64) | −3.39 (−4.84 to −0.59), primary |

## Six angles

- **Information.** The floor (12.1) never forbade this. What a router can
  reach after its prefix (23.1) sits between the floor and the single best
  (40.5), and the control realises about half the distance.
- **Bayesian.** The posterior after sixteen structured rows is close to a
  sufficient statistic for the class. Ranking by it is enough; the learned
  routers found nothing it misses.
- **Algorithm selection.** The virtual best is now keyed by a property of
  the function (its class), and a cheap prefix makes that property
  observable, which is exactly the reopening condition of ADR 0011. The
  same machinery that failed on generator labels succeeds on properties.
- **Cost.** Tail risk dominates. With a failure cost of 512 and four
  verification probes, a single wrong commit outweighs many saved probes.
  The prefix that pays is the one that makes wrong routes rare.
- **Identifiability.** The anchors alone left juntas looking symmetric.
  The weights resolved it, and the result moved from inconclusive to
  confirmed. The prefix is the experiment's real design variable.
- **Implementation.** Every router is a real scheduler, every reported
  choice was replayed, and the splits stay NPN-clean when seeds are pooled.
  The verification count was chosen by a rule that favoured the fixed
  strategies (ledger entry 30), so the result holds despite that tilt.

## Decision

1. **M3 is reached.** On the structure-keyed laboratory, a hand-authored
   Bayes router, spending a sixteen-probe structural prefix and then
   trying the classes in posterior order, closes about half the
   label-free gap. It beats the best fixed order run after the same
   prefix by 3.4 probes per task. All three intervals that carried a
   claim exclude zero, two of them on fresh seeds pre-registered against
   the previous run's surprise.
2. **Learned routing is closed negative on this laboratory.** Tabular and
   ridge routers, fitted on train and chosen on validation, never beat the
   Bayes ranking in four comparisons. A learned router has a claim only
   where the posterior is not a sufficient statistic, for instance under
   costs it does not model; that would be a new laboratory and a new
   pre-registration.
3. **Scope.** The claim covers arity 8, the three structural classes of
   ADR 0015, a verification count of 4 and a failure cost of 512. Whether
   a system that can afford to search the union of the classes implicitly
   would beat the router in probes is the compute question of ADR 0015,
   decision 5. It remains open, and the floor of 12.1 is that
   question's reference point.
