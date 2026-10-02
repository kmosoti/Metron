# ADR 0012: Capability candidates are proposed by the system and judged by the laboratory on held-out targets

Status: accepted. Date: 2026-10-02.

## Context

Milestone M4 requires that compositions the system finds useful become
capabilities only when the laboratory says so, on targets the system has
not seen, at matched compute, with reuse measured. The prior-art review
warns (Berlot-Attwell et al. 2024, 2025) that library-learning gains
often vanish when reuse is counted and compute is matched.

## Decision

- **Proposal is the system's.** `metron_app::propose` turns an answered
  episode into a `CapabilityCandidate`: the distinct operators along the
  answer's provenance, a claimed end-to-end contract composed from the
  contracts on that path (first source frames, last target frame, every
  assumption and loss, approximate if any step was), the answer's
  evidence, and a program: the episode's executed trace with maximal
  repeated units generalised into repeatable sequences, so the candidate
  is not tied to how many probes its own target needed.
- **Judgement is the laboratory's.** `metron promote` runs a strategy on
  the train split, collects candidates keyed by program, runs each
  candidate as a fixed strategy on the NPN-clean test split, and asks
  `PromotionGate::judge` for a decision under criteria the system cannot
  read. The report records the gate's fingerprint, train support, reuse
  frequency (how often the proposing strategy executed the same program on
  held-out tasks), the candidate's held-out solved rate and probes, and
  the proposing strategy's own held-out figures on the same tasks.
- Candidates that fail held-out targets are rejected with reasons but
  never thresholds.

## What was measured

`experiments/reports/promotion-arity5-7f5ef8af8c7d.md` (arity 5, 14 train and 10
test tasks):

| Proposing strategy | Candidate | Held-out solved | Decision |
|---|---|---:|---|
| exhaustive | probe-next → partial table → complete by default → commit | 100% | promoted |
| greedy-pool | greedy probe / filter loop → single survivor → commit | 100% | promoted |
| affine-blind | affine probe → affine solve → commit | 30% | rejected |
| affine-verified-then-greedy | the greedy fallback alone | 100% (parent: 80%) | promoted |
| affine-verified-then-greedy | greedy probe → affine solve → commit | 30% | rejected |

The last strategy shows the mechanism doing its job: the composition
extracted from its successful episodes drops the risky affine commit and
beats its parent on held-out targets at the same probe budget.

## Consequences

- M4's exit criteria hold: decisions carry the gate's fingerprint, a
  failing candidate is rejected in tests, and reuse frequency is reported
  against the inlined reference at equal budget.
- Promoted candidates are reports, not registered operators. Registering
  a promoted program as a named strategy is a manifest edit, which keeps
  the system from promoting itself.
- Generalising a trace by repeating its longest repeated unit is a
  deliberate, simple choice; richer program induction is future work and
  must be judged by the same gate.
