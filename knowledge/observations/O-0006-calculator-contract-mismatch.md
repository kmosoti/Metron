---
id: O-0006
type: observation
status: active
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# A descriptive calculator objective did not select a coding contract

At source `30cd33805813588149695373d43d9965708313ce`, submit a version-1 task
whose objective requests a Rust calculator to the existing standalone runtime:

```sh
metron-runtime run --mode harness --task calculator.json --run-dir fresh-run
```

The objective requested source, tests, arithmetic and zero-division handling.
The process returned exit 0, phase completed and `verified_success: true`.
Its only published artifact was the fixed fixture result:

```json
{"count":3,"sum":22,"input_digest":"65bc2af2478adeca86e655e783fe44285fb64fb9c1fef24c450a380f35e21999"}
```

No calculator source was generated. The replay head was
`2f5173aef123b9c9b38bb53bfca0d4000bd25d6898cf02542600029ff7af8440`.
The run used scripted inference; it establishes no local-model coding behavior.

Source inspection shows that SumPlanner and the verifier select sum-v1 directly;
the objective is descriptive. The sum verifier established its declared fixture
property, not fulfillment of the requested calculator objective. This is a
task-selection/acceptance boundary gap, not evidence of successful coding.

The original operational task, command, report and artifact remain in the local
calculator-as-is-20261111 run record. A regression must submit that objective in
both modes with missing/unknown coding-family selection and assert rejection
before run state/model loading. Explicit fixture selection must remain possible,
and existing persisted runs must retain their replay identity.

**Relations:** motivates ADR 0022 and #23; narrows O-0004's generic CLI use.
Revisit after explicit task contracts and independent coding acceptance execute.
