# ADR 0017: Routing pays on fresh seeds; whether the choice pays beyond its prefix is pre-registered

Status: accepted. Date: 2026-10-03.

## Context

ADR 0016 recorded that on the pre-registered prefix no router closed the
gap with an interval excluding zero. It also recorded that on a secondary
prefix (the affine anchors plus one row of every weight) the hand-authored
Bayes ranking closed 0.50. It pre-registered a single confirmatory
comparison on fresh seeds 401 to 410:
`experiments/manifests/structure-confirm-arity8.json`, committed before
the run.

## The confirmatory result

`experiments/reports/structure-confirm-arity8-3863ebaa25ab-route.md`
gives 150 train, 33 validation and 117 test tasks, with pooled hygiene
verified. All 234 replays of the control and the selected router matched
their simulated choices.

| Primary prefix: anchors and weights | Test cost | Solved | Gap closed | 95% CI |
|---|---:|---:|---:|---:|
| Single best fixed strategy, chosen on train | 49.8 | | 0 | |
| Bayes ranking (control), selected on validation | 32.1 | 100% | 0.55 | 0.23 to 0.75 |
| Tabular router (learned) | 34.0 | 99.1% | 0.49 | 0.07 to 0.73 |
| Ridge router (learned) | 43.8 | 97.4% | 0.18 | −0.51 to 0.59 |
| Reverse ranking (negative control) | 39.3 | 99.1% | 0.33 | −0.16 to 0.59 |
| Best router possible after the prefix | 23.7 | | | |
| Virtual best | 17.4 | | 1 | |

**The pre-registered comparison succeeds.** The control's interval
excludes zero, and ADR 0016's prediction holds. No learned router beats
the control in either run.

## What the negative control says

The reverse ranking, which tries the least likely class first, also
improves on the single best fixed strategy here (point estimate 0.33).
The control beats it by 7.2 probes, with an interval from −2.9 to 18.7.
The reason is mechanical. The prefix's sixteen rows are observations that
every learner's solve must agree with, so they double as verification,
and no strategy in the fixed portfolio spends them. Part of the
measured gain is therefore the prefix, not the choice.

A post-hoc analysis, now in every routing report and labelled as such,
compares each router with the best fixed order run after the same
prefix, chosen on train:

| Run | Best fixed order after the prefix | Its test cost | Control minus it | 95% CI |
|---|---|---:|---:|---:|
| exploratory (301 to 310) | junta, symmetric, affine | 32.5 | −4.6 | −4.8 to −4.4 |
| confirmatory (401 to 410) | symmetric, junta, affine | 43.1 | −11.0 | −28.4 to 3.3 |

The ordering's own share is suggested by one run and not established by
the other. At ten seeds, the second run's three expensive failures make
it too noisy to tell.

## Decision

1. **M3's pre-registered criterion is met.** A selector, the hand-authored
   Bayes ranking after the anchors and weights, closes 0.55 of the
   label-free gap (95% CI 0.23 to 0.75) on fresh seeds. The claim covers
   the router as a whole: prefix plus ranking.
2. **Learned routing is closed negative on this laboratory.** Neither the
   tabular nor the ridge router beat the control in either run. The
   posterior is close to a sufficient statistic, and what the learners
   picked up (fail-safe ordering) the control gets from a better prefix.
3. **Whether the choice pays beyond its prefix is pre-registered now,**
   before it runs. `experiments/manifests/structure-ordering-arity8.json`
   repeats the confirmatory manifest except in three respects: seeds 501
   to 530 (thirty seeds, about 900 tasks, against the tail noise), the
   anchors-and-weights prefix only, and the primary comparison set to
   `fixed-order-after-prefix`. The single comparison is the control's test
   cost minus that of the best fixed order run after the same prefix,
   chosen on train. The ordering pays if the 95% interval lies below zero.
4. **Prediction:** it does, by a few probes per task, as in the
   exploratory run.
5. ADR 0018 records the outcome.
