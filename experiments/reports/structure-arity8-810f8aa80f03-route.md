# Routing: structure-arity8 (10 seeds, arity 8)

Tasks: 147 train, 37 validation, 116 test; splits by NPN-class hash, pooled hygiene verified. Cost model: probe weight 1, failure cost 512. Routers fitted on train, chosen on validation, judged once on test. The control and the selected router were replayed as real schedulers on 232 test episodes; every choice and score matched.

## Verdicts (primary prefix, pre-registered in ADR 0015)

- Routing pays: no; no router's gap-closed interval on test excludes zero.
- Learning pays: no; no learned router beats the control with an interval on the paired difference that excludes zero.
- Prediction 3 (the control closes at least a third of the gap): fails; it closes 0.179 (95% CI -0.726 to 0.710).
- Prediction 4 (no learned router beats the control): holds.

## Prefix `anchors`

Single best fixed strategy, chosen on train: `cascade-junta-symmetric-affine` (train 29.646, validation 27.865, test 38.569). Virtual best on test over the fixed strategies: 17.414. Gap on test: 21.155. Best any router with this prefix could do on test: 19.172. Entropy floor: 12.100.

| Router | Learned | Train | Validation | Test | Solved (test) | Gap closed (test) | 95% CI | Test minus control | 95% CI |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `posterior-order` (selected) | no | 40.320 | 21.595 | 34.784 | 98.3% | 0.179 | -0.726 to 0.710 | n/a | n/a |
| `posterior-reverse` | no | 39.599 | 54.892 | 53.103 | 95.7% | -0.687 | -2.050 to 0.165 | 18.319 | -3.793 to 40.190 |
| `tabular` | yes | 24.592 | 21.973 | 29.948 | 100.0% | 0.407 | -0.165 to 0.776 | -4.836 | -18.207 to 5.379 |
| `ridge(λ = 0.01)` | yes | 26.605 | 25.649 | 30.552 | 100.0% | 0.379 | -0.080 to 0.650 | -4.233 | -18.509 to 5.974 |

### Test cost by class

| Router | affine | junta | symmetric |
|---|---:|---:|---:|
| `posterior-order` | 13.00 | 70.17 | 20.00 |
| `posterior-reverse` | 28.00 | 63.12 | 107.83 |
| `tabular` | 13.00 | 56.64 | 20.39 |
| `ridge(λ = 0.01)` | 17.00 | 51.17 | 24.61 |

### Test choices (class: candidate × tasks)

- `posterior-order`: affine: `cascade-affine-symmetric-junta` × 56; junta: `cascade-affine-junta-symmetric` × 3, `cascade-junta-affine-symmetric` × 31, `cascade-symmetric-junta-affine` × 8; symmetric: `cascade-symmetric-affine-junta` × 13, `cascade-symmetric-junta-affine` × 5
- `posterior-reverse`: affine: `cascade-symmetric-junta-affine` × 56; junta: `cascade-affine-junta-symmetric` × 8, `cascade-symmetric-affine-junta` × 31, `cascade-symmetric-junta-affine` × 3; symmetric: `cascade-affine-junta-symmetric` × 5, `cascade-junta-affine-symmetric` × 13
- `tabular`: affine: `cascade-affine-symmetric-junta` × 56; junta: `cascade-affine-symmetric-junta` × 8, `cascade-junta-affine-symmetric` × 34; symmetric: `cascade-affine-symmetric-junta` × 5, `cascade-symmetric-affine-junta` × 13
- `ridge(λ = 0.01)`: affine: `cascade-junta-affine-symmetric` × 56; junta: `cascade-junta-affine-symmetric` × 34, `cascade-junta-symmetric-affine` × 8; symmetric: `cascade-junta-symmetric-affine` × 18

# Secondary prefixes

## Prefix `anchors-and-weights`

Single best fixed strategy, chosen on train: `cascade-junta-symmetric-affine` (train 29.646, validation 27.865, test 38.569). Virtual best on test over the fixed strategies: 17.414. Gap on test: 21.155. Best any router with this prefix could do on test: 23.466. Entropy floor: 12.100.

| Router | Learned | Train | Validation | Test | Solved (test) | Gap closed (test) | 95% CI | Test minus control | 95% CI |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `posterior-order` (selected) | no | 36.565 | 22.486 | 27.940 | 100.0% | 0.502 | 0.091 to 0.737 | n/a | n/a |
| `posterior-reverse` | no | 35.755 | 29.189 | 48.828 | 97.4% | -0.485 | -1.320 to 0.074 | 20.888 | 7.569 to 36.647 |
| `tabular` | yes | 24.878 | 23.784 | 32.345 | 99.1% | 0.294 | -0.421 to 0.671 | 4.405 | -0.172 to 12.759 |
| `ridge(λ = 0.01)` | yes | 29.755 | 26.054 | 30.595 | 100.0% | 0.377 | -0.069 to 0.643 | 2.655 | 2.517 to 2.828 |

### Test cost by class

| Router | affine | junta | symmetric |
|---|---:|---:|---:|
| `posterior-order` | 20.00 | 41.93 | 20.00 |
| `posterior-reverse` | 28.00 | 85.24 | 28.67 |
| `tabular` | 20.00 | 53.43 | 21.56 |
| `ridge(λ = 0.01)` | 24.00 | 41.83 | 24.89 |

### Test choices (class: candidate × tasks)

- `posterior-order`: affine: `cascade-affine-symmetric-junta` × 56; junta: `cascade-junta-affine-symmetric` × 36, `cascade-junta-symmetric-affine` × 5, `cascade-symmetric-junta-affine` × 1; symmetric: `cascade-symmetric-affine-junta` × 15, `cascade-symmetric-junta-affine` × 3
- `posterior-reverse`: affine: `cascade-symmetric-junta-affine` × 56; junta: `cascade-affine-junta-symmetric` × 1, `cascade-affine-symmetric-junta` × 35, `cascade-symmetric-affine-junta` × 6; symmetric: `cascade-affine-junta-symmetric` × 18
- `tabular`: affine: `cascade-affine-symmetric-junta` × 56; junta: `cascade-junta-affine-symmetric` × 30, `cascade-junta-symmetric-affine` × 4, `cascade-symmetric-junta-affine` × 8; symmetric: `cascade-junta-symmetric-affine` × 3, `cascade-symmetric-affine-junta` × 15
- `ridge(λ = 0.01)`: affine: `cascade-junta-affine-symmetric` × 56; junta: `cascade-junta-affine-symmetric` × 36, `cascade-junta-symmetric-affine` × 6; symmetric: `cascade-affine-symmetric-junta` × 15, `cascade-junta-symmetric-affine` × 3

