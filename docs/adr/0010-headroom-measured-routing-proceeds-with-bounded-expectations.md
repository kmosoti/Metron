# ADR 0010: Headroom is measured; routing proceeds, with bounded expectations

Status: accepted. Date: 2026-10-02.

## Context

ADR 0006 and milestone M2 require measuring single-best-solver versus
virtual-best-solver headroom before any adaptive routing is built. The
measurement was run with `metron headroom` on
`experiments/manifests/headroom-arity5.json` and `headroom-arity6.json`:
six structural families, pools of 40 members per family, 5 targets per
family, 10 task-set seeds (300 tasks each), NPN-clean splits, a probe cap of
32 (arity 5) and 64 (arity 6), and a pre-registered cost model of one unit
per probe with a PAR-style failure cost of twice the cap. Reports:
`experiments/reports/headroom-arity5-a7988cb29cc7.md` and
`experiments/reports/headroom-arity6-0e6e2ae3ccfa.md`.

## What was measured

| | arity 5 | arity 6 |
|---|---:|---:|
| Single best solver | `greedy-pool`, 8.03 probes, 100% solved | `greedy-pool`, 7.98 probes, 100% solved |
| Virtual best solver | 5.39 | 5.36 |
| Gap (95% CI, stratified by family) | 2.64 (2.57 to 2.72) | 2.62 (2.55 to 2.69) |
| VBS / SBS | 0.671 | 0.672 |
| Family-restricted greedy, on its own family | 5.3 to 5.5 probes | 5.3 to 5.4 probes |
| Family-restricted greedy, off its family | fails (penalised) | fails (penalised) |
| Exhaustive | 32 | 64 |
| Affine shortcut, blind | 6 or 7 probes; 17% solved | 7 probes; 17% solved |
| Affine shortcut, verified then greedy | 92% solved | 83% solved |

The per-task best is always the version-space strategy restricted to the
target's own family. Unrestricted greedy identification costs about
log2(240) ≈ 7.9 probes; restricted identification costs about log2(40) ≈
5.3. The gap, 2.6 probes, is log2(6) ≈ 2.58 bits: the information needed
to say which of six families the target belongs to.

## Decision

1. The M3 gate's first condition holds: the gap is positive and its
   interval excludes zero, at both arities, with ten seeds.
2. The gap is not free money. A selector must learn the family from the
   same observations the strategies use, or from features that cost
   something. The realisable headroom is bounded by the cost of family
   identification, which for a sequential probing strategy is at most
   the gap itself and is likely a fraction of it.
3. M3 therefore proceeds, in this order: (a) a hand-authored heuristic
   selector that probes a short prefix, filters each family's version
   space, and commits to the family with the fewest surviving members;
   (b) learned selectors (tabular, LinUCB, Bayesian strategy regression)
   with the heuristic as the control, trained on the train split, chosen
   on validation, reported on the test split; (c) a cost model that
   charges work units as well as probes, because family filtering is not
   free even when probes are.
4. A learned selector counts as a success only if its gap-closed interval
   excludes zero *and* beats the heuristic. Otherwise the result is
   negative and is recorded as such.

## Consequences

- `docs/architecture/milestones.md` marks M2 reached and M3 open with the
  gate recorded here.
- Selectors are scored as extra columns in the same cost table as the
  fixed strategies, so "gap closed" is on the same scale.
- The affine shortcut's weak verification (greedy probes test the pool,
  not the affine hypothesis) is noted as a strategy-design finding, not a
  laboratory problem; a hypothesis-testing probe is a candidate operator
  for M3's strategy set.
