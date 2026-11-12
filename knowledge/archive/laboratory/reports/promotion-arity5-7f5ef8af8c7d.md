# Promotion: promotion-arity5 (arity 5, 14 train / 10 test tasks)

## Strategy `exhaustive`

Gate fingerprint `fbcd54ba0fe5`, manifest `7f5ef8af8c7d`.

| Candidate | Train support | Reuse (held-out) | Held-out solved | Probes (solved) | Reference solved | Reference probes (solved) | Decision |
|---|---:|---:|---:|---:|---:|---:|---|
| `probe-next-unobserved -> observations-to-partial-table -> complete-table-by-default -> commit-truth-table` | 14/14 | 100% | 100% | 32.00 | 100% | 32.00 | promoted |

`probe-next-unobserved -> observations-to-partial-table -> complete-table-by-default -> commit-truth-table` claims [observations] -> truth-table/complete (approximate: unobserved rows are filled with a default, not inferred)
- assumes unobserved rows evaluate to false

## Strategy `greedy-pool`

Gate fingerprint `fbcd54ba0fe5`, manifest `7f5ef8af8c7d`.

| Candidate | Train support | Reuse (held-out) | Held-out solved | Probes (solved) | Reference solved | Reference probes (solved) | Decision |
|---|---:|---:|---:|---:|---:|---:|---|
| `greedy-split-probe -> version-space-filter -> single-survivor-to-table -> commit-truth-table` | 14/14 | 100% | 100% | 7.70 | 100% | 7.70 | promoted |

`greedy-split-probe -> version-space-filter -> single-survivor-to-table -> commit-truth-table` claims [observations] -> truth-table/complete (exact)
- assumes the hidden function is a member of the published pool

## Strategy `affine-blind`

Gate fingerprint `fbcd54ba0fe5`, manifest `7f5ef8af8c7d`.

| Candidate | Train support | Reuse (held-out) | Held-out solved | Probes (solved) | Reference solved | Reference probes (solved) | Decision |
|---|---:|---:|---:|---:|---:|---:|---|
| `affine-probe -> affine-solve -> commit-truth-table` | 2/14 | 100% | 30% | 6.00 | 30% | 6.00 | rejected: held-out pass rate below threshold |

`affine-probe -> affine-solve -> commit-truth-table` claims [observations] -> truth-table/complete (approximate: rows other than the observed ones are extrapolated from the affine hypothesis)
- assumes the hidden function is affine over GF(2)

## Strategy `affine-verified-then-greedy`

Gate fingerprint `fbcd54ba0fe5`, manifest `7f5ef8af8c7d`.

| Candidate | Train support | Reuse (held-out) | Held-out solved | Probes (solved) | Reference solved | Reference probes (solved) | Decision |
|---|---:|---:|---:|---:|---:|---:|---|
| `greedy-split-probe -> version-space-filter -> single-survivor-to-table -> commit-truth-table` | 12/14 | 50% | 100% | 8.50 | 80% | 8.00 | promoted |
| `greedy-split-probe -> affine-solve -> commit-truth-table` | 2/14 | 0% | 30% | 6.00 | 80% | 8.00 | rejected: held-out pass rate below threshold |

`greedy-split-probe -> version-space-filter -> single-survivor-to-table -> commit-truth-table` claims [observations] -> truth-table/complete (exact)
- assumes the hidden function is a member of the published pool

`greedy-split-probe -> affine-solve -> commit-truth-table` claims [observations] -> truth-table/complete (approximate: rows other than the observed ones are extrapolated from the affine hypothesis)
- assumes the hidden function is affine over GF(2)

