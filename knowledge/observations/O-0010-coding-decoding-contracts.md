---
id: O-0010
type: observation
status: active
updated: 2026-11-11
added_by: agent
human_ratified: false
---

# Coding argument constraints and the decoding boundary

Native CUDA trials with the corrected LFM3 tokenizer reached the SDK tool-call
path, but did not write an application. The v3.5 controller's feedback bounded
known errors without producing useful progress in those trials:

| Trial | Observed result |
| --- | --- |
| 07, full toolkit | A valid plan followed by `list(directory="/")`; two model requests, no tool effects; v3.4 rejected the path. |
| 08, full toolkit | A valid plan followed by three absolute-directory proposals despite validation feedback; four charged model requests, no tool effects; v3.5 stopped at its bound. |
| 09, calculator-scoped catalog | Native plan arguments included object-valued and stringified steps; three charged model requests, no tool effects. |

All retained journals audit, account and behavior-replay. These are negative
application results, not calculator or repair acceptance. The bounded controller
and strict host action parser prevented the malformed proposals from dispatching.

## Source-supported cause

At the pinned SDK revision, the [Qwen XML grammar](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/tools/parsers/qwen.rs)
restricts function and parameter tag names, but uses unrestricted XML text for
parameter values. Its XML branch does not enforce array item types, path
patterns or required parameters when `strict=true`. The later parser rejects
duplicate parameter keys, but its top-level array coercion does not validate
array elements. Object-valued plan steps can therefore reach the application's
action validator. A nullable directory accepts literal `null`; `/` remains a
string rather than being repaired.

The worker's `Function.parameters` matches the SDK API's schema shape. The
observations do not establish a failure to activate the XML grammar. The
[sampling path](https://github.com/EricLBuehler/mistral.rs/blob/fb4cdc3730dc64b366724e7637a205a8b7747499/mistralrs-core/src/pipeline/sampling.rs)
can also fall back to unconstrained sampling after a grammar compilation error;
setting a constraint alone is not proof that every generation used it.

## Discriminating alternative

A CPU test using llguidance 1.9.1 and the corrected LFM3 tokenizer accepts a
string-valued plan and null root directory under the existing JSON action
schema, and rejects object-valued/stringified steps and absolute paths. A second
test applies the exact phase-filtered helper: eleven cases cover plan-only and
four-action catalogs, allowed null values, rejected paths and excluded actions.
Schema hashes are pinned by protocol tests to the exercised grammar bytes.

The selected implementation adds a new recipe identity for phase-filtered JSON
actions. It uses the same model, GGUF quantization, CUDA backend, corrected
tokenizer and admitted catalog. No SDK function metadata is supplied in this
mode; its JSON schema is selected only from the typed host-owned catalog. The
v3.6 controller uses format-neutral instructions, leaving the recipe to specify
JSON. Earlier native/XML and JSON recipes remain available for their journals.

For a single plan branch, the sorted schema makes this grammar generate `steps`
before `tool`. The action parser accepts ordinary JSON object order; constrained
generation chooses a valid subset. Native throughput and task quality require
measurement rather than an assumption about that ordering.

The CPU controls demonstrate admissible grammar bytes, not useful model
generation or full application completion. Independent host parsing, policy,
approval, fresh admission, usage accounting and uncertain-outcome holds remain
necessary. Native calculator/repair and memory/resource gates stay open.

**Relations:** refines O-0008/O-0009 and #26; supports the next discriminating
native experiment in #28. No production profile or alpha.1 completion follows.
