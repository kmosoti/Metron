# ADR 0009: Language-model inference comes from Claude Code sessions, not an API client

Status: accepted. Date: 2026-10-02.

## Context

The proposal planned HTTP clients for Claude models. This project does not
use the Claude API. The inference available to it is the Claude Code
session that runs the experiments. That session can read and write files
and run the CLI, and nothing more.

## Decision

- There is no API client in `metron-adapters` and none is planned.
- A consult operator (`OperatorKind::Consult { service }`) calls the
  `ExternalService` port. The `ClaudeCodeBridge` adapter implements it as a
  file protocol under the run directory: a request file
  (`llm/requests/<id>.json`) with the prompt, and a response file
  (`llm/responses/<id>.json` or `.txt`) the session writes.
- When no response exists, the call is `Pending`, the runner restores the
  inquiry to its state before the operator ran, journals `Suspended`, and
  returns a checkpoint; the composition root persists it. `metron answer`
  writes the response; `metron resume` retries the operator.
- Every answered consultation issues an `ExternalCall` receipt and a
  `ServiceAnswered` journal event with the full request and response, so
  `metron replay` re-runs the episode from the journal without the session
  and checks that the effective events match.
- A proposal is never evidence. It lands in the `consultations` frame;
  only a transform that verifies it against every observation may move it
  to a frame that can be committed, under an approximate contract that
  says the unobserved rows are still a proposal.

## Consequences

- Episodes that consult are asynchronous. The headroom harness refuses
  strategies that consult; such strategies are measured by running them to
  completion first and replaying, or by a scripted service in tests.
- Token counts are unknown; cost records calls, not tokens.
- Replacing the session with a real model client later would be a new
  `ServiceBackend` and nothing else.
