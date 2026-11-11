//! Pinned executable inventories are data, not tool execution permissions.
use crate::{
    BoundedText, Digest, MAX_ENVELOPE, MAX_TEXT, ValidationError, canonical_bytes,
    structural_digest,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolId {
    Jj,
    Ripgrep,
    Z3,
    Lean,
    Cargo,
    Rustc,
    Bubblewrap,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPin {
    pub tool: ToolId,
    pub executable: BoundedText<MAX_TEXT>,
    pub sha256: Digest,
    pub version_prefix: BoundedText<256>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCatalog {
    pub schema_version: u16,
    pub tools: Vec<ToolPin>,
}
impl ToolCatalog {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != 1 {
            return Err(ValidationError::Version);
        }
        if self.tools.is_empty() || self.tools.len() > 7 {
            return Err(ValidationError::Limit);
        }
        let mut seen = std::collections::BTreeSet::new();
        for tool in &self.tools {
            if !seen.insert(tool.tool)
                || !tool.executable.as_str().starts_with('/')
                || tool.executable.as_str().contains('\0')
                || tool.version_prefix.as_str().trim().is_empty()
                || tool.version_prefix.as_str().contains(['\n', '\r'])
            {
                return Err(ValidationError::Identifier);
            }
        }
        canonical_bytes(self, MAX_ENVELOPE)?;
        Ok(())
    }
    pub fn digest(&self) -> Result<Digest, ValidationError> {
        self.validate()?;
        structural_digest("tool-catalog/v1", &[&canonical_bytes(self, MAX_ENVELOPE)?])
    }
}
