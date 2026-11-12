# Headroom: selectors-arity5 (10 seeds, arity 5)

Tasks: 300. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 64.

**Single best solver:** `greedy-pool` at mean cost 8.033. **Virtual best solver:** mean cost 5.393. **Gap:** 2.640 (95% CI 2.570 to 2.717); VBS/SBS = 0.671.

**Entropy floor:** 7.907. A strategy that is not told the target's family averages at least this much on correct answers, so a router can save at most 0.126 against the single best solver, 4.8% of the gap.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 32.000 | 32.000 | 32.000 to 32.000 | 100.0% | 32.00 | -9.078 |
| `greedy-pool` | 8.033 | 8.000 | 8.000 to 8.000 | 100.0% | 8.03 | 0.000 |
| `affine-verified-then-greedy` | 13.327 | 9.253 | 9.027 to 9.487 | 92.3% | 9.12 | -2.005 |
| `greedy-affine-only` | 54.220 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -17.495 |
| `greedy-monotone-only` | 54.233 | 64.000 | 64.000 to 64.000 | 16.7% | 5.40 | -17.500 |
| `greedy-read-once-only` | 54.243 | 64.000 | 64.000 to 64.000 | 16.7% | 5.46 | -17.504 |
| `greedy-threshold-only` | 54.220 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -17.495 |
| `greedy-k-term-dnf-only` | 54.250 | 64.000 | 64.000 to 64.000 | 16.7% | 5.50 | -17.506 |
| `greedy-decision-tree-only` | 54.227 | 64.000 | 64.000 to 64.000 | 16.7% | 5.36 | -17.497 |
| `select-most-survivors-k0` | 54.227 | 64.000 | 64.000 to 64.000 | 16.7% | 5.36 | -17.497 |
| `select-most-survivors-k1` | 51.203 | 64.000 | 63.233 to 64.000 | 22.0% | 5.83 | -16.352 |
| `select-most-survivors-k2` | 49.150 | 63.267 | 58.360 to 64.000 | 25.7% | 6.14 | -15.574 |
| `select-most-survivors-k3` | 46.503 | 57.927 | 52.600 to 63.247 | 30.3% | 6.32 | -14.572 |
| `select-most-survivors-k4` | 44.843 | 54.540 | 48.860 to 59.880 | 33.3% | 6.53 | -13.943 |
| `select-most-survivors-k5` | 40.553 | 45.847 | 40.127 to 51.560 | 41.0% | 6.81 | -12.318 |
| `select-most-survivors-k6` | 32.673 | 30.047 | 24.367 to 35.720 | 55.0% | 7.04 | -9.333 |
| `select-fewest-survivors-k0` | 54.220 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -17.495 |
| `select-fewest-survivors-k3` | 60.223 | 64.000 | 64.000 to 64.000 | 6.3% | 4.37 | -19.769 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `affine-verified-then-greedy` | 4 |
| `greedy-affine-only` | 46 |
| `greedy-decision-tree-only` | 50 |
| `greedy-k-term-dnf-only` | 50 |
| `greedy-monotone-only` | 50 |
| `greedy-read-once-only` | 50 |
| `greedy-threshold-only` | 50 |

## Per family

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | `select-most-survivors-k0` | `select-most-survivors-k1` | `select-most-survivors-k2` | `select-most-survivors-k3` | `select-most-survivors-k4` | `select-most-survivors-k5` | `select-most-survivors-k6` | `select-fewest-survivors-k0` | `select-fewest-survivors-k3` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 32.00 | 7.82 | 7.48 | 5.32 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 60.52 | 59.34 | 61.70 | 60.54 | 48.04 | 5.32 | 61.66 | `greedy-affine-only` | 5.32 |
| decision-tree | 50 | 32.00 | 7.98 | 14.00 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 5.36 | 5.36 | 62.82 | 55.84 | 51.24 | 45.52 | 39.84 | 21.46 | 64.00 | 60.42 | `greedy-decision-tree-only` | 5.36 |
| k-term-dnf | 50 | 32.00 | 8.00 | 12.22 | 64.00 | 64.00 | 64.00 | 64.00 | 5.50 | 64.00 | 64.00 | 22.10 | 38.44 | 40.86 | 39.76 | 30.76 | 21.78 | 64.00 | 60.40 | `greedy-k-term-dnf-only` | 5.50 |
| monotone | 50 | 32.00 | 8.08 | 13.08 | 64.00 | 5.40 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 44.26 | 28.24 | 23.80 | 22.76 | 24.10 | 23.22 | 64.00 | 62.80 | `greedy-monotone-only` | 5.40 |
| read-once | 50 | 32.00 | 8.08 | 14.60 | 64.00 | 64.00 | 5.46 | 64.00 | 64.00 | 64.00 | 64.00 | 59.36 | 53.62 | 57.10 | 51.32 | 45.68 | 44.62 | 64.00 | 58.04 | `greedy-read-once-only` | 5.46 |
| threshold | 50 | 32.00 | 8.24 | 18.58 | 64.00 | 64.00 | 64.00 | 5.32 | 64.00 | 64.00 | 64.00 | 54.68 | 58.24 | 46.68 | 48.00 | 42.40 | 36.92 | 64.00 | 58.02 | `greedy-threshold-only` | 5.32 |


# Headroom: selectors-arity5 (10 seeds, arity 5), cost model `no-penalty-probes-only`

Tasks: 300. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 32.

**Single best solver:** `greedy-pool` at mean cost 8.033. **Virtual best solver:** mean cost 5.393. **Gap:** 2.640 (95% CI 2.570 to 2.717); VBS/SBS = 0.671.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 32.000 | 32.000 | 32.000 to 32.000 | 100.0% | 32.00 | -9.078 |
| `greedy-pool` | 8.033 | 8.000 | 8.000 to 8.000 | 100.0% | 8.03 | 0.000 |
| `affine-verified-then-greedy` | 10.873 | 9.253 | 9.027 to 9.487 | 92.3% | 9.12 | -1.076 |
| `greedy-affine-only` | 27.553 | 32.000 | 32.000 to 32.000 | 16.7% | 5.32 | -7.394 |
| `greedy-monotone-only` | 27.567 | 32.000 | 32.000 to 32.000 | 16.7% | 5.40 | -7.399 |
| `greedy-read-once-only` | 27.577 | 32.000 | 32.000 to 32.000 | 16.7% | 5.46 | -7.403 |
| `greedy-threshold-only` | 27.553 | 32.000 | 32.000 to 32.000 | 16.7% | 5.32 | -7.394 |
| `greedy-k-term-dnf-only` | 27.583 | 32.000 | 32.000 to 32.000 | 16.7% | 5.50 | -7.405 |
| `greedy-decision-tree-only` | 27.560 | 32.000 | 32.000 to 32.000 | 16.7% | 5.36 | -7.396 |
| `select-most-survivors-k0` | 27.560 | 32.000 | 32.000 to 32.000 | 16.7% | 5.36 | -7.396 |
| `select-most-survivors-k1` | 26.243 | 32.000 | 31.660 to 32.000 | 22.0% | 5.83 | -6.898 |
| `select-most-survivors-k2` | 25.363 | 31.693 | 29.560 to 32.000 | 25.7% | 6.14 | -6.564 |
| `select-most-survivors-k3` | 24.210 | 29.340 | 27.000 to 31.673 | 30.3% | 6.32 | -6.128 |
| `select-most-survivors-k4` | 23.510 | 27.873 | 25.393 to 30.227 | 33.3% | 6.53 | -5.862 |
| `select-most-survivors-k5` | 21.673 | 24.087 | 21.567 to 26.600 | 41.0% | 6.81 | -5.167 |
| `select-most-survivors-k6` | 18.273 | 17.247 | 14.767 to 19.720 | 55.0% | 7.04 | -3.879 |
| `select-fewest-survivors-k0` | 27.553 | 32.000 | 32.000 to 32.000 | 16.7% | 5.32 | -7.394 |
| `select-fewest-survivors-k3` | 30.250 | 32.000 | 32.000 to 32.000 | 6.3% | 4.37 | -8.415 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `affine-verified-then-greedy` | 4 |
| `greedy-affine-only` | 46 |
| `greedy-decision-tree-only` | 50 |
| `greedy-k-term-dnf-only` | 50 |
| `greedy-monotone-only` | 50 |
| `greedy-read-once-only` | 50 |
| `greedy-threshold-only` | 50 |

## Per family

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | `select-most-survivors-k0` | `select-most-survivors-k1` | `select-most-survivors-k2` | `select-most-survivors-k3` | `select-most-survivors-k4` | `select-most-survivors-k5` | `select-most-survivors-k6` | `select-fewest-survivors-k0` | `select-fewest-survivors-k3` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 32.00 | 7.82 | 7.48 | 5.32 | 32.00 | 32.00 | 32.00 | 32.00 | 32.00 | 32.00 | 32.00 | 30.44 | 29.90 | 30.98 | 30.46 | 25.00 | 5.32 | 30.94 | `greedy-affine-only` | 5.32 |
| decision-tree | 50 | 32.00 | 7.98 | 10.80 | 32.00 | 32.00 | 32.00 | 32.00 | 32.00 | 5.36 | 5.36 | 31.46 | 28.32 | 26.28 | 23.76 | 21.28 | 13.14 | 32.00 | 30.34 | `greedy-decision-tree-only` | 5.36 |
| k-term-dnf | 50 | 32.00 | 8.00 | 10.30 | 32.00 | 32.00 | 32.00 | 32.00 | 5.50 | 32.00 | 32.00 | 13.14 | 20.52 | 21.66 | 21.20 | 17.32 | 13.46 | 32.00 | 30.32 | `greedy-k-term-dnf-only` | 5.50 |
| monotone | 50 | 32.00 | 8.08 | 11.80 | 32.00 | 5.40 | 32.00 | 32.00 | 32.00 | 32.00 | 32.00 | 23.14 | 16.08 | 14.20 | 13.80 | 14.50 | 14.26 | 32.00 | 31.44 | `greedy-monotone-only` | 5.40 |
| read-once | 50 | 32.00 | 8.08 | 11.40 | 32.00 | 32.00 | 5.46 | 32.00 | 32.00 | 32.00 | 32.00 | 29.92 | 27.38 | 28.94 | 26.36 | 23.92 | 23.50 | 32.00 | 29.24 | `greedy-read-once-only` | 5.46 |
| threshold | 50 | 32.00 | 8.24 | 13.46 | 32.00 | 32.00 | 32.00 | 5.32 | 32.00 | 32.00 | 32.00 | 27.80 | 29.44 | 24.28 | 24.96 | 22.56 | 20.28 | 32.00 | 29.22 | `greedy-threshold-only` | 5.32 |


# Headroom: selectors-arity5 (10 seeds, arity 5), cost model `probes-plus-work`

Tasks: 300. Cost model: probe weight 1, work weight 0.001, external weight 0, failure cost 64.

**Single best solver:** `affine-verified-then-greedy` at mean cost 20.919. **Virtual best solver:** mean cost 9.096. **Gap:** 11.823 (95% CI 10.542 to 13.205); VBS/SBS = 0.435.

**Entropy floor:** 7.907. A strategy that is not told the target's family averages at least this much on correct answers, so a router can save at most 13.012 against the single best solver, 110.1% of the gap.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 32.624 | 32.624 | 32.624 to 32.624 | 100.0% | 32.00 | -0.990 |
| `greedy-pool` | 30.892 | 30.828 | 30.815 to 30.840 | 100.0% | 8.03 | -0.844 |
| `affine-verified-then-greedy` | 20.919 | 18.640 | 18.274 to 19.027 | 92.3% | 9.12 | 0.000 |
| `greedy-affine-only` | 54.857 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -2.870 |
| `greedy-monotone-only` | 54.870 | 64.000 | 64.000 to 64.000 | 16.7% | 5.40 | -2.872 |
| `greedy-read-once-only` | 54.882 | 64.000 | 64.000 to 64.000 | 16.7% | 5.46 | -2.873 |
| `greedy-threshold-only` | 54.859 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -2.871 |
| `greedy-k-term-dnf-only` | 54.893 | 64.000 | 64.000 to 64.000 | 16.7% | 5.50 | -2.874 |
| `greedy-decision-tree-only` | 54.863 | 64.000 | 64.000 to 64.000 | 16.7% | 5.36 | -2.871 |
| `select-most-survivors-k0` | 54.943 | 64.000 | 64.000 to 64.000 | 16.7% | 5.36 | -2.878 |
| `select-most-survivors-k1` | 53.692 | 64.000 | 63.389 to 64.000 | 22.0% | 5.83 | -2.772 |
| `select-most-survivors-k2` | 52.922 | 63.467 | 59.865 to 64.000 | 25.7% | 6.14 | -2.707 |
| `select-most-survivors-k3` | 51.454 | 59.697 | 55.905 to 63.469 | 30.3% | 6.32 | -2.583 |
| `select-most-survivors-k4` | 50.571 | 57.436 | 53.492 to 61.163 | 33.3% | 6.53 | -2.508 |
| `select-most-survivors-k5` | 47.819 | 51.559 | 47.612 to 55.495 | 41.0% | 6.81 | -2.275 |
| `select-most-survivors-k6` | 42.625 | 40.962 | 37.112 to 44.823 | 55.0% | 7.04 | -1.836 |
| `select-fewest-survivors-k0` | 54.937 | 64.000 | 64.000 to 64.000 | 16.7% | 5.32 | -2.877 |
| `select-fewest-survivors-k3` | 61.230 | 64.000 | 64.000 to 64.000 | 6.3% | 4.37 | -3.410 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `affine-verified-then-greedy` | 31 |
| `greedy-affine-only` | 19 |
| `greedy-decision-tree-only` | 50 |
| `greedy-k-term-dnf-only` | 50 |
| `greedy-monotone-only` | 50 |
| `greedy-read-once-only` | 50 |
| `greedy-threshold-only` | 50 |

## Per family

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | `select-most-survivors-k0` | `select-most-survivors-k1` | `select-most-survivors-k2` | `select-most-survivors-k3` | `select-most-survivors-k4` | `select-most-survivors-k5` | `select-most-survivors-k6` | `select-fewest-survivors-k0` | `select-fewest-survivors-k3` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 32.62 | 30.62 | 8.86 | 9.14 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 61.39 | 60.63 | 62.39 | 61.60 | 53.10 | 9.62 | 62.30 | `affine-verified-then-greedy` | 8.37 |
| decision-tree | 50 | 32.62 | 30.85 | 22.40 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 9.18 | 9.66 | 63.04 | 57.87 | 54.81 | 51.00 | 47.27 | 34.83 | 64.00 | 61.37 | `greedy-decision-tree-only` | 9.18 |
| k-term-dnf | 50 | 32.62 | 30.84 | 20.95 | 64.00 | 64.00 | 64.00 | 64.00 | 9.36 | 64.00 | 64.00 | 30.21 | 44.85 | 47.35 | 46.94 | 41.01 | 35.14 | 64.00 | 61.35 | `greedy-k-term-dnf-only` | 9.36 |
| monotone | 50 | 32.62 | 30.90 | 23.04 | 64.00 | 9.22 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 48.15 | 37.45 | 35.29 | 35.16 | 36.50 | 36.23 | 64.00 | 63.12 | `greedy-monotone-only` | 9.22 |
| read-once | 50 | 32.62 | 30.95 | 23.22 | 64.00 | 64.00 | 9.29 | 64.00 | 64.00 | 64.00 | 64.00 | 60.26 | 56.25 | 59.07 | 55.09 | 51.34 | 50.76 | 64.00 | 59.63 | `greedy-read-once-only` | 9.29 |
| threshold | 50 | 32.62 | 31.19 | 27.05 | 64.00 | 64.00 | 64.00 | 9.15 | 64.00 | 64.00 | 64.00 | 56.49 | 59.71 | 51.58 | 52.85 | 49.19 | 45.70 | 64.00 | 59.61 | `greedy-threshold-only` | 9.15 |
