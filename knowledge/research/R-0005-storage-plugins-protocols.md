---
id: R-0005
type: research
status: proposed
updated: 2026-11-10
added_by: agent
human_ratified: false
source_proposal: 4c3781daa7211efd39bcdb95dcb15491be1abe39
---

# 05 — Plugin system, storage and interop

## The Sans-IO split is physical, not rhetorical

`metron-protocol` and cognition logic expose serializable domain objects and pure decision functions. Their dependency graph **must not contain** `tokio`, `reqwest`, `rusqlite` or OS/browser adapters, and their source must not call `std::process` or ambient I/O. The composition root owns scheduling, dependency injection and port implementations. Alpha.0 uses synchronous bounded driver and store ports; asynchronous scheduling is a later design question.

**Do not hide an effect behind a “helpful” utility in the core.** A time provider, RNG, model client, terminal, filesystem, persistence backend or secret store must be an injected effect at an adapter boundary. Logical clock and seeded pseudo-random values may be explicit event inputs.

## Plugin categories and versioned contracts

```text
TrustedNativePlugin  -> ordinary Rust trait, statically linked and reviewed
WasmComponentPlugin  -> component-model/WIT guest, host-call capability filtered
ProcessAdapter       -> external sandboxed process, versioned IPC
ResourcePlugin       -> declarative role/persona/skill/preset data (not executable)
```

**The first extension release, alpha.3, uses statically linked trusted-native plugins.** Alpha.0 wires scripted drivers directly; alpha.1 wires the trusted local inference implementation directly, as specified in [R-0008](R-0008-local-inference-resource-profile.md). Wasm and process plugins require their own later evidence; native dylib hot loading has no ordinary stable Rust ABI.

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
  request_hash BLOB NOT NULL,
  lease_owner TEXT,
  lease_epoch INTEGER NOT NULL DEFAULT 0,
  lease_expires_at INTEGER,
  reservation_id TEXT
);

CREATE TABLE resource_budget (
  scope_id TEXT NOT NULL,
  resource TEXT NOT NULL,
  ceiling INTEGER NOT NULL CHECK (ceiling >= 0),
  spent INTEGER NOT NULL CHECK (spent >= 0),
  reserved INTEGER NOT NULL CHECK (reserved >= 0),
  state TEXT NOT NULL CHECK (state IN ('open', 'breached', 'suspended')),
  PRIMARY KEY (scope_id, resource)
);

CREATE TABLE budget_reservation (
  reservation_id TEXT NOT NULL,
  intent_id TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  resource TEXT NOT NULL,
  amount INTEGER NOT NULL CHECK (amount >= 0),
  state TEXT NOT NULL,
  PRIMARY KEY (reservation_id, scope_id, resource),
  UNIQUE (intent_id, scope_id, resource),
  FOREIGN KEY (scope_id, resource) REFERENCES resource_budget(scope_id, resource)
);

CREATE TABLE graph_projection_checkpoint (
  projection_id TEXT PRIMARY KEY,
  stream_id TEXT NOT NULL,
  applied_seq INTEGER NOT NULL
);
```

For open accounts, the admission transaction requires
`state = 'open' AND spent + reserved + requested <= ceiling`. This condition
belongs to admission. Settlement stores actual usage even when an external
provider exceeds its bound. It then marks the account `breached` and denies new
admissions. Receipt append must remain possible in that state. An unrepresentable
actual debit remains in the exact receipt payload with an incomplete balance
and suspended admission until reconciliation. Test over-limit settlement and
receipt retention explicitly.

The outbox, reservations, grant/admission revision and event append share the
same SQLite transaction boundary. Enable and verify foreign keys. Persisted
resource counters use the supported nonnegative signed 64-bit integer domain.
Task admission rejects values beyond that domain with a typed error. Check all
intermediate additions before writing, and test the maximum accepted value and
one above it through the public task input and database round trip.

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

## Toolchain and implementation language

P0 migrates both workspaces to edition 2024, MSRV 1.100, and the exact
`beta-2026-11-07` compiler pin. Stable 1.100 adoption requires separate
qualification. See [ADR 0019](../../docs/adr/0019-runtime-compatibility.md).

Metron-owned product code is Rust. Conventional SQLite includes a C engine.
Dependency manifests, binary artifacts and local model weights need individual
license review for the reference distribution. The local model profile must name
its exact weights, license, quantization and runtime before making a FOSS claim.

## Storage evidence and dispatch implications

SQLite documents one concurrent writer and explicit transaction modes. WAL
permits readers alongside a writer, with durability depending on configuration
and the storage system. For a local authoritative journal, select a supported,
patched engine version, keep the database and WAL together, set and verify the
chosen durability pragmas, and test bounded busy handling and recovery. The
research probe's system SQLite version is a recorded experiment environment,
not the selected product dependency. [SQLite transactions](https://www.sqlite.org/lang_transaction.html),
[WAL](https://www.sqlite.org/wal.html),
[atomic commit](https://www.sqlite.org/atomiccommit.html).

The database transaction makes event/state/outbox/budget updates atomic within
that authority. External tools and APIs need their own correlation/idempotency
semantics. An outbox relay can deliver a duplicate, so recording an intent and
holding a worker lease cannot establish exactly-once external behavior. Store
lease epochs for stale-worker fencing and leave post-dispatch ambiguity visible.
[AWS transactional outbox guidance](https://docs.aws.amazon.com/prescriptive-guidance/latest/cloud-design-patterns/transactional-outbox.html).

The proposed [harness admission contract](R-0004-harness-contracts.md) reserves
budgets in the same authority as dispatch admission. The
[contract probe](../observations/O-0003-runtime-contract-probes.md) tested two
SQLite connections contending for one unit. The actual Rust adapter still needs
concurrency, duplicate-request, settlement and real crash tests.

MCP's current Rust SDK uses Tokio, which belongs in an effect adapter. Its root
license records a transition with retained MIT contributions and separate
non-specification documentation licensing. Inspect the exact consumed release
and include required notices. ACP is an editor/agent boundary with version
negotiation, distinct from Metron's domain protocol. The
[pinned source ledger](R-0001-agent-harness-landscape.md) records the canonical
locations and reviewed revisions.

GraphLite documents Sled-backed storage. A GraphLite projection needs its own
checkpoint and rebuild contract. Cross-engine atomicity is an unproven
assumption. Begin with SQLite relations, then compare correctness, query
latency, projection lag, rebuild time and operational cost before adding another
engine. [GraphLite documentation](https://docs.rs/graphlite/latest/graphlite/).

## Open-source licensing

- All new Metron-owned source and public manifests: **Apache-2.0** (SPDX header where suitable).
- Dependencies: automated license review plus manual checks for transitive source/binary/assets and notices; not every “open repo” has a compatible license.
- Default local profile: LFM3-1.7B Q4_K_M through mistral.rs, with shared weights for multiple roles and an explicitly qualified LFM3 CPU route. The selected weights use the custom LFM Open License v1.0. Audit source, backend binaries and weights separately; this profile is not an entirely-FOSS distribution. Exact identities and license sources are in [R-0008](R-0008-local-inference-resource-profile.md).
- Proprietary hosted models may be supported through optional adapters but do **not** make the end-to-end runtime entirely FOSS.
- Model prompt output and derived source require a provenance and IP policy; no copying upstream Pi/DSH implementation merely because it is readable.

## Plugin test contract

- Invalid or missing dependency fails registration before tool advertisement.
- Plugin cannot read host credentials except through explicitly granted resource-scoped handles.
- A request to widen grants is denied regardless of persona, role, manifest or tool description.
- A failed startup leaves no partially registered tools.
- A pinned running task is not switched to a new plugin version by hot reload.
- A rebuilt graph projection at watermark n yields the same deterministic representation as full replay through n.

## Provenance and relations

- derived-from [original 05-plugins-memory-protocols.md](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/05-plugins-memory-protocols.md)
- indexed-by [runtime research and sequence](README.md)
- reviewed-against [original proposal findings](../observations/O-0001-runtime-proposal-review.md)

## Revisit when

Implementation evidence, a changed workload, a source revision or a failed acceptance
criterion contradicts the proposed contract. Record the new evidence and update
linked decisions before expanding scope.
