## Summary

- Motivation and changes
- Related issue or proposal

## Stack and target

- [ ] Standalone change targeting `main`
- [ ] Stack layer targeting its immediate predecessor (`stack/...`)
- [ ] Parent PR and ordered stack links

## Boundaries

- [ ] Core purity, sealed evaluation, receipts/replay and transform contracts preserved
- [ ] Accepted ADRs preserved or explicitly superseded
- [ ] Runtime changes satisfy the P0 compatibility decision

## Architecture diagram for human review

- Current diagram or link, with relevant components and authority boundaries
- How the change fits the diagram, including proposed components
- [ ] Diagram reviewed against the source and diff before human approval

## Verification

- Exact commands and results
- Checks and environments untested

## Knowledge and documentation

- Knowledge IDs, `added_by` and `human_ratified` state, or none
- Docs, ADR and priors changes, or none

## Review and landing

- Authoring agent and model
- Independent reviewer and model, reasoning level and cross-family or fresh-context fallback
- [ ] Rebased onto the target (`main` or predecessor stack layer) with linear history
- [ ] All review conversations resolved
- [ ] Independent agent verification completed
- [ ] Approved by `@kmosoti` (`.github/CODEOWNERS`) and rebase-merged into `main`
