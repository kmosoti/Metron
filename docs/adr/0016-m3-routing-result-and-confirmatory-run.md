# ADR 0016: Routing does not pay on the pre-registered prefix; a confirmatory run is pre-registered

Status: accepted. Date: 2026-10-03.

## Context

ADR 0015 pre-registered M3 on the structure-keyed laboratory. Its pilot
(seeds 900 to 904, `experiments/reports/structure-pilot-arity8-*`) fixed
the verification count at 4. The fresh headroom
(`experiments/reports/structure-arity8-810f8aa80f03.md`, seeds 301 to 310)
held predictions 1 and 2: a label-free gap of 16.2 probes (95% CI 12.3 to
21.5) and an entropy floor of 12.1, far below the single best cascade.

`metron route` then ran every label-free fixed strategy and every
candidate after each prefix on the same 300 tasks, fitted the learned
routers on the 147 train tasks, chose on the 37 validation tasks, and
judged once on the 116 test tasks. Splits are assigned by hashing each
task's NPN class, and pooled hygiene was verified. The control and the
selected router were replayed as real schedulers on every test task; all
232 replays matched their simulated choice and score. Report:
`experiments/reports/structure-arity8-810f8aa80f03-route.md`.

## Result on the pre-registered primary prefix (the affine anchors)

| | Test cost | Solved | Gap closed | 95% CI |
|---|---:|---:|---:|---:|
| Single best cascade, chosen on train (junta → symmetric → affine) | 38.6 | | 0 | |
| Bayes ranking (hand-authored control), selected on validation | 34.8 | 98.3% | 0.18 | −0.73 to 0.71 |
| Tabular router (learned) | 29.9 | 100% | 0.41 | −0.17 to 0.78 |
| Ridge router (learned, λ = 0.01) | 30.6 | 100% | 0.38 | −0.08 to 0.65 |
| Reverse ranking (negative control) | 53.1 | 95.7% | −0.69 | −2.05 to 0.17 |
| Best router possible after the anchors | 19.2 | | | |
| Virtual best (told the class) | 17.4 | | 1 | |

Against the criteria of ADR 0015:

1. **Routing pays: no.** No router's interval excludes zero.
2. **Learning pays: no.** No learned router beats the control with an
   interval on the paired difference that excludes zero.
3. Prediction 3 (the control closes at least a third) **fails**: 0.18.
4. Prediction 4 (learned routers do not beat the control) **holds**.

On the secondary prefix (the anchors plus one row of every weight), the
control costs 27.9, solves every test task, and closes 0.50 of the gap
(95% CI 0.09 to 0.74). The learned routers cost 32.3 and 30.6.

## Why, from six angles

- **Information.** The routing information is there. The best router
  possible after the anchors costs 19.2, within 1.8 probes of the virtual
  best. The floor no longer forbids routing; something else stops it.
- **Bayesian.** The control routes by the most probable class and ignores
  what a wrong route costs. After the anchors, a junta whose relevant
  variables are all insensitive at zero looks symmetric (posterior about
  0.25 against 0.12). The control sent 11 of 42 test juntas to a learner
  for another class first.
- **Cost.** With a failure cost of 512, routing is a tail-risk problem. Two
  wrong answers out of 116 add 8.8 probes to the control's mean, more than
  its whole saving, and they make the interval straddle zero.
- **Algorithm selection.** The learned routers learned the risk. Ridge
  sends every task junta-first, the order that fails safe, and solves
  everything. Validation, at 37 tasks, cannot tell routers apart when
  they differ by one or two tail events. It chose the control by 0.4
  probes, and on test the control was the worst of the three.
- **Identifiability.** Adding one row per weight to the prefix resolves
  most of the junta-symmetric ambiguity. Junta tasks fall from 70.2 to
  41.9 under the control.
- **Implementation.** The pilot rule chose `v` by the best fixed cascade.
  That cascade is junta-first and fails safe, so the rule favoured little
  verification. A router that tries the likely class first needs more. The
  pre-registered rule, fixed before any router existed, tilted the design
  toward the single best solver. That is a lesson for the next design, not
  a reason to rerun this one.

## Multiplicity

Two prefixes times four routers is eight intervals. One of them clearing
zero is not evidence. The secondary result is a hypothesis.

## Decision

1. **M3 is not reached on the pre-registered analysis.** The negative is
   recorded as such.
2. **A confirmatory run is pre-registered here, before it runs.**
   `experiments/manifests/structure-confirm-arity8.json` repeats
   `structure-arity8` exactly except in two respects. Its seeds are 401
   to 410 (fresh targets). Its primary prefix is the anchors plus one row
   of every weight; the anchors alone are reported as secondary.
   Strategies, `v = 4`, candidates, routers, bins, penalties, cost model,
   splits and resamples are unchanged.
3. **One confirmatory comparison:** the hand-authored control's gap-closed
   95% interval on the test split, primary prefix, must exclude zero. The
   learned routers and the secondary prefix are reported but carry no
   claim. Class-hash splitting keeps each NPN class in the split it had in
   the exploratory run, so the confirmation is about new targets, not new
   classes.
4. **Prediction:** the control closes a positive share of the gap with an
   interval that excludes zero. Some regression from the exploratory 0.50
   is expected.
5. ADR 0017 records the outcome and M3's status, whichever way it goes.
