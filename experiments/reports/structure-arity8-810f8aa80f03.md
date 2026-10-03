# Headroom: structure-arity8 (10 seeds, arity 8)

Tasks: 300. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 512.

**Single best solver:** `cascade-junta-symmetric-affine` at mean cost 32.877. **Virtual best solver:** mean cost 16.697. **Gap:** 16.180 (95% CI 12.283 to 21.490); VBS/SBS = 0.508.

**Entropy floor:** 12.100. A strategy that is not told the target's family averages at least this much on correct answers, so a router can save at most 20.777 against the single best solver, 128.4% of the gap.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `table` | 256.000 | 256.000 | 256.000 to 256.000 | 100.0% | 256.00 | -13.790 |
| `cascade-affine-symmetric-junta` | 98.960 | 21.947 | 21.253 to 22.840 | 84.7% | 24.16 | -4.084 |
| `cascade-affine-junta-symmetric` | 100.157 | 25.580 | 24.487 to 26.833 | 84.7% | 25.57 | -4.158 |
| `cascade-symmetric-affine-junta` | 59.203 | 22.960 | 22.800 to 23.407 | 93.3% | 26.86 | -1.627 |
| `cascade-symmetric-junta-affine` | 55.327 | 22.687 | 22.007 to 23.393 | 93.7% | 24.45 | -1.388 |
| `cascade-junta-affine-symmetric` | 38.307 | 31.340 | 30.607 to 31.993 | 98.7% | 31.91 | -0.336 |
| `cascade-junta-symmetric-affine` | 32.877 | 28.633 | 27.707 to 29.560 | 99.3% | 29.66 | 0.000 |
| `affine-then-table` | 214.253 | 215.500 | 215.500 to 215.500 | 84.7% | 160.33 | -11.210 |
| `symmetric-then-table` | 191.213 | 215.500 | 215.500 to 215.500 | 93.7% | 169.52 | -9.786 |
| `junta-then-table` | 151.977 | 163.413 | 147.987 to 177.613 | 99.7% | 150.77 | -7.361 |
| `affine-blind` | 344.333 | 428.167 | 428.167 to 428.167 | 33.3% | 9.00 | -19.249 |
| `symmetric-blind` | 344.333 | 428.167 | 428.167 to 428.167 | 33.3% | 9.00 | -19.249 |
| `junta-blind` | 288.390 | 311.947 | 279.653 to 344.293 | 45.7% | 22.34 | -15.792 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `cascade-affine-junta-symmetric` | 12 |
| `cascade-affine-symmetric-junta` | 102 |
| `cascade-junta-affine-symmetric` | 59 |
| `cascade-symmetric-affine-junta` | 100 |
| `cascade-symmetric-junta-affine` | 27 |

## Per family

| Family | Tasks | `table` | `cascade-affine-symmetric-junta` | `cascade-affine-junta-symmetric` | `cascade-symmetric-affine-junta` | `cascade-symmetric-junta-affine` | `cascade-junta-affine-symmetric` | `cascade-junta-symmetric-affine` | `affine-then-table` | `symmetric-then-table` | `junta-then-table` | `affine-blind` | `symmetric-blind` | `junta-blind` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 100 | 256.00 | 13.00 | 13.00 | 48.31 | 51.16 | 29.07 | 32.54 | 13.00 | 268.80 | 166.34 | 9.00 | 512.00 | 320.94 | `cascade-affine-symmetric-junta` | 13.00 |
| junta | 100 | 256.00 | 184.29 | 181.45 | 116.30 | 101.82 | 33.59 | 33.59 | 332.80 | 291.84 | 31.03 | 512.00 | 512.00 | 32.23 | `junta-then-table` | 24.09 |
| symmetric | 100 | 256.00 | 99.59 | 106.02 | 13.00 | 13.00 | 52.26 | 32.50 | 296.96 | 13.00 | 258.56 | 512.00 | 9.00 | 512.00 | `cascade-symmetric-affine-junta` | 13.00 |
