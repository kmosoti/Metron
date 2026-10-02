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
| `metron-core` | The research objects and the ports: `Inquiry`, `Question`, `View`, `Frame`, `TransformContract`, `Observation`, `Evidence`, `Operator` (observe, transform, consult, commit), `ResourceReceipt`, `Episode`, `EpisodeCheckpoint`, `CapabilityCandidate`; the `Oracle`, `Knowledge`, `ExternalService`, `Receipts` and `Clock` ports; a seeded RNG; content hashing. | `serde`, `serde_json`, `sha2`, `thiserror` |
| `metron-app` | Use cases: `OperatorRegistry` (validates specs), `Scheduler` / `FixedSchedule` (nested, repeatable), `SurvivorCountSelector` (the hand-authored routing control), `propose` (capability candidates from answered episodes), `EpisodeRunner` (journals every step, receipt and service answer, enforces write rules per kind, suspends to a checkpoint and resumes). | `metron-core` |
| `metron-lab` | The immutable evaluator: `HiddenFunction` (sealed), six structural `Family` generators, `npn` canonicalisation, `HypothesisPool` (public knowledge), `TaskSet` with NPN-clean splits, query `bounds`, `LabWorld` (oracle, knowledge, judge), `Protocol`, `Verdict`, `PromotionGate` (private criteria), `headroom` analysis and `stats`, the `retrieval` workload, `BooleanFixture`, `Manifest`. | `metron-core` |
| `metron-operators` | The strategy building blocks: exhaustive pipeline, version-space filter (optionally family-restricted), greedy split probe, single-survivor table, affine probe and solve, a formula parser, LLM proposal and verified formula-to-table, commit. | `metron-core` |
| `metron-adapters` | Effects: `SystemClock`, file helpers, `ResultsWriter`, the `ClaudeCodeBridge` (file-based LLM protocol), `ReplayService`, `StubService`. No API client, by ADR 0009. | `metron-core` |
| `metron-cli` | Library and binary: `ComposedWorld`, `metron run / resume / answer / replay / headroom / promote / retrieval / verify / explain / npn-classes`. | everything |
| `tests/architecture` | Executable invariants and milestone checks. | everything (dev) |

## An episode, end to end

1. The composition root reads a **manifest** (`experiments/manifests/*.json`)
   and either the **fixture** it names or the **task set** it specifies
   (families, pool size, targets per family, split fractions, seed). The
   target is sealed into a `HiddenFunction` inside a `LabWorld`. From here
   on, nothing outside `metron-lab` can read the table.
2. The lab publishes a `Question`: the task kind, a statement, the public
   parameters (arity, rows, probe cap, and the hypothesis pool's document
   names and hash) and the frame an answer must be in. The pool itself is
   served through the `Knowledge` port: public, static, no receipt. An
   `Inquiry` is created from the question with the manifest's budget.
3. The runner takes operators from a `FixedSchedule`. For each one it:
   - skips it if it is not applicable;
   - applies it with the episode's seeded RNG;
   - **drains the world's receipts and journals them before reading the
     operator's result**;
   - checks the operator wrote only what its kind permits (observe → the
     observations frame; consult → the consultations frame; transform → its
     contract's target frame, from views in its source frames; commit → no
     views), that only observe operators obtained observations, and that
     only consult operators called a service;
   - stamps the contract onto every view a transform wrote and journals the
     write together with the observations it transitively rests on;
   - journals the application with input and output state hashes.
4. The episode ends on an answer, an exhausted schedule, the step limit, a
   stall, the budget, an operator error or a contract violation. The journal
   is a hash chain over a replay projection (wall-clock readings excluded),
   so two runs with the same manifest and seed produce the same head hash.
   If a consult operator's request is unanswered, the episode **suspends**
   instead: the inquiry is restored to its state before the operator, a
   `Suspended` event is journaled, and a checkpoint is written to the run
   directory. `metron answer` and `metron resume` continue it; `metron
   replay` re-runs it from the journaled answers and checks the effective
   events match.
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

## Measuring headroom

`metron headroom <manifest>` runs every strategy in the manifest on every
task of every task-set seed, scores each episode under the manifest's
pre-registered cost model (probe-weighted, with a PAR-style failure cost for
unanswered or incorrect episodes), and writes `cost-table.json`,
`headroom.json` and `headroom.md`: single best solver, virtual best solver,
the gap with a family-stratified bootstrap interval, interquartile means
with intervals, solved rates, and "gap closed" for every column. Selector
columns (routers) can be appended to the same table and scored on the same
scale. Reports that matter are copied to `experiments/reports/`.

## What is deliberately absent

No hyperdimensional memory (dropped by measurement, ADR 0013), no learned
routing (closed on this configuration, ADR 0011), no model API client
(ADR 0009). `docs/architecture/milestones.md` states each
milestone as an outcome with exit criteria.
