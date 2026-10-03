# Routing: structure-ordering-arity8 (30 seeds, arity 8)

Tasks: 420 train, 132 validation, 348 test; splits by NPN-class hash, pooled hygiene verified. Cost model: probe weight 1, failure cost 512. Routers fitted on train, chosen on validation, judged once on test. The control and the selected router were replayed as real schedulers on 696 test episodes; every choice and score matched.

## Verdicts (primary prefix)

- Primary comparison (pre-registered in ADR 0017): the control's test cost minus that of the best fixed order after the same prefix (`cascade-junta-symmetric-affine`, chosen on train) is -3.388 (95% CI -4.839 to -0.586). The ordering pays: the interval lies below zero.
- Routing pays: yes; the gap-closed interval on test excludes zero for `posterior-order`, `tabular`, `ridge(λ = 0.01)`.
- Learning pays: no; no learned router beats the control with an interval on the paired difference that excludes zero.
- Prediction 3 (the control closes at least a third of the gap): holds; it closes 0.467 (95% CI 0.214 to 0.645).
- Prediction 4 (no learned router beats the control): holds.

## Prefix `anchors-and-weights`

Single best fixed strategy, chosen on train: `cascade-junta-symmetric-affine` (train 31.462, validation 36.432, test 40.506). Virtual best on test over the fixed strategies: 17.072. Gap on test: 23.434. Best any router with this prefix could do on test: 23.138. Entropy floor: 12.100.

| Router | Learned | Train | Validation | Test | Solved (test) | Gap closed (test) | 95% CI | Test minus control | 95% CI |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `posterior-order` (selected) | no | 42.431 | 23.841 | 29.555 | 99.7% | 0.467 | 0.214 to 0.645 | n/a | n/a |
| `posterior-reverse` | no | 33.452 | 32.886 | 39.851 | 98.6% | 0.028 | -0.357 to 0.314 | 10.296 | 3.055 to 17.920 |
| `tabular` | yes | 26.005 | 27.205 | 30.411 | 98.9% | 0.431 | 0.122 to 0.648 | 0.856 | -3.836 to 6.109 |
| `ridge(λ = 0.01)` | yes | 26.440 | 23.841 | 31.434 | 99.7% | 0.387 | 0.146 to 0.579 | 1.879 | 1.644 to 2.034 |

**Pre-registered primary comparison (ADR 0017).** The best fixed order run after the same prefix, chosen on train, is `cascade-junta-symmetric-affine` (train 28.245, test 32.943). Each router's test cost minus it:

| Router | Test minus best fixed order after the prefix | 95% CI |
|---|---:|---:|
| `posterior-order` | -3.388 | -4.839 to -0.586 |
| `posterior-reverse` | 6.908 | 0.328 to 13.966 |
| `tabular` | -2.532 | -8.115 to 3.845 |
| `ridge(λ = 0.01)` | -1.509 | -3.052 to 1.322 |

### Test cost by class

| Router | affine | junta | symmetric |
|---|---:|---:|---:|
| `posterior-order` | 20.00 | 48.95 | 20.93 |
| `posterior-reverse` | 28.00 | 64.47 | 28.05 |
| `tabular` | 20.00 | 52.06 | 20.00 |
| `ridge(λ = 0.01)` | 24.00 | 48.95 | 20.00 |

### Test choices (class: candidate × tasks)

- `posterior-order`: affine: `cascade-affine-symmetric-junta` × 177; junta: `cascade-junta-affine-symmetric` × 100, `cascade-junta-symmetric-affine` × 9, `cascade-symmetric-junta-affine` × 4; symmetric: `cascade-junta-symmetric-affine` × 2, `cascade-symmetric-affine-junta` × 54, `cascade-symmetric-junta-affine` × 2
- `posterior-reverse`: affine: `cascade-symmetric-junta-affine` × 177; junta: `cascade-affine-junta-symmetric` × 4, `cascade-affine-symmetric-junta` × 92, `cascade-symmetric-affine-junta` × 17; symmetric: `cascade-affine-junta-symmetric` × 56, `cascade-affine-symmetric-junta` × 2
- `tabular`: affine: `cascade-affine-symmetric-junta` × 177; junta: `cascade-junta-affine-symmetric` × 83, `cascade-symmetric-junta-affine` × 30; symmetric: `cascade-symmetric-affine-junta` × 54, `cascade-symmetric-junta-affine` × 4
- `ridge(λ = 0.01)`: affine: `cascade-junta-affine-symmetric` × 177; junta: `cascade-junta-affine-symmetric` × 17, `cascade-junta-symmetric-affine` × 92, `cascade-symmetric-junta-affine` × 4; symmetric: `cascade-symmetric-affine-junta` × 54, `cascade-symmetric-junta-affine` × 4

