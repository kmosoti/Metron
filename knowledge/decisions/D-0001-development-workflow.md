---
id: D-0001
type: decision
status: adopted-for-integration
updated: 2026-11-10
added_by: agent
human_ratified: false
---

# Development workflow

## Decision

Adopt rust-ui-template's two-branch workflow, colocated Jujutsu preference,
independent reviewer-owned integration, commit/PR templates and linked knowledge
provenance. Configure `develop` for integration and `main` for human-gated release.
Preserve Metron's laboratory invariants, Apache-2.0 licensing, ADR process and
scientific priors ledger.

## Evidence and alternatives

The requested reference defines these mechanisms in its
[working agreement](https://github.com/kmosoti/rust-ui-template/blob/e959615423c7deaa0608fc3a51769b71239bc2b1/AGENTS.md)
and [contribution guide](https://github.com/kmosoti/rust-ui-template/blob/e959615423c7deaa0608fc3a51769b71239bc2b1/CONTRIBUTING.md).
Metron's existing `check` CI executes the laboratory invariants and smoke journal
verification. Require that check on both protected branches alongside the release
gate on `main`. Independent review remains a contributor responsibility because
shared GitHub credentials cannot prove model identity.

Replacing existing ADRs with knowledge decisions would split established
architectural authority. Keep those ADRs and link supporting knowledge records.
UI-specific boundaries from the reference do not apply to Metron's crate graph.

## Relations

- informed-by [contribution workflow](../../CONTRIBUTING.md)
- contextualized-by [runtime proposal review](../observations/O-0001-runtime-proposal-review.md)

## Revisit when

GitHub account separation permits machine-enforced reviewer independence, or a
measured integration failure calls for changes to the branch/review contract.
