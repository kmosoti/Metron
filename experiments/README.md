# Experiments

| Directory | Owner | Contents |
|---|---|---|
| `manifests/` | the laboratory | One JSON file per experiment: name, seed, lab configuration (family, fixture, probe cap), system plan (fixed schedule, budget, step limits). Its hash is written into every journal. |
| `fixtures/` | the laboratory | Hidden targets as data. The system under study never reads these; the composition root hands their text to `metron-lab`, which seals them. |
| `results/` | generated | One directory per run: `run.json`, `manifest.json`, `episode.jsonl` (the hash-chained journal), `episode.json`, `summary.json` (outcome and verdict), `checkpoint.json` while suspended, and `llm/requests` and `llm/responses` for consultations. Headroom runs write `cost-table.json`, `headroom.json` and `headroom.md`. Not committed; reproducible from manifest and seed. |
| `reports/` | committed | Reports worth keeping, named with the manifest hash, plus the decision they led to. |

## Commands

```sh
cargo test --workspace                                      # unit tests + architecture invariants
cargo run -p metron-cli -- run experiments/manifests/smoke-majority3.json
cargo run -p metron-cli -- verify experiments/results/<run-dir>/episode.jsonl
cargo run -p metron-cli -- explain experiments/manifests/smoke-majority3.json
cargo run -p metron-cli -- headroom experiments/manifests/headroom-arity5.json
cargo run -p metron-cli -- promote experiments/manifests/promotion-arity5.json
cargo run --release -p metron-cli -- retrieval experiments/manifests/retrieval-arity6.json
cargo run -p metron-cli -- run experiments/manifests/llm-consult-majority3.json   # suspends with a prompt
cargo run -p metron-cli -- answer <run-dir> --text "(x0 & x1) | (x0 & x2) | (x1 & x2)" --by "me"
cargo run -p metron-cli -- resume <run-dir>
cargo run -p metron-cli -- replay <run-dir>
cargo run -p metron-cli -- figures                          # redraw docs/figures from reports/
```

The smoke run probes every row of a hidden three-input function and is
judged correct. The headroom run compares fixed strategies on generated
task sets and reports how much an ideal per-task selector would save. The
consultation run suspends after four probes with a prompt for a language
model; the Claude Code session answers it, the proposal is verified against
the observations, and the episode replays from its journal without the
session. `metron figures` redraws the front page's figures from the
committed reports; `metron figures --check` and the test suite fail when a
report changes and its figure does not.

## Manifests

| Manifest | What it does |
|---|---|
| `smoke-majority3` | Probes all eight rows of a fixture; judged correct. |
| `smoke-majority3-capped` | Caps the oracle at four probes; the approximate `complete-table-by-default` contract fills the rest and the verdict reports the disagreement. |
| `llm-consult-majority3` | Probes four rows, suspends with a prompt for the language-model service, verifies the proposal on resume, commits if consistent, otherwise falls back to the exhaustive pipeline. |
| `headroom-arity5`, `headroom-arity6` | Ten task-set seeds, six families, strategies from exhaustive to family-restricted version spaces; `metron headroom` reports SBS, VBS and gap closed. |
| `promotion-arity5` | Four fixed strategies; `metron promote` proposes candidate compositions from the train split and the laboratory's gate judges them on the test split (ADR 0012). |
| `retrieval-arity6` | A store of about 5,000 solved functions; `metron retrieval` compares exact scan, bitmask index, Bloom filters and hyperdimensional codes on recall, bytes and nanoseconds (ADR 0013). |
| `selectors-arity5`, `selectors-arity6` | The same, plus `survivor-count` selector columns (rule `most` with prefix 0 to 6, and `fewest` as the negative control) and two extra cost models. The M3 control measurement (ADR 0011). |
| `structure-pilot-arity8` | The pre-registered pilot of ADR 0015 on seeds 900 to 904: every cascade of the three structural learners at verification counts 4, 8, 12 and 16. It fixed the count at 4. |
| `structure-arity8` | The structure-keyed laboratory of ADR 0015: affine, symmetric and three-variable junta targets at arity 8, no pool, the structure promise published instead. Label-free cascades, single learners with the truth table as fallback, and blind learners as reference columns. Ten seeds, 300 tasks. |

All runs are deterministic for a given manifest and seed; consultation runs
replay from their journals.
