# Metron

Hexagonal Cognitive Kernel: a Rust laboratory for a resource-bounded system
that learns hidden structure through multiple representations, built so
that the experimental rules are enforced by the architecture rather than by
discipline.

The repository establishes the research objects (inquiry, observation,
evidence, view, frame, transform contract, operator, resource receipt,
episode, capability candidate) and four invariants that `tests/architecture`
enforces on every build:

1. the core is pure and dependencies point inward;
2. the laboratory owns ground truth; the system cannot inspect hidden
   targets, the judge or the promotion criteria;
3. every external operation produces a receipt, so cost accounting and
   replay are possible;
4. representations connect only through explicit transform contracts.

On top of that it holds the mixed Boolean laboratory (six structural
families at arities up to 8, NPN-clean splits, query rulers), the strategy
operators the laboratory compares, a headroom harness that reports single
best solver, virtual best solver and gap closed, capability promotion
judged by the laboratory on held-out targets, a retrieval workload that
compares exact and sketch indexes, and language-model consultation that
enters through a Claude Code session rather than an API client. Milestones are outcomes with exit criteria
(`docs/architecture/milestones.md`); learned routing is gated on the measured
headroom (ADR 0006), and there is no model API client by design (ADR 0009).

## Quick start

```sh
cargo test --workspace                                      # unit tests + architecture invariants
cargo run -p metron-cli -- run experiments/manifests/smoke-majority3.json
cargo run -p metron-cli -- headroom experiments/manifests/headroom-arity5.json
cargo run -p metron-cli -- promote experiments/manifests/promotion-arity5.json
cargo run --release -p metron-cli -- retrieval experiments/manifests/retrieval-arity6.json
cargo run -p metron-cli -- run experiments/manifests/llm-consult-majority3.json   # suspends with a prompt
cargo run -p metron-cli -- answer <run-dir> --text "(x0 & x1) | (x0 & x2) | (x1 & x2)" --by "me"
cargo run -p metron-cli -- resume <run-dir>
cargo run -p metron-cli -- replay <run-dir>
cargo run -p metron-cli -- verify <run-dir>/episode.jsonl
```

The smoke run probes every row of a hidden three-input function and is
judged correct. The headroom run compares fixed strategies on generated
task sets and reports how much an ideal per-task selector would save. The
consultation run suspends after four probes with a prompt for a language
model; the Claude Code session answers it, the proposal is verified against
the observations, and the episode replays from its journal without the
session.

## Layout

| Path | What |
|---|---|
| `crates/metron-core` | Pure domain and ports |
| `crates/metron-app` | Use cases: operator registry, fixed schedule, episode runner |
| `crates/metron-lab` | Immutable evaluator: sealed hidden targets, oracle, judge, promotion gate, manifests |
| `crates/metron-operators` | Reference operators forming one complete pipeline |
| `crates/metron-adapters` | Effects: clock, files, results |
| `crates/metron-cli` | Composition root (`metron run`, `verify`, `explain`) |
| `experiments/` | Manifests, fixtures, generated results, committed reports |
| `docs/architecture` | Overview and the invariants with their enforcement |
| `docs/research` | Archived inputs and the priors ledger: every prior the design relies on, with its test |
| `docs/adr` | Architecture decision records |
| `tests/architecture` | Executable invariants |

Working rules for contributors and coding agents are in `AGENTS.md`.

## License

Apache-2.0. See `LICENSE`.
