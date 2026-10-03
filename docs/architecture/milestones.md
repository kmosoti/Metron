# Milestones, by outcome

Milestones are outcomes, not dates. Each one names what must be true when
it is done and how that is checked. A milestone is reached when its exit
criteria hold in the repository, not when a calendar says so. Later
milestones are gated on earlier ones; a gate can close a milestone as
"not worth doing", which is also a result.

| # | Outcome | Status |
|---|---|---|
| M0 | The rules are executable | reached |
| M1 | The laboratory can tell strategies apart | reached |
| M2 | We know whether routing can pay | reached: ADR 0010, `experiments/reports/headroom-arity{5,6}-*.md` |
| M3 | A selector closes measured headroom under matched budgets | closed on the family-labelled lab (ADR 0011, explained by the entropy floor); on the structure-keyed lab, not reached on the pre-registered prefix (ADR 0016); confirmatory run pre-registered |
| M4 | Candidates are judged, not trusted | reached: ADR 0012, `experiments/reports/promotion-arity5-7f5ef8af8c7d.md` |
| M5 | Retrieval earns its place or leaves | reached, HDC dropped: ADR 0013, `experiments/reports/retrieval-arity6-e59c328070b5.md` |
| M6 | An LLM can propose, never decide | reached |

## M0 — The rules are executable

**Outcome.** The four invariants (`invariants.md`) are enforced by types and
by `tests/architecture`; one sealed task family and one fixed pipeline run
end to end; journals replay to the same head hash.

**Exit criteria.** `cargo test --workspace` passes the architecture suite.
The smoke manifests run, verify, and reproduce their journal heads.

## M1 — The laboratory can tell strategies apart

**Outcome.** Mixed structural families at arities 5 to 8, published
hypothesis pools, NPN-clean splits, query rulers, and strategies whose cost
profiles differ by family.

**Exit criteria.**
- NPN canonicalisation matches OEIS A000370 at arity ≤ 4 and is invariant
  under random transforms at arities 5 to 8 (`metron-lab::npn` tests).
- No NPN class spans two splits (`TaskSet::verify_split_hygiene`, run on
  every generated set).
- Greedy splitting sits between the information lower bound and the exact
  optimal tree on explicit small classes (`metron-lab::bounds` tests).
- At least two fixed strategies whose per-family costs differ (visible in
  the M2 report: affine shortcut versus pool-wide greedy, restricted versus
  unrestricted version spaces).

## M2 — We know whether routing can pay

**Outcome.** A headroom report: single best solver, virtual best solver,
gap with a stratified bootstrap interval, per-family breakdown, over at
least ten task-set seeds; and a recorded decision on routing.

**Exit criteria.** `experiments/reports/` holds the report produced by
`metron headroom`, reproducible from the manifest; an ADR records the
go/no-go for M3 and why.

**Result.** At arities 5 and 6 over ten seeds, the single best fixed
strategy spends 8.0 probes, a perfect per-task selector 5.4; the gap is
2.6 probes (95% interval 2.6 to 2.7), which is log2 of the number of
families. Routing proceeds with the expectation that only part of that gap
is realisable, because a selector must pay to identify the family. ADR
0010 records the decision.

## M3 — A selector closes measured headroom under matched budgets

**Gate.** M2 reports a gap whose interval excludes zero and that a
hand-authored heuristic does not already close.

**Outcome.** One or more learned selectors with the controls the prior-art
review names (fixed heuristic, tabular, LinUCB, Bayesian strategy
regression), trained on the train split, chosen on validation, reported on
the untouched test split with matched budgets.

**Exit criteria.** The selector's gap-closed interval excludes zero and
beats the best hand-authored heuristic, or the negative result is written
up with the same rigour.

**Result.** The hand-authored control (`metron-app::selector`, measured in
`experiments/reports/selectors-arity{5,6}-*.md`) solves at most 55% of
tasks at any prefix length and spends about as many probes per solved task
as the single best fixed strategy. The family label that defines the
per-task best is not identifiable from probes, so the gap is an upper
bound, not a target. M3 is closed on this configuration (ADR 0011) and
reopens only with a task set whose per-task best is keyed by structure
that can be verified for fewer probes than it saves.

**The floor.** The entropy of the target distribution bounds every
strategy that is not told the label (ADR 0015). On the family-labelled
lab it is `log2 240 = 7.907`; the single best solver sits 0.126 and 0.070
above it, so at most 4.8% and 2.7% of the gap was reachable, and the
negative result above was predictable from arithmetic. Every headroom
report now states the floor.

**Reopened (ADR 0015).** A structure-keyed task set publishes no pool:
affine, symmetric and three-variable junta targets at arity 8, each class
learned by its own representation, with the truth table as the fallback.
Headroom is measured over label-free strategies only; the floor (about
12.1 probes) sits far below the best cascade, so routing is not ruled out
by arithmetic. The protocol, the controls, the success criteria and four
predictions are pre-registered in ADR 0015.

**Result on the structure-keyed lab (ADR 0016).** Headroom is real: a
label-free gap of 16.2 probes (95% CI 12.3 to 21.5) above a floor of 12.1.
On the pre-registered prefix (the affine anchors) no router's gap-closed
interval on the test split excludes zero. The hand-authored Bayes ranking
closes 0.18, and the learned tabular and ridge routers close 0.41 and
0.38, all with intervals through zero. Two wrong answers at 512 each
outweigh the saving. On a secondary prefix (the anchors plus one row per
weight) the Bayes ranking closes 0.50 (CI 0.09 to 0.74). With eight
intervals in play that is a hypothesis, so a confirmatory run on fresh
seeds 401 to 410 is pre-registered with that prefix as primary and a
single comparison.

## M4 — Candidates are judged, not trusted

**Outcome.** Compositions proposed from episodes become
`CapabilityCandidate`s; the laboratory's `PromotionGate` judges them on
held-out targets at matched compute; reuse frequency is measured.

**Exit criteria.** Promotion decisions are journaled with the gate's
fingerprint; a candidate that fails held-out targets is rejected in tests;
a report states reuse frequency against the inlined reference at equal
budget.

**Result.** `metron promote` proposes candidates from train-split episodes
and the gate judges them on the test split. The exhaustive and greedy
compositions are promoted at 100% held-out; the blind affine composition
is rejected at 30%; the greedy fallback extracted from the verified-affine
strategy is promoted at 100% where its parent solved 80%. ADR 0012.

## M5 — Retrieval earns its place or leaves

**Outcome.** A cross-episode retrieval workload (given k observed rows,
retrieve previously solved functions consistent with them from a store of
10⁴ to 10⁶ entries) compared across exact bitmask scan, a Bloom filter, an
ANN index and hyperdimensional codes, measured as recall@k per byte and per
nanosecond.

**Exit criteria.** A report with the decision. If the exact scan wins at
this scale, hyperdimensional retrieval is dropped and the report says so.

**Result.** On a store of 5,126 functions on six inputs, the bitmask index
has 100% recall at 16 bytes per entry and about 10 microseconds per query;
8,192-bit hyperdimensional codes reach 96 to 99% recall at 1,024 bytes per
entry and 100 times the query time. Hyperdimensional retrieval is dropped
for the laboratory's native objects. ADR 0013.

## M6 — An LLM can propose, never decide

**Outcome.** Language-model inference enters through a Claude Code session,
not an API client: a consult operator writes a request file, the episode
suspends to a checkpoint, the session answers, the episode resumes, and
the proposal is verified against every observation before it reaches any
frame that can be committed. Every call is receipted and journaled in
full, so the run replays without the session.

**Exit criteria.** The suspend → answer → resume → replay cycle passes in
`tests/architecture`; a consult operator in a strategy is scored under the
same headroom harness as every other strategy.
