# ADR 0013: Hyperdimensional retrieval is dropped for the laboratory's native objects

Status: accepted. Date: 2026-10-02.

## Context

Milestone M5 asks whether hyperdimensional codes earn a place as the
cross-episode retrieval mechanism, against exact and conventional
baselines, measured as recall per byte and per nanosecond. The prior-art
review predicted the answer for the laboratory's native objects: a truth
table on six inputs is 64 bits, and an 8,192-bit hypervector is 128 times
larger than the thing it sketches.

## What was measured

`metron retrieval experiments/manifests/retrieval-arity6.json`, report
`experiments/reports/retrieval-arity6-e59c328070b5.md`: a store of 5,126 distinct
functions on six inputs from all six families; 200 queries per setting;
queries of 4, 8, 16 and 32 observed rows of a stored target; the answer is
every stored function consistent with the observations.

| Method | Bytes per entry | Recall at answer size (4 / 8 / 16 / 32 rows) | Microseconds per query |
|---|---:|---|---:|
| exact scan | 8 | 1.000 everywhere | 80 to 100 |
| bitmask index | 16 | 1.000 everywhere | 10 to 21 |
| Bloom filter per table (512 bits) | 64 | 0.91 / 0.90 / 0.92 / 0.98 | 155 to 253 |
| HDC bundle, 1,024 bits | 128 | 0.69 / 0.53 / 0.47 / 0.80 | 300 to 530 |
| HDC bundle, 4,096 bits | 512 | 0.95 / 0.86 / 0.86 / 0.96 | 700 to 1,560 |
| HDC bundle, 8,192 bits | 1,024 | 0.99 / 0.97 / 0.96 / 0.99 | 1,300 to 3,240 |

The bitmask index dominates on every axis. Hyperdimensional codes reach
the exact methods' recall only at 128 times their memory and 100 times
their query time, and their recall falls as the answer set gets smaller,
which is the regime that matters.

## Decision

1. Hyperdimensional retrieval is dropped for the laboratory's native
   objects. No HDC memory crate is built; the measurement code stays in
   `metron-lab::retrieval` as the standing comparison.
2. Cross-episode retrieval, when an experiment needs it, uses the bitmask
   index (one bitset over the store per (row, value), ANDed across the
   observations).
3. HDC may be reconsidered only for objects that are not already compact
   bit vectors, with the same four-way comparison re-run first.

## Consequences

- M5 is reached with a negative result for HDC, as the milestone allows.
- The proposal's plan to make hyperdimensional memory foundational is
  closed; ADR 0006's last deferred item is resolved.
- The retrieval workload is reproducible from the manifest; timings are
  the one measured quantity in the repository that is wall-clock, and
  they are laboratory measurements, never inputs to a decision the system
  makes.
