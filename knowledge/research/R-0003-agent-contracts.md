---
id: R-0003
type: research
status: proposed
updated: 2026-11-10
added_by: agent
human_ratified: false
source_proposal: 4c3781daa7211efd39bcdb95dcb15491be1abe39
---

# 03 — Agent contract

## Scope and excluded responsibilities

`metron-agent` owns **the choice of what to attempt and why**. It does **not** own process execution, filesystem access, networking, browser automation, host credentials, hard spending limits or the meaning of “verified success”.

A runnable agent distribution may bundle and launch `metron-harness` through a composition root, but library logic imports only `metron-protocol` and ports. A standalone harness does not depend on any agent strategy.

## Domain vocabulary and instance model

```text
AgentIdentity      := PrincipalId + InstanceId + Revision
AgentState         := Identity + Goals + EvidenceView + Beliefs + DecisionState
RoleAssignment     := RoleSpec + AssignmentScope + Provenance
PersonaAssignment  := PersonaSpec + Revision
CognitiveStrategy  := StrategyId + Version + ConfigHash
SkillReference     := SkillId + Version + EvidenceOfApplicability
Delegation         := ParentTask + ChildIdentity + SubsetGrants + SubBudget
```

Persist identity and assignment **by ID and version**; never use the model's chosen name or free-text persona as a security principal.

### Role and persona

A `RoleSpec` contains:
- `purpose`: bounded responsibility (e.g. investigator, implementer, reviewer).
- `input_contract` / `output_contract`: evidence and artifact expectations.
- `selection_hints`: optional strategy/skill preferences.
- `stop_conditions`: explicit completion/abstention cases.

A `PersonaSpec` contains:
- `interaction_style`: response style, language and formatting preferences.
- `communication_boundaries`: style-level constraints, not kernel policy.
- `context_policy_ref`: how to render identity/role/prompt pieces within the budget.

A role is **not** a principal and a persona is **not** a permission set. A child agent role does not create another process until a host-approved delegated task requests it.

### Skills

Treat a skill as a **versioned procedure**: input conditions, steps, expected outputs, failure signals, costs and evaluation history. A skill can be human-authored or proposed by a model. It cannot ship executable code that gains privileges merely by being installed.

Skill promotion is separate from skill retrieval. The current laboratory already makes this distinction for `CapabilityCandidate` and held-out promotion (ADR 0012). Do not rename the laboratory's existing concepts during product incubation.

## Memory contracts

Memory is **not** an untyped chat transcript.

| Type | Example | Update semantics |
|---|---|---|
| Observation | Tool reported command exit code 1, stdout hash X | append-only, refer to receipt |
| Claim | External page states company is hiring | source-cited; source may be untrusted |
| Belief | The posting may be a duplicate | revise posterior/justification without erasing claim |
| Episode | A bounded sequence of decisions and observations | append events and result |
| Goal | Deliver tested pull request | versioned desired-state and terminal conditions |
| Procedure | Run a validation gate before declaring completion | versioned skill with test evidence |
| Relation | company -> offers -> project | derived projection with freshness watermark |
| Preference | format replies concisely | scoped persona/config, not evidence of reality |

Every belief must carry `evidence_refs`, `uncertainty`, `created_at` / `updated_at` as recorded context, and `state_revision`. If no supporting observation exists, it is an explicit hypothesis; do not synthesize confidence percentages without calibration.

Separate the *identity's durable memory* from a *task's temporary context*. A delegated child sees a scoped view, not unrestricted durable storage.

## Sans-IO cognition API

```rust
// Contract sketch; canonical types are described in R-0006.
pub trait Cognition {
    fn transition(
        &self,
        state: &AgentState,
        event: &AgentEvent,
    ) -> Result<Transition, DecisionError>;
}
```

The strategy can propose `RequestModel`, `RequestMemory`, `RequestTool`, `RequestApproval`, `WakeAt` and `DelegateTask` intents. It cannot execute them. Model results and scheduler wakeups are *events* passed into the next pure transition.

Cognition implementations may later include plan/act/observe, evidence-backed hypothesis search, beam/MCTS planning, graph-guided choice and experimental adaptive-symbolic policies. Every implementation shares the event/intent contract and is tested at matched compute.

## Default cognitive strategy: evidence-first

Start deliberately simple:

```text
event arrives
  -> normalize evidence (or mark source untrusted)
  -> update explicit goal/task state
  -> if insufficient information: choose bounded observation
  -> else compare candidate next actions by constraints and utility
  -> if authority needed: request capability/approval
  -> else emit one typed work intent
  -> wait for receipt/result; never fabricate completion
  -> verify goal terminal condition through independent outcome
```

Do not confuse a model's chain-of-thought with externally verifiable rationale. Persist a **decision summary**: considered choices, cited evidence, constraints and uncertainty, without requiring hidden model reasoning.

## Identity, goals and autonomy

A goal has priority, owner, scope, acceptance criteria, last observation timestamp, deadline when relevant, spend/resource budget and stop conditions.

Persistent operation needs explicit wakeup events and task leases. Scheduling is an external effect; “background agent” is not a license to run without a host-issued deadline/capability.

For a coding task, source inspection, edits and test execution each use the task's scoped capabilities. Publishing a release requires its own authority and
approval contract.

## Delegation design (deferred beyond first alpha)

- Parent requests a subtask with a role and evidence view.
- Host validates intersection of grants and budget; generates a distinct child principal.
- Child operates through the same harness protocol, with private scratch space and scoped context.
- Parent receives child *artifacts and evidence*, not an automatically trusted “success”.
- Cycle detection, bounded depth, cancellation, budget attribution and abort-on-parent-cancel are required before general delegation ships.

Default v0.1-alpha.0 runs **one agent instance**, not multiple synthetic agent voices.

## Learning and Harness² intersection

Split adaptation into stages:

1. **Within-task revision**: select a candidate prompt/skill/context policy for a task, probe it without privileged effects.
2. **Cross-task library**: store intervention + task distribution + measured outcomes + costs and failure cases.
3. **Promotion**: independently evaluate on matched holdouts, record regressions, require human approval for promotion.
4. **Policy adjustment**: activate the approved snapshot via version pointer, keeping rollback.

Do not give a learning plugin control of host policy, sandbox, evaluator fixtures or approval state. Changes to role/persona/skills can be proposed as data. Changes to executable drivers require signed-reviewed code and fresh threat tests.

## Agent exit contract

- Same `State + Event + StrategyVersion` yields identical `Transition` with a deterministic test driver.
- Changing persona cannot increase the capability set or affect host authorization results.
- Every current belief can be traced to observations/claims or labeled as unverified.
- An agent without a configured harness can still compute and serialize intents; it must report “not executed”.
- One default identity/role/persona completes a scripted task without needing a second agent.
- All experimental strategy variants can be evaluated through the same opaque port interfaces.

## Provenance and relations

- derived-from [original 03-agent.md](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/03-agent.md)
- indexed-by [runtime research and sequence](README.md)
- reviewed-against [original proposal findings](../observations/O-0001-runtime-proposal-review.md)

## Revisit when

Implementation evidence, a changed workload, a source revision or a failed acceptance
criterion contradicts the proposed contract. Record the new evidence and update
linked decisions before expanding scope.
