---
id: O-0009
type: observation
status: active
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Known proposal errors and bounded correction

The integrated native run at `bea186b1803f0e828069cd19589994aaadac81aa`
loaded LFM3-1.7B Q4_K_M on CUDA with the corrected tokenizer from
[O-0008](O-0008-coding-loop-and-confinement.md). It produced a valid plan, then
one native `list(directory="/")` call. Domain validation rejected that absolute
path. No tool ran and no application file was written.

The two model requests used 389 and 1,389 input tokens under a 2,048-token
context with 512 reserved output tokens. Both model requests were charged, no
reservation remained, and audit/accounting/behavior replay passed. The journal
head is `384d966d7aad8c1062121aead21e0facff255ddb6bef32ec9a7cc9bccad2e1dc`.
The private operational record retains requests, native calls, worker events,
binary hashes and GPU observations.

This is a known invalid proposal, distinguishable from an uncertain execution.
The v3.4 controller terminates on that result. A bounded recovery mechanism can
preserve the effect boundary while asking the model to correct its next proposal.
Converting `/` to a permitted path in the host would conceal the model error.

## Versioned correction contract

The v3.5 controller retains the invalid observation and records bounded static
validation feedback linked to its evidence ID. It creates a new model intent
without creating a tool intent. A valid subsequent proposal follows ordinary
policy, approval and fresh admission. The third invalid proposal terminates the
task; model and decision budgets also bound correction.

Only known observed model parse, action-validation and planning-phase errors
qualify. Failure receipts, unknown effects, integrity failures, forbidden or
revoked actions and approval/lease failures retain their stopping behavior. The
host does not repair arguments, redispatch an uncertain effect or widen a grant.
Earlier controller versions retain their original prompt and transition rules.

Controlled tests exercise plan → invalid absolute directory → source-linked
feedback → a new valid proposal. Agent and standalone harness journals retain
four charged model observations, two ordinary effects, valid accounting and
behavior replay. Unit controls cover the correction bound, invalid feedback,
phase errors, budgets, uncertainty and policy rejection. These establish the
boundary contract; useful native correction still needs a live result.

## Remaining release evidence

No calculator or frozen repair has passed independent native acceptance. Issue
#27 also requires coding-specific resource/batching/cache qualification; legacy
solver/selector qualification cannot substitute for it. The operational
comparison's sub-second candidate threshold permits selecting no candidate; it
is not stated as a general alpha.1 latency threshold.

A source audit also found incomplete #25 requirements: retained unverified notes
do not represent stale/conflicting source status, and byte-sliced retrieval and
focused views do not retain a complete source-selection manifest. Those need
directed implementation and tests before #25 can close. Evaluator isolation has
structural and sandbox evidence; a private-data-to-memory exclusion probe remains
necessary. These gaps are not implied complete by the cross-task retrieval test.

**Relations:** refines D-0005, ADR-0022 and the coding priors from O-0008. Supports
bounded known-error handling in #26; keeps #7, #25, #27 and #28 acceptance open.
