# Headroom: selectors-arity6 (10 seeds, arity 6)

Tasks: 300. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 128.

**Single best solver:** `greedy-pool` at mean cost 7.977. **Virtual best solver:** mean cost 5.357. **Gap:** 2.620 (95% CI 2.550 to 2.690); VBS/SBS = 0.672.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 64.000 | 64.000 | 64.000 to 64.000 | 100.0% | 64.00 | -21.383 |
| `greedy-pool` | 7.977 | 8.000 | 8.000 to 8.000 | 100.0% | 7.98 | 0.000 |
| `affine-verified-then-greedy` | 30.483 | 10.747 | 10.447 to 11.080 | 82.7% | 10.04 | -8.590 |
| `greedy-affine-only` | 107.547 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -38.004 |
| `greedy-monotone-only` | 107.547 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -38.004 |
| `greedy-read-once-only` | 107.557 | 128.000 | 128.000 to 128.000 | 16.7% | 5.34 | -38.008 |
| `greedy-threshold-only` | 107.563 | 128.000 | 128.000 to 128.000 | 16.7% | 5.38 | -38.010 |
| `greedy-k-term-dnf-only` | 107.573 | 128.000 | 128.000 to 128.000 | 16.7% | 5.44 | -38.014 |
| `greedy-decision-tree-only` | 107.570 | 128.000 | 128.000 to 128.000 | 16.7% | 5.42 | -38.013 |
| `select-most-survivors-k0` | 107.570 | 128.000 | 128.000 to 128.000 | 16.7% | 5.42 | -38.013 |
| `select-most-survivors-k1` | 96.660 | 126.387 | 117.447 to 128.000 | 25.7% | 5.90 | -33.849 |
| `select-most-survivors-k2` | 92.667 | 118.340 | 107.787 to 128.000 | 29.0% | 6.16 | -32.324 |
| `select-most-survivors-k3` | 87.897 | 108.687 | 96.593 to 120.760 | 33.0% | 6.47 | -30.504 |
| `select-most-survivors-k4` | 81.490 | 95.833 | 83.740 to 107.920 | 38.3% | 6.67 | -28.059 |
| `select-most-survivors-k5` | 77.190 | 87.093 | 74.187 to 99.933 | 42.0% | 7.02 | -26.417 |
| `select-most-survivors-k6` | 68.817 | 70.260 | 56.667 to 83.160 | 49.0% | 7.22 | -23.221 |
| `select-fewest-survivors-k0` | 107.547 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -38.004 |
| `select-fewest-survivors-k3` | 120.583 | 128.000 | 128.000 to 128.000 | 6.0% | 4.39 | -42.980 |

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

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | `select-most-survivors-k0` | `select-most-survivors-k1` | `select-most-survivors-k2` | `select-most-survivors-k3` | `select-most-survivors-k4` | `select-most-survivors-k5` | `select-most-survivors-k6` | `select-fewest-survivors-k0` | `select-fewest-survivors-k3` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 64.00 | 7.82 | 7.52 | 5.28 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 120.66 | 108.48 | 108.48 | 108.58 | 96.46 | 89.34 | 5.28 | 118.16 | `greedy-affine-only` | 5.28 |
| decision-tree | 50 | 64.00 | 7.92 | 37.80 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 5.42 | 5.42 | 110.92 | 115.78 | 110.94 | 91.48 | 81.88 | 52.72 | 128.00 | 123.02 | `greedy-decision-tree-only` | 5.42 |
| k-term-dnf | 50 | 64.00 | 7.96 | 40.76 | 128.00 | 128.00 | 128.00 | 128.00 | 5.44 | 128.00 | 128.00 | 42.48 | 74.38 | 79.32 | 76.88 | 72.18 | 69.92 | 128.00 | 128.00 | `greedy-k-term-dnf-only` | 5.44 |
| monotone | 50 | 64.00 | 8.02 | 21.68 | 128.00 | 5.28 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 49.90 | 47.60 | 45.44 | 35.88 | 43.44 | 43.66 | 128.00 | 118.10 | `greedy-monotone-only` | 5.28 |
| read-once | 50 | 64.00 | 7.98 | 36.40 | 128.00 | 128.00 | 5.34 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 110.98 | 91.54 | 98.90 | 96.60 | 82.12 | 128.00 | 120.58 | `greedy-read-once-only` | 5.34 |
| threshold | 50 | 64.00 | 8.16 | 38.74 | 128.00 | 128.00 | 128.00 | 5.38 | 128.00 | 128.00 | 128.00 | 128.00 | 98.78 | 91.66 | 77.22 | 72.58 | 75.14 | 128.00 | 115.64 | `greedy-threshold-only` | 5.38 |


# Headroom: selectors-arity6 (10 seeds, arity 6), cost model `no-penalty-probes-only`

Tasks: 300. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 64.

**Single best solver:** `greedy-pool` at mean cost 7.977. **Virtual best solver:** mean cost 5.357. **Gap:** 2.620 (95% CI 2.550 to 2.690); VBS/SBS = 0.672.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 64.000 | 64.000 | 64.000 to 64.000 | 100.0% | 64.00 | -21.383 |
| `greedy-pool` | 7.977 | 8.000 | 8.000 to 8.000 | 100.0% | 7.98 | 0.000 |
| `affine-verified-then-greedy` | 19.390 | 10.747 | 10.447 to 11.080 | 82.7% | 10.04 | -4.356 |
| `greedy-affine-only` | 54.213 | 64.000 | 64.000 to 64.000 | 16.7% | 5.28 | -17.648 |
| `greedy-monotone-only` | 54.213 | 64.000 | 64.000 to 64.000 | 16.7% | 5.28 | -17.648 |
| `greedy-read-once-only` | 54.223 | 64.000 | 64.000 to 64.000 | 16.7% | 5.34 | -17.651 |
| `greedy-threshold-only` | 54.230 | 64.000 | 64.000 to 64.000 | 16.7% | 5.38 | -17.654 |
| `greedy-k-term-dnf-only` | 54.240 | 64.000 | 64.000 to 64.000 | 16.7% | 5.44 | -17.658 |
| `greedy-decision-tree-only` | 54.237 | 64.000 | 64.000 to 64.000 | 16.7% | 5.42 | -17.656 |
| `select-most-survivors-k0` | 54.237 | 64.000 | 64.000 to 64.000 | 16.7% | 5.42 | -17.656 |
| `select-most-survivors-k1` | 49.087 | 63.240 | 58.993 to 64.000 | 25.7% | 5.90 | -15.691 |
| `select-most-survivors-k2` | 47.227 | 59.460 | 54.453 to 64.000 | 29.0% | 6.16 | -14.981 |
| `select-most-survivors-k3` | 45.017 | 54.927 | 49.233 to 60.600 | 33.0% | 6.47 | -14.137 |
| `select-most-survivors-k4` | 42.023 | 48.900 | 43.207 to 54.587 | 38.3% | 6.67 | -12.995 |
| `select-most-survivors-k5` | 40.070 | 44.853 | 38.773 to 50.867 | 42.0% | 7.02 | -12.249 |
| `select-most-survivors-k6` | 36.177 | 36.980 | 30.640 to 43.053 | 49.0% | 7.22 | -10.763 |
| `select-fewest-survivors-k0` | 54.213 | 64.000 | 64.000 to 64.000 | 16.7% | 5.28 | -17.648 |
| `select-fewest-survivors-k3` | 60.423 | 64.000 | 64.000 to 64.000 | 6.0% | 4.39 | -20.018 |

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

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | `select-most-survivors-k0` | `select-most-survivors-k1` | `select-most-survivors-k2` | `select-most-survivors-k3` | `select-most-survivors-k4` | `select-most-survivors-k5` | `select-most-survivors-k6` | `select-fewest-survivors-k0` | `select-fewest-survivors-k3` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 64.00 | 7.82 | 7.52 | 5.28 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 60.50 | 54.72 | 54.72 | 54.82 | 49.10 | 45.82 | 5.28 | 59.28 | `greedy-affine-only` | 5.28 |
| decision-tree | 50 | 64.00 | 7.92 | 22.44 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 5.42 | 5.42 | 55.88 | 58.18 | 55.90 | 46.68 | 42.20 | 28.40 | 64.00 | 61.58 | `greedy-decision-tree-only` | 5.42 |
| k-term-dnf | 50 | 64.00 | 7.96 | 24.12 | 64.00 | 64.00 | 64.00 | 64.00 | 5.44 | 64.00 | 64.00 | 23.28 | 38.54 | 40.92 | 39.76 | 37.62 | 36.64 | 64.00 | 64.00 | `greedy-k-term-dnf-only` | 5.44 |
| monotone | 50 | 64.00 | 8.02 | 16.56 | 64.00 | 5.28 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 26.86 | 25.84 | 24.96 | 20.52 | 24.24 | 24.46 | 64.00 | 59.22 | `greedy-monotone-only` | 5.28 |
| read-once | 50 | 64.00 | 7.98 | 22.32 | 64.00 | 64.00 | 5.34 | 64.00 | 64.00 | 64.00 | 64.00 | 64.00 | 55.94 | 46.74 | 50.26 | 49.24 | 42.44 | 64.00 | 60.42 | `greedy-read-once-only` | 5.34 |
| threshold | 50 | 64.00 | 8.16 | 23.38 | 64.00 | 64.00 | 64.00 | 5.38 | 64.00 | 64.00 | 64.00 | 64.00 | 50.14 | 46.86 | 40.10 | 38.02 | 39.30 | 64.00 | 58.04 | `greedy-threshold-only` | 5.38 |


# Headroom: selectors-arity6 (10 seeds, arity 6), cost model `probes-plus-work`

Tasks: 300. Cost model: probe weight 1, work weight 0.001, external weight 0, failure cost 128.

**Single best solver:** `affine-verified-then-greedy` at mean cost 43.286. **Virtual best solver:** mean cost 12.356. **Gap:** 30.930 (95% CI 27.030 to 35.207); VBS/SBS = 0.285.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `exhaustive` | 66.272 | 66.272 | 66.272 to 66.272 | 100.0% | 64.00 | -0.743 |
| `greedy-pool` | 53.926 | 53.886 | 53.865 to 53.905 | 100.0% | 7.98 | -0.344 |
| `affine-verified-then-greedy` | 43.286 | 29.736 | 29.023 to 30.535 | 82.7% | 10.04 | 0.000 |
| `greedy-affine-only` | 108.823 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -2.119 |
| `greedy-monotone-only` | 108.820 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -2.119 |
| `greedy-read-once-only` | 108.839 | 128.000 | 128.000 to 128.000 | 16.7% | 5.34 | -2.119 |
| `greedy-threshold-only` | 108.851 | 128.000 | 128.000 to 128.000 | 16.7% | 5.38 | -2.120 |
| `greedy-k-term-dnf-only` | 108.855 | 128.000 | 128.000 to 128.000 | 16.7% | 5.44 | -2.120 |
| `greedy-decision-tree-only` | 108.851 | 128.000 | 128.000 to 128.000 | 16.7% | 5.42 | -2.120 |
| `select-most-survivors-k0` | 108.931 | 128.000 | 128.000 to 128.000 | 16.7% | 5.42 | -2.122 |
| `select-most-survivors-k1` | 102.375 | 126.698 | 119.450 to 128.000 | 25.7% | 5.90 | -1.910 |
| `select-most-survivors-k2` | 100.973 | 120.684 | 112.641 to 128.000 | 29.0% | 6.16 | -1.865 |
| `select-most-survivors-k3` | 98.386 | 113.861 | 104.957 to 122.703 | 33.0% | 6.47 | -1.781 |
| `select-most-survivors-k4` | 94.260 | 104.806 | 96.050 to 113.550 | 38.3% | 6.67 | -1.648 |
| `select-most-survivors-k5` | 91.560 | 98.838 | 89.594 to 108.007 | 42.0% | 7.02 | -1.561 |
| `select-most-survivors-k6` | 85.820 | 87.021 | 77.404 to 96.228 | 49.0% | 7.22 | -1.375 |
| `select-fewest-survivors-k0` | 108.903 | 128.000 | 128.000 to 128.000 | 16.7% | 5.28 | -2.121 |
| `select-fewest-survivors-k3` | 122.430 | 128.000 | 128.000 to 128.000 | 6.0% | 4.39 | -2.559 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `affine-verified-then-greedy` | 52 |
| `greedy-decision-tree-only` | 50 |
| `greedy-k-term-dnf-only` | 49 |
| `greedy-monotone-only` | 49 |
| `greedy-read-once-only` | 50 |
| `greedy-threshold-only` | 50 |

## Per family

| Family | Tasks | `exhaustive` | `greedy-pool` | `affine-verified-then-greedy` | `greedy-affine-only` | `greedy-monotone-only` | `greedy-read-once-only` | `greedy-threshold-only` | `greedy-k-term-dnf-only` | `greedy-decision-tree-only` | `select-most-survivors-k0` | `select-most-survivors-k1` | `select-most-survivors-k2` | `select-most-survivors-k3` | `select-most-survivors-k4` | `select-most-survivors-k5` | `select-most-survivors-k6` | `select-fewest-survivors-k0` | `select-fewest-survivors-k3` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 66.27 | 53.63 | 8.90 | 12.94 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 121.96 | 113.02 | 113.51 | 113.89 | 105.29 | 100.37 | 13.42 | 120.63 | `affine-verified-then-greedy` | 8.90 |
| decision-tree | 50 | 66.27 | 53.88 | 51.04 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 13.10 | 13.58 | 113.99 | 118.61 | 115.37 | 101.44 | 94.83 | 74.15 | 128.00 | 124.25 | `greedy-decision-tree-only` | 13.10 |
| k-term-dnf | 50 | 66.27 | 53.89 | 53.73 | 128.00 | 128.00 | 128.00 | 128.00 | 13.13 | 128.00 | 128.00 | 57.96 | 86.96 | 91.95 | 90.79 | 87.85 | 86.54 | 128.00 | 128.00 | `greedy-k-term-dnf-only` | 13.12 |
| monotone | 50 | 66.27 | 53.90 | 41.47 | 128.00 | 12.92 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 64.33 | 66.64 | 67.19 | 61.21 | 67.37 | 67.93 | 128.00 | 120.56 | `greedy-monotone-only` | 12.87 |
| read-once | 50 | 66.27 | 54.00 | 51.31 | 128.00 | 128.00 | 13.03 | 128.00 | 128.00 | 128.00 | 128.00 | 128.00 | 115.01 | 101.07 | 106.95 | 105.58 | 95.34 | 128.00 | 122.43 | `greedy-read-once-only` | 13.03 |
| threshold | 50 | 66.27 | 54.25 | 53.27 | 128.00 | 128.00 | 128.00 | 13.11 | 128.00 | 128.00 | 128.00 | 128.00 | 105.61 | 101.23 | 91.29 | 88.45 | 90.58 | 128.00 | 118.72 | `greedy-threshold-only` | 13.11 |
