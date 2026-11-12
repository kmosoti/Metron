---
id: O-0013
type: observation
status: observed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# Native coding execution and feedback limits

Three actual agent-mode attempts used LFM3-1.7B Q4 on an RTX 3060 through native
recipe V7. They started from the required inputs: the frozen repair fixture or
an empty calculator workspace. No application implementation was supplied.
[Bounded result records](evidence/O-0013-native-coding-feedback.json) identify
the code, worker, prompt counts, outcomes and operational-record hashes.

| Task | Observed behavior | Result |
| --- | --- | --- |
| Rust repair | Planned, read, authored an edit, received three real Cargo compiler errors, read and edited again, passed one candidate-written test, then reread source | Eight model requests and seven tools posted; next admission rejected at the eight-request cap. No freeze or evaluator |
| Calculator | Planned and ran `cargo init`, then submitted three invalid Read proposals with a null path | Five model requests and one tool posted; rejected at the invalid-proposal limit. No calculator implementation, freeze or evaluator |
| Rust repair, twelve-request follow-up | Same objective/profile and original fixture; further comment-only and identical-content edits, plus another candidate test pass | Twelve model requests and eleven tools posted; next admission rejected at the cap. Public behavior remained wrong; no freeze or evaluator |

All attempts retained valid chain/accounting/projection and behavior replay, with
zero outstanding reservations and no integrity hold. An independent read-only
audit of repair matched all seven histories and tool actions to real selected
receipts, both CAS edits to their read digests, and eight prompt preflights to
receipt usage. Cargo/Rustc identities matched the admitted catalog.

The repair's passing test asserted the implementation's incorrect behavior.
The source still counts empty/trailing segments and panics on missing input,
contradicting the public task requirements. Compilation and candidate tests do
not establish semantic acceptance. No private evaluator feedback was supplied.
Increasing the repair allowance from eight to twelve did not produce a submitted
repair in these conditions. More calls alone did not resolve the public behavior
errors. The configured 4K context is not a test of a full 4K input or batching/cache use.

Inspection found two coding-loop gaps: focused validation feedback did not show
the rejected proposal or concrete error, and the native catalog excluded the
existing List port. Bounded error/preview data and policy-gated native List are
candidate corrections. Their effect on successful model completion remains
unmeasured; extra context or a larger catalog could also hurt selection.

Relations: extends [O-0012](O-0012-paired-tool-history.md); informs
[coding contracts](../../docs/coding.md) and
[alpha.1 gates](../decisions/D-0005-alpha1-delivery.md). Revisit after matched
native attempts with the new feedback/catalog and independent acceptance in both
modes. Model, quantization, sampler and frozen evaluator must remain identified.
