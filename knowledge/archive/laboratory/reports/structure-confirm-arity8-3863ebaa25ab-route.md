# Routing: structure-confirm-arity8 (10 seeds, arity 8)

Tasks: 150 train, 33 validation, 117 test; splits by NPN-class hash, pooled hygiene verified. Cost model: probe weight 1, failure cost 512. Routers fitted on train, chosen on validation, judged once on test. The control and the selected router were replayed as real schedulers on 234 test episodes; every choice and score matched.

## Verdicts (primary prefix)

- Routing pays: yes; the gap-closed interval on test excludes zero for `posterior-order`, `tabular`.
- Learning pays: no; no learned router beats the control with an interval on the paired difference that excludes zero.
- Prediction 3 (the control closes at least a third of the gap): holds; it closes 0.547 (95% CI 0.232 to 0.754).
- Post hoc, not pre-registered (ADR 0017): against the best fixed order after the same prefix (`cascade-symmetric-junta-affine`, chosen on train), the control's test cost differs by -11.026 (95% CI -28.436 to 3.308).
- Prediction 4 (no learned router beats the control): holds.

## Prefix `anchors-and-weights`

Single best fixed strategy, chosen on train: `cascade-junta-symmetric-affine` (train 33.073, validation 56.424, test 49.786). Virtual best on test over the fixed strategies: 17.436. Gap on test: 32.350. Best any router with this prefix could do on test: 23.718. Entropy floor: 12.100.

| Router | Learned | Train | Validation | Test | Solved (test) | Gap closed (test) | 95% CI | Test minus control | 95% CI |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `posterior-order` (selected) | no | 42.160 | 22.727 | 32.094 | 100.0% | 0.547 | 0.232 to 0.754 | n/a | n/a |
| `posterior-reverse` | no | 32.720 | 29.303 | 39.265 | 99.1% | 0.325 | -0.164 to 0.591 | 7.171 | -2.880 to 18.667 |
| `tabular` | yes | 24.993 | 24.667 | 33.983 | 99.1% | 0.489 | 0.071 to 0.727 | 1.889 | -7.761 to 13.470 |
| `ridge(λ = 0.01)` | yes | 26.740 | 23.788 | 43.846 | 97.4% | 0.184 | -0.508 to 0.587 | 11.752 | -2.667 to 27.453 |

**Post hoc (ADR 0017, not pre-registered).** The best fixed order run after the same prefix, chosen on train, is `cascade-symmetric-junta-affine` (train 29.820, test 43.120). Each router's test cost minus it:

| Router | Test minus best fixed order after the prefix | 95% CI |
|---|---:|---:|
| `posterior-order` | -11.026 | -28.436 to 3.308 |
| `posterior-reverse` | -3.855 | -20.949 to 12.171 |
| `tabular` | -9.137 | -23.299 to 1.829 |
| `ridge(λ = 0.01)` | 0.726 | -12.923 to 14.325 |

### Test cost by class

| Router | affine | junta | symmetric |
|---|---:|---:|---:|
| `posterior-order` | 20.00 | 55.56 | 21.33 |
| `posterior-reverse` | 28.00 | 61.64 | 28.29 |
| `tabular` | 20.00 | 61.13 | 21.52 |
| `ridge(λ = 0.01)` | 24.00 | 85.69 | 20.00 |

### Test choices (class: candidate × tasks)

- `posterior-order`: affine: `cascade-affine-symmetric-junta` × 57; junta: `cascade-junta-affine-symmetric` × 34, `cascade-junta-symmetric-affine` × 5; symmetric: `cascade-junta-symmetric-affine` × 1, `cascade-symmetric-affine-junta` × 19, `cascade-symmetric-junta-affine` × 1
- `posterior-reverse`: affine: `cascade-symmetric-junta-affine` × 57; junta: `cascade-affine-symmetric-junta` × 34, `cascade-symmetric-affine-junta` × 5; symmetric: `cascade-affine-junta-symmetric` × 20, `cascade-affine-symmetric-junta` × 1
- `tabular`: affine: `cascade-affine-symmetric-junta` × 57; junta: `cascade-affine-symmetric-junta` × 29, `cascade-junta-affine-symmetric` × 1, `cascade-symmetric-junta-affine` × 9; symmetric: `cascade-junta-affine-symmetric` × 1, `cascade-symmetric-affine-junta` × 19, `cascade-symmetric-junta-affine` × 1
- `ridge(λ = 0.01)`: affine: `cascade-junta-affine-symmetric` × 57; junta: `cascade-symmetric-affine-junta` × 39; symmetric: `cascade-symmetric-affine-junta` × 2, `cascade-symmetric-junta-affine` × 19

# Secondary prefixes

## Prefix `anchors`

Single best fixed strategy, chosen on train: `cascade-junta-symmetric-affine` (train 33.073, validation 56.424, test 49.786). Virtual best on test over the fixed strategies: 17.436. Gap on test: 32.350. Best any router with this prefix could do on test: 19.350. Entropy floor: 12.100.

| Router | Learned | Train | Validation | Test | Solved (test) | Gap closed (test) | 95% CI | Test minus control | 95% CI |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `posterior-order` (selected) | no | 35.113 | 22.667 | 39.376 | 96.6% | 0.322 | -0.456 to 0.775 | n/a | n/a |
| `posterior-reverse` | no | 32.853 | 28.273 | 45.239 | 97.4% | 0.141 | -0.584 to 0.574 | 5.863 | -13.684 to 24.821 |
| `tabular` | yes | 23.653 | 23.667 | 36.308 | 97.4% | 0.417 | -0.251 to 0.800 | -3.068 | -19.128 to 11.453 |
| `ridge(λ = 0.01)` | yes | 23.947 | 23.455 | 39.957 | 97.4% | 0.304 | -0.385 to 0.689 | 0.581 | -15.692 to 15.513 |

**Post hoc (ADR 0017, not pre-registered).** The best fixed order run after the same prefix, chosen on train, is `cascade-symmetric-junta-affine` (train 29.820, test 43.120). Each router's test cost minus it:

| Router | Test minus best fixed order after the prefix | 95% CI |
|---|---:|---:|
| `posterior-order` | -3.744 | -18.308 to 12.419 |
| `posterior-reverse` | 2.120 | -14.496 to 18.359 |
| `tabular` | -6.812 | -19.000 to 5.436 |
| `ridge(λ = 0.01)` | -3.162 | -15.812 to 10.256 |

### Test cost by class

| Router | affine | junta | symmetric |
|---|---:|---:|---:|
| `posterior-order` | 13.00 | 88.36 | 20.00 |
| `posterior-reverse` | 28.00 | 67.28 | 51.10 |
| `tabular` | 13.00 | 79.15 | 20.00 |
| `ridge(λ = 0.01)` | 17.00 | 84.26 | 20.00 |

### Test choices (class: candidate × tasks)

- `posterior-order`: affine: `cascade-affine-symmetric-junta` × 57; junta: `cascade-affine-junta-symmetric` × 5, `cascade-junta-affine-symmetric` × 26, `cascade-symmetric-junta-affine` × 8; symmetric: `cascade-symmetric-affine-junta` × 10, `cascade-symmetric-junta-affine` × 11
- `posterior-reverse`: affine: `cascade-symmetric-junta-affine` × 57; junta: `cascade-affine-junta-symmetric` × 8, `cascade-symmetric-affine-junta` × 26, `cascade-symmetric-junta-affine` × 5; symmetric: `cascade-affine-junta-symmetric` × 11, `cascade-junta-affine-symmetric` × 10
- `tabular`: affine: `cascade-affine-symmetric-junta` × 57; junta: `cascade-junta-affine-symmetric` × 5, `cascade-symmetric-affine-junta` × 26, `cascade-symmetric-junta-affine` × 8; symmetric: `cascade-symmetric-affine-junta` × 10, `cascade-symmetric-junta-affine` × 11
- `ridge(λ = 0.01)`: affine: `cascade-junta-affine-symmetric` × 57; junta: `cascade-junta-affine-symmetric` × 5, `cascade-symmetric-affine-junta` × 34; symmetric: `cascade-symmetric-affine-junta` × 11, `cascade-symmetric-junta-affine` × 10

