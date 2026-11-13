---
id: ADR-0027
type: decision
status: proposed
updated: 2026-11-13
added_by: agent
human_ratified: false
---

# 0027 — Stable Rust 1.100 toolchain pin

## Context and decision

[ADR 0019](0019-runtime-compatibility.md) selected the dated Rust 1.100 beta
and required separate qualification before switching to stable. Rust 1.100.0
stable was released on 2026-11-12 ([official release notes](https://doc.rust-lang.org/releases.html#version-11000-2026-11-12)).
This change adopts the exact `1.100.0` stable toolchain for the root workspace,
CI and sandbox process environment. Edition 2024 and the Rust 1.100 minimum stay
unchanged; the lockfile is unchanged.

The root CPU workspace passed formatting, locked all-target clippy and locked
workspace tests under this pin; see [O-0018](../../knowledge/observations/O-0018-rust-1-100-stable-workspace.md)
for exact commands and scope.

The toolchain change leaves protocol, persistence and authority boundaries
unchanged. Existing workspace and product-scope supersession records in
[ADR 0023](0023-first-class-agent-and-harness.md) and
[ADR 0024](0024-product-scope-and-laboratory-archive.md) continue to apply. The
[architecture overview](../architecture/overview.md) remains the component and
authority-boundary reference. This is workspace compiler evidence only; existing
GPU worker compiler provenance and native alpha.1 acceptance remain as recorded.

## Consequences

Stable Rust 1.100.0 is the repository's reproducible compiler pin. This decision
supersedes only ADR 0019's beta compiler selection and its separate stable
qualification requirement. It does not reinstate the workspace layout or
laboratory scope superseded by later decisions.
