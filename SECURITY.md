# Security Policy

## Supported Versions

| Version | Supported | Notes |
| --- | --- | --- |
| `0.1.1-alpha.1` | Yes (best-effort alpha) | Current security-hardened alpha release |
| `0.1.0-alpha.1` | No | Upgrade to `0.1.1-alpha.1` |
| `< 0.1.0-alpha.1` | No | Pre-split development snapshots |

Metron is currently pre-`1.0` alpha software (`v0.1.x-alpha.*`). Security fixes land on protected `main` and are released only from tagged commits that are verified ancestors of `origin/main`.

## Reporting a Vulnerability

**Do not report security vulnerabilities through public GitHub issues, discussions, or pull requests.**

Please report suspected vulnerabilities privately using GitHub Security Advisories for `kmosoti/Metron` ("Report a vulnerability" under the repository's **Security** tab), or by contacting the repository maintainers privately with:

- Affected crate(s), binary, or workflow (`metron-sandbox`, `metron-wasm-aot-loader`, `metron-mcp`, `metron-provider-http`, `metron-host`, etc.).
- Version/commit SHA, kernel/architecture (`uname -a`), and configuration/feature flags (`compiler`, `local-cpu`, etc.).
- Clear reproduction steps, proof-of-concept inputs, and security impact assessment.

Please allow reasonable time for triage, verification, and patch release coordination before public disclosure. Never include live API keys, OAuth tokens, or private user data in reports.

## Architecture & Trust Boundaries (`v0.1.1-alpha.1`)

Metron enforces strict separation across crates (`tests/architecture/tests/runtime_boundaries.rs`):

- Every workspace crate except `metron-wasm-aot-loader` compiles with `#![forbid(unsafe_code)]`.
- Pure protocol crates (`metron-protocol`, `metron-policy`) have zero I/O, network, process, or filesystem dependencies.
- Release builds are locked (`--locked`), workflow actions are pinned to 40-hex commit SHAs, tags must be ancestors of protected `main`, and distributed artifacts carry `SHA256SUMS`, `RELEASE-EVIDENCE.json`, and GitHub build-provenance attestations (`actions/attest-build-provenance`).

### 1. Lane A: Wasmtime 51 AOT Sandbox & `metron-wasm-aot-loader`

- **Compile/Load Privilege Split:** Untrusted `.wasm` compilation to `.cwasm` runs isolated inside `metron-wasm-compiler` (`metron-sandbox` `--features compiler`) under Landlock filesystem isolation (`ro` input only, `wo` output only) and `seccomp` (`deny_exec: true`), bounded by `MAX_EXECUTABLE_BYTES`. Production runtimes omit Cranelift and load only verified AOT artifacts.
- **Minimal `unsafe` TCB (`crates/metron-wasm-aot-loader/src/lib.rs`):** Kept strictly under 80 lines (79 lines) as the sole `unsafe_code` boundary in the workspace, enforcing magic seal, format version, target arch/triple, tunables match, bounded `take(max_bytes + 1)` BLAKE3 digest verification, and HMAC verification before calling Wasmtime `Component::deserialize_file`.
- **Execution Jail (`wasm_aot_jail`):** Delivers the 32-byte HMAC key and AOT image over an anonymous `stdin` pipe rather than key material on disk, seals the bounded `.cwasm` payload into a `memfd` capped by `max_cwasm_bytes`, enforces two-phase `seccomp` (`PROT_EXEC` denial after load), and executes without ambient host filesystem/network privileges.

### 2. Lane B: Landlock / Seccomp OS Jail (`metron-jail` in `metron-sandbox`)

- **Isolation Stack:** Combines rootless user namespaces, `HardRequirement` Landlock filesystem policies (fail-closed if kernel Landlock ABI requirements cannot be enforced), and `seccomp-bpf` syscall filtering (`deny_prot_exec`, `deny_exec`, socket restrictions).
- **Resource Control & Input Hygiene:** Uses `systemd-run --user --collect` cgroup v2 limits with strict `validate_unit_name` validation (rejecting traversal, options injection, and invalid characters) or direct rlimits fallback, closes non-stdio file descriptors via `/proc/self/fd`, reads jail specs via `O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC` with regular-file and size checks, seals tool inputs in `F_SEAL_WRITE` `memfd`, and writes receipts exclusively (`O_CREAT | O_EXCL | O_NOFOLLOW`, mode `0600`).

### 3. `metron-mcp` (`rmcp` Bridge)

- **Allowlisted Dispatch:** Executes only explicitly registered `SandboxedToolSpec` entries (Lane A AOT or Lane B native executables pinned by lowercase 64-hex SHA-256 digest and tool-name hygiene).
- **Pre-Sandbox Validation & Resource Limits:** Enforces bounded JSON Schema checks (including bounded nested object/array recursion depth), serialized payload byte limits before sandbox `memfd`/pipe allocation, cooperative cancellation, checked sequence tracking, SHA-256 argument digests, and bounded output receipts.

### 4. `metron-provider-http` (HTTP `ModelProvider`)

- **TLS & Redirect Policy:** Uses `reqwest` with `rustls` TLS and disabled HTTP redirects (`Policy::none()`) to prevent open-redirect SSRF pivots.
- **SSRF & Authority Guard:** Validates `EndpointConfig.base_url` against malformed authorities, embedded userinfo (`@`), query/fragment strings, unspecified addresses (`0.0.0.0`, `::`), link-local / cloud metadata addresses (`169.254.0.0/16`, `fe80::/10`, `metadata.google.internal`), while permitting explicit local loopback inference endpoints (`127.0.0.1`, `::1`, `localhost`).
- **Bounded Reads & Secret Hygiene:** Enforces `max_response_bytes` incrementally per streamed chunk for both standard responses, SSE streams, and non-2xx error responses before full buffering; protects resilience cooldown arithmetic against overflow; and omits raw bearer/API credentials from receipts, diagnostics, and error payloads.

## Out-of-Scope / Threat Model Assumptions

The following are outside Metron's standalone security boundary:

- Compromised host Linux kernel, kernel Landlock/seccomp/userns escapes, CPU hardware side-channels, or privileged local attackers (`root` or equal-UID `ptrace` capabilities outside jailed processes).
- Compromised upstream third-party LLM providers or malicious local model weights explicitly configured by the host operator.
- Operators explicitly granting broad host read/write mounts, network access, or unsandboxed tools to a session/profile outside Lane A/B boundaries.
- Pre-launch tampering of the host OS binary directory, `PATH`, or environment before `metron-host` starts.
