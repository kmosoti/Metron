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

Inspection of those V7 attempts found two coding-loop gaps: focused validation feedback did not show
the rejected proposal or concrete error, and the native catalog excluded the
existing List port. Bounded error/preview data and policy-gated native List are
corrections implemented in V8. Their effect on successful model completion was
initially unmeasured; extra context or a larger catalog could also hurt selection.

Relations: extends [O-0012](O-0012-paired-tool-history.md); informs
[coding contracts](../../docs/coding.md) and
[alpha.1 gates](../decisions/D-0005-alpha1-delivery.md). Revisit after matched
native attempts with the new feedback/catalog and independent acceptance in both
modes. Model, quantization, sampler and frozen evaluator must remain identified.

## V8 directory proposals

A subsequent Q4/CUDA calculator attempt used V8, bounded rejected-proposal
feedback and the policy-gated List tool. It completed a root List with null and
`cargo init`, but proposed `list` with directory `.` three times. After the first
invalid proposal it recovered to a valid root List, then attempted a nonexistent
`.cargo.toml` and returned to the invalid directory spelling. It ended rejected
with eight model requests and four tools posted, zero reservations or hold, and
valid accounting and behavior replay. No application implementation, frozen
artifact or independent evaluator was reached. Its independent audit matched
all eight prompt preflights to usage and all four tool actions to receipts.

This supports a narrower diagnosis than a general failure to discover files:
the native directory interface rejects a conventional root spelling even though
that same root is already available through null. V9 canonicalizes exactly that
spelling for native List and Run only, preserving raw evidence, old recipe
behavior and all host authorization. Whether it improves task completion needs
actual native evidence; it does not address incorrect Rust or task semantics.

A fresh V8 repair attempt clarified only the public objective and examples,
keeping the original fixture, engine and budgets. After two invalid List calls
with dot, it recovered to Read the source. The next request was tokenized at
1,400 input tokens and started, but no completion was recorded before worker
exit 300.137 seconds later, matching the configured 300-second deadline. It
ended blocked with four model requests and one tool posted and one model
reservation retained. Chain/accounting/projection and behavior replay remained
valid; no edit, freeze or evaluator occurred. The unanswered execution remains
unresolved and must not be redispatched as though it had never run.

A clarified V8 calculator follow-up also kept the empty workspace and the
same engine/budgets. It planned, then proposed List with dot three times despite
explicit null-root guidance. It ended rejected after four model requests and
zero tools, with no reservations or hold and valid audit/replay. Clarifying the
public objective alone did not resolve this directory mismatch in that attempt.

## Bounded thinking-mode diagnostic

A separate model-only probe submitted one actual prior Read/action/result pair
with thinking disabled and then enabled. Both arms used the pinned Q4 model,
CUDA, no initial grammar constraint, automatic tool choice, no prefix cache,
4K context and a 1,024-token output allowance. No proposed tool was executed.
The disabled arm used 1,419 prompt and 308 completion tokens and proposed one
Edit containing invalid Rust and public-contract errors. Its arguments matched
the earlier paired-history diagnostic. The enabled arm used 1,418 prompt and
1,024 completion tokens, all reported as reasoning, and ended at length with no
content or tool call. Token preflights matched reported usage in both arms.

These two fixed-order samples, without a controlled random seed or warm-up,
do not establish a quality or speed ranking. They show that merely enabling
thinking within this allowance did not yield an executable next action.
No candidate artifact, evaluator result or production setting follows from the
probe. The current non-thinking recipe remains an experimental configuration.
