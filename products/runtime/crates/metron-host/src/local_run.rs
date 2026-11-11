//! Validate a selected profile before any local effect is admitted.
use metron_inference_local::{
    artifact::{AUXILIARY_FILES, Artifact, sha256_file, verify_artifact, verify_auxiliary},
    driver::{EventSink, LocalInferenceDriver, WorkerEvent},
};
use metron_protocol::{
    Digest, MAX_ENVELOPE,
    cli::CliOutcome,
    inference::{MAX_SELECTED_PROFILE, SelectedProfile, parse_selected_profile},
};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

type Failure = (CliOutcome, &'static str);
const RECIPE: &[u8] = include_bytes!("../../metron-testkit/fixtures/prompt-recipe-v1.json");
const CALIBRATION: &[u8] = include_bytes!("../../metron-testkit/fixtures/calibration-v1.json");
fn read(path: &Path, limit: usize) -> Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| (CliOutcome::InvalidInput, "cannot open selected input"))?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| (CliOutcome::OperationalFailure, "cannot read selected input"))?;
    if bytes.len() > limit {
        return Err((CliOutcome::InvalidInput, "selected input exceeds bound"));
    }
    Ok(bytes)
}
pub struct SelectedRun {
    pub selected: SelectedProfile,
    pub worker: PathBuf,
    pub artifact: PathBuf,
}
pub fn load(
    profile_file: &Path,
    worker: &Path,
    cache: &Path,
    heldout: &Path,
) -> Result<SelectedRun, Failure> {
    let selected = parse_selected_profile(&read(profile_file, MAX_SELECTED_PROFILE)?)
        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid selected profile"))?;
    let worker_digest = Digest::new(
        sha256_file(worker).map_err(|_| (CliOutcome::OperationalFailure, "cannot hash worker"))?,
    )
    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid worker digest"))?;
    crate::qualification::validate_selection(
        &selected,
        RECIPE,
        CALIBRATION,
        &read(heldout, MAX_ENVELOPE)?,
        &worker_digest,
    )
    .map_err(|_| {
        (
            CliOutcome::IntegrityFailure,
            "selected qualification is invalid",
        )
    })?;
    let profile = &selected.profile;
    crate::hardware::validate_cpu(profile.cpu_identity.as_str(), &profile.cpu_features_digest)
        .map_err(|_| (CliOutcome::Rejected, "CPU identity differs"))?;
    crate::hardware::validate_gpu(&profile.device)
        .map_err(|_| (CliOutcome::Rejected, "GPU identity differs"))?;
    crate::hardware::preflight(
        &profile.device,
        &selected.cold_report,
        &selected.warm_report,
    )
    .map_err(|_| {
        (
            CliOutcome::Rejected,
            "insufficient measured resource headroom",
        )
    })?;
    let artifact = [Artifact::Control, Artifact::Challenger]
        .into_iter()
        .find(|a| {
            a.filename() == profile.artifact_name.as_str()
                && a.sha256() == profile.artifact_digest.as_str()
                && a.size() == profile.artifact_bytes.get()
        })
        .ok_or((CliOutcome::IntegrityFailure, "unsupported artifact"))?;
    let artifact_path = cache.join(artifact.filename());
    verify_artifact(&artifact_path, artifact).map_err(|_| {
        (
            CliOutcome::IntegrityFailure,
            "model artifact verification failed",
        )
    })?;
    let aux = cache
        .join("auxiliary")
        .join(profile.auxiliary_revision.as_str());
    for file in AUXILIARY_FILES {
        verify_auxiliary(&aux.join(file.filename), *file).map_err(|_| {
            (
                CliOutcome::IntegrityFailure,
                "auxiliary verification failed",
            )
        })?;
    }
    Ok(SelectedRun {
        selected,
        worker: worker.to_path_buf(),
        artifact: artifact_path,
    })
}

struct Guards {
    cpu: Option<crate::measurement::CpuMonitor>,
    gpu: Option<crate::measurement::GpuMonitor>,
}
pub fn driver(run: &SelectedRun) -> Result<LocalInferenceDriver, Failure> {
    let selected = run.selected.clone();
    let admission_check = Arc::new(move || {
        let profile = &selected.profile;
        crate::hardware::validate_cpu(profile.cpu_identity.as_str(), &profile.cpu_features_digest)?;
        crate::hardware::validate_gpu(&profile.device)?;
        crate::hardware::preflight(
            &profile.device,
            &selected.cold_report,
            &selected.warm_report,
        )
    });
    let device = run.selected.profile.device.clone();
    let guards = Arc::new(Mutex::new(Guards {
        cpu: None,
        gpu: None,
    }));
    let sink: EventSink = Arc::new(move |event| {
        if let WorkerEvent::Spawned { pid, control } = event {
            let Ok(mut guards) = guards.lock() else {
                let _ = control.terminate();
                return;
            };
            let cpu_control = control.clone();
            match crate::measurement::CpuMonitor::start(move || {
                let _ = cpu_control.terminate();
            }) {
                Ok(cpu) => guards.cpu = Some(cpu),
                Err(_) => {
                    let _ = control.terminate();
                    return;
                }
            }
            if let metron_protocol::inference::LocalDevice::Cuda { uuid, .. } = &device {
                let gpu_control = control.clone();
                match crate::measurement::GpuMonitor::start(uuid.as_str(), pid, move || {
                    let _ = gpu_control.terminate();
                }) {
                    Ok(gpu) => guards.gpu = Some(gpu),
                    Err(_) => {
                        let _ = control.terminate();
                    }
                }
            }
        }
    });
    LocalInferenceDriver::new(
        run.worker.clone(),
        run.artifact.clone(),
        run.selected.profile.clone(),
        Some(sink),
    )
    .map(|driver| driver.with_admission_check(admission_check))
    .map_err(|_| {
        (
            CliOutcome::InvalidInput,
            "invalid local worker configuration",
        )
    })
}
