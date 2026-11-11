---
id: R-0008
type: research
status: proposed
updated: 2026-11-10
added_by: agent
human_ratified: false
---

# Local inference plugin and resource profile

The default local model is **LFM3-1.7B Q4_K_M**, shared by planning and
tool-selection roles through the **mistral.rs** integration in
[R-0009](R-0009-mistralrs-batching-and-cache-contracts.md). The LFM2.5 artifact
below is a historical size comparison and is excluded from the product profile.
Minimize idle residency and qualify batching/cache settings against total
resource use. The product runtime and live inference adapter remain planned.

## Sources and artifact identities

The first artifact is selected; the second is retained only to audit the earlier
two-model memory comparison.

Audited on 2026-11-10 using official model cards, repository metadata and bounded
GGUF header reads. GB means 10^9 bytes; GiB means 2^30 bytes.

| Official artifact | Repository revision | Bytes | GB | GiB | Published SHA-256 |
| --- | --- | ---: | ---: | ---: | --- |
| [LFM3-1.7B-Q4_K_M.gguf](https://huggingface.co/LiquidAI/LFM3-1.7B-GGUF/tree/cd0c74c575b5c416f3011b5b4c298077f26ca8c2) | `cd0c74c575b5c416f3011b5b4c298077f26ca8c2` | 1,099,627,808 | 1.100 | 1.024 | `3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4` |
| [LFM2.5-1.2B-Thinking-Q4_K_M.gguf](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Thinking-GGUF/tree/b71945805f7d4d87dabd73e26098d44bcad6c35f) | `b71945805f7d4d87dabd73e26098d44bcad6c35f` | 730,899,072 | 0.731 | 0.681 | `4a1e1824f71091f368137b6966094461019f6af3a5aadd96033bee59fc9eaeea` |

Combined file size is 1,830,526,880 bytes: **1.831 GB / 1.705 GiB**.
These are downloadable artifact sizes, not measured VRAM or process working sets.
Tensor placement, repacking and metadata change resident weight allocations.
The hashes above are published LFS identities; full files have not been
downloaded and independently hashed in this audit.

The [LFM3 model card](https://huggingface.co/LiquidAI/LFM3-1.7B/blob/1da06bec6a819da10b54c67e5c90972b46d81871/README.md)
describes tool calling and structured outputs but discourages agentic coding and
complex multistep reasoning. The
[Thinking model card](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Thinking/blob/fa4681e33c27d2861d1eacc9d29ccb68840344a0/README.md)
also discourages programming and knowledge-intensive tasks. Neither has been validated as a
coding backend for Metron. The selected model's coding acceptance gate requires independent
task verification; model size or upstream benchmarks cannot satisfy that gate.

Both checkpoints use **LFM Open License v1.0**, a custom license with commercial
conditions. Inspect each pinned repository's `LICENSE` and preserve required
notices before distribution. Metron-owned source remains Apache-2.0. This
default weight profile does not support an unqualified entirely-FOSS deployment
claim. [LFM3 license](https://huggingface.co/LiquidAI/LFM3-1.7B/blob/1da06bec6a819da10b54c67e5c90972b46d81871/LICENSE),
[Thinking license](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Thinking/blob/fa4681e33c27d2861d1eacc9d29ccb68840344a0/LICENSE).

## Memory claims and deductions

There is **no established 2,048-token OOM threshold**. Total device memory includes
resident tensors, attention caches, convolution state, compute/work buffers,
CUDA context and allocator overhead. Batch size, parallel slots, cache dtype,
backend build and placement all matter. Q4_K_M quantizes weights; it does not
automatically make the KV cache 4-bit.

Subtracting the combined *file sizes* from 2 GiB leaves 302.27 MiB; subtracting
from decimal 2 GB leaves 169.47 MB. Neither remainder is a usable cache allowance
without measuring the other allocations and actual resident weights.

For a single sequence with FP16 keys and values, an attention-only estimate is:

```text
KV bytes = sum over attention layers(
    retained_tokens * KV_heads * (key_head_dim + value_head_dim) * 2 bytes
)
```

The pinned [LFM3 configuration](https://huggingface.co/LiquidAI/LFM3-1.7B/blob/1da06bec6a819da10b54c67e5c90972b46d81871/config.json)
and GGUF header contain six global-attention layers, five sliding-attention
layers, four KV heads, 128-dimensional heads and a 256-token sliding window.
The pinned [Thinking configuration](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Thinking/blob/fa4681e33c27d2861d1eacc9d29ccb68840344a0/config.json)
and header contain six attention layers, eight KV heads and 64-dimensional heads
(`2048 / 32`). At **2,048 tokens per model**, full-window attention allocation
would be 44 MiB and 24 MiB respectively: 68 MiB combined. Ideal bounded sliding
retention lowers the first estimate to 26.5 MiB, or 50.5 MiB combined.
These calculations exclude padding, convolution state and every other runtime
allocation. They establish neither safety at 2,048 nor failure above it.

The Thinking card advertises a 32,768-token context while its pinned native
configuration and GGUF header encode 128,000. Do not infer validated long-context
quality from the metadata maximum. This discrepancy concerns the historical
comparison only; it creates no supported Thinking profile in Metron.

Read actual device capacity, free VRAM and Linux `MemAvailable` at admission.
A 2 GiB Metron device budget is distinct from physical GPU capacity. System RAM
is neither reserved CPU-model capacity nor automatically additional dedicated
VRAM. An assumed 18 GB of shared memory cannot establish current headroom.

## Proposed plugin boundary

Introduce `products/runtime/crates/metron-inference-local` in alpha.1, after P0
and alpha.0. It implements the harness model effect port and supervises a Rust
worker using the pinned mistral.rs SDK. Alpha.3 can register the same trusted
adapter through its static registry. No inference dependency enters the agent
or laboratory. The component/authority diagram and complete scheduling, cache
and acceptance contracts are in [R-0009](R-0009-mistralrs-batching-and-cache-contracts.md#composition-and-authority).

Use one pinned LFM3 text weight set for multiple roles. Load it on demand, keep
it for an active task, and release it afterwards. CPU fallback uses that same
qualified LFM3 checkpoint. Additional LFM3 modalities are gated by actual feature
needs, artifact/license checks and measured resource fit.

Start at 2,048 total tokens per sequence and at most 512 generated tokens,
including system/template/tools/input and output reservation in the context.
Use one active sequence as the measurement baseline; qualify adaptive continuous
batching, chunked prefill and bounded hybrid prefix reuse through R-0009.
The 2 GiB aggregate Metron VRAM target is distinct from the KV-pool budget and
must include every worker allocation. A memory planner or telemetry alone does
not establish a hard allocator cap.

## Effect and replay contract

Install weights explicitly, independently of inference requests; verify complete
artifacts against published digests. Follow persisted intent, grant/lease/budget
admission, dispatch and idempotent settlement. Keep per-request identities and
receipts even when requests share a batch or prefix blocks. Record model/build,
quantization, template, sampling, device/cache settings, token bounds, actual
usage and uncertainty. Unknown shared-cost attribution remains explicit.
Replay recorded model observations; never assume seeded GPU regeneration is
identical. Only the harness authorizes and executes tools.

## Evidence and revisit conditions

Only artifact metadata, header contents and arithmetic were checked here; no
model inference, OOM experiment, task-quality or latency benchmark was run.
Read official API metadata with `?blobs=true` at each pinned revision; request
`Range: bytes=0-65535` from immutable GGUF resolve URLs and inspect GGUF v3
metadata. The audit read 64 KiB per file, not complete weights.

Relations: **implements** the model-adapter plan in
[D-0003](../decisions/D-0003-runtime-implementation-sequence.md);
**constrains** [R-0005 plugins and licenses](R-0005-storage-plugins-protocols.md)
and [R-0006 crate boundaries](R-0006-rust-implementation-contracts.md);
**extended-by** [R-0009 runtime and batching](R-0009-mistralrs-batching-and-cache-contracts.md);
**registers** resource/quality priors in the
[canonical ledger](../../docs/research/priors.md).

Revisit on a new qualifying artifact/backend, failed model-quality or memory
acceptance, changed hardware, or a matched experiment demonstrating a more
resource-efficient profile at the required quality and latency.
