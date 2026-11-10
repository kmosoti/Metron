---
title: Rust-shaped normative implementation contract
status: PROPOSED — examples specify code shape, not implemented source
syntax-target: Rust 2024 stable
---

# 07 — Implementation contract written as the target code

> **Instructions to coding agents:** Treat ownership, types, paths, boundaries, and tests here as normative targets. The snippets are implementation sketches, **not claims of compilation**. A spec-shaped prompt might reduce ambiguity; only executed compiler, behavior and security tests establish compliance.

## Proposed workspace (created only after P0 ADR)

```text
products/runtime/
├── Cargo.toml                      # separate workspace from laboratory
├── Cargo.lock
├── rust-toolchain.toml             # pin validated stable compiler
├── crates/
│   ├── metron-protocol/
│   │   └── src/{lib.rs,task.rs,identity.rs,event.rs,intent.rs,receipt.rs}
│   ├── metron-agent/
│   │   └── src/{lib.rs,state.rs,cognition.rs,role.rs,persona.rs,goals.rs}
│   ├── metron-harness/
│   │   └── src/{lib.rs,model.rs,tool.rs,policy.rs,effect.rs,verify.rs}
│   ├── metron-store-sqlite/
│   ├── metron-driver-local/
│   ├── metron-host/                # executable composition root
│   └── metron-testkit/
└── tests/{architecture,end_to_end,adversarial}/
```

Dependency direction:

```text
metron-protocol -> serde, serde_json, thiserror (no effects)
metron-agent -> metron-protocol
metron-harness -> metron-protocol
metron-store-sqlite -> metron-protocol + SQLite library
metron-driver-local -> metron-protocol + metron-harness + OS/async adapters
metron-host -> all product crates (sole composition root)
metron-testkit -> pure port implementations and scripted drivers

FORBIDDEN:
metron-agent -> metron-harness
metron-harness -> metron-agent
metron-protocol -> tokio / SQL / net / filesystem / process
products/runtime -> existing metron-lab evaluation fixtures
```

The current root laboratory remains unchanged until an approved ADR establishes a bridge.

## Source: pure task and identity protocol

```rust
// file: products/runtime/crates/metron-protocol/src/task.rs
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrincipalId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceBudget {
    pub max_model_requests: u32,
    pub max_tool_calls: u32,
    pub max_elapsed_ms: u64,
    pub max_spend_micros_usd: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    pub operation: String,
    pub resource: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub schema_version: u16,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub objective: String,
    pub input_refs: Vec<String>,
    pub acceptance_refs: Vec<String>,
    pub requested: Vec<CapabilityRequest>,
    pub budget: ResourceBudget,
}
```

Use integer money units, not floating-point dollars. `TaskSpec` requests capabilities; it does **not** include granted permissions or secrets.

## Source: effect intents are not authority

```rust
// file: products/runtime/crates/metron-protocol/src/intent.rs
use serde::{Deserialize, Serialize};
use crate::task::TaskId;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntentId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IntentKind {
    RequestModel {
        context_ref: String,
        model_route: String,
    },
    ReadWorkspace {
        relative_path: String,
    },
    RunTool {
        tool_id: String,
        arguments_ref: String,
    },
    RequestApproval {
        action_digest: String,
    },
    PersistMemory {
        change_ref: String,
    },
    ScheduleWakeup {
        logical_after_ticks: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub schema_version: u16,
    pub intent_id: IntentId,
    pub task_id: TaskId,
    pub kind: IntentKind,
}
```

Deserializing model output into `Intent` does not make it an `AuthorizedEffect`. Versioned schemas and validation prevent unknown variants being silently accepted.

## Source: pure cognition strategy

```rust
// file: products/runtime/crates/metron-agent/src/cognition.rs
use metron_protocol::intent::Intent;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentState {
    pub revision: u64,
    pub goal_refs: Vec<String>,
    pub belief_refs: Vec<String>,
    pub persona_ref: String,
    pub role_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentEvent {
    TaskAccepted { task_id: String },
    EvidenceObserved { receipt_ref: String },
    ApprovalDenied { intent_id: String },
    Wakeup { logical_tick: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transition {
    pub next: AgentState,
    pub intents: Vec<Intent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionError {
    InvalidEvent,
    InvariantViolation,
    MissingEvidence,
}

pub trait Cognition: Send + Sync {
    fn transition(
        &self,
        state: &AgentState,
        event: &AgentEvent,
    ) -> Result<Transition, DecisionError>;
}
```

No `async`, file access, database, sockets, wall clock, ambient entropy or hidden model call in this trait. `Transition.next` is proposed state, not a committed mutation.

## Source: authorization kernel and effect driver

```rust
// file: products/runtime/crates/metron-harness/src/effect.rs
use std::{future::Future, pin::Pin};
use metron_protocol::intent::Intent;

#[derive(Debug)]
pub enum EffectOutcome {
    Observed { receipt_ref: String },
    Failed { receipt_ref: String },
    Uncertain { dispatch_ref: String },
}

// Construction must be private to the host policy kernel module.
pub struct AuthorizedEffect {
    intent: Intent,
    grant_ref: String,
}

pub trait EffectDriver: Send + Sync {
    fn execute(
        &self,
        command: AuthorizedEffect,
    ) -> Pin<Box<dyn Future<Output = EffectOutcome> + Send + '_>>;
}
```

```rust
// file: products/runtime/crates/metron-harness/src/policy.rs
use metron_protocol::{intent::Intent, task::{PrincipalId, ResourceBudget}};

pub enum Authorization {
    Denied { reason: String },
    AwaitingApproval { request_id: String },
    Permitted { authorization_ref: String },
}

pub trait PolicyKernel: Send + Sync {
    fn authorize(
        &self,
        principal: &PrincipalId,
        intent: &Intent,
        grants: &GrantSnapshot,
        limits: &ResourceBudget,
    ) -> Authorization;
}
```

`GrantSnapshot` intentionally remains a *host-owned opaque type* to be specified in the implementation ADR. A plugin cannot manufacture it from JSON or role/persona instructions. Effective grants require revalidation at effect dispatch.

## Source: atomic state/outbox contract

```rust
// file: products/runtime/crates/metron-store-sqlite/src/journal.rs
pub struct ExpectedRevision(pub u64);

pub enum CommitResult {
    Applied { new_revision: u64 },
    Conflict { actual_revision: u64 },
}

pub trait Journal {
    type Error;

    fn commit_transition(
        &mut self,
        expected: ExpectedRevision,
        event_ref: &str,
        transition_ref: &str,
        pending_intent_refs: &[String],
    ) -> Result<CommitResult, Self::Error>;
}
```

The concrete SQLite implementation MUST persist event, state change and outbox entries atomically with uniqueness constraints. This signature is schematic; production code needs actual typed event bytes, transaction scoping and error semantics. External effects never participate in the SQLite transaction.

## Composition algorithm (pseudocode, not Rust source)

```text
for each accepted event:
  state = journal.load(identity)
  transition = cognition.transition(state, event)
  validate_transition(transition)
  commit(event, transition, outbox, expected_state_revision)
  on conflict: reload + recompute pure transition

  for each committed outbox intent:
    authorization = policy.authorize(principal, intent, effective_grants)
    if denied: record denial event
    if approval needed: suspend and ask external approver
    if permitted:
      persist dispatch marker
      execute with scoped, host-minted effect token
      persist receipt OR mark outcome Uncertain
      independently verify against acceptance criteria
      enqueue new observation event
```

A process that crashed after dispatch can leave an uncertain external operation. It is **not** automatically retried for non-idempotent effects.

## Code generation output shape

Every implementation task supplied to a coding model should use this response contract:

```text
FILE: exact repository-relative path
PURPOSE: single ownership boundary
PUBLIC API: typed signatures, invariants, errors
DEPENDENCIES: explicitly allowed crates
IMPLEMENTATION: safe Rust 2024, minimal effects
TESTS: positive, negative, crash/timeout, adversarial
EVIDENCE: commands actually run and their real results
DEVIATIONS: none, or proposal requiring a new ADR
```

Use narrow source files, Rust doc comments on public APIs, exhaustive match arms, typed IDs, structured errors, and explicit ownership/lifetimes. Do not optimize through raw pointers or self-referential SQLite rows without a benchmark and separate safety review.

## Test specification (these are placeholders, not passing tests)

```rust
#[cfg(test)]
mod architecture_contract {
    #[test]
    fn same_state_and_event_yield_same_transition() {
        // Compare serialized transitions with explicit seed and policy.
    }

    #[test]
    fn persona_change_never_grants_a_new_capability() {
        // Hold host grants fixed while varying role/persona.
    }

    #[test]
    fn denied_effect_never_reaches_a_driver() {
        // Spy effect driver observes zero calls.
    }

    #[test]
    fn crash_after_non_idempotent_dispatch_remains_uncertain() {
        // No blind replay; reconciliation required.
    }

    #[test]
    fn hidden_holdout_is_inaccessible_to_candidate() {
        // Sealed test fixture cannot be read through direct or indirect tools.
    }

    #[test]
    fn receipt_payload_changes_replay_hash() {
        // Volatile wall clock omitted; authoritative payload included.
    }
}
```

Do not count empty tests as evidence. Implement actual fixtures and assertions before making any alpha success claim.

## Compilation and review gates

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo tree --workspace
# Run architecture dependency tests, negative capability tests,
# crash-recovery fixtures, license/security audit, and eval script.
```

Use `#![forbid(unsafe_code)]` in protocol and cognition crates. A platform-specific sandbox driver may need lower-level OS interaction; isolate any unavoidable unsafe implementation behind reviewed modules and an ADR.

The first coding PR after this spec: **P0 compatibility ADR + empty runtime workspace with executable dependency-invariant tests**. A subsequent separate PR implements the scripted alpha.0 loop. Do not bundle migration, GraphLite, Wasm, model routing and desktop control into one “foundational” change.
