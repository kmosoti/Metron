---
id: O-0014
type: observation
status: observed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# Native file-tool contract

Four bounded LFM3-1.7B Q4/CUDA V9 attempts used the frozen calculator or Rust
repair acceptance contract. All ended rejected without an artifact freeze or
independent evaluator result. [Bounded evidence](evidence/O-0014-native-file-tool-contract.json)
identifies their actual source, binaries, inputs, accounting and journal heads.

| Input and mode | Actual behavior | Outcome |
| --- | --- | --- |
| Original repair fixture, agent | Root List with raw dot succeeded after canonicalization; ten reads alternated source and manifest | Twelve model requests and eleven tools; no edit |
| Empty calculator workspace, agent | Listed root, requested Cargo help, ran Cargo init, then alternated List and manifest Read | Twelve model requests and eleven tools; only the default Cargo scaffold |
| Public buggy source and SHA supplied inline, standalone harness; narrower tool set | One create-style edit was denied for missing expected digest; five Cargo test/build operations used the unchanged fixture; final three edit proposals were invalid | Ten model requests and six tools; zero candidate tests and no source change |
| Empty calculator workspace with ordinary Cargo bootstrap command specified; narrower tool set | Executed the requested Cargo init, then submitted three Read proposals with null paths | Five model requests and one tool; no application implementation |

All four have valid chain/accounting/projection and behavior replay, zero
outstanding reservations and no integrity hold. The first two independently
audited runs matched all twelve prompt preflights to reported usage and all
eleven known tool receipts to their next histories. The root-directory alias
worked for actual List and Run effects, but did not establish useful coding.

The focused inputs changed both the public objective and allowed tool subset;
they are not matched-input quality comparisons. The repair input contained only
the original public buggy fixture, not a repair implementation. The calculator
bootstrap specified an ordinary Cargo command, not application code. Candidate
Cargo success with zero tests is not contract acceptance. No private evaluator
feedback was given to the model.

## File interface hypothesis

These observations expose mechanical failures at the model/tool boundary:
required filenames became null and a replacement proposal used the create-only
precondition. They do not prove that correcting the interface will yield correct
Rust or good planning. Earlier attempts also produced incorrect source and
repeated successful reads ([O-0013](O-0013-native-coding-feedback.md)).

V10 separates creation from replacement and derives replacement identity from an
actual retained Read. It keeps the existing typed action, grant and host CAS.
An untruncated line excerpt can bind the full-file metadata digest; this is not
proof of full-file visibility. Task prose, memory text and model-provided SHA
strings are not substitutes for that observed reference. Failed, omitted,
truncated or mismatched Read data cannot bind a replacement.

The pinned SDK's private XML parser and schema coercion distinguish quoted scalar
text from the intended string value. V10 uses XML-safe JSON strings and decodes
exactly once after the SDK, rather than allowing bare primitive text to change
type. The pinned [XML parser](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/tools/parsers/qwen.rs)
and [schema coercion](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/tools/mod.rs#L377)
are the implementation sources. Controlled tests establish normalization,
omission and stale/CAS behavior,
including live SQLite receipt validation and audit/reconciliation in both modes.
Grammar and SDK-shaped values are separate from executing the actual SDK parser;
the bounded real-parser probe and native V10 coding outcomes must be recorded
before claiming those results.

Relations: extends [O-0013](O-0013-native-coding-feedback.md); informs
[coding interfaces and authority diagram](../../docs/coding.md), prior 64 and
[alpha.1 gates](../decisions/D-0005-alpha1-delivery.md). Revisit after actual
parser evidence and independently accepted calculator/repair artifacts in both
modes. Model, quantization, sampler, input configuration and evaluator identity
must remain explicit.
