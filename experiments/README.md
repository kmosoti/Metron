# Experiments

| Directory | Owner | Contents |
|---|---|---|
| `manifests/` | the laboratory | One JSON file per experiment: name, seed, lab configuration (family, fixture, probe cap), system plan (fixed schedule, budget, step limits). Its hash is written into every journal. |
| `fixtures/` | the laboratory | Hidden targets as data. The system under study never reads these; the composition root hands their text to `metron-lab`, which seals them. |
| `results/` | generated | One directory per run: `manifest.json`, `episode.jsonl` (the hash-chained journal), `episode.json`, `summary.json` (outcome and verdict). Not committed; reproducible from manifest and seed. |

Run one:

```sh
cargo run -p metron-cli -- run experiments/manifests/smoke-majority3.json
cargo run -p metron-cli -- verify experiments/results/<run-dir>/episode.jsonl
cargo run -p metron-cli -- explain experiments/manifests/smoke-majority3.json
```

`smoke-majority3` probes all eight rows and is judged correct.
`smoke-majority3-capped` caps the oracle at four probes; the approximate
`complete-table-by-default` contract fills the rest and the verdict reports
the disagreement. Both runs are deterministic for a given seed.
