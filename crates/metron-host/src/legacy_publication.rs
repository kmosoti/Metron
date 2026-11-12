//! Read-only inspection of a historical artifact on a trusted local filesystem.
use metron_protocol::{domain::ArtifactPlan, types::content_digest};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

fn invalid_plan(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Check an existing exact plan without creating, changing or syncing it.
pub(super) fn inspect(run_dir: &Path, plan: &ArtifactPlan) -> io::Result<bool> {
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
    Ok(bytes == candidate)
}
