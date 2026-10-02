# ADR 0001: Record architecture decisions

Status: accepted. Date: 2026-10-02.

## Context

Metron exists to run experiments whose validity depends on rules that are
easy to break by accident: who may see the hidden target, what counts as
independent evidence, which costs are charged. Those rules must be written
down where the code is, and changes to them must be visible.

## Decision

Architectural decisions are recorded as numbered files in `docs/adr/`.
Each states its context, the decision and its consequences. An accepted ADR
is not edited; a later ADR supersedes it.

## Consequences

Reviewers can check a change against the ADR it touches. Agents working in
the repository (see `AGENTS.md`) are expected to read the ADRs before
changing the shape of the system.
