---
title: metron-harness — execution and computer use product
status: PROPOSED
---

# 04 — Harness contract

## Product boundary

`metron-harness` owns **how and where authorized operations execute**, not why a goal is worth pursuing. It can run standalone using a human-supplied task, typed plan or interactive CLI. An agent supplies requests through the shared protocol and sees only evidence/results.

### Required subsystems and authority

```text
HarnessHost
├── Task admission (schema, principal, authority, budget)
├── Context builder (bounded, source-tagged material)
├── Model adapter (external request/reply; no hidden provider in cognition)
├── Tool registry (typed tools, schemas, human-readable affordances)
├── Authorization kernel (NOT a replaceable plugin)
├── Effect scheduler / outbox (leases, idempotency, cancellation)
├── Effect drivers (workspace, subprocess; later browser/desktop)
├── Receipt + event journal (cost, request, result, uncertainty)
├── Verification runner (independent criteria)
└── Human approval port (out-of-band authority)
```

The trusted authorization kernel checks every effect **at the lowest supported call boundary**. Tool wrappers, context instructions or “please ask first” prompts are not enforcement.

## Effect lifecycle: exact states

```text
Requested
  -> Validated
  -> Denied | AwaitingApproval | Authorized
  -> PersistedIntent
  -> Dispatching
  -> ObservedSuccess | ObservedFailure | Uncertain
  -> IndependentlyVerified | Rejected | NeedsReconciliation
```

A running process exit code 0 proves only that the process reported success. It does not prove tests passed, output is correct, external service state converged, or an invoice was paid.

**Crash-window matrix**

| Crash point | Durable record | Recovery |
|---|---|---|
| before admission commit | no accepted intent | caller may submit again with same request ID |
| after outbox commit, before effect | pending intent | leased worker dispatches once; retries are policy-bound |
| after dispatch, before receipt | dispatch recorded, outcome unknown | reconcile using external correlation/idempotency token; no blind repeat |
| after receipt, before verification | observed effect recorded | re-run pure verifier or controlled read-only verification |
| after verification, before response | verified outcome stored | return existing result without reexecution |

No general claim of exactly-once effects. For a non-idempotent API without reliable status lookup, abstain and require operator action.

## Tool interface

Every tool declaration includes:

```text
ToolSpec {
    tool_id, version, description,
    input_schema, output_schema,
    effect_class, resources_touched,
    requested_capabilities,
    timeout, budget,
    verification_spec
}
```

Effect classes: `ReadOnly`, `WorkspaceMutation`, `ExternalMutation`, `HighRisk`. Classification is metadata used by the policy engine, not a grant.

Tool output carries structured status, stdout/stderr or artifact hashes, truncation flags, resource usage, exit signals and the receipt ID. Never silently truncate logs while claiming full inspection.

### ACI design lessons from SWE-agent

Offer ergonomic primitives with bounded outputs:
- list/search files with capped results and file names, not entire trees;
- read slices of files by line range;
- apply a structured patch, then inspect resulting diff;
- run checks with timeouts and summarized output plus trace artifacts;
- surface explicit messages when there is no output;
- report syntax errors and command failures as typed observations.

Generic shell access may remain available **only within an actual OS sandbox**; a model prompt allowlist is not a sandbox.

## Authorization and sandbox: fail closed

**Authority inputs:** principal, task ID, scoped path roots, tool/method, network destinations when supported, command execution policy, CPU/time/memory/token/money budgets, approval token and expiry.

**Enforcement:** use OS facilities matched to the target. On Linux, evaluate namespaces, cgroups, Landlock, seccomp and ephemeral isolated workspaces. Support is kernel/environment-specific and must be tested; never claim universal containment merely because one primitive is enabled.

**Path safety:** normalize and check roots, but also defend against symlink swaps / TOCTOU. Prefer relative handles to opened directory descriptors where available, and sandbox filesystem mounts rather than relying solely on string prefix comparisons.

**Process safety:** spawn explicit program/argv; avoid interpolating model text into `sh -c`; clear ambient environment and pass a narrow allowlist. No secret injection into prompts or logs. Prevent indirect escapes through package managers, network tools, editors, scripting interpreters and symlink/mount tricks.

**Network safety:** no outbound access in restricted workspace by default. Connection mediation must apply to **all** descendants and tool paths. Explicitly test a direct driver and a shell-based alternate route.

**Untrusted text:** email/web/tool output can propose data, not new instructions or grants. Mark provenance and preserve trust tiers through context assembly.

**Approval:** a human approves a specific action digest, principal, scope, target, TTL and budget. “Always allow this agent” is out of scope for first alpha. The agent cannot forge or persist its own approval.

## Model and context pipeline

The model adapter takes a typed request with model identifier, message sequence, tool descriptors, output schema, budget, temperature/sampling parameters where supported and a context snapshot hash. It returns bounded chunks and usage/finish reason with provider uncertainties.

Provider routing is **a policy plugin** after alpha 0, not part of the core. The complete FOSS reference profile must work with an openly licensed locally runnable model where hardware permits; remote proprietary providers are optional edges and not part of the FOSS claim.

The context assembler:
1. carries source/permission tags;
2. caps total bytes/tokens and allocates budgets by priority;
3. distinguishes trusted host instructions from untrusted evidence;
4. never includes secrets by default;
5. records which evidence refs were supplied;
6. keeps opaque/provider-specific prompt formats at the driver boundary.

## Computer use extension plan

Do not conflate shell execution, browser automation and graphical desktop control. They have different observational limits and risk.

| Driver | Minimum observable state | Extra risk | Release |
|---|---|---|---|
| Workspace read/write | relative files, diffs, hashes | unintended mutation | alpha 1 |
| Terminal | argv, cwd, env allowlist, exit/stream | arbitrary code & descendants | alpha 1 under verified sandbox |
| Browser DOM | URL, DOM snapshot, navigation events | prompt injection, cross-origin data | alpha 3 optional |
| Browser screenshots | pixels, click/type, screenshot refs | coordinate drift, hidden overlays | alpha 3 optional |
| Desktop | window focus, screen capture, input events | global OS access, secrets | alpha 4, gated independent threat review |

Computer-use actions MUST include a precondition about the observed target and a postcondition or explicit `Uncertain`. A click does not establish that a workflow completed. Human takeover and emergency stop are first-class host operations.

## Receipts, observability and evaluation

A receipt should include `intent_id`, `task_id`, `principal_id`, `driver_id`, `request_hash`, `response_hash`, `started_at`, `ended_at`, `resource_use`, `status`, `artifact_refs` and `external_correlation_ref` where supported. Volatile timestamps may be stored but excluded from deterministic replay hash projections.

Keep a durable journal for authoritative state. High-rate optional telemetry may be dropped with explicit loss counters, but **never** substitute a lossy ring buffer for authoritative effect/approval events.

Independent verification can be deterministic unit tests, schema checks, diff policy, observable UI postconditions or an explicitly human-approved result. Model-as-judge may provide diagnostic evidence but cannot alone grant a high-risk acceptance state.

## Harness exit contract

- Standalone CLI can run a bounded task without importing `metron-agent`.
- Every attempted effect is policy checked, receipted or marked uncertain.
- No unauthorized write/network action occurs through direct or indirect tool paths in adversarial tests.
- Cancellation terminates process trees or reports inability to guarantee it.
- A suspended/restarted job resumes from durable state without duplicating confirmed operations; unresolved side effects are flagged.
- Each output is an artifact + independently checkable acceptance result, not just conversational prose.
