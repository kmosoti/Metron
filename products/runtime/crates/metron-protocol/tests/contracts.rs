use metron_protocol::*;

fn receipt(decimal: &str) -> Receipt {
    Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V1,
        producer_receipt_id: ProducerReceiptId::new("producer").unwrap(),
        task_id: TaskId::new("task").unwrap(),
        intent_id: Digest::new("a".repeat(64)).unwrap(),
        dispatch_id: Digest::new("b".repeat(64)).unwrap(),
        action_digest: Digest::new("c".repeat(64)).unwrap(),
        usage: ActualUsage::Known {
            usage: UsageVector {
                model_requests: DecimalUsage::new(decimal).unwrap(),
                tool_calls: DecimalUsage::new("0").unwrap(),
                spend_micros_usd: DecimalUsage::new("0").unwrap(),
            },
        },
        output: ReceiptOutput::Failure {
            diagnostic: BoundedText::new("failure").unwrap(),
        },
    })
    .unwrap()
}

#[test]
fn external_primitives_and_closed_json_reject_invalid_spellings() {
    for bad in ["", "a/b", "é", &"a".repeat(129)] {
        assert!(TaskId::new(bad).is_err());
    }
    for bad in ["01", "-1", "1.0", "", "+1"] {
        assert!(DecimalUsage::new(bad).is_err());
    }
    assert!(
        DecimalUsage::new("18446744073709551616")
            .unwrap()
            .as_u64()
            .is_none()
    );
    assert!(Digest::new("A".repeat(64)).is_err());
    assert!(
        SqlU64::new(i64::MAX as u64)
            .unwrap()
            .checked_add(1)
            .is_err()
    );
    let valid = serde_json::to_string(&receipt("1")).unwrap();
    let duplicate = valid.replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    let unknown = valid.replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"extra\":true",
        1,
    );
    let version = valid.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
    let variant = valid.replacen("\"kind\":\"known\"", "\"kind\":\"bogus\"", 1);
    for input in [duplicate, unknown, version, variant] {
        assert!(parse_envelope::<Receipt>(input.as_bytes()).is_err());
    }
}

#[test]
fn fixture_artifact_and_structural_hash_vectors() {
    assert_eq!(
        content_digest(b"{\"values\":[17,-4,9]}\n").as_str(),
        "65bc2af2478adeca86e655e783fe44285fb64fb9c1fef24c450a380f35e21999"
    );
    let result = CandidateResult {
        count: 3,
        sum: 22,
        input_digest: Digest::new(
            "65bc2af2478adeca86e655e783fe44285fb64fb9c1fef24c450a380f35e21999",
        )
        .unwrap(),
    };
    let bytes = result.artifact_bytes().unwrap();
    assert_eq!(bytes.len(), 103);
    assert_eq!(
        content_digest(&bytes).as_str(),
        "0263b7cb6102022b514b982db0ef741055954c84d0022f7e7bd998a1d1eecf19"
    );
    assert_eq!(
        structural_digest("x", &[b"ab", b"c"]).unwrap().as_str(),
        "c8d00f9389033b292934eda705675f94ddc902fe5a84c6521a3cf72cd0a9ea59"
    );
    assert_ne!(
        structural_digest("x", &[b"ab", b"c"]).unwrap(),
        structural_digest("x", &[b"a", b"bc"]).unwrap()
    );
    assert_ne!(
        structural_digest("x", &[b"abc"]).unwrap(),
        structural_digest("y", &[b"abc"]).unwrap()
    );
}

#[test]
fn largest_valid_receipt_round_trips_through_structural_wrappers() {
    let overhead = canonical_bytes(&receipt("1"), MAX_ENVELOPE).unwrap().len() - 1;
    let digits = "9".repeat(MAX_ENVELOPE - overhead);
    let value = receipt(&digits);
    assert_eq!(
        canonical_bytes(&value, MAX_ENVELOPE).unwrap().len(),
        MAX_ENVELOPE
    );
    let data = CommandData {
        schema_version: SchemaVersion::V1,
        command_id: CommandId::new("cmd").unwrap(),
        operation: StoreOperation::RecordReceipt {
            receipt: value.clone(),
            lease_epoch: SqlU64::new(0).unwrap(),
        },
    };
    let command = StoreCommand::new(data).unwrap();
    let command_bytes = canonical_bytes(&command, MAX_INTERNAL).unwrap();
    assert_eq!(
        parse_internal::<StoreCommand>(&command_bytes).unwrap(),
        command
    );
    let hash = Digest::new("0".repeat(64)).unwrap();
    let record = JournalRecord::new(JournalRecordData {
        seq: SqlU64::new(0).unwrap(),
        previous: hash.clone(),
        event: JournalEvent::CommandCommitted {
            command,
            result: StoreResult::ReceiptRecorded {
                evidence_id: value.evidence_id().unwrap(),
                evidence_revision: SqlU64::new(1).unwrap(),
                settled: false,
                conflict: false,
            },
        },
        digest: hash,
    })
    .unwrap();
    let encoded = canonical_bytes(&record, MAX_INTERNAL).unwrap();
    assert_eq!(parse_internal::<JournalRecord>(&encoded).unwrap(), record);
    assert_eq!(
        parse_internal::<JournalRecord>(&vec![b' '; MAX_INTERNAL + 1]),
        Err(ValidationError::Limit)
    );
    let mut too_large = value.data().clone();
    if let ActualUsage::Known { usage } = &mut too_large.usage {
        usage.model_requests = DecimalUsage::new(format!("{digits}9")).unwrap();
    }
    assert!(Receipt::new(too_large).is_err());
}

#[test]
fn materialization_is_deterministic_and_rejects_changed_binding() {
    let intent = Intent {
        schema_version: SchemaVersion::V1,
        task_id: TaskId::new("task").unwrap(),
        ordinal: 2,
        kind: IntentKind::PublishArtifact {
            candidate: CandidateResult {
                count: 3,
                sum: 22,
                input_digest: content_digest(b"input"),
            },
        },
    };
    let principal = PrincipalId::new("principal").unwrap();
    let genesis = content_digest(b"genesis");
    let mut prepared = PreparedIntent::materialize(&intent, &principal, &genesis).unwrap();
    assert!(
        prepared
            .validate_against(&intent, &principal, &genesis)
            .is_ok()
    );
    prepared.action_digest = content_digest(b"tampered");
    assert_eq!(
        prepared.validate_against(&intent, &principal, &genesis),
        Err(ValidationError::Binding)
    );
}

#[test]
fn materialization_rejects_tampered_plan_tariff_and_model_context() {
    let principal = PrincipalId::new("principal").unwrap();
    let genesis = content_digest(b"genesis");
    let mut intent = Intent {
        schema_version: SchemaVersion::V1,
        task_id: TaskId::new("task").unwrap(),
        ordinal: 1,
        kind: IntentKind::PublishArtifact {
            candidate: CandidateResult {
                count: 3,
                sum: 22,
                input_digest: content_digest(b"input"),
            },
        },
    };
    let original = PreparedIntent::materialize(&intent, &principal, &genesis).unwrap();
    let mut changed = original.clone();
    changed.intent_id = content_digest(b"other");
    assert!(
        changed
            .validate_against(&intent, &principal, &genesis)
            .is_err()
    );
    let mut changed = original.clone();
    if let ApprovedRequest::PublishArtifact { plan } = &mut changed.request {
        plan.relative_path = BoundedText::new("artifacts/other/result.json").unwrap();
    }
    assert!(
        changed
            .validate_against(&intent, &principal, &genesis)
            .is_err()
    );
    let mut changed = original;
    if let ApprovedRequest::PublishArtifact { plan } = &mut changed.request {
        plan.tariff.tool_calls = SqlU64::new(2).unwrap();
    }
    assert!(
        changed
            .validate_against(&intent, &principal, &genesis)
            .is_err()
    );
    intent.kind = IntentKind::RequestModel {
        input_digest: content_digest(b"fixture"),
        context: BoundedText::new("context").unwrap(),
    };
    let mut changed = PreparedIntent::materialize(&intent, &principal, &genesis).unwrap();
    if let ApprovedRequest::RequestModel { context, .. } = &mut changed.request {
        *context = BoundedText::new("different").unwrap();
    }
    assert!(
        changed
            .validate_against(&intent, &principal, &genesis)
            .is_err()
    );
}

#[test]
fn transition_validation_preserves_decision_cap_and_revision() {
    let state = AgentState {
        task_id: TaskId::new("task").unwrap(),
        genesis_digest: content_digest(b"genesis"),
        max_decision_steps: 0,
        revision: SqlU64::new(0).unwrap(),
        decision_steps: 0,
        phase: Phase::New,
        pending_intent: None,
    };
    let mut next = state.clone();
    next.revision = SqlU64::new(1).unwrap();
    assert!(
        Transition::Finalization { next: next.clone() }
            .validate_from(&state)
            .is_ok()
    );
    next.decision_steps = 1;
    let intent = Intent {
        schema_version: SchemaVersion::V1,
        task_id: state.task_id.clone(),
        ordinal: 0,
        kind: IntentKind::ReadFixture,
    };
    assert!(
        Transition::Decision { next, intent }
            .validate_from(&state)
            .is_err()
    );
}

#[test]
fn independently_spelled_nested_json_is_closed_and_task_defaults_to_eight() {
    let task = br#"{"schema_version":1,"task_id":"task","principal_id":"p","objective":"sum","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null}}"#;
    let parsed: Task = parse_envelope(task).unwrap();
    assert_eq!(parsed.data().max_decision_steps, 8);
    let duplicate = br#"{"schema_version":1,"task_id":"task","principal_id":"p","objective":"sum","budget":{"max_model_requests":1,"max_model_requests":2,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null}}"#;
    let unknown = br#"{"schema_version":1,"task_id":"task","principal_id":"p","objective":"sum","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":null,"extra":1}}"#;
    let elapsed = br#"{"schema_version":1,"task_id":"task","principal_id":"p","objective":"sum","budget":{"max_model_requests":1,"max_tool_calls":2,"max_spend_micros_usd":0,"max_elapsed_ms":0}}"#;
    for invalid in [duplicate.as_slice(), unknown.as_slice(), elapsed.as_slice()] {
        assert!(parse_envelope::<Task>(invalid).is_err());
    }
    let outcome = br#"{"status":"not_committed","value":"no_eligible","extra":true}"#;
    assert!(serde_json::from_slice::<WriteOutcome<StoreResult>>(outcome).is_err());
}
