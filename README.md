# Metron

Metron is a Rust-native agent and execution harness with explicit authority,
bounded effects and durable replay.

[![ci](https://github.com/kmosoti/Metron/actions/workflows/ci.yml/badge.svg)](https://github.com/kmosoti/Metron/actions/workflows/ci.yml)

- [metron-agent](crates/metron-agent): pure task coordination and typed proposals.
- [metron-harness](crates/metron-harness): independently usable admission,
  orchestration and standalone planning.
- Supporting crates provide the host, SQLite state, scoped tools and local
  inference through a separate mistral.rs worker.

All crates share the root Cargo workspace, lockfile and Rust 1.100 toolchain pin.
See the [architecture diagrams](docs/architecture/overview.md) for component and
execution boundaries.

## Use and develop

[Agent and harness usage](docs/agent-harness.md) describes the composed host.
[Bounded coding](docs/coding.md) covers task, policy, tool and model bindings.

```sh
cargo build --locked -p metron-host
cargo test --locked --workspace
```

The host builds the `metron-runtime` CLI. Build the local inference worker
separately with the required backend feature; a host build does not select CUDA.
The repository pins `beta-2026-11-07` (Rust 1.100.0-beta.12).

Alpha.1 is in development. Local LFM3 Q4 CUDA generation is verified; useful
model-written coding and all required resource gates have not yet passed.
[Issue #7](https://github.com/kmosoti/Metron/issues/7) tracks release acceptance.
Historical sum journals have private read-only replay support; sum execution is
not a public product workflow. See the compatibility limits in the knowledge
records before making claims about legacy profiles.

## Research and contribution

The [knowledge index](knowledge/README.md) holds research and dated observations.
Historical Boolean-laboratory findings remain in the
[knowledge archive](knowledge/archive/laboratory/README.md), tied to their
source revision; the laboratory is not part of the current product or its
development commands.

Read [CONTRIBUTING.md](CONTRIBUTING.md) for development, review and human release
procedures. Architecture diagrams accompany human review.
