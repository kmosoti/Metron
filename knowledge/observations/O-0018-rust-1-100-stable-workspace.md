---
id: O-0018
type: observation
status: observed
updated: 2026-11-13
added_by: agent
human_ratified: false
---

# Stable Rust 1.100 workspace verification

On the stable `1.100.0` toolchain (`rustc 1.100.0`, commit `0ef1bc386`, dated
2026-11-09; Cargo `1.100.0`, commit `3acc79c0c`, dated 2026-11-05), the root CPU
workspace passed:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked` — 271 passed, 0 failed, 7 ignored across
  35 test summaries.

This verifies the workspace on the stable compiler pin. Tests were run without
CUDA or model-device execution; seven ignored tests include six physical sandbox
checks and one pinned-tokenizer check. The GPU image compiler provenance and
native alpha.1 acceptance evidence are unchanged. No edition, MSRV, dependency
lockfile or compatibility changes were made.

Relations: records workspace evidence for [ADR 0027](../../docs/adr/0027-stable-rust-1-100-toolchain-pin.md),
which supersedes only the compiler pin and separate stable qualification
requirement in [ADR 0019](../../docs/adr/0019-runtime-compatibility.md).
