//! Bounded task fixtures for runtime integration tests.
#![forbid(unsafe_code)]
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
            Checkpoint::EffectReturned => "effect",
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

pub mod coding;
