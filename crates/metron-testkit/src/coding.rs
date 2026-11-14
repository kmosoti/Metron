//! Controlled coding inputs and supplied model effects for contract tests.
use metron_harness::{
    driver::DriverOutcome,
    policy::{AuthorizedEffect, EffectDriver},
};
use metron_protocol::{coding::*, coding_runtime::*, *};

/// Bounded coding task/policy with no native model or tool installation.
pub fn fixture(name: &str, mode: EngineMode) -> (CodingTask, CodingPolicy, Genesis) {
    let task = CodingTask::new(CodingTaskData {
        schema_version: 3,
        family: CodingFamily::CodingV1,
        task_id: TaskId::new(name).unwrap(),
        principal_id: PrincipalId::new("principal").unwrap(),
        project_id: BoundedText::new("contract-tests").unwrap(),
        objective: BoundedText::new("Exercise coding admission and recorded observations").unwrap(),
        acceptance_digest: content_digest(b"independent-test-verifier"),
        tool_catalog_digest: content_digest(b"test-catalog"),
        model_profile_digest: content_digest(b"coding-scripted/v1"),
        memory_collection: None,
        budget: BudgetRequest {
            max_model_requests: SqlU64::new(8).unwrap(),
            max_tool_calls: SqlU64::new(8).unwrap(),
            max_spend_micros_usd: SqlU64::new(0).unwrap(),
            max_elapsed_ms: None,
        },
        max_decision_steps: 16,
    })
    .unwrap();
    let policy = CodingPolicy {
        schema_version: 1,
        principal_id: task.data().principal_id.clone(),
        project_id: task.data().project_id.clone(),
        workspace_root: BoundedText::new("/controlled-test-workspace").unwrap(),
        memory_collection: None,
        allowed_tools: vec![CodingTool::Edit, CodingTool::Finish],
        allowed_programs: vec![],
        require_mutation_approval: false,
        max_file_bytes: 65536,
        max_output_bytes: 4096,
        command_timeout_ms: 1000,
        command_memory_bytes: SqlU64::new(268435456).unwrap(),
        max_processes: 16,
    };
    policy.validate(&task).unwrap();
    let genesis = Genesis {
        mode,
        engine_version: BoundedText::new(match mode {
            EngineMode::Agent => "coding-agent/v3.10",
            EngineMode::Harness => "coding-harness/v3.10",
        })
        .unwrap(),
        require_publication_approval: false,
        policy_digest: policy.digest(&task).unwrap(),
        local_profile: None,
        coding_policy: Some(Box::new(policy.clone())),
    };
    (task, policy, genesis)
}

/// Deterministic model observations only; exhausted input yields uncertainty.
#[derive(Default)]
pub struct ModelReplies {
    replies: std::collections::VecDeque<String>,
    pub calls: usize,
}
impl ModelReplies {
    pub fn new(replies: &[&str]) -> Self {
        Self {
            replies: replies.iter().map(|s| (*s).into()).collect(),
            calls: 0,
        }
    }
}
impl EffectDriver for ModelReplies {
    type Outcome = DriverOutcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> DriverOutcome {
        self.calls += 1;
        let ApprovedRequest::Coding { step, .. } = &effect.request().request else {
            panic!("expected coding effect")
        };
        let CodingStep::Model { input_digest, .. } = step.as_ref() else {
            panic!("test driver must not execute tools")
        };
        let Some(content) = self.replies.pop_front() else {
            return DriverOutcome::Uncertain;
        };
        let output = ReceiptOutput::Coding {
            observation: Box::new(CodingObservation::ScriptedModel {
                input_digest: input_digest.clone(),
                content: BoundedText::new(content).unwrap(),
            }),
        };
        DriverOutcome::Receipt(Box::new(receipt(
            effect.request(),
            output,
            ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new("1").unwrap(),
                    tool_calls: DecimalUsage::new("0").unwrap(),
                    spend_micros_usd: DecimalUsage::new("0").unwrap(),
                },
            },
            effect.request().action.dispatch_id.as_str(),
        )))
    }
}

/// Exact receipt binding for evidence/frontier tests, independent of inference.
pub fn receipt(
    prepared: &PreparedIntent,
    output: ReceiptOutput,
    usage: ActualUsage,
    producer: &str,
) -> Receipt {
    Receipt::new(ReceiptData {
        schema_version: SchemaVersion::V3,
        producer_receipt_id: ProducerReceiptId::new(producer).unwrap(),
        task_id: prepared.action.task_id.clone(),
        intent_id: prepared.intent_id.clone(),
        dispatch_id: prepared.action.dispatch_id.clone(),
        action_digest: prepared.action_digest.clone(),
        usage,
        output,
    })
    .unwrap()
}

/// Content-addressed version-3 store command for controlled contract tests.
pub fn command(operation: StoreOperation) -> StoreCommand {
    let bytes = canonical_bytes(&operation, MAX_INTERNAL).unwrap();
    StoreCommand::new(CommandData {
        schema_version: SchemaVersion::V3,
        command_id: CommandId::new(content_digest(&bytes).as_str()).unwrap(),
        operation,
    })
    .unwrap()
}

/// SHA-256 of pre-native corpus revision 3 (`schema_version = 2`).
/// Provenance: pre-native frozen independent corpus at `/tmp/metron-intent-contracts-agy-5hobgpxs/independent-corpus.json`
/// supersedes revision 2; behavioral vectors and labels are unchanged.
pub const INDEPENDENT_CORPUS_SHA256: &str =
    "d1176eadd84d484145f847fbb18e4e68d633f1ff9715d0e99d3adc4d7e8d81af";

/// Exact SHA-256 of the accompanying frozen `corpus.freeze.json` receipt.
pub const INDEPENDENT_CORPUS_FREEZE_SHA256: &str =
    "eaf60331444bb525a674b47bcdf463a88dfa271892db621d09222b10a8fb2fc7";

/// Development-only frozen independent corpus payload (`schema_version = 2`).
pub fn independent_corpus_json() -> &'static str {
    include_str!("../fixtures/independent-corpus.json")
}

/// Development-only frozen corpus metadata receipt (`corpus.freeze.json`).
pub fn independent_corpus_freeze_json() -> &'static str {
    include_str!("../fixtures/corpus.freeze.json")
}
