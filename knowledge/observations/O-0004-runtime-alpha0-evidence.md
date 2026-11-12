---
id: O-0004
type: observation
status: bounded-product-evidence
updated: 2026-11-10
added_by: agent
human_ratified: false
---

# Scripted runtime alpha.0 evidence

## Observed behavior

The product now exercises the interfaces that the standalone feasibility probes
in [O-0003](O-0003-runtime-contract-probes.md) modeled. The isolated runtime uses
Rust `beta-2026-11-07` (1.100.0-beta.12), edition 2024, and the committed product
lockfile. The workload is the scripted `sum-v1` fixture, not a coding benchmark or
live language model.

The executable evidence is retained in the product tests:

| Property | Reproducible evidence |
| --- | --- |
| Pure, bounded decisions | [Agent tests](../../crates/metron-agent/src/lib.rs), including decision caps and uncertain outcomes |
| Transactional accounting and recovery | [SQLite tests](../../crates/metron-store-sqlite/src/engine.rs), including concurrency, stale leases, exact unposted vectors, collisions and reconciliation fences |
| Opaque consumed execution authority | [Policy doctests](../../crates/metron-harness/src/policy.rs), external consumer plus construction, cloning, deserialization, default and reuse failures |
| Process-kill recovery | [Crash tests](../../crates/metron-host/tests/crash.rs), killing actual children after outbox, admission, synced publication, receipt/inbox and verification commits |
| Replay beyond hash validity | [Behavioral replay negative control](../../crates/metron-host/tests/behavior_replay.rs), changing a transition and recomputing a valid journal chain |
| Read-only audits and separate artifact checking | [CLI audit tests](../../crates/metron-host/tests/cli_audit.rs), corrupted artifact/cache and bounded audit capacity |
| Approval grants no execution by itself | [CLI approval test](../../crates/metron-host/tests/cli_workflow.rs) |
| Unknown and large usage remain visible | [Reconciliation tests](../../crates/metron-host/tests/reconcile_unknown.rs), including exact decimal `184467440737095516160` |

An independent consumer of the public store API additionally checked two SQLite
connections competing for the final principal budget unit: exactly one admission
and one durable denial. It checked an exact 64 KiB receipt whose usage could not
be posted, preservation of all six task/principal counters, new-ID semantic
no-ops and frozen duplicate results, a coherent WAL snapshot while another
connection advanced time, cross-task producer-ID collisions, and an actual
tariff overrun with budget headroom. Twenty large no-op commands forced journal
pagination across the 1 MiB aggregate boundary. These checks passed against the
reviewed store candidate `9979cfe` ([PR #16](https://github.com/kmosoti/Metron/pull/16)).
An online backup of the nonempty unposted-usage database reopened read-only with
the same complete projection; creating a backup over an existing file failed.
Independent read-only audits also agreed with both stages of a two-intent
reconciliation fixture: compatible resolved evidence cleared holds, while later
evidence restored them.

## Independent CLI reproduction

Against combined candidate `5bc6f4f`, two fresh runs per mode used this task:

```json
{"schema_version":1,"task_id":"independent","principal_id":"operator","objective":"Sum the fixture values","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null},"max_decision_steps":8}
```

Build and run from `products/runtime`:

```sh
cargo build --locked -p metron-host
cargo run --locked -p metron-host -- run --task task.json --run-dir fresh-run --mode agent
cargo run --locked -p metron-host -- replay fresh-run
cargo run --locked -p metron-host -- verify fresh-run
```

Repeat with a second fresh directory, then repeat both with `--mode harness`.
Each run, replay and verify exited zero and reported verified success. All five
audit flags were true on verify. Repeated heads matched within each mode:

| Mode | Replay head |
| --- | --- |
| agent | `32402b9901bffdbf97e733d1b7ce5f50ecc18b1464b0926e34680562e6969b40` |
| harness | `fb8e1eb8500b0b36e40ce451d77012dc298e24059dda686bb18010a9fd0e80b6` |

Each run produced one 103-byte artifact with SHA-256
`0263b7cb6102022b514b982db0ef741055954c84d0022f7e7bd998a1d1eecf19`,
independently calculated from file bytes. These heads bind the complete task,
mode and version; changing any of them can change the head.

The combined candidate passed product workspace formatting, Clippy with warnings
denied, tests and doctests. Laboratory formatting, Clippy, tests, normal and
capped smoke runs and journal verification also passed during integration. Their
unchanged smoke heads were respectively
`84146217504d2265196fb2172c2f48e30d7a2ba887db0b7c4537b4d0282d6779`
and `bfbaf96e27d2b0608680f37450ec987bd6bbecbbedeea6f9765ba106672e21fe`;
the capped workload retained its expected disagreement.

## Interpretation and limits

This supports deterministic replay, host-owned admission and bounded recovery
for the scripted workload and tested checkpoints. It does not establish model
quality, coding capability, OS confinement, power-loss guarantees or comparative
memory/performance results. There is no measured LFM3 footprint or batching
benchmark in alpha.0. Audit capacity is an explicit operational limit, not a
claim that total process memory is 1 MiB.

## Relations and revisit condition

- implements-contracts-from [D-0003](../decisions/D-0003-runtime-implementation-sequence.md)
- refines [O-0003](O-0003-runtime-contract-probes.md)
- supports-within-scripted-scope PH-01, PH-02 and PH-04 in the [priors ledger](../../docs/research/priors.md)
- specified-by [ADR 0020](../../docs/adr/0020-runtime-alpha0-execution.md)

Revisit when introducing live inference, broader effects, new storage formats or
additional recovery guarantees. Qualify new workloads independently.
