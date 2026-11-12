---
id: O-0016
type: observation
status: observed
updated: 2026-11-12
added_by: agent
human_ratified: false
---

# V6 comparison proposal evidence and disposition

PR [#43](https://github.com/kmosoti/Metron/pull/43), observed as closed and
unmerged on 2026-11-12, proposed a model-only native V6 comparison over a
four-case synthetic read/edit corpus. Its report is explicitly
nonselecting: it does not execute model proposals or promote a profile.

The PR body reports Rust beta-2026-11-07 offline tests at tree
`83ebf3e60889af37c18d57bf68ad7b1a825fda02`: 6 comparison-admission tests,
6 fake-worker transport tests and 8 host comparison tests passed; focused
Clippy, rustfmt and `git diff --check` passed. It reports no GPU, SDK or kernel
build/run. These are PR-authored validation claims, not a current exact-head
independent rerun. They establish no coding capability, actual backend batch
membership, cache hit, model equivalence or profile selection.

Preserve two commit identities: remote PR head
`2cd337c83615f11a85efaab04d480a6cef2999a3` (base `0066f906d274bb087034dd0c6f2b1759ccddf723`),
and local commit `76063e1eaa90dfb4e3746af9057bd779326846b7`. Their source
trees are identical at `83ebf3e60889af37c18d57bf68ad7b1a825fda02`; their
commit metadata differs. These commit IDs and the shared tree hash preserve the
proposal's source provenance.

The V6 comparison command and test-specific public
`CodingComparisonCase`/`create_coding`/`admit_coding` APIs were confined to the
PR prototype; they never entered the current product base and remain excluded.
Current file-backed comparison and qualification authorities reside in
`metron_inference_local`, while the production harness consumes explicit effect
and verifier ports. The proposal is not a product acceptance path for its
four-case corpus and does not justify adding a fixed-case API or framework. Its
reported short/long cases and concurrency schedules do not qualify native
coding, batching, cache behavior or long-context fit.

Relations: linked to [O-0015](O-0015-native-coding-v10-and-context.md) for
current native run outcomes and to the product-scope decision in
[ADR 0024](../../docs/adr/0024-product-scope-and-laboratory-archive.md).
Revisit only if a concrete product requirement needs model-only efficiency
validation and a design using the current typed effect boundary is prepared.
