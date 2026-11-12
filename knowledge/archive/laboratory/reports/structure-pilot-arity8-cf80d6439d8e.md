# Headroom: structure-pilot-arity8 (5 seeds, arity 8)

Tasks: 150. Cost model: probe weight 1, work weight 0, external weight 0, failure cost 512.

**Single best solver:** `cascade-junta-symmetric-affine-v4` at mean cost 32.493. **Virtual best solver:** mean cost 16.460. **Gap:** 16.033 (95% CI 12.020 to 23.240); VBS/SBS = 0.507.

**Entropy floor:** 12.100. A strategy that is not told the target's family averages at least this much on correct answers, so a router can save at most 20.393 against the single best solver, 127.2% of the gap.

| Strategy | Mean cost | IQM cost | IQM 95% CI | Solved | Mean probes (solved) | Gap closed |
|---|---:|---:|---:|---:|---:|---:|
| `table` | 256.000 | 256.000 | 256.000 to 256.000 | 100.0% | 256.00 | -13.940 |
| `cascade-affine-symmetric-junta-v4` | 67.553 | 20.829 | 20.526 to 21.513 | 90.7% | 21.80 | -2.187 |
| `cascade-affine-junta-symmetric-v4` | 73.413 | 23.092 | 21.776 to 24.961 | 90.7% | 28.26 | -2.552 |
| `cascade-symmetric-affine-junta-v4` | 65.487 | 22.750 | 22.500 to 23.237 | 91.3% | 23.12 | -2.058 |
| `cascade-symmetric-junta-affine-v4` | 56.720 | 22.171 | 21.237 to 23.276 | 93.3% | 24.20 | -1.511 |
| `cascade-junta-affine-symmetric-v4` | 34.953 | 32.026 | 30.763 to 33.329 | 99.3% | 31.75 | -0.153 |
| `cascade-junta-symmetric-affine-v4` | 32.493 | 29.000 | 27.474 to 30.513 | 99.3% | 29.28 | 0.000 |
| `cascade-affine-symmetric-junta-v8` | 48.440 | 27.605 | 27.342 to 28.066 | 96.0% | 29.12 | -0.995 |
| `cascade-affine-junta-symmetric-v8` | 53.160 | 30.474 | 29.592 to 31.895 | 95.3% | 30.70 | -1.289 |
| `cascade-symmetric-affine-junta-v8` | 37.093 | 30.197 | 30.026 to 30.382 | 98.7% | 30.68 | -0.287 |
| `cascade-symmetric-junta-affine-v8` | 36.400 | 30.145 | 28.592 to 31.592 | 99.3% | 33.21 | -0.244 |
| `cascade-junta-affine-symmetric-v8` | 39.133 | 39.329 | 37.934 to 40.658 | 100.0% | 39.13 | -0.414 |
| `cascade-junta-symmetric-affine-v8` | 36.167 | 35.697 | 33.921 to 37.408 | 100.0% | 36.17 | -0.229 |
| `cascade-affine-symmetric-junta-v12` | 40.627 | 34.816 | 34.500 to 35.263 | 99.3% | 37.46 | -0.507 |
| `cascade-affine-junta-symmetric-v12` | 43.893 | 38.618 | 37.697 to 39.592 | 99.3% | 40.75 | -0.711 |
| `cascade-symmetric-affine-junta-v12` | 38.660 | 38.211 | 38.026 to 38.395 | 100.0% | 38.66 | -0.385 |
| `cascade-symmetric-junta-affine-v12` | 38.047 | 36.526 | 34.829 to 38.289 | 100.0% | 38.05 | -0.346 |
| `cascade-junta-affine-symmetric-v12` | 46.553 | 46.382 | 44.803 to 47.895 | 100.0% | 46.55 | -0.877 |
| `cascade-junta-symmetric-affine-v12` | 43.013 | 42.171 | 40.329 to 44.000 | 100.0% | 43.01 | -0.656 |
| `cascade-affine-symmetric-junta-v16` | 46.573 | 42.539 | 42.250 to 43.026 | 99.3% | 43.45 | -0.878 |
| `cascade-affine-junta-symmetric-v16` | 49.840 | 46.684 | 45.539 to 47.895 | 99.3% | 46.74 | -1.082 |
| `cascade-symmetric-affine-junta-v16` | 45.640 | 46.105 | 45.947 to 46.263 | 100.0% | 45.64 | -0.820 |
| `cascade-symmetric-junta-affine-v16` | 43.853 | 43.013 | 40.974 to 45.237 | 100.0% | 43.85 | -0.709 |
| `cascade-junta-affine-symmetric-v16` | 53.987 | 53.263 | 51.500 to 55.026 | 100.0% | 53.99 | -1.341 |
| `cascade-junta-symmetric-affine-v16` | 49.873 | 48.434 | 46.474 to 50.513 | 100.0% | 49.87 | -1.084 |

## Per-task best (VBS choices)

| Strategy | Tasks where best |
|---|---:|
| `cascade-affine-junta-symmetric-v4` | 16 |
| `cascade-affine-symmetric-junta-v4` | 51 |
| `cascade-junta-affine-symmetric-v4` | 21 |
| `cascade-symmetric-affine-junta-v4` | 52 |
| `cascade-symmetric-junta-affine-v4` | 8 |
| `cascade-symmetric-junta-affine-v8` | 2 |

## Per family

| Family | Tasks | `table` | `cascade-affine-symmetric-junta-v4` | `cascade-affine-junta-symmetric-v4` | `cascade-symmetric-affine-junta-v4` | `cascade-symmetric-junta-affine-v4` | `cascade-junta-affine-symmetric-v4` | `cascade-junta-symmetric-affine-v4` | `cascade-affine-symmetric-junta-v8` | `cascade-affine-junta-symmetric-v8` | `cascade-symmetric-affine-junta-v8` | `cascade-symmetric-junta-affine-v8` | `cascade-junta-affine-symmetric-v8` | `cascade-junta-symmetric-affine-v8` | `cascade-affine-symmetric-junta-v12` | `cascade-affine-junta-symmetric-v12` | `cascade-symmetric-affine-junta-v12` | `cascade-symmetric-junta-affine-v12` | `cascade-junta-affine-symmetric-v12` | `cascade-junta-symmetric-affine-v12` | `cascade-affine-symmetric-junta-v16` | `cascade-affine-junta-symmetric-v16` | `cascade-symmetric-affine-junta-v16` | `cascade-symmetric-junta-affine-v16` | `cascade-junta-affine-symmetric-v16` | `cascade-junta-symmetric-affine-v16` | Best | VBS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| affine | 50 | 256.00 | 13.00 | 13.00 | 52.90 | 55.86 | 29.70 | 33.04 | 17.00 | 17.00 | 31.40 | 37.06 | 36.02 | 41.62 | 21.00 | 21.00 | 39.40 | 46.98 | 42.38 | 50.24 | 25.00 | 25.00 | 47.40 | 57.28 | 48.76 | 58.90 | `cascade-affine-symmetric-junta-v4` | 13.00 |
| junta | 50 | 256.00 | 148.86 | 158.20 | 130.56 | 101.30 | 36.24 | 36.24 | 90.66 | 84.34 | 62.88 | 55.14 | 30.80 | 30.80 | 56.18 | 51.30 | 55.58 | 46.16 | 34.80 | 34.80 | 62.56 | 54.76 | 64.52 | 49.28 | 38.80 | 38.80 | `cascade-junta-affine-symmetric-v8` | 23.38 |
| symmetric | 50 | 256.00 | 40.80 | 49.04 | 13.00 | 13.00 | 38.92 | 28.20 | 37.66 | 58.14 | 17.00 | 17.00 | 50.58 | 36.08 | 44.70 | 59.38 | 21.00 | 21.00 | 62.48 | 44.00 | 52.16 | 69.76 | 25.00 | 25.00 | 74.40 | 51.92 | `cascade-symmetric-affine-junta-v4` | 13.00 |
