---
id: ADR-0028
type: decision
status: proposed
updated: 2026-11-14
added_by: agent
human_ratified: false
---

# 0028 — Experimental intent contracts for bounded coding

## Context

Native coding acceptance remains open. Existing coding contracts bind an
objective and frozen acceptance criteria, but they do not establish that prose
captures the assumptions and decisions needed to execute a task safely. A
contract proposed from task inputs and model assumptions may make those
assumptions explicit and expose unresolved decisions before coding. Contract
readiness is structural evidence only; it is not evidence that the contract is
semantically correct or that a coding result is correct.

The existing `ComparisonSession` is the bounded host mechanism available for
controlled comparison. A private experimental path can compare proposal forms
without changing the frozen bytes or identities of
`coding_acceptance.rs`, product schemas, durable state, or authority transitions.

## Proposed decision

Add a host-private experimental contract path that proposes, validates, freezes,
experiments with, and audits a contract using the existing comparison session.
Keep source/provenance, baseline, approved deltas, model assumptions, and
resolutions explicit in the experimental record. Validation establishes
structural readiness, never semantic correctness.

Use a fixed trusted checker registry and a custom `CodingVerifier` that
preserves the complete frozen coding acceptance source and its identities.
Retain the established core acceptance version and settings. Do not introduce
an extensible task DSL, public workflow API, new wire schema, or new durable
authority state for this experiment.

Keep ordinary Cargo and rustfmt tools available in every pilot arm. The
architecture-assistance treatment adds scaffold, guidance, and diagnostic
projection only; it must not receive a different checker, acceptance authority,
or ordinary tool catalog. Pin the new campaign catalog to the coherent Rust
1.100.0-beta.12 Cargo/rustc pair and ripgrep 15.2.0; this campaign pin is
separate from the stable Rust 1.100 product checks.

The proposed pilot uses native LFM3 with output bounded to 512 tokens,
selected context of 4,096 tokens and a maximum context ceiling of 8,192 tokens,
and objective input bounded to 1,024 bytes with full-digest binding and
overflow rejection. It compares twelve cases (six new
and six existing) across two factors: prose versus typed contract, and
architecture assistance off versus on, for at most 48 attempts after four
reviewed contract diagnostics. At least one of the two diagnostics must pass
in each slice; zero accepted results in either slice fails the gate. Two
accepted diagnostics in a slice is allowed. If the diagnostic gate fails,
stop coding, finish extraction, and do not promote. The planned
decision rule prefers typed contracts only if each slice gains at least one
accepted result without regression, safety faults, added silent assumptions,
or blockers; architecture assistance is eligible only with separate gains in
both slices.

Bind the complete raw request, resolution, bundle digest, and shared public
offline/locked harness preamble into the 1,024-byte objective; overflow fails
explicitly. Keep case vectors, labels, resolutions, and baselines frozen before
native outputs are observed. The pre-native corpus revision 3 digest is
`d1176eadd84d484145f847fbb18e4e68d633f1ff9715d0e99d3adc4d7e8d81af` (schema
2); revision 2 is superseded pre-study provenance. Revision 3 makes
file/network functionality advisory because the trusted checker cannot prove
artifact file-read purity. Sandbox and network confinement require separate
environmental evidence.

Bind the exact host executable digest into immutable pre-effect settings and
the evaluator identity. New effects and resume require that exact current host
binary; changing the host executable requires a fresh task/workflow. The native
inference worker, backend, and profile remain a separate pinned identity.
Historical offline audit may inspect retained prior identities and recorded
consistency without claiming a fresh rerun under a different current
executable.

Bound preparation to three model requests, with at most twelve total model
requests and sixteen admitted coding tool effects per attempt. Final verifier
checks have separate fixed bounds. The internal runner supports diagnostic
and experimental execution. Public contract run/resume remain unconditionally
deferred; enabling them requires a positive pilot and a separate reviewed
change.
Keep the selected CLI binary fixed, cap the selected workspace at 128 files and
two path crates, and cap sealed vectors at eight. These limits bound the
experiment; they do not establish general coding capability.

## Consequences

The host implements private proposal, validation, freezing, experiment,
finalization, and audit. Public contract run/resume remain deferred pending a
positive native promotion result. Current portable workspace tests, formatting,
and Clippy pass, but do not establish native model quality. The implementation
keeps experiment records and checker provenance separate from durable product
contracts and acceptance authority. A failed, incomplete, or unavailable pilot
leaves native coding acceptance unmet. Reports must distinguish completed
checks, unmet gates, and unavailable evidence. Alpha.1 remains open until its
independently accepted native coding and resource gates pass.

The architecture overview distinguishes the implemented host experiment path
from gated public run/resume and promotion.

**Relations:** complements [ADR 0022](0022-alpha1-coding-and-memory.md) and
[ADR 0025](0025-file-backed-inference-session-authority.md); does not amend
frozen coding acceptance identities or alpha.1 gates. Revisit after the
implementation and pilot are independently reviewed.
