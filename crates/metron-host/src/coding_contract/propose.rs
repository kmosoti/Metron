//! Host-private contract proposal protocol (`propose`): derives a compliant `LOCAL_TEAM_RECIPE_SHA`
//! profile derivative, executes `<= 3` bounded preparation stages (`Proposal`, `Revision`, `Review`)
//! using one-shot `ComparisonSession` permits, preserves raw responses/unknown holds without
//! auto-retry, and compiles host-authenticated `ContractDraftV1` without granting model authority.

use super::io::{
    create_external_private_dir, ensure_dir_outside, open_external_nofollow, read_external_json,
    unlink_pinned_leaf, write_external_new_json,
};
use super::types::*;
use super::validation::{HostAuthContext, validate_against_auth_with_mode};
use metron_inference_local::{
    artifact::{Artifact, verify_artifact},
    comparison::{ComparisonCase, ComparisonResult, ComparisonSession},
    driver::{LocalInferenceDriver, QualificationOutcome},
};
#[cfg(test)]
use metron_protocol::{
    ActualUsage, DecimalUsage, ProducerReceiptId, ReceiptData, ReceiptOutput, SchemaVersion,
    UsageVector,
};
use metron_protocol::{
    BoundedText, Digest, LOCAL_TEAM_RECIPE_SHA, LocalOutputSchema, LocalProfile, LocalRole,
    MAX_INTERNAL, MAX_LOCAL_CONTEXT, PrincipalId, Receipt, TaskId, coding::WorkspacePath,
    content_digest, gguf_embedded_tokenizer_digest, structural_digest,
};
#[cfg(test)]
use std::collections::VecDeque;
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(test)]
pub(crate) const UNCERTAIN_TEST_SENTINEL: &str = "__UNCERTAIN__";

#[derive(Clone, Debug)]
pub(crate) struct ProposeParams {
    pub contract_id: String,
    pub workflow: ContractWorkflow,
    pub selected_binary: String,
    pub max_stages: u8,
    pub architecture_scaffold: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ProposeOutcome {
    pub outcome: &'static str,
    pub ledger: ProposeSessionLedgerV1,
    pub draft: Option<ContractDraftV1>,
    pub validation: Option<ContractValidationReportV1>,
    pub bundle: Option<ContractBundleV1>,
}

/// Derive a compliant `LOCAL_TEAM_RECIPE_SHA` `LocalProfile` for `Reasoner`/`Text` contract
/// preparation while preserving the source profile's model artifact, tokenizer, backend,
/// device, sampler parameters, context length (`<= 8192`), input budget, and 512 output cap.
pub(crate) fn derive_team_text_profile(
    coding_profile: &LocalProfile,
) -> Result<(LocalProfile, Digest, Digest), String> {
    coding_profile
        .validate()
        .map_err(|e| format!("source coding profile invalid: {e:?}"))?;
    if coding_profile.context_tokens > 8192 {
        return Err(format!(
            "source profile context_tokens {} exceeds 8192 ceiling",
            coding_profile.context_tokens
        ));
    }
    if coding_profile.max_output_tokens != 512 {
        return Err(format!(
            "source profile max_output_tokens must remain 512, found {}",
            coding_profile.max_output_tokens
        ));
    }
    let source_digest = coding_profile
        .digest()
        .map_err(|e| format!("source profile digest: {e:?}"))?;

    let mut derived = coding_profile.clone();
    derived.prompt_recipe_digest =
        Digest::new(LOCAL_TEAM_RECIPE_SHA).map_err(|e| format!("team recipe digest: {e:?}"))?;
    derived.embedded_tokenizer_digest = gguf_embedded_tokenizer_digest(&derived.artifact_digest)
        .map_err(|e| format!("team tokenizer digest: {e:?}"))?;
    // Non-coding recipes validate deadline_ms <= 60_000 while keeping context/input/output & sampler.
    derived.deadline_ms = coding_profile.deadline_ms.min(60_000);
    derived
        .validate()
        .map_err(|e| format!("derived team profile invalid: {e:?}"))?;
    let team_digest = derived
        .digest()
        .map_err(|e| format!("team profile digest: {e:?}"))?;
    if team_digest == source_digest {
        return Err(
            "derived TEAM profile must have a separate profile digest from coding profile".into(),
        );
    }
    Ok((derived, source_digest, team_digest))
}

pub(crate) fn resolve_pinned_artifact(profile: &LocalProfile) -> Result<Artifact, String> {
    let want = profile.artifact_digest.as_str();
    for candidate in [
        Artifact::Control,
        Artifact::Challenger,
        Artifact::Q6K,
        Artifact::Q8Zero,
    ] {
        if candidate.sha256() == want {
            return Ok(candidate);
        }
    }
    Err(format!(
        "profile artifact_digest {want} does not match any pinned local artifact"
    ))
}

fn build_stage_prompt(
    params: &ProposeParams,
    auth: &HostAuthContext,
    stage: ProposeStageNameV1,
    prior_raw: Option<&str>,
    prior_diagnostics: &[String],
) -> Result<BoundedText<MAX_LOCAL_CONTEXT>, String> {
    let workflow = match params.workflow {
        ContractWorkflow::New => "new",
        ContractWorkflow::Existing => "existing",
    };
    let mode_note = if params.architecture_scaffold {
        "architecture_scaffold=on: identify workspace modules, cargo dependencies, preserved files, allowed edits, and exact interface obligations cleanly."
    } else {
        "architecture_scaffold=off: extract exact behavioral obligations, assumptions, and unresolved ambiguities faithfully."
    };

    let harness_preamble = if params.selected_binary == "metron-case" {
        std::borrow::Cow::Borrowed(PUBLIC_HARNESS_PREAMBLE)
    } else {
        std::borrow::Cow::Owned(format!(
            "Final checks use offline+locked Cargo metadata/test/build. Prepare Cargo.lock with normal development tools. Selected CLI: {}.",
            params.selected_binary
        ))
    };

    let mut prompt = format!(
        "Role: Metron contract preparation assistant ({mode_note}).\n\
         Public harness environment: {harness_preamble}\n\
         Output ONLY one strict JSON object matching:\n\
         {{\"intended_delta\":[string],\"approved_compatibility\":[string],\"allowed_edit_paths\":[string],\"preserved_files\":[string],\"mandatory_obligations\":[{{\"id\":string,\"clause\":string,\"checker\":string|null}}],\"assumptions\":[{{\"id\":string,\"text\":string,\"material\":bool}}],\"unknowns\":[{{\"id\":string,\"question\":string,\"material\":bool}}],\"resolutions\":[{{\"id\":string,\"unknown_id\":string,\"answer\":string}}]}}\n\
         Rules:\n\
         - Optional nonmaterial assumption id `prepared-guidance` contains implementation guidance of 1..=32 UTF-8 bytes; never replace original requirements with guidance.\n\
         - Declare EVERY unresolved material ambiguity, contradiction, or unspecified edge case in `unknowns`.\n\
         - Never invent requirements not backed by request/clarifications/baseline; mark inferences in `assumptions`.\n\
         - Allowed `checker` values (or null): \"snapshot-local-path-v1\", \"cargo-offline-locked-v1\", \"cargo-test-offline-locked-v1\", \"selected-cli-vectors-v1\", \"preserved-file-v1\", \"forbidden-file-v1\", \"fixed-file-v1\", \"interface-signature-v1\".\n\
         - Keep clause strings concise and bounded; workflow={workflow}; selected_binary={}.\n\
         Original request:\n{}\n",
        params.selected_binary,
        auth.raw_request.trim()
    );

    if let Some(ws) = &auth.workspace_snapshot
        && !ws.is_empty()
    {
        prompt.push_str("Observed baseline files (path:sha256_prefix):\n");
        for (path, digest) in ws {
            prompt.push_str(&format!("- {path}: {}\n", &digest.as_str()[..12]));
        }
    }
    for (id, src) in &auth.sources_by_id {
        if matches!(
            src.kind,
            SourceKind::ClarificationAnswer | SourceKind::TrustedSpec
        ) && let Some(text) = &src.utf8_text
        {
            prompt.push_str(&format!(
                "Host-approved {} ({id}):\n{}\n",
                match src.kind {
                    SourceKind::ClarificationAnswer => "clarification",
                    _ => "trusted_spec",
                },
                text.trim()
            ));
        }
    }
    match stage {
        ProposeStageNameV1::Proposal => {}
        ProposeStageNameV1::Revision => {
            prompt.push_str("Stage: REVISION. Fix validation errors or JSON issues below.\n");
            if let Some(prev) = prior_raw {
                prompt.push_str("Previous response snippet:\n");
                let cap = prev.floor_char_boundary(prev.len().min(600));
                prompt.push_str(&prev[..cap]);
                prompt.push('\n');
            }
            for diag in prior_diagnostics.iter().take(6) {
                prompt.push_str(&format!("- {diag}\n"));
            }
        }
        ProposeStageNameV1::Review => {
            prompt.push_str(
                "Stage: REVIEW. Verify all mandatory clauses and unresolved unknowns carefully.\n",
            );
            if let Some(prev) = prior_raw {
                let cap = prev.floor_char_boundary(prev.len().min(600));
                prompt.push_str(&prev[..cap]);
                prompt.push('\n');
            }
        }
    }
    if prompt.len() > MAX_LOCAL_CONTEXT {
        return Err(format!(
            "contract propose prompt length {} exceeds MAX_LOCAL_CONTEXT ({MAX_LOCAL_CONTEXT})",
            prompt.len()
        ));
    }
    BoundedText::new(prompt).map_err(|e| format!("propose prompt invalid: {e:?}"))
}

fn strip_single_json_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(after_open) = trimmed.strip_prefix("```json")
        && let Some(inner) = after_open.strip_suffix("```")
    {
        return inner.trim();
    }
    if let Some(after_open) = trimmed.strip_prefix("```")
        && let Some(inner) = after_open.strip_suffix("```")
    {
        return inner.trim();
    }
    trimmed
}

/// Parse unconstrained text model output into `CompactProposalDtoV1` strictly on host side.
pub(crate) fn parse_compact_proposal_dto(raw: &str) -> Result<CompactProposalDtoV1, String> {
    let json_text = strip_single_json_fence(raw);
    if json_text.is_empty() {
        return Err("empty model response (expected compact contract JSON object)".into());
    }
    let dto = serde_json::from_str::<CompactProposalDtoV1>(json_text)
        .map_err(|e| format!("compact proposal JSON parse error: {e}"))?;
    for assumption in &dto.assumptions {
        if assumption.id.as_str() == "prepared-guidance"
            && (assumption.text.as_str().len() > 32 || assumption.material)
        {
            return Err("prepared guidance must be nonmaterial and at most 32 UTF-8 bytes".into());
        }
    }
    Ok(dto)
}

fn build_draft_from_dto(
    params: &ProposeParams,
    auth: &HostAuthContext,
    dto: &CompactProposalDtoV1,
    baseline_files_raw: &[(WorkspacePath, Digest)],
) -> Result<ContractDraftV1, String> {
    let mut sources = Vec::new();
    for (id, src) in &auth.sources_by_id {
        sources.push(ContractSourceEntryV1 {
            id: BoundedText::new(id.clone()).map_err(|e| format!("source id: {e:?}"))?,
            claimed_kind: src.kind,
            content_sha256: src.sha256.clone(),
            label: BoundedText::new(
                &src.label[..src.label.floor_char_boundary(src.label.len().min(256))],
            )
            .map_err(|e| format!("source label: {e:?}"))?,
            host_kind: Some(src.kind),
            host_authenticated: true,
        });
    }
    if sources.is_empty() {
        return Err("propose requires at least one host-authenticated source entry".into());
    }

    let clarification_source_id = auth
        .sources_by_id
        .iter()
        .find(|(_, s)| matches!(s.kind, SourceKind::ClarificationAnswer))
        .or_else(|| {
            auth.sources_by_id
                .iter()
                .find(|(_, s)| matches!(s.kind, SourceKind::TrustedSpec))
        })
        .map(|(k, _)| k.clone());

    let default_source_ref = BoundedText::new("src-request").map_err(|e| format!("{e:?}"))?;
    let mut mandatory_obligations = Vec::with_capacity(dto.mandatory_obligations.len());
    for ob in &dto.mandatory_obligations {
        let checker_str = ob
            .checker
            .as_ref()
            .map(|c| c.as_str())
            .unwrap_or(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1);
        mandatory_obligations.push(ContractObligationV1 {
            id: ob.id.clone(),
            clause: ob.clause.clone(),
            checker: BoundedText::new(checker_str).map_err(|e| format!("checker: {e:?}"))?,
            source_ref: default_source_ref.clone(),
            approved_exception: false,
        });
    }
    // Preserve the complete host-approved answer independently of model summaries.
    for (source_id, source) in &auth.sources_by_id {
        if source.kind == SourceKind::ClarificationAnswer
            && let Some(text) = &source.utf8_text
        {
            for (index, clause) in split_text_lossless_clauses(text, 256, MAX_REQUIREMENTS)?
                .into_iter()
                .enumerate()
            {
                mandatory_obligations.push(ContractObligationV1 {
                    id: BoundedText::new(format!("clar-r{}", index + 1))
                        .map_err(|e| format!("{e:?}"))?,
                    clause: BoundedText::new(clause).map_err(|e| format!("{e:?}"))?,
                    checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1)
                        .map_err(|e| format!("{e:?}"))?,
                    source_ref: BoundedText::new(source_id.clone())
                        .map_err(|e| format!("{e:?}"))?,
                    approved_exception: false,
                });
            }
        }
    }
    let uses_cli_checker = mandatory_obligations
        .iter()
        .any(|ob| ob.checker.as_str() == CHECKER_CLI_SELECTED_VECTORS_V1);

    let mut assumptions = Vec::with_capacity(dto.assumptions.len());
    for a in &dto.assumptions {
        assumptions.push(ContractAssumptionV1 {
            id: a.id.clone(),
            text: a.text.clone(),
            source_ref: default_source_ref.clone(),
            material: a.material,
            approved: false,
        });
    }

    let mut resolution_by_unknown: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut resolutions = Vec::new();
    for r in &dto.resolutions {
        if let Some(clar_id) = &clarification_source_id {
            resolution_by_unknown
                .insert(r.unknown_id.as_str().to_string(), r.id.as_str().to_string());
            resolutions.push(ContractResolutionV1 {
                id: r.id.clone(),
                unknown_id: r.unknown_id.clone(),
                answer: r.answer.clone(),
                source_ref: BoundedText::new(clar_id.clone())
                    .map_err(|e| format!("clar source: {e:?}"))?,
                approved: false,
            });
        }
    }

    let mut unknowns = Vec::with_capacity(dto.unknowns.len());
    for u in &dto.unknowns {
        let res_id = resolution_by_unknown
            .get(u.id.as_str())
            .map(|id| BoundedText::new(id.clone()))
            .transpose()
            .map_err(|e| format!("resolution id: {e:?}"))?;
        unknowns.push(ContractUnknownV1 {
            id: u.id.clone(),
            question: u.question.clone(),
            material: u.material,
            resolved_by: res_id,
            blocking: true,
        });
    }

    let baseline_entries: Vec<BaselineFileRefV1> = baseline_files_raw
        .iter()
        .map(|(path, sha256)| BaselineFileRefV1 {
            path: path.clone(),
            sha256: sha256.clone(),
            preserved: dto.preserved_files.contains(path),
        })
        .collect();

    Ok(ContractDraftV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        contract_id: BoundedText::new(params.contract_id.clone())
            .map_err(|e| format!("contract_id: {e:?}"))?,
        workflow: params.workflow,
        raw_request: BoundedText::new(auth.raw_request.clone())
            .map_err(|e| format!("raw_request: {e:?}"))?,
        raw_request_sha256: auth.raw_request_sha256.clone(),
        sources,
        baseline: ContractBaselineV1 {
            workspace_label: Some(
                BoundedText::new(if params.workflow == ContractWorkflow::Existing {
                    "observed-workspace"
                } else {
                    "empty-new"
                })
                .map_err(|e| format!("{e:?}"))?,
            ),
            files: baseline_entries,
            local_rust_package_count: MAX_LOCAL_RUST_PACKAGES as u8,
            selected_cli_target_count: MAX_SELECTED_CLI_TARGETS as u8,
            selected_cli_binary: BoundedText::new(params.selected_binary.clone())
                .map_err(|e| format!("selected_binary: {e:?}"))?,
        },
        compatibility: ContractCompatibilityV1 {
            approved_compatibility: dto.approved_compatibility.clone(),
            intended_delta: dto.intended_delta.clone(),
            allowed_edit_paths: dto.allowed_edit_paths.clone(),
            forbidden_files: Vec::new(),
            preserved_files: dto.preserved_files.clone(),
        },
        requirements: ContractRequirementsV1 {
            mandatory_obligations,
            assumptions,
            resolutions,
            unknowns,
        },
        checker_policy: ContractCheckerPolicyV1 {
            metadata_checker: BoundedText::new(CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1)
                .map_err(|e| format!("{e:?}"))?,
            build_checker: BoundedText::new(CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1)
                .map_err(|e| format!("{e:?}"))?,
            test_checker: BoundedText::new(CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1)
                .map_err(|e| format!("{e:?}"))?,
            cli_checker: if uses_cli_checker {
                Some(
                    BoundedText::new(CHECKER_CLI_SELECTED_VECTORS_V1)
                        .map_err(|e| format!("{e:?}"))?,
                )
            } else {
                None
            },
            max_local_packages: MAX_LOCAL_RUST_PACKAGES as u8,
            fixed_files: Vec::new(),
            interface_checks: Vec::new(),
            cli_vectors: Vec::new(),
        },
        lineage: None,
        untrusted_claims: None,
    })
}

pub(crate) enum ProposeTransportConfig {
    NativeTeam {
        coding_profile_path: PathBuf,
        worker_path: PathBuf,
        artifact_path: PathBuf,
    },
    RecordedRepliesFile {
        replies_path: PathBuf,
    },
    #[cfg(test)]
    InMemoryReplies {
        replies: VecDeque<String>,
        origin: EvidenceOriginV1,
    },
}

#[cfg(test)]
struct ControlledRepliesDriver {
    replies: VecDeque<String>,
    task_id: TaskId,
}

enum StageDispatchOutcome {
    Settled {
        raw_response: String,
        receipt: Receipt,
    },
    Rejected {
        diagnostic: String,
        receipt: Receipt,
    },
    Uncertain,
}

enum ActiveDriver {
    Native {
        session: Box<ComparisonSession>,
        driver: Box<LocalInferenceDriver>,
    },
    #[cfg(test)]
    Controlled(ControlledRepliesDriver),
}

impl ActiveDriver {
    fn execute_stage(
        &mut self,
        stage_index: u8,
        prompt: &BoundedText<MAX_LOCAL_CONTEXT>,
    ) -> Result<(Digest, StageDispatchOutcome), String> {
        #[cfg(test)]
        let prompt_digest = content_digest(prompt.as_str().as_bytes());
        match self {
            ActiveDriver::Native { session, driver } => {
                let case = ComparisonCase {
                    id: BoundedText::new(format!("propose-stage-{stage_index}"))
                        .map_err(|e| format!("case id: {e:?}"))?,
                    role: LocalRole::Reasoner,
                    output_schema: LocalOutputSchema::Text,
                    context: prompt.clone(),
                };
                let permits = session
                    .admit(&[case])
                    .map_err(|e| format!("comparison session admit stage {stage_index}: {e}"))?;
                let dispatch_id = permits
                    .first()
                    .ok_or_else(|| "missing permit from ComparisonSession::admit".to_string())?
                    .request()
                    .dispatch_id
                    .clone();
                let mut outcomes = driver.execute_comparison_cohort(permits, Duration::ZERO);
                let outcome = outcomes.pop().ok_or_else(|| {
                    "empty outcome vector from LocalInferenceDriver::execute_comparison_cohort"
                        .to_string()
                })?;
                let results = match outcome {
                    QualificationOutcome::Observed(obs) => vec![ComparisonResult {
                        dispatch_id: dispatch_id.clone(),
                        observation: Some(*obs),
                        rejection: None,
                    }],
                    QualificationOutcome::Rejected(diagnostic) => vec![ComparisonResult {
                        dispatch_id: dispatch_id.clone(),
                        observation: None,
                        rejection: Some(diagnostic),
                    }],
                    QualificationOutcome::Uncertain => vec![ComparisonResult {
                        dispatch_id: dispatch_id.clone(),
                        observation: None,
                        rejection: None,
                    }],
                };
                let mut receipts = session
                    .settle(&results)
                    .map_err(|e| format!("comparison session settle stage {stage_index}: {e}"))?;
                let receipt_opt = receipts.pop().flatten();
                let cmp_res = &results[0];
                let mapped = match (receipt_opt, &cmp_res.observation, &cmp_res.rejection) {
                    (Some(receipt), Some(obs), None) => StageDispatchOutcome::Settled {
                        raw_response: obs.content.as_str().to_string(),
                        receipt,
                    },
                    (Some(receipt), None, Some(diag)) => StageDispatchOutcome::Rejected {
                        diagnostic: diag.as_str().to_string(),
                        receipt,
                    },
                    _ => StageDispatchOutcome::Uncertain,
                };
                Ok((dispatch_id, mapped))
            }
            #[cfg(test)]
            ActiveDriver::Controlled(ctrl) => {
                let dispatch_id = structural_digest(
                    "controlled-propose-dispatch/v1",
                    &[
                        ctrl.task_id.as_str().as_bytes(),
                        &[stage_index],
                        &prompt_digest.bytes(),
                    ],
                )
                .map_err(|e| format!("controlled dispatch digest: {e:?}"))?;
                let reply = ctrl
                    .replies
                    .pop_front()
                    .unwrap_or_else(|| UNCERTAIN_TEST_SENTINEL.to_string());
                if reply == UNCERTAIN_TEST_SENTINEL {
                    return Ok((dispatch_id, StageDispatchOutcome::Uncertain));
                }
                let action_digest = structural_digest(
                    "controlled-propose-action/v1",
                    &[&dispatch_id.bytes(), &prompt_digest.bytes()],
                )
                .map_err(|e| format!("controlled action digest: {e:?}"))?;
                let receipt = Receipt::new(ReceiptData {
                    schema_version: SchemaVersion::V1,
                    producer_receipt_id: ProducerReceiptId::new(dispatch_id.as_str())
                        .map_err(|e| format!("receipt id: {e:?}"))?,
                    task_id: ctrl.task_id.clone(),
                    intent_id: prompt_digest,
                    dispatch_id: dispatch_id.clone(),
                    action_digest,
                    usage: ActualUsage::Known {
                        usage: UsageVector {
                            model_requests: DecimalUsage::new("1").map_err(|e| format!("{e:?}"))?,
                            tool_calls: DecimalUsage::new("0").map_err(|e| format!("{e:?}"))?,
                            spend_micros_usd: DecimalUsage::new("0")
                                .map_err(|e| format!("{e:?}"))?,
                        },
                    },
                    output: ReceiptOutput::Fixture {
                        bytes: BoundedText::new("controlled_propose_reply")
                            .map_err(|e| format!("{e:?}"))?,
                        digest: content_digest(reply.as_bytes()),
                    },
                })
                .map_err(|e| format!("controlled receipt: {e:?}"))?;
                Ok((
                    dispatch_id,
                    StageDispatchOutcome::Settled {
                        raw_response: reply,
                        receipt,
                    },
                ))
            }
        }
    }
}

pub(crate) fn run_propose(
    output_dir: &Path,
    workspace_fd: Option<&File>,
    predecessor_fd: Option<&File>,
    auth: &HostAuthContext,
    params: &ProposeParams,
    transport: ProposeTransportConfig,
) -> Result<ProposeOutcome, String> {
    if !(1..=MAX_PREP_REQUESTS).contains(&params.max_stages) {
        return Err(format!(
            "propose max_stages must be 1..={MAX_PREP_REQUESTS}, got {}",
            params.max_stages
        ));
    }
    let mut forbidden = Vec::new();
    if let Some(pred) = predecessor_fd {
        forbidden.push((pred, "the immutable predecessor run directory"));
    }

    // Never resume or overwrite an existing propose output dir with pending/held state.
    if output_dir.exists() {
        let (existing_fd, existing_abs) = open_external_nofollow(output_dir, true)?;
        if let Some(ws) = workspace_fd {
            ensure_dir_outside(&existing_fd, ws, "the candidate workspace")?;
        }
        if let Some(pred) = predecessor_fd {
            ensure_dir_outside(
                &existing_fd,
                pred,
                "the immutable predecessor run directory",
            )?;
        }
        if existing_abs.join("propose-hold.json").exists()
            || existing_abs.join("propose-in-flight.json").exists()
            || existing_abs.join("propose-ledger.json").exists()
        {
            return Err(format!(
                "refusing automatic retry/overwrite on existing propose directory {} (existing hold/in-flight/ledger preserved)",
                existing_abs.display()
            ));
        }
        return Err(format!(
            "propose output directory {} already exists; choose a fresh directory",
            existing_abs.display()
        ));
    }

    let (dir_fd, dir_abs) = create_external_private_dir(output_dir, workspace_fd, &forbidden)?;
    let principal_id = PrincipalId::new("contract-propose").map_err(|e| format!("{e:?}"))?;
    let session_digest = structural_digest(
        "contract-propose-session/v1",
        &[
            params.contract_id.as_bytes(),
            &auth.raw_request_sha256.bytes(),
            dir_abs.to_string_lossy().as_bytes(),
        ],
    )
    .map_err(|e| format!("session digest: {e:?}"))?;
    let task_id = TaskId::new(format!("propose-{}", &session_digest.as_str()[..16]))
        .map_err(|e| format!("propose task_id: {e:?}"))?;

    let (evidence_origin, source_profile_digest, team_profile_digest, mut active) = match transport
    {
        ProposeTransportConfig::NativeTeam {
            coding_profile_path,
            worker_path,
            artifact_path,
        } => {
            let (_, coding_profile, _) =
                read_external_json::<LocalProfile>(&coding_profile_path, workspace_fd)?;
            let (team_profile, src_dig, team_dig) = derive_team_text_profile(&coding_profile)?;
            let (_, worker_abs) = open_external_nofollow(&worker_path, false)?;
            let (_, artifact_abs) = open_external_nofollow(&artifact_path, false)?;
            let artifact = resolve_pinned_artifact(&team_profile)?;
            verify_artifact(&artifact_abs, artifact)
                .map_err(|e| format!("verify native artifact {}: {e}", artifact_abs.display()))?;
            write_external_new_json(
                &dir_abs.join("native-preparation.json"),
                workspace_fd,
                &forbidden,
                &NativePreparationInputs {
                    profile: team_profile.clone(),
                    session_id: session_digest.clone(),
                    task_id: task_id.clone(),
                    principal_id: principal_id.clone(),
                    max_requests: u32::from(params.max_stages),
                },
            )?;
            let session = ComparisonSession::create(
                &dir_abs,
                session_digest.clone(),
                task_id.clone(),
                principal_id.clone(),
                team_profile.clone(),
                u32::from(params.max_stages),
            )
            .map_err(|e| format!("ComparisonSession::create: {e}"))?;
            let driver = LocalInferenceDriver::new(worker_abs, artifact_abs, team_profile, None)
                .map_err(|e| format!("LocalInferenceDriver::new: {e}"))?;
            (
                EvidenceOriginV1::NativeTeamSession,
                Some(src_dig),
                Some(team_dig),
                ActiveDriver::Native {
                    session: Box::new(session),
                    driver: Box::new(driver),
                },
            )
        }
        #[cfg(not(test))]
        ProposeTransportConfig::RecordedRepliesFile { replies_path } => {
            let _ = replies_path;
            return Err(
                "recorded model replies cannot execute preparation; import drafts through validate"
                    .into(),
            );
        }
        #[cfg(test)]
        ProposeTransportConfig::RecordedRepliesFile { replies_path } => {
            let (_, replies, _) = read_external_json::<Vec<String>>(&replies_path, workspace_fd)?;
            if replies.is_empty() || replies.len() > usize::from(MAX_PREP_REQUESTS) {
                return Err(format!(
                    "recorded replies count {} must be 1..={MAX_PREP_REQUESTS}",
                    replies.len()
                ));
            }
            (
                EvidenceOriginV1::ImportedRecordedReplies,
                None,
                None,
                ActiveDriver::Controlled(ControlledRepliesDriver {
                    replies: replies.into_iter().collect(),
                    task_id: task_id.clone(),
                }),
            )
        }
        #[cfg(test)]
        ProposeTransportConfig::InMemoryReplies { replies, origin } => {
            if origin == EvidenceOriginV1::NativeTeamSession {
                return Err("InMemoryReplies cannot claim NativeTeamSession origin".into());
            }
            if replies.is_empty() || replies.len() > usize::from(MAX_PREP_REQUESTS) {
                return Err(format!(
                    "in-memory replies count {} must be 1..={MAX_PREP_REQUESTS}",
                    replies.len()
                ));
            }
            (
                origin,
                None,
                None,
                ActiveDriver::Controlled(ControlledRepliesDriver {
                    replies,
                    task_id: task_id.clone(),
                }),
            )
        }
    };

    let mut baseline_files_raw = Vec::new();
    if let Some(ws) = &auth.workspace_snapshot {
        for (p, sha) in ws {
            baseline_files_raw.push((
                WorkspacePath::new(p).map_err(|e| format!("baseline path {p}: {e:?}"))?,
                sha.clone(),
            ));
        }
    }

    let mut ledger = ProposeSessionLedgerV1 {
        schema_version: CONTRACT_SCHEMA_VERSION,
        session_id: session_digest.clone(),
        task_id: task_id.clone(),
        contract_id: BoundedText::new(params.contract_id.clone())
            .map_err(|e| format!("contract_id: {e:?}"))?,
        evidence_origin,
        source_coding_profile_digest: source_profile_digest,
        team_profile_digest,
        max_prep_requests: params.max_stages,
        admitted_prep_requests: 0,
        unknown_hold: false,
        quality_failure: false,
        stages: Vec::new(),
        prospective_bundle_digest: None,
    };

    let mut last_raw: Option<String> = None;
    let mut last_diagnostics: Vec<String> = Vec::new();
    let mut final_draft: Option<ContractDraftV1> = None;
    let mut final_validation: Option<ContractValidationReportV1> = None;
    let mut final_bundle: Option<ContractBundleV1> = None;

    for idx in 0..params.max_stages {
        if ledger.admitted_prep_requests >= params.max_stages
            || ledger.admitted_prep_requests >= MAX_PREP_REQUESTS
        {
            break;
        }
        let stage = match idx {
            0 => ProposeStageNameV1::Proposal,
            1 => ProposeStageNameV1::Revision,
            _ => ProposeStageNameV1::Review,
        };
        let prompt =
            build_stage_prompt(params, auth, stage, last_raw.as_deref(), &last_diagnostics)?;
        let prompt_sha = content_digest(prompt.as_str().as_bytes());

        let pre_dispatch_id = structural_digest(
            "propose-inflight/v1",
            &[&session_digest.bytes(), &[idx], &prompt_sha.bytes()],
        )
        .map_err(|e| format!("inflight digest: {e:?}"))?;
        let inflight = ProposeInFlightV1 {
            session_id: session_digest.clone(),
            task_id: task_id.clone(),
            stage_index: idx,
            stage,
            dispatch_id: pre_dispatch_id,
            prompt_sha256: prompt_sha.clone(),
        };
        write_external_new_json(
            &dir_abs.join("propose-in-flight.json"),
            workspace_fd,
            &forbidden,
            &inflight,
        )?;

        if evidence_origin == EvidenceOriginV1::NativeTeamSession {
            write_external_new_json(
                &dir_abs.join(format!("stage-{idx}-input.json")),
                workspace_fd,
                &forbidden,
                &NativePreparationCase {
                    id: BoundedText::new(format!("propose-stage-{idx}"))
                        .map_err(|e| format!("{e:?}"))?,
                    context: prompt.clone(),
                },
            )?;
        }
        ledger.admitted_prep_requests = ledger.admitted_prep_requests.saturating_add(1);
        let (dispatch_id, dispatch_outcome) = active.execute_stage(idx, &prompt)?;

        match dispatch_outcome {
            StageDispatchOutcome::Uncertain => {
                // Retain propose-in-flight.json AND write immutable propose-hold.json + ledger.
                ledger.unknown_hold = true;
                ledger.stages.push(ProposeStageRecordV1 {
                    stage_index: idx,
                    stage,
                    dispatch_id: dispatch_id.clone(),
                    prompt_sha256: prompt_sha,
                    raw_response: None,
                    raw_response_sha256: None,
                    rejection: Some(
                        BoundedText::new("uncertain preparation model outcome")
                            .map_err(|e| format!("{e:?}"))?,
                    ),
                    receipt: None,
                    parse_succeeded: false,
                    parse_error: None,
                });
                write_external_new_json(
                    &dir_abs.join("propose-hold.json"),
                    workspace_fd,
                    &forbidden,
                    &serde_json::json!({
                        "schema_version": CONTRACT_SCHEMA_VERSION,
                        "status": "unknown_outcome_hold",
                        "stage_index": idx,
                        "dispatch_id": dispatch_id,
                    }),
                )?;
                write_external_new_json(
                    &dir_abs.join("propose-ledger.json"),
                    workspace_fd,
                    &forbidden,
                    &ledger,
                )?;
                return Ok(ProposeOutcome {
                    outcome: "unknown_outcome_blocked",
                    ledger,
                    draft: None,
                    validation: None,
                    bundle: None,
                });
            }
            StageDispatchOutcome::Rejected {
                diagnostic,
                receipt,
            } => {
                unlink_pinned_leaf(&dir_fd, "propose-in-flight.json")?;
                ledger.quality_failure = true;
                let diag_cut =
                    &diagnostic[..diagnostic.floor_char_boundary(diagnostic.len().min(256))];
                let stage_rec = ProposeStageRecordV1 {
                    stage_index: idx,
                    stage,
                    dispatch_id,
                    prompt_sha256: prompt_sha,
                    raw_response: None,
                    raw_response_sha256: None,
                    rejection: Some(BoundedText::new(diag_cut).map_err(|e| format!("{e:?}"))?),
                    receipt: Some(receipt),
                    parse_succeeded: false,
                    parse_error: None,
                };
                write_external_new_json(
                    &dir_abs.join(format!("stage-{idx}.json")),
                    workspace_fd,
                    &forbidden,
                    &stage_rec,
                )?;
                ledger.stages.push(stage_rec);
                last_diagnostics = vec![diagnostic];
            }
            StageDispatchOutcome::Settled {
                raw_response,
                receipt,
            } => {
                unlink_pinned_leaf(&dir_fd, "propose-in-flight.json")?;
                let raw_sha = content_digest(raw_response.as_bytes());
                let bounded_raw = BoundedUtf8::<MAX_INTERNAL>::new(raw_response.clone())?;
                last_raw = Some(raw_response.clone());

                let mut stage_rec = ProposeStageRecordV1 {
                    stage_index: idx,
                    stage,
                    dispatch_id,
                    prompt_sha256: prompt_sha,
                    raw_response: Some(bounded_raw),
                    raw_response_sha256: Some(raw_sha),
                    rejection: None,
                    receipt: Some(receipt),
                    parse_succeeded: false,
                    parse_error: None,
                };

                match parse_compact_proposal_dto(&raw_response) {
                    Ok(dto) => {
                        stage_rec.parse_succeeded = true;
                        let draft = build_draft_from_dto(params, auth, &dto, &baseline_files_raw)?;
                        let (val_report, bundle) =
                            validate_against_auth_with_mode(&draft, Some(auth), true)?;
                        let fatal_errors: Vec<String> = val_report
                            .findings
                            .iter()
                            .filter(|f| f.severity == FindingSeverity::Error)
                            .map(|f| format!("{}: {}", f.code.as_str(), f.message.as_str()))
                            .collect();
                        let diag_lines: Vec<String> = val_report
                            .findings
                            .iter()
                            .map(|f| format!("{}: {}", f.code.as_str(), f.message.as_str()))
                            .collect();
                        ledger.prospective_bundle_digest =
                            val_report.prospective_bundle_digest.clone();
                        final_draft = Some(draft);
                        final_validation = Some(val_report);
                        final_bundle = bundle;
                        write_external_new_json(
                            &dir_abs.join(format!("stage-{idx}.json")),
                            workspace_fd,
                            &forbidden,
                            &stage_rec,
                        )?;
                        ledger.stages.push(stage_rec);
                        if fatal_errors.is_empty() {
                            ledger.quality_failure = false;
                            break;
                        }
                        ledger.quality_failure = true;
                        last_diagnostics = diag_lines;
                    }
                    Err(err) => {
                        ledger.quality_failure = true;
                        let err_cut = &err[..err.floor_char_boundary(err.len().min(256))];
                        stage_rec.parse_error =
                            Some(BoundedText::new(err_cut).map_err(|e| format!("{e:?}"))?);
                        write_external_new_json(
                            &dir_abs.join(format!("stage-{idx}.json")),
                            workspace_fd,
                            &forbidden,
                            &stage_rec,
                        )?;
                        ledger.stages.push(stage_rec);
                        last_diagnostics = vec![err];
                    }
                }
            }
        }
    }

    if let Some(draft) = &final_draft {
        write_external_new_json(&dir_abs.join("draft.json"), workspace_fd, &forbidden, draft)?;
    }
    if let Some(val) = &final_validation {
        write_external_new_json(
            &dir_abs.join("validation-report.json"),
            workspace_fd,
            &forbidden,
            val,
        )?;
    }
    write_external_new_json(
        &dir_abs.join("propose-ledger.json"),
        workspace_fd,
        &forbidden,
        &ledger,
    )?;

    let outcome = if ledger.unknown_hold {
        "unknown_outcome_blocked"
    } else if ledger.quality_failure || final_draft.is_none() {
        "quality_failure"
    } else if final_validation.as_ref().is_some_and(|v| v.declared_ready) {
        "succeeded"
    } else {
        "blocked_needs_clarification"
    };

    Ok(ProposeOutcome {
        outcome,
        ledger,
        draft: final_draft,
        validation: final_validation,
        bundle: final_bundle,
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativePreparationInputs {
    pub profile: LocalProfile,
    pub session_id: Digest,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub max_requests: u32,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativePreparationCase {
    pub id: BoundedText<128>,
    pub context: BoundedText<MAX_LOCAL_CONTEXT>,
}
