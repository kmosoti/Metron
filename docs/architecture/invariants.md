# The four invariants

Each invariant is stated, then the mechanism that encodes it, then the test
that fails if it is broken. `cargo test --workspace` runs all of them; CI
runs `cargo test --workspace` on every push.

## 1. `metron-core` is pure

**Statement.** The core cannot depend on adapters, language models,
databases, files, or experiment internals. Dependencies point inward.

**Encoding.**
- `crates/metron-core/Cargo.toml` lists `serde`, `serde_json`, `sha2` and
  `thiserror` and nothing else.
- The core's ports (`Oracle`, `Clock`) are traits; the core ships a
  `ManualClock` only. `SystemClock` is in `metron-adapters` because reading
  time is an effect.
- `metron-app`, `metron-lab`, `metron-operators` and `metron-adapters`
  depend on `metron-core` alone. Only `metron-cli` sees them all.

**Tests.** `tests/architecture/tests/invariant_1_core_is_pure.rs`:
- the core's dependency allow-list;
- no `std::fs`, `std::net`, `std::process`, `std::env`, `std::io`,
  `std::thread`, `std::time`, `extern crate` or `include_*!` in core code;
- no inner crate depends on another inner crate;
- no file, network, process or environment access in app, lab or operators.

## 2. The laboratory owns ground truth

**Statement.** The cognitive system cannot inspect hidden targets,
evaluation code or promotion criteria.

**Encoding.**
- `HiddenFunction` has private fields, no accessor for the table, no
  `Serialize`, and a redacted `Debug`.
- The only path from the system to the target is the `Oracle` port; every
  probe through it is receipted and capped by the lab's `Protocol`.
- `LabWorld::judge` is the only judge and runs after the episode. Operators
  are generic over `W: Oracle`; `judge` is not on that trait, and no crate
  the operators can depend on exposes it.
- `PromotionCriteria` has private fields and a redacted `Debug`; the system
  sees a `PromotionVerdict` with reasons, never thresholds.
- Fixtures are read only by the composition root and handed to the lab as
  text; the lab itself does no I/O.

**Tests.** `tests/architecture/tests/invariant_2_lab_owns_ground_truth.rs`:
- field privacy and non-serialisability of `HiddenFunction` (source check)
  and redaction (runtime check);
- field privacy and redaction of `PromotionCriteria`;
- no crate other than the CLI depends on `metron-lab`;
- `fn judge`, `fixture`, `experiments/` and `HiddenFunction` do not appear
  in core, app or operator sources.

## 3. Every external operation produces a receipt

**Statement.** Every externally executed operation yields a resource and
evidence receipt, so replay and cost accounting are possible.

**Encoding.**
- `Oracle::probe` returns an `Observation` that names its `ReceiptId`; the
  oracle seals a `ResourceReceipt` (request hash, response hash, cost,
  observations) at the moment of execution.
- The runner drains `Oracle::drain_receipts` after **every** application,
  before it reads the operator's result, and journals each receipt. An
  operator cannot suppress a receipt by failing, by ignoring the
  observation, or by being the wrong kind of operator.
- Receipt cost is charged to the inquiry by the runner, not reported by the
  operator.
- The journal is a hash chain; receipts are verified as part of the chain.
  Wall-clock readings are kept but excluded from the digest, so a replay
  with the same manifest and seed reproduces the head hash.

**Tests.** `tests/architecture/tests/invariant_3_receipts.rs` and
`replay_and_manifests.rs`:
- probes answered == receipts journaled == observations recorded;
- a transform that probes still leaves its receipt and is stopped as a
  contract violation;
- an observe operator that probes and then errors still leaves its receipt;
- the protocol cap is enforced by the oracle;
- two identical runs produce identical journals.

## 4. Representations are connected by explicit transform contracts

**Statement.** Views in different frames are connected only through
contracts that say what is preserved, lost and assumed, and whether the
transform is exact or approximate.

**Encoding.**
- `OperatorKind::Transform { contract }` is the only way to write a frame
  other than `observations`; `TransformContract::validate` runs at
  registration.
- At runtime the runner checks every written view: declared in `writes`,
  in the contract's `to` frame, derived only from views in the contract's
  `from` frames. Observe operators may write only the `observations` frame;
  commit operators write nothing; only observe operators obtain
  observations.
- The runner stamps the contract onto each written view's `Derivation` and
  journals it with the view's transitive observation set.
- Approximate contracts must describe what is approximate; the reference
  `complete-table-by-default` operator is one, and says so.

**Tests.** `tests/architecture/tests/invariant_4_transform_contracts.rs`:
- all reference contracts validate and connect `observations` to the
  answer frame;
- the registry rejects contracts with no source frame and transforms that
  write nothing;
- the runner stops on a view written in the wrong frame, a view derived
  from a frame the contract does not read, an undeclared write, and a
  transform that commits;
- journaled view writes carry the expected contract and the full
  observation provenance, and the committed answer's evidence names every
  observation and the full operator path.
