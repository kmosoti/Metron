//! The Claude Code bridge: language-model inference through the agent
//! session that runs the experiment.
//!
//! Protocol, relative to a run directory:
//!
//! 1. A consult operator calls the `llm` service. If
//!    `llm/responses/<request_id>.json` (or `.txt`) exists, the bridge
//!    reads it, issues a receipt and an answer, and returns the content.
//! 2. Otherwise the bridge writes `llm/requests/<request_id>.json`, which
//!    holds the prompt and says where to put the answer, and returns
//!    [`ServiceError::Pending`]. The runner suspends the episode and the
//!    composition root writes a checkpoint.
//! 3. A Claude Code session (or a person) reads the request, writes the
//!    response file, and runs `metron resume <run-dir>`.
//!
//! `request_id` is derived from the inquiry and the request content, so a
//! resumed episode asks for the same file it was waiting on. Every answered
//! consultation is receipted as an external call and journaled in full, so
//! the episode replays without the session.

use crate::service::ServiceBackend;
use metron_core::cost::Cost;
use metron_core::id::{InquiryId, OperatorId};
use metron_core::inquiry::Representation;
use metron_core::receipt::{OperationKind, ResourceReceipt};
use metron_core::world::{IdSource, ServiceAnswer, ServiceError, ServiceResponse};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The service name the bridge answers for.
pub const SERVICE: &str = "llm";

/// What the bridge writes for the agent to read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestFile {
    /// Identifier; also the file stem.
    pub request_id: String,
    /// Service.
    pub service: String,
    /// Operator that asked.
    pub operator: OperatorId,
    /// Inquiry it serves.
    pub inquiry: InquiryId,
    /// The request content. For the reference operators this is JSON with
    /// a `prompt` field.
    pub request: Representation,
    /// Where to write the answer and in what shape.
    pub how_to_answer: String,
}

/// What the agent writes back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseFile {
    /// The answer: a JSON representation, or a plain string which is taken
    /// as text.
    pub response: serde_json::Value,
    /// Who answered.
    #[serde(default = "default_answered_by")]
    pub answered_by: String,
}

fn default_answered_by() -> String {
    "claude-code".into()
}

/// The bridge.
#[derive(Debug)]
pub struct ClaudeCodeBridge {
    dir: PathBuf,
    receipts: Vec<ResourceReceipt>,
    answers: Vec<ServiceAnswer>,
}

impl ClaudeCodeBridge {
    /// Creates a bridge rooted at `run_dir/llm`.
    pub fn new(run_dir: impl AsRef<Path>) -> Self {
        Self {
            dir: run_dir.as_ref().join("llm"),
            receipts: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// The directory requests are written to.
    #[must_use]
    pub fn requests_dir(&self) -> PathBuf {
        self.dir.join("requests")
    }

    /// The directory responses are read from.
    #[must_use]
    pub fn responses_dir(&self) -> PathBuf {
        self.dir.join("responses")
    }

    /// Path of a request file.
    #[must_use]
    pub fn request_path(&self, request_id: &str) -> PathBuf {
        self.requests_dir().join(format!("{request_id}.json"))
    }

    /// Path of a JSON response file.
    #[must_use]
    pub fn response_path(&self, request_id: &str) -> PathBuf {
        self.responses_dir().join(format!("{request_id}.json"))
    }

    /// Path of a plain-text response file.
    #[must_use]
    pub fn response_text_path(&self, request_id: &str) -> PathBuf {
        self.responses_dir().join(format!("{request_id}.txt"))
    }

    /// The request identifier for a request.
    #[must_use]
    pub fn request_id(inquiry: InquiryId, request: &Representation) -> String {
        format!("inq{}-{}", inquiry.0, request.content_hash().short())
    }

    /// Request identifiers with a request file but no response yet.
    pub fn pending_requests(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.requests_dir()) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let stem = path.file_stem()?.to_str()?.to_owned();
                (path.extension().is_some_and(|x| x == "json")
                    && !self.response_path(&stem).exists()
                    && !self.response_text_path(&stem).exists())
                .then_some(stem)
            })
            .collect();
        ids.sort();
        ids
    }

    fn read_response(&self, request_id: &str) -> Result<Option<ResponseFile>, ServiceError> {
        let json_path = self.response_path(request_id);
        if json_path.exists() {
            let text = fs::read_to_string(&json_path)
                .map_err(|e| ServiceError::Malformed(e.to_string()))?;
            let parsed: ResponseFile = serde_json::from_str(&text)
                .map_err(|e| ServiceError::Malformed(format!("{}: {e}", json_path.display())))?;
            return Ok(Some(parsed));
        }
        let text_path = self.response_text_path(request_id);
        if text_path.exists() {
            let text = fs::read_to_string(&text_path)
                .map_err(|e| ServiceError::Malformed(e.to_string()))?;
            return Ok(Some(ResponseFile {
                response: serde_json::Value::String(text.trim().to_owned()),
                answered_by: default_answered_by(),
            }));
        }
        Ok(None)
    }

    fn write_request(&self, file: &RequestFile) -> Result<(), ServiceError> {
        let path = self.request_path(&file.request_id);
        if path.exists() {
            return Ok(());
        }
        fs::create_dir_all(self.requests_dir())
            .map_err(|e| ServiceError::Unavailable(e.to_string()))?;
        fs::create_dir_all(self.responses_dir())
            .map_err(|e| ServiceError::Unavailable(e.to_string()))?;
        let text = serde_json::to_string_pretty(file)
            .map_err(|e| ServiceError::Malformed(e.to_string()))?;
        fs::write(&path, text).map_err(|e| ServiceError::Unavailable(e.to_string()))
    }
}

impl ServiceBackend for ClaudeCodeBridge {
    fn service(&self) -> &str {
        SERVICE
    }

    fn consult_using(
        &mut self,
        ids: &mut dyn IdSource,
        operator: &OperatorId,
        inquiry: InquiryId,
        request: &Representation,
    ) -> Result<ServiceResponse, ServiceError> {
        let started = Instant::now();
        let request_id = Self::request_id(inquiry, request);
        let Some(file) = self.read_response(&request_id)? else {
            self.write_request(&RequestFile {
                request_id: request_id.clone(),
                service: SERVICE.into(),
                operator: operator.clone(),
                inquiry,
                request: request.clone(),
                how_to_answer: format!(
                    "Write the answer to {} as JSON {{\"response\": <text or a {{\"kind\":...,\"value\":...}} representation>, \"answered_by\": \"<who>\"}}, or write plain text to {}; then run `metron resume <run-dir>`.",
                    self.response_path(&request_id).display(),
                    self.response_text_path(&request_id).display()
                ),
            })?;
            return Err(ServiceError::Pending { request_id });
        };
        let content = match file.response {
            serde_json::Value::String(s) => Representation::Text(s),
            other => serde_json::from_value::<Representation>(other.clone())
                .unwrap_or(Representation::Json(other)),
        };
        let receipt = ResourceReceipt::seal(
            ids.next_receipt(),
            OperationKind::ExternalCall {
                service: SERVICE.into(),
            },
            operator.clone(),
            inquiry,
            request.content_hash(),
            content.content_hash(),
            Cost::external(1),
            Vec::new(),
            u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
        self.answers.push(ServiceAnswer {
            receipt: receipt.id,
            service: SERVICE.into(),
            request: request.clone(),
            response: content.clone(),
            answered_by: file.answered_by.clone(),
        });
        self.receipts.push(receipt);
        Ok(ServiceResponse {
            content,
            answered_by: file.answered_by,
        })
    }

    fn take_receipts(&mut self) -> Vec<ResourceReceipt> {
        std::mem::take(&mut self.receipts)
    }

    fn take_answers(&mut self) -> Vec<ServiceAnswer> {
        std::mem::take(&mut self.answers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_core::world::Counter;

    #[test]
    fn pending_then_answered() {
        let dir = std::env::temp_dir().join(format!("metron-bridge-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut bridge = ClaudeCodeBridge::new(&dir);
        let mut ids = Counter::default();
        let op = OperatorId::from("ask");
        let request = Representation::Json(serde_json::json!({"prompt": "hello"}));
        let err = bridge
            .consult_using(&mut ids, &op, InquiryId(1), &request)
            .unwrap_err();
        let ServiceError::Pending { request_id } = err else {
            panic!("expected pending");
        };
        assert!(bridge.request_path(&request_id).exists());
        assert_eq!(bridge.pending_requests(), vec![request_id.clone()]);
        assert!(bridge.take_receipts().is_empty());
        // The same request again is still the same pending id.
        assert_eq!(
            bridge.consult_using(&mut ids, &op, InquiryId(1), &request),
            Err(ServiceError::Pending {
                request_id: request_id.clone()
            })
        );
        fs::write(bridge.response_text_path(&request_id), "x0 & x1\n").unwrap();
        let response = bridge
            .consult_using(&mut ids, &op, InquiryId(1), &request)
            .unwrap();
        assert_eq!(response.content, Representation::Text("x0 & x1".into()));
        assert_eq!(response.answered_by, "claude-code");
        let receipts = bridge.take_receipts();
        assert_eq!(receipts.len(), 1);
        assert!(
            matches!(receipts[0].operation, OperationKind::ExternalCall { ref service } if service == "llm")
        );
        assert_eq!(receipts[0].cost, Cost::external(1));
        let answers = bridge.take_answers();
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].receipt, receipts[0].id);
        assert!(bridge.pending_requests().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_responses_carry_who_answered() {
        let dir = std::env::temp_dir().join(format!("metron-bridge-json-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut bridge = ClaudeCodeBridge::new(&dir);
        let mut ids = Counter::default();
        let request = Representation::Text("q".into());
        let id = ClaudeCodeBridge::request_id(InquiryId(2), &request);
        fs::create_dir_all(bridge.responses_dir()).unwrap();
        fs::write(
            bridge.response_path(&id),
            r#"{"response": {"kind": "text", "value": "x1"}, "answered_by": "session-42"}"#,
        )
        .unwrap();
        let r = bridge
            .consult_using(&mut ids, &OperatorId::from("ask"), InquiryId(2), &request)
            .unwrap();
        assert_eq!(r.content, Representation::Text("x1".into()));
        assert_eq!(r.answered_by, "session-42");
        let _ = fs::remove_dir_all(&dir);
    }
}
