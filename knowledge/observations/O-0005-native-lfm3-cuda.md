---
id: O-0005
type: observation
status: active
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Native LFM3 CUDA execution

## Established scope

LFM3-1.7B Q4_K_M generated a correct JSON sum through the native mistral.rs
SDK in Metron's isolated worker. This establishes one GPU execution, not a
qualified production task, autonomous coding capability or the combined
concurrency/latency target in [ADR 0021](../../docs/adr/0021-local-inference.md).

The source is `f45f7c27d5c7a2c4ae12b463384de0bc5473fa9c`; the worker executable
SHA-256 is `f74e87d7ee2d7dbc134b04abcaaa8c5af82d303c5ae3fc9e71734cd1e4148b81`. The SDK is pinned to
`fb4cdc3730dc64b366724e7637a205a8b7747499`. Model and auxiliary identities are
listed in the [runtime setup](../../docs/agent-harness.md#local-worker-and-comparisons).

## Request and execution evidence

The worker was invoked once as `metron-inference-worker`. A local protocol client
sent one bounded `WorkerRequest::Generate` using the frozen Solver/Sum recipe,
then captured the final observation and closed the worker. Inference ran in Rust;
the client only handled framing and NVIDIA monitoring. There was no serving
endpoint or backend tool execution.

```json
{"values":[17,-4,9]}
```

The generated and schema-validated output was:

```json
{"count":3,"sum":22}
```

| Property | Observed configuration/result |
| --- | --- |
| Logical model | `LiquidAI/LFM3-1.7B` |
| GGUF | `LFM3-1.7B-Q4_K_M.gguf`, official revision `cd0c74c575b5c416f3011b5b4c298077f26ca8c2` |
| Execution | NVIDIA GeForce RTX 3060 Laptop GPU, 6 GiB, driver 615.78.08 |
| Host | Fedora 44; Intel Core i7-10750H; nominal 32 GB RAM |
| Context/output limits | 2,048 context tokens, 512 reserved generated tokens |
| Actual usage | 60 prompt tokens, 17 generated tokens |
| Process GPU memory | Sampled peak 1,238 MiB |
| Worker-specific activity | NVIDIA process monitoring reached 99% SM activity |
| Device check | Worker configuration reported CUDA and required a CUDA device |

The CUDA build used toolkit 12.9.86, G++ 14.3, compute capability 8.6 and
`NVCC_APPEND_FLAGS=--Ofast-compile=max`. This recorded compiler workaround can
change kernel performance; see [R-0010](../research/R-0010-lfm3-native-inference.md).
An earlier Q6_K execution also generated the correct JSON sum with a sampled
1,558 MiB process peak. Neither short request establishes long-context memory
requirements or an OOM threshold.

## Implementation verification

Rust `1.100.0-beta.12` was used. Runtime formatting and Clippy with warnings denied
passed; 135 runtime tests passed, including authorization, accounting, streaming,
arrival-snapshot bounds, crash/uncertainty and replay checks. Laboratory source
remains separate. Its normal and capped smoke journals retain heads
`84146217504d2265196fb2172c2f48e30d7a2ba887db0b7c4537b4d0282d6779` and
`bfbaf96e27d2b0608680f37450ec987bd6bbecbbedeea6f9765ba106672e21fe`.
Previously recorded v1 agent and harness databases replay to their original heads
with chain, accounting, projection and behavior audits valid.

## Comparison outcome

The fixed operational comparison uses 8,192-token profiles, reserved output,
Q4/Q6/Q8 and concurrency 1/2/4/8. It includes one accounted bootstrap before
concurrent warmup; cold loading is not charged against other requests' queue
wait. All twelve profiles failed the accounted warmup entry gate. The command exited
1 with `comparisons did not complete` and `chosen: null`. No scored repetitions,
near-8K inputs or staggered cases ran, so the combined quality/latency goal was
not demonstrated and no production profile was promoted.

| Quantization | Admitted requests | Final observations / receipts | Unknown outcomes | Maximum sampled process MiB | Minimum sampled free MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Q4_K_M | 19 | 10 | 9 | 1,400 | 4,273 |
| Q6_K | 19 | 6 | 13 | 1,686 | 3,987 |
| Q8_0 | 19 | 11 | 8 | 2,102 | 3,571 |

These are short-entry observations across concurrency 1/2/4/8, not matched
successful long-context comparisons. The effective profile, artifact and binary
identities stayed fixed within each profile. Cold means a new worker process;
OS file and driver caches were not cleared.

Completed selector replies violated the frozen closed schema. For example, Q4
returned `{"tool":"read_fixture","stage":"need_input"}`; Q6 and Q8 included
`parameters`, and one Q6 reply concatenated several objects. The expected output
has exactly one `tool` field. These remain charged observations, not zero-cost
rejections. Some larger cohorts ended without a final bound observation; their
cause was not established and their usage remains unknown. No request was
reported as a known rejection, and unknown sessions remain blocked.

All sampled process peaks stayed below the 4 GiB operational target and sampled
free memory stayed above 1 GiB during these entry checks. This does not establish
an OOM boundary or long-context capacity. There is no accepted throughput ranking
or quantization recommendation from this incomplete comparison. The frozen
recipe and schema were retained without tuning against these failures.

**Relations:** supports R-0010 and priors 51/53/56; validates limited feasibility,
not production qualification. Revisit after a model/backend/build change, revised
output constraints or contradictory execution evidence. Issue #7's sandboxed
coding and later release gates remain open.
