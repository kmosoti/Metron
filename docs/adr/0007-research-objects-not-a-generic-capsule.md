# ADR 0007: Research objects, not a generic capsule, are the central abstractions

Status: accepted. Date: 2026-10-02.

## Context

The proposal centres a generic `Capsule` carrying "context, goals, derived
features" and a semantic vector. That abstraction is premature: it decides
representation before the research objects are understood, and it blurs
the distinctions the invariants need (what is an observation, what is a
redescription, what is an answer).

## Decision

The core's central types are the objects the research is about:

| Object | Type | Role |
|---|---|---|
| Inquiry | `inquiry::Inquiry` | working state about one `Question` |
| Observation | `evidence::Observation` | a receipted fact from the oracle |
| Evidence | `evidence::Evidence` | observation plus operator path |
| View | `inquiry::View` | state in one frame, with `Derivation` |
| Frame | `frame::Frame` | a representation system |
| TransformContract | `frame::TransformContract` | how frames connect |
| Operator | `operator::Operator<W>` | observe / transform / commit |
| ResourceReceipt | `receipt::ResourceReceipt` | cost and evidence of an external operation |
| Episode | `journal::Episode` | one bounded, journaled run |
| CapabilityCandidate | `capability::CapabilityCandidate` | a proposed composition, judged by the lab |

Vectors, if they come, are a `Representation` variant inside a view in a
frame with a contract, not the carrier of the inquiry.

## Consequences

- Operators read and write views by name and frame; they cannot stash
  untyped state.
- The provenance rule is computable from the objects themselves.
- Renaming is cheap now and expensive later; this is the moment to fix the
  vocabulary.
