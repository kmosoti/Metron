# Metron

Hexagonal Cognitive Kernel: a Rust laboratory for a resource-bounded system
that learns hidden structure through multiple representations, built so
that the experimental rules are enforced by the architecture rather than by
discipline.

The current slice is deliberately small. It establishes the research
objects (inquiry, observation, evidence, view, frame, transform contract,
operator, resource receipt, episode, capability candidate) and four
invariants that `tests/architecture` enforces on every build:

1. the core is pure and dependencies point inward;
2. the laboratory owns ground truth; the system cannot inspect hidden
   targets, the judge or the promotion criteria;
3. every external operation produces a receipt, so cost accounting and
   replay are possible;
4. representations connect only through explicit transform contracts.

Hyperdimensional memory, learned routing, model clients, the mixed Boolean
laboratory and headroom measurement are deliberately not here yet; ADR 0006
says in what order they come and what gates each.

## Quick start

```sh
cargo test --workspace                                     # unit tests + architecture invariants
cargo run -p metron-cli -- run experiments/manifests/smoke-majority3.json
cargo run -p metron-cli -- run experiments/manifests/smoke-majority3-capped.json
cargo run -p metron-cli -- explain experiments/manifests/smoke-majority3.json
cargo run -p metron-cli -- verify experiments/results/<run-dir>/episode.jsonl
```

The first run probes every row of a hidden three-input function and is
judged correct. The second caps the oracle at four probes; the approximate
`complete-table-by-default` contract fills in the rest and the verdict
reports the disagreement. Same manifest and seed, same journal hash.

## Layout

| Path | What |
|---|---|
| `crates/metron-core` | Pure domain and ports |
| `crates/metron-app` | Use cases: operator registry, fixed schedule, episode runner |
| `crates/metron-lab` | Immutable evaluator: sealed hidden targets, oracle, judge, promotion gate, manifests |
| `crates/metron-operators` | Reference operators forming one complete pipeline |
| `crates/metron-adapters` | Effects: clock, files, results |
| `crates/metron-cli` | Composition root (`metron run`, `verify`, `explain`) |
| `experiments/` | Manifests, fixtures, generated results |
| `docs/architecture` | Overview and the invariants with their enforcement |
| `docs/research` | Archived inputs: the proposal and the prior-art review |
| `docs/adr` | Architecture decision records |
| `tests/architecture` | Executable invariants |

Working rules for contributors and coding agents are in `AGENTS.md`.

## License

Apache-2.0. See `LICENSE`.
