---
title: Hypotheses, validation and failure analysis
status: PROPOSED
---

# 08 — Evidence ledger and test program

## Research discipline

Separate:
- **Verified in existing laboratory** (cite an in-repo reproducible report/ADR).
- **Observed in external source** (paper/repository documentation, not replication).
- **Derived** (logical consequence contingent on explicit assumptions).
- **Hypothesized** (requires experiment).
- **Rejected/deferred** (failed test, dominated, unresolved cost or risk).

No unverified hypothesis becomes an architectural invariant through repetition. The current lab's accepted [priors ledger](../../research/priors.md) remains authoritative for existing scientific claims; new product priors below must be reconciled there when a product ADR is accepted. Do not silently append product assumptions as measured laboratory facts.

## Product hypothesis ledger

| ID | Claim and origin | Status | Falsifying experiment / source |
|---|---|---|---|
| PH-01 | Pure Sans-IO transitions make behavior replayable and testable (architectural deduction) | proposed | inject same ordered events, seed, config/version and verify identical transitions; perturb timestamps only |
| PH-02 | Independent host grants block prompt/role/persona privilege escalation (security design) | proposed | injection through persona, skill, tool output, preset and delegated task; assert zero disallowed driver calls |
| PH-03 | A small tool loop plus ergonomic ACI can solve bounded coding tasks (SWE-agent/Pi) | evidence-supported, not measured in Metron | compare static basic tools against ergonomic file-view/edit/test tools on matched tasks |
| PH-04 | Durable outbox + reconciliation reduces duplicate irreversible effects (transaction engineering) | derived under external API assumptions | crash at every dispatch step, duplicate retries, ambiguous remote response |
| PH-05 | A SQLite nodes/edges projection suffices for v0 memory workloads | hypothesis | query depth, recall, p50/p95 latency and bytes at representative workloads vs GraphLite |
| PH-06 | GraphLite adds value to a SQLite-canonical agent memory store | **untested** | matched indexing/query benchmark with projection lag, rebuild time and double-engine ops burden |
| PH-07 | Static Rust plugins cover enough alpha use cases | hypothesis | implement 2 providers + 2 tools + 2 strategies without runtime downcasting or unsafe ABI hacks |
| PH-08 | Wasm Components improve third-party plugin security/isolation | plausible, not yet proven in Metron | hostile guest attempts forbidden host calls, resource exhaustion, version incompatibility |
| PH-09 | Task-local harness adaptation beats static configuration after matched total cost (Harness² motivation) | external authors report gains, Metron untested | B vs A randomized matched runs, with total model/tool budget controls |
| PH-10 | Cross-task improvement transfers to unseen tasks (Harness²) | external authors report gains, Metron untested | C vs B holdout evaluation, split hygiene and regression analysis |
| PH-11 | Learned model routing beats a carefully tuned deterministic control | contradicted in current Boolean lab; new domains untested | include simple/manual Bayes and cost-only policies; measure matched quality goodput |
| PH-12 | Self-referential SQLite/GraphLite zero-copy memory is a performance win | unsupported / unsafe as proposed | require proof of lifetime safety, profiler result and reproducible benchmark |
| PH-13 | Roles and personalities should be data/config, not autonomous agents | design premise | evaluate role switching under identical grants and agent identity, check behavior without cross-session privilege leak |
| PH-14 | Code-shaped specs reduce generation errors | plausible, **unverified** | randomized coder prompts: narrative vs type/path/test-shaped specs, same model/tasks/compute; measured compiles and contract violations |
| PH-15 | Full Rust source improves maintainability for this developer | preference, not general scientific fact | compare dependency, iteration and deployment cost with a smaller mixed-language baseline; keep Rust as deliberate product constraint |

## Experimental design: alpha 0–3

**Unit:** one task attempt with immutable task ID, strategy/config digest, model/adapter version, seed where applicable, environment digest, budget and observed outcome.

**Holdouts:** hidden acceptance fixture not exposed in prompts, memory, artifacts, caches, logs, tool source or internet. For code tasks, use unique task variants/commits, not only duplicated public benchmark issues.

**Baselines:**
- L0 scripted deterministic static policy (safety/replay negative control).
- L1 minimal human-driven standalone harness.
- L2 same underlying model and tools in a reference harness (Pi/Codex where accessible and ethically comparable).

**Metrics:** accepted-task rate (independently verified), blocked unauthorized attempts, false-success rate, cost per verified solution, latency p50/p95, number of tool calls, tokens, elapsed CPU/wall time, human intervention rate, recovery time, number of ambiguous effects, and storage footprint.

**Quality law:** never compare aggregate tokens/sec while silently allowing different verified task quality. Report verified goodput and failed-attempt costs. No design is better merely because it emits more agent messages.

## Adaptation experiment: A/B/C

A. **Static harness:** frozen prompt/context/skills/tools, no configuration search.

B. **Task-local adaptive:** Propose–Probe–Compare within current task, no persistent improvement library.

C. **Cross-task adaptive:** same as B, with proposed reusable lessons from prior **training** tasks only.

**Controls:**
- same base model and sampling distribution;
- same maximum total model calls, tokens, wall time and external tool budget, as well as same policy grants;
- separate training/validation/test tasks with verified hygiene; no candidate sees private test criteria;
- matched random task order and repeated seeds for stochastic systems;
- pre-register primary metric, comparison and acceptable regression margin;
- unmodified static strategy as negative control;
- report variance/confidence intervals and failed/regressed tasks, not only best run;
- measure transfer to fresh task families after configuration promotion.

**Decision:** C becomes default only if it improves verified utility against B on fresh tasks without significant security regression and within a justified cost budget. Otherwise keep adaptation experimental. The existing lab's negative learned-router result is a warning against assuming more adaptive machinery is better.

## Harness and autonomy adversarial matrix

| Attack/failure | Expected result | Priority |
|---|---|---|
| prompt injection in email/web/tool output | untrusted text cannot grant capabilities or rewrite policy | release-blocking |
| role/persona says “ignore policy” | same grant set; denial unchanged | release-blocking |
| agent requests tool through alternate path or nested interpreter | no bypass to file/net/secret | release-blocking before real shell |
| symlink/race across workspace root | host sandbox blocks escape | release-blocking before mutation |
| plugin tries to register “policy override” tool | registration rejected; no host policy replacement | release-blocking |
| child requests wider grant/budget | intersection enforced; denial receipted | before delegation |
| plugin fails during init/start | rollback registrations; no leaked handles | before plugin release |
| crash after remote mutation, before ack | `Uncertain` + reconciliation; no blind duplicate | release-blocking for external effects |
| double outbox dispatch / stale lease | one known result or explicit uncertain state | release-blocking |
| malicious evaluator leak via tool read/cache | candidate cannot access holdout labels | release-blocking for adaptive eval |
| evaluator and candidate use same model bias | record grader agreement separately from independent correctness | adaptation |
| modified snapshot promoted without new held-out test | promotion denied | adaptation |
| model returns “verified=true” without evidence | outcome rejected; no trusted success | all releases |
| lossy telemetry buffer fills | diagnostic loss counted; authoritative journal unaffected | instrumentation |
| graph index lags SQLite | freshness disclosed; query fails/waits/falls back | graph backend |
| customer data between tenants | no read, model prompt or trace leakage | before multi-tenant release |

Threat tests should include both direct and *indirect* actions. A policy that denies “delete file” but allows “run shell: rm” is not a policy.

## Model and hardware experiment scope

The local Fedora machine can host development and execution, while a small NixOS GPU node can test constrained inference. Do not assume 4 GB GPU VRAM or 8 GB host RAM can run frontier coding models well. Use recorded model version, quantization, context size, warm/cold performance and memory pressure.

For mobile and remote operation, do not expose unprotected host control on the public internet. Private network access is not a substitute for tool-level authorization or authentication.

## Rejected shortcuts

1. **“Exactly once” generic tool execution**: no general distributed guarantee across unrelated external systems; use durable intents + idempotency + reconciliation.
2. **Unbounded self-improving agent**: violates experimental separation and introduces hidden policy-mutation risk.
3. **Large plugin marketplace at alpha 0**: expands attack surface before core contracts stabilize.
4. **Unverified GraphLite cross-engine zero-copy**: references cannot outlive SQLite row statement buffers and two engines do not share a transaction.
5. **Static metrics without confidence or controls**: encourages Goodhart effects and overclaims.
6. **Code-shaped document as verification**: formatting may improve prompting; tests are the actual evidence.

## Reporting template

Every experiment report uses:

```yaml
report_schema: 1
proposal_revision: 0
source_commit: "<sha>"
task_set_hash: "<sha256>"
model_snapshot: "<id>"
toolchain: "<rustc --version>"
environment: "<verified OS and sandbox settings>"
baseline: "<configuration digest>"
candidate: "<configuration digest>"
budgets:
  model_calls: 0
  tools: 0
  wall_ms: 0
  currency_micros_usd: 0
primary_metric: "<name + verification method>"
results:
  baseline: null
  candidate: null
  uncertainty: null
security:
  unauthorized_effects: null
  unverified_successes: null
decision: "REACHED | CLOSED_NEGATIVE | DEFERRED"
notes: "<observed, derived, unresolved>"
```

Use actual measurements in place of placeholders. Inability to run a benchmark is recorded as “not run”, not as success.

## Final decision checkpoint

Before converting this proposal to an ADR or implementing a new alpha, answer with evidence:

1. Does the new product boundary preserve every accepted current lab invariant?
2. Does the proposed addition solve a tested pain that a smaller harness cannot?
3. Can the same task be run with another cognitive strategy without changing harness code?
4. Can the same cognition be run with a scripted harness without external I/O?
5. Is every external effect accountable, scope-restricted and explicitly uncertain when proof is missing?
6. Does the design survive a counterfactual where GraphLite, Wasm, a provider or a learning strategy is removed?
7. What exact observation would make us **delete** this added complexity?

If an answer is unresolved, write the smallest test or reject/defer the feature. Do not replace missing evidence with another design diagram.
