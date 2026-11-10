---
title: Metron grammar and architecture-selection logic
status: PROPOSED
revision: 0
---

# 02 — Grammar, logic and decision reduction

## 2.1. Ontology: definitions before implementations

Do not use *agent*, *worker*, *role*, *persona*, *skill*, *strategy* and *harness* as interchangeable nouns.

| Word | Meaning | May hold authority? | Source |
|---|---|---|---|
| `Identity` | durable principal + instance ID | only through host-issued grants | host identity store |
| `Agent` | identity + goals + cognitive state + decision strategy | not directly | metron-agent |
| `Persona` | communication and interaction policy | **never** | declarative data |
| `Role` | responsibility, output contract, collaboration context | **never** | declarative data |
| `Cognition` | replaceable transition function / planning policy | **never** | trusted strategy |
| `Skill` | reusable, versioned knowledge/procedure | **never** | data or signed artifact |
| `Memory` | observations, claims, beliefs, episodes, goals and relations | **never** | store via ports |
| `Task` | bounded request with constraints and acceptance criteria | only delegated host grants | protocol |
| `Session` | interaction context/cursor (may branch) | **never** | session store |
| `Intent` | proposed effect, not yet authorized/executed | **never** | pure transition |
| `Capability` | host-authorized operation + resource scope + limit | **yes** | policy kernel |
| `Tool` | typed *interface* to effect | no, absent grant | harness registry |
| `Driver` | adapter executing an authorized effect | exercised authority is scoped | harness |
| `Worker` | isolated process or runtime serving jobs | granted per job | execution host |
| `Receipt` | evidence of an attempted external effect and resources consumed | no | effect boundary |
| `Observation` | source-attested record of what happened | no | trusted observer + receipt |
| `Belief` | revisable agent interpretation with evidence references | no | agent |
| `Outcome` | verifier-attested result against acceptance criteria | no | verifier |
| `Preset` | versioned composition request | **never** | config |
| `Plugin` | implementation of a declared extension contract | no until host grants | registry |
| `Promotion` | independently authorized adoption of candidate change | host-only | eval gate |

**Naming:** refer to *agent instance*, *delegated agent instance*, *harness worker*, *execution preset*, *role*, *persona*, *cognitive strategy* and *skill*. A “reviewer role” is not a second autonomous instance until an actual delegated identity/task exists. The agent distribution bundles a default harness; there is only one harness implementation contract.

## 2.2. Core grammar (conceptual EBNF, not a parser definition)

```ebnf
MetronProduct   = AgentProduct?, HarnessProduct, Protocol, HostPolicy ;
AgentProduct    = Identity, Goals, MemoryView, Cognition, { RoleAssignment },
                  Persona, { SkillReference }, TaskPlanner ;
HarnessProduct  = ModelPort, ContextPort, ToolRegistry, EffectHost,
                  PermissionGate, ReceiptSink, Verifier ;
Protocol        = TaskSpec, EventSchema, IntentSchema, OutcomeSchema, Version ;
HostPolicy      = Principal, GrantSet, BudgetPolicy, ApprovalPolicy, AuditPolicy ;

TaskSpec        = TaskId, Objective, InputReferences, AcceptanceCriteria,
                  RequestedGrants, ResourceBudget, Deadline? ;
Transition      = State, Event, "->", StateDelta, { Intent } ;
Intent          = ModelRequest | ToolRequest | MemoryQuery |
                  ApprovalRequest | WakeupRequest | DelegateRequest ;
Execution       = ValidatedIntent, AuthorizedGrant, Driver, Receipt, Observation ;
Promotion       = CandidateSnapshot, ProbePlan, IndependentEvaluation,
                  AdoptionDecision ;
AdoptionDecision = Rejected | Experimental | Approved | RolledBack ;
```

This grammar is a way to enforce distinctions in source layout. It does **not** imply all named components belong in the first alpha.

## 2.3. Well-formedness constraints

**C1 — Purity and determinism.**
For equal serializable state, policy snapshot and input event, a deterministic cognitive strategy yields the same transition. External time, randomness and model output must arrive as events, not hidden ambient state.

**C2 — No authority escalation.**
For delegated principal `child` of `parent`:
```text
Granted(child, task) ⊆ Granted(parent, task)
Granted(child, task) ⊆ HostMaximum(child)
Requested(preset) != Granted(host)
Role and Persona do not appear on the left of an authorization rule.
```

**C3 — Effect accountability.**
A proposed intent is not an executed effect. A timeout or disconnect after dispatch creates an **uncertain** result until reconciled; non-idempotent operations are not replayed blindly.

**C4 — Provenance and knowledge.**
A claim cites observations or is marked hypothetical. A belief can be revised without mutating historical observations. An answer must carry derivation/transform contracts when it crosses representation frames (existing laboratory invariant).

**C5 — Evaluation separation.**
A candidate cannot read sealed fixtures or promotion criteria; it can see authorized feedback only. Promotion is a separate host action after holdout verification and cost/risk accounting.

**C6 — Privilege separation.**
Extension registration, configuration hot reload, tool advertisement, prompt edits and model feedback cannot change kernel authorization rules.

**C7 — Versioned compatibility.**
Every persistent event, task contract, preset and plugin manifest has a schema version and a declared migration. Unknown required enum variants fail closed.

## 2.4. Pure transition law

```text
step: (State, Event) -> Transition { StateDelta, Intents }

validate: Transition -> Valid | Rejected
commit: (ExpectedRevision, Event, StateDelta, OutboxIntents)
      -> CommittedRevision | Conflict
authorize: (Principal, GrantSet, Intent, HostPolicy)
         -> Denied | AwaitingApproval | Permitted
dispatch: PermittedIntent -> EffectReceipt + Result | EffectUncertain
observe: EffectReceipt + Result -> NewEvent
verify: (Evidence, AcceptanceCriteria) -> Outcome
```

The outbox and state update MUST be atomic **inside the same storage authority**. This does not make a network or shell side effect exactly once. Idempotency/reconciliation remain effect-specific.

## 2.5. Research-to-architecture reduction pipeline

Do not choose a design by “number of desirable features”. Apply the following sequence.

```text
Question
  -> objective + concrete task distribution + hard constraints
  -> candidate mechanism families
  -> source evidence / assumptions
  -> elimination by hard invariant
  -> Pareto comparison under matched budgets
  -> counterfactual stress tests
  -> smallest runnable vertical slice
  -> falsifiable acceptance contract
  -> measured result
  -> retain / simplify / defer / reject
```

### Hard constraints (gates, not weighted bonuses)

H0. No incompatible change to existing laboratory without an accepted ADR.
H1. First-party implementation in Rust, public Apache-2.0 sources, compatible dependencies.
H2. Pure decision logic and replaceable effect drivers.
H3. Default-deny external authority; tests cover alternative execution paths.
H4. Durable causal evidence; no model-originated “success” accepted without verification.
H5. Agent and harness can be used separately.
H6. Test fixtures, evaluation criteria and promotion authority are out of reach of candidates.

### Candidate families tested

| Candidate | Benefit | Decisive cost/weakness | Decision |
|---|---|---|---|
| Port Hermes/DSH wholesale | familiar features quickly | Python/TS and upstream coupling; violates Rust-native source goal | Reject as base |
| Minimal Pi-like loop, no isolated authority | fast first demo | insecure by default for computer use | Reject without policy kernel |
| Large Rust agent OS from day one | broad feature coverage | excessive maintenance, weak ability to attribute benefit | Defer |
| Agent-first graph-mind | cognition research starts immediately | no trustworthy world interaction/eval | Defer |
| Harness-first only | isolated tool correctness | postpones separable cognition proof | Keep, but pair with thin agent |
| **Minimal vertical kernel + thin agent + harness** | isolates all essential boundaries; measurable | upfront protocol and receipt rigor | **Select for alpha 0** |
| Harness²-style recursive self-editor | potential adaptation | cost, contamination and possible regression | Later isolated experiment |

### Qualitative Pareto frontier

Compare designs on: **security**, **falsifiability**, **implementation complexity**, **time to first independent use**, **extensibility**, **operator burden**, **resource cost**, **integration cost**, **known regression risk**.

Use explicitly documented ordinal judgments (low / medium / high) until a benchmark produces quantities. Do not invent a “0.93 architecture score”. A candidate dominated on security and implementation cost by a simpler alternative is rejected unless it produces demonstrated task-quality improvement.

### Sensitivity / counterfactuals

- If the domain is **local coding only**, the minimum standalone harness may be sufficient; cognition layers can be optional.
- If tasks include **untrusted external email/web**, isolation, injection-resistance and approvals become release-blocking.
- If long-lived operations **survive host crashes**, durable outbox, leases and reconciliation are necessary; volatile traces are not enough.
- If the exact graph traversal workload **dominates latency**, promote an indexed graph projection; otherwise SQLite nodes/edges remain simpler.
- If model inference **dominates cost**, invest in context reduction and routing before lock-free adjacency caching.
- If adaptive strategies **do not beat a matched static baseline**, stop recursive optimization, retain the harness.

## 2.6. Release-vector decision

Alpha 0 solves the *architecture-separation* hypothesis:

```text
one identity
+ one role/persona
+ one pure cognitive strategy
+ one scripted model adapter
+ one bounded local harness
+ one verifier
+ one event/outbox journal
= one replayable, evidence-backed task
```

**Exclude by design at alpha 0:** GraphLite, vector indexes, speculative self-editing, networked desktop control, multi-agent scheduling, general orchestration DSL, native hot-loaded plugins, unsafe zero-copy SQL buffers.

Later release gates in [06-alpha-release-contracts.md](06-alpha-release-contracts.md) deliberately reintroduce those possibilities one at a time, under measurements.
