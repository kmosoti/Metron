# Experimental bounded coding

The coding entry point connects a version-3 task to workspace tools, a local
model, scoped notes and independent artifact acceptance. Both `agent` and
`harness` modes use the same admission, reservation, receipt and verification
boundaries. The standalone planner does not instantiate `metron-agent`.

Coding remains experimental until the native task and resource gates in
[D-0005](../../knowledge/decisions/D-0005-alpha1-delivery.md) pass. A successful
controlled script, a model response or a candidate-written test does not select
a production profile. See [ADR 0022](../../docs/adr/0022-alpha1-coding-and-memory.md)
for the authority diagram.

Native coding recipe v3 uses LFM3's XML function-call format through the Rust SDK.
The admitted model step supplies its phase/tool catalog separately from prompt
text. A versioned tokenizer correction preserves the GGUF vocabulary and changes
the 14 user-defined token flags required by that format; model load verifies the
source artifacts and the SDK checks every token ID. Its profile tokenizer
identity describes that transform; the worker separately logs the generated file
hash. Native calls are proposals and pass the same domain and admission checks
as other coding actions. See [the compatibility evidence](../../knowledge/observations/O-0008-coding-loop-and-confinement.md#native-tool-call-tokenizer-incompatibility).

## Inputs and invocation

Provide separate absolute paths for an owner-private workspace and the private
run directory. The workspace contains only candidate-accessible project files;
keep model artifacts, credentials, memory databases, journals and evaluator
inputs outside it. The run directory must not already exist for a fresh task.
Process scratch must reside on disk. Linux user namespaces, bubblewrap, a user
systemd manager and cgroup v2 are required; isolation failure has no fallback.

The operator supplies these bounded JSON inputs:

| Input | Contract |
| --- | --- |
| Task | `CodingTaskData`: schema 3, family `coding-v1`, unique task ID, principal/project, objective, acceptance/tool/model digests, optional memory collection, request/tool budgets and decision ceiling. |
| Policy | `CodingPolicy`: matching principal/project/collection, workspace root, allowed tools/programs, mutation approval and file/output/process limits. |
| Catalog | Pinned executable paths, byte digests and version prefixes; see [toolkit inventory](README.md). A catalog grants no authority. |
| Profile | Hardware-bound Q4 LFM3 profile with pinned artifact, tokenizer, template, worker and recipe identities. |
| Acceptance | A frozen built-in spec, currently `{"version":1,"kind":"calculator-v1"}` or the frozen nonempty-line repair spec. Custom trusted callers can implement the public `CodingVerifier` port. |

Compute identifiers through the Rust serializer rather than hashing an external
JSON reformatting:

```sh
metron-runtime coding identities --catalog catalog.json --profile profile.json --acceptance acceptance.json
metron-runtime coding experiment --experimental --run-dir /absolute/private/run --task task.json --policy policy.json --catalog catalog.json --acceptance acceptance.json --profile profile.json --worker /absolute/pinned/metron-inference-worker --artifact /absolute/models/LFM3-1.7B-Q4_K_M.gguf --mode agent
```

Use `--mode harness` for the standalone caller. `--script script.json` replaces
the profile/worker/artifact arguments for controlled tests; it uses a distinct
model identity and never represents native generation. Native attempts require
`LiquidAI/LFM3-1.7B` Q4_K_M and verified local CUDA hardware when the profile
selects CUDA. Artifacts are loaded offline; there is no hosted fallback.

## Actions and observations

The first model response records a plan. Later responses propose one typed action:
list, literal ripgrep search, bounded read, compare-and-swap edit, a permitted
Rust program, private JJ snapshot/diff, SMT, Lean, scoped memory or finish.
Use workspace-relative paths and JSON `null` for the root directory:

```json
{"tool":"list","directory":null}
{"tool":"read","path":"README.md","first_line":1,"line_count":40}
{"tool":"run","program":"cargo","args":["test","--offline"],"directory":null}
```

An edit with `expected_digest: null` creates a missing file. Replacing a file
requires its complete observed digest. Read excerpts retain a digest of the
complete file. Tool observations record status, truncation, file references and
whether the process tree is closed. An unavailable file is not a reason to
repeat an unchanged failed read.

The current controller retains the model's plan, a compact action summary and
at most two bounded tool views. The model sees explicit truncation and file
hashes needed for edits; the journal and state retain evidence references and
full-output hashes. Planner/coder/reviewer personas do not change grants.
Each prior controller and recipe keeps its own replay behavior.

JJ operates on a private bounded source copy; its repository metadata is never
mounted from the caller's checkout. Receipts identify content trees rather than
clock-dependent commit IDs. Ripgrep searches a private source copy literally.
Cargo and rustc run with an isolated network, clean environment, disk scratch
and cgroup process/memory/deadline controls. Host toolchains and shared libraries
remain trusted inputs; executable pinning does not qualify an entire OS image.
The disk reserve monitor is sampled and is not a filesystem quota.

## Scoped notes

The host binds every query and append to the task's principal, project and
collection. Model parameters cannot select a different scope. Grant a collection
explicitly before using it in a task:

```sh
metron-runtime coding memory-grant --db /absolute/private/notes.sqlite --principal operator --project project --collection notes
metron-runtime coding memory-revoke --db /absolute/private/notes.sqlite --principal operator --project project --collection notes
```

Notes are immutable model-authored claims, with source task/evidence and content
hashes. Contradictory notes remain separate. They never become verification or
permissions. Grants are checked transactionally on each operation and again
before model requests that could expose retrieved context. Revocation blocks
further use of that context. SQLite stores canonical notes; FTS5 is rebuildable.
Database files and sidecars must be private regular files.

## Completion and recovery

`finish` requests verification of a frozen file set and its implied directories.
The grader compiles that immutable source separately and runs its sealed binary
with a fresh scratch directory for each private case. The candidate cannot
access expected outputs or the grader's build cache. Only an artifact-bound
trusted verdict completes the task. Unknown cleanup or grading remains blocked;
a successful candidate test does not set the verdict.

```sh
metron-runtime coding resume --run-dir /absolute/private/run
metron-runtime coding audit --run-dir /absolute/private/run
```

Audit is read-only and uses recorded observations. It needs neither the model
nor the original workspace, and checks retained artifact/report integrity when
present. Resume never silently redispatches an uncertain effect. Outstanding
reservations remain held. An unresolved process marker prevents workspace reuse
until cleanup has been established by an operator; deleting a marker alone is
not cleanup evidence.

## Local verification

Run the ordinary runtime workspace checks. Physical tool probes are explicitly
ignored by default because they require a pinned local catalog and the Linux
sandbox. Execute them with a catalog stored on disk:

```sh
METRON_CODING_PROBE_CATALOG=/absolute/disk/tool-catalog.json cargo test --locked -p metron-host --bin metron-runtime -- --ignored
```

These probes cover admitted JJ/rg/SMT/Lean effects and receipt replay, namespace
and descendant cleanup, plus frozen-build execution and case isolation. Native
calculator/repair acceptance and R-0008/R-0009 efficiency qualification are
separate release evidence.
