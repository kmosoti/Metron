//! One-shot authority for bounded operational model comparisons, without qualification authority.
use metron_protocol::{
    ActualUsage, BoundedText, DecimalUsage, Digest, LOCAL_TEAM_RECIPE_SHA, LocalObservation,
    LocalOutputSchema, LocalProfile, LocalRole, MAX_LOCAL_CONTEXT, MAX_QUALIFICATION_REPORT,
    MAX_TEXT, PrincipalId, ProducerReceiptId, Receipt, ReceiptData, ReceiptOutput, SchemaVersion,
    SubmissionCohort, TaskId, UsageVector, canonical_bytes, content_digest, structural_digest,
};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::Path,
};

const LEGACY_RECIPE: &str = "7b8725c7b6328da6a97f487991ca56ac09b05e484a8d8738384c22645a0931a4";

pub struct ComparisonCase {
    pub id: BoundedText<128>,
    pub role: LocalRole,
    pub output_schema: LocalOutputSchema,
    pub context: BoundedText<MAX_LOCAL_CONTEXT>,
}
pub struct ComparisonRequest {
    pub dispatch_id: Digest,
    pub request_digest: Digest,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub action_digest: Digest,
    pub input_digest: Digest,
    pub role: LocalRole,
    pub output_schema: LocalOutputSchema,
    pub context: BoundedText<MAX_LOCAL_CONTEXT>,
    pub profile_digest: Digest,
    pub cohort: SubmissionCohort,
}
/// A one-shot authority, neither clonable nor deserializable.
///
/// ```compile_fail
/// use metron_harness::comparison::ComparisonPermit;
/// fn duplicate(permit: &ComparisonPermit) -> ComparisonPermit { permit.clone() }
/// ```
///
/// ```compile_fail
/// use metron_harness::comparison::ComparisonPermit;
/// let _: ComparisonPermit = metron_protocol::parse_envelope(b"{}").unwrap();
/// ```
pub struct ComparisonPermit {
    request: ComparisonRequest,
}
impl ComparisonPermit {
    pub fn request(&self) -> &ComparisonRequest {
        &self.request
    }
}
pub struct ComparisonResult {
    pub dispatch_id: Digest,
    pub observation: Option<LocalObservation>,
    pub rejection: Option<BoundedText<MAX_TEXT>>,
}
pub struct ComparisonSession {
    journal: File,
    session_id: Digest,
    task_id: TaskId,
    principal_id: PrincipalId,
    profile: LocalProfile,
    profile_digest: Digest,
    max_requests: u32,
    issued: u32,
    outstanding: Vec<ComparisonRequest>,
    blocked: bool,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn digest(domain: &str, parts: &[&[u8]]) -> io::Result<Digest> {
    structural_digest(domain, parts).map_err(|_| invalid("invalid comparison digest"))
}
fn append(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| invalid("comparison record too large"))?;
    file.write_all(&len.to_be_bytes())?;
    file.write_all(bytes)?;
    file.sync_all()
}
macro_rules! record {
    ($file:expr, $value:expr $(,)?) => {
        canonical_bytes(&$value, MAX_QUALIFICATION_REPORT)
            .map_err(|_| invalid("comparison record too large"))
            .and_then(|bytes| append($file, &bytes))
    };
}
impl ComparisonSession {
    pub fn create(
        directory: &Path,
        session_id: Digest,
        task_id: TaskId,
        principal_id: PrincipalId,
        profile: LocalProfile,
        max_requests: u32,
    ) -> io::Result<Self> {
        if !(1..=1024).contains(&max_requests) {
            return Err(invalid("invalid comparison budget"));
        }
        let profile_digest = profile
            .digest()
            .map_err(|_| invalid("invalid comparison profile"))?;
        let mut journal = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(format!("comparison-{}.journal", session_id.as_str())))?;
        File::open(directory)?.sync_all()?;
        record!(
            &mut journal,
            (
                "created",
                &session_id,
                &task_id,
                &principal_id,
                &profile_digest,
                max_requests,
            ),
        )?;
        Ok(Self {
            journal,
            session_id,
            task_id,
            principal_id,
            profile,
            profile_digest,
            max_requests,
            issued: 0,
            outstanding: Vec::new(),
            blocked: false,
        })
    }
    pub fn admit(&mut self, cases: &[ComparisonCase]) -> io::Result<Vec<ComparisonPermit>> {
        let count = u32::try_from(cases.len()).map_err(|_| invalid("invalid cohort"))?;
        if self.blocked
            || !self.outstanding.is_empty()
            || count == 0
            || count > u32::from(self.profile.max_sequences)
            || self
                .issued
                .checked_add(count)
                .is_none_or(|n| n > self.max_requests)
        {
            return Err(invalid("comparison session unavailable"));
        }
        let recipe = self.profile.prompt_recipe_digest.as_str();
        let mut requests = Vec::with_capacity(cases.len());
        for (position, case) in cases.iter().enumerate() {
            if case.id.as_str().is_empty()
                || !matches!(
                    (case.role, case.output_schema),
                    (LocalRole::Solver, LocalOutputSchema::Sum)
                        | (LocalRole::ToolSelector, LocalOutputSchema::ToolSelection)
                        | (LocalRole::Reasoner, LocalOutputSchema::Text)
                )
                || (recipe != LOCAL_TEAM_RECIPE_SHA
                    && (recipe != LEGACY_RECIPE || case.role == LocalRole::Reasoner))
            {
                return Err(invalid("invalid comparison case"));
            }
            let ordinal = (self.issued + position as u32).to_be_bytes();
            let input_digest = content_digest(case.context.as_str().as_bytes());
            let role_bytes = canonical_bytes(&(case.role, case.output_schema), 1024)
                .map_err(|_| invalid("invalid comparison role"))?;
            let request_digest = digest(
                "comparison-request/v1",
                &[
                    &self.session_id.bytes(),
                    &ordinal,
                    case.id.as_str().as_bytes(),
                    &self.profile_digest.bytes(),
                    &input_digest.bytes(),
                    &role_bytes,
                ],
            )?;
            let dispatch_id = digest(
                "comparison-dispatch/v1",
                &[&self.session_id.bytes(), &ordinal, &request_digest.bytes()],
            )?;
            let action_digest = digest(
                "comparison-action/v1",
                &[
                    &dispatch_id.bytes(),
                    &request_digest.bytes(),
                    self.task_id.as_str().as_bytes(),
                    self.principal_id.as_str().as_bytes(),
                ],
            )?;
            requests.push(ComparisonRequest {
                dispatch_id,
                request_digest,
                task_id: self.task_id.clone(),
                principal_id: self.principal_id.clone(),
                action_digest,
                input_digest,
                role: case.role,
                output_schema: case.output_schema,
                context: case.context.clone(),
                profile_digest: self.profile_digest.clone(),
                cohort: SubmissionCohort {
                    id: BoundedText::new("pending").map_err(|_| invalid("invalid cohort"))?,
                    size: count as u16,
                    position: position as u16,
                },
            });
        }
        let cohort = digest(
            "comparison-cohort/v1",
            &[
                &self.session_id.bytes(),
                &self.issued.to_be_bytes(),
                &count.to_be_bytes(),
            ],
        )?;
        for request in &mut requests {
            request.cohort.id =
                BoundedText::new(cohort.as_str()).map_err(|_| invalid("invalid cohort"))?;
        }
        let evidence: Vec<_> = requests
            .iter()
            .map(|r| {
                (
                    (
                        &r.dispatch_id,
                        &r.request_digest,
                        &r.task_id,
                        &r.principal_id,
                        &r.action_digest,
                        &r.input_digest,
                    ),
                    (
                        r.role,
                        r.output_schema,
                        &r.context,
                        &r.profile_digest,
                        &r.cohort,
                    ),
                )
            })
            .collect();
        if let Err(error) = record!(&mut self.journal, ("admitted", evidence)) {
            self.blocked = true;
            return Err(error);
        }
        self.issued += count;
        // Requests in the session are independent copies; the permits themselves remain non-clonable.
        let permits = requests
            .iter()
            .map(|r| ComparisonPermit {
                request: copy_request(r),
            })
            .collect();
        self.outstanding = requests;
        Ok(permits)
    }
    pub fn settle(&mut self, results: &[ComparisonResult]) -> io::Result<Vec<Option<Receipt>>> {
        if self.blocked || self.outstanding.is_empty() || self.outstanding.len() != results.len() {
            self.blocked = true;
            return Err(invalid("comparison settlement mismatch"));
        }
        let mut receipts = Vec::with_capacity(results.len());
        for (request, result) in self.outstanding.iter().zip(results) {
            if result.dispatch_id != request.dispatch_id
                || (result.observation.is_some() && result.rejection.is_some())
                || result.observation.as_ref().is_some_and(|o| {
                    o.input_digest != request.input_digest
                        || o.submission_cohort != request.cohort
                        || o.validate(
                            &self.profile,
                            &request.request_digest,
                            request.output_schema,
                        )
                        .is_err()
                })
            {
                self.blocked = true;
                return Err(invalid("comparison result binding mismatch"));
            }
            let output = if let Some(observation) = &result.observation {
                Some((
                    ReceiptOutput::LocalModel {
                        observation: Box::new(observation.clone()),
                    },
                    "1",
                ))
            } else {
                result.rejection.as_ref().map(|diagnostic| {
                    (
                        ReceiptOutput::Failure {
                            diagnostic: diagnostic.clone(),
                        },
                        "0",
                    )
                })
            };
            let receipt = output
                .map(|(output, model)| -> io::Result<Receipt> {
                    Receipt::new(ReceiptData {
                        schema_version: SchemaVersion::V2,
                        producer_receipt_id: ProducerReceiptId::new(request.dispatch_id.as_str())
                            .map_err(|_| invalid("invalid receipt id"))?,
                        task_id: request.task_id.clone(),
                        intent_id: request.request_digest.clone(),
                        dispatch_id: request.dispatch_id.clone(),
                        action_digest: request.action_digest.clone(),
                        usage: ActualUsage::Known {
                            usage: UsageVector {
                                model_requests: DecimalUsage::new(model)
                                    .map_err(|_| invalid("invalid usage"))?,
                                tool_calls: DecimalUsage::new("0")
                                    .map_err(|_| invalid("invalid usage"))?,
                                spend_micros_usd: DecimalUsage::new("0")
                                    .map_err(|_| invalid("invalid usage"))?,
                            },
                        },
                        output,
                    })
                    .map_err(|_| invalid("invalid receipt"))
                })
                .transpose();
            match receipt {
                Ok(receipt) => receipts.push(receipt),
                Err(error) => {
                    self.blocked = true;
                    return Err(error);
                }
            }
        }
        let evidence: Vec<_> = results
            .iter()
            .zip(&receipts)
            .map(|(r, receipt)| (&r.dispatch_id, &r.observation, &r.rejection, receipt))
            .collect();
        if let Err(error) = record!(&mut self.journal, ("settled", evidence)) {
            self.blocked = true;
            return Err(error);
        }
        self.outstanding.clear();
        if receipts.iter().any(Option::is_none) {
            self.blocked = true;
        }
        Ok(receipts)
    }
}
fn copy_request(r: &ComparisonRequest) -> ComparisonRequest {
    ComparisonRequest {
        dispatch_id: r.dispatch_id.clone(),
        request_digest: r.request_digest.clone(),
        task_id: r.task_id.clone(),
        principal_id: r.principal_id.clone(),
        action_digest: r.action_digest.clone(),
        input_digest: r.input_digest.clone(),
        role: r.role,
        output_schema: r.output_schema,
        context: r.context.clone(),
        profile_digest: r.profile_digest.clone(),
        cohort: r.cohort.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{LocalDevice, SqlU64, gguf_embedded_tokenizer_digest};
    fn local_profile() -> LocalProfile {
        LocalProfile {
            schema_version: SchemaVersion::V2,
            auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").unwrap(),
            auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871")
                .unwrap(),
            artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").unwrap(),
            artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2")
                .unwrap(),
            artifact_name: BoundedText::new("LFM3-1.7B-Q4_K_M.gguf").unwrap(),
            artifact_digest: Digest::new(
                "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
            )
            .unwrap(),
            artifact_bytes: SqlU64::new(1099627808).unwrap(),
            embedded_tokenizer_digest: gguf_embedded_tokenizer_digest(
                &Digest::new("3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4")
                    .unwrap(),
            )
            .unwrap(),
            template_digest: content_digest(b"template"),
            prompt_recipe_digest: content_digest(b"recipe"),
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

    fn setup(tag: &str) -> (std::path::PathBuf, ComparisonSession) {
        let dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("comparison-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut profile = local_profile();
        profile.prompt_recipe_digest = Digest::new(LOCAL_TEAM_RECIPE_SHA).unwrap();
        profile.max_sequences = 2;
        let session = ComparisonSession::create(
            &dir,
            content_digest(b"session"),
            TaskId::new("task").unwrap(),
            PrincipalId::new("principal").unwrap(),
            profile,
            2,
        )
        .unwrap();
        (dir, session)
    }
    fn case() -> ComparisonCase {
        ComparisonCase {
            id: BoundedText::new("a").unwrap(),
            role: LocalRole::Reasoner,
            output_schema: LocalOutputSchema::Text,
            context: BoundedText::new("hello").unwrap(),
        }
    }
    #[test]
    fn admission_budget_binding_and_unknown() {
        let (dir, mut session) = setup("unknown");
        let permits = session.admit(&[case(), case()]).unwrap();
        assert_ne!(
            permits[0].request().dispatch_id,
            permits[1].request().dispatch_id
        );
        assert!(session.admit(&[case()]).is_err());
        let results = vec![
            ComparisonResult {
                dispatch_id: permits[0].request().dispatch_id.clone(),
                observation: None,
                rejection: Some(BoundedText::new("rejected").unwrap()),
            },
            ComparisonResult {
                dispatch_id: permits[1].request().dispatch_id.clone(),
                observation: None,
                rejection: None,
            },
        ];
        let receipts = session.settle(&results).unwrap();
        assert!(receipts[0].is_some());
        assert!(receipts[1].is_none());
        assert!(session.admit(&[case()]).is_err());
        assert!(
            ComparisonSession::create(
                &dir,
                content_digest(b"session"),
                TaskId::new("task").unwrap(),
                PrincipalId::new("principal").unwrap(),
                local_profile(),
                1
            )
            .is_err()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn wrong_binding_permanently_blocks() {
        let dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("comparison-binding-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut profile = local_profile();
        profile.prompt_recipe_digest = Digest::new(LOCAL_TEAM_RECIPE_SHA).unwrap();
        let mut session = ComparisonSession::create(
            &dir,
            content_digest(b"other"),
            TaskId::new("task").unwrap(),
            PrincipalId::new("principal").unwrap(),
            profile,
            1,
        )
        .unwrap();
        let mut bad = case();
        bad.output_schema = LocalOutputSchema::Sum;
        assert!(session.admit(&[bad]).is_err());
        let _permit = session.admit(&[case()]).unwrap();
        assert!(
            session
                .settle(&[ComparisonResult {
                    dispatch_id: content_digest(b"wrong"),
                    observation: None,
                    rejection: None
                }])
                .is_err()
        );
        assert!(session.admit(&[case()]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn known_result_consumes_budget() {
        let dir = std::env::var_os("CARGO_TARGET_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("comparison-budget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut profile = local_profile();
        profile.prompt_recipe_digest = Digest::new(LOCAL_TEAM_RECIPE_SHA).unwrap();
        let mut session = ComparisonSession::create(
            &dir,
            content_digest(b"budget"),
            TaskId::new("task").unwrap(),
            PrincipalId::new("principal").unwrap(),
            profile,
            1,
        )
        .unwrap();
        let permits = session.admit(&[case()]).unwrap();
        let results = [ComparisonResult {
            dispatch_id: permits[0].request().dispatch_id.clone(),
            observation: None,
            rejection: Some(BoundedText::new("rejected").unwrap()),
        }];
        assert!(session.settle(&results).unwrap()[0].is_some());
        assert!(session.admit(&[case()]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn observed_receipt_binds_output_and_usage() {
        let (dir, mut session) = setup("observed");
        let permits = session.admit(&[case()]).unwrap();
        let request = permits[0].request();
        let observation = LocalObservation {
            schema_version: SchemaVersion::V2,
            profile_digest: request.profile_digest.clone(),
            request_digest: request.request_digest.clone(),
            input_digest: request.input_digest.clone(),
            content: BoundedText::new("answer").unwrap(),
            reasoning: None,
            native_calls: None,
            prompt_tokens: 1,
            completion_tokens: 1,
            submission_cohort: request.cohort.clone(),
            backend_batch: metron_protocol::BatchProvenance::Unknown,
            response: Some(metron_protocol::LocalResponse::Text),
        };
        let result = ComparisonResult {
            dispatch_id: request.dispatch_id.clone(),
            observation: Some(observation),
            rejection: None,
        };
        let receipts = session.settle(&[result]).unwrap();
        let receipt = receipts[0].as_ref().unwrap();
        assert!(matches!(
            &receipt.data().output,
            ReceiptOutput::LocalModel { .. }
        ));
        let ActualUsage::Known { usage } = &receipt.data().usage else {
            panic!("unknown usage")
        };
        assert_eq!(usage.model_requests.as_str(), "1");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settlement_io_failure_blocks_reuse() {
        let (dir, mut session) = setup("io");
        let permits = session.admit(&[case()]).unwrap();
        let journal_path = dir.join(format!(
            "comparison-{}.journal",
            session.session_id.as_str()
        ));
        session.journal = File::open(journal_path).unwrap();
        let result = ComparisonResult {
            dispatch_id: permits[0].request().dispatch_id.clone(),
            observation: None,
            rejection: Some(BoundedText::new("rejected").unwrap()),
        };
        assert!(session.settle(&[result]).is_err());
        assert!(session.admit(&[case()]).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
