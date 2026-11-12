use metron_inference_local::artifact::{
    AUXILIARY_FILES, Artifact, sha256_file, verify_artifact, verify_auxiliary,
};
use metron_protocol::{
    BoundedText, Digest, MAX_ENVELOPE, SchemaVersion, canonical_bytes,
    cli::{CliOutcome, ModelCommand, ModelCommandReport},
    inference::{
        CalibrationFreeze, MAX_QUALIFICATION_REPORT, MAX_SELECTED_PROFILE, SelectedProfile,
        parse_qualification_report,
    },
    parse_envelope,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::ExitCode,
};

fn host_observation(
    sampler: &crate::measurement::ProcessSampler,
) -> std::io::Result<metron_protocol::inference::PhaseMeasurement> {
    use metron_protocol::inference::{MeasurementPhase, MeasurementSource, PhaseMeasurement};
    sampler.sample_now()?;
    let peak = sampler.take_window()?[6];
    let ticks = crate::measurement::clock_ticks_per_second()?;
    let sql = |value: Option<u64>| value.and_then(|v| metron_protocol::SqlU64::new(v).ok());
    Ok(PhaseMeasurement {
        session_id: metron_protocol::content_digest(
            format!("host-{}", std::process::id()).as_bytes(),
        ),
        phase: MeasurementPhase::HostLifetime,
        repetition: 0,
        cpu_source: Some(MeasurementSource::Procfs),
        gpu_source: None,
        cpu_sample_interval_ms: Some(25),
        cpu_clock_ticks_per_second: u32::try_from(ticks).ok(),
        gpu_sample_interval_ms: None,
        peak_rss_bytes: sql(peak.rss_bytes),
        peak_pss_bytes: sql(peak.pss_bytes),
        peak_gpu_bytes: None,
        min_gpu_free_bytes: None,
        cpu_time_ms: if peak.cpu_samples >= 2 {
            sql(crate::measurement::cpu_time_ms(&peak, ticks))
        } else {
            None
        },
    })
}

const RECIPE: &[u8] = include_bytes!("../../metron-testkit/fixtures/prompt-recipe-v1.json");
const CALIBRATION: &[u8] = include_bytes!("../../metron-testkit/fixtures/calibration-v1.json");

fn options(
    args: &[String],
    allowed: &[&str],
) -> Option<std::collections::BTreeMap<String, String>> {
    if args.len() != allowed.len() * 2 {
        return None;
    }
    let mut result = std::collections::BTreeMap::new();
    for pair in args.as_chunks::<2>().0 {
        if !allowed.contains(&pair[0].as_str())
            || pair[1].starts_with("--")
            || result.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return None;
        }
    }
    Some(result)
}
fn read(path: &str, limit: usize) -> Result<Vec<u8>, &'static str> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| "cannot open input")?
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read input")?;
    if bytes.len() > limit {
        return Err("input exceeds bound");
    }
    Ok(bytes)
}
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temp = parent.join(format!(".metron-freeze-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::hard_link(&temp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let _ = fs::remove_file(temp);
    result
}
pub fn execute(args: &[String]) -> ExitCode {
    if args.first().is_some_and(|a| a == "budget") {
        let result = options(&args[1..], &["--input"])
            .ok_or("expected model budget --input FILE".to_string())
            .and_then(|opts| crate::resource_budget::calculate(Path::new(&opts["--input"])));
        return match result {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().is_some_and(|arg| arg == "compare") {
        let args = &args[1..];
        let mut opts = std::collections::BTreeMap::new();
        let (pairs, remainder) = args.as_chunks::<2>();
        if !remainder.is_empty()
            || pairs.iter().any(|p| {
                ![
                    "--worker",
                    "--artifacts",
                    "--output",
                    "--quant",
                    "--concurrency",
                ]
                .contains(&p[0].as_str())
                    || p[1].starts_with("--")
                    || opts.insert(p[0].clone(), p[1].clone()).is_some()
            })
        {
            eprintln!("invalid model compare options");
            return ExitCode::FAILURE;
        }
        let (Some(worker), Some(artifacts), Some(output)) = (
            opts.get("--worker"),
            opts.get("--artifacts"),
            opts.get("--output"),
        ) else {
            eprintln!("model compare requires --worker, --artifacts and --output");
            return ExitCode::FAILURE;
        };
        let concurrency = match opts.get("--concurrency") {
            None => None,
            Some(value) => match value.parse::<u16>() {
                Ok(n @ (1 | 2 | 4 | 8)) => Some(n),
                _ => {
                    eprintln!("invalid model compare concurrency");
                    return ExitCode::FAILURE;
                }
            },
        };
        return match crate::comparison_runner::run(
            Path::new(worker),
            Path::new(artifacts),
            Path::new(output),
            opts.get("--quant").map_or("all", String::as_str),
            concurrency,
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("model comparison failed: {error}");
                ExitCode::FAILURE
            }
        };
    }

    let heldout_mode = args.first().is_some_and(|arg| arg == "qualify-heldout");
    let (command, arguments) = match args.first().map(String::as_str) {
        Some("install") => (ModelCommand::Install, &args[1..]),
        Some("candidate") => (ModelCommand::Candidate, &args[1..]),
        Some("freeze") => (ModelCommand::Freeze, &args[1..]),
        Some("qualify") => (ModelCommand::Qualify, &args[1..]),
        Some("qualify-heldout") => (ModelCommand::Qualify, &args[1..]),
        Some("select") => (ModelCommand::Select, &args[1..]),
        _ => (ModelCommand::Install, args),
    };
    let mut report = ModelCommandReport {
        schema_version: SchemaVersion::V2,
        command,
        outcome: CliOutcome::Succeeded,
        profile_digest: None,
        artifact_digest: None,
        evidence_digest: None,
        diagnostic: None,
    };
    let result: Result<(), (CliOutcome, &'static str)> = (|| {
        match command {
            ModelCommand::Candidate => {
                let opts = options(
                    arguments,
                    &[
                        "--artifact",
                        "--device",
                        "--threads",
                        "--sequences",
                        "--worker",
                        "--model-cache",
                        "--output",
                    ],
                )
                .ok_or((CliOutcome::InvalidInput, "missing candidate input"))?;
                let artifact = match opts["--artifact"].as_str() {
                    "control" => Artifact::Control,
                    "challenger" => Artifact::Challenger,
                    _ => return Err((CliOutcome::InvalidInput, "unknown artifact")),
                };
                let cache = Path::new(&opts["--model-cache"]);
                if fs::symlink_metadata(cache.join("tokenizer.json")).is_ok() {
                    return Err((CliOutcome::IntegrityFailure, "ambiguous root tokenizer"));
                }
                verify_artifact(&cache.join(artifact.filename()), artifact).map_err(|_| {
                    (
                        CliOutcome::IntegrityFailure,
                        "model artifact verification failed",
                    )
                })?;
                let aux = cache
                    .join("auxiliary")
                    .join("1da06bec6a819da10b54c67e5c90972b46d81871");
                for file in AUXILIARY_FILES {
                    verify_auxiliary(&aux.join(file.filename), *file).map_err(|_| {
                        (
                            CliOutcome::IntegrityFailure,
                            "auxiliary verification failed",
                        )
                    })?;
                }
                let cpu = crate::hardware::cpu_identity()
                    .map_err(|_| (CliOutcome::OperationalFailure, "CPU identity unavailable"))?;
                let device = match opts["--device"].as_str() {
                    "cpu" => metron_protocol::inference::LocalDevice::Cpu,
                    value if value.starts_with("cuda:") => {
                        crate::hardware::gpu_identity(&value[5..]).map_err(|_| {
                            (CliOutcome::OperationalFailure, "GPU identity unavailable")
                        })?
                    }
                    _ => return Err((CliOutcome::InvalidInput, "invalid device")),
                };
                let threads = opts["--threads"]
                    .parse::<u8>()
                    .map_err(|_| (CliOutcome::InvalidInput, "invalid thread count"))?;
                let sequences = opts["--sequences"]
                    .parse::<u16>()
                    .map_err(|_| (CliOutcome::InvalidInput, "invalid sequence count"))?;
                let artifact_digest = Digest::new(artifact.sha256())
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid artifact digest"))?;
                let template = AUXILIARY_FILES
                    .iter()
                    .find(|f| f.filename == "chat_template.jinja")
                    .ok_or((CliOutcome::IntegrityFailure, "missing pinned template"))?;
                let profile = metron_protocol::LocalProfile {
                    schema_version: SchemaVersion::V2,
                    auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").map_err(|_| {
                        (CliOutcome::IntegrityFailure, "invalid auxiliary repository")
                    })?,
                    auxiliary_revision: BoundedText::new(
                        "1da06bec6a819da10b54c67e5c90972b46d81871",
                    )
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid auxiliary revision"))?,
                    artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").map_err(
                        |_| (CliOutcome::IntegrityFailure, "invalid artifact repository"),
                    )?,
                    artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2")
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid artifact revision"))?,
                    artifact_name: BoundedText::new(artifact.filename())
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid artifact name"))?,
                    embedded_tokenizer_digest:
                        metron_protocol::inference::gguf_embedded_tokenizer_digest(&artifact_digest)
                            .map_err(|_| {
                                (CliOutcome::IntegrityFailure, "invalid tokenizer identity")
                            })?,
                    artifact_digest: artifact_digest.clone(),
                    artifact_bytes: metron_protocol::SqlU64::new(artifact.size())
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid artifact size"))?,
                    template_digest: Digest::new(template.sha256)
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid template digest"))?,
                    prompt_recipe_digest: metron_protocol::content_digest(RECIPE),
                    backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499")
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid backend revision"))?,
                    backend_build_digest: Digest::new(
                        sha256_file(Path::new(&opts["--worker"]))
                            .map_err(|_| (CliOutcome::OperationalFailure, "cannot hash worker"))?,
                    )
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid worker digest"))?,
                    cpu_identity: BoundedText::new(cpu.identity)
                        .map_err(|_| (CliOutcome::OperationalFailure, "CPU identity too long"))?,
                    cpu_features_digest: cpu.features,
                    device,
                    cpu_threads: threads,
                    context_tokens: 2048,
                    max_input_tokens: 1536,
                    max_output_tokens: 512,
                    temperature_milli: 100,
                    top_k: 50,
                    repeat_penalty_milli: 1100,
                    max_sequences: sequences,
                    queue_capacity: 8,
                    queue_wait_ms: 5000,
                    deadline_ms: 60000,
                    chunked_prefill: false,
                    scoped_prefix_cache: false,
                    cache_target_bytes: None,
                    strict_memory_cap_bytes: None,
                };
                profile.validate().map_err(|_| {
                    (
                        CliOutcome::InvalidInput,
                        "candidate configuration unsupported",
                    )
                })?;
                let bytes = canonical_bytes(&profile, MAX_ENVELOPE)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot encode candidate"))?;
                write_new(Path::new(&opts["--output"]), &bytes)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot write candidate"))?;
                report.profile_digest = Some(
                    profile
                        .digest()
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid profile digest"))?,
                );
                report.artifact_digest = Some(artifact_digest);
            }
            ModelCommand::Install => {
                let opts = options(arguments, &["--artifact", "--cache-dir"]).ok_or((
                    CliOutcome::InvalidInput,
                    "expected --artifact and --cache-dir",
                ))?;
                let artifact = match opts["--artifact"].as_str() {
                    "control" => Artifact::Control,
                    "challenger" => Artifact::Challenger,
                    _ => return Err((CliOutcome::InvalidInput, "unknown artifact")),
                };
                crate::installer::install(artifact, Path::new(&opts["--cache-dir"])).map_err(
                    |_| {
                        (
                            CliOutcome::OperationalFailure,
                            "artifact installation failed",
                        )
                    },
                )?;
                report.artifact_digest = Some(
                    Digest::new(artifact.sha256())
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid artifact digest"))?,
                );
            }
            ModelCommand::Freeze => {
                let opts = options(
                    arguments,
                    &[
                        "--calibration-report",
                        "--cold-report",
                        "--worker",
                        "--output",
                    ],
                )
                .ok_or((
                    CliOutcome::InvalidInput,
                    "expected calibration report, cold report, worker and output",
                ))?;
                let warm = parse_qualification_report(
                    &read(&opts["--calibration-report"], MAX_QUALIFICATION_REPORT)
                        .map_err(|m| (CliOutcome::InvalidInput, m))?,
                )
                .map_err(|_| (CliOutcome::InvalidInput, "invalid calibration report"))?;
                let cold = parse_qualification_report(
                    &read(&opts["--cold-report"], MAX_QUALIFICATION_REPORT)
                        .map_err(|m| (CliOutcome::InvalidInput, m))?,
                )
                .map_err(|_| (CliOutcome::InvalidInput, "invalid cold report"))?;
                let worker = Digest::new(
                    sha256_file(Path::new(&opts["--worker"]))
                        .map_err(|_| (CliOutcome::OperationalFailure, "cannot hash worker"))?,
                )
                .map_err(|_| (CliOutcome::IntegrityFailure, "invalid worker digest"))?;
                let freeze = crate::qualification::freeze_calibration(
                    RECIPE,
                    CALIBRATION,
                    &warm,
                    &cold,
                    &worker,
                )
                .map_err(|m| (CliOutcome::Rejected, m))?;
                let bytes = canonical_bytes(&freeze, MAX_ENVELOPE)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot encode freeze"))?;
                write_new(Path::new(&opts["--output"]), &bytes)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot write freeze"))?;
                report.profile_digest = Some(freeze.profile_digest.clone());
                report.evidence_digest = Some(
                    freeze
                        .digest()
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid freeze digest"))?,
                );
            }
            ModelCommand::Select => {
                let opts = options(
                    arguments,
                    &[
                        "--freeze",
                        "--calibration-report",
                        "--warm-report",
                        "--cold-report",
                        "--heldout",
                        "--worker",
                        "--model-cache",
                        "--output",
                    ],
                )
                .ok_or((CliOutcome::InvalidInput, "missing selection input"))?;
                let parse_report = |path: &str| -> Result<_, (CliOutcome, &'static str)> {
                    parse_qualification_report(
                        &read(path, MAX_QUALIFICATION_REPORT)
                            .map_err(|m| (CliOutcome::InvalidInput, m))?,
                    )
                    .map_err(|_| (CliOutcome::InvalidInput, "invalid qualification report"))
                };
                let calibration_report = parse_report(&opts["--calibration-report"])?;
                let warm_report = parse_report(&opts["--warm-report"])?;
                let cold_report = parse_report(&opts["--cold-report"])?;
                let freeze_bytes = read(&opts["--freeze"], MAX_ENVELOPE)
                    .map_err(|m| (CliOutcome::InvalidInput, m))?;
                let freeze: CalibrationFreeze = parse_envelope(&freeze_bytes)
                    .map_err(|_| (CliOutcome::InvalidInput, "invalid freeze"))?;
                let worker = Digest::new(
                    sha256_file(Path::new(&opts["--worker"]))
                        .map_err(|_| (CliOutcome::OperationalFailure, "cannot hash worker"))?,
                )
                .map_err(|_| (CliOutcome::IntegrityFailure, "invalid worker digest"))?;
                let expected = crate::qualification::freeze_calibration(
                    RECIPE,
                    CALIBRATION,
                    &calibration_report,
                    &cold_report,
                    &worker,
                )
                .map_err(|m| (CliOutcome::Rejected, m))?;
                if expected != freeze
                    || canonical_bytes(&freeze, MAX_ENVELOPE).ok().as_deref() != Some(&freeze_bytes)
                {
                    return Err((
                        CliOutcome::IntegrityFailure,
                        "durable calibration freeze mismatch",
                    ));
                }
                let heldout = read(&opts["--heldout"], MAX_ENVELOPE)
                    .map_err(|m| (CliOutcome::InvalidInput, m))?;
                let profile = warm_report.profile.clone();
                let selected = SelectedProfile {
                    schema_version: SchemaVersion::V2,
                    profile: profile.clone(),
                    recipe_digest: freeze.recipe_digest.clone(),
                    calibration_digest: freeze.calibration_digest.clone(),
                    heldout_digest: freeze.heldout_digest.clone(),
                    calibration_freeze: freeze,
                    cold_report,
                    warm_report,
                };
                crate::qualification::validate_selection(
                    &selected,
                    RECIPE,
                    CALIBRATION,
                    &heldout,
                    &worker,
                )
                .map_err(|m| (CliOutcome::Rejected, m))?;
                crate::hardware::validate_cpu(
                    profile.cpu_identity.as_str(),
                    &profile.cpu_features_digest,
                )
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
                    .ok_or((CliOutcome::IntegrityFailure, "unsupported model artifact"))?;
                let cache = Path::new(&opts["--model-cache"]);
                verify_artifact(&cache.join(artifact.filename()), artifact).map_err(|_| {
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
                let encoded = canonical_bytes(&selected, MAX_SELECTED_PROFILE)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot encode selection"))?;
                write_new(Path::new(&opts["--output"]), &encoded)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot write selection"))?;
                report.profile_digest = Some(
                    profile
                        .digest()
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid profile"))?,
                );
                report.artifact_digest = Some(profile.artifact_digest);
                report.evidence_digest = Some(metron_protocol::content_digest(&encoded));
            }
            ModelCommand::Qualify => {
                let operational_start = std::time::Instant::now();
                let host_sampler = crate::measurement::ProcessSampler::start(std::process::id());
                host_sampler.sample_now().map_err(|_| {
                    (
                        CliOutcome::OperationalFailure,
                        "host measurement unavailable",
                    )
                })?;
                if heldout_mode {
                    let opts = options(
                        arguments,
                        &[
                            "--freeze",
                            "--calibration-report",
                            "--cold-report",
                            "--heldout",
                            "--worker",
                            "--model-cache",
                            "--session-dir",
                            "--output",
                        ],
                    )
                    .ok_or((
                        CliOutcome::InvalidInput,
                        "missing heldout qualification input",
                    ))?;
                    let parse_report = |path: &str| -> Result<_, (CliOutcome, &'static str)> {
                        parse_qualification_report(
                            &read(path, MAX_QUALIFICATION_REPORT)
                                .map_err(|m| (CliOutcome::InvalidInput, m))?,
                        )
                        .map_err(|_| (CliOutcome::InvalidInput, "invalid qualification report"))
                    };
                    let warm = parse_report(&opts["--calibration-report"])?;
                    let cold = parse_report(&opts["--cold-report"])?;
                    let profile = warm.profile.clone();
                    let worker = Digest::new(
                        sha256_file(Path::new(&opts["--worker"]))
                            .map_err(|_| (CliOutcome::OperationalFailure, "cannot hash worker"))?,
                    )
                    .map_err(|_| (CliOutcome::IntegrityFailure, "invalid worker digest"))?;
                    let proof =
                        metron_inference_local::qualification::ValidatedCalibration::from_reports(
                            Path::new(&opts["--freeze"]),
                            RECIPE,
                            CALIBRATION,
                            &warm,
                            &cold,
                            &worker,
                        )
                        .map_err(|_| {
                            (
                                CliOutcome::Rejected,
                                "calibration freeze verification failed",
                            )
                        })?;
                    crate::hardware::validate_cpu(
                        profile.cpu_identity.as_str(),
                        &profile.cpu_features_digest,
                    )
                    .map_err(|_| (CliOutcome::Rejected, "CPU identity differs"))?;
                    crate::hardware::validate_gpu(&profile.device)
                        .map_err(|_| (CliOutcome::Rejected, "GPU identity differs"))?;
                    crate::hardware::preflight(&profile.device, &cold, &warm).map_err(|_| {
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
                        .ok_or((CliOutcome::IntegrityFailure, "unsupported model artifact"))?;
                    let cache = Path::new(&opts["--model-cache"]);
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
                    // Open heldout only after validating the pre-existing freeze and its evidence.
                    let heldout = read(&opts["--heldout"], MAX_ENVELOPE)
                        .map_err(|m| (CliOutcome::InvalidInput, m))?;
                    let mut combined = crate::qualification_runner::heldout(
                        crate::qualification_runner::HeldoutInputs {
                            profile: profile.clone(),
                            worker: Path::new(&opts["--worker"]),
                            artifact: &artifact_path,
                            directory: Path::new(&opts["--session-dir"]),
                            recipe: RECIPE,
                            calibration: CALIBRATION,
                            heldout: &heldout,
                            proof: &proof,
                            warm,
                        },
                    )
                    .map_err(|_| {
                        (
                            CliOutcome::OperationalFailure,
                            "heldout qualification stopped; inspect session journal",
                        )
                    })?;
                    combined
                        .host_measurements
                        .push(host_observation(&host_sampler).map_err(|_| {
                            (
                                CliOutcome::OperationalFailure,
                                "host measurement unavailable",
                            )
                        })?);
                    let elapsed: u64 = operational_start
                        .elapsed()
                        .as_millis()
                        .try_into()
                        .map_err(|_| (CliOutcome::OperationalFailure, "elapsed time overflow"))?;
                    combined.operational_elapsed_ms = Some(
                        metron_protocol::SqlU64::new(
                            combined
                                .operational_elapsed_ms
                                .map_or(0, |v| v.get())
                                .checked_add(elapsed)
                                .ok_or((CliOutcome::OperationalFailure, "elapsed time overflow"))?,
                        )
                        .map_err(|_| (CliOutcome::OperationalFailure, "elapsed time overflow"))?,
                    );
                    let bytes =
                        canonical_bytes(&combined, MAX_QUALIFICATION_REPORT).map_err(|_| {
                            (
                                CliOutcome::OperationalFailure,
                                "cannot encode heldout report",
                            )
                        })?;
                    write_new(Path::new(&opts["--output"]), &bytes).map_err(|_| {
                        (
                            CliOutcome::OperationalFailure,
                            "cannot write heldout report",
                        )
                    })?;
                    report.profile_digest = Some(
                        profile
                            .digest()
                            .map_err(|_| (CliOutcome::IntegrityFailure, "invalid profile"))?,
                    );
                    report.artifact_digest = Some(profile.artifact_digest);
                    report.evidence_digest = Some(metron_protocol::content_digest(&bytes));
                    return Ok(());
                }
                let opts = options(
                    arguments,
                    &[
                        "--model-profile",
                        "--worker",
                        "--model-cache",
                        "--session-dir",
                        "--cold-output",
                        "--warm-output",
                    ],
                )
                .ok_or((CliOutcome::InvalidInput, "missing calibration input"))?;
                let profile: metron_protocol::LocalProfile = parse_envelope(
                    &read(&opts["--model-profile"], MAX_ENVELOPE)
                        .map_err(|m| (CliOutcome::InvalidInput, m))?,
                )
                .map_err(|_| (CliOutcome::InvalidInput, "invalid candidate profile"))?;
                profile
                    .validate()
                    .map_err(|_| (CliOutcome::InvalidInput, "invalid candidate profile"))?;
                let worker = Digest::new(
                    sha256_file(Path::new(&opts["--worker"]))
                        .map_err(|_| (CliOutcome::OperationalFailure, "cannot hash worker"))?,
                )
                .map_err(|_| (CliOutcome::IntegrityFailure, "invalid worker digest"))?;
                if worker != profile.backend_build_digest {
                    return Err((
                        CliOutcome::IntegrityFailure,
                        "worker build differs from profile",
                    ));
                }
                crate::hardware::validate_cpu(
                    profile.cpu_identity.as_str(),
                    &profile.cpu_features_digest,
                )
                .map_err(|_| (CliOutcome::Rejected, "CPU identity differs"))?;
                crate::hardware::validate_gpu(&profile.device)
                    .map_err(|_| (CliOutcome::Rejected, "GPU identity differs"))?;
                crate::hardware::calibration_preflight(&profile).map_err(|_| {
                    (
                        CliOutcome::Rejected,
                        "insufficient estimated resource headroom",
                    )
                })?;
                let artifact = [Artifact::Control, Artifact::Challenger]
                    .into_iter()
                    .find(|a| {
                        a.filename() == profile.artifact_name.as_str()
                            && a.sha256() == profile.artifact_digest.as_str()
                            && a.size() == profile.artifact_bytes.get()
                    })
                    .ok_or((CliOutcome::IntegrityFailure, "unsupported model artifact"))?;
                let cache = Path::new(&opts["--model-cache"]);
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
                // Setup is shared by the command; keep its host sample separate from
                // the two phase windows and report its wall time only once.
                let setup_host = host_observation(&host_sampler).map_err(|_| {
                    (
                        CliOutcome::OperationalFailure,
                        "host measurement unavailable",
                    )
                })?;
                let setup_ms = operational_start.elapsed().as_millis();
                let cold_start = std::time::Instant::now();
                let mut cold_boundary = None;
                let (mut cold, mut warm) = crate::qualification_runner::calibrate(
                    profile.clone(),
                    Path::new(&opts["--worker"]),
                    &artifact_path,
                    Path::new(&opts["--session-dir"]),
                    RECIPE,
                    CALIBRATION,
                    || {
                        let boundary = std::time::Instant::now();
                        let sample = host_observation(&host_sampler)?;
                        cold_boundary = Some((boundary, sample));
                        Ok(())
                    },
                )
                .map_err(|_| {
                    (
                        CliOutcome::OperationalFailure,
                        "calibration stopped; inspect session journal",
                    )
                })?;
                let warm_end = std::time::Instant::now();
                let warm_host = host_observation(&host_sampler).map_err(|_| {
                    (
                        CliOutcome::OperationalFailure,
                        "host measurement unavailable",
                    )
                })?;
                let (boundary, cold_host) = cold_boundary.ok_or((
                    CliOutcome::OperationalFailure,
                    "cold phase boundary unavailable",
                ))?;
                let phase_ms = |duration: std::time::Duration| {
                    let ms: u64 = duration
                        .as_millis()
                        .try_into()
                        .map_err(|_| (CliOutcome::OperationalFailure, "elapsed time overflow"))?;
                    metron_protocol::SqlU64::new(ms)
                        .map_err(|_| (CliOutcome::OperationalFailure, "elapsed time overflow"))
                };
                cold.host_measurements.push(setup_host);
                cold.host_measurements.push(cold_host);
                warm.host_measurements.push(warm_host);
                cold.operational_elapsed_ms = Some(phase_ms(boundary.duration_since(cold_start))?);
                warm.operational_elapsed_ms = Some(phase_ms(warm_end.duration_since(boundary))?);
                eprintln!(
                    "qualification shared setup: {setup_ms} ms; host setup sample recorded separately in cold report"
                );
                let cold_bytes = canonical_bytes(&cold, MAX_QUALIFICATION_REPORT)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot encode cold report"))?;
                let warm_bytes = canonical_bytes(&warm, MAX_QUALIFICATION_REPORT)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot encode warm report"))?;
                write_new(Path::new(&opts["--cold-output"]), &cold_bytes)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot write cold report"))?;
                write_new(Path::new(&opts["--warm-output"]), &warm_bytes)
                    .map_err(|_| (CliOutcome::OperationalFailure, "cannot write warm report"))?;
                report.profile_digest = Some(
                    profile
                        .digest()
                        .map_err(|_| (CliOutcome::IntegrityFailure, "invalid profile"))?,
                );
                report.artifact_digest = Some(profile.artifact_digest);
                report.evidence_digest = Some(metron_protocol::content_digest(&warm_bytes));
            }
        }
        Ok(())
    })();
    if let Err((outcome, message)) = result {
        report.outcome = outcome;
        report.diagnostic = BoundedText::new(message).ok();
    }
    let code = match report.outcome {
        CliOutcome::Succeeded => 0,
        CliOutcome::Rejected => 1,
        CliOutcome::InvalidInput => 2,
        CliOutcome::Denied => 3,
        CliOutcome::Blocked => 4,
        CliOutcome::IntegrityFailure => 5,
        CliOutcome::OperationalFailure => 6,
    };
    match report.encode() {
        Ok(bytes) => println!("{}", String::from_utf8_lossy(&bytes)),
        Err(_) => return ExitCode::from(6),
    }
    if code != 0
        && let Some(message) = report.diagnostic
    {
        eprintln!("{}", message.as_str());
    }
    ExitCode::from(code)
}
