# Architecture overview

Metron is a laboratory for studying a resource-bounded system that learns
hidden structure through multiple representations. The repository encodes
the experimental rules in its architecture, so the first slice is
deliberately boring: it establishes the objects and the four invariants in
`invariants.md`, and nothing else.

## Crates and the direction of dependency

```
                    ┌─────────────────────────────┐
                    │        metron-cli           │  composition root
                    │  (the only crate that sees  │
                    │   lab + app + operators +   │
                    │   adapters together)        │
                    └──┬──────┬──────┬───────┬────┘
                       │      │      │       │
          ┌────────────▼┐ ┌───▼────┐ ┌▼──────────┐ ┌▼───────────────┐
          │ metron-lab  │ │metron- │ │ metron-   │ │ metron-adapters│
          │ immutable   │ │  app   │ │ operators │ │ persistence,   │
          │ evaluator   │ │use     │ │ reference │ │ clock, (later) │
          │ ground truth│ │cases   │ │ operators │ │ models         │
          └──────┬──────┘ └───┬────┘ └─────┬─────┘ └───────┬────────┘
                 │            │            │               │
                 └────────────┴─────┬──────┴───────────────┘
                                    ▼
                           ┌─────────────────┐
                           │   metron-core   │  pure domain + ports
                           │ serde, sha2 only│
                           └─────────────────┘
```

Every arrow points at `metron-core`. The four middle crates do not know
about each other. `tests/architecture` reads the manifests and sources and
fails the build if an arrow appears that is not in this picture.

| Crate | Role | May depend on |
|---|---|---|
| `metron-core` | The research objects and the ports: `Inquiry`, `Question`, `View`, `Frame`, `TransformContract`, `Observation`, `Evidence`, `Operator`, `ResourceReceipt`, `Episode`, `CapabilityCandidate`; the `Oracle` and `Clock` ports; a seeded RNG; content hashing. | `serde`, `serde_json`, `sha2`, `thiserror` |
| `metron-app` | Use cases: `OperatorRegistry` (validates specs), `Scheduler` / `FixedSchedule`, `EpisodeRunner` (journals every step and receipt, enforces write rules). | `metron-core` |
| `metron-lab` | The immutable evaluator: `HiddenFunction` (sealed), `LabWorld` (the oracle and the judge), `Protocol`, `Verdict`, `PromotionGate` (private criteria), `BooleanFixture`, `Manifest`. | `metron-core` |
| `metron-operators` | Reference operators that form one complete pipeline: probe → partial table → complete table → commit. | `metron-core` |
| `metron-adapters` | Effects: `SystemClock`, file helpers, `ResultsWriter`. Model clients will live here when the programme reaches them. | `metron-core` |
| `metron-cli` | `metron run / verify / explain`. Wires everything together. | everything |
| `tests/architecture` | Executable invariants. | everything (dev) |

## An episode, end to end

1. The composition root reads a **manifest** (`experiments/manifests/*.json`)
   and the **fixture** it names. The fixture is sealed into a
   `HiddenFunction` inside a `LabWorld`. From here on, nothing outside
   `metron-lab` can read the table.
2. The lab publishes a `Question`: the task kind, a statement, the public
   parameters (arity, rows, probe cap) and the frame an answer must be in.
   An `Inquiry` is created from it with the manifest's budget.
3. The runner takes operators from a `FixedSchedule`. For each one it:
   - skips it if it is not applicable;
   - applies it with the episode's seeded RNG;
   - **drains the world's receipts and journals them before reading the
     operator's result**;
   - checks the operator wrote only what its kind permits (observe → the
     observations frame; transform → its contract's target frame, from views
     in its source frames; commit → no views) and that only observe
     operators obtained observations;
   - stamps the contract onto every view a transform wrote and journals the
     write together with the observations it transitively rests on;
   - journals the application with input and output state hashes.
4. The episode ends on an answer, an exhausted schedule, the step limit, a
   stall, the budget, an operator error or a contract violation. The journal
   is a hash chain over a replay projection (wall-clock readings excluded),
   so two runs with the same manifest and seed produce the same head hash.
5. The lab judges the inquiry's committed answer against the hidden target
   and returns a `Verdict`. The system never sees it during the episode.
6. The results writer stores the manifest, the journal (`episode.jsonl`),
   the full episode and a summary with the verdict under
   `experiments/results/`.

## Vocabulary

* **Inquiry** — the system's working state about one question. Not a
  generic "capsule": it has exactly the parts the research needs (views,
  observations, answer, cost, budget).
* **Frame** — a representation system (`observations`,
  `truth-table/partial`, `truth-table/complete`, …).
* **View** — the inquiry's state in one frame, with its `Derivation`: which
  operator wrote it, from which views, incorporating which observations,
  under which contract.
* **TransformContract** — `from` frames, `to` frame, `preserves`, `loses`,
  `assumes`, and `fidelity` (exact or approximate with a description).
* **Observation** — a fact obtained through the `Oracle` port, always
  carrying the id of the receipt that paid for it.
* **Evidence** — a link from an answer (or later, a claim) to an
  observation, with the operator path in between. Independence is counted
  in distinct observations.
* **ResourceReceipt** — issued by the port that executed an external
  operation; cost, request and response hashes, observations produced.
* **Episode** — one bounded run with its journal and outcome.
* **CapabilityCandidate** — a proposed reusable composition. Only the lab's
  `PromotionGate` can promote it.

## What is deliberately absent

No hyperdimensional memory, no learned routing, no model clients, no mixed
Boolean families, no SBS/VBS harness. ADR 0006 records why and what has to
be true before each is added.
