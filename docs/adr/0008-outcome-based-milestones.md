# ADR 0008: Milestones are outcomes with exit criteria, not dates

Status: accepted. Date: 2026-10-02.

## Context

The deep-research proposal planned the work as M0 to M4 with durations
(2, 4, 6, 8 and 6 weeks). Durations say nothing about what must be true
for a milestone to count, invite "done by the date" rather than "done",
and cannot express a gate that closes a milestone as not worth doing.

## Decision

Milestones are defined in `docs/architecture/milestones.md` as outcomes
with exit criteria that can be checked in the repository (a passing test,
a committed report, a recorded decision). A milestone is reached when the
criteria hold. Later milestones name the gate they depend on; a gate may
close a milestone, and that closure is recorded as a result.

## Consequences

- No milestone has a duration or a date.
- M3 (routing) cannot start before M2 (headroom) has a committed report
  and an ADR with the decision, which is the order the prior-art review
  argues for.
- Progress is reported as which exit criteria hold, with evidence.
