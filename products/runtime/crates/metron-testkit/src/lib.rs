//! Bounded task fixtures for runtime integration tests.
#![forbid(unsafe_code)]
use metron_protocol::{
    domain::{BudgetRequest, Task, TaskData},
    types::{BoundedText, PrincipalId, SchemaVersion, SqlU64, TaskId, ValidationError},
};

/// A minimal sum-v1 task with exactly enough budget for its three effects.
pub fn sum_task(task_id: TaskId, principal_id: PrincipalId) -> Result<Task, ValidationError> {
    Task::new(TaskData {
        schema_version: SchemaVersion::V1,
        task_id,
        principal_id,
        objective: BoundedText::new("Sum the fixture values")?,
        budget: BudgetRequest {
            max_model_requests: SqlU64::new(1)?,
            max_tool_calls: SqlU64::new(2)?,
            max_spend_micros_usd: SqlU64::new(0)?,
            max_elapsed_ms: None,
        },
        max_decision_steps: 8,
    })
}

/// An observer for child-process tests. It announces a production checkpoint and
/// blocks there until the parent terminates the child.
pub struct FileBarrier {
    marker: std::path::PathBuf,
    target: String,
}
impl FileBarrier {
    pub fn new(marker: std::path::PathBuf, target: String) -> Self {
        Self { marker, target }
    }
}
impl metron_harness::orchestrator::Observer for FileBarrier {
    fn checkpoint(&mut self, point: metron_harness::orchestrator::Checkpoint) {
        use metron_harness::orchestrator::Checkpoint;
        let name = match point {
            Checkpoint::OutboxCommitted => "outbox",
            Checkpoint::AdmissionCommitted => "admission",
            Checkpoint::PublicationSynced => "publication",
            Checkpoint::ReceiptCommitted => "receipt",
            Checkpoint::VerificationCommitted => "verification",
        };
        if name == self.target {
            std::fs::write(&self.marker, name).expect("write test checkpoint");
            loop {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}
