//! Runs frozen calibration cases through one-shot qualification permits.
use crate::qualification_capture::Capture;
use metron_harness::qualification::{
    QualificationResult, QualificationSession, SessionKind, SessionSources, ValidatedCalibration,
};
use metron_inference_local::driver::{LocalInferenceDriver, QualificationOutcome};
use metron_protocol::{
    BoundedText, Digest, LocalProfile, MAX_ENVELOPE, SchemaVersion, SqlU64, canonical_bytes,
    content_digest,
    inference::{
        BatchProvenance, FrozenCase, LocalResponse, QualificationCase, QualificationReport,
        parse_frozen_corpus, qualification_corpora_digest,
    },
};
use std::{io, path::Path, time::Instant};

fn error(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub struct HeldoutInputs<'a> {
    pub profile: LocalProfile,
    pub worker: &'a Path,
    pub artifact: &'a Path,
    pub directory: &'a Path,
    pub recipe: &'a [u8],
    pub calibration: &'a [u8],
    pub heldout: &'a [u8],
    pub proof: &'a ValidatedCalibration,
    pub warm: QualificationReport,
}
pub fn heldout(inputs: HeldoutInputs<'_>) -> io::Result<QualificationReport> {
    let HeldoutInputs {
        profile,
        worker,
        artifact,
        directory,
        recipe,
        calibration,
        heldout,
        proof,
        mut warm,
    } = inputs;
    let frozen = parse_frozen_corpus(heldout).map_err(|_| error("invalid heldout corpus"))?;
    let cal = parse_frozen_corpus(calibration).map_err(|_| error("invalid calibration corpus"))?;
    let warmup = cal
        .cases
        .iter()
        .find(|c| matches!(c, FrozenCase::Solver { id, .. } if id.as_str() == "calibration-sum-02"))
        .ok_or_else(|| error("missing representative"))?;
    let id = crate::session_identity::session_id(
        directory,
        &profile,
        "heldout",
        &[recipe, calibration, heldout],
    )?;
    let mut session = QualificationSession::create(
        directory,
        id.clone(),
        SessionKind::Warm,
        profile.clone(),
        SessionSources {
            recipe,
            corpus: heldout,
            calibration,
            calibration_proof: Some(proof),
        },
    )?;
    let (mut capture, sink) = Capture::new(&profile, &id)?;
    let mut driver = LocalInferenceDriver::new(
        worker.to_owned(),
        artifact.to_owned(),
        profile.clone(),
        Some(sink),
    )?;
    warm.warmups.extend(submit(
        &mut session,
        &mut driver,
        &mut capture,
        &id,
        &[Trial {
            case: warmup,
            repetition: 0,
            warmup: true,
        }],
    )?);
    let mut startup = capture.finish_window(0)?;
    startup.phase = metron_protocol::inference::MeasurementPhase::StartupWarmup;
    warm.measurements.push(startup);
    let mut scored_elapsed = std::time::Duration::ZERO;
    for repetition in 0..3 {
        let start = Instant::now();
        for group in frozen.cases.chunks(usize::from(profile.max_sequences)) {
            let trials: Vec<_> = group
                .iter()
                .map(|case| Trial {
                    case,
                    repetition,
                    warmup: false,
                })
                .collect();
            warm.cases.extend(submit(
                &mut session,
                &mut driver,
                &mut capture,
                &id,
                &trials,
            )?);
        }
        scored_elapsed += start.elapsed();
        warm.measurements.push(capture.finish_window(repetition)?);
    }
    driver.shutdown();
    warm.measurements.push(capture.finish_worker(0)?);
    let extra_ms: u64 = scored_elapsed
        .as_millis()
        .try_into()
        .map_err(|_| error("elapsed time overflow"))?;
    warm.elapsed_ms = Some(number(
        warm.elapsed_ms
            .ok_or_else(|| error("missing calibration elapsed"))?
            .get()
            .checked_add(extra_ms)
            .ok_or_else(|| error("elapsed time overflow"))?,
    )?);
    warm.accepted_tasks = warm
        .cases
        .iter()
        .filter(|case| case.response.is_some())
        .count()
        .try_into()
        .map_err(|_| error("too many cases"))?;
    let mut latencies: Vec<_> = warm
        .cases
        .iter()
        .filter_map(|case| case.latency_ms.map(|v| v.get()))
        .collect();
    latencies.sort_unstable();
    let percentile = |p: usize| {
        latencies
            .get((latencies.len() * p).div_ceil(100).saturating_sub(1))
            .copied()
            .and_then(|n| SqlU64::new(n).ok())
    };
    warm.p50_latency_ms = percentile(50);
    warm.p95_latency_ms = percentile(95);
    Ok(warm)
}
fn number(value: u64) -> io::Result<SqlU64> {
    SqlU64::new(value).map_err(|_| error("measurement overflow"))
}
fn expected(case: &FrozenCase) -> io::Result<(String, Digest, LocalResponse)> {
    let (id, bytes, answer) = match case {
        FrozenCase::Solver { id, expected, .. } => (
            id.as_str(),
            canonical_bytes(expected, MAX_ENVELOPE),
            LocalResponse::Sum(expected.clone()),
        ),
        FrozenCase::ToolSelector { id, expected, .. } => (
            id.as_str(),
            canonical_bytes(expected, MAX_ENVELOPE),
            LocalResponse::ToolSelection(expected.clone()),
        ),
    };
    Ok((
        id.to_owned(),
        content_digest(&bytes.map_err(|_| error("invalid expected output"))?),
        answer,
    ))
}

struct Trial<'a> {
    case: &'a FrozenCase,
    repetition: u8,
    warmup: bool,
}
fn submit(
    session: &mut QualificationSession,
    driver: &mut LocalInferenceDriver,
    capture: &mut Capture,
    session_id: &Digest,
    trials: &[Trial<'_>],
) -> io::Result<Vec<QualificationCase>> {
    let cases: Vec<_> = trials
        .iter()
        .map(|t| expected(t.case))
        .collect::<io::Result<_>>()?;
    let requested: Vec<_> = trials
        .iter()
        .zip(&cases)
        .map(|(trial, (id, _, _))| (id.as_str(), trial.repetition, trial.warmup))
        .collect();
    let permits = session.admit(&requested)?;
    let identity: Vec<_> = permits
        .iter()
        .map(|p| {
            let r = p.request();
            (
                r.dispatch_id.clone(),
                r.request_digest.clone(),
                r.action_digest.clone(),
                r.input_digest.clone(),
                r.output_schema,
                r.cohort.clone(),
            )
        })
        .collect();
    let outcomes = driver.execute_qualification_cohort(permits);
    if outcomes.len() != trials.len() {
        return Err(error("worker returned wrong cohort size"));
    }
    let mut settled = Vec::with_capacity(outcomes.len());
    for (i, outcome) in outcomes.iter().enumerate() {
        let (observation, rejection) = match outcome {
            QualificationOutcome::Observed(value) => (Some((**value).clone()), None),
            QualificationOutcome::Rejected(value) => (None, Some(value.clone())),
            QualificationOutcome::Uncertain => (None, None),
        };
        settled.push(QualificationResult {
            dispatch_id: identity[i].0.clone(),
            observation,
            rejection,
        });
    }
    session.settle(&settled)?;
    let mut records = Vec::with_capacity(outcomes.len());
    for (i, outcome) in outcomes.into_iter().enumerate() {
        let (dispatch_id, request_digest, action_digest, input_digest, output_schema, cohort) =
            &identity[i];
        let (content, reasoning, prompt_tokens, completion_tokens, batch, verdict) = match outcome {
            QualificationOutcome::Observed(observation) => {
                let passed = observation.response.as_ref() == Some(&cases[i].2);
                (
                    Some(observation.content.clone()),
                    observation.reasoning.clone(),
                    Some(observation.prompt_tokens),
                    Some(observation.completion_tokens),
                    observation.backend_batch.clone(),
                    passed,
                )
            }
            QualificationOutcome::Rejected(_) => {
                (None, None, None, None, BatchProvenance::Unknown, false)
            }
            QualificationOutcome::Uncertain => {
                return Err(error("qualification dispatch uncertain; session blocked"));
            }
        };
        let timing = capture.take_timing(dispatch_id).ok();
        let verdict = verdict && timing.as_ref().and_then(|t| t.token_count) == prompt_tokens;
        records.push(QualificationCase {
            case_id: BoundedText::new(&cases[i].0).map_err(|_| error("invalid case ID"))?,
            repetition: trials[i].repetition,
            session_id: session_id.clone(),
            dispatch_id: dispatch_id.clone(),
            request_digest: request_digest.clone(),
            action_digest: action_digest.clone(),
            submission_cohort: cohort.clone(),
            backend_batch: batch,
            input_digest: input_digest.clone(),
            expected_digest: cases[i].1.clone(),
            output_schema: *output_schema,
            response_digest: content
                .as_ref()
                .map(|c| content_digest(c.as_str().as_bytes())),
            response: content,
            reasoning,
            latency_ms: timing.as_ref().and_then(|t| number(t.latency_ms).ok()),
            token_ids_digest: timing.as_ref().and_then(|t| t.token_ids_digest.clone()),
            rendered_digest: timing.as_ref().and_then(|t| t.rendered_digest.clone()),
            prompt_time_us: timing
                .as_ref()
                .and_then(|t| t.prompt_time_us)
                .and_then(|n| number(n).ok()),
            completion_time_us: timing
                .as_ref()
                .and_then(|t| t.completion_time_us)
                .and_then(|n| number(n).ok()),
            prompt_tokens,
            completion_tokens,
            passed: verdict,
        });
    }
    Ok(records)
}

pub fn calibrate(
    profile: LocalProfile,
    worker: &Path,
    artifact: &Path,
    directory: &Path,
    recipe: &[u8],
    corpus: &[u8],
    mut cold_finished: impl FnMut() -> io::Result<()>,
) -> io::Result<(QualificationReport, QualificationReport)> {
    let frozen = parse_frozen_corpus(corpus).map_err(|_| error("invalid calibration corpus"))?;
    let session_id = |label: &str| {
        crate::session_identity::session_id(directory, &profile, label, &[recipe, corpus])
    };
    let worker_digest = profile.backend_build_digest.clone();
    let corpus_digest = qualification_corpora_digest(
        &content_digest(corpus),
        &Digest::new("6a39feb6e0919ce2dbca7c64005f1cadd38f84a3ac5eab4279ae9032e02a3e5c")
            .map_err(|_| error("invalid digest"))?,
    )
    .map_err(|_| error("invalid corpus binding"))?;
    let make_report = |cold,
                       cases: Vec<QualificationCase>,
                       warmups,
                       measurements,
                       elapsed: u64|
     -> io::Result<QualificationReport> {
        let mut latencies: Vec<_> = cases
            .iter()
            .filter_map(|c: &QualificationCase| c.latency_ms.map(|v| v.get()))
            .collect();
        latencies.sort_unstable();
        let percentile = |p: usize| {
            latencies
                .get((latencies.len() * p).div_ceil(100).saturating_sub(1))
                .copied()
                .and_then(|n| SqlU64::new(n).ok())
        };
        Ok(QualificationReport {
            schema_version: SchemaVersion::V2,
            evaluator_digest: content_digest(b"metron-local-inference-eval/v1"),
            corpus_digest: corpus_digest.clone(),
            profile: profile.clone(),
            worker_build_digest: worker_digest.clone(),
            cold,
            measurements,
            host_measurements: Vec::new(),
            operational_elapsed_ms: None,
            elapsed_ms: Some(number(elapsed)?),
            accepted_tasks: cases.iter().filter(|c| c.response.is_some()).count() as u32,
            p50_latency_ms: percentile(50),
            p95_latency_ms: percentile(95),
            cases,
            warmups,
        })
    };
    let warmup = frozen
        .cases
        .iter()
        .find(|c| matches!(c, FrozenCase::Solver { id, .. } if id.as_str() == "calibration-sum-02"))
        .ok_or_else(|| error("missing representative"))?;
    let tool = frozen.cases.iter().find(|c| matches!(c, FrozenCase::ToolSelector { id, .. } if id.as_str() == "calibration-tool-00")).ok_or_else(|| error("missing representative"))?;
    let mut cold_cases = Vec::new();
    let mut cold_measurements = Vec::new();
    let cold_start = Instant::now();
    for repetition in 0..3 {
        let id = session_id(&format!("cold-{repetition}"))?;
        let mut session = QualificationSession::create(
            directory,
            id.clone(),
            SessionKind::Cold,
            profile.clone(),
            SessionSources {
                recipe,
                corpus,
                calibration: corpus,
                calibration_proof: None,
            },
        )?;
        let (mut capture, sink) = Capture::new(&profile, &id)?;
        let mut driver = LocalInferenceDriver::new(
            worker.to_owned(),
            artifact.to_owned(),
            profile.clone(),
            Some(sink),
        )?;
        for case in [warmup, tool] {
            cold_cases.extend(submit(
                &mut session,
                &mut driver,
                &mut capture,
                &id,
                &[Trial {
                    case,
                    repetition,
                    warmup: false,
                }],
            )?);
        }
        cold_measurements.push(capture.finish_window(repetition)?);
        driver.shutdown();
        cold_measurements.push(capture.finish_worker(repetition)?);
    }
    let cold = make_report(
        true,
        cold_cases,
        Vec::new(),
        cold_measurements,
        cold_start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    )?;
    cold_finished()?;
    let id = session_id("warm")?;
    let mut session = QualificationSession::create(
        directory,
        id.clone(),
        SessionKind::Warm,
        profile.clone(),
        SessionSources {
            recipe,
            corpus,
            calibration: corpus,
            calibration_proof: None,
        },
    )?;
    let (mut capture, sink) = Capture::new(&profile, &id)?;
    let mut driver = LocalInferenceDriver::new(
        worker.to_owned(),
        artifact.to_owned(),
        profile.clone(),
        Some(sink),
    )?;
    let warmups = submit(
        &mut session,
        &mut driver,
        &mut capture,
        &id,
        &[Trial {
            case: warmup,
            repetition: 0,
            warmup: true,
        }],
    )?;
    let mut startup = capture.finish_window(0)?;
    startup.phase = metron_protocol::inference::MeasurementPhase::StartupWarmup;
    let mut cases = Vec::new();
    let mut measurements = vec![startup];
    let mut scored_elapsed = std::time::Duration::ZERO;
    for repetition in 0..3 {
        let scored_start = Instant::now();
        for group in frozen.cases.chunks(usize::from(profile.max_sequences)) {
            let trials: Vec<_> = group
                .iter()
                .map(|case| Trial {
                    case,
                    repetition,
                    warmup: false,
                })
                .collect();
            cases.extend(submit(
                &mut session,
                &mut driver,
                &mut capture,
                &id,
                &trials,
            )?);
        }
        scored_elapsed += scored_start.elapsed();
        measurements.push(capture.finish_window(repetition)?);
    }
    let warm_elapsed = scored_elapsed.as_millis().try_into().unwrap_or(u64::MAX);
    driver.shutdown();
    measurements.push(capture.finish_worker(0)?);
    let warm = make_report(false, cases, warmups, measurements, warm_elapsed)?;
    Ok((cold, warm))
}
