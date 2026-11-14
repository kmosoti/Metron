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
allowed-tool policy included unsupported Search. Request binding rejected
that policy, surfacing a misleading frame-size diagnostic; an actual frame
overflow was not established. The policy was corrected by removing Search
from all arms and adding regression coverage for both native grammar states while retaining Cargo. Neither invalid
campaign supports a model-quality acceptance claim.

Campaign 03 used the corrected policy and a fresh host executable with SHA-256
`d01da5a8a91945f8b47ff2a40769437b8ae2486349745f10a8c8cf2cb4328321`. It used
the original profile settings (context 4,096; input 3,584; output 512), the
same model sampling configuration, and corpus revision 3. All four native
diagnostics were rejected: new absolute-value (5 model requests, 0 tools), new
uppercase-string (12, 0), existing-negate (12, 0), and existing-colon (12, 11).
Across diagnostics there were 41 model observations, 42,581 prompt tokens,
3,155 completion tokens, and 11 tool effects; no diagnostic reservations
remain. Both slices accepted zero of two diagnostics, so the primary verdict
is `diagnostic_gate_blocked`. The selected arm is null and there is no
architecture-assistance finding. No pilot coding effect was admitted.

The campaign then recorded all 48 predeclared preparation-only rows. Each had
one admitted request, `unknown_hold=true`, `quality_failure=false`, no raw
response, no clarified stage, and zero pilot coding effects. All 48 rows are
`UNASSESSABLE`; there was no semantic comparison. Sixteen literal raw-readiness
flag matches (four per arm) are absent-output false flags matching four
`BLOCKED` labels; they are not semantic accuracy. The campaign therefore
supports a negative diagnostic-gate result but no pilot coding-quality
comparison across preparation forms or architecture assistance.

The immutable campaign report SHA-256 is
`59e7cb0151aa021298ced7d3cb473dce2a39d328015918464b54bee9c83b2657`; its
canonical configuration digest is
`cab1634ebe7c00ce5c1fb6972645b09a914b09ac4e6cc9e09a25ed7d9a854054`. The
independent annotation binding and offline finalizer completed with
`diagnostic_gate_blocked`, no selected arm, and no architecture finding. The
CLI returned exit code 4 for this expected blocked disposition, with empty
stderr; finalization caused no model or tool reruns. The annotation file
SHA-256 is
`52518761eae2eaf91dafe1c29c256fa2fb4601b261ae6244e0f3dabdbfd121bb`; the
canonical annotation-set digest is
`fefbe7dbd0d738e947034193a8ebc745178f0fe07c16439b019aac46b8b0a304`. The
finalized receipt digest is
`abea3256689ab8b50a382ec03ba5c3d9549ac1aa5a06e0f71d7a0777f0797e89`, with
raw file SHA-256 `4afb5f35f3b114092349f04774a0e8d9ce392d099879d796ed3d9a47649ee921`.
The finalized experiment-report file SHA-256 is
`1c116394dabdbf1659fe354c2eb895419743f832b7a783ccb309fb035442a9be`. These
checks establish structural and provenance consistency within the finalizer's
scope, not cryptographic attestation or semantic correctness of the model
outputs. The public contract `run` and `resume` remain unconditionally
deferred; enabling them would require a separate reviewed change.

The campaign artifact root is
`/home/kmosoti/.cache/metron-alpha1/artifacts/intent-contracts-v3-study-01/campaign-03`.
The earlier invalid campaign artifacts are retained under the same study root.
Do not conflate these private run artifacts with product source or alpha.1
acceptance evidence.

Optional operational capture measured the whole command at 6,581.536 seconds,
with device-wide peak use of 1,872 MiB and minimum free memory of 3,986 MiB.
The capture includes other applications, and portable verification overlapped
the campaign. It does not qualify fit, latency, or throughput; KV/cache and
runtime-workspace breakdowns were not measured. The GGUF artifact size was
1,099,627,808 bytes and is distinct from GPU memory. Preparation used a
bounded 60-second profile deadline; the coding diagnostics used 300 seconds.
The cause of the unknown preparation outcomes was not established.

## Portable checks and limitations

The integration owner reported the full workspace test suite passing with 300
passed, 0 failed, and 8 ignored, plus `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets -- -D warnings`. A real-tools smoke also
passed in 7.99 seconds using the existing pinned catalog and disk-backed
bubblewrap/systemd/cgroup setup, but its evidence origin was
`ImportedRecordedReplies` / test-only; it is not native generation. The smoke
preceded the campaign, while final portable workspace checks overlapped part
of it. Device and timing observations are operational only and do not qualify
model fit or close alpha.1 native coding or resource acceptance.

**Relations:** informs [ADR 0028](../../docs/adr/0028-experimental-intent-contracts.md),
[the coding contract guide](../../docs/coding.md), and the open alpha.1 gates in
[D-0005](../decisions/D-0005-alpha1-delivery.md). The campaign and offline
finalization are recorded above. Alpha.1 native coding and resource acceptance
remain open. Human ratification remains pending.
