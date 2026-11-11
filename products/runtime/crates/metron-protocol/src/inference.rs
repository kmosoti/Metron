//! Bounded, versioned local inference identities and observations.
use crate::{
    BoundedText, Digest, MAX_ENVELOPE, SchemaVersion, SqlU64, ValidationError, canonical_bytes,
    structural_digest,
};
use serde::{Deserialize, Serialize};

pub const MAX_MODEL_OUTPUT: usize = 16 * 1024;
pub const MAX_LOCAL_CONTEXT: usize = 64 * 1024;
pub const MAX_LOCAL_REQUEST_FRAME: usize = 512 * 1024;
pub const MAX_QUALIFICATION_REPORT: usize = 16 * 1024 * 1024;
pub const LOCAL_REASONER_SYSTEM: &str = "Answer the user's request directly. Return only the requested answer. Do not call or execute tools.";
/// SHA-256 of the compact JSON array [solver system, selector system, reasoner system], in that order.
pub const LOCAL_TEAM_RECIPE_SHA: &str =
    "20f6fb9769c95906c19e09cf60873ebc9a4517179fd34e9814856b9081670136";

/// Identify the embedded tokenizer through the full verified GGUF digest.
/// This is not the byte hash of an external tokenizer file.
pub fn gguf_embedded_tokenizer_digest(artifact_digest: &Digest) -> Result<Digest, ValidationError> {
    structural_digest("gguf-embedded-tokenizer/v1", &[&artifact_digest.bytes()])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOutputSchema {
    Sum,
    ToolSelection,
    Text,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalRole {
    Solver,
    ToolSelector,
    Reasoner,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SumResponse {
    pub count: u32,
    pub sum: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    ReadFixture,
    RequestModel,
    PublishArtifact,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSelectionResponse {
    pub tool: ToolChoice,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LocalResponse {
    Sum(SumResponse),
    ToolSelection(ToolSelectionResponse),
    Text,
}

pub fn parse_local_response(
    bytes: &[u8],
    schema: LocalOutputSchema,
) -> Result<LocalResponse, ValidationError> {
    if bytes.len() > MAX_MODEL_OUTPUT {
        return Err(ValidationError::Limit);
    }
    if schema == LocalOutputSchema::Text {
        let text = std::str::from_utf8(bytes).map_err(|e| ValidationError::Json(e.to_string()))?;
        return if text.is_empty() {
            Err(ValidationError::Limit)
        } else {
            Ok(LocalResponse::Text)
        };
    }
    match schema {
        LocalOutputSchema::Sum => serde_json::from_slice(bytes).map(LocalResponse::Sum),
        LocalOutputSchema::ToolSelection => {
            serde_json::from_slice(bytes).map(LocalResponse::ToolSelection)
        }
        LocalOutputSchema::Text => unreachable!(),
    }
    .map_err(|e| ValidationError::Json(e.to_string()))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalDevice {
    Cpu,
    Cuda {
        uuid: BoundedText<128>,
        model: BoundedText<128>,
        memory_bytes: SqlU64,
        driver: BoundedText<128>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BatchProvenance {
    Unknown,
    Known {
        id: BoundedText<128>,
        size: u16,
        position: u16,
    },
}

/// Membership in a caller submission group, not evidence of backend batching.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmissionCohort {
    pub id: BoundedText<128>,
    pub size: u16,
    pub position: u16,
}

/// Immutable effective configuration. Paths and wall-clock observations are excluded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalProfile {
    pub schema_version: SchemaVersion,
    pub auxiliary_repository: BoundedText<128>,
    pub auxiliary_revision: BoundedText<128>,
    pub artifact_repository: BoundedText<128>,
    pub artifact_revision: BoundedText<128>,
    pub artifact_name: BoundedText<128>,
    pub artifact_digest: Digest,
    pub artifact_bytes: SqlU64,
    /// Identity derived from the complete pinned GGUF; not a tokenizer.json hash.
    pub embedded_tokenizer_digest: Digest,
    pub template_digest: Digest,
    pub prompt_recipe_digest: Digest,
    pub backend_revision: BoundedText<128>,
    pub backend_build_digest: Digest,
    pub cpu_identity: BoundedText<256>,
    pub cpu_features_digest: Digest,
    pub device: LocalDevice,
    pub cpu_threads: u8,
    pub context_tokens: u32,
    pub max_input_tokens: u32,
    pub max_output_tokens: u32,
    pub temperature_milli: u16,
    pub top_k: u16,
    pub repeat_penalty_milli: u16,
    pub max_sequences: u16,
    pub queue_capacity: u16,
    pub queue_wait_ms: u32,
    pub deadline_ms: u32,
    pub chunked_prefill: bool,
    pub scoped_prefix_cache: bool,
    /// Optional effective cache target; this is not a process memory cap.
    pub cache_target_bytes: Option<SqlU64>,
    /// Strict allocation caps cannot be promised by the current backend.
    pub strict_memory_cap_bytes: Option<SqlU64>,
}
impl LocalProfile {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != SchemaVersion::V2
            || self.artifact_bytes.get() == 0
            || self.context_tokens == 0
            || self.cpu_threads == 0
            || self.cpu_threads > 4
            || self.context_tokens > 8192
            || self.max_input_tokens == 0
            || self.max_output_tokens == 0
            || self.max_output_tokens > 512
            || self
                .max_input_tokens
                .checked_add(self.max_output_tokens)
                .is_none_or(|n| n > self.context_tokens)
            || self.strict_memory_cap_bytes.is_some()
            || !matches!(self.max_sequences, 1 | 2 | 4 | 8)
            || self.queue_capacity == 0
            || self.queue_capacity > 8
            || self.queue_wait_ms == 0
            || self.queue_wait_ms > 5000
            || self.deadline_ms == 0
            || self.deadline_ms > 60000
            || self.top_k == 0
            || self.repeat_penalty_milli == 0
            || self.temperature_milli != 100
            || self.top_k != 50
            || self.repeat_penalty_milli != 1100
        {
            return Err(ValidationError::Limit);
        }
        for text in [
            &self.auxiliary_repository,
            &self.auxiliary_revision,
            &self.artifact_repository,
            &self.artifact_revision,
            &self.artifact_name,
            &self.backend_revision,
        ] {
            if text.as_str().is_empty() {
                return Err(ValidationError::Identifier);
            }
        }
        if self.cpu_identity.as_str().is_empty() {
            return Err(ValidationError::Identifier);
        }
        if let LocalDevice::Cuda {
            uuid,
            model,
            memory_bytes,
            driver,
        } = &self.device
            && (!uuid.as_str().starts_with("GPU-")
                || model.as_str().is_empty()
                || driver.as_str().is_empty()
                || memory_bytes.get() == 0)
        {
            return Err(ValidationError::Binding);
        }
        for revision in [
            &self.auxiliary_revision,
            &self.artifact_revision,
            &self.backend_revision,
        ] {
            if revision.as_str().len() != 40
                || !revision
                    .as_str()
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(ValidationError::Identifier);
            }
        }
        if self.auxiliary_repository.as_str() != "LiquidAI/LFM3-1.7B"
            || self.auxiliary_revision.as_str() != "1da06bec6a819da10b54c67e5c90972b46d81871"
            || self.artifact_repository.as_str() != "LiquidAI/LFM3-1.7B-GGUF"
            || self.artifact_revision.as_str() != "cd0c74c575b5c416f3011b5b4c298077f26ca8c2"
            || self.backend_revision.as_str() != "fb4cdc3730dc64b366724e7637a205a8b7747499"
        {
            return Err(ValidationError::Binding);
        }
        let artifact = (
            self.artifact_name.as_str(),
            self.artifact_bytes.get(),
            self.artifact_digest.as_str(),
        );
        if artifact
            != (
                "LFM3-1.7B-Q4_K_M.gguf",
                1099627808,
                "3793e1e6e2266919d6e015287221b41100175a98eddaba92e111aed3b05c93c4",
            )
            && artifact
                != (
                    "LFM3-1.7B-QAD-Q4_0.gguf",
                    1049615680,
                    "43a7d09a0e9cba91b63eb7052488b9564569bddfd65754d2e426465e572a7452",
                )
            && artifact
                != (
                    "LFM3-1.7B-Q6_K.gguf",
                    1427638304,
                    "54e1bd947c77167823d45bf495f1c3e2d226ca7fb242f83cfa141ee3a6ab55c6",
                )
            && artifact
                != (
                    "LFM3-1.7B-Q8_0.gguf",
                    1846238496,
                    "8f6b24182258205e9e01e21f1f24a1f208c1e8115578825d202ce39976af20b6",
                )
        {
            return Err(ValidationError::Binding);
        }
        if self.embedded_tokenizer_digest != gguf_embedded_tokenizer_digest(&self.artifact_digest)?
        {
            return Err(ValidationError::Binding);
        }
        canonical_bytes(self, MAX_ENVELOPE)?;
        Ok(())
    }
    pub fn digest(&self) -> Result<Digest, ValidationError> {
        self.validate()?;
        structural_digest("local-profile/v2", &[&canonical_bytes(self, MAX_ENVELOPE)?])
    }
}

/// Inline provenance includes all output, even when parsing fails.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalObservation {
    pub schema_version: SchemaVersion,
    pub profile_digest: Digest,
    pub request_digest: Digest,
    pub input_digest: Digest,
    pub content: BoundedText<MAX_MODEL_OUTPUT>,
    pub reasoning: Option<BoundedText<MAX_MODEL_OUTPUT>>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub submission_cohort: SubmissionCohort,
    pub backend_batch: BatchProvenance,
    pub response: Option<LocalResponse>,
}
impl LocalObservation {
    pub fn validate(
        &self,
        profile: &LocalProfile,
        request_digest: &Digest,
        schema: LocalOutputSchema,
    ) -> Result<(), ValidationError> {
        profile.validate()?;
        if self.schema_version != SchemaVersion::V2
            || self.profile_digest != profile.digest()?
            || &self.request_digest != request_digest
            || self.prompt_tokens > profile.max_input_tokens
            || self.completion_tokens > profile.max_output_tokens
            || self
                .prompt_tokens
                .checked_add(self.completion_tokens)
                .is_none_or(|n| n > profile.context_tokens)
            || self
                .content
                .as_str()
                .len()
                .checked_add(self.reasoning.as_ref().map_or(0, |v| v.as_str().len()))
                .is_none_or(|n| n > MAX_MODEL_OUTPUT)
        {
            return Err(ValidationError::Binding);
        }
        if self.submission_cohort.id.as_str().is_empty()
            || self.submission_cohort.size == 0
            || self.submission_cohort.size > profile.max_sequences
            || self.submission_cohort.position >= self.submission_cohort.size
        {
            return Err(ValidationError::Binding);
        }
        if let BatchProvenance::Known { id, size, position } = &self.backend_batch
            && (id.as_str().is_empty()
                || *size == 0
                || *size > profile.max_sequences
                || position >= size)
        {
            return Err(ValidationError::Binding);
        }
        if self.response != parse_local_response(self.content.as_str().as_bytes(), schema).ok() {
            return Err(ValidationError::Binding);
        }
        canonical_bytes(self, MAX_ENVELOPE)?;
        Ok(())
    }
}

/// A bounded qualification result binds the evaluator and each case to the effective profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationCase {
    pub case_id: BoundedText<128>,
    pub repetition: u8,
    pub session_id: Digest,
    pub dispatch_id: Digest,
    pub request_digest: Digest,
    pub action_digest: Digest,
    pub submission_cohort: SubmissionCohort,
    pub backend_batch: BatchProvenance,
    pub input_digest: Digest,
    pub expected_digest: Digest,
    pub output_schema: LocalOutputSchema,
    pub response: Option<BoundedText<MAX_MODEL_OUTPUT>>,
    pub reasoning: Option<BoundedText<MAX_MODEL_OUTPUT>>,
    pub latency_ms: Option<SqlU64>,
    /// Backend-reported durations, not sampled resource phase boundaries.
    pub prompt_time_us: Option<SqlU64>,
    pub completion_time_us: Option<SqlU64>,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    /// Digests observed from the worker tokenizer; None means unavailable.
    pub token_ids_digest: Option<Digest>,
    pub rendered_digest: Option<Digest>,
    pub response_digest: Option<Digest>,
    pub passed: bool,
}
/// Inputs used to independently reconstruct qualification identities.
pub struct QualificationBinding<'a> {
    pub session: &'a Digest,
    pub case_id: &'a str,
    pub repetition: u8,
    pub warmup: bool,
    pub profile: &'a Digest,
    pub input: &'a Digest,
    pub role: LocalRole,
    pub schema: LocalOutputSchema,
}
pub fn qualification_bindings(
    binding: &QualificationBinding<'_>,
) -> Result<(Digest, Digest, Digest), ValidationError> {
    if binding.case_id.is_empty() || binding.case_id.len() > 128 || binding.repetition >= 3 {
        return Err(ValidationError::Binding);
    }
    let dispatch = structural_digest(
        "qualification-dispatch/v1",
        &[
            &binding.session.bytes(),
            binding.case_id.as_bytes(),
            &[binding.repetition, u8::from(binding.warmup)],
        ],
    )?;
    let role_bytes = canonical_bytes(&(binding.role, binding.schema), MAX_ENVELOPE)?;
    let request = structural_digest(
        "qualification-request/v1",
        &[
            &binding.profile.bytes(),
            &binding.input.bytes(),
            &role_bytes,
        ],
    )?;
    let action = structural_digest(
        "qualification-action/v1",
        &[
            &binding.session.bytes(),
            &dispatch.bytes(),
            &request.bytes(),
        ],
    )?;
    Ok((dispatch, request, action))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementPhase {
    WholeRun,
    /// Worker startup and unscored warmup observations.
    StartupWarmup,
    /// Sampled lifetime of one worker, from startup through cleanup.
    WorkerLifetime,
    /// Host PID only; excludes child workers and telemetry subprocesses.
    HostLifetime,
    Unattributed,
    Load,
    Prefill,
    Decode,
    Capture,
    Cleanup,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementSource {
    Procfs,
    NvidiaSmi,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseMeasurement {
    /// The qualification session whose worker was sampled.
    pub session_id: Digest,
    pub phase: MeasurementPhase,
    pub repetition: u8,
    pub cpu_source: Option<MeasurementSource>,
    pub gpu_source: Option<MeasurementSource>,
    pub cpu_sample_interval_ms: Option<u32>,
    /// Resolution of the sampled process CPU counter, obtained from CLK_TCK.
    pub cpu_clock_ticks_per_second: Option<u32>,
    pub gpu_sample_interval_ms: Option<u32>,
    /// None means unavailable, never a zero measurement.
    pub peak_rss_bytes: Option<SqlU64>,
    pub peak_pss_bytes: Option<SqlU64>,
    pub peak_gpu_bytes: Option<SqlU64>,
    pub min_gpu_free_bytes: Option<SqlU64>,
    pub cpu_time_ms: Option<SqlU64>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationReport {
    pub schema_version: SchemaVersion,
    pub evaluator_digest: Digest,
    pub corpus_digest: Digest,
    pub profile: LocalProfile,
    pub worker_build_digest: Digest,
    pub cold: bool,
    pub measurements: Vec<PhaseMeasurement>,
    /// Host PID only; never combined with worker process observations.
    pub host_measurements: Vec<PhaseMeasurement>,
    /// End-to-end operational elapsed, including startup and warmup.
    pub operational_elapsed_ms: Option<SqlU64>,
    /// Scored trial interval.
    pub elapsed_ms: Option<SqlU64>,
    pub accepted_tasks: u32,
    pub p50_latency_ms: Option<SqlU64>,
    pub p95_latency_ms: Option<SqlU64>,
    /// Separately retained unscored warmup observations, in worker-start order.
    pub warmups: Vec<QualificationCase>,
    pub cases: Vec<QualificationCase>,
}
impl QualificationReport {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.profile.validate()?;
        if self.schema_version != SchemaVersion::V2
            || self.cases.is_empty()
            || self.cases.len() > 144
            || self.measurements.len() > 21
            || self.host_measurements.len() > 3
            || self.warmups.len() > 2
            || (self.cold && !self.warmups.is_empty())
        {
            return Err(ValidationError::Limit);
        }
        if self.host_measurements.iter().any(|m| {
            m.phase != MeasurementPhase::HostLifetime
                || m.peak_gpu_bytes.is_some()
                || m.min_gpu_free_bytes.is_some()
        }) || self
            .measurements
            .iter()
            .any(|m| m.phase == MeasurementPhase::HostLifetime)
        {
            return Err(ValidationError::Binding);
        }
        for measurement in self.measurements.iter().chain(&self.host_measurements) {
            if measurement.repetition >= 3
                || ((measurement.peak_rss_bytes.is_some()
                    || measurement.peak_pss_bytes.is_some()
                    || measurement.cpu_time_ms.is_some())
                    && (measurement.cpu_source != Some(MeasurementSource::Procfs)
                        || measurement.cpu_sample_interval_ms.is_none()))
                || ((measurement.peak_gpu_bytes.is_some()
                    || measurement.min_gpu_free_bytes.is_some())
                    && (measurement.gpu_source != Some(MeasurementSource::NvidiaSmi)
                        || measurement.gpu_sample_interval_ms.is_none()))
                || (measurement.cpu_time_ms.is_some()
                    && !measurement
                        .cpu_clock_ticks_per_second
                        .is_some_and(|ticks| (1..=1_000_000).contains(&ticks)))
                || measurement
                    .cpu_clock_ticks_per_second
                    .is_some_and(|ticks| ticks == 0 || ticks > 1_000_000)
                || (measurement.cpu_sample_interval_ms == Some(0)
                    || measurement.gpu_sample_interval_ms == Some(0))
            {
                return Err(ValidationError::Binding);
            }
        }
        let mut ids = std::collections::BTreeSet::new();
        for case in &self.cases {
            if case.case_id.as_str().is_empty()
                || case.repetition >= 3
                || !ids.insert((case.case_id.as_str(), case.repetition))
                || (case.passed && (case.response_digest.is_none() || case.response.is_none()))
            {
                return Err(ValidationError::Binding);
            }
        }
        for case in &self.warmups {
            if case.case_id.as_str() != "calibration-sum-02" {
                return Err(ValidationError::Binding);
            }
        }
        let mut dispatches = std::collections::BTreeSet::new();
        let mut cohorts = std::collections::BTreeMap::<
            (&Digest, &str),
            (u16, std::collections::BTreeSet<u16>, Option<&str>),
        >::new();
        for (case, warmup) in self
            .cases
            .iter()
            .map(|case| (case, false))
            .chain(self.warmups.iter().map(|case| (case, true)))
        {
            let role = match case.output_schema {
                LocalOutputSchema::Sum => LocalRole::Solver,
                LocalOutputSchema::ToolSelection => LocalRole::ToolSelector,
                LocalOutputSchema::Text => LocalRole::Reasoner,
            };
            let expected = qualification_bindings(&QualificationBinding {
                session: &case.session_id,
                case_id: case.case_id.as_str(),
                repetition: case.repetition,
                warmup,
                profile: &self.profile.digest()?,
                input: &case.input_digest,
                role,
                schema: case.output_schema,
            })?;
            if expected
                != (
                    case.dispatch_id.clone(),
                    case.request_digest.clone(),
                    case.action_digest.clone(),
                )
                || !dispatches.insert(&case.dispatch_id)
            {
                return Err(ValidationError::Binding);
            }
            if case.submission_cohort.id.as_str().is_empty()
                || case.submission_cohort.size == 0
                || case.submission_cohort.size > self.profile.max_sequences
                || case.submission_cohort.position >= case.submission_cohort.size
            {
                return Err(ValidationError::Binding);
            }
            let group = cohorts
                .entry((&case.session_id, case.submission_cohort.id.as_str()))
                .or_insert_with(|| {
                    (
                        case.submission_cohort.size,
                        std::collections::BTreeSet::new(),
                        None,
                    )
                });
            if group.0 != case.submission_cohort.size
                || !group.1.insert(case.submission_cohort.position)
            {
                return Err(ValidationError::Binding);
            }
            if case.submission_cohort.position == 0 {
                group.2 = Some(case.dispatch_id.as_str());
            }
            if let BatchProvenance::Known { id, size, position } = &case.backend_batch
                && (id.as_str().is_empty()
                    || *size == 0
                    || *size > self.profile.max_sequences
                    || position >= size)
            {
                return Err(ValidationError::Binding);
            }
            if case.passed && (case.response_digest.is_none() || case.response.is_none()) {
                return Err(ValidationError::Binding);
            }
            if case
                .response
                .as_ref()
                .map_or(0, |v| v.as_str().len())
                .checked_add(case.reasoning.as_ref().map_or(0, |v| v.as_str().len()))
                .is_none_or(|n| n > MAX_MODEL_OUTPUT)
            {
                return Err(ValidationError::Limit);
            }
            canonical_bytes(case, MAX_ENVELOPE)?;
            if case
                .response
                .as_ref()
                .map(|value| crate::content_digest(value.as_str().as_bytes()))
                != case.response_digest
            {
                return Err(ValidationError::Binding);
            }
        }
        if cohorts.iter().any(|((_, id), (size, positions, first))| {
            positions.len() != usize::from(*size) || *first != Some(*id)
        }) {
            return Err(ValidationError::Binding);
        }
        canonical_bytes(self, MAX_QUALIFICATION_REPORT)?;
        Ok(())
    }
}
pub fn parse_qualification_report(bytes: &[u8]) -> Result<QualificationReport, ValidationError> {
    if bytes.len() > MAX_QUALIFICATION_REPORT {
        return Err(ValidationError::Limit);
    }
    let report: QualificationReport =
        serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))?;
    report.validate()?;
    Ok(report)
}

/// Worker protocol messages do not carry the consumed authorization token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerRequest {
    Generate {
        dispatch_id: Digest,
        request_digest: Digest,
        task_id: crate::TaskId,
        principal_id: crate::PrincipalId,
        action_digest: Digest,
        input_digest: Digest,
        role: LocalRole,
        output_schema: LocalOutputSchema,
        context: BoundedText<MAX_LOCAL_CONTEXT>,
        profile: Box<LocalProfile>,
        artifact_path: BoundedText<4096>,
        cohort: Box<SubmissionCohort>,
    },
    Cancel {
        dispatch_id: Digest,
    },
    Shutdown,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerReply {
    /// Worker-side request arrival while these SDK streams await a terminal response.
    Arrived {
        dispatch_id: Digest,
        in_flight: Vec<Digest>,
    },
    Loading {
        dispatch_id: Digest,
    },
    Loaded {
        dispatch_id: Digest,
    },
    /// Digests bind SDK-rendered prompt bytes and big-endian u32 token IDs.
    Tokenized {
        dispatch_id: Digest,
        token_count: u32,
        token_ids_digest: Digest,
        rendered_digest: Digest,
    },
    Started {
        dispatch_id: Digest,
    },
    Delta {
        dispatch_id: Digest,
        sequence: u32,
        text: BoundedText<2048>,
        reasoning: bool,
    },
    Completed {
        dispatch_id: Digest,
        observation: Box<LocalObservation>,
        prompt_time_us: Option<SqlU64>,
        completion_time_us: Option<SqlU64>,
    },
    Rejected {
        dispatch_id: Digest,
        diagnostic: BoundedText<{ crate::MAX_TEXT }>,
    },
    CancelledBeforeStart {
        dispatch_id: Digest,
    },
}
pub fn parse_worker_request(bytes: &[u8]) -> Result<WorkerRequest, ValidationError> {
    if bytes.len() > MAX_LOCAL_REQUEST_FRAME {
        return Err(ValidationError::Limit);
    }
    let value: WorkerRequest =
        serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))?;
    value.validate()?;
    Ok(value)
}
pub fn worker_request_bytes(value: &WorkerRequest) -> Result<Vec<u8>, ValidationError> {
    value.validate()?;
    canonical_bytes(value, MAX_LOCAL_REQUEST_FRAME)
}
/// Contextual observation validation still requires the admitted request and profile.
pub fn parse_worker_reply(bytes: &[u8]) -> Result<WorkerReply, ValidationError> {
    let value: WorkerReply = crate::parse_envelope(bytes)?;
    if let WorkerReply::Arrived {
        dispatch_id,
        in_flight,
    } = &value
    {
        let unique: std::collections::BTreeSet<_> = in_flight.iter().collect();
        if in_flight.len() > 8 || unique.len() != in_flight.len() || in_flight.contains(dispatch_id)
        {
            return Err(ValidationError::Binding);
        }
    }
    canonical_bytes(&value, MAX_ENVELOPE)?;
    Ok(value)
}
impl WorkerRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if let Self::Generate {
            context,
            input_digest,
            profile,
            artifact_path,
            cohort,
            role,
            output_schema,
            ..
        } = self
        {
            profile.validate()?;
            if crate::content_digest(context.as_str().as_bytes()) != *input_digest
                || !artifact_path.as_str().starts_with('/')
                || artifact_path.as_str().contains('\0')
                || cohort.id.as_str().is_empty()
                || cohort.size == 0
                || cohort.size > profile.max_sequences
                || cohort.position >= cohort.size
                || !matches!(
                    (role, output_schema),
                    (LocalRole::Solver, LocalOutputSchema::Sum)
                        | (LocalRole::ToolSelector, LocalOutputSchema::ToolSelection)
                        | (LocalRole::Reasoner, LocalOutputSchema::Text)
                )
                || (*role == LocalRole::Reasoner
                    && profile.prompt_recipe_digest.as_str() != LOCAL_TEAM_RECIPE_SHA)
            {
                return Err(ValidationError::Binding);
            }
        }
        canonical_bytes(self, MAX_LOCAL_REQUEST_FRAME)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorpusSplit {
    Calibration,
    Heldout,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorStage {
    NeedInput,
    HaveInput,
    HaveCandidate,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectorInput {
    pub stage: SelectorStage,
    pub note: BoundedText<1024>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrozenCase {
    Solver {
        id: BoundedText<128>,
        input: crate::SumInput,
        expected: SumResponse,
    },
    ToolSelector {
        id: BoundedText<128>,
        input: SelectorInput,
        expected: ToolSelectionResponse,
    },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenCorpus {
    pub schema_version: SchemaVersion,
    pub evaluator: BoundedText<128>,
    pub prompt_recipe_sha256: Digest,
    pub split: CorpusSplit,
    pub cases: Vec<FrozenCase>,
}
pub fn parse_frozen_corpus(bytes: &[u8]) -> Result<FrozenCorpus, ValidationError> {
    if bytes.len() > MAX_ENVELOPE {
        return Err(ValidationError::Limit);
    }
    let corpus: FrozenCorpus =
        serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))?;
    if corpus.schema_version != SchemaVersion::V1
        || corpus.evaluator.as_str() != "metron-local-inference-eval/v1"
    {
        return Err(ValidationError::Version);
    }
    let count = match corpus.split {
        CorpusSplit::Calibration => 32,
        CorpusSplit::Heldout => 16,
    };
    if corpus.cases.len() != count {
        return Err(ValidationError::Limit);
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut solver = 0;
    for case in &corpus.cases {
        let id = match case {
            FrozenCase::Solver {
                id,
                input,
                expected,
            } => {
                solver += 1;
                let sum = input
                    .values
                    .iter()
                    .try_fold(0i64, |sum, value| sum.checked_add(*value))
                    .ok_or(ValidationError::Limit)?;
                if input.values.len() != expected.count as usize || sum != expected.sum {
                    return Err(ValidationError::Binding);
                }
                id
            }
            FrozenCase::ToolSelector {
                id,
                input,
                expected,
            } => {
                let actual = match input.stage {
                    SelectorStage::NeedInput => ToolChoice::ReadFixture,
                    SelectorStage::HaveInput => ToolChoice::RequestModel,
                    SelectorStage::HaveCandidate => ToolChoice::PublishArtifact,
                };
                if expected.tool != actual {
                    return Err(ValidationError::Binding);
                }
                id
            }
        };
        if id.as_str().is_empty() || !ids.insert(id.as_str()) {
            return Err(ValidationError::Binding);
        }
    }
    if solver * 2 != count {
        return Err(ValidationError::Binding);
    }
    Ok(corpus)
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptSampling {
    pub temperature: f64,
    pub top_k: u16,
    pub repetition_penalty: f64,
    pub enable_thinking: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptRecipe {
    pub schema_version: SchemaVersion,
    pub evaluator: BoundedText<128>,
    pub solver_system: BoundedText<1024>,
    pub selector_system: BoundedText<1024>,
    pub sampling: PromptSampling,
    pub max_total_tokens: u32,
    pub max_generated_tokens: u32,
    pub byte_output_limit: u32,
}
pub fn parse_prompt_recipe(bytes: &[u8]) -> Result<PromptRecipe, ValidationError> {
    if bytes.len() > MAX_ENVELOPE {
        return Err(ValidationError::Limit);
    }
    let recipe: PromptRecipe =
        serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))?;
    if recipe.schema_version != SchemaVersion::V1
        || recipe.evaluator.as_str() != "metron-local-inference-eval/v1"
        || recipe.sampling.temperature != 0.1
        || recipe.sampling.top_k != 50
        || recipe.sampling.repetition_penalty != 1.1
        || recipe.sampling.enable_thinking
        || recipe.max_total_tokens != 2048
        || recipe.max_generated_tokens != 512
        || recipe.byte_output_limit != 16384
    {
        return Err(ValidationError::Binding);
    }
    Ok(recipe)
}

pub const MAX_SELECTED_PROFILE: usize = 20 * 1024 * 1024;
/// Bind both frozen split files without making their concatenation ambiguous.
pub fn qualification_corpora_digest(
    calibration: &Digest,
    heldout: &Digest,
) -> Result<Digest, ValidationError> {
    structural_digest(
        "qualification-corpora/v1",
        &[&calibration.bytes(), &heldout.bytes()],
    )
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationFreeze {
    pub schema_version: SchemaVersion,
    pub profile_digest: Digest,
    pub recipe_digest: Digest,
    pub calibration_digest: Digest,
    pub heldout_digest: Digest,
    pub calibration_cases_digest: Digest,
    pub calibration_warmup_digest: Digest,
    pub cold_report_digest: Digest,
}
impl CalibrationFreeze {
    pub fn digest(&self) -> Result<Digest, ValidationError> {
        if self.schema_version != SchemaVersion::V2 {
            return Err(ValidationError::Version);
        }
        structural_digest(
            "calibration-freeze/v2",
            &[&canonical_bytes(self, MAX_ENVELOPE)?],
        )
    }
}
pub fn qualification_report_digest(
    report: &QualificationReport,
) -> Result<Digest, ValidationError> {
    report.validate()?;
    structural_digest(
        "qualification-report/v2",
        &[&canonical_bytes(report, MAX_QUALIFICATION_REPORT)?],
    )
}
pub fn qualification_calibration_cases_digest(
    report: &QualificationReport,
) -> Result<Digest, ValidationError> {
    report.validate()?;
    let mut cases: Vec<_> = report
        .cases
        .iter()
        .filter(|c| c.case_id.as_str().starts_with("calibration-"))
        .collect();
    if cases.len() != 96 {
        return Err(ValidationError::Binding);
    }
    cases.sort_by(|a, b| {
        (a.case_id.as_str(), a.repetition).cmp(&(b.case_id.as_str(), b.repetition))
    });
    structural_digest(
        "qualification-calibration-cases/v2",
        &[&canonical_bytes(&cases, MAX_QUALIFICATION_REPORT)?],
    )
}
/// Only the first, pre-calibration warmup is fixed by the freeze.
pub fn qualification_calibration_warmup_digest(
    report: &QualificationReport,
) -> Result<Digest, ValidationError> {
    report.validate()?;
    let warmup = report.warmups.first().ok_or(ValidationError::Binding)?;
    structural_digest(
        "qualification-calibration-warmup/v2",
        &[&canonical_bytes(warmup, MAX_ENVELOPE)?],
    )
}
/// Untrusted selection proposal. The host must independently re-evaluate all cases.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedProfile {
    pub schema_version: SchemaVersion,
    pub profile: LocalProfile,
    pub recipe_digest: Digest,
    pub calibration_digest: Digest,
    pub heldout_digest: Digest,
    pub calibration_freeze: CalibrationFreeze,
    pub cold_report: QualificationReport,
    pub warm_report: QualificationReport,
}
pub fn parse_selected_profile(bytes: &[u8]) -> Result<SelectedProfile, ValidationError> {
    if bytes.len() > MAX_SELECTED_PROFILE {
        return Err(ValidationError::Limit);
    }
    let selection: SelectedProfile =
        serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))?;
    selection.cold_report.validate()?;
    selection.warm_report.validate()?;
    if selection.schema_version != SchemaVersion::V2
        || selection.profile != selection.cold_report.profile
        || selection.profile != selection.warm_report.profile
        || selection.recipe_digest != selection.profile.prompt_recipe_digest
        || !selection.cold_report.cold
        || selection.warm_report.cold
        || selection.cold_report.cases.len() != 6
        || selection.warm_report.cases.len() != 144
        || selection.warm_report.warmups.is_empty()
        || selection.cold_report.worker_build_digest != selection.warm_report.worker_build_digest
        || selection.cold_report.corpus_digest != selection.warm_report.corpus_digest
        || selection.warm_report.corpus_digest
            != qualification_corpora_digest(
                &selection.calibration_digest,
                &selection.heldout_digest,
            )?
        || selection.cold_report.evaluator_digest != selection.warm_report.evaluator_digest
        || selection.warm_report.evaluator_digest
            != crate::content_digest(b"metron-local-inference-eval/v1")
        || selection.calibration_freeze.schema_version != SchemaVersion::V2
        || selection.calibration_freeze.profile_digest != selection.profile.digest()?
        || selection.calibration_freeze.recipe_digest != selection.recipe_digest
        || selection.calibration_freeze.calibration_digest != selection.calibration_digest
        || selection.calibration_freeze.heldout_digest != selection.heldout_digest
        || selection.calibration_freeze.calibration_cases_digest
            != qualification_calibration_cases_digest(&selection.warm_report)?
        || selection.calibration_freeze.calibration_warmup_digest
            != qualification_calibration_warmup_digest(&selection.warm_report)?
        || selection.calibration_freeze.cold_report_digest
            != qualification_report_digest(&selection.cold_report)?
    {
        return Err(ValidationError::Binding);
    }
    canonical_bytes(&selection, MAX_SELECTED_PROFILE)?;
    Ok(selection)
}
