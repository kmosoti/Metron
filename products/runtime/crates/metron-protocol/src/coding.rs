//! Coding proposals describe requested effects. They never convey authority.
//! Lexical path validation is additional input hygiene, not OS containment.
use crate::{
    BoundedText, BudgetRequest, Digest, MAX_ENVELOPE, MAX_MODEL_OUTPUT, MAX_TEXT, PrincipalId,
    TaskId, ValidationError, canonical_bytes, structural_digest,
};
use serde::{Deserialize, Serialize};

pub const CODING_TASK_VERSION: u16 = 3;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodingFamily {
    CodingV1,
}

/// Host resolves project, collection and acceptance references independently.
/// A reference in a model proposal is not permission to resolve it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodingTaskData {
    pub schema_version: u16,
    pub family: CodingFamily,
    pub task_id: TaskId,
    pub principal_id: PrincipalId,
    pub project_id: BoundedText<128>,
    pub objective: BoundedText<MAX_TEXT>,
    pub acceptance_digest: Digest,
    pub tool_catalog_digest: Digest,
    pub model_profile_digest: Digest,
    pub memory_collection: Option<BoundedText<128>>,
    pub budget: BudgetRequest,
    pub max_decision_steps: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CodingTaskData", into = "CodingTaskData")]
pub struct CodingTask(CodingTaskData);

impl CodingTask {
    pub fn new(data: CodingTaskData) -> Result<Self, ValidationError> {
        if data.schema_version != CODING_TASK_VERSION {
            return Err(ValidationError::Version);
        }
        data.budget.validate()?;
        if data.max_decision_steps == 0
            || data.max_decision_steps > 64
            || data.objective.as_str().trim().is_empty()
            || data.project_id.as_str().trim().is_empty()
            || data
                .memory_collection
                .as_ref()
                .is_some_and(|v| v.as_str().trim().is_empty())
        {
            return Err(ValidationError::Limit);
        }
        canonical_bytes(&data, MAX_ENVELOPE)?;
        Ok(Self(data))
    }
    pub fn data(&self) -> &CodingTaskData {
        &self.0
    }
    pub fn digest(&self) -> Result<Digest, ValidationError> {
        structural_digest("coding-task/v3", &[&canonical_bytes(self, MAX_ENVELOPE)?])
    }
}
impl TryFrom<CodingTaskData> for CodingTask {
    type Error = ValidationError;
    fn try_from(data: CodingTaskData) -> Result<Self, Self::Error> {
        Self::new(data)
    }
}
impl From<CodingTask> for CodingTaskData {
    fn from(task: CodingTask) -> Self {
        task.0
    }
}

/// Relative UTF-8 file names only. Sandbox and directory handles must still
/// enforce containment against symlinks and concurrent filesystem changes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WorkspacePath(String);
impl WorkspacePath {
    pub fn new(path: impl Into<String>) -> Result<Self, ValidationError> {
        let path = path.into();
        if path.is_empty()
            || path.len() > MAX_TEXT
            || path.contains(['\0', '\\', '\n', '\r'])
            || path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".." || part.len() > 255)
            || matches!(path.split('/').next(), Some(".git" | ".jj"))
        {
            return Err(ValidationError::Identifier);
        }
        Ok(Self(path))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for WorkspacePath {
    type Error = ValidationError;
    fn try_from(path: String) -> Result<Self, Self::Error> {
        Self::new(path)
    }
}
impl From<WorkspacePath> for String {
    fn from(path: WorkspacePath) -> Self {
        path.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPersona {
    Planner,
    Coder,
    Reviewer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RustProgram {
    Cargo,
    Rustc,
    Rustfmt,
    Clippy,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingTool {
    Plan,
    List,
    Search,
    Read,
    Edit,
    Run,
    JjSnapshot,
    JjDiff,
    Smt,
    Lean,
    MemoryPut,
    MemorySearch,
    Finish,
}

/// Host-owned immutable policy; it is never populated from a model proposal.
/// Filesystem identity and sandbox support need additional host preflight.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodingPolicy {
    pub schema_version: u16,
    pub principal_id: PrincipalId,
    pub project_id: BoundedText<128>,
    pub workspace_root: BoundedText<MAX_TEXT>,
    pub memory_collection: Option<BoundedText<128>>,
    pub allowed_tools: Vec<CodingTool>,
    pub allowed_programs: Vec<RustProgram>,
    pub require_mutation_approval: bool,
    pub max_file_bytes: u32,
    pub max_output_bytes: u32,
    pub command_timeout_ms: u32,
    pub command_memory_bytes: crate::SqlU64,
    pub max_processes: u16,
}
impl CodingPolicy {
    pub fn validate(&self, task: &CodingTask) -> Result<(), ValidationError> {
        let data = task.data();
        if self.schema_version != 1
            || self.principal_id != data.principal_id
            || self.project_id != data.project_id
            || self.memory_collection != data.memory_collection
            || !self.workspace_root.as_str().starts_with('/')
            || self.workspace_root.as_str().contains('\0')
        {
            return Err(ValidationError::Binding);
        }
        let unique: std::collections::BTreeSet<_> = self.allowed_tools.iter().collect();
        if self.allowed_tools.is_empty()
            || self.allowed_tools.len() != unique.len()
            || self.allowed_tools.len() > 13
            || self.allowed_programs.len() > 4
            || self
                .allowed_programs
                .iter()
                .enumerate()
                .any(|(i, p)| self.allowed_programs[..i].contains(p))
            || self.max_file_bytes == 0
            || self.max_file_bytes > 1024 * 1024
            || self.max_output_bytes == 0
            || self.max_output_bytes > MAX_MODEL_OUTPUT as u32
            || self.command_timeout_ms == 0
            || self.command_timeout_ms > 120_000
            || self.command_memory_bytes.get() < 128 * 1024 * 1024
            || self.command_memory_bytes.get() > 64 * 1024 * 1024 * 1024
            || self.max_processes == 0
            || self.max_processes > 256
        {
            return Err(ValidationError::Limit);
        }
        canonical_bytes(self, MAX_ENVELOPE)?;
        Ok(())
    }
    pub fn digest(&self, task: &CodingTask) -> Result<Digest, ValidationError> {
        self.validate(task)?;
        structural_digest("coding-policy/v1", &[&canonical_bytes(self, MAX_ENVELOPE)?])
    }
    /// Requested tool/profile data cannot raise these host grants. Persona is
    /// absent by construction; changing one cannot add an allowed action.
    pub fn permits(&self, action: &CodingAction) -> bool {
        action.validate().is_ok()
            && self.allowed_tools.contains(&action.tool())
            && match action {
                CodingAction::Run { program, .. } => self.allowed_programs.contains(program),
                CodingAction::MemoryPut { .. } | CodingAction::MemorySearch { .. } => {
                    self.memory_collection.is_some()
                }
                _ => true,
            }
    }
}

/// This is an untrusted proposal. Finish requests verification; it cannot set
/// a verdict. Personas deliberately have no place in the authority binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum CodingAction {
    Plan {
        steps: Vec<BoundedText<256>>,
    },
    List {
        directory: Option<WorkspacePath>,
    },
    Search {
        query: BoundedText<MAX_TEXT>,
        directory: Option<WorkspacePath>,
    },
    Read {
        path: WorkspacePath,
        first_line: u32,
        line_count: u16,
    },
    Edit {
        path: WorkspacePath,
        expected_digest: Option<Digest>,
        content: BoundedText<MAX_MODEL_OUTPUT>,
    },
    Run {
        program: RustProgram,
        args: Vec<BoundedText<MAX_TEXT>>,
        directory: Option<WorkspacePath>,
    },
    JjSnapshot,
    JjDiff,
    Smt {
        problem: BoundedText<MAX_MODEL_OUTPUT>,
    },
    Lean {
        statement: BoundedText<MAX_TEXT>,
        proof: BoundedText<MAX_MODEL_OUTPUT>,
    },
    MemoryPut {
        content: BoundedText<MAX_TEXT>,
        source_evidence: Option<Digest>,
    },
    MemorySearch {
        query: BoundedText<MAX_TEXT>,
        limit: u8,
    },
    Finish {
        summary: BoundedText<MAX_TEXT>,
    },
}
impl CodingAction {
    pub fn tool(&self) -> CodingTool {
        match self {
            Self::Plan { .. } => CodingTool::Plan,
            Self::List { .. } => CodingTool::List,
            Self::Search { .. } => CodingTool::Search,
            Self::Read { .. } => CodingTool::Read,
            Self::Edit { .. } => CodingTool::Edit,
            Self::Run { .. } => CodingTool::Run,
            Self::JjSnapshot => CodingTool::JjSnapshot,
            Self::JjDiff => CodingTool::JjDiff,
            Self::Smt { .. } => CodingTool::Smt,
            Self::Lean { .. } => CodingTool::Lean,
            Self::MemoryPut { .. } => CodingTool::MemoryPut,
            Self::MemorySearch { .. } => CodingTool::MemorySearch,
            Self::Finish { .. } => CodingTool::Finish,
        }
    }
    pub fn mutates_workspace(&self) -> bool {
        matches!(
            self,
            Self::Edit { .. } | Self::Run { .. } | Self::JjSnapshot
        )
    }
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Plan { steps }
                if steps.is_empty()
                    || steps.len() > 12
                    || steps.iter().any(|s| s.as_str().trim().is_empty()) =>
            {
                return Err(ValidationError::Limit);
            }
            Self::Read {
                first_line,
                line_count,
                ..
            } if *first_line == 0 || *line_count == 0 || *line_count > 200 => {
                return Err(ValidationError::Limit);
            }
            Self::Read {
                first_line,
                line_count,
                ..
            } if first_line.checked_add(u32::from(*line_count) - 1).is_none() => {
                return Err(ValidationError::Limit);
            }
            Self::Run { args, .. }
                if args.len() > 32 || args.iter().any(|arg| arg.as_str().contains('\0')) =>
            {
                return Err(ValidationError::Limit);
            }
            Self::MemorySearch { limit, .. } if *limit == 0 || *limit > 20 => {
                return Err(ValidationError::Limit);
            }
            _ => {}
        }
        canonical_bytes(self, MAX_ENVELOPE)?;
        Ok(())
    }
    pub fn digest(&self) -> Result<Digest, ValidationError> {
        self.validate()?;
        structural_digest("coding-action/v1", &[&canonical_bytes(self, MAX_ENVELOPE)?])
    }
}

/// Strict deserialization AND semantic validation, including frame size.
pub fn parse_coding_action(bytes: &[u8]) -> Result<CodingAction, ValidationError> {
    let action: CodingAction = crate::parse_envelope(bytes)?;
    action.validate()?;
    Ok(action)
}
