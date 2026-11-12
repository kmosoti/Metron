# Research inputs

These documents are historical research inputs archived for provenance. They
are not current product design. Current architecture is described by
`docs/architecture/overview.md` and product decisions; Boolean-laboratory scope
is superseded by [ADR 0024](../adr/0024-product-scope-and-laboratory-archive.md).

| File | What it is | How it is used |
|---|---|---|
| `proposal-deep-research-2026-10.md` | A machine-generated research proposal for a "Hexagonal Cognitive Kernel": architecture sketch, deployment plan, benchmark design, milestones. | Source of the original vocabulary (operators, routing, ledger, milestones). Its architecture is **not** adopted verbatim: it makes HDC foundational and introduces a generic `Capsule` before the research objects are mature. See ADR 0006 and ADR 0007. |
| `prior-art-review-2026-10.md` | A source-verified prior-art review of the same proposal against query learning, algorithm selection, metareasoning, synthesis, library learning and VSA capacity. | Historical research context. Its Boolean-laboratory recommendations describe the archived lab, not the current product roadmap. See ADR 0006 and the historical evidence archive. |

Citations in the review were verified by its author to the extent stated in
its own "Verification table" and "Caveats" sections. `priors.md` is the
priors ledger (ADR 0014): every prior the design relies on, its source, the
test or primary source that checks it, the observation, and its status.
