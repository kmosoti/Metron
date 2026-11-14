---
id: O-0020
type: observation
status: observed
updated: 2026-11-14
added_by: agent
human_ratified: false
---

# O-0020 — Intent-contract experiment implementation and native pilot

## Scope and frozen inputs

This observation records the experimental host-private intent-contract path
and its first native pilot campaign. It does not change the alpha.1 acceptance
criteria. The core coding acceptance source (`coding_acceptance.rs`) and its
identities remain unchanged. Contract readiness is structural and declared
readiness only; it is not semantic correctness. The M1/M2 implementation and
portable workspace checks are not evidence of native coding quality.

The pre-native independent corpus uses schema 2, revision 3, with digest
`d1176eadd84d484145f847fbb18e4e68d633f1ff9715d0e99d3adc4d7e8d81af`. It
supersedes revision 2 before native outputs were observed. The only corpus text
change was from a strict runtime file/network prohibition to advisory wording
that no file or network functionality is required; case vectors, labels,
resolutions, and baselines did not change. Because the trusted checker does not
prove artifact file-read purity, filesystem and network confinement require
separate environmental evidence.

## Implementation boundary

The host implements contract proposal, validation, freezing, experiment,
finalization, and audit. The local-inference crate provides read-only
comparison-session audit over expected profile and ordered case inputs. These
audits establish recorded or structural consistency within their stated scope;
they are not cryptographic proof against a malicious host and do not establish
model quality. Production recorded-reply and controlled-reply execution paths
are rejected. Imported drafts may be validated, but are not native evidence.

The public contract `run` and `resume` commands are currently unconditionally
deferred and return a blocked result. A positive native pilot is necessary but
not sufficient to enable them; that would require a separate reviewed change.
The exact running host executable digest is bound in immutable pre-effect
settings and evaluator identity. New effects and resume require the same host
binary; a changed binary requires a fresh task/workflow. The inference worker,
backend, and profile are a separate pinned identity. Historical offline audits
may verify retained recorded consistency without claiming a fresh run under a
different executable.

## Native campaign status

Campaigns 01 and 02 were operationally invalid and are retained for provenance,
but excluded from model-quality counts. Campaign 01 lacked the pinned CUDA
library search path; it stopped while preserving unknown reservations. The
only recorded device observation was 181 MiB GPU memory. Campaign 02 reached
model planning but was rejected before a valid native run because its runtime
allowed-tool policy included unsupported Search and the request exceeded the
worker frame limit.
The policy was corrected by removing Search from all arms and adding regression
coverage for both native grammar states while retaining Cargo. Neither invalid
campaign supports a model-quality acceptance claim.

Campaign 03 uses the corrected policy and a fresh host executable with SHA-256
`d01da5a8a91945f8b47ff2a40769437b8ae2486349745f10a8c8cf2cb4328321`. It uses
the original profile settings (context 4,096; input 3,584; output 512), the
same model sampling configuration, and corpus revision 3. At the time of this
record, three diagnostics had been rejected: absolute-value used 5 model
requests, uppercase-string used 12, and existing-negate used 12. All used zero
tool effects and have zero remaining reservations. The fourth diagnostic,
existing-colon, and subsequent preparation rows were still pending. The
new-application slice accepted zero of its two diagnostics, so the diagnostic
gate already fails. These are coding diagnostic attempts; no pilot coding
attempt is permitted and promotion is blocked. The campaign continues the
predeclared four diagnostics and 48 preparation-only rows. Final counts must
be added from the immutable report before this record is complete.

The campaign artifact root is
`/home/kmosoti/.cache/metron-alpha1/artifacts/intent-contracts-v3-study-01/campaign-03`.
The earlier invalid campaign artifacts are retained under the same study root.
Do not conflate these private run artifacts with product source or alpha.1
acceptance evidence.

## Portable checks and limitations

The integration owner reported the full workspace test suite passing with 300
passed, 0 failed, and 8 ignored, plus `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets -- -D warnings`. A real-tools smoke also
passed in 7.99 seconds using the existing pinned catalog and disk-backed
bubblewrap/systemd/cgroup setup, but its evidence origin was
`ImportedRecordedReplies` / test-only; it is not native generation. These
checks ran during the native campaign; device and timing observations are
operational only and do not qualify model fit or close alpha.1 native coding or
resource acceptance.

**Relations:** informs [ADR 0028](../../docs/adr/0028-experimental-intent-contracts.md),
[the coding contract guide](../../docs/coding.md), and the open alpha.1 gates in
[D-0005](../decisions/D-0005-alpha1-delivery.md). Revisit this record after
campaign 03 has a sealed final report and its exact attempt counts and
disposition are recorded. Human ratification remains pending.
