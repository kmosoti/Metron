//! Bounded host-private contract schemas, deterministic digests, and objective synthesis.

use metron_protocol::{
    BoundedText, Digest, LocalProfile, MAX_INTERNAL, MAX_TEXT, Receipt, TaskId, canonical_bytes,
    coding::{CodingPolicy, CodingTask, WorkspacePath},
    structural_digest,
    toolkit::ToolCatalog,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};
use std::path::PathBuf;

pub(crate) const CONTRACT_SCHEMA_VERSION: u16 = 1;
pub(crate) const CHECKER_IMPL_VERSION_TAG: &str = "metron-contract-checker-v2";
pub(crate) const MAX_SOURCE_REFS: usize = 32;
pub(crate) const MAX_REQUIREMENTS: usize = 16;
pub(crate) const MAX_ASSUMPTIONS: usize = 16;
pub(crate) const MAX_UNKNOWNS: usize = 16;
pub(crate) const MAX_RESOLUTIONS: usize = 16;
pub(crate) const MAX_CHECKER_BINDINGS: usize = 8;
pub(crate) const MAX_CLI_VECTORS: usize = 8;
pub(crate) const MAX_CLI_ARGS: usize = 16;
pub(crate) const MAX_SCOPE_PATHS: usize = 64;
pub(crate) const MAX_BASELINE_FILES: usize = 128;
pub(crate) const MAX_LOCAL_RUST_PACKAGES: usize = 2;
pub(crate) const MAX_SELECTED_CLI_TARGETS: usize = 1;
pub(crate) const MAX_REPORT_CHECKS: usize = 32;
pub(crate) const MAX_FINDINGS: usize = 64;
pub(crate) const MAX_BASELINE_TOTAL_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_RAW_REQUEST_BYTES: usize = 4096;
pub(crate) const MAX_PREP_REQUESTS: u8 = 3;
pub(crate) const MAX_PILOT_TOTAL_MODEL_REQUESTS: u32 = 12;
pub(crate) const MAX_PILOT_ADMITTED_TOOL_EFFECTS: u32 = 16;
pub(crate) const MAX_ANNOTATIONS: usize = 128;
pub(crate) const EXPECTED_INDEPENDENT_CORPUS_SHA256: &str =
    "d1176eadd84d484145f847fbb18e4e68d633f1ff9715d0e99d3adc4d7e8d81af";
pub(crate) const EXPECTED_INDEPENDENT_CORPUS_FREEZE_SHA256: &str =
    "eaf60331444bb525a674b47bcdf463a88dfa271892db621d09222b10a8fb2fc7";
pub(crate) const PUBLIC_HARNESS_PREAMBLE: &str = "Final checks use offline+locked Cargo metadata/test/build. Prepare Cargo.lock with normal development tools. Selected CLI: metron-case.";
pub(crate) const OBJECTIVE_PROJECTION_RECIPE_TAG: &str = "contract-objective-compact-lossless/v3";

pub(crate) const CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1: &str = "snapshot-local-path-v1";
pub(crate) const CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1: &str = "cargo-offline-locked-v1";
pub(crate) const CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1: &str = "cargo-test-offline-locked-v1";
pub(crate) const CHECKER_CLI_SELECTED_VECTORS_V1: &str = "selected-cli-vectors-v1";
pub(crate) const CHECKER_FIXED_FILE_V1: &str = "fixed-file-v1";
pub(crate) const CHECKER_FORBIDDEN_FILE_V1: &str = "forbidden-file-v1";
pub(crate) const CHECKER_PRESERVED_FILE_V1: &str = "preserved-file-v1";
pub(crate) const CHECKER_SOURCE_LITERAL_V1: &str = "source-literal-v1";

pub(crate) const CONTRACT_CHECKER_REGISTRY: &[&str] = &[
    CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1,
    CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1,
    CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1,
    CHECKER_CLI_SELECTED_VECTORS_V1,
    CHECKER_FIXED_FILE_V1,
    CHECKER_FORBIDDEN_FILE_V1,
    CHECKER_PRESERVED_FILE_V1,
    CHECKER_SOURCE_LITERAL_V1,
];

/// Closed registry check; arbitrary shell or external checker identifiers are rejected.
pub(crate) fn is_trusted_checker(name: &str) -> bool {
    CONTRACT_CHECKER_REGISTRY.contains(&name)
}

/// UTF-8 byte-bounded string that explicitly permits empty (`""`) and whitespace/LF-only
/// values (such as CLI empty arguments `""` or expected outputs `""` / `"\n"`), while
/// rejecting NUL bytes and UTF-8 lengths exceeding `MAX`.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct BoundedUtf8<const MAX: usize>(String);

impl<const MAX: usize> BoundedUtf8<MAX> {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self, String> {
        let s = value.into();
        if s.len() > MAX {
            return Err(format!("UTF-8 byte length {} exceeds {MAX}", s.len()));
        }
        if s.contains('\0') {
            return Err("bounded string must not contain NUL byte".into());
        }
        Ok(Self(s))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const MAX: usize> Serialize for BoundedUtf8<MAX> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const MAX: usize> Deserialize<'de> for BoundedUtf8<MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::new(s).map_err(DeError::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContractWorkflow {
    New,
    Existing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceKind {
    OperatorRequest,
    ClarificationAnswer,
    RepositoryFile,
    TrustedSpec,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FindingSeverity {
    Error,
    ReadinessBlock,
    Warning,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContractCheckStatus {
    Passed,
    Failed,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractSourceEntryV1 {
    pub id: BoundedText<128>,
    pub claimed_kind: SourceKind,
    #[serde(default)]
    pub host_kind: Option<SourceKind>,
    pub label: BoundedText<256>,
    pub content_sha256: Digest,
    #[serde(default)]
    pub host_authenticated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BaselineFileRefV1 {
    pub path: WorkspacePath,
    pub sha256: Digest,
    #[serde(default)]
    pub preserved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractBaselineV1 {
    #[serde(default)]
    pub workspace_label: Option<BoundedText<128>>,
    #[serde(default)]
    pub files: Vec<BaselineFileRefV1>,
    #[serde(default)]
    pub local_rust_package_count: u8,
    pub selected_cli_binary: BoundedText<128>,
    #[serde(default = "default_selected_cli_targets")]
    pub selected_cli_target_count: u8,
}

fn default_selected_cli_targets() -> u8 {
    1
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractCompatibilityV1 {
    #[serde(default)]
    pub approved_compatibility: Vec<BoundedText<256>>,
    #[serde(default)]
    pub intended_delta: Vec<BoundedText<256>>,
    #[serde(default)]
    pub preserved_files: Vec<WorkspacePath>,
    #[serde(default)]
    pub allowed_edit_paths: Vec<WorkspacePath>,
    #[serde(default)]
    pub forbidden_files: Vec<WorkspacePath>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractObligationV1 {
    pub id: BoundedText<128>,
    pub clause: BoundedText<256>,
    pub checker: BoundedText<128>,
    pub source_ref: BoundedText<128>,
    #[serde(default)]
    pub approved_exception: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractAssumptionV1 {
    pub id: BoundedText<128>,
    pub text: BoundedText<256>,
    pub material: bool,
    pub source_ref: BoundedText<128>,
    /// Untrusted model/draft claim; never grants assumption authorization on its own.
    #[serde(default)]
    pub approved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractUnknownV1 {
    pub id: BoundedText<128>,
    pub question: BoundedText<256>,
    /// Descriptive metadata only; all declared unknowns block `declared_ready` until
    /// linked to a host-authenticated clarification or trusted-spec resolution.
    pub material: bool,
    #[serde(default)]
    pub resolved_by: Option<BoundedText<128>>,
    /// Descriptive metadata only; setting `blocking = false` does not bypass readiness.
    #[serde(default = "default_true")]
    pub blocking: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractResolutionV1 {
    pub id: BoundedText<128>,
    pub unknown_id: BoundedText<128>,
    pub answer: BoundedText<256>,
    pub source_ref: BoundedText<128>,
    /// Untrusted draft flag; never resolves an unknown without matching host-authenticated
    /// `ClarificationAnswer` or `TrustedSpec` content.
    #[serde(default)]
    pub approved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractRequirementsV1 {
    pub mandatory_obligations: Vec<ContractObligationV1>,
    #[serde(default)]
    pub assumptions: Vec<ContractAssumptionV1>,
    #[serde(default)]
    pub resolutions: Vec<ContractResolutionV1>,
    #[serde(default)]
    pub unknowns: Vec<ContractUnknownV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixedFileCheckV1 {
    pub id: BoundedText<128>,
    pub path: WorkspacePath,
    pub expected_sha256: Digest,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InterfaceCheckModeV1 {
    /// Lexical source-token check after stripping comments and string/char literals.
    /// Does not claim compiler/typechecker Rust API proof.
    #[default]
    SourceLexicalTokens,
    /// Rejected at validation/checker runtime until backed by compiler semantic analysis.
    SemanticRustApi,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InterfaceCheckV1 {
    pub id: BoundedText<128>,
    pub path: WorkspacePath,
    #[serde(default)]
    pub mode: InterfaceCheckModeV1,
    #[serde(default)]
    pub required_substrings: Vec<BoundedText<256>>,
    #[serde(default)]
    pub forbidden_substrings: Vec<BoundedText<256>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SealedCliVectorV1 {
    #[serde(default)]
    pub args: Vec<BoundedUtf8<256>>,
    pub expected_exit_code: i32,
    pub expected_stdout: BoundedUtf8<MAX_TEXT>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractCheckerPolicyV1 {
    pub metadata_checker: BoundedText<128>,
    pub build_checker: BoundedText<128>,
    pub test_checker: BoundedText<128>,
    #[serde(default)]
    pub cli_checker: Option<BoundedText<128>>,
    #[serde(default = "default_max_local_packages")]
    pub max_local_packages: u8,
    #[serde(default)]
    pub fixed_files: Vec<FixedFileCheckV1>,
    #[serde(default)]
    pub interface_checks: Vec<InterfaceCheckV1>,
    #[serde(default)]
    pub cli_vectors: Vec<SealedCliVectorV1>,
}

fn default_max_local_packages() -> u8 {
    MAX_LOCAL_RUST_PACKAGES as u8
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UntrustedDraftClaimsV1 {
    #[serde(default)]
    pub model_self_approved: bool,
    #[serde(default)]
    pub model_chosen_executable: Option<BoundedText<256>>,
    #[serde(default)]
    pub notes: Vec<BoundedText<256>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractLineageV1 {
    pub revision: u32,
    pub task_id: TaskId,
    pub predecessor_bundle_digest: Digest,
    pub predecessor_task_id: TaskId,
    /// Coherent audited journal head from `SqliteStore` (never raw main file bytes without WAL).
    #[serde(default)]
    pub predecessor_replay_head: Option<Digest>,
    #[serde(default)]
    pub predecessor_unknown_hold: bool,
    #[serde(default)]
    pub predecessor_open_reservations: u32,
    #[serde(default)]
    pub predecessor_unadmitted_pending_intents: u32,
    /// Must remain false; revision metadata cannot claim unknown reservations cleared.
    #[serde(default)]
    pub asserts_unknown_reservations_cleared: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractDraftV1 {
    pub schema_version: u16,
    pub contract_id: BoundedText<128>,
    pub workflow: ContractWorkflow,
    pub raw_request: BoundedText<MAX_RAW_REQUEST_BYTES>,
    pub raw_request_sha256: Digest,
    pub sources: Vec<ContractSourceEntryV1>,
    pub baseline: ContractBaselineV1,
    pub compatibility: ContractCompatibilityV1,
    pub requirements: ContractRequirementsV1,
    pub checker_policy: ContractCheckerPolicyV1,
    #[serde(default)]
    pub lineage: Option<ContractLineageV1>,
    #[serde(default)]
    pub untrusted_claims: Option<UntrustedDraftClaimsV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractBundleBodyV1 {
    pub schema_version: u16,
    pub contract_id: BoundedText<128>,
    pub workflow: ContractWorkflow,
    pub draft_digest: Digest,
    pub raw_request: BoundedText<MAX_RAW_REQUEST_BYTES>,
    pub raw_request_sha256: Digest,
    pub sources: Vec<ContractSourceEntryV1>,
    pub baseline: ContractBaselineV1,
    pub compatibility: ContractCompatibilityV1,
    pub requirements: ContractRequirementsV1,
    pub checker_policy: ContractCheckerPolicyV1,
    #[serde(default)]
    pub lineage: Option<ContractLineageV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractBundleV1 {
    pub schema_version: u16,
    pub bundle_digest: Digest,
    pub objective: BoundedText<MAX_TEXT>,
    pub body: ContractBundleBodyV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractCheckResultV1 {
    pub id: BoundedText<128>,
    pub checker: BoundedText<128>,
    pub status: ContractCheckStatus,
    #[serde(default)]
    pub output_digest: Option<Digest>,
}

/// Deterministic verifier resource usage embedded inside `ContractReportBodyV1`.
/// Excludes wall-clock timing so report identities stay wall-clock-free.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractVerifierCostV1 {
    pub cargo_invocations: u32,
    pub cli_invocations: u32,
    pub stdout_bytes: u64,
}

/// Observational wall-clock telemetry stored outside hashed contract/report identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractExecutionTelemetryV1 {
    pub wall_ms: u64,
}

/// Execution provenance derived exclusively from actual host driver/session/receipt paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceOriginV1 {
    NativeTeamSession,
    ImportedRecordedReplies,
    ScriptedTestDriver,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProposeStageNameV1 {
    Proposal,
    Revision,
    Review,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactObligationDtoV1 {
    pub id: BoundedText<128>,
    pub clause: BoundedText<256>,
    #[serde(default)]
    pub checker: Option<BoundedText<128>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactAssumptionDtoV1 {
    pub id: BoundedText<128>,
    pub text: BoundedText<256>,
    #[serde(default)]
    pub material: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactUnknownDtoV1 {
    pub id: BoundedText<128>,
    pub question: BoundedText<256>,
    #[serde(default = "default_true")]
    pub material: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactResolutionDtoV1 {
    pub id: BoundedText<128>,
    pub unknown_id: BoundedText<128>,
    pub answer: BoundedText<256>,
}

/// Host-parsed compact preparation output emitted by unconstrained `LocalOutputSchema::Text`.
/// Cannot approve itself, grant authority, or pick arbitrary executables.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactProposalDtoV1 {
    #[serde(default)]
    pub intended_delta: Vec<BoundedText<256>>,
    #[serde(default)]
    pub approved_compatibility: Vec<BoundedText<256>>,
    #[serde(default)]
    pub allowed_edit_paths: Vec<WorkspacePath>,
    #[serde(default)]
    pub preserved_files: Vec<WorkspacePath>,
    #[serde(default)]
    pub mandatory_obligations: Vec<CompactObligationDtoV1>,
    #[serde(default)]
    pub assumptions: Vec<CompactAssumptionDtoV1>,
    #[serde(default)]
    pub unknowns: Vec<CompactUnknownDtoV1>,
    #[serde(default)]
    pub resolutions: Vec<CompactResolutionDtoV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProposeStageRecordV1 {
    pub stage_index: u8,
    pub stage: ProposeStageNameV1,
    pub dispatch_id: Digest,
    pub prompt_sha256: Digest,
    #[serde(default)]
    pub raw_response: Option<BoundedUtf8<MAX_INTERNAL>>,
    #[serde(default)]
    pub raw_response_sha256: Option<Digest>,
    #[serde(default)]
    pub rejection: Option<BoundedText<256>>,
    #[serde(default)]
    pub receipt: Option<Receipt>,
    pub parse_succeeded: bool,
    #[serde(default)]
    pub parse_error: Option<BoundedText<256>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProposeInFlightV1 {
    pub session_id: Digest,
    pub task_id: TaskId,
    pub stage_index: u8,
    pub stage: ProposeStageNameV1,
    pub dispatch_id: Digest,
    pub prompt_sha256: Digest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProposeSessionLedgerV1 {
    pub schema_version: u16,
    pub session_id: Digest,
    pub task_id: TaskId,
    pub contract_id: BoundedText<128>,
    pub evidence_origin: EvidenceOriginV1,
    #[serde(default)]
    pub source_coding_profile_digest: Option<Digest>,
    #[serde(default)]
    pub team_profile_digest: Option<Digest>,
    pub max_prep_requests: u8,
    pub admitted_prep_requests: u8,
    pub unknown_hold: bool,
    pub quality_failure: bool,
    pub stages: Vec<ProposeStageRecordV1>,
    #[serde(default)]
    pub prospective_bundle_digest: Option<Digest>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExperimentArmV1 {
    ProsePlain,
    ProseArchitecture,
    TypedPlain,
    TypedArchitecture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CorpusSliceV1 {
    New,
    Existing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExpectedReadinessV1 {
    Ready,
    Blocked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AnnotationStageV1 {
    Raw,
    Clarified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaseSemanticAnnotationV1 {
    pub case_id: BoundedText<128>,
    pub arm: ExperimentArmV1,
    pub stage: AnnotationStageV1,
    pub raw_output_digest: Digest,
    pub clause_complete: bool,
    pub silent_material_assumptions: u32,
    pub unjustified_block: bool,
    pub critical_false_ready: bool,
    pub reviewer: BoundedText<128>,
    pub notes: BoundedText<256>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractSemanticAnnotationSetV1 {
    pub schema_version: u16,
    pub campaign_id: BoundedText<128>,
    pub corpus_sha256: Digest,
    pub config_digest: Digest,
    pub annotations: Vec<CaseSemanticAnnotationV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PromotionVerdictV1 {
    PromotedTypedOnly,
    PromotedTypedPlusArchitecture,
    DiagnosticGateBlocked,
    NegativeDoNotPromote,
    BlockedInvalidAnnotationsOrOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArmComparisonSummaryV1 {
    pub arm: ExperimentArmV1,
    pub new_accepted: u32,
    pub existing_accepted: u32,
    pub authority_faults: u32,
    pub oracle_leaks: u32,
    pub critical_false_ready: u32,
    pub silent_material_assumptions: u32,
    pub unjustified_blocks: u32,
    pub raw_readiness_exact_matches: u32,
    pub total_model_requests: u32,
    pub total_admitted_tool_effects: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PromotionGateDecisionV1 {
    pub verdict: PromotionVerdictV1,
    pub eligible_typed_plain: bool,
    pub eligible_typed_architecture: bool,
    pub selected_arm: Option<ExperimentArmV1>,
    pub architecture_only_finding: bool,
    pub annotation_set_digest: Option<Digest>,
    pub reasons: Vec<BoundedText<256>>,
    pub summaries: Vec<ArmComparisonSummaryV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractPromotionReceiptV1 {
    pub schema_version: u16,
    pub receipt_digest: Digest,
    pub campaign_id: BoundedText<128>,
    pub corpus_sha256: Digest,
    pub config_digest: Digest,
    pub evidence_origin: EvidenceOriginV1,
    pub decision: PromotionGateDecisionV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractReportBodyV1 {
    pub schema_version: u16,
    pub bundle_digest: Digest,
    pub task_digest: Digest,
    pub tree_digest: Digest,
    pub checker_implementation_digest: Digest,
    pub evaluator_digest: Digest,
    pub checks: Vec<ContractCheckResultV1>,
    pub verifier_cost: ContractVerifierCostV1,
    pub accepted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractReportV1 {
    pub schema_version: u16,
    pub report_digest: Digest,
    pub body: ContractReportBodyV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractSettingsV1 {
    pub schema_version: u16,
    pub task: CodingTask,
    pub policy: CodingPolicy,
    pub catalog: ToolCatalog,
    pub bundle_digest: Digest,
    pub bundle_file_sha256: Digest,
    pub checker_implementation_digest: Digest,
    pub host_executable_sha256: Digest,
    pub expected_evaluator_digest: Digest,
    pub evidence_origin: EvidenceOriginV1,
    pub dev_checks_enabled: bool,
    pub architecture_scaffold: bool,
    pub prep_requests_admitted: u32,
    pub remaining_coding_model_requests: u32,
    #[serde(default)]
    pub profile: Option<LocalProfile>,
    #[serde(default)]
    pub worker: Option<PathBuf>,
    #[serde(default)]
    pub artifact: Option<PathBuf>,
    #[serde(default)]
    pub predecessor_run_dir: Option<PathBuf>,
    #[serde(default)]
    pub predecessor_task_id: Option<TaskId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationFindingV1 {
    pub code: BoundedText<64>,
    pub severity: FindingSeverity,
    pub message: BoundedText<256>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContractValidationReportV1 {
    pub schema_version: u16,
    pub draft_digest: Digest,
    pub prospective_bundle_digest: Option<Digest>,
    pub structurally_valid: bool,
    pub declared_ready: bool,
    pub objective_bytes: Option<usize>,
    pub findings: Vec<ValidationFindingV1>,
}

pub(crate) fn draft_digest(draft: &ContractDraftV1) -> Result<Digest, String> {
    let bytes = canonical_bytes(draft, MAX_INTERNAL).map_err(|e| format!("draft bytes: {e:?}"))?;
    structural_digest("coding-contract-draft/v1", &[&bytes])
        .map_err(|e| format!("draft digest: {e:?}"))
}

pub(crate) fn bundle_body_digest(body: &ContractBundleBodyV1) -> Result<Digest, String> {
    let bytes = canonical_bytes(body, MAX_INTERNAL).map_err(|e| format!("bundle bytes: {e:?}"))?;
    structural_digest("coding-contract-bundle/v1", &[&bytes])
        .map_err(|e| format!("bundle digest: {e:?}"))
}

pub(crate) fn report_body_digest(body: &ContractReportBodyV1) -> Result<Digest, String> {
    let bytes = canonical_bytes(body, MAX_INTERNAL).map_err(|e| format!("report bytes: {e:?}"))?;
    structural_digest("coding-contract-report/v1", &[&bytes])
        .map_err(|e| format!("report digest: {e:?}"))
}

pub(crate) fn current_host_executable_digest() -> Result<Digest, String> {
    // Cache once per process against the immutable running executable image.
    // On Linux, pin /proc/self/exe via an open file descriptor so pathname
    // replacement cannot swap the underlying inode mid-hash; a new process
    // computes its own SHA-256 on startup so cross-build resume still rejects.
    static HOST_EXECUTABLE_DIGEST: std::sync::OnceLock<Result<Digest, String>> =
        std::sync::OnceLock::new();
    HOST_EXECUTABLE_DIGEST
        .get_or_init(|| {
            let hex = {
                #[cfg(target_os = "linux")]
                {
                    use std::os::fd::AsRawFd;
                    let file = std::fs::File::open("/proc/self/exe")
                        .map_err(|e| format!("open current host executable /proc/self/exe: {e}"))?;
                    let fd_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
                    let hex =
                        metron_inference_local::artifact::sha256_file(&fd_path).map_err(|e| {
                            format!("hash current host executable {}: {e:?}", fd_path.display())
                        })?;
                    drop(file);
                    hex
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let exe = std::env::current_exe()
                        .map_err(|e| format!("resolve current host executable path: {e}"))?;
                    metron_inference_local::artifact::sha256_file(&exe).map_err(|e| {
                        format!("hash current host executable {}: {e:?}", exe.display())
                    })?
                }
            };
            Digest::new(hex).map_err(|e| format!("host executable digest: {e:?}"))
        })
        .clone()
}

pub(crate) fn contract_verifier_implementation_digest(
    catalog: &ToolCatalog,
) -> Result<Digest, String> {
    let host_exe = current_host_executable_digest()?;
    contract_verifier_implementation_digest_with_host(catalog, &host_exe)
}

pub(crate) fn contract_verifier_implementation_digest_with_host(
    catalog: &ToolCatalog,
    host_executable_sha256: &Digest,
) -> Result<Digest, String> {
    let catalog_digest = catalog
        .digest()
        .map_err(|e| format!("tool catalog digest: {e:?}"))?;
    #[derive(Serialize)]
    struct VerifierImplView<'a> {
        schema_version: u16,
        impl_tag: &'static str,
        host_executable_sha256: &'a Digest,
        catalog_digest: &'a Digest,
        registry: &'static [&'static str],
        max_baseline_files: usize,
        max_baseline_total_bytes: usize,
        max_local_rust_packages: usize,
        max_selected_cli_targets: usize,
        max_cli_vectors: usize,
    }
    let view = VerifierImplView {
        schema_version: CONTRACT_SCHEMA_VERSION,
        impl_tag: CHECKER_IMPL_VERSION_TAG,
        host_executable_sha256,
        catalog_digest: &catalog_digest,
        registry: CONTRACT_CHECKER_REGISTRY,
        max_baseline_files: MAX_BASELINE_FILES,
        max_baseline_total_bytes: MAX_BASELINE_TOTAL_BYTES,
        max_local_rust_packages: MAX_LOCAL_RUST_PACKAGES,
        max_selected_cli_targets: MAX_SELECTED_CLI_TARGETS,
        max_cli_vectors: MAX_CLI_VECTORS,
    };
    let bytes = canonical_bytes(&view, MAX_INTERNAL)
        .map_err(|e| format!("verifier impl digest bytes: {e:?}"))?;
    structural_digest("coding-contract-verifier-impl/v2", &[&bytes])
        .map_err(|e| format!("verifier impl digest: {e:?}"))
}

pub(crate) fn promotion_receipt_digest(
    campaign_id: &BoundedText<128>,
    corpus_sha256: &Digest,
    config_digest: &Digest,
    evidence_origin: EvidenceOriginV1,
    decision: &PromotionGateDecisionV1,
) -> Result<Digest, String> {
    #[derive(Serialize)]
    struct View<'a> {
        schema_version: u16,
        campaign_id: &'a BoundedText<128>,
        corpus_sha256: &'a Digest,
        config_digest: &'a Digest,
        evidence_origin: EvidenceOriginV1,
        decision: &'a PromotionGateDecisionV1,
    }
    let view = View {
        schema_version: CONTRACT_SCHEMA_VERSION,
        campaign_id,
        corpus_sha256,
        config_digest,
        evidence_origin,
        decision,
    };
    let bytes =
        canonical_bytes(&view, MAX_INTERNAL).map_err(|e| format!("promotion receipt: {e:?}"))?;
    structural_digest("coding-contract-promotion/v1", &[&bytes])
        .map_err(|e| format!("promotion receipt digest: {e:?}"))
}

/// Canonical closed-evaluator identity bound to both contract policy and expected
/// host verifier implementation + tool catalog digest.
pub(crate) fn expected_contract_evaluator_digest(
    bundle: &ContractBundleV1,
    checker_implementation_digest: &Digest,
) -> Result<Digest, String> {
    #[derive(Serialize)]
    struct EvaluatorPolicyView<'a> {
        schema_version: u16,
        checker_implementation_digest: &'a Digest,
        selected_cli_binary: &'a str,
        selected_cli_target_count: u8,
        local_rust_package_count: u8,
        checker_policy: &'a ContractCheckerPolicyV1,
        forbidden_files: &'a [WorkspacePath],
        preserved_files: &'a [WorkspacePath],
        allowed_edit_paths: &'a [WorkspacePath],
    }
    let view = EvaluatorPolicyView {
        schema_version: CONTRACT_SCHEMA_VERSION,
        checker_implementation_digest,
        selected_cli_binary: bundle.body.baseline.selected_cli_binary.as_str(),
        selected_cli_target_count: bundle.body.baseline.selected_cli_target_count,
        local_rust_package_count: bundle.body.baseline.local_rust_package_count,
        checker_policy: &bundle.body.checker_policy,
        forbidden_files: &bundle.body.compatibility.forbidden_files,
        preserved_files: &bundle.body.compatibility.preserved_files,
        allowed_edit_paths: &bundle.body.compatibility.allowed_edit_paths,
    };
    let bytes =
        canonical_bytes(&view, MAX_INTERNAL).map_err(|e| format!("evaluator policy: {e:?}"))?;
    structural_digest("coding-contract-evaluator/v2", &[&bytes])
        .map_err(|e| format!("evaluator digest: {e:?}"))
}

pub(crate) fn split_text_lossless_clauses(
    input: &str,
    max_chunk_bytes: usize,
    max_clauses: usize,
) -> Result<Vec<String>, String> {
    let mut remaining = input.trim();
    if remaining.is_empty() {
        return Ok(Vec::new());
    }
    if max_chunk_bytes < 4 {
        return Err("max_chunk_bytes must be at least 4 for UTF-8 boundaries".into());
    }
    let mut clauses = Vec::new();
    while !remaining.is_empty() {
        if clauses.len() >= max_clauses {
            return Err(format!(
                "source text exceeds {max_clauses} lossless clauses of <= {max_chunk_bytes} bytes"
            ));
        }
        if remaining.len() <= max_chunk_bytes {
            clauses.push(remaining.to_string());
            break;
        }
        let limit = remaining.floor_char_boundary(max_chunk_bytes);
        let window = &remaining[..limit];
        let sentence_split = window
            .char_indices()
            .filter(|&(_, ch)| matches!(ch, '.' | '!' | '?' | '\n'))
            .map(|(idx, ch)| idx + ch.len_utf8())
            .rfind(|&end| {
                end == remaining.len() || remaining[end..].starts_with(char::is_whitespace)
            });
        let word_split = window.rfind(char::is_whitespace);
        let split_at = sentence_split
            .or(word_split)
            .filter(|&idx| idx > 0)
            .unwrap_or(limit);
        if split_at == 0 {
            return Err("unable to split source text along UTF-8 boundary".into());
        }
        let chunk = remaining[..split_at].trim();
        if !chunk.is_empty() {
            clauses.push(chunk.to_string());
        }
        remaining = remaining[split_at..].trim_start();
    }
    Ok(clauses)
}

/// Project complete public sources and approved obligations without guessing prose equivalence.
/// Literal containment only removes duplicate bytes; it does not establish semantic coverage.
pub(crate) fn build_contract_objective(
    bundle_digest: &Digest,
    body: &ContractBundleBodyV1,
) -> Result<BoundedText<MAX_TEXT>, (usize, String)> {
    let preamble = if body.baseline.selected_cli_binary.as_str() == "metron-case" {
        std::borrow::Cow::Borrowed(PUBLIC_HARNESS_PREAMBLE)
    } else {
        std::borrow::Cow::Owned(format!(
            "Final checks use offline+locked Cargo metadata/test/build. Prepare Cargo.lock with normal development tools. Selected CLI: {}.",
            body.baseline.selected_cli_binary.as_str()
        ))
    };
    let mut text = format!(
        "{preamble}\nBundle: {}\n{}",
        bundle_digest.as_str(),
        body.raw_request.as_str()
    );

    // These complete source clauses are added by the host before freezing, not inferred
    // from a model's possibly abridged resolution. Authenticate the reconstructed bytes.
    for source in &body.sources {
        if source.host_kind != Some(SourceKind::ClarificationAnswer) {
            continue;
        }
        let host_chunks = body
            .requirements
            .mandatory_obligations
            .iter()
            .filter(|ob| ob.source_ref == source.id && ob.id.as_str().starts_with("host-clar-"))
            .map(|ob| ob.clause.as_str())
            .collect::<String>();
        let clauses: Vec<&str> = body
            .requirements
            .mandatory_obligations
            .iter()
            .filter(|ob| ob.source_ref == source.id && ob.id.as_str().starts_with("clar-r"))
            .map(|ob| ob.clause.as_str())
            .collect();
        let reconstructed = clauses.join(" ");
        let complete_answer = if !host_chunks.is_empty()
            && metron_protocol::content_digest(host_chunks.as_bytes()) == source.content_sha256
        {
            host_chunks
        } else if !clauses.is_empty()
            && metron_protocol::content_digest(reconstructed.as_bytes()) == source.content_sha256
        {
            reconstructed
        } else if let Some(resolution) = body.requirements.resolutions.iter().find(|res| {
            res.source_ref == source.id
                && metron_protocol::content_digest(res.answer.as_str().as_bytes())
                    == source.content_sha256
        }) {
            resolution.answer.as_str().to_owned()
        } else {
            return Err((
                text.len(),
                format!(
                    "complete authenticated clarification '{}' is unavailable for objective projection",
                    source.id.as_str()
                ),
            ));
        };
        if !text.contains(&complete_answer) {
            text.push_str("\nResolution: ");
            text.push_str(&complete_answer);
        }
    }

    let mut append_clause = |clause: &str| {
        if !clause.is_empty() && !text.contains(clause) {
            text.push('\n');
            text.push_str(clause);
        }
    };
    for obligation in &body.requirements.mandatory_obligations {
        append_clause(obligation.clause.as_str());
    }
    for clause in &body.compatibility.approved_compatibility {
        append_clause(clause.as_str());
    }
    for clause in &body.compatibility.intended_delta {
        append_clause(clause.as_str());
    }
    for resolution in &body.requirements.resolutions {
        append_clause(resolution.answer.as_str());
    }

    if !body.compatibility.preserved_files.is_empty() {
        text.push_str("\nPreserve:");
        for (idx, path) in body.compatibility.preserved_files.iter().enumerate() {
            text.push(if idx == 0 { ' ' } else { ',' });
            text.push_str(path.as_str());
        }
        text.push('.');
    }
    if !body.compatibility.allowed_edit_paths.is_empty() {
        text.push_str("\nEdit:");
        for (idx, path) in body.compatibility.allowed_edit_paths.iter().enumerate() {
            text.push(if idx == 0 { ' ' } else { ',' });
            text.push_str(path.as_str());
        }
        text.push('.');
    }

    let mut guidance = body
        .requirements
        .assumptions
        .iter()
        .filter(|assumption| assumption.id.as_str() == "prepared-guidance");
    if let Some(advisory) = guidance.next() {
        if guidance.next().is_some() || advisory.material || advisory.text.as_str().len() > 32 {
            return Err((
                text.len(),
                "prepared-guidance must be one optional nonmaterial advisory of at most 32 UTF-8 bytes"
                    .into(),
            ));
        }
        if !advisory.text.as_str().is_empty() {
            text.push_str("\nGuide: ");
            text.push_str(advisory.text.as_str());
        }
    }

    let byte_len = text.len();
    if byte_len > MAX_TEXT {
        return Err((
            byte_len,
            format!(
                "deterministic objective length {byte_len} UTF-8 bytes exceeds {MAX_TEXT}-byte limit without truncation"
            ),
        ));
    }
    BoundedText::new(text).map_err(|error| {
        (
            byte_len,
            format!("deterministic objective rejected by BoundedText<{MAX_TEXT}>: {error:?}"),
        )
    })
}
