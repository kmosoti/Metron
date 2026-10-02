# Experiments

| Directory | Owner | Contents |
|---|---|---|
| `manifests/` | the laboratory | One JSON file per experiment: name, seed, lab configuration (family, fixture, probe cap), system plan (fixed schedule, budget, step limits). Its hash is written into every journal. |
| `fixtures/` | the laboratory | Hidden targets as data. The system under study never reads these; the composition root hands their text to `metron-lab`, which seals them. |
| `results/` | generated | One directory per run: `run.json`, `manifest.json`, `episode.jsonl` (the hash-chained journal), `episode.json`, `summary.json` (outcome and verdict), `checkpoint.json` while suspended, and `llm/requests` and `llm/responses` for consultations. Headroom runs write `cost-table.json`, `headroom.json` and `headroom.md`. Not committed; reproducible from manifest and seed. |
| `reports/` | committed | Reports worth keeping, named with the manifest hash, plus the decision they led to. |

Run one:

```sh
cargo run -p metron-cli -- run experiments/manifests/smoke-majority3.json
cargo run -p metron-cli -- verify experiments/results/<run-dir>/episode.jsonl
cargo run -p metron-cli -- explain experiments/manifests/smoke-majority3.json
```

| Manifest | What it does |
|---|---|
| `smoke-majority3` | Probes all eight rows of a fixture; judged correct. |
| `smoke-majority3-capped` | Caps the oracle at four probes; the approximate `complete-table-by-default` contract fills the rest and the verdict reports the disagreement. |
| `llm-consult-majority3` | Probes four rows, suspends with a prompt for the language-model service, verifies the proposal on resume, commits if consistent, otherwise falls back to the exhaustive pipeline. |
| `headroom-arity5`, `headroom-arity6` | Ten task-set seeds, six families, strategies from exhaustive to family-restricted version spaces; `metron headroom` reports SBS, VBS and gap closed. |
| `selectors-arity5`, `selectors-arity6` | The same, plus `survivor-count` selector columns (rule `most` with prefix 0 to 6, and `fewest` as the negative control) and two extra cost models. The M3 control measurement (ADR 0011). |

All runs are deterministic for a given manifest and seed; consultation runs
replay from their journals.
