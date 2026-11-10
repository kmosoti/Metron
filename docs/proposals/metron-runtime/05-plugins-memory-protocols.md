---
title: Metron plugins, memory, protocol and licensing contracts
status: PROPOSED
---

# 05 — Plugin system, storage and interop

## The Sans-IO split is physical, not rhetorical

`metron-protocol` and cognition logic expose serializable domain objects and pure decision functions. Their dependency graph **must not contain** `tokio`, `reqwest`, `rusqlite`, `std::process` or OS/browser adapters. The composition root owns async scheduling, dependency injection and port implementations.

**Do not hide an effect behind a “helpful” utility in the core.** A time provider, RNG, model client, terminal, filesystem, persistence backend or secret store must be an injected effect at an adapter boundary. Logical clock and seeded pseudo-random values may be explicit event inputs.

## Plugin categories and versioned contracts

```text
TrustedNativePlugin  -> ordinary Rust trait, statically linked and reviewed
WasmComponentPlugin  -> component-model/WIT guest, host-call capability filtered
ProcessAdapter       -> external sandboxed process, versioned IPC
ResourcePlugin       -> declarative role/persona/skill/preset data (not executable)
```

**Only statically linked trusted-native plugins ship at the first alpha.** Wasm and process plugins are separate later milestones; native dylib hot loading is not an ordinary stable Rust ABI.

### Registration lifecycle

```text
Discovered -> ManifestValidated -> DependenciesResolved
           -> PolicyChecked -> Initialized -> Active
           -> Draining -> Stopped

error path: any state -> Failed -> CleanupVerified
```

Plugin manifests name stable id, version, host API range, exported services, required services, scopes, requested capabilities, resource ceilings and artifact digest. A manifest's requested capabilities are not authority.

The registry rejects cyclic required dependencies, ambiguous service ownership, incompatible versions, missing required services and scope breaches. Plugin start failure unwinds its registrations; when a task is in progress, a plugin cannot be hot-replaced in place. Pinned snapshots resume on their original implementation or explicitly enter migration/repair.

Do not give plugins a mutable global service context that exposes every secret and effect. Expose a **capability-filtered host facade** and opaque handles. Revocation and cancellation must be enforced independently of plugin code.

### Presets versus policy

Presets are versioned requests for composition. They combine a role, persona, cognition strategy, memory view, model route and harness tool surface. The authority-bearing policy kernel decides the effective grants after principal/task/scope intersection.

```toml
[preset]
id = "metron.investigator"
schema_version = 1

[agent]
role = "investigator@1"
persona = "analytical@1"
cognition = "evidence-first@1"

[harness]
backend = "local-scripted"
model_route = "scripted@1"
sandbox = "restricted@1"
requested_capabilities = ["workspace.read"]
```

This is a composition example, **not** a persistent security policy file. Host enforcement policy is separate, signed/authorized and not writable by model-facing code.

## Interoperability boundaries

- **MCP:** implement tools/resources via the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk) at harness edges. MCP transport is not equivalent to local filesystem authority; every MCP call still undergoes policy checks.
- **ACP:** optional editor client/agent-host integration through the [Agent Client Protocol](https://github.com/zed-industries/agent-client-protocol); treat as presentation/control edge, not internal semantics.
- **Wasm Components:** later plugin ABI using WIT and [Wasmtime](https://docs.wasmtime.dev/). Guest sandboxing does not make host-call permissions safe automatically.
- **Native Metron wire:** versioned task/event/receipt messages. Internal protocol should remain transport-agnostic, supporting in-process test transport, Unix socket, and later authenticated remote RPC.

Do not invent an A2A or general distributed-agent protocol in v0.1. Validate which external standards materially help interoperability before adding adapters.

## Canonical memory: SQLite first

**Canonical journal and entity state:** SQLite, with a single authoritative stream of ordered events and explicit transactional outbox. Use WAL mode where supported and appropriate, proper busy handling, explicit schema migrations, checkpoints and backup/recovery testing.

**Graph memory:** initially SQLite node/edge relations and indexed queries; optionally a rebuildable GraphLite projection later. [GraphLite](https://docs.rs/graphlite/latest/graphlite/) describes a standalone Sled-backed graph database, not an extension of SQLite's transaction engine. Two-engine atomic commits must not be assumed.

Potential canonical tables (logical schema; implementation may normalize further):

```sql
CREATE TABLE event_stream (
  stream_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  event_id TEXT NOT NULL UNIQUE,
  schema_version INTEGER NOT NULL,
  kind TEXT NOT NULL,
  payload BLOB NOT NULL,
  prev_hash BLOB,
  event_hash BLOB NOT NULL,
  PRIMARY KEY (stream_id, seq)
);

CREATE TABLE effect_outbox (
  intent_id TEXT PRIMARY KEY,
  stream_id TEXT NOT NULL,
  dispatch_state TEXT NOT NULL,
  idempotency_key TEXT,
  external_ref TEXT,
  request_hash BLOB NOT NULL
);

CREATE TABLE graph_projection_checkpoint (
  projection_id TEXT PRIMARY KEY,
  stream_id TEXT NOT NULL,
  applied_seq INTEGER NOT NULL
);
```

The outbox transaction and event append must share the same SQLite database boundary. External APIs do not share that transaction. Recover uncertain dispatches by querying the external resource or escalating.

Graph projection invariant:

```text
GraphView(stream, n) = Project(EventStream(stream)[0..n])
freshness(GraphView) = last_applied_seq
```

For graph reads requiring freshest state, either wait for the projection watermark, query canonical tables, or return `StaleProjection`. Never silently claim it represents the latest events.

### Memory query contract

A query states domain, principal, evidence provenance, time boundary, depth/size budget, freshness requirement and allowed knowledge scopes. The result includes source observations, transformation path, projection watermark and truncation metadata.

Delete and retention policy must cover canonical events **and** derived projections, caches, backups and embeddings. Append-only evidence and privacy deletion are in tension; define redaction/tombstone and key-destruction policies in an ADR before storing customer data.

### Storage provenance and value types

Observations are source-attested; claims are attributed to source; beliefs are revisions. Store source URI/file digest and trust class separately from natural-language content. Never treat retrieved text as a capability grant. Procedure snapshots and promotion history are distinct from “facts”.

## Review of proposed low-level memory primitives

| Optimization idea | Main failure mode | Initial design | Gate to reconsider |
|---|---|---|---|
| Raw zero-copy references from SQLite rows into GraphLite | SQLite row buffers have statement-scoped lifetimes; graph engine is separate | own the bytes/strings that cross transaction and async boundaries | profiler demonstrates copying dominates and lifetime-safe alternative exists |
| Self-referential graph context via raw pointers/`ouroboros` | pinning/lifetime complexity, invalidation, hidden aliasing | immutable `Arc<[u8]>` with offsets; borrowed slices only while owner exists | reproducible throughput gain after safe code review |
| Lock-free SWMR graph cache with `AtomicPtr`/epochs | reclamation races, ABA, operational complexity | bounded immutable snapshot cache; `RwLock` or `ArcSwap` after profiling | contention evidenced on representative traces |
| `SlotMap` + `SmallVec` adjacency index | local IDs not durable, cache line assumption untested | secondary in-memory traversal index where needed | measured graph query bottleneck |
| Lossy telemetry ring buffer | loss of critical event/receipt records | durable SQLite journal; bounded channels for optional telemetry | telemetry-only throughput requirement |
| `bumpalo` per-step arena | lifetime leakage and allocator complexity | normal owned values | allocation flamegraph shows material gain |

The default is **safe Rust and explicit ownership**. Do not tell coding agents that Rust 1.100 makes raw SQLite pointers safe or that `SmallVec<[EdgeKey; 8]>` always fits one cache line.

## Runtime versions and “pure Rust”

At the proposal date (November 10, 2026), [Rust Forge](https://forge.rust-lang.org/) lists **Rust 1.99 stable** and **1.100 beta**, targeted for November 12. The existing repository declares `rust-version = "1.97"`. A new product workspace should use an explicitly tested toolchain; do not silently raise the lab's MSRV. Use 1.99-compatible, stable Rust patterns until 1.100 actually ships and CI validates it.

**Important compatibility distinction:** writing all Metron-owned code in Rust is consistent with a dependency such as SQLite (whose original engine is C). A literally **100% Rust dependency closure** is incompatible with using conventional SQLite engines. Since SQLite was explicitly selected, the proposed requirement is “all first-party code Rust; interoperable FOSS libraries permitted.” If “zero native C even in dependencies” becomes hard, the storage decision must be reopened rather than described inaccurately.

## Open-source licensing

- All new Metron-owned source and public manifests: **Apache-2.0** (SPDX header where suitable).
- Dependencies: automated license review plus manual checks for transitive source/binary/assets and notices; not every “open repo” has a compatible license.
- Local reference configuration: entirely FOSS userspace stack and openly licensed local model weights, distributed subject to their own license terms.
- Proprietary hosted models may be supported through optional adapters but do **not** make the end-to-end runtime entirely FOSS.
- Model prompt output and derived source require a provenance and IP policy; no copying upstream Pi/DSH implementation merely because it is readable.

## Plugin test contract

- Invalid or missing dependency fails registration before tool advertisement.
- Plugin cannot read host credentials except through explicitly granted resource-scoped handles.
- A request to widen grants is denied regardless of persona, role, manifest or tool description.
- A failed startup leaves no partially registered tools.
- A pinned running task is not switched to a new plugin version by hot reload.
- A rebuilt graph projection at watermark n yields the same deterministic representation as full replay through n.
