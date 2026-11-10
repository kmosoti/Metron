---
id: R-0001
type: research
status: source-audited
updated: 2026-11-10
added_by: agent
human_ratified: false
source_proposal: 4c3781daa7211efd39bcdb95dcb15491be1abe39
---

# Agent and harness source audit

## Method and limits

Primary repository metadata and README content were retrieved at the revisions
below. Selected architecture, extension, memory and license files were inspected
at those same revisions. Paper claims were checked against the linked versioned
papers. These observations establish what each source documents. Upstream
benchmarks and sandbox claims have not been reproduced in Metron.

A README entry is evidence of a documented interface or claim. It provides no
independent evidence of task quality, isolation, deployment suitability or
license compatibility of the complete dependency tree. Repository license
metadata and badges require file-level confirmation before adopting code.

## Mechanisms with architectural consequences

Pi documents a compact extensible harness with separate agent and model packages.
Its extension guide states that extensions share the process's OS permissions.
Metron can test a small core and an explicit extension lifecycle while keeping
host authorization at the effect boundary. [Pi README](https://github.com/earendil-works/pi/blob/7f5b609d1c299bd2038b4732d782e703c9bbc513/README.md),
[extension trust boundary](https://github.com/earendil-works/pi/blob/7f5b609d1c299bd2038b4732d782e703c9bbc513/packages/coding-agent/docs/extensions.md).

DeepSeek Harness composes typed services, events and reversible registrations
through Cordis and ordered profiles. Its architecture makes even the agent loop
replaceable. Metron's proposed trusted-native registry can borrow dependency and
cleanup semantics. Policy replacement remains confined to reviewed host code.
[DeepSeek architecture](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/docs/architecture.md),
[Cordis primer](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/docs/cordis-primer.md).

Hermes documents persistent skills and cross-session memory. Its memory provider
interface explicitly scopes instances and lifecycle operations to profiles.
Metron should make memory provenance, scope and cancellation observable before
adding persistent autonomy. A memory provider's documented API establishes no
measured improvement on Metron tasks. [Hermes README](https://github.com/NousResearch/hermes-agent/blob/7ad8d980a1675d2dc0b5a22c166bebefcff8899a/README.md),
[memory provider](https://github.com/NousResearch/hermes-agent/blob/7ad8d980a1675d2dc0b5a22c166bebefcff8899a/agent/memory_provider.py).

Harness² holds model weights fixed and refines execution configuration through
Propose–Probe–Compose. It also accumulates a separate improvement harness across
tasks. The paper reports results across three benchmarks and four configurations.
Those reported gains motivate a later matched-cost experiment. Metron needs its
own sealed evaluation, fixed budgets, regression checks and reversible promotion.
The original proposal's shorthand Propose–Probe–Compare refers to Metron's
comparison gate. The upstream method's name is Propose–Probe–Compose.
[Harness² paper v1](https://arxiv.org/html/2610.14224v1),
[implementation](https://github.com/google-research/harness2/blob/6868817518876de8fd170c6a8354daca273b6191/README.md).

The harness source-code study describes eleven systems, seven subsystems and
29 patterns. It reports deterministic retrieval in its inspected corpus. That
observation is bounded by the selected revisions and does not rule out vector
retrieval in a different workload. Its dated submission metadata and versioned
paper identify the evidence independently of the identifier's spelling.
[Source-code study v1](https://arxiv.org/abs/2609.00006v1).

Rig and Swiftide already document Rust agent/tool APIs. Goose documents a Rust
runtime across CLI and application surfaces. Their existence reduces the novelty
of a Rust agent. The proposed Metron contribution is an independently usable
pure cognition library with a separately testable effect contract. In alpha.1,
compare a thin provider adapter using an existing library with a direct adapter
before accepting duplicate orchestration or provider code.
[Rig](https://github.com/0xPlaygrounds/rig/blob/e2a39ecda5a1b3a02d5f0c1d7809cf7ccdcb6aa8/README.md), [Swiftide](https://github.com/bosun-ai/swiftide/blob/e41a89fc18d8998c87f23bfcc4135f3a43518b9e/README.md),
[Goose](https://github.com/aaif-goose/goose/blob/629fb19cb9cdc1d2e3d15870eff23859200852c1/README.md).

## Pinned repository inventory

Each row is a documentation inspection. The source link pins the complete commit
SHA. No row represents a passing Metron integration or an execution audit.

| Source | Documented focus or role in the comparison |
| --- | --- |
| [earendil-works/pi @ `7f5b609d1c29`](https://github.com/earendil-works/pi/blob/7f5b609d1c299bd2038b4732d782e703c9bbc513/README.md) | Minimal extensible harness, separate runtime and model packages |
| [deepseek-ai/deepseek-harness @ `fcc0ad462172`](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/README.md) | Cordis service composition, developer-preview compatibility changes |
| [NousResearch/hermes-agent @ `7ad8d980a167`](https://github.com/NousResearch/hermes-agent/blob/7ad8d980a1675d2dc0b5a22c166bebefcff8899a/README.md) | Persistent assistant, skills and memory providers |
| [google-research/harness2 @ `686881751887`](https://github.com/google-research/harness2/blob/6868817518876de8fd170c6a8354daca273b6191/README.md) | Task-local refinement and cross-task improvement configuration |
| [openai/codex @ `ad3d3521eddc`](https://github.com/openai/codex/blob/ad3d3521eddcee24cd6ce0d335d597544578c5e1/README.md) | Local coding assistant and Rust workspace reference |
| [aaif-goose/goose @ `629fb19cb9cd`](https://github.com/aaif-goose/goose/blob/629fb19cb9cdc1d2e3d15870eff23859200852c1/README.md) | Rust CLI, desktop and embedding surfaces, canonical aaif-goose repository |
| [0xPlaygrounds/rig @ `e2a39ecda5a1`](https://github.com/0xPlaygrounds/rig/blob/e2a39ecda5a1b3a02d5f0c1d7809cf7ccdcb6aa8/README.md) | Rust LLM library and plugin-based terminal harness |
| [bosun-ai/swiftide @ `e41a89fc18d8`](https://github.com/bosun-ai/swiftide/blob/e41a89fc18d8998c87f23bfcc4135f3a43518b9e/README.md) | Rust agent harness, typed task graphs and RAG pipelines |
| [zeroclaw-labs/zeroclaw @ `a4beef9a6d69`](https://github.com/zeroclaw-labs/zeroclaw/blob/a4beef9a6d6965ff3b5290f94ff97bd0b4bcbb85/README.md) | Rust runtime with model, channel and tool adapters |
| [RightNow-AI/openfang @ `acf2587e46be`](https://github.com/RightNow-AI/openfang/blob/acf2587e46be174c10200489c9a2d23a39a98aeb/README.md) | Rust agent OS reference, breadth deferred for Metron |
| [modelcontextprotocol/rust-sdk @ `f2b0ddf113d1`](https://github.com/modelcontextprotocol/rust-sdk/blob/f2b0ddf113d1ed426b77a81511eac6e4bffa0ac4/README.md) | Official Tokio-based MCP SDK, boundary adapter candidate |
| [agentclientprotocol/agent-client-protocol @ `f3ed0fca2fd6`](https://github.com/agentclientprotocol/agent-client-protocol/blob/f3ed0fca2fd6d85ef7ae77cfd074cc8605e490dc/README.md) | ACP wire types and pointers to the runtime SDK |
| [SWE-agent/SWE-agent @ `3ea751c087f3`](https://github.com/SWE-agent/SWE-agent/blob/3ea751c087f32b16e039a2233dd6eefecef325d5/README.md) | Tool-oriented coding-agent research reference |
| [SWE-agent/mini-swe-agent @ `04d809ceab9d`](https://github.com/SWE-agent/mini-swe-agent/blob/04d809ceab9df28f9adaed044884180159172930/README.md) | Small bash-based coding loop, complexity baseline |
| [OpenHands/software-agent-sdk @ `848d6d331dc9`](https://github.com/OpenHands/software-agent-sdk/blob/848d6d331dc95e392eca3e0ec12550a94262e35c/README.md) | SDK, agent server and workspace/event boundaries |
| [Aider-AI/aider @ `5dc9490bb35f`](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/README.md) | Git-oriented code editing workflow |
| [anthropics/claude-code @ `ab49b8966bd8`](https://github.com/anthropics/claude-code/blob/ab49b8966bd80199f493a31dad9396f8743e73a7/README.md) | Terminal coding product reference, code reuse license unresolved |
| [google-gemini/gemini-cli @ `e5d88fb2c036`](https://github.com/google-gemini/gemini-cli/blob/e5d88fb2c036ce7e0c27131469cd963221b9d9e7/README.md) | Coding CLI with current enterprise and benchmarking scope caveats |
| [anomalyco/opencode @ `028ce1c6e199`](https://github.com/anomalyco/opencode/blob/028ce1c6e1991c9bd4411798e85d9464fb40ce0e/README.md) | Coding-agent comparison candidate, deeper feature assertions deferred |
| [cline/cline @ `d9b24c20317a`](https://github.com/cline/cline/blob/d9b24c20317af6412b7a2c32f6f6453420500961/README.md) | Tools with human approvals and rules/skills |
| [RooCodeInc/Roo-Code @ `b867ec914575`](https://github.com/RooCodeInc/Roo-Code/blob/b867ec9145750d0ae1ff7f02d35406e9bf2a0b16/README.md) | Historical IDE agent, repository archive flag true |
| [mistralai/mistral-vibe @ `b20313e01d69`](https://github.com/mistralai/mistral-vibe/blob/b20313e01d6926d4191da89c32d5da11e3a0a060/README.md) | Terminal coding and programmatic-mode comparison |
| [openclaw/openclaw @ `50b29a94f244`](https://github.com/openclaw/openclaw/blob/50b29a94f2447772e76b379edbdf9a448a2932b3/README.md) | Persistent personal assistant, local state and gateway boundaries |
| [langchain-ai/langgraph @ `e5e37b5d70db`](https://github.com/langchain-ai/langgraph/blob/e5e37b5d70dbfd93c7006bd90a658c6179a95946/README.md) | Stateful orchestration and documented durable execution |
| [microsoft/autogen @ `d07c667a3239`](https://github.com/microsoft/autogen/blob/d07c667a3239ea4ab7af62a53b0f4ff205a1f23d/README.md) | README declares retirement, GitHub archive flag false at inspection |
| [microsoft/agent-framework @ `beba9219ff2b`](https://github.com/microsoft/agent-framework/blob/beba9219ff2b613170fd57add02094244f4286ce/README.md) | Python/.NET agent and workflow framework |
| [crewAIInc/crewAI @ `0b8b216db35e`](https://github.com/crewAIInc/crewAI/blob/0b8b216db35eec36610b5e08c31eb31a29f5c9c1/README.md) | Python role/task orchestration comparison |
| [pydantic/pydantic-ai @ `7fe7068018bd`](https://github.com/pydantic/pydantic-ai/blob/7fe7068018bd577c63527cb10687ec51a6d1b54c/README.md) | Typed extensible Python agent loop |
| [mastra-ai/mastra @ `79d21c536efc`](https://github.com/mastra-ai/mastra/blob/79d21c536efc3f0eb5953fcd215216190cd56514/README.md) | TypeScript agents and workflows, mixed licensing by path |
| [huggingface/smolagents @ `96f33faaf028`](https://github.com/huggingface/smolagents/blob/96f33faaf028479119ec8d34507b47694cf14e34/README.md) | Small code-agent baseline and external sandbox options |
| [letta-ai/letta @ `8816d3a85168`](https://github.com/letta-ai/letta/blob/8816d3a8516893bfdc0fa14bfb1235226012e295/README.md) | Stateful memory reference, redirects current source to letta-code |

## Source corrections and unresolved licensing

Goose resolves from `block/goose` to `aaif-goose/goose`. ACP resolves from the
old Zed location to `agentclientprotocol/agent-client-protocol`. The pins above
use the canonical locations returned by GitHub.

AutoGen's README describes the project as retired and directs users to Microsoft
Agent Framework. The API's archive flag was false. Record both observations
when evaluating maintenance rather than treating an API flag as support policy.
[AutoGen README](https://github.com/microsoft/autogen/blob/d07c667a3239ea4ab7af62a53b0f4ff205a1f23d/README.md).

MCP's root manifest declares Apache-2.0. Its LICENSE describes a transition with
legacy MIT contributions and separate documentation licensing. Mastra's license
separates enterprise paths from the Apache-2.0 portion. These require a release
inventory for the exact consumed files and transitive dependencies.
[MCP manifest](https://github.com/modelcontextprotocol/rust-sdk/blob/f2b0ddf113d1ed426b77a81511eac6e4bffa0ac4/Cargo.toml),
[MCP license](https://github.com/modelcontextprotocol/rust-sdk/blob/f2b0ddf113d1ed426b77a81511eac6e4bffa0ac4/LICENSE),
[Mastra license](https://github.com/mastra-ai/mastra/blob/79d21c536efc3f0eb5953fcd215216190cd56514/LICENSE.md).

OpenFang's README badge and repository license metadata disagree, and retrieval
of a root `LICENSE` returned 404. Its code is an architectural comparison input
with reuse licensing unresolved. Claude Code's public repository metadata also
provided no license identifier. No source is selected for copying on that basis.
[OpenFang README](https://github.com/RightNow-AI/openfang/blob/acf2587e46be174c10200489c9a2d23a39a98aeb/README.md),
[Claude Code README](https://github.com/anthropics/claude-code/blob/ab49b8966bd80199f493a31dad9396f8743e73a7/README.md).

SQLite's original engine is C. The proposed source constraint covers Metron-owned
runtime code in Rust and compatible FOSS dependencies. A literal Rust-only
transitive implementation would require revisiting SQLite. GraphLite documents
Sled-backed storage, so it would introduce a separate projection engine.
[SQLite](https://www.sqlite.org/about.html),
[GraphLite](https://docs.rs/graphlite/latest/graphlite/).

## Selection and falsification

The selected working proposal is a minimal scripted vertical slice with separate
protocol, cognition, harness, persistence and host boundaries. This is a design
judgment based on the requested Rust ownership model, existing lab constraints
and the need to test purity, authority and recovery independently. Upstream
feature counts provide no ranking of this design.

The competing baseline is a small static harness with the same scripted tools,
model backend and budgets. Reject an added cognition, memory, graph or adaptation
layer when it fails its named comparison after total setup and execution costs.

## Relations

- derived-from [original landscape](https://github.com/kmosoti/Metron/blob/4c3781daa7211efd39bcdb95dcb15491be1abe39/docs/proposals/metron-runtime/01-landscape.md)
- informs [runtime charter](../decisions/D-0002-runtime-charter.md)
- sequenced-by [implementation plan](../decisions/D-0003-runtime-implementation-sequence.md)
- tested-through [evaluation program](R-0007-runtime-evaluation.md)

## Revisit when

An upstream source changes, an integration is selected, or a matched Metron
experiment contradicts the proposed boundary or complexity tradeoff. Pin new
source and dependency versions at implementation time.
