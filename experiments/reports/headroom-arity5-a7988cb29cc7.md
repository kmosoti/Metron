# Headroom: headroom-arity5 (10 seeds, arity 5)

Tasks: 300. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 64.

**Single best solver:** `greedy-pool` at mean cost 8.033. **Virtual best solver:** mean cost 5.393. **Gap:** 2.640 (95% CI 2.570 to 2.717); VBS/SBS = 0.671.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 32.000 | 32.000 | 32.000 to 32.000 | 100.0% | 32.00 | -9.078 |
| `greedy-pool` | 8.033 | 8.000 | 8.000 to 8.000 | 100.0% | 8.03 | 0.000 |
| `affine-blind` | 53.753 | 64.000 | 64.000 to 64.000 | 17.7% | 6.00 | -17.318 |
| `affine-verified-then-greedy` | 13.327 | 9.253 | 9.013 to 9.487 | 92.3% | 9.12 | -2.005 |
| `greedy-affine-only` | 54.220 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -17.495 |
| `greedy-monotone-only` | 54.233 | 64.000 | 64.000 to 64.000 | 16.7% | 5.40 | -17.500 |
| `greedy-read-once-only` | 54.243 | 64.000 | 64.000 to 64.000 | 16.7% | 5.46 | -17.504 |
| `greedy-threshold-only` | 54.220 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -17.495 |
| `greedy-k-term-dnf-only` | 54.250 | 64.000 | 64.000 to 64.000 | 16.7% | 5.50 | -17.506 |
| `greedy-decision-tree-only` | 54.227 | 64.000 | 64.000 to 64.000 | 16.7% | 5.36 | -17.497 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `affine-blind` | 16 |
| `greedy-affine-only` | 34 |
| `greedy-decision-tree-only` | 50 |
| `greedy-k-term-dnf-only` | 50 |
| `greedy-monotone-only` | 50 |
| `greedy-read-once-only` | 50 |
| `greedy-threshold-only` | 50 |

## Per family

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-blind` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 32.00 | 7.82 | 6.00 | 7.48 | 5.32 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | `greedy-affine-only` | 5.32 |
| decision-tree | 50 | 32.00 | 7.98 | 64.00 | 14.00 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 5.36 | `greedy-decision-tree-only` | 5.36 |
| k-term-dnf | 50 | 32.00 | 8.00 | 62.84 | 12.22 | 64.00 | 64.00 | 64.00 | 64.00 | 5.50 | 64.00 | `greedy-k-term-dnf-only` | 5.50 |
| monotone | 50 | 32.00 | 8.08 | 61.68 | 13.08 | 64.00 | 5.40 | 64.00 | 64.00 | 64.00 | 64.00 | `greedy-monotone-only` | 5.40 |
| read-once | 50 | 32.00 | 8.08 | 64.00 | 14.60 | 64.00 | 64.00 | 5.46 | 64.00 | 64.00 | 64.00 | `greedy-read-once-only` | 5.46 |
| threshold | 50 | 32.00 | 8.24 | 64.00 | 18.58 | 64.00 | 64.00 | 64.00 | 5.32 | 64.00 | 64.00 | `greedy-threshold-only` | 5.32 |
