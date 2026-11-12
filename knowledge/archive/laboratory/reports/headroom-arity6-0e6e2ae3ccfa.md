# Headroom: headroom-arity6 (10 seeds, arity 6)

Tasks: 300. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 128.

**Single best solver:** `greedy-pool` at mean cost 7.977. **Virtual best solver:** mean cost 5.357. **Gap:** 2.620 (95% CI 2.550 to 2.690); VBS/SBS = 0.672.

**Entropy floor:** 7.907. A strategy that is not told the target's family averages at least this much on correct answers, so a router can save at most 0.070 against the single best solver, 2.7% of the gap.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 64.000 | 64.000 | 64.000 to 64.000 | 100.0% | 64.00 | -21.383 |
| `greedy-pool` | 7.977 | 8.000 | 8.000 to 8.000 | 100.0% | 7.98 | 0.000 |
| `affine-blind` | 107.027 | 128.000 | 128.000 to 128.000 | 17.3% | 7.00 | -37.805 |
| `affine-verified-then-greedy` | 30.483 | 10.747 | 10.440 to 11.067 | 82.7% | 10.04 | -8.590 |
| `greedy-affine-only` | 107.547 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -38.004 |
| `greedy-monotone-only` | 107.547 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -38.004 |
| `greedy-read-once-only` | 107.557 | 128.000 | 128.000 to 128.000 | 16.7% | 5.34 | -38.008 |
| `greedy-threshold-only` | 107.563 | 128.000 | 128.000 to 128.000 | 16.7% | 5.38 | -38.010 |
| `greedy-k-term-dnf-only` | 107.573 | 128.000 | 128.000 to 128.000 | 16.7% | 5.44 | -38.014 |
| `greedy-decision-tree-only` | 107.570 | 128.000 | 128.000 to 128.000 | 16.7% | 5.42 | -38.013 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `greedy-affine-only` | 50 |
| `greedy-decision-tree-only` | 50 |
| `greedy-k-term-dnf-only` | 50 |
| `greedy-monotone-only` | 50 |
| `greedy-read-once-only` | 50 |
| `greedy-threshold-only` | 50 |

## Per family

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-blind` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 64.00 | 7.82 | 7.00 | 7.52 | 5.28 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | `greedy-affine-only` | 5.28 |
| decision-tree | 50 | 64.00 | 7.92 | 128.00 | 37.80 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 5.42 | `greedy-decision-tree-only` | 5.42 |
| k-term-dnf | 50 | 64.00 | 7.96 | 125.58 | 40.76 | 128.00 | 128.00 | 128.00 | 128.00 | 5.44 | 128.00 | `greedy-k-term-dnf-only` | 5.44 |
| monotone | 50 | 64.00 | 8.02 | 125.58 | 21.68 | 128.00 | 5.28 | 128.00 | 128.00 | 128.00 | 128.00 | `greedy-monotone-only` | 5.28 |
| read-once | 50 | 64.00 | 7.98 | 128.00 | 36.40 | 128.00 | 128.00 | 5.34 | 128.00 | 128.00 | 128.00 | `greedy-read-once-only` | 5.34 |
| threshold | 50 | 64.00 | 8.16 | 128.00 | 38.74 | 128.00 | 128.00 | 128.00 | 5.38 | 128.00 | 128.00 | `greedy-threshold-only` | 5.38 |
