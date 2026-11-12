---
id: O-0015
type: observation
status: observed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# Native V10 coding cases and context capacity

Two closed V10 agent-mode cases and one guided harness-mode case ended
rejected. Immutable run metadata and the available audit evidence are summarized in
[bounded evidence](evidence/O-0015-native-coding-v10-and-context.json).

| Case | Observed result | Boundary |
| --- | --- | --- |
| Repair | 1123.189 s; 12 model requests, 11 tool calls (5 Reads, 4 successful Read-derived CAS edits, 2 Cargo exits 101); generated Rust contained a literal LF character and HTML entities; last edit was not compiled | No Finish, freeze or evaluator result; rejected, exit 4 |
| Calculator | 253.708 s; 4 model requests, no tools; first Plan step was 280 bytes against a 256-byte limit; later 130-byte plan/36-byte step was valid; two Run proposals had invalid absolute/workspace paths; args used Unicode em dash flags and omitted values | Workspace remained empty; no application effect, Finish, freeze or evaluator result; rejected, exit 4 |
| Guided repair, harness | 894.670 s; 12 model requests, 11 tool calls; two `cargo.toml` Reads failed; a Read and one edit of `src/main.rs` succeeded; then 7 further Reads (5 `src/main.rs`, 2 `Cargo.toml`). The two missing reads targeted lowercase `cargo.toml`; all five later successful source reads returned the same post-edit digest | No Cargo Run, Finish, freeze or evaluator; rejected, exit 4. Host summary reports valid chain/accounting/projection/replay and zero reservations/holds; no separate independent run audit is available |

Both journals passed chain, accounting, projection and behavior replay checks,
with zero reservations, holds or unposted usage. Independent audits verified
preflight counts and the repair case's four edit CAS references. In the first
SDK call, a scalar was decoded once to the canonical exact value; there is no
evidence that decoding corrupted source. These cases provide evidence of
mechanical and generated-code failures in these runs, not comparative quality
or general model capability. No private feedback or coordinator app templates
were supplied to the two agent baselines. The guided harness run used a changed
public instruction variant: it spelled out action order, minimal edits, JSON
argument details and plan-step length, but supplied no implementation. Its
successful edit wrote Rust syntax rendered as HTML entities; the later source
Reads returned the same post-edit bytes each time. This variant is not a
matched-input quality comparison, and the host summary has not yet received an
independent third-party audit. Guided calculator cases remain unlaunched.

All three used LFM3-1.7B-GGUF Q4_K_M on a 6-GiB RTX 3060 Laptop, SDK
`fb4cdc3730dc64b366724e7637a205a8b7747499`, context/input/output caps
4096/3584/512, temperature 0.1, top-k 50, repetition 1.1, and
12/24/48 budgets. The first two use the sealed agent-mode baseline; the guided
case is harness mode with public task-specific instruction changes. Exact
binaries, source identity, model digest and recipe are
in the evidence JSON. These settings and results do not establish a selected
alpha.1 profile. The earlier forced parser probes are separately recorded in
[O-0014](O-0014-native-file-tool-contract.md) and are not capability evidence.

## Context and memory accounting

The publisher's LFM3-1.7B configuration documents 262,144 maximum positions;
its 1.6-GB KV-cache figure is for 128K context. Source-derived logical
retention for six global layers and five 256-token sliding caches is about
3.002 GiB at two-byte K/V values for one sequence. This is not measured runtime
memory; convolution state, allocation slack and implementation retention also
matter. Q4 weight quantization does not establish KV-cache dtype. GGUF file
size likewise is not an exact resident-weight VRAM measurement.

The trained position limit remains a capacity limit, not a promise that a full
prefill fits. Inspection of the [pinned LFM3 mask construction](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/models/lfm3.rs#L1235-L1255)
and [mask implementation](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/layers_masker.rs#L193-L231)
found the non-flash CUDA path's sliding-attention mask materializes a square
f32 prefill mask: at 262,144 tokens the
host mask alone is 256 GiB (and the corresponding BF16 GPU mask would be
128 GiB). This is a source-derived workspace blocker for that path, not a
measured launch. The separate steady K/V estimate is about 3.002 GiB at two
bytes per value with six global layers and five retained 256-token sliding
caches; it does not include prefill workspace or establish fit. Actual runtime
allocations and component dtypes remain unmeasured. The full trained context
should be usable only through a runtime/prefill path whose bounded workspace
fits available VRAM with operating margin. Capacity claims must distinguish
documented limit, estimated cache, source-derived workspace behavior, and
measured device memory.

### Paged-cache geometry (source-derived, not measured)

A separate read-only source and inventory analysis of the pinned SDK revision
found that the first-party worker requests `PagedCacheType::Auto`. In this
route the cache planner allocates paged K/V tensors for all eleven attention
layers (six global and five sliding-window); the five 256-token SWA windows do
not reduce their allocated tensor capacity. This differs from the unpaged
logical-retention estimate above, so that 3.002-GiB figure must not be used as
the first-party paged pool estimate.

With the SDK default 32-token block size, 262,144 usable tokens require 8,193
allocated blocks including the reserved block, covering 262,176 positions in
each layer. If Auto resolves to BF16/F16, the resulting eleven-layer K/V pool
payload is 5,906,300,928 bytes (5.50067 GiB), before recurrent state, weights,
allocator overhead or runtime workspace. On a 6-GiB device that leaves
536,150,016 bytes. The packed tensors for the 24 FFN matrices alone account
for 685,670,400 serialized bytes, so this full-context configuration cannot fit
within 6 GiB even before other weights or transient memory. This conclusion is
source-derived from the pinned cache geometry and artifact inventory; it is
not an observed allocation or a measurement of complete model VRAM.

The conclusion is bounded to the current all-GPU paged route and the
BF16/F16 Auto prediction. F32 requires more pool memory. An explicit one-byte
cache dtype would be a different, currently unqualified precision
configuration; this analysis neither enables it nor establishes that it fits.
It also does not establish a universally safe shorter context or a blanket
incapacity claim for every backend or device mapping. Keep the trained maximum
of 262,144 positions distinct from current input/transport limits, and require
measured memory and numerical checks for any bounded production setting. The
supporting read-only geometry analysis is preserved as
`product-paged-cache-geometry.md`.

Relations: extends [O-0014](O-0014-native-file-tool-contract.md); informs
[prior 64](../../docs/research/priors.md) and the alpha.1 gates in
[D-0005](../decisions/D-0005-alpha1-delivery.md). Revisit after sealed,
independently accepted coding artifacts and measured full-context VRAM on the
intended runtime/device.
