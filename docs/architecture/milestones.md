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
| M3 | A selector closes measured headroom under matched budgets | closed as a negative result on this configuration (ADR 0011); reopens with an identifiable-structure task set |
| M4 | Candidates are judged, not trusted | open |
| M5 | Retrieval earns its place or leaves | open |
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

## M4 — Candidates are judged, not trusted

**Outcome.** Compositions proposed from episodes become
`CapabilityCandidate`s; the laboratory's `PromotionGate` judges them on
held-out targets at matched compute; reuse frequency is measured.

**Exit criteria.** Promotion decisions are journaled with the gate's
fingerprint; a candidate that fails held-out targets is rejected in tests;
a report states reuse frequency against the inlined reference at equal
budget.

## M5 — Retrieval earns its place or leaves

**Outcome.** A cross-episode retrieval workload (given k observed rows,
retrieve previously solved functions consistent with them from a store of
10⁴ to 10⁶ entries) compared across exact bitmask scan, a Bloom filter, an
ANN index and hyperdimensional codes, measured as recall@k per byte and per
nanosecond.

**Exit criteria.** A report with the decision. If the exact scan wins at
this scale, hyperdimensional retrieval is dropped and the report says so.

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
