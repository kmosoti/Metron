//! Publication of the exact persisted plan on a trusted local filesystem.
use metron_protocol::{domain::ArtifactPlan, types::content_digest};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

fn invalid_plan(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Atomically install a plan once. Existing destination is always a conflict.
pub(crate) fn publish(
    run_dir: &Path,
    plan: &ArtifactPlan,
    synced: &mut dyn FnMut(),
) -> io::Result<()> {
    let bytes = plan
        .candidate_bytes()
        .map_err(|_| invalid_plan("invalid plan"))?;
    if bytes.len() as u64 != plan.content_length.get()
        || content_digest(&bytes) != plan.content_digest
    {
        return Err(invalid_plan("plan content mismatch"));
    }
    let expected = format!(
        "artifacts/{}/{}/result.json",
        plan.intent_id.as_str(),
        plan.dispatch_id.as_str()
    );
    if plan.relative_path.as_str() != expected {
        return Err(invalid_plan("invalid plan path"));
    }
    let artifacts = run_dir.join("artifacts");
    let intent = artifacts.join(plan.intent_id.as_str());
    let dir = intent.join(plan.dispatch_id.as_str());
    fs::create_dir_all(&dir)?;
    // Sync each potentially new directory entry before publishing a durable result.
    for path in [
        run_dir,
        artifacts.as_path(),
        intent.as_path(),
        dir.as_path(),
    ] {
        File::open(path)?.sync_all()?;
    }
    let temp = dir.join(".result.tmp");
    let final_path = dir.join("result.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::hard_link(&temp, &final_path)?;
    File::open(&dir)?.sync_all()?;
    synced();
    fs::remove_file(&temp)?;
    File::open(&dir)?.sync_all()?;
    Ok(())
}

/// Check and resync an existing exact plan, without creating or replacing it.
fn check(run_dir: &Path, plan: &ArtifactPlan, sync: bool) -> io::Result<bool> {
    let expected = format!(
        "artifacts/{}/{}/result.json",
        plan.intent_id.as_str(),
        plan.dispatch_id.as_str()
    );
    if plan.relative_path.as_str() != expected {
        return Err(invalid_plan("invalid plan path"));
    }
    let candidate = plan
        .candidate_bytes()
        .map_err(|_| invalid_plan("invalid plan"))?;
    let expected_len =
        u64::try_from(candidate.len()).map_err(|_| invalid_plan("invalid length"))?;
    if expected_len != plan.content_length.get()
        || content_digest(&candidate) != plan.content_digest
    {
        return Err(invalid_plan("plan content mismatch"));
    }
    let path = run_dir.join(expected);
    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != expected_len {
        return Ok(false);
    }
    let mut bytes = Vec::with_capacity(candidate.len() + 1);
    (&mut file).take(expected_len + 1).read_to_end(&mut bytes)?;
    if bytes != candidate {
        return Ok(false);
    }
    if sync {
        file.sync_all()?;
        File::open(
            path.parent()
                .ok_or_else(|| invalid_plan("missing parent"))?,
        )?
        .sync_all()?;
    }
    Ok(true)
}

pub(crate) fn reconcile(run_dir: &Path, plan: &ArtifactPlan) -> io::Result<bool> {
    check(run_dir, plan, true)
}
pub(crate) fn inspect(run_dir: &Path, plan: &ArtifactPlan) -> io::Result<bool> {
    check(run_dir, plan, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{
        domain::{ApprovedRequest, CandidateResult, Intent, IntentKind, PreparedIntent},
        types::{PrincipalId, SchemaVersion, TaskId, content_digest},
    };
    #[test]
    fn publication_is_no_replace_and_reconcilable() {
        let directory =
            std::env::temp_dir().join(format!("metron-publication-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let intent = Intent {
            schema_version: SchemaVersion::V1,
            task_id: TaskId::new("test").unwrap(),
            ordinal: 2,
            kind: IntentKind::PublishArtifact {
                candidate: CandidateResult {
                    count: 3,
                    sum: 22,
                    input_digest: content_digest(b"fixture"),
                },
            },
        };
        let prepared = PreparedIntent::materialize(
            &intent,
            &PrincipalId::new("test").unwrap(),
            &content_digest(b"genesis"),
        )
        .unwrap();
        let ApprovedRequest::PublishArtifact { plan } = prepared.request else {
            panic!("expected plan")
        };
        let mut calls = 0;
        publish(&directory, &plan, &mut || calls += 1).unwrap();
        assert_eq!(calls, 1);
        assert!(inspect(&directory, &plan).unwrap());
        assert!(reconcile(&directory, &plan).unwrap());
        let mut invalid = plan.clone();
        invalid.content_length = metron_protocol::SqlU64::new(0).unwrap();
        assert_eq!(
            inspect(&directory, &invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            publish(&directory, &plan, &mut || {}).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::write(directory.join(plan.relative_path.as_str()), b"different").unwrap();
        assert!(!inspect(&directory, &plan).unwrap());
        fs::remove_dir_all(directory).unwrap();
    }
}
