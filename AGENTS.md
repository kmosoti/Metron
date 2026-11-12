# Working in Metron

Metron develops a Rust-native agent and execution harness. Read
[CONTRIBUTING.md](CONTRIBUTING.md), the [knowledge index](knowledge/README.md)
and [architecture overview](docs/architecture/overview.md) before substantive
changes. Historical Boolean-laboratory evidence is in
`knowledge/archive/laboratory/`; its executable sources and fixtures are not
part of the current product.

## Product boundaries

- `metron-agent` and `metron-protocol` keep coordination and typed contracts
  free of ambient I/O. The harness coordinates bounded effects; the host
  composes concrete tools and verification; SQLite owns durable admission,
  receipts and replay; the local supervisor owns worker lifecycle; only the
  worker links the pinned inference SDK.
- Keep proposal, grant, admission, effect, receipt/unknown outcome, settlement
  and independent artifact verification as distinct authority transitions.
  Unknown outcomes retain reservations and are not redispatched automatically.
- Keep filesystem mutation confined and CAS-bound. Scoped memory may provide
  evidence but never grants execution authority. Model output, roles and notes
  do not grant authority.
- `metron-testkit` is development-only support. Its fixtures do not define
  product task architecture. The current alpha.1 gates remain open until
  independently accepted native coding and required resource evidence pass.
- Keep the `sum-v1` compatibility boundary explicit; it is not a general task
  framework. Preserve legacy journal/read-only replay behavior unless a
  separately reviewed migration establishes compatibility.

## Layout

The current product crates live under `crates/`: `metron-agent`,
`metron-harness`, `metron-protocol`, `metron-host`, `metron-store-sqlite`,
`metron-inference-local`, `metron-inference-worker`, and development-only
`metron-testkit`. `tests/architecture` checks product dependency and authority
boundaries. Architecture and interface documentation lives in `docs/`; dated
evidence and decisions live in `knowledge/`.

## Development and verification

- Run `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace`; CI runs the full product checks.
- Preserve exact journal/replay semantics, one-shot admission, receipt-based
  accounting, crash recovery, path confinement, CAS mutation, SDK isolation,
  and independent verification. Changes to wire formats, durable state or
  authority boundaries require an ADR and focused regression evidence.
- Distinguish model weight artifact bytes, KV/cache state, runtime workspace
  and measured device memory. A trained context limit or KV estimate alone
  does not establish physical fit. Permit larger trained contexts only when
  the selected runtime path and observed physical headroom support them.
- Record decisions as new ADRs; do not rewrite accepted ADRs. Priors are
  hypotheses and need a primary source or discriminating test. Keep historical
  laboratory results explicitly scoped to their original revision/workload.
- Keep wall-clock time out of decisions and deterministic hashes.
- Do not add a generic task DSL or workflow framework without a concrete,
  reviewed product requirement.

## Collaboration and cleanup

For this product-core effort, Luna (GPT-6 Luna, medium) owns knowledge and
documentation; Sol (GPT-6.1 Sol, medium) owns product implementation; Sol
(GPT-6.1 Sol, high) owns architecture synthesis and direction. Use high effort
for complex design synthesis, medium for routine implementation and review;
never use max or ultra for review. Coordinate path ownership before edits.

Remove task-owned temporary execution/build artifacts after their work is
complete and provenance is retained. Preserve shared model, SDK and kernel
caches, active immutable executables, journals, sealed audits, and anything
with unresolved replay or compatibility obligations. Do not keep every
executable revision by default; preserve a concise provenance record and the
specific artifacts required by active validation.

## Review and release

Read [CONTRIBUTING.md](CONTRIBUTING.md) for branch and review workflow. Keep
human release approval mandatory for `main`; agents do not approve or bypass
that gate. Human-review changes include a current architecture diagram showing
dependencies and authority boundaries. Never claim incomplete alpha.1 gates as
passed.
