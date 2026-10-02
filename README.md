# Metron

**Hexagonal Cognitive Kernel: a laboratory where a resource-bounded system
learns hidden Boolean functions through several representations, and where
every claim has to survive measurement.**

[![ci](https://github.com/kmosoti/Metron/actions/workflows/ci.yml/badge.svg)](https://github.com/kmosoti/Metron/actions/workflows/ci.yml)

The system never sees the function it is trying to learn. It asks questions
through a port, pays for every answer with a receipt, and commits a guess
that the laboratory judges once the episode is over. These rules are not a
convention: the build fails if any of them is broken.

## The picture

```mermaid
flowchart LR
    RUN["Episode runner<br/>journal: a hash chain"] -->|applies| OPS["Operators<br/>observe · transform · consult · commit"]
    OPS -->|writes views| INQ["Inquiry<br/>views in frames,<br/>joined by contracts"]
    OPS -->|probe a row| ORACLE(["Oracle port"])
    OPS -->|read| KNOW(["Knowledge port"])
    OPS -->|ask| SERVICE(["Service port"])
    ORACLE --> TARGET[("Hidden target")]
    ORACLE -.->|receipt| RUN
    KNOW --> POOL[("Hypothesis pool")]
    SERVICE -.->|request file| SESSION["Claude Code session"]
    INQ ==>|committed answer| JUDGE{{"Judge"}}
    subgraph SYSTEM["The system under study"]
        RUN
        OPS
        INQ
    end
    subgraph LAB["The laboratory, sealed"]
        TARGET
        POOL
        JUDGE
    end
```

Four rules hold everywhere, and `cargo test` checks them on every build: the
core is pure, the laboratory owns ground truth, every external operation
leaves a receipt, and representations meet only through explicit transform
contracts that say what they preserve, lose and assume.

## One episode

```mermaid
sequenceDiagram
    autonumber
    participant R as Runner
    participant O as Operator
    participant L as Laboratory
    participant J as Journal
    participant I as Inquiry
    R->>O: apply, with the episode's seeded RNG
    O->>L: probe row 5
    L-->>O: f(row 5) = 1
    L-->>R: a receipt with cost and hashes
    R->>J: journal the receipt before reading the result
    O->>I: write a view under a transform contract
    R->>R: check what this kind of operator may write
    Note over R,I: repeat until an operator commits an answer
    R->>J: the answer and the head hash
    L->>L: judge the answer after the episode
```

The same manifest and seed always produce the same journal head, so any run
can be replayed and audited, including runs that paused to ask a language
model a question.

## What the laboratory has found

**A perfect router would save probes.** Six structural families, ten
task-set seeds. A clairvoyant per-task router beats the best fixed strategy
by about log2 6 probes: exactly the information in the family label.

![Cost ladder: exhaustive, verified affine, greedy over the pool, perfect router](docs/figures/headroom.svg)

**But the label it routes on is not in the probes.** A real router has to
learn the family from the same probes it is trying to save. It cannot: more
prefix only walks it back to the single best strategy.

![Solved rate of the most-survivors router against prefix length](docs/figures/selectors.svg)

**Exact beats hyperdimensional for these objects.** Retrieving every stored
function consistent with a few observed rows, the bitmask index is complete,
smallest and fastest.

![Recall against bytes per entry for exact, Bloom and hyperdimensional indexes](docs/figures/retrieval.svg)

**The system proposes; the laboratory decides.** Compositions extracted from
episodes are judged on held-out targets, never on the episodes that produced
them.

![Held-out pass rates of proposed compositions](docs/figures/promotion.svg)

**Every prior is a hypothesis.** Whatever the design relies on, whether
recalled, cited or derived, carries a test or a primary source.

![The priors ledger by status](docs/figures/priors.svg)

These pictures are drawn from the committed reports by `metron figures`, and
the test suite fails if a report changes without its picture.

## Where it stands

```mermaid
flowchart LR
    M0["M0<br/>the rules are executable"]:::reached
    M1["M1<br/>the lab tells strategies apart"]:::reached
    M2["M2<br/>headroom measured"]:::reached
    M3["M3<br/>routing<br/>closed: the gap was the free label"]:::closed
    M4["M4<br/>candidates judged on held-out targets"]:::reached
    M5["M5<br/>retrieval<br/>hyperdimensional codes dropped"]:::dropped
    M6["M6<br/>an LLM proposes, never decides"]:::reached
    M0 --> M1 --> M2 --> M3
    M1 --> M4
    M1 --> M5
    M0 --> M6
    classDef reached fill:#d3f2e3,stroke:#009E73,color:#0b3d2a
    classDef closed fill:#fde4d6,stroke:#D55E00,color:#5a1e00
    classDef dropped fill:#eceff1,stroke:#8c959f,color:#24292f
```

Milestones are outcomes with exit criteria, never dates. A milestone closed
by a negative result is a result.

## Explore

```mermaid
flowchart TB
    CLI["metron-cli<br/>composition root"] --> LAB["metron-lab<br/>sealed evaluator"]
    CLI --> APP["metron-app<br/>use cases"]
    CLI --> OPS["metron-operators<br/>strategies"]
    CLI --> ADA["metron-adapters<br/>effects"]
    LAB --> CORE["metron-core<br/>pure domain and ports"]
    APP --> CORE
    OPS --> CORE
    ADA --> CORE
```

| If you want to… | Go to |
|---|---|
| see the rules the build enforces | [docs/architecture/invariants.md](docs/architecture/invariants.md) |
| follow an episode end to end, crate by crate | [docs/architecture/overview.md](docs/architecture/overview.md) |
| read the milestones and their exit criteria | [docs/architecture/milestones.md](docs/architecture/milestones.md) |
| know why things are the way they are | [docs/adr](docs/adr/README.md) |
| check what was assumed and what survived | [docs/research/priors.md](docs/research/priors.md) |
| open the raw measurements | [experiments/reports](experiments/reports/README.md) |
| run experiments, or answer a consultation as the language model | [experiments/README.md](experiments/README.md) |
| change the code, as a person or a coding agent | [AGENTS.md](AGENTS.md) |

```sh
cargo test --workspace                                                    # the rules, as tests
cargo run -p metron-cli -- run experiments/manifests/smoke-majority3.json # one episode
cargo run --release -p metron-cli -- headroom experiments/manifests/headroom-arity5.json
cargo run -p metron-cli -- figures                                        # redraw this page
```

Apache-2.0. See [LICENSE](LICENSE).
