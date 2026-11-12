---
id: O-0017
type: observation
status: observed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# Paged and unpaged resource diagnostic

Four fresh-process diagnostics compared the pinned LFM3-1.7B Q4_K_M CUDA
worker's paged and unpaged prefill routes at 128 and 513 SDK-rendered tokens.
All used one sequence, BF16 activations/KV, the same prompt and token IDs within
each pair, a matched identity logits callback with neutral penalties, and
prefix cache and CUDA graphs disabled. They ran on an RTX 3060 Laptop GPU
(6,442,450,944 reported bytes) with the pinned SDK and exact source, model,
profile, helper and environment pins recorded in
[`pins.json`](evidence/O-0017-paged-unpaged-resource-diagnostic.json).
The controls are scoped numerical and resource diagnostics, not qualification.

| Pair | Exact input identity | Logits | Source-derived paged chunk plan | Resource observations |
| --- | --- | --- | --- | --- |
| 128 tokens | Rendered prompt SHA-256 `c4cfc839df2e7d5785c5cfade5fcef79377d83af296d07df2d270cf42bca0d85`; token-ID SHA-256 `5e21ecba8088467b2cff845718776fc5e0fa38a61e33f229f737be43256a88fc` | 128,000 logits; paged and unpaged bitwise identical; max absolute and relative RMS error 0; top token 2083 in both | `[128]` | Paged pool payload 67,043,328 bytes, 93 blocks, 2,944 usable tokens against a 67,108,864-byte planning target. Unpaged callback logical KV payload 2,883,584 bytes. Sampled process-GPU maxima: 1,306,525,696 bytes unpaged and 1,373,634,560 bytes paged. |
| 513 tokens | Rendered prompt SHA-256 `37f76efd1688b053393d982cc0a900e76cbb8b83a3e3fb91dfb60e7440f714da`; token-ID SHA-256 `5c105d3532034392ce5cec434cbc84d45a399ef7afb37287e799473bcd5e5474` | 128,000 logits; max absolute difference 0.8125, RMS difference 0.16086321727, relative RMS 0.04115282540 (4.115%); same top token 2083 | `[256, 256, 1]` | Paged pool payload 67,043,328 bytes, 93 blocks, 2,944 usable tokens against a 67,108,864-byte planning target. Unpaged callback logical KV payload 8,925,184 bytes. Sampled process-GPU maxima: 1,407,188,992 bytes for each route. |

The relative RMS definition is `sqrt(sum((unpaged-paged)^2) / sum(unpaged^2))`.
The 513-token difference is material in this measurement, but no defensible
full-model BF16 tolerance was established; therefore it is diagnostic only and
does not by itself show a defect or qualify the route. All 128,000 logits in
each output were finite. The SDK does not expose a direct per-forward chunk
participation trace. The `[128]` and `[256, 256, 1]` plans are derived from the
pinned SDK source and the asserted request/metadata predicates, not directly
traced events.

The actual paged pool tensor payload and the profile's KV planning target are
separate quantities. The unpaged callback's logical KV payload is neither an
allocated resident-byte count nor a GPU peak. The process maxima came from
external PID-scoped 0.5-second sampling and may miss true peaks. Exact resident
weight bytes and exclusive runtime/workspace residency remain unknown; device
aggregate measurements include other processes and allocator counters may
overlap. See [resource reporting semantics](../../docs/agent-harness.md#resource-observations).

The raw stdout/stderr files, per-run command records and samples, pins, and
comparison are retained outside the repository at
`/home/kmosoti/.cache/metron-alpha1/artifacts/product-phase-c-controls/`.
Their principal output and provenance hashes are recorded in the compact
[evidence record](evidence/O-0017-paged-unpaged-resource-diagnostic.json).
No coding acceptance, cache/batching qualification, physical-fit guarantee or
native coding milestone is established. Native calculator and repair gates in
both modes remain open.

Relations: supplements [O-0015](O-0015-native-coding-v10-and-context.md) with
bounded actual-route observations and informs the out-of-band reporting
contract in [agent-harness](../../docs/agent-harness.md#resource-observations).
Revisit after a defensible frozen numerical tolerance, direct tracing if
needed, and independently accepted resource and coding gates.
