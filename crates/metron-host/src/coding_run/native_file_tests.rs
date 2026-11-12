use super::tests::{assert_audits, driver, setup};
use super::*;
use metron_protocol::store::{RuntimeSnapshot, StorePort, WriteOutcome};

fn profile() -> LocalProfile {
    let artifact =
        Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4").unwrap();
    LocalProfile {
        schema_version: SchemaVersion::V2,
        auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
        auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871").unwrap(),
        artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
        artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2").unwrap(),
        artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
        embedded_tokenizer_digest: gguf_lfm3_corrected_tokenizer_digest(&artifact).unwrap(),
        artifact_digest: artifact,
        artifact_bytes: SqlU64::new(1099627808).unwrap(),
        template_digest: content_digest(b"template"),
        prompt_recipe_digest: Digest::new(LOCAL_CODING_V10_RECIPE_SHA).unwrap(),
        backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499").unwrap(),
        backend_build_digest: content_digest(b"build"),
        cpu_identity: BoundedText::new("test-cpu").unwrap(),
        cpu_features_digest: content_digest(b"cpu-features"),
        device: LocalDevice::Cpu,
        cpu_threads: 2,
        context_tokens: 2048,
        max_input_tokens: 1536,
        max_output_tokens: 512,
        temperature_milli: 100,
        top_k: 50,
        repeat_penalty_milli: 1100,
        max_sequences: 1,
        queue_capacity: 8,
        queue_wait_ms: 5000,
        deadline_ms: 60000,
        chunked_prefill: false,
        scoped_prefix_cache: false,
        cache_target_bytes: None,
        strict_memory_cap_bytes: None,
    }
}

struct RecordedCalls {
    inner: Driver,
    profile: LocalProfile,
    calls: VecDeque<(&'static str, &'static str)>,
    held: Option<Receipt>,
    reconcile_first_edit: bool,
    edit_count: usize,
    workspace: PathBuf,
}
impl EffectDriver for RecordedCalls {
    type Outcome = DriverOutcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
        if let ApprovedRequest::Coding { step, .. } = &effect.request().request
            && let CodingStep::Model {
                context,
                input_digest,
                previous_tool_turn,
                ..
            } = step.as_ref()
        {
            let (name, args) = self.calls.pop_front().expect("recorded native call");
            if name == "edit" {
                self.edit_count += 1;
                if self.edit_count == 2 {
                    fs::write(self.workspace.join("note.txt"), "external change\n").unwrap();
                }
            }
            let mut sdk_args: serde_json::Value = serde_json::from_str(args).unwrap();
            for key in ["path", "content", "summary", "program", "directory"] {
                if let Some(value) = sdk_args.get_mut(key)
                    && value.is_string()
                {
                    *value = serde_json::Value::String(serde_json::to_string(value).unwrap());
                }
            }
            let native =
                NativeCallEvidence::capture(1, vec![(name.to_owned(), sdk_args.to_string())], None)
                    .unwrap();
            let content = native
                .action_content_for_context(
                    &self.profile.prompt_recipe_digest,
                    context.as_str(),
                    previous_tool_turn.as_deref(),
                )
                .unwrap();
            let response = parse_local_response(content.as_bytes(), LocalOutputSchema::Text).ok();
            let observation = LocalObservation {
                schema_version: SchemaVersion::V2,
                profile_digest: self.profile.digest().unwrap(),
                request_digest: effect.request().request_digest.clone(),
                input_digest: input_digest.clone(),
                content: BoundedText::new(content).unwrap(),
                reasoning: None,
                native_calls: Some(native),
                prompt_tokens: 1,
                completion_tokens: 1,
                submission_cohort: SubmissionCohort {
                    id: BoundedText::new(effect.request().action.dispatch_id.as_str()).unwrap(),
                    size: 1,
                    position: 0,
                },
                backend_batch: BatchProvenance::Unknown,
                response,
            };
            let result = self.inner.known(
                &effect,
                ReceiptOutput::LocalModel {
                    observation: Box::new(observation),
                },
                1,
                0,
            );
            if name == "edit" && self.edit_count == 1 && self.reconcile_first_edit {
                let DriverOutcome::Receipt(receipt) = result else {
                    panic!("known receipt")
                };
                self.held = Some(*receipt);
                return DriverOutcome::Uncertain;
            }
            return result;
        }
        self.inner.execute(effect)
    }
}

#[test]
fn contextual_file_tools_admit_reconcile_and_replay_both_modes() {
    for mode in [EngineMode::Agent, EngineMode::Harness] {
        let name = if mode == EngineMode::Agent {
            "file-v10-agent"
        } else {
            "file-v10-harness"
        };
        let (root, mut settings, original) = setup(
            name,
            mode,
            vec![CodingTool::Read, CodingTool::Edit, CodingTool::Finish],
            false,
        );
        drop(original);
        let profile = profile();
        profile.validate().unwrap();
        settings.context_version = 11;
        settings.profile = Some(profile.clone());
        settings.script = None;
        let mut data = settings.task.data().clone();
        data.model_profile_digest = profile.digest().unwrap();
        settings.task = CodingTask::new(data).unwrap();
        let mut store = SqliteStore::create(root.join("run/v10.sqlite")).unwrap();
        let result = store
            .apply(
                command(StoreOperation::CreateTask {
                    task: Task::new_coding(settings.task.clone()).unwrap(),
                    genesis: settings.genesis().unwrap(),
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(result, WriteOutcome::Committed { .. }));
        let mut worker = RecordedCalls {
            inner: driver(&root, &settings, &[]),
            profile,
            calls: VecDeque::from([
                ("plan", r#"{"steps":["inspect note"]}"#),
                ("create", r#"{"path":"note.txt","content":"first\n"}"#),
                (
                    "read",
                    r#"{"path":"note.txt","first_line":1,"line_count":100}"#,
                ),
                ("edit", r#"{"content":"second\n"}"#),
                (
                    "create",
                    r#"{"path":"note.txt","content":"overwrite denied\n"}"#,
                ),
                (
                    "read",
                    r#"{"path":"note.txt","first_line":1,"line_count":100}"#,
                ),
                ("edit", r#"{"content":"stale denied\n"}"#),
                ("finish", r#"{"summary":"request verification"}"#),
            ]),
            held: None,
            reconcile_first_edit: true,
            edit_count: 0,
            workspace: root.join("workspace"),
        };
        worker.inner.store_path = root.join("run/v10.sqlite");
        let task = Task::new_coding(settings.task.clone()).unwrap();
        let genesis = settings.genesis().unwrap();
        let engine = crate::cognition(&genesis, &task).unwrap();
        assert_eq!(
            orchestrator::drive_with_driver(
                &mut store,
                &settings.task.data().task_id,
                engine.as_ref(),
                &root.join("run"),
                &mut NoObserver,
                &mut worker
            )
            .unwrap(),
            orchestrator::DriveStatus::Blocked
        );
        let held = worker.held.take().expect("lost first edit receipt");
        let id = held.data().intent_id.clone();
        let evidence_id = held.evidence_id().unwrap();
        let epoch = {
            let mut snap = store.audit().unwrap();
            snap.intent(&id).unwrap().unwrap().lease_epoch
        };
        store
            .apply(
                command(StoreOperation::RecordReceipt {
                    receipt: held,
                    lease_epoch: epoch,
                })
                .unwrap(),
            )
            .unwrap();
        let intent = {
            let mut snap = store.audit().unwrap();
            snap.intent(&id).unwrap().unwrap()
        };
        let worker_id = WorkerId::new("v10-reconciler").unwrap();
        store
            .apply(
                command(StoreOperation::ClaimReconciliation {
                    task_id: settings.task.data().task_id.clone(),
                    intent_id: id.clone(),
                    worker: worker_id.clone(),
                    expected_epoch: intent.reconciliation_epoch,
                    expected_evidence_revision: intent.evidence_revision,
                })
                .unwrap(),
            )
            .unwrap();
        let intent = {
            let mut snap = store.audit().unwrap();
            snap.intent(&id).unwrap().unwrap()
        };
        let lease_epoch = intent.reconciliation_lease.as_ref().unwrap().epoch;
        let result = store
            .apply(
                command(StoreOperation::SettleReconciliation {
                    task_id: settings.task.data().task_id.clone(),
                    intent_id: id,
                    worker: worker_id,
                    lease_epoch,
                    expected_evidence_revision: intent.evidence_revision,
                    evidence_id,
                    reconciled_receipt: None,
                })
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(
            result,
            WriteOutcome::Committed(StoreReply {
                result: StoreResult::ReconciliationSettled { .. },
                ..
            })
        ));
        assert_eq!(
            orchestrator::drive_with_driver(
                &mut store,
                &settings.task.data().task_id,
                engine.as_ref(),
                &root.join("run"),
                &mut NoObserver,
                &mut worker
            )
            .unwrap(),
            orchestrator::DriveStatus::Blocked
        );
        assert_eq!(
            fs::read_to_string(root.join("workspace/note.txt")).unwrap(),
            "external change\n"
        );
        let mut snapshot = store.audit().unwrap();
        let projection = snapshot.projection(&settings.task.data().task_id).unwrap();
        assert_eq!(projection.state.phase, Phase::AwaitingVerification);
        let mut creates = Vec::new();
        let mut edits = Vec::new();
        let mut receipt_checked = false;
        for intent in snapshot
            .intents(&settings.task.data().task_id, None)
            .unwrap()
            .records
        {
            for evidence in snapshot.evidence(&intent.intent_id, None).unwrap().records {
                match (&intent.intent.kind, &evidence.receipt.data().output) {
                    (IntentKind::Coding { step }, ReceiptOutput::Coding { observation }) => {
                        if let (
                            CodingStep::Tool {
                                action:
                                    CodingAction::Edit {
                                        path,
                                        expected_digest,
                                        ..
                                    },
                                ..
                            },
                            CodingObservation::Tool { output, .. },
                        ) = (step.as_ref(), observation.as_ref())
                        {
                            assert_eq!(path.as_str(), "note.txt");
                            if let Some(digest) = expected_digest {
                                edits.push((digest.clone(), output.status.clone()))
                            } else {
                                creates.push(output.status.clone())
                            }
                        }
                    }
                    (IntentKind::Coding { .. }, ReceiptOutput::LocalModel { observation })
                        if observation.native_calls.as_ref().is_some_and(|v| {
                            v.calls.first().is_some_and(|c| c.name.as_str() == "edit")
                        }) =>
                    {
                        let call = &observation.native_calls.as_ref().unwrap().calls[0];
                        let raw: serde_json::Value =
                            serde_json::from_str(call.arguments.as_str()).unwrap();
                        assert_eq!(raw.as_object().unwrap().len(), 1);
                        assert!(raw.get("content").is_some());
                        let prepared = intent.prepared.as_ref().unwrap();
                        metron_protocol::coding_runtime::validate_coding_receipt(
                            &genesis,
                            prepared,
                            &evidence.receipt,
                        )
                        .unwrap();
                        let mut forged = prepared.clone();
                        let ApprovedRequest::Coding { step, .. } = &mut forged.request else {
                            unreachable!()
                        };
                        let CodingStep::Model { profile_digest, .. } = step.as_mut() else {
                            unreachable!()
                        };
                        *profile_digest = content_digest(b"forged profile");
                        assert!(
                            metron_protocol::coding_runtime::validate_coding_receipt(
                                &genesis,
                                &forged,
                                &evidence.receipt
                            )
                            .is_err()
                        );
                        let mut forged = prepared.clone();
                        let ApprovedRequest::Coding { step, .. } = &mut forged.request else {
                            unreachable!()
                        };
                        let CodingStep::Model { context, .. } = step.as_mut() else {
                            unreachable!()
                        };
                        *context = BoundedText::new("{}").unwrap();
                        assert!(
                            metron_protocol::coding_runtime::validate_coding_receipt(
                                &genesis,
                                &forged,
                                &evidence.receipt
                            )
                            .is_err()
                        );
                        let mut forged = prepared.clone();
                        let ApprovedRequest::Coding { step, .. } = &mut forged.request else {
                            unreachable!()
                        };
                        let CodingStep::Model {
                            previous_tool_turn, ..
                        } = step.as_mut()
                        else {
                            unreachable!()
                        };
                        *previous_tool_turn = None;
                        assert!(
                            metron_protocol::coding_runtime::validate_coding_receipt(
                                &genesis,
                                &forged,
                                &evidence.receipt
                            )
                            .is_err()
                        );
                        let mut forged = evidence.receipt.data().clone();
                        let ReceiptOutput::LocalModel { observation } = &mut forged.output else {
                            unreachable!()
                        };
                        observation.content = BoundedText::new("{}").unwrap();
                        assert!(
                            metron_protocol::coding_runtime::validate_coding_receipt(
                                &genesis,
                                prepared,
                                &Receipt::new(forged).unwrap()
                            )
                            .is_err()
                        );
                        receipt_checked = true;
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(creates, vec![ToolStatus::Succeeded, ToolStatus::Denied]);
        assert_eq!(
            edits,
            vec![
                (content_digest(b"first\n"), ToolStatus::Succeeded),
                (content_digest(b"second\n"), ToolStatus::Denied)
            ]
        );
        assert!(receipt_checked);
        drop(snapshot);
        assert_audits(&mut store, &settings);
        drop(worker);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn objective_digest_cannot_authorize_edit_without_read_in_either_mode() {
    for mode in [EngineMode::Agent, EngineMode::Harness] {
        let name = if mode == EngineMode::Agent {
            "v10-noread-agent"
        } else {
            "v10-noread-harness"
        };
        let (root, mut settings, original) = setup(
            name,
            mode,
            vec![CodingTool::Edit, CodingTool::Finish],
            false,
        );
        drop(original);
        let profile = profile();
        settings.context_version = 11;
        settings.profile = Some(profile.clone());
        settings.script = None;
        fs::write(root.join("workspace/note.txt"), "unchanged\n").unwrap();
        let mut data = settings.task.data().clone();
        data.model_profile_digest = profile.digest().unwrap();
        data.objective = BoundedText::new(format!(
            "A supplied digest is {}",
            content_digest(b"unchanged\n").as_str()
        ))
        .unwrap();
        settings.task = CodingTask::new(data).unwrap();
        let mut store = SqliteStore::create(root.join("run/v10-no-read.sqlite")).unwrap();
        store
            .apply(
                command(StoreOperation::CreateTask {
                    task: Task::new_coding(settings.task.clone()).unwrap(),
                    genesis: settings.genesis().unwrap(),
                })
                .unwrap(),
            )
            .unwrap();
        let mut worker = RecordedCalls {
            inner: driver(&root, &settings, &[]),
            profile,
            calls: VecDeque::from([
                ("plan", r#"{"steps":["inspect"]}"#),
                ("edit", r#"{"content":"must not write"}"#),
                ("finish", r#"{"summary":"request verification"}"#),
            ]),
            held: None,
            reconcile_first_edit: false,
            edit_count: 0,
            workspace: root.join("workspace"),
        };
        worker.inner.store_path = root.join("run/v10-no-read.sqlite");
        let task = Task::new_coding(settings.task.clone()).unwrap();
        let engine = crate::cognition(&settings.genesis().unwrap(), &task).unwrap();
        assert_eq!(
            orchestrator::drive_with_driver(
                &mut store,
                &settings.task.data().task_id,
                engine.as_ref(),
                &root.join("run"),
                &mut NoObserver,
                &mut worker
            )
            .unwrap(),
            orchestrator::DriveStatus::Blocked
        );
        assert_eq!(
            fs::read_to_string(root.join("workspace/note.txt")).unwrap(),
            "unchanged\n"
        );
        let mut snap = store.audit().unwrap();
        let projection = snap.projection(&settings.task.data().task_id).unwrap();
        assert_eq!(projection.state.phase, Phase::AwaitingVerification);
        assert_eq!(
            projection.state.coding_context.unwrap().invalid_proposals,
            1
        );
        assert!(!snap.intents(&settings.task.data().task_id,None).unwrap().records.iter().any(|i| matches!(&i.intent.kind, IntentKind::Coding { step } if matches!(step.as_ref(), CodingStep::Tool { .. }))));
        drop(snap);
        assert_audits(&mut store, &settings);
        drop(worker);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
