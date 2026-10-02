# ADR 0003: The laboratory owns ground truth

Status: accepted. Date: 2026-10-02.

## Context

If the system can inspect the hidden target, the evaluator, or the
promotion criteria, every result is suspect. The prior-art review's core
critique of the original lab design was partly about this: a lab whose
budget covers the whole truth table measures nothing.

## Decision

- Hidden targets are sealed types in `metron-lab` (`HiddenFunction`):
  private fields, no accessor, no serialisation, redacted `Debug`.
- The system reaches the target only through the core's `Oracle` port, one
  receipted probe at a time, under a `Protocol` cap the lab enforces.
- The judge (`LabWorld::judge`) and the promotion gate (`PromotionGate`)
  live in the lab; the judge runs after the episode; criteria are private.
- Fixtures and manifests are parsed from text; the lab does no I/O. The
  composition root reads the files.
- The lab is "immutable" in the sense that its evaluator, protocol and
  criteria are versioned code with fingerprints recorded in results
  (`target_fingerprint`, `PromotionGate::fingerprint`), so a change is
  visible in every result that used it.

## Consequences

- Operators are generic over `W: Oracle`; nothing on that bound reveals
  the target or the verdict.
- `tests/architecture/tests/invariant_2_lab_owns_ground_truth.rs` checks
  the sealing, the redaction, and the dependency direction.
- When the mixed Boolean lab arrives (ADR 0006), its families, split
  hygiene (NPN canonicalisation) and headroom harness will also live here.
