---
id: O-0012
type: observation
status: observed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# Paired tool history diagnostic

A single fixed LFM3-1.7B Q4 CUDA request was given an actual prior assistant
`read` call and its matching recorded tool result. The SDK returned one valid
`edit` proposal with the expected path and file digest. Preflight tokenization
matched usage: 1,419 prompt tokens, 308 completion tokens, 171.7 seconds elapsed.
These are one diagnostic's measurements, not throughput estimates.

The generated Rust was not executed. Inspection found an `Option` pattern on a
`String`, an unavailable iterator method, whitespace handling inconsistent with
the task, and a missing-input panic. This establishes neither a successful repair
nor coding acceptance. No private evaluator output was supplied.

The original SDK tool-call ID had not been retained. The diagnostic assigned a
new correlation alias to the recovered exact call arguments and matching result;
it did not reconstruct that ID. Two aliases produced identical SDK token IDs and
detokenized prompt bytes. Raw XML before the SDK parser was unavailable.

The [bounded evidence record](evidence/O-0012-paired-tool-history.json) contains
the returned action, model identity, measurements and operational-record hashes.
It summarizes the run; it does not include every executable or input artifact.

**Hypothesis:** preserving completed assistant/tool pairs in the ordinary coding
loop may let compiler feedback support correction. One edit with incorrect Rust
cannot validate that hypothesis. The implementation must remain generic to
`metron-agent` and `metron-harness`; this temporary diagnostic defines no product
component or task-specific workflow.

Relations: extends [O-0011](O-0011-native-xml-constraints.md); tests the coding-loop
hypothesis in [R-0007](../research/R-0007-runtime-evaluation.md); informs
[coding context](../../docs/coding.md). Revisit after actual sandboxed repair and
calculator runs in both modes, including failures and independent evaluation.
