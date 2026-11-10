---
title: Agent and harness landscape audit
status: PROPOSED evidence index
as_of: 2026-11-10
confidence: mixed — primary-source inspected entries marked P
---

# 01 — Research landscape and design extraction

## Method and evidence rules

This is a **bounded source-index and architectural comparison**, not a line-by-line audit of every repo. Classification:

- **P** — official source, docs or primary paper inspected for the cited feature.
- **I** — project identified through an official repository or published docs; detailed feature assertions remain to verify in source.
- **H** — historical, archived or research-only; not a current production dependency recommendation.
- **U** — unclear license, maturity or exact integration; treat as unresolved.

Do not infer real-world task success from GitHub stars, advertised security layers, release cadence, or published benchmark maxima. Source statements and our design deductions are separated below. Repo URLs are for follow-up pinning to exact commits before implementation.

## Production coding agents and harnesses

| System | Class | Evidence | Relevant mechanism or boundary | Metron decision |
|---|---|---|---|---|
| [Pi](https://github.com/earendil-works/pi) | minimal coding agent | P | compact loop; extensions for tools, hooks, commands, prompt/context; Pi packages and configurable sessions | Copy the **small core / extension discipline**, not permissive in-process trust |
| [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/architecture.md) | extensible coding harness | P | Cordis services, dependency injection, profile/bundle composition, replaceable agent loop; developer preview | Copy service registry, lifecycle and configuration ideas, **not** universal replaceability of security policy |
| [Codex CLI](https://github.com/openai/codex) | coding harness | P | Rust-oriented CLI/app server; model/tool/sandbox execution boundary | Baseline for coding acceptance, not a product dependency |
| [Claude Code](https://github.com/anthropics/claude-code) | coding harness | I | agent loop, tools, permissions, hooks and subagents | Baseline; confirm license/sandbox behavior before code reuse |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli) | coding harness | I | tools, workspace context, approvals, extensions | Context and approvals comparison |
| [OpenCode](https://github.com/anomalyco/opencode) | coding harness | I | provider-flexible coding session and clients | Interface, provider routing and session comparison |
| [Aider](https://github.com/Aider-AI/aider) | terminal pair programmer | P | Git-centric edits, repository map, lint/test loop | Compare minimum required source-edit and verify tools |
| [OpenHands SDK](https://github.com/OpenHands/software-agent-sdk) | agent execution / workspace | P | SDK/server split, isolated local or ephemeral workspaces, typed event API | Separate conversation orchestration from execution workspace |
| [SWE-agent](https://github.com/SWE-agent/SWE-agent/blob/main/docs/background/aci.md) | research coding agent | P | agent-computer interface: focused editor/search/lint feedback shapes success | Treat tool output ergonomics and computer interface as evaluated designs |
| [mini-SWE-agent](https://github.com/SWE-agent/mini-swe-agent) | minimal research harness | I | small coding-agent implementation | Negative control against feature-heavy designs |
| [Goose](https://github.com/block/goose) | Rust-based coding assistant | I | MCP tooling and interactive agent interface | Rust-native competition; avoid claiming novelty for Rust or MCP |
| [Cline](https://github.com/cline/cline) | IDE agent | I | approval-centric local computer/tool operations | Approval UX reference |
| [Roo Code](https://github.com/RooCodeInc/Roo-Code) | IDE multi-mode agent | H | role modes and MCP; **upstream repository archived May 15 2026** | Historical mode taxonomy, not active dependency |
| [Mistral Vibe](https://github.com/mistralai/mistral-vibe) | terminal coding agent | I | coding loop and tool UX | Comparative baseline |

## Persistent assistants, Rust platforms, orchestration and memory

| System | Class | Evidence | Relevant mechanism or boundary | Metron decision |
|---|---|---|---|---|
| [Hermes Agent](https://github.com/NousResearch/hermes-agent) | persistent assistant | P | gateway/session, cron, skills, memory, channels, delegation | Agent product UX, persistence and scheduled-work comparator |
| [OpenClaw](https://github.com/openclaw/openclaw) | personal agent | I | multi-channel persistent assistants | Threat model for long-lived permissions and untrusted messages |
| [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/book/src/architecture/overview.md) | Rust assistant runtime | P | layered Rust agent runtime, providers, channels, memory, security | Rust-native competitor; preserve narrower Metron research thesis |
| [OpenFang](https://github.com/RightNow-AI/openfang) | Rust autonomous agent platform | P | agent OS, scheduler, tools, Wasm, memory, security; project states pre-1.0 | Do not duplicate huge agent OS in v0.1 |
| [Rig](https://github.com/0xPlaygrounds/rig) | Rust agent library | I | model calls, typed tools/agents | Candidate adapter/library; use strict version pinning |
| [Swiftide](https://github.com/bosun-ai/swiftide) | Rust agent/RAG framework | I | composable pipelines and agents | Evaluate as an alternative to custom orchestration, not foundation by default |
| [LangGraph](https://github.com/langchain-ai/langgraph) | stateful orchestration | P | durable graph execution, checkpoints, HITL | Distinguish workflow durability from agent reasoning |
| [AutoGen](https://github.com/microsoft/autogen) | research multiagent framework | H | role-based agent conversations; **archived/no longer maintained** | Historical design, not production dependency |
| [Microsoft Agent Framework](https://github.com/microsoft/agent-framework) | successor orchestration framework | I | official AutoGen successor; production-oriented agents | Review if enterprise integration becomes necessary |
| [CrewAI](https://github.com/crewAIInc/crewAI) | multi-agent orchestration | I | roles/tasks/crews | Explicit role != autonomous agent instance |
| [Pydantic AI](https://github.com/pydantic/pydantic-ai) | typed agent SDK | P | typed model/tool contracts, evals, graph and durable runs | Strong structured-output / eval API comparator |
| [Mastra](https://github.com/mastra-ai/mastra) | TypeScript agent framework | P | agents, workflows, model routing, evals; **mixed Apache-2.0/enterprise parts** | Declared license-by-path matters to FOSS review |
| [smolagents](https://github.com/huggingface/smolagents) | experimental lightweight agents | I | minimal planning/code-agent baseline | Negative control for complexity |
| [Letta](https://github.com/letta-ai/letta) | memory-centered agent | P | persistent memory/identity; current active harness source linked to letta-code | Compare explicit memory updates vs implicit session summary |

## Research and experiments with architectural consequences

| Research | Evidence | Supported observation | Does **not** prove |
|---|---|---|---|
| [Harness Engineering: Source-Code Study of Eleven Systems](https://arxiv.org/abs/2609.00006) | P, July 2026 paper | taxonomy of 7 harness subsystems, 29 patterns, ~4M lines inspected; minimal loops recur | one framework universally wins; no future agents can use vector search |
| [Google Harness²](https://arxiv.org/abs/2610.14224), [code](https://github.com/google-research/harness2) | P, Oct 2026 paper and Apache-2.0 repo | Propose–Probe–Compose task refinement plus cross-task improvement library; authors report benchmark gains | safe self-modification, monotonically improving performance, or transfer to Metron |
| [SWE-agent ACI work](https://github.com/SWE-agent/SWE-agent/blob/main/docs/background/aci.md) | P, official docs and paper link | specialized editor/search/lint feedback influences coding outcomes | every tool should be custom built |
| [Metron's own lab](../../architecture/milestones.md), [ADR 0012](../../adr/0012-capability-promotion-judged-on-held-out-targets.md) | P, in-repo reproducible reports | independent held-out promotion can reject tempting candidates | real-world adaptive harness gains outside lab |
| [Metron routing ADR 0018](../../adr/0018-m3-reached-routing-among-structural-learners.md) | P, in-repo report | a hand-authored Bayes router outperformed baseline under measured conditions; learned router did not win | generalized superiority of learned routing |

Additional evaluation candidates (not in the verified core comparison): [AgentBench](https://arxiv.org/abs/2308.03688), [SWE-bench](https://www.swebench.com/), [Terminal-Bench](https://www.tbench.ai/), [OSWorld](https://os-world.github.io/), [GAIA](https://huggingface.co/gaia-benchmark), [BrowseComp](https://openai.com/index/browsecomp/). Determine current licenses, task provenance, contamination risks, hardware requirements and evaluation rules before adopting any as an alpha gate.

## Synthesis: design patterns that survive intersection

**Intersected across independent architectures**

1. Model/tool loops can be small; the operational system around them cannot be dismissed.
2. Tools and computer-use surfaces need explicit schemas, affordances, response size bounds and honest failure states.
3. Persistence, replay, authorization, approval and verifiability are different responsibilities.
4. Identity, role and persona are distinct from granted capabilities.
5. Plugin composition is useful, but **plugin replaceability is not a substitute for an external trust boundary**.
6. Context retrieval should start deterministic and inspectable; memory architecture should compete on measured tasks.
7. Improvement requires independent, budget-matched evaluation, not the agent reviewing its own answer.

**Where serious designs differ**

- Pi prioritizes extension minimalism; DSH prioritizes service replaceability; OpenFang/ZeroClaw include more batteries; OpenHands emphasizes workspaces; LangGraph emphasizes durable graphs; Harness² emphasizes configuration search.
- None establishes that Metron should copy all features. A small Sans-IO kernel plus externally enforced effects is a deliberate specialization, not a generic “agent OS”.

## Rejections and open questions

- **Reject:** building a complete multi-agent scheduler, browser desktop, graph database and self-modifier at alpha 0.
- **Reject:** every component as a hot-swappable plugin. Authority and invariant enforcement are nondelegable.
- **Defer:** external model API client inside existing research lab (conflicts ADR 0009).
- **Defer:** adopting GraphLite as canonical memory merely because it's Rust-native; [GraphLite docs](https://docs.rs/graphlite/latest/graphlite/) describe a standalone Sled-backed GQL database, hence two storage engines if SQLite remains canonical.
- **Unverified:** which live task set yields measurable gains from persistent adaptation over task-local adaptation at matched compute.
- **Unverified:** whether product/runtime can share code with the lab without violating existing crate dependency and sealed-evaluation tests.

## Primary-source index for specifications and implementation checks

- Pi: [repo](https://github.com/earendil-works/pi), [packages](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md).
- DSH: [architecture](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/architecture.md), [Cordis primer](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/cordis-primer.md), [preview notice](https://github.com/deepseek-ai/deepseek-harness/blob/master/README.md).
- Hermes: [repo/releases](https://github.com/NousResearch/hermes-agent).
- Harness²: [paper](https://arxiv.org/abs/2610.14224), [repo](https://github.com/google-research/harness2).
- Rust native competitors: [ZeroClaw architecture](https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/book/src/architecture/overview.md), [OpenFang](https://github.com/RightNow-AI/openfang), [Rig](https://github.com/0xPlaygrounds/rig), [Swiftide](https://github.com/bosun-ai/swiftide).
- Interop: [Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk), [ACP](https://github.com/zed-industries/agent-client-protocol), [Wasmtime component model](https://docs.wasmtime.dev/), [Rust Forge channel schedule](https://forge.rust-lang.org/).
- Storage: [SQLite lifetime documentation](https://www.sqlite.org/c3ref/column_blob.html), [GraphLite](https://docs.rs/graphlite/latest/graphlite/).

**Source verification rule:** Every code-facing dependency, license assertion or benchmark claim requires a pinned commit/tag and an evidence entry at implementation time. A current README is a lead, not a proof of runtime behavior.
