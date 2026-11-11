---
id: O-0007
type: observation
status: active
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Explicit task selection and preliminary toolkit checks

Source: `c5606f47667c10d5970d53b351afb252bc67814d`. This slice implements
task selection, pure coding DTOs and an operator inventory, not coding effects,
cross-task memory, an autonomous loop or full alpha.1 qualification.

## Executed results

| Check | Observed result |
| --- | --- |
| New calculator objective, missing/unknown family, both modes | Exit 2 before run directory creation; no sum verdict or model load |
| Explicit `sum-v1` selection | Legacy task completes; replay/verify retain its head |
| Coding proposals | Reject extra verdict fields, multiple objects, shell program IDs, invalid paths/ranges and frame overruns |
| Task identities | Acceptance, catalog, profile and collection changes alter the coding task digest; legacy schema domain remains v1/v2 |
| Pinned inventory | Seven executable image hashes and version probes identified; no runtime qualification minted |
| Inventory integrity | Source replacement cannot alter the executed sealed image; sealed bytes cannot be overwritten |
| FIFO catalog | Rejects without waiting for a writer |
| Linked SQLite | Version 3.50.2; FTS5 fixed-content positive/scope-negative probes passed |
| ripgrep | Match/no-match/invalid-regex returned 0/1/2 respectively |
| jj | Ignored-working-copy diff omitted new content; explicit snapshot then diff included it |
| Z3 valid model | `unsat` for an overrun under the admitted reservation premise |
| Z3 weakened model | `sat`, with spent=0, reserved=1, requested=1, limit=1 |
| Lean valid models | Reservation lemma accepted without axioms; grant-subset lemma accepted using `propext` |
| Lean false model | Exit 1, type mismatch when the reservation premise omits reserved usage |

The inventory catalog digest is
`35ebf336cb8799b464425682bb45563d895e8a1ada8063d96a3a9ff94e3ee289`.
It binds host-specific executable paths as well as expected hashes and versions;
another installation needs its own inventory and identity.

## Tool identities

| Tool | Version | Executable SHA-256 |
| --- | --- | --- |
| jj | 0.46.0 | `8dae6d15c956912dd3f54449a49eeddb53bd917ba79f0f469d07319f31880679` |
| ripgrep | 15.2.0, e89fff89ac | `e62198eb19b136b88c330af83647b5a962cb99b6b1f066758568f12de1974849` |
| Z3 | 4.16.0 | `992b97e6a76d629abed5f13f34dcc70c67f29ade22b937130bbf06f3c1eecf26` |
| Lean | 4.35.0, eb6cafb7290e8bd5f2937522d8d804f58f02bafc | `bf8d54e4714cc4b03d3f6bb34c83b7202b87e49c8bfcbff6895c085bb90ceb38` |
| Cargo | 1.100.0-beta.12 | `7fdd7f8e385c610b70a7d8b2d902ad8e4850a21ae9ace987d9e3cafe084c3bbb` |
| rustc | 1.100.0-beta.12 | `0d291bef01d00946dab48b80f761cc5b9aa467963e0569e87c5188fc31f60dda` |
| bubblewrap | 0.12.0 | `6da06f152b0865172d73348c34cb88487c326ce2f21cd980fc25ff10c4dbcdfb` |

The official Lean archive's full size and hash matched R-0011 before extraction.
No global toolchain was replaced. Executable-image identity does not pin every
shared library or interpreter; the inventory explicitly remains operator-only
and relies on trusted dependencies. Runtime admission and confinement are still
separate requirements.

## Reproduction and verification

Run both workspaces' format, Clippy with warnings denied, and test commands from
CONTRIBUTING.md using the dated Rust pin. Use an SSD Cargo target directory and
leave CARGO_TARGET_DIR unset to exercise the standard environment. Results:
**131 laboratory tests and 144 runtime tests passed**, with no ignored tests.
Both laboratory smoke journals verified with unchanged heads:

- normal: `84146217504d2265196fb2172c2f48e30d7a2ba887db0b7c4537b4d0282d6779`
- capped: `bfbaf96e27d2b0608680f37450ec987bd6bbecbbedeea6f9765ba106672e21fe`

Use `metron-runtime toolkit inspect CATALOG.json` for the version/hash inventory.
Run the public models under `products/runtime/contracts/` with the pinned tools;
the false Lean file must fail. The operational inventory, full probe outputs,
commands and check logs are retained in the local alpha1 artifact record.

The toolkit smoke checks did not test sandbox escape resistance, actual
cross-task memory authorization, runtime SMT/Lean adapters, coding quality,
batching efficiency or GPU execution. Formal lemmas concern their explicit
mathematical models; the Rust/SQL implementation needs its independent tests.

**Relations:** addresses O-0006's new-submission ambiguity; supports R-0011 and
D-0005 foundation obligations; does not establish full #23 or alpha.1 acceptance.
Revisit after changes to source, tool distributions, confinement or task contracts.
