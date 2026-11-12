---
id: O-0015
type: observation
status: observed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# Native V10 coding cases and context capacity

Two closed V10 agent-mode cases both ended rejected. Their immutable run
metadata and independent sealed audits are summarized in
[bounded evidence](evidence/O-0015-native-coding-v10-and-context.json).

| Case | Observed result | Boundary |
| --- | --- | --- |
| Repair | 1123.189 s; 12 model requests, 11 tool calls (5 Reads, 4 successful Read-derived CAS edits, 2 Cargo exits 101); generated Rust contained a literal LF character and HTML entities; last edit was not compiled | No Finish, freeze or evaluator result; rejected, exit 4 |
| Calculator | 253.708 s; 4 model requests, no tools; first Plan step was 280 bytes against a 256-byte limit; later 130-byte plan/36-byte step was valid; two Run proposals had invalid absolute/workspace paths; args used Unicode em dash flags and omitted values | Workspace remained empty; no application effect, Finish, freeze or evaluator result; rejected, exit 4 |

Both journals passed chain, accounting, projection and behavior replay checks,
with zero reservations, holds or unposted usage. Independent audits verified
preflight counts and the repair case's four edit CAS references. In the first
SDK call, a scalar was decoded once to the canonical exact value; there is no
evidence that decoding corrupted source. These cases provide evidence of
mechanical and generated-code failures in these runs, not comparative quality
or general model capability. No private feedback or coordinator app templates
were supplied. The separate guided repair case remains active and is not an
outcome in this record.

Both used LFM3-1.7B-GGUF Q4_K_M on a 6-GiB RTX 3060 Laptop, SDK
`fb4cdc3730dc64b366724e7637a205a8b7747499`, context/input/output caps
4096/3584/512, no-thinking, temperature 0.1, top-k 50, repetition 1.1, and
12/24/48 budgets. Exact binaries, source identity, model digest and recipe are
in the evidence JSON. These settings and results do not establish a selected
alpha.1 profile. The earlier forced parser probes are separately recorded in
[O-0014](O-0014-native-file-tool-contract.md) and are not capability evidence.

## Context and memory accounting

The publisher's LFM3-1.7B material states 262,144 maximum positions in model
configuration; its 1.6-GB KV-cache figure is for 128K context. A linear
full-attention-only extrapolation to 262K is roughly 3.2 GB under six global
layers, four KV heads, 128 dimensions and two-byte K/V values. This is a
conditional estimate, not measured runtime memory; sliding-attention,
convolution state, allocator/runtime overhead and implementation retention
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
should be usable only through a runtime/prefill path whose required workspace
fits available VRAM with operating margin. Capacity claims must distinguish
documented limit, estimated cache, source-derived workspace behavior, and
measured device memory.

Relations: extends [O-0014](O-0014-native-file-tool-contract.md); informs
[prior 64](../../docs/research/priors.md) and the alpha.1 gates in
[D-0005](../decisions/D-0005-alpha1-delivery.md). Revisit after sealed,
independently accepted coding artifacts and measured full-context VRAM on the
intended runtime/device.
