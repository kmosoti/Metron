//! One-shot authority for bounded operational model comparisons, without qualification authority.
use metron_protocol::{
    ActualUsage, BoundedText, DecimalUsage, Digest, LOCAL_TEAM_RECIPE_SHA, LocalObservation,
    LocalOutputSchema, LocalProfile, LocalRole, MAX_LOCAL_CONTEXT, MAX_QUALIFICATION_REPORT,
    MAX_TEXT, PrincipalId, ProducerReceiptId, Receipt, ReceiptData, ReceiptOutput, SchemaVersion,
    SubmissionCohort, TaskId, UsageVector, canonical_bytes, content_digest, structural_digest,
};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
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
/// use metron_inference_local::comparison::ComparisonPermit;
/// fn duplicate(permit: &ComparisonPermit) -> ComparisonPermit { permit.clone() }
/// ```
///
/// ```compile_fail
/// use metron_inference_local::comparison::ComparisonPermit;
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

/// Exact source expectations for a read-only audit. Cases must be supplied in
/// admission order; their IDs are not present in the journal and cannot be inferred.
pub struct ComparisonAuditExpectation<'a> {
    pub session_id: &'a Digest,
    pub task_id: &'a TaskId,
    pub principal_id: &'a PrincipalId,
    pub profile: &'a LocalProfile,
    pub max_requests: u32,
    pub cases: &'a [ComparisonCase],
}

/// A validated request binding and its durable outcome; this grants no authority.
pub struct ComparisonAuditedRequest {
    pub request: ComparisonRequest,
    pub outcome: ComparisonAuditOutcome,
}

#[derive(Debug)]
pub enum ComparisonAuditOutcome {
    /// An admitted request with no settlement frame. Its reservation is retained.
    Unfinished,
    /// A settlement explicitly recorded an unknown outcome. Its reservation is retained.
    Unknown,
    /// The receipt and (if present) observation passed the same binding checks as settlement.
    Receipt(Box<Receipt>),
}

/// Structural journal evidence, not authentication of a native worker or semantic acceptance.
/// Callers must establish journal provenance separately. A local-model receipt only
/// establishes consistency of the recorded observation with the supplied profile/input.
pub struct ComparisonAudit {
    pub journal_digest: Digest,
    pub admitted: u32,
    pub settled: u32,
    pub receipts: u32,
    pub local_model_receipts: u32,
    pub unknown: u32,
    pub unfinished: u32,
    pub requests: Vec<ComparisonAuditedRequest>,
}

/// Audit the existing comparison format without opening it for writing, creating
/// permits, or resuming execution. Reject partial frames, noncanonical records,
/// invalid transitions and bindings. The whole stream is bounded to 256 MiB and
/// each frame to the writer's existing record limit. A complete admission without
/// settlement is valid uncertainty, whereas a partially written frame is an error.
pub fn audit_comparison_session(
    path: &Path,
    expected: &ComparisonAuditExpectation<'_>,
) -> io::Result<ComparisonAudit> {
    use sha2::{Digest as _, Sha256};
    if !(1..=1024).contains(&expected.max_requests)
        || expected.cases.len() > expected.max_requests as usize
    {
        return Err(invalid("invalid comparison audit expectation"));
    }
    let profile_digest = expected
        .profile
        .digest()
        .map_err(|_| invalid("invalid comparison profile"))?;
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes_read = 0usize;
    let mut next_frame = || -> io::Result<Option<Vec<u8>>> {
        let mut prefix = [0; 4];
        if file.read(&mut prefix[..1])? == 0 {
            return Ok(None);
        }
        file.read_exact(&mut prefix[1..])?;
        let len = u32::from_be_bytes(prefix) as usize;
        bytes_read = bytes_read
            .checked_add(4 + len)
            .ok_or_else(|| invalid("comparison audit size overflow"))?;
        if len == 0 || len > MAX_QUALIFICATION_REPORT || bytes_read > 256 * 1024 * 1024 {
            return Err(invalid("comparison audit size limit"));
        }
        let mut bytes = vec![0; len];
        file.read_exact(&mut bytes)?;
        hasher.update(prefix);
        hasher.update(&bytes);
        Ok(Some(bytes))
    };
    let created = canonical_bytes(
        &(
            "created",
            expected.session_id,
            expected.task_id,
            expected.principal_id,
            &profile_digest,
            expected.max_requests,
        ),
        MAX_QUALIFICATION_REPORT,
    )
    .map_err(|_| invalid("invalid comparison creation"))?;
    if next_frame()?.as_deref() != Some(created.as_slice()) {
        return Err(invalid("comparison creation mismatch"));
    }
    let mut audit = ComparisonAudit {
        journal_digest: content_digest(b""),
        admitted: 0,
        settled: 0,
        receipts: 0,
        local_model_receipts: 0,
        unknown: 0,
        unfinished: 0,
        requests: Vec::new(),
    };
    let mut pending_start = 0usize;
    while let Some(bytes) = next_frame()? {
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| invalid("malformed comparison frame"))?;
        match value.get(0).and_then(serde_json::Value::as_str) {
            Some("admitted") => {
                if pending_start != audit.requests.len() || audit.unknown != 0 {
                    return Err(invalid("comparison admission after unresolved outcome"));
                }
                let count = value
                    .get(1)
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| invalid("invalid comparison cohort"))?
                    .len();
                let end = audit
                    .requests
                    .len()
                    .checked_add(count)
                    .ok_or_else(|| invalid("comparison admission overflow"))?;
                if count == 0
                    || count > usize::from(expected.profile.max_sequences)
                    || end > expected.cases.len()
                    || end > expected.max_requests as usize
                {
                    return Err(invalid("comparison admission budget mismatch"));
                }
                let cohort_id = digest(
                    "comparison-cohort/v1",
                    &[
                        &expected.session_id.bytes(),
                        &audit.admitted.to_be_bytes(),
                        &(count as u32).to_be_bytes(),
                    ],
                )?;
                for (position, case) in expected.cases[audit.requests.len()..end].iter().enumerate()
                {
                    let recipe = expected.profile.prompt_recipe_digest.as_str();
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
                        return Err(invalid("invalid comparison audit case"));
                    }
                    let ordinal = (audit.admitted + position as u32).to_be_bytes();
                    let input_digest = content_digest(case.context.as_str().as_bytes());
                    let role_bytes = canonical_bytes(&(case.role, case.output_schema), 1024)
                        .map_err(|_| invalid("invalid comparison role"))?;
                    let request_digest = digest(
                        "comparison-request/v1",
                        &[
                            &expected.session_id.bytes(),
                            &ordinal,
                            case.id.as_str().as_bytes(),
                            &profile_digest.bytes(),
                            &input_digest.bytes(),
                            &role_bytes,
                        ],
                    )?;
                    let dispatch_id = digest(
                        "comparison-dispatch/v1",
                        &[
                            &expected.session_id.bytes(),
                            &ordinal,
                            &request_digest.bytes(),
                        ],
                    )?;
                    let action_digest = digest(
                        "comparison-action/v1",
                        &[
                            &dispatch_id.bytes(),
                            &request_digest.bytes(),
                            expected.task_id.as_str().as_bytes(),
                            expected.principal_id.as_str().as_bytes(),
                        ],
                    )?;
                    audit.requests.push(ComparisonAuditedRequest {
                        request: ComparisonRequest {
                            dispatch_id,
                            request_digest,
                            task_id: expected.task_id.clone(),
                            principal_id: expected.principal_id.clone(),
                            action_digest,
                            input_digest,
                            role: case.role,
                            output_schema: case.output_schema,
                            context: case.context.clone(),
                            profile_digest: profile_digest.clone(),
                            cohort: SubmissionCohort {
                                id: BoundedText::new(cohort_id.as_str())
                                    .map_err(|_| invalid("invalid cohort"))?,
                                size: count as u16,
                                position: position as u16,
                            },
                        },
                        outcome: ComparisonAuditOutcome::Unfinished,
                    });
                }
                let evidence: Vec<_> = audit.requests[pending_start..]
                    .iter()
                    .map(|entry| {
                        let r = &entry.request;
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
                if canonical_bytes(&("admitted", evidence), MAX_QUALIFICATION_REPORT)
                    .map_err(|_| invalid("invalid admission"))?
                    != bytes
                {
                    return Err(invalid("comparison admission binding mismatch"));
                }
                audit.admitted += count as u32;
            }
            Some("settled") => {
                type Settlement = (
                    Digest,
                    Option<LocalObservation>,
                    Option<BoundedText<MAX_TEXT>>,
                    Option<Receipt>,
                );
                let (tag, settlements): (String, Vec<Settlement>) = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("invalid comparison settlement"))?;
                if pending_start == audit.requests.len()
                    || settlements.len() != audit.requests.len() - pending_start
                    || canonical_bytes(&(&tag, &settlements), MAX_QUALIFICATION_REPORT)
                        .map_err(|_| invalid("invalid comparison settlement"))?
                        != bytes
                {
                    return Err(invalid("comparison settlement count mismatch"));
                }
                for (entry, (dispatch_id, observation, rejection, receipt)) in
                    audit.requests[pending_start..].iter_mut().zip(settlements)
                {
                    let r = &entry.request;
                    if dispatch_id != r.dispatch_id
                        || (observation.is_some() && rejection.is_some())
                        || observation.as_ref().is_some_and(|o| {
                            o.input_digest != r.input_digest
                                || o.submission_cohort != r.cohort
                                || o.validate(expected.profile, &r.request_digest, r.output_schema)
                                    .is_err()
                        })
                    {
                        return Err(invalid("comparison observation binding mismatch"));
                    }
                    let output = if let Some(observation) = observation {
                        Some((
                            ReceiptOutput::LocalModel {
                                observation: Box::new(observation),
                            },
                            "1",
                        ))
                    } else {
                        rejection.map(|diagnostic| (ReceiptOutput::Failure { diagnostic }, "0"))
                    };
                    match (output, receipt) {
                        (None, None) => {
                            entry.outcome = ComparisonAuditOutcome::Unknown;
                            audit.unknown += 1;
                        }
                        (Some((output, model)), Some(receipt)) => {
                            let expected_data = ReceiptData {
                                schema_version: SchemaVersion::V2,
                                producer_receipt_id: ProducerReceiptId::new(r.dispatch_id.as_str())
                                    .map_err(|_| invalid("invalid receipt id"))?,
                                task_id: r.task_id.clone(),
                                intent_id: r.request_digest.clone(),
                                dispatch_id: r.dispatch_id.clone(),
                                action_digest: r.action_digest.clone(),
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
                            };
                            if receipt.data() != &expected_data {
                                return Err(invalid("comparison receipt binding mismatch"));
                            }
                            audit.receipts += 1;
                            audit.local_model_receipts += u32::from(model == "1");
                            entry.outcome = ComparisonAuditOutcome::Receipt(Box::new(receipt));
                        }
                        _ => return Err(invalid("comparison receipt presence mismatch")),
                    }
                }
                audit.settled += (audit.requests.len() - pending_start) as u32;
                pending_start = audit.requests.len();
            }
            _ => return Err(invalid("unknown comparison frame")),
        }
    }
    if audit.requests.len() != expected.cases.len() {
        return Err(invalid("comparison expected cases mismatch"));
    }
    audit.unfinished = audit.admitted - audit.settled;
    audit.journal_digest = Digest::new(format!("{:x}", hasher.finalize()))
        .map_err(|_| invalid("invalid journal digest"))?;
    Ok(audit)
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
    fn audited(
        session: &ComparisonSession,
        dir: &Path,
        cases: &[ComparisonCase],
    ) -> io::Result<ComparisonAudit> {
        audit_comparison_session(
            &dir.join(format!(
                "comparison-{}.journal",
                session.session_id.as_str()
            )),
            &ComparisonAuditExpectation {
                session_id: &session.session_id,
                task_id: &session.task_id,
                principal_id: &session.principal_id,
                profile: &session.profile,
                max_requests: session.max_requests,
                cases,
            },
        )
    }

    fn journal_path(session: &ComparisonSession, dir: &Path) -> std::path::PathBuf {
        dir.join(format!(
            "comparison-{}.journal",
            session.session_id.as_str()
        ))
    }

    fn frames(bytes: &[u8]) -> Vec<serde_json::Value> {
        let mut frames = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            frames.push(serde_json::from_slice(&bytes[offset..offset + len]).unwrap());
            offset += len;
        }
        frames
    }

    fn write_frames(path: &Path, values: &[serde_json::Value]) {
        type Admission = (
            (Digest, Digest, TaskId, PrincipalId, Digest, Digest),
            (
                LocalRole,
                LocalOutputSchema,
                BoundedText<MAX_LOCAL_CONTEXT>,
                Digest,
                SubmissionCohort,
            ),
        );
        type Settlement = (
            Digest,
            Option<LocalObservation>,
            Option<BoundedText<MAX_TEXT>>,
            Option<Receipt>,
        );
        let mut file = File::create(path).unwrap();
        for value in values {
            let mut bytes = serde_json::to_vec(value).unwrap();
            // Restore writer field order so valid-shaped tampering tests binding checks.
            match value[0].as_str() {
                Some("admitted") => {
                    if let Ok(typed) =
                        serde_json::from_value::<(String, Vec<Admission>)>(value.clone())
                    {
                        bytes = serde_json::to_vec(&typed).unwrap();
                    }
                }
                Some("settled") => {
                    if let Ok(typed) =
                        serde_json::from_value::<(String, Vec<Settlement>)>(value.clone())
                    {
                        bytes = serde_json::to_vec(&typed).unwrap();
                    }
                }
                _ => {}
            }
            append(&mut file, &bytes).unwrap();
        }
    }

    #[test]
    fn audit_preserves_unfinished_and_unknown_reservations() {
        let (dir, mut session) = setup("audit-holds");
        let cases = [case(), case()];
        let permits = session.admit(&cases).unwrap();
        let before = std::fs::read(journal_path(&session, &dir)).unwrap();
        let audit = audited(&session, &dir, &cases).unwrap();
        assert_eq!(
            (
                audit.admitted,
                audit.settled,
                audit.unfinished,
                audit.unknown
            ),
            (2, 0, 2, 0)
        );
        assert!(
            audit
                .requests
                .iter()
                .all(|r| matches!(r.outcome, ComparisonAuditOutcome::Unfinished))
        );
        assert_eq!(audit.journal_digest, content_digest(&before));
        assert_eq!(std::fs::read(journal_path(&session, &dir)).unwrap(), before);
        session
            .settle(&[
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
            ])
            .unwrap();
        let audit = audited(&session, &dir, &cases).unwrap();
        assert_eq!(
            (
                audit.settled,
                audit.receipts,
                audit.local_model_receipts,
                audit.unknown,
                audit.unfinished
            ),
            (2, 1, 0, 1, 0)
        );
        assert!(matches!(
            audit.requests[1].outcome,
            ComparisonAuditOutcome::Unknown
        ));
        let path = journal_path(&session, &dir);
        let mut records = frames(&std::fs::read(&path).unwrap());
        records.push(records[1].clone());
        write_frames(&path, &records);
        assert!(audited(&session, &dir, &cases).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_rejects_partial_oversized_reordered_and_duplicate_frames() {
        let (dir, mut session) = setup("audit-frames");
        let cases = [case()];
        let permits = session.admit(&cases).unwrap();
        session
            .settle(&[ComparisonResult {
                dispatch_id: permits[0].request().dispatch_id.clone(),
                observation: None,
                rejection: Some(BoundedText::new("rejected").unwrap()),
            }])
            .unwrap();
        let path = journal_path(&session, &dir);
        let original = std::fs::read(&path).unwrap();
        let records = frames(&original);
        // Missing frame bytes, even if the prior complete frames are valid, are never repaired.
        for cut in [0, 1, 3, original.len() - 1] {
            std::fs::write(&path, &original[..cut]).unwrap();
            assert!(audited(&session, &dir, &cases).is_err());
        }
        let mut malformed = original.clone();
        malformed.extend_from_slice(&1u32.to_be_bytes());
        malformed.push(b'{');
        std::fs::write(&path, malformed).unwrap();
        assert!(audited(&session, &dir, &cases).is_err());
        let mut oversized = original.clone();
        oversized.extend_from_slice(&((MAX_QUALIFICATION_REPORT + 1) as u32).to_be_bytes());
        std::fs::write(&path, oversized).unwrap();
        assert!(audited(&session, &dir, &cases).is_err());
        for indices in [
            vec![0, 2, 1],
            vec![0, 1, 1, 2],
            vec![0, 1, 2, 2],
            vec![0, 0],
        ] {
            write_frames(
                &path,
                &indices
                    .into_iter()
                    .map(|i| records[i].clone())
                    .collect::<Vec<_>>(),
            );
            assert!(audited(&session, &dir, &cases).is_err());
        }
        std::fs::write(&path, &original).unwrap();
        assert!(audited(&session, &dir, &cases).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_checks_ordinals_across_cohorts_and_cumulative_budget() {
        let (dir, mut session) = setup("audit-cohorts");
        for _ in 0..2 {
            let permits = session.admit(&[case()]).unwrap();
            session
                .settle(&[ComparisonResult {
                    dispatch_id: permits[0].request().dispatch_id.clone(),
                    observation: None,
                    rejection: Some(BoundedText::new("rejected").unwrap()),
                }])
                .unwrap();
        }
        let cases = [case(), case()];
        let audit = audited(&session, &dir, &cases).unwrap();
        assert_eq!((audit.admitted, audit.settled, audit.receipts), (2, 2, 2));
        assert_ne!(
            audit.requests[0].request.dispatch_id,
            audit.requests[1].request.dispatch_id
        );
        assert_ne!(
            audit.requests[0].request.cohort.id,
            audit.requests[1].request.cohort.id
        );
        // A matching reduced creation budget cannot hide cumulative over-admission.
        let path = journal_path(&session, &dir);
        let mut records = frames(&std::fs::read(&path).unwrap());
        records[0][5] = serde_json::json!(1);
        write_frames(&path, &records);
        session.max_requests = 1;
        assert!(audited(&session, &dir, &cases).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn audit_rejects_counterfeit_receipts_and_source_bindings() {
        let (dir, mut session) = setup("audit-bindings");
        let cases = [case()];
        let permits = session.admit(&cases).unwrap();
        session
            .settle(&[ComparisonResult {
                dispatch_id: permits[0].request().dispatch_id.clone(),
                observation: None,
                rejection: Some(BoundedText::new("rejected").unwrap()),
            }])
            .unwrap();
        let path = journal_path(&session, &dir);
        let original = std::fs::read(&path).unwrap();
        let records = frames(&original);
        let mut wrong_case = case();
        wrong_case.id = BoundedText::new("different-source-id").unwrap();
        assert!(audited(&session, &dir, &[wrong_case]).is_err());
        let mut wrong_context = case();
        wrong_context.context = BoundedText::new("different-context").unwrap();
        assert!(audited(&session, &dir, &[wrong_context]).is_err());
        assert!(audited(&session, &dir, &[]).is_err());
        // The existing session has budget two; asserting a budget of one must fail creation binding.
        session.max_requests = 1;
        assert!(audited(&session, &dir, &cases).is_err());
        session.max_requests = 2;
        let replacements = [
            (vec![0, 2], serde_json::json!("wrong-task")),
            (
                vec![1, 1, 0, 0, 4],
                serde_json::json!(content_digest(b"wrong-action")),
            ),
            (vec![2, 1, 0, 3], serde_json::Value::Null),
            (vec![2, 1], serde_json::json!([])),
        ];
        for (indices, replacement) in replacements {
            let mut changed = serde_json::Value::Array(records.clone());
            let mut field = &mut changed;
            for index in indices {
                field = &mut field[index];
            }
            *field = replacement;
            write_frames(&path, changed.as_array().unwrap());
            assert!(audited(&session, &dir, &cases).is_err());
        }
        let mut changed = records.clone();
        changed[2][1][0][3]["dispatch_id"] = serde_json::json!(content_digest(b"wrong-dispatch"));
        write_frames(&path, &changed);
        assert!(audited(&session, &dir, &cases).is_err());
        let mut changed = records.clone();
        changed[2][1][0][3]["usage"]["usage"]["model_requests"] = serde_json::json!("1");
        write_frames(&path, &changed);
        assert!(audited(&session, &dir, &cases).is_err());
        std::fs::write(&path, original).unwrap();
        assert!(audited(&session, &dir, &cases).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
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
        let audit = audited(&session, &dir, &[case()]).unwrap();
        assert_eq!(
            (
                audit.admitted,
                audit.settled,
                audit.receipts,
                audit.local_model_receipts,
                audit.unknown,
                audit.unfinished
            ),
            (1, 1, 1, 1, 0, 0)
        );
        let ComparisonAuditOutcome::Receipt(read_receipt) = &audit.requests[0].outcome else {
            panic!("missing receipt")
        };
        assert_eq!(read_receipt.as_ref(), receipt);
        let path = journal_path(&session, &dir);
        let bytes = std::fs::read(&path).unwrap();
        // Tampering either the separate observation or receipt copy cannot pass binding.
        let mut changed = frames(&bytes);
        changed[2][1][0][1]["input_digest"] = serde_json::json!(content_digest(b"wrong-input"));
        write_frames(&path, &changed);
        assert!(audited(&session, &dir, &[case()]).is_err());
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
