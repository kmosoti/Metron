---
id: O-0008
type: observation
status: observed
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Coding loop, confinement and native-model failures

The experimental coding implementation adds explicit task/policy bindings,
admitted file and process effects, scoped persistent notes, a bounded model
context, frozen source artifacts and independent acceptance. Controlled proposals
exercise these boundaries. **Native LFM3 application acceptance has not passed;
these observations do not establish alpha.1 readiness.**

Implementation baseline: `15e461c741025346c16889aadef9699c72beeeca`, with
subsequent regression checks recorded alongside their source changes. The
[coding entry point](../../docs/coding.md) documents the executable
interface; [ADR-0022](../../docs/adr/0022-alpha1-coding-and-memory.md) diagrams the
authority and verification boundaries. Operational inputs, commands, binary
hashes, journals and test logs are retained in the local alpha1 artifact record.

## Controlled observations

| Property | Evidence and limit |
| --- | --- |
| Independent modes | Controlled agent and standalone harness runs reach `AwaitingVerification`; a model's `finish` cannot mark its own task complete. |
| Admission | Forbidden and revoked actions do not mutate files. Exact approval bindings are required before permitted mutations. An uncertain model dispatch retains its reservation across reopen without a second dispatch. |
| Confined files | Physical probes reject symlinks, hard links, special files, metadata paths and incorrect compare-and-swap hashes. Nested directory creation remains anchored to the workspace. |
| Confined processes | Actual pinned-tool probes exercise jj, rg, Z3 and Lean; network and home access fail, and timed-out descendants are cleaned up. These tests are explicitly gated because they require Linux user namespaces, systemd and an executable catalog. |
| Frozen acceptance | Private evaluator probes reject changed source and unexpected files. Each private case uses the same sealed candidate image and fresh scratch, preventing a candidate from changing its executable for the next case. |
| Shared toolkit loop | A controlled admitted sequence performs plan, jj baseline, file edit, rg search, jj diff, SMT check, Lean check and finish. Its journal audits and behavior replay pass; no model-written application is implied. |
| Persistent memory | An admitted Task A `memory_put`, reopen of both durable stores, and admitted Task B `memory_search` preserve the note, source task and `model_authored_unverified` trust label. Both journals audit and replay. |
| Memory authority | Revoked memory grants stop subsequent model dispatch; memory text cannot widen the tool policy. Database and sidecar permission tests reject shared, linked or unsafe files. |
| Laboratory compatibility | All 131 laboratory tests pass after the coding changes. Existing runtime journal domains and absent coding fields retain their legacy encoding. |

The integrated portable runtime suite passed 190 tests, with four physical probes
explicitly ignored there. Running those four probes separately with the pinned
catalog and model artifacts passed. Both workspaces' lint checks pass. The fresh
laboratory smokes retain the original normal and capped journal heads, and all
five archived native coding journals audit and behavior-replay with unchanged
heads under the new host.

Process confinement trusts the host's selected toolchain and shared libraries.
The tool catalog seals primary executable bytes; it does not pin the entire
operating system. Disk admission reserves headroom and bounds ordinary outputs;
it is not a filesystem quota. Physical sandbox checks must run explicitly rather
than being inferred from portable test success.

## Prompt serialization and replay

A native journal failed replay when the same controller was rebuilt without the
SDK's `serde_json/preserve_order` feature. Its model context had been serialized
through `Value`, so dependency feature unification changed object order and the
committed input digest. The effect was reproducible across the archived native
and host-only binaries.

Versioned controllers now use explicit serialization: the original v3 format
retains its archived field order, v3.1/v3.2 sort their values explicitly, and
v3.3 uses an ordered context structure. Literal prompt-byte/hash tests pass with
and without `preserve_order`. The archived v3, v3.1 and v3.2 native journals
audit and behavior-replay with the current host. New context formats require a
new controller identity rather than changing old replay semantics.

## Native coding observations

These runs used the pinned LFM3-1.7B Q4_K_M GGUF and CUDA backend from
[O-0005](O-0005-native-lfm3-cuda.md), one sequence, 2,048 context tokens and
512 reserved output tokens. A later first-party worker extended the bounded
coding deadline to 300 seconds and constrained proposal paths and digests.
The model's weights were not duplicated for agent roles.

| Attempt | Observed outcome |
| --- | --- |
| Initial full context | The first model request exceeded its 60-second deadline; the RTX 3060 was active. No application files were written. |
| Phase-aware contexts | Correcting a contradictory planning catalog yielded a plan, followed by an absolute workspace path rejected by the domain. |
| Constrained paths | The model repeatedly read a missing `Cargo.toml`; accumulated context exceeded the 1,536-token input allowance before a later dispatch. |
| Focused context | Keeping a plan and the latest two tool views held repeated requests near 543 input tokens. The model repeatedly listed the empty root; it still wrote no files. The owned worker was stopped and the in-flight reservation remained uncertain. |
| Direct create diagnostic | The model generated syntactically valid manifest TOML but supplied an invented non-null create hash. No tool was executed. |

All retained coding journals preserve the observed outcomes, accounting and
behavior replay. None is an accepted calculator or repair. Compact context
solves the demonstrated growth problem; these runs do not show that it solves
coding progress. A failed task is not retried through an uncertain admission.

## Native tool-call tokenizer incompatibility

The pinned model's [official tool format](https://huggingface.co/LiquidAI/LFM3-1.7B/blob/1da06bec6a819da10b54c67e5c90972b46d81871/README.md)
uses XML. Direct SDK tool metadata probes with both `Required` and `Auto`
choices failed before yielding a callable action. Both reported an error while
applying token 125021 after a `<function=` prefix.

The GGUF maps ID 125021 to `<function=` with `USER_DEFINED` type 4 and
`tokenizer.ggml.pre=lfm2`. The official auxiliary tokenizer maps the same ID
and text with `special=false`. At the pinned SDK revision, the
[GGUF converter](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/gguf/gguf_tokenizer.rs#L241)
treats user-defined tokens as ordinary only for `flint`; this LFM3 token instead
becomes special. The [grammar bridge](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/pipeline/llg.rs#L23)
then prefixes its bytes with `0xff`.

A CPU test using the pinned tokenizer libraries reproduces the reported
mechanism: a literal `<function=` grammar accepts ID 125021 with the ordinary
bytes and fails with the special-token prefix, reporting `got '<'; applying
'ÿ'`. This test reconstructs the converter's classification from its source;
it does not directly invoke its crate-private conversion function. The generic
`ParserTooComplex` error category does not establish a grammar complexity limit.

The public tokenizer override validates vocabulary length and every ID against
the GGUF. The diagnostic corrected tokenizer preserves all 128,000 GGUF IDs,
matches the 125,033 official auxiliary IDs and all 293,320 merges, and retains
the SDK converter's preprocessing and decoder configuration. Its sole semantic
change is the special flag for the 14 user-defined tokens. Nine CPU corpus cases,
including those delimiters, retain IDs, offsets and decoded bytes when special
tokens are included.

A matched native `Auto` tool probe with that correction succeeded in 30.03
seconds: one `edit` call, `Cargo.toml`, null create hash and valid dependency-free
Rust 2024 TOML. Both the failed and corrected probes used strict function
schemas. No tool was executed. This supports the parser cause for that request;
it does not establish coding-task success.

The Rust compatibility helper validates the pinned inputs, materializes the
corrected tokenizer with recursively sorted object keys and verifies existing
output before reuse. A physical-model regression matches the diagnostic JSON,
including all 3,107 added tokens and the 14 corrected flags, and rejects tampered
output. Its canonical file SHA-256 is
`2435fed9a91d7f8bc1c8744bf13f01fc210609592f2db7fbbbd4dbc30f46b9c9`.
The new native coding recipe and v3.4 controller bind this behavior and the
host-owned phase/tool catalog independently of untrusted objective or plan text.
Earlier controller encodings remain available for replay.

**Relations:** supports D-0005 and ADR-0022 implementation boundaries; refines
R-0010/R-0011 and the serialization and compact-context priors. The remaining
release obligations include native calculator and frozen repair acceptance in
both modes, resource/batching/cache gates and independent review. No production
inference profile is selected.
