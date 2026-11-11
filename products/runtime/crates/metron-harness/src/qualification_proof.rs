//! Independent evaluation of a selected model profile's recorded responses.
use std::collections::{BTreeMap, BTreeSet};

use metron_protocol::SchemaVersion;
use metron_protocol::{
    Digest, MAX_ENVELOPE, canonical_bytes, content_digest,
    inference::{
        CalibrationFreeze, CorpusSplit, FrozenCase, LocalDevice, LocalOutputSchema, LocalResponse,
        MAX_SELECTED_PROFILE, QualificationReport, SelectedProfile, parse_frozen_corpus,
        parse_local_response, parse_prompt_recipe, parse_selected_profile,
        qualification_calibration_cases_digest, qualification_calibration_warmup_digest,
        qualification_corpora_digest, qualification_report_digest,
    },
};

const RECIPE_SHA: &str = "7b8725c7b6328da6a97f487991ca56ac09b05e484a8d8738384c22645a0931a4";
const CALIBRATION_SHA: &str = "eeb8d45078ae78ef231dfcbd68b413cb784eeab250c95df206584c3c96aa5842";
const HELDOUT_SHA: &str = "6a39feb6e0919ce2dbca7c64005f1cadd38f84a3ac5eab4279ae9032e02a3e5c";

/// Freeze the already evaluated calibration choice before any heldout data is loaded.
pub fn freeze_calibration(
    recipe: &[u8],
    calibration: &[u8],
    warm: &QualificationReport,
    cold: &QualificationReport,
    worker_digest: &Digest,
) -> Result<CalibrationFreeze, &'static str> {
    let recipe_hash = content_digest(recipe);
    let calibration_hash = content_digest(calibration);
    let heldout_hash = Digest::new(HELDOUT_SHA).map_err(|_| "invalid heldout identity")?;
    if recipe_hash.as_str() != RECIPE_SHA
        || calibration_hash.as_str() != CALIBRATION_SHA
        || warm.profile != cold.profile
        || warm.profile.prompt_recipe_digest != recipe_hash
        || warm.profile.backend_build_digest != *worker_digest
        || warm.worker_build_digest != *worker_digest
        || cold.worker_build_digest != *worker_digest
        || warm.evaluator_digest != content_digest(b"metron-local-inference-eval/v1")
        || cold.evaluator_digest != warm.evaluator_digest
        || warm.corpus_digest
            != qualification_corpora_digest(&calibration_hash, &heldout_hash)
                .map_err(|_| "invalid corpus digest")?
        || cold.corpus_digest != warm.corpus_digest
    {
        return Err("calibration provenance mismatch");
    }
    parse_prompt_recipe(recipe).map_err(|_| "invalid frozen recipe")?;
    let corpus = parse_frozen_corpus(calibration).map_err(|_| "invalid calibration corpus")?;
    if corpus.split != CorpusSplit::Calibration || corpus.prompt_recipe_sha256 != recipe_hash {
        return Err("calibration corpus binding mismatch");
    }
    let cases: BTreeMap<_, _> = corpus.cases.iter().map(|c| (case_id(c), c)).collect();
    validate_report(warm, &cases, false)?;
    validate_warmups(warm)?;
    let representatives: BTreeMap<_, _> = cases
        .iter()
        .filter(|(id, _)| **id == "calibration-sum-02" || **id == "calibration-tool-00")
        .map(|(id, case)| (*id, *case))
        .collect();
    if representatives.len() != 2 {
        return Err("missing cold calibration case");
    }
    validate_report(cold, &representatives, true)?;
    Ok(CalibrationFreeze {
        schema_version: SchemaVersion::V2,
        profile_digest: warm.profile.digest().map_err(|_| "invalid profile")?,
        recipe_digest: recipe_hash,
        calibration_digest: calibration_hash,
        heldout_digest: heldout_hash,
        calibration_cases_digest: qualification_calibration_cases_digest(warm)
            .map_err(|_| "invalid calibration cases")?,
        calibration_warmup_digest: qualification_calibration_warmup_digest(warm)
            .map_err(|_| "invalid calibration warmup")?,
        cold_report_digest: qualification_report_digest(cold).map_err(|_| "invalid cold report")?,
    })
}

/// Recompute every verdict from frozen inputs and raw outputs; stored verdicts are not authority.
pub fn validate_selection(
    selection: &SelectedProfile,
    recipe: &[u8],
    calibration: &[u8],
    heldout: &[u8],
    worker_digest: &Digest,
) -> Result<(), &'static str> {
    let encoded =
        canonical_bytes(selection, MAX_SELECTED_PROFILE).map_err(|_| "invalid selected profile")?;
    parse_selected_profile(&encoded).map_err(|_| "invalid selected profile evidence")?;
    let recipe_hash = content_digest(recipe);
    let calibration_hash = content_digest(calibration);
    let heldout_hash = content_digest(heldout);
    if recipe_hash.as_str() != RECIPE_SHA
        || calibration_hash.as_str() != CALIBRATION_SHA
        || heldout_hash.as_str() != HELDOUT_SHA
        || selection.recipe_digest != recipe_hash
        || selection.calibration_digest != calibration_hash
        || selection.heldout_digest != heldout_hash
        || selection.profile.backend_build_digest != *worker_digest
        || selection.cold_report.worker_build_digest != *worker_digest
        || selection.warm_report.worker_build_digest != *worker_digest
        || selection.cold_report.profile != selection.profile
        || selection.warm_report.profile != selection.profile
        || selection.cold_report.evaluator_digest
            != content_digest(b"metron-local-inference-eval/v1")
        || selection.warm_report.evaluator_digest != selection.cold_report.evaluator_digest
        || selection.warm_report.corpus_digest
            != qualification_corpora_digest(&calibration_hash, &heldout_hash)
                .map_err(|_| "invalid corpus digest")?
        || selection.cold_report.corpus_digest != selection.warm_report.corpus_digest
    {
        return Err("qualification provenance mismatch");
    }
    parse_prompt_recipe(recipe).map_err(|_| "invalid frozen recipe")?;
    let calibration = parse_frozen_corpus(calibration).map_err(|_| "invalid calibration corpus")?;
    let heldout = parse_frozen_corpus(heldout).map_err(|_| "invalid heldout corpus")?;
    if calibration.split != CorpusSplit::Calibration
        || heldout.split != CorpusSplit::Heldout
        || calibration.prompt_recipe_sha256 != recipe_hash
        || heldout.prompt_recipe_sha256 != recipe_hash
    {
        return Err("qualification corpus binding mismatch");
    }
    let mut cases = BTreeMap::new();
    for case in calibration.cases.iter().chain(&heldout.cases) {
        let id = case_id(case);
        if cases.insert(id, case).is_some() {
            return Err("duplicate qualification case");
        }
    }
    validate_report(&selection.warm_report, &cases, false)?;
    validate_warmups(&selection.warm_report)?;
    let cold: BTreeMap<_, _> = cases
        .iter()
        .filter(|(id, _)| **id == "calibration-sum-02" || **id == "calibration-tool-00")
        .map(|(id, case)| (*id, *case))
        .collect();
    if cold.len() != 2 {
        return Err("missing cold calibration case");
    }
    validate_report(&selection.cold_report, &cold, true)
}

fn validate_warmups(report: &QualificationReport) -> Result<(), &'static str> {
    if report.warmups.is_empty() || report.warmups.len() > 2 {
        return Err("missing warmup evidence");
    }
    let expected_input = b"{\"values\":[17,-4,9]}";
    let expected_output = b"{\"count\":3,\"sum\":22}";
    for trial in &report.warmups {
        if trial.case_id.as_str() != "calibration-sum-02"
            || trial.repetition != 0
            || trial.input_digest != content_digest(expected_input)
            || trial.expected_digest != content_digest(expected_output)
            || trial.output_schema != LocalOutputSchema::Sum
            || !trial.passed
            || trial.latency_ms.is_none()
            || trial.prompt_tokens.is_none()
            || trial.completion_tokens.is_none()
            || trial.token_ids_digest.is_none()
            || trial.rendered_digest.is_none()
            || trial.prompt_time_us.is_none()
            || trial.completion_time_us.is_none()
            || !tokens_within_profile(report, trial)
            || trial.response.as_ref().and_then(|r| {
                parse_local_response(r.as_str().as_bytes(), LocalOutputSchema::Sum).ok()
            }) != Some(LocalResponse::Sum(
                metron_protocol::inference::SumResponse { count: 3, sum: 22 },
            ))
        {
            return Err("warmup evidence mismatch");
        }
    }
    Ok(())
}

fn case_id(case: &FrozenCase) -> &str {
    match case {
        FrozenCase::Solver { id, .. } | FrozenCase::ToolSelector { id, .. } => id.as_str(),
    }
}

fn validate_report(
    report: &QualificationReport,
    cases: &BTreeMap<&str, &FrozenCase>,
    cold: bool,
) -> Result<(), &'static str> {
    report
        .validate()
        .map_err(|_| "invalid qualification report")?;
    if report.cold != cold || report.cases.len() != cases.len() * 3 {
        return Err("incomplete qualification trials");
    }
    if report.accepted_tasks as usize != report.cases.len()
        || !report.elapsed_ms.is_some_and(|value| value.get() > 0)
        || report.p50_latency_ms.is_none()
        || report.p95_latency_ms.is_none()
    {
        return Err("qualification resource or latency measurement unavailable");
    }
    let session_windows: BTreeSet<_> = report
        .cases
        .iter()
        .map(|case| (case.session_id.as_str(), case.repetition))
        .collect();
    for (session, repetition) in &session_windows {
        let mut whole = report.measurements.iter().filter(|m| {
            m.phase == metron_protocol::inference::MeasurementPhase::WholeRun
                && m.repetition == *repetition
                && m.session_id.as_str() == *session
        });
        let m = whole.next().ok_or("missing whole-run measurement")?;
        if whole.next().is_some()
            || !m.peak_rss_bytes.is_some_and(|v| v.get() > 0)
            || !m.peak_pss_bytes.is_some_and(|v| v.get() > 0)
            || m.cpu_time_ms.is_none()
            || (matches!(report.profile.device, LocalDevice::Cuda { .. })
                && (!m.peak_gpu_bytes.is_some_and(|v| v.get() > 0)
                    || !m
                        .min_gpu_free_bytes
                        .is_some_and(|v| v.get() >= 1024 * 1024 * 1024)))
        {
            return Err("qualification whole-run measurement unavailable");
        }
    }
    let lifetime_records: Vec<_> = report
        .measurements
        .iter()
        .filter(|m| m.phase == metron_protocol::inference::MeasurementPhase::WorkerLifetime)
        .collect();
    let sessions: BTreeSet<_> = session_windows.iter().map(|(id, _)| *id).collect();
    if lifetime_records.len() != sessions.len()
        || sessions.iter().any(|id| {
            lifetime_records
                .iter()
                .filter(|m| m.session_id.as_str() == *id)
                .count()
                != 1
        })
    {
        return Err("worker lifetime sessions mismatch");
    }
    if lifetime_records.iter().any(|m| {
        !m.peak_rss_bytes.is_some_and(|v| v.get() > 0)
            || !m.peak_pss_bytes.is_some_and(|v| v.get() > 0)
            || m.cpu_time_ms.is_none()
            || (matches!(report.profile.device, LocalDevice::Cuda { .. })
                && (!m.peak_gpu_bytes.is_some_and(|v| v.get() > 0)
                    || !m
                        .min_gpu_free_bytes
                        .is_some_and(|v| v.get() >= 1024 * 1024 * 1024)))
    }) {
        return Err("worker lifetime measurement unavailable");
    }
    let mut seen = BTreeSet::new();
    let mut latencies = Vec::with_capacity(report.cases.len());
    for trial in &report.cases {
        let id = trial.case_id.as_str();
        let case = cases.get(id).ok_or("unknown qualification case")?;
        if trial.repetition >= 3 || !seen.insert((id, trial.repetition)) {
            return Err("duplicate qualification trial");
        }
        if trial.latency_ms.is_none()
            || trial.prompt_tokens.is_none()
            || trial.completion_tokens.is_none()
            || trial.token_ids_digest.is_none()
            || trial.rendered_digest.is_none()
            || trial.prompt_time_us.is_none()
            || trial.completion_time_us.is_none()
            || !tokens_within_profile(report, trial)
        {
            return Err("qualification trial measurement unavailable");
        }
        latencies.push(trial.latency_ms.expect("checked above").get());
        let (input, expected, schema) = match case {
            FrozenCase::Solver {
                input, expected, ..
            } => (
                canonical_bytes(input, MAX_ENVELOPE).map_err(|_| "invalid case input")?,
                LocalResponse::Sum(expected.clone()),
                LocalOutputSchema::Sum,
            ),
            FrozenCase::ToolSelector {
                input, expected, ..
            } => (
                canonical_bytes(input, MAX_ENVELOPE).map_err(|_| "invalid case input")?,
                LocalResponse::ToolSelection(expected.clone()),
                LocalOutputSchema::ToolSelection,
            ),
        };
        // The expected digest identifies the exact output object, not the tagged enum.
        let expected_bytes = match &expected {
            LocalResponse::Sum(v) => canonical_bytes(v, MAX_ENVELOPE),
            LocalResponse::ToolSelection(v) => canonical_bytes(v, MAX_ENVELOPE),
            LocalResponse::Text => return Err("text is not a frozen corpus output"),
        }
        .map_err(|_| "invalid expected output")?;
        let raw = trial.response.as_ref().ok_or("missing model response")?;
        if trial.input_digest != content_digest(&input)
            || trial.expected_digest != content_digest(&expected_bytes)
            || trial.output_schema != schema
            || parse_local_response(raw.as_str().as_bytes(), schema).ok() != Some(expected)
            || !trial.passed
        {
            return Err("qualification output does not match frozen expectation");
        }
    }
    if cold {
        let mut sessions = BTreeSet::new();
        for repetition in 0..3 {
            let mut group = report
                .cases
                .iter()
                .filter(|case| case.repetition == repetition);
            let first = group.next().ok_or("missing cold trial")?;
            let second = group.next().ok_or("missing cold trial")?;
            if group.next().is_some()
                || first.session_id != second.session_id
                || !sessions.insert(first.session_id.as_str())
            {
                return Err("cold trials do not use distinct workers");
            }
        }
    } else {
        let calibration_sessions: BTreeSet<_> = report
            .cases
            .iter()
            .filter(|c| c.case_id.as_str().starts_with("calibration-"))
            .map(|c| c.session_id.as_str())
            .collect();
        let heldout_sessions: BTreeSet<_> = report
            .cases
            .iter()
            .filter(|c| !c.case_id.as_str().starts_with("calibration-"))
            .map(|c| c.session_id.as_str())
            .collect();
        if calibration_sessions.len() != 1
            || heldout_sessions.len() > 1
            || report
                .warmups
                .first()
                .is_none_or(|w| !calibration_sessions.contains(w.session_id.as_str()))
            || (!heldout_sessions.is_empty()
                && (report.warmups.len() != 2
                    || report
                        .warmups
                        .get(1)
                        .is_none_or(|w| !heldout_sessions.contains(w.session_id.as_str()))
                    || calibration_sessions == heldout_sessions))
        {
            return Err("warm trials do not match their worker sessions");
        }
    }
    latencies.sort_unstable();
    let percentile = |numerator: usize| latencies[(latencies.len() * numerator).div_ceil(100) - 1];
    if report.p50_latency_ms.map(|v| v.get()) != Some(percentile(50))
        || report.p95_latency_ms.map(|v| v.get()) != Some(percentile(95))
    {
        return Err("qualification latency aggregates mismatch");
    }
    Ok(())
}

fn tokens_within_profile(
    report: &QualificationReport,
    trial: &metron_protocol::inference::QualificationCase,
) -> bool {
    match (trial.prompt_tokens, trial.completion_tokens) {
        (Some(input), Some(output)) => {
            input <= report.profile.max_input_tokens
                && output <= report.profile.max_output_tokens
                && input
                    .checked_add(output)
                    .is_some_and(|total| total <= report.profile.context_tokens)
        }
        _ => false,
    }
}
