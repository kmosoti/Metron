---
id: R-0010
type: research
status: active
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Native LFM3 inference evidence and comparison

This record extends [R-0009](R-0009-mistralrs-batching-and-cache-contracts.md)
with execution evidence and the operational comparison in
[ADR 0021](../../docs/adr/0021-local-inference.md). It does not confer production
qualification.

## Observed

The pinned native mistral.rs revision
[`fb4cdc3730dc64b366724e7637a205a8b7747499`](https://github.com/EricLBuehler/mistral.rs/tree/fb4cdc3730dc64b366724e7637a205a8b7747499)
contains LFM3 support; the published 0.9.5 release does not, as documented in
R-0009. One Q6_K generation completed on an NVIDIA RTX 3060 Laptop GPU. The
worker reported a CUDA device, NVIDIA monitoring attributed 98% SM activity to
its process, and the observed process peak was 1,558 MiB. The request to sum
three integers produced `{"count":3,"sum":22}`. This single result establishes
that this configuration executed a generation on the GPU; it does not establish
latency, throughput, broader correctness, or a selectable configuration.

A CUDA 12.9 compiler invocation for one SDK kernel crashed with a `cicc`
segmentation fault without a recorded cgroup OOM. A retry of that translation
unit with `--Ofast-compile=max` succeeded in the observed environment. The successful first GPU worker used CUDA 12.9.86, G++ 14.3, compute capability
8.6 and `NVCC_APPEND_FLAGS=--Ofast-compile=max` across its CUDA translation
units. Its executable SHA-256 is
`d7e912e4fdf20f057d48fc78ec8fc915eb4c233aaafafc2891206ffb23fa4a70`.
This is a local compiler workaround, not a universal requirement; the flag
changes compiler optimization work and may affect generated-kernel performance.
Compare builds only with their flags and binary identities recorded.

## Comparison and uncertainty

The fixed comparison evaluates official Q4_K_M, Q6_K and Q8_0 artifacts at
concurrency 1, 2, 4 and 8 with short and near-8K rendered inputs and three
repetitions. It checks answer and proposal quality, usable-output p95, and GPU
memory, with an 8,192-token context and reserved output. The accepted thresholds
are in ADR 0021. Staggered overlap uses a worker-owned snapshot of SDK streams
whose terminal choices have not yet been consumed; buffered host replies cannot
establish that fact. This does not measure simultaneous GPU execution. A measured failure to meet them can leave no selectable
candidate. The Q6 proof above is not a substitute for those measurements.

**Relations:** extends R-0009; supports ADR 0021; does not supersede the frozen
production qualification. Revisit after the fixed comparison, on a model/backend
or compiler change, or when new measurements contradict the recorded feasibility.
