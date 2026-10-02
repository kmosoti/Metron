# Research inputs

These documents are **inputs** to Metron, archived verbatim for provenance.
They are not the design. Where they disagree with `docs/architecture/` or an
ADR, the architecture documents win.

| File | What it is | How it is used |
|---|---|---|
| `proposal-deep-research-2026-10.md` | A machine-generated research proposal for a "Hexagonal Cognitive Kernel": architecture sketch, deployment plan, benchmark design, milestones. | Source of the original vocabulary (operators, routing, ledger, milestones). Its architecture is **not** adopted verbatim: it makes HDC foundational and introduces a generic `Capsule` before the research objects are mature. See ADR 0006 and ADR 0007. |
| `prior-art-review-2026-10.md` | A source-verified prior-art review of the same design against the literature (query learning, algorithm selection, rational metareasoning, Soar/ACT-R, oracle-guided synthesis, library learning, VSA capacity). | The stronger of the two. Its central recommendations shape the roadmap: measure SBS–VBS headroom before building adaptive routing; redesign the Boolean laboratory for n = 5–8 with mixed families and NPN-deduplicated splits; treat HDC as a replaceable retrieval experiment; report rliable-style. See ADR 0006. |

Citations in the review were verified by its author to the extent stated in
its own "Verification table" and "Caveats" sections; nothing here re-verifies
them.
