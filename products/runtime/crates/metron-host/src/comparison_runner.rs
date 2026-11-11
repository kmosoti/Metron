//! Accounted, local operational comparison; does not select a production profile.
use crate::{hardware, measurement::GpuMonitor};
use metron_harness::comparison::{ComparisonCase, ComparisonResult, ComparisonSession};
use metron_inference_local::{
    artifact::{self, AUXILIARY_FILES, Artifact},
    driver::{EventSink, LocalInferenceDriver, QualificationOutcome, WorkerEvent},
};
use metron_protocol::{
    BoundedText, Digest, LOCAL_TEAM_RECIPE_SHA, LocalDevice, LocalObservation, LocalOutputSchema,
    LocalProfile, LocalResponse, LocalRole, PrincipalId, Receipt, SchemaVersion, SqlU64, TaskId,
    gguf_embedded_tokenizer_digest,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs, io,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn err(message: impl std::fmt::Display) -> io::Error {
    io::Error::other(message.to_string())
}
#[derive(Default)]
struct Timing {
    submitted_at: Option<Instant>,
    started: Option<Instant>,
    dispatch_begin: Option<Instant>,
    sent: Option<Instant>,
    first_content: Option<Instant>,
    completed: Option<Instant>,
    tokens: Option<u32>,
    arrived_in_flight: Option<Vec<Digest>>,
}
#[derive(Default)]
struct Events {
    timings: BTreeMap<Digest, Timing>,
    gpu: Option<GpuMonitor>,
    error: Option<String>,
    loaded: Option<Instant>,
    spawned: Option<Instant>,
}
#[derive(Serialize)]
struct Record {
    id: String,
    context: String,
    repetition: u8,
    warmup: bool,
    staggered: bool,
    scheduled_arrival_offset_ms: u64,
    dispatch_id: Digest,
    observation: Option<LocalObservation>,
    rejection: Option<String>,
    receipt: Option<Receipt>,
    tokenized: Option<u32>,
    arrived_in_flight: Option<Vec<Digest>>,
    elapsed_ms: Option<u64>,
    started_ms: Option<u64>,
    dispatch_begin_ms: Option<u64>,
    sent_ms: Option<u64>,
    #[serde(skip)]
    started_at: Option<Instant>,
    #[serde(skip)]
    completed_at: Option<Instant>,
    #[serde(skip)]
    dispatch_begin_at: Option<Instant>,
    #[serde(skip)]
    sent_at: Option<Instant>,
    usable_ms: Option<u64>,
    verdict: String,
}
#[derive(Serialize)]
struct Report {
    quant: String,
    concurrency: u16,
    profile: LocalProfile,
    latency_definition: &'static str,
    startup_load_ms: Option<u64>,
    warmup_ms: Option<u64>,
    scored_ms: u64,
    peak_gpu_bytes: Option<u64>,
    min_gpu_free_bytes: Option<u64>,
    records: Vec<Record>,
    failure: Option<String>,
    trial_throughput: Vec<f64>,
    p95_reasoner_short: Option<u64>,
    p95_reasoner_near: Option<u64>,
    p95_selector_short: Option<u64>,
    p95_selector_near: Option<u64>,
    qualified: bool,
    ideal: bool,
    completed: bool,
    stagger_overlap: bool,
    transport_overlap: bool,
}
fn has_visible_content(text: &str) -> bool {
    text.chars().any(|c| {
        !c.is_whitespace()
            && !c.is_control()
            && !matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}')
    })
}
fn scheduled_arrival(base: Instant, stagger: Duration, position: u32) -> Instant {
    base + stagger * position
}
fn transport_overlap(
    reasoner_started: Option<Instant>,
    dispatch_begin: Option<Instant>,
    sent: Option<Instant>,
    reasoner_completed: Option<Instant>,
) -> bool {
    matches!((reasoner_started, dispatch_begin, sent, reasoner_completed), (Some(start), Some(begin), Some(ack), Some(end)) if start <= begin && begin <= ack && ack < end)
}
fn stagger_overlap(reasoner_id: &Digest, selector_snapshot: Option<&[Digest]>) -> bool {
    selector_snapshot.is_some_and(|ids| ids.contains(reasoner_id))
}
fn ms(a: Instant, b: Instant) -> u64 {
    u64::try_from(b.saturating_duration_since(a).as_millis()).unwrap_or(u64::MAX)
}
// Round start times upward so a fractional millisecond past the arrival boundary
// cannot be mistaken for overlap.
fn started_ms(a: Instant, b: Instant) -> u64 {
    let nanos = b.saturating_duration_since(a).as_nanos();
    u64::try_from(nanos.div_ceil(1_000_000)).unwrap_or(u64::MAX)
}
fn case(id: usize, near: bool) -> io::Result<ComparisonCase> {
    let (name, role, output_schema, text) = match id {
        0 => (
            "math",
            LocalRole::Reasoner,
            LocalOutputSchema::Text,
            "Compute 17 - 4 + 9. Return only the integer.",
        ),
        1 => (
            "function",
            LocalRole::Reasoner,
            LocalOutputSchema::Text,
            "Write only a Rust function `fn sum_i32(values: &[i32]) -> i32` that uses Iterator::sum. No other items.",
        ),
        _ => (
            "selector",
            LocalRole::ToolSelector,
            LocalOutputSchema::ToolSelection,
            "{\"stage\":\"need_input\",\"note\":\"public untrusted data\"}",
        ),
    };
    let padding = if near {
        format!("\nUntrusted padding, ignore: {}", "x ".repeat(7900))
    } else {
        String::new()
    };
    // Selector input remains valid JSON even when padded.
    let context = if id == 2 && near {
        format!(
            "{{\"stage\":\"need_input\",\"note\":\"{}\"}}",
            "x ".repeat(7900)
        )
    } else {
        format!("{text}{padding}")
    };
    Ok(ComparisonCase {
        id: BoundedText::new(name).map_err(err)?,
        role,
        output_schema,
        context: BoundedText::new(context).map_err(err)?,
    })
}
fn warmup_cohorts(concurrency: u16) -> Vec<Vec<(usize, bool)>> {
    let exercise = if concurrency == 1 {
        vec![(2, false)]
    } else {
        (0..usize::from(concurrency))
            .map(|position| (if position % 2 == 0 { 0 } else { 2 }, false))
            .collect()
    };
    vec![vec![(0, false)], exercise]
}
fn warmup_ready(schema: LocalOutputSchema, observation: &LocalObservation) -> bool {
    !observation.content.as_str().is_empty()
        && matches!(
            (schema, &observation.response),
            (LocalOutputSchema::Text, Some(LocalResponse::Text))
                | (
                    LocalOutputSchema::ToolSelection,
                    Some(LocalResponse::ToolSelection(_))
                )
                | (LocalOutputSchema::Sum, Some(LocalResponse::Sum(_)))
        )
}
fn verdict(id: &str, o: &LocalObservation) -> &'static str {
    match id {
        "math" => {
            if o.content.as_str().trim() == "22" {
                "correct"
            } else {
                "incorrect"
            }
        }
        "selector" => {
            if matches!(&o.response, Some(LocalResponse::ToolSelection(v)) if v.tool == metron_protocol::inference::ToolChoice::ReadFixture)
            {
                "correct"
            } else {
                "incorrect"
            }
        }
        _ => {
            let s = o.content.as_str().trim();
            let s = if let Some(s) = s
                .strip_prefix("```rust\n")
                .or_else(|| s.strip_prefix("```\n"))
            {
                s.strip_suffix("```").unwrap_or(s).trim()
            } else {
                s
            };
            let compact: String = s.chars().filter(|c| !c.is_whitespace()).collect();
            let valid = [
                "fn sum_i32(values: &[i32]) -> i32 { values.iter().sum() }",
                "fn sum_i32(values: &[i32]) -> i32 { values.iter().copied().sum() }",
                "fn sum_i32(values: &[i32]) -> i32 { values.iter().sum::<i32>() }",
                "fn sum_i32(values: &[i32]) -> i32 { values.iter().copied().sum::<i32>() }",
            ];
            if valid
                .iter()
                .any(|v| v.chars().filter(|c| !c.is_whitespace()).eq(compact.chars()))
            {
                "correct"
            } else {
                "review_required"
            }
        }
    }
}
fn profile(
    artifact: Artifact,
    worker: &Path,
    device: LocalDevice,
    concurrency: u16,
) -> io::Result<LocalProfile> {
    let cpu = hardware::cpu_identity()?;
    let digest = Digest::new(artifact.sha256()).map_err(err)?;
    let template = AUXILIARY_FILES
        .iter()
        .find(|f| f.filename == "chat_template.jinja")
        .ok_or_else(|| err("missing template"))?;
    let p = LocalProfile {
        schema_version: SchemaVersion::V2,
        auxiliary_repository: BoundedText::new("LiquidAI/LFM3-1.7B").map_err(err)?,
        auxiliary_revision: BoundedText::new("1da06bec6a819da10b54c67e5c90972b46d81871")
            .map_err(err)?,
        artifact_repository: BoundedText::new("LiquidAI/LFM3-1.7B-GGUF").map_err(err)?,
        artifact_revision: BoundedText::new("cd0c74c575b5c416f3011b5b4c298077f26ca8c2")
            .map_err(err)?,
        artifact_name: BoundedText::new(artifact.filename()).map_err(err)?,
        artifact_bytes: SqlU64::new(artifact.size()).map_err(err)?,
        embedded_tokenizer_digest: gguf_embedded_tokenizer_digest(&digest).map_err(err)?,
        artifact_digest: digest,
        template_digest: Digest::new(template.sha256).map_err(err)?,
        prompt_recipe_digest: Digest::new(LOCAL_TEAM_RECIPE_SHA).map_err(err)?,
        backend_revision: BoundedText::new("fb4cdc3730dc64b366724e7637a205a8b7747499")
            .map_err(err)?,
        backend_build_digest: Digest::new(artifact::sha256_file(worker)?).map_err(err)?,
        cpu_identity: BoundedText::new(cpu.identity).map_err(err)?,
        cpu_features_digest: cpu.features,
        device,
        cpu_threads: 2,
        context_tokens: 8192,
        max_input_tokens: 8064,
        max_output_tokens: 128,
        temperature_milli: 100,
        top_k: 50,
        repeat_penalty_milli: 1100,
        max_sequences: concurrency,
        queue_capacity: 8,
        queue_wait_ms: 5000,
        deadline_ms: 60000,
        chunked_prefill: false,
        scoped_prefix_cache: false,
        cache_target_bytes: None,
        strict_memory_cap_bytes: None,
    };
    p.validate().map_err(err)?;
    Ok(p)
}
fn submit(
    session: &mut ComparisonSession,
    driver: &mut LocalInferenceDriver,
    events: &Arc<Mutex<Events>>,
    requests: &[(usize, bool)],
    rep: u8,
    warmup: bool,
    stagger: Duration,
) -> io::Result<Vec<Record>> {
    let cases = requests
        .iter()
        .map(|(id, near)| case(*id, *near))
        .collect::<io::Result<Vec<_>>>()?;
    let arrival_base = Instant::now();
    let permits = session.admit(&cases)?;
    let identities: Vec<_> = permits
        .iter()
        .map(|p| p.request().dispatch_id.clone())
        .collect();
    {
        let mut state = events.lock().map_err(err)?;
        for (position, id) in identities.iter().enumerate() {
            let position = u32::try_from(position).map_err(err)?;
            state.timings.entry(id.clone()).or_default().submitted_at =
                Some(scheduled_arrival(arrival_base, stagger, position));
        }
    }
    let outcomes = driver.execute_comparison_cohort(permits, stagger);
    if outcomes.len() != cases.len() {
        return Err(err("wrong comparison outcome count"));
    }
    let results: Vec<_> = outcomes
        .iter()
        .zip(&identities)
        .map(|(o, id)| {
            let (observation, rejection) = match o {
                QualificationOutcome::Observed(v) => (Some((**v).clone()), None),
                QualificationOutcome::Rejected(v) => (None, Some(v.clone())),
                QualificationOutcome::Uncertain => (None, None),
            };
            ComparisonResult {
                dispatch_id: id.clone(),
                observation,
                rejection,
            }
        })
        .collect();
    let receipts = session.settle(&results)?;
    let mut state = events.lock().map_err(err)?;
    if let Some(message) = &state.error {
        return Err(err(message.clone()));
    }
    let mut records = Vec::new();
    for (position, (((case, (id, near)), result), receipt)) in cases
        .iter()
        .zip(requests)
        .zip(results)
        .zip(receipts)
        .enumerate()
    {
        let timing = state
            .timings
            .remove(&result.dispatch_id)
            .unwrap_or_default();
        let (mut verdict, usable) = match &result.observation {
            Some(observation) => {
                let correctness = if warmup {
                    if warmup_ready(case.output_schema, observation) {
                        "warmup_ready"
                    } else {
                        "warmup_failed"
                    }
                } else {
                    verdict(case.id.as_str(), observation)
                };
                let usable = if *id == 2 {
                    if matches!(observation.response, Some(LocalResponse::ToolSelection(_))) {
                        timing.completed
                    } else {
                        None
                    }
                } else {
                    timing.first_content
                };
                (correctness.to_owned(), usable)
            }
            None if warmup => ("warmup_failed".into(), None),
            None if result.rejection.is_some() => ("rejected".into(), None),
            None => ("uncertain".into(), None),
        };
        if warmup && receipt.is_none() {
            verdict = "warmup_failed".into();
        }
        records.push(Record {
            id: case.id.as_str().into(),
            context: if *near { "near_8k" } else { "short" }.into(),
            repetition: rep,
            warmup,
            staggered: !stagger.is_zero(),
            scheduled_arrival_offset_ms: u64::try_from(
                (stagger * u32::try_from(position).map_err(err)?).as_millis(),
            )
            .map_err(err)?,
            dispatch_id: result.dispatch_id,
            observation: result.observation,
            rejection: result.rejection.map(|r| r.as_str().to_owned()),
            receipt,
            tokenized: timing.tokens,
            arrived_in_flight: timing.arrived_in_flight,
            elapsed_ms: timing
                .submitted_at
                .zip(timing.completed)
                .map(|(a, b)| ms(a, b)),
            started_ms: timing
                .submitted_at
                .zip(timing.started)
                .map(|(a, b)| started_ms(a, b)),
            dispatch_begin_ms: timing
                .submitted_at
                .zip(timing.dispatch_begin)
                .map(|(a, b)| ms(a, b)),
            sent_ms: timing.submitted_at.zip(timing.sent).map(|(a, b)| ms(a, b)),
            started_at: timing.started,
            completed_at: timing.completed,
            dispatch_begin_at: timing.dispatch_begin,
            sent_at: timing.sent,
            usable_ms: timing.submitted_at.zip(usable).map(|(a, b)| ms(a, b)),
            verdict,
        });
    }
    Ok(records)
}
fn percentile(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    values
        .get((values.len() * 95).div_ceil(100).saturating_sub(1))
        .copied()
}
fn save(path: &Path, report: &Report) -> io::Result<()> {
    let data = serde_json::to_vec_pretty(report).map_err(err)?;
    fs::write(path, data)
}
fn run_profile(
    worker: &Path,
    artifacts: &Path,
    output: &Path,
    quant: &str,
    artifact: Artifact,
    concurrency: u16,
    device: LocalDevice,
) -> io::Result<Report> {
    let p = profile(artifact, worker, device.clone(), concurrency)?;
    artifact::verify_artifact(&artifacts.join(artifact.filename()), artifact)?;
    for f in AUXILIARY_FILES {
        artifact::verify_auxiliary(
            &artifacts
                .join("auxiliary/1da06bec6a819da10b54c67e5c90972b46d81871")
                .join(f.filename),
            *f,
        )?;
    }
    // Bind the versioned case manifest to the actual short and padded inputs.
    let mut manifest = Vec::new();
    for near in [false, true] {
        for id in 0..3 {
            let case = case(id, near)?;
            manifest.push((case.id, case.role, case.output_schema, case.context));
        }
    }
    let manifest = metron_protocol::canonical_bytes(
        &("comparison-cases/v1", manifest),
        metron_protocol::MAX_QUALIFICATION_REPORT,
    )
    .map_err(err)?;
    let session_id = crate::session_identity::session_id(
        output,
        &p,
        "comparison",
        &[LOCAL_TEAM_RECIPE_SHA.as_bytes(), &manifest],
    )?;
    let mut session = ComparisonSession::create(
        output,
        session_id,
        TaskId::new(format!("compare-{quant}-{concurrency}")).map_err(err)?,
        PrincipalId::new("comparison").map_err(err)?,
        p.clone(),
        1024,
    )?;
    let events = Arc::new(Mutex::new(Events::default()));
    let shared = Arc::clone(&events);
    let sink: EventSink = Arc::new(move |event| {
        let Ok(mut state) = shared.lock() else {
            return;
        };
        match event {
            WorkerEvent::Spawned { pid, control } => {
                state.spawned = Some(Instant::now());
                if let LocalDevice::Cuda { uuid, .. } = &device {
                    match GpuMonitor::start(uuid.as_str(), pid, move || {
                        let _ = control.terminate();
                    }) {
                        Ok(monitor) => state.gpu = Some(monitor),
                        Err(e) => state.error = Some(e.to_string()),
                    }
                }
            }

            WorkerEvent::Dispatching {
                dispatch_id,
                began_at,
            } => state.timings.entry(dispatch_id).or_default().dispatch_begin = Some(began_at),
            WorkerEvent::Sent { dispatch_id } => {
                state.timings.entry(dispatch_id).or_default().sent = Some(Instant::now())
            }
            WorkerEvent::Arrived {
                dispatch_id,
                in_flight,
            } => {
                state
                    .timings
                    .entry(dispatch_id)
                    .or_default()
                    .arrived_in_flight = Some(in_flight);
            }
            WorkerEvent::Loaded { .. } => state.loaded = Some(Instant::now()),
            WorkerEvent::Started { dispatch_id } => {
                state.timings.entry(dispatch_id).or_default().started = Some(Instant::now())
            }
            WorkerEvent::Tokenized {
                dispatch_id,
                token_count,
                ..
            } => state.timings.entry(dispatch_id).or_default().tokens = Some(token_count),
            WorkerEvent::Delta {
                dispatch_id,
                text,
                reasoning,
                received_at,
                ..
            } => {
                if !reasoning && has_visible_content(text.as_str()) {
                    let t = state.timings.entry(dispatch_id).or_default();
                    if t.first_content.is_none() {
                        t.first_content = Some(received_at);
                    }
                }
            }
            WorkerEvent::Completed {
                dispatch_id,
                received_at,
                ..
            } => state.timings.entry(dispatch_id).or_default().completed = Some(received_at),
            _ => {}
        }
    });
    let mut driver = LocalInferenceDriver::new(
        worker.to_owned(),
        artifacts.join(artifact.filename()),
        p.clone(),
        Some(sink),
    )?;
    let mut report = Report {
        quant: quant.into(),
        concurrency,
        profile: p,
        latency_definition: "Milliseconds from scheduled arrival before cohort admission (base plus stagger times position) through first visible content for reasoners or schema-valid final proposal for selectors; elapsed ends at completion.",
        startup_load_ms: None,
        warmup_ms: None,
        scored_ms: 0,
        peak_gpu_bytes: None,
        min_gpu_free_bytes: None,
        records: Vec::new(),
        failure: None,
        trial_throughput: Vec::new(),
        p95_reasoner_short: None,
        p95_reasoner_near: None,
        p95_selector_short: None,
        p95_selector_near: None,
        qualified: false,
        ideal: false,
        completed: false,
        stagger_overlap: false,
        transport_overlap: false,
    };
    let path = output.join(format!("comparison-{quant}-{concurrency}.json"));
    let result: io::Result<()> = (|| {
        let cohorts = warmup_cohorts(concurrency);
        for cohort in cohorts {
            let warm = submit(
                &mut session,
                &mut driver,
                &events,
                &cohort,
                0,
                true,
                Duration::ZERO,
            )?;
            let failed = warm
                .iter()
                .any(|r| r.verdict != "warmup_ready" || r.receipt.is_none());
            report.records.extend(warm);
            if failed {
                return Err(err("accounted warmup failed"));
            }
        }
        report.warmup_ms = events
            .lock()
            .map_err(err)?
            .loaded
            .map(|loaded| ms(loaded, Instant::now()));
        check_gpu(&events, &mut report)?;
        for rep in 0..3 {
            let trial_start = Instant::now();
            let before = report.records.len();
            for near in [false, true] {
                for round in 0..3 {
                    let cases: Vec<_> = (0..usize::from(concurrency))
                        .map(|pos| ((round + pos) % 3, near))
                        .collect();
                    report.records.extend(submit(
                        &mut session,
                        &mut driver,
                        &events,
                        &cases,
                        rep,
                        false,
                        Duration::ZERO,
                    )?);
                    check_gpu(&events, &mut report)?;
                    save(&path, &report)?;
                }
            }
            let elapsed = trial_start.elapsed();
            report.scored_ms = report
                .scored_ms
                .saturating_add(elapsed.as_millis().try_into().unwrap_or(u64::MAX));
            let completed = report.records[before..]
                .iter()
                .filter(|r| r.observation.is_some())
                .count();
            report
                .trial_throughput
                .push(completed as f64 / elapsed.as_secs_f64());
        }
        if concurrency >= 2 {
            let pair = submit(
                &mut session,
                &mut driver,
                &events,
                &[(0, true), (2, false)],
                0,
                false,
                Duration::from_millis(100),
            )?;
            if let [reasoner, selector] = pair.as_slice() {
                report.stagger_overlap =
                    stagger_overlap(&reasoner.dispatch_id, selector.arrived_in_flight.as_deref());
                report.transport_overlap = transport_overlap(
                    reasoner.started_at,
                    selector.dispatch_begin_at,
                    selector.sent_at,
                    reasoner.completed_at,
                );
            }
            report.records.extend(pair);
            check_gpu(&events, &mut report)?;
        }
        report.completed = true;
        Ok(())
    })();
    if let Err(e) = result {
        report.failure = Some(e.to_string());
    }
    if let Err(error) = check_gpu(&events, &mut report) {
        let detail = format!("GPU capture failed: {error}");
        report.failure = Some(match report.failure.take() {
            Some(original) => format!("{original}; {detail}"),
            None => detail,
        });
    }
    if let Ok(state) = events.lock() {
        report.startup_load_ms = state.spawned.zip(state.loaded).map(|(a, b)| ms(a, b));
    }
    driver.shutdown();
    for (role, near) in [(false, false), (false, true), (true, false), (true, true)] {
        let vals: Vec<_> = report
            .records
            .iter()
            .filter(|r| {
                !r.warmup && (r.id == "selector") == role && (r.context == "near_8k") == near
            })
            .filter_map(|r| r.usable_ms)
            .collect();
        let p95 = percentile(vals);
        match (role, near) {
            (false, false) => report.p95_reasoner_short = p95,
            (false, true) => report.p95_reasoner_near = p95,
            (true, false) => report.p95_selector_short = p95,
            (true, true) => report.p95_selector_near = p95,
        }
    }
    if report.failure.is_none()
        && report.records.iter().any(|r| {
            r.context == "near_8k" && !r.tokenized.is_some_and(|n| (7900..=8064).contains(&n))
        })
    {
        report.failure =
            Some("near-8K rendered token count outside 7900..8064 or unavailable".into());
    }
    let latency = [
        report.p95_reasoner_short,
        report.p95_reasoner_near,
        report.p95_selector_short,
        report.p95_selector_near,
    ];
    report.qualified = report.failure.is_none()
        && concurrency >= 2
        && report.stagger_overlap
        && report.trial_throughput.len() == 3
        && report.records.iter().filter(|r| !r.warmup).all(|r| {
            r.verdict == "correct"
                && r.receipt.is_some()
                && (r.context != "near_8k"
                    || r.tokenized.is_some_and(|n| (7900..=8064).contains(&n)))
        })
        && report
            .peak_gpu_bytes
            .is_some_and(|n| n <= 4096 * 1024 * 1024)
        && report
            .min_gpu_free_bytes
            .is_some_and(|n| n >= 1024 * 1024 * 1024)
        && latency.iter().all(|v| v.is_some_and(|n| n < 1000));
    report.ideal = report.qualified && latency.iter().all(|v| v.is_some_and(|n| n < 500));
    save(&path, &report)?;
    Ok(report)
}
fn check_gpu(events: &Arc<Mutex<Events>>, report: &mut Report) -> io::Result<()> {
    let state = events.lock().map_err(err)?;
    let gpu = state
        .gpu
        .as_ref()
        .ok_or_else(|| err("GPU monitor unavailable"))?;
    let current_sample = gpu.sample_now();
    let sample = gpu.take_window()?;
    report.peak_gpu_bytes = report.peak_gpu_bytes.max(sample.maximum_process_bytes);
    report.min_gpu_free_bytes = match (report.min_gpu_free_bytes, sample.minimum_free_bytes) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    current_sample?;
    if sample.failure.is_some()
        || report
            .peak_gpu_bytes
            .is_some_and(|n| n > 4096 * 1024 * 1024)
    {
        return Err(err("GPU memory boundary exceeded or unavailable"));
    }
    Ok(())
}
#[derive(Serialize)]
struct Summary {
    chosen: Option<String>,
    reason: String,
    profiles: Vec<String>,
    failures: Vec<String>,
}
pub fn run(
    worker: &Path,
    artifacts: &Path,
    output: &Path,
    quant: &str,
    concurrency_filter: Option<u16>,
) -> io::Result<()> {
    fs::create_dir_all(output)?;
    let device = hardware::gpu_identity("0")?;
    let variants = [
        ("q4", Artifact::Control),
        ("q6", Artifact::Q6K),
        ("q8", Artifact::Q8Zero),
    ];
    if !["all", "q4", "q6", "q8"].contains(&quant) {
        return Err(err("unknown quantization"));
    }
    let mut reports = Vec::new();
    let mut failures = Vec::new();
    for (name, artifact) in variants {
        if quant != "all" && quant != name {
            continue;
        }
        for concurrency in [1, 2, 4, 8] {
            if concurrency_filter.is_some_and(|wanted| wanted != concurrency) {
                continue;
            }
            match run_profile(
                worker,
                artifacts,
                output,
                name,
                artifact,
                concurrency,
                device.clone(),
            ) {
                Ok(report) => {
                    let memory_failure =
                        report.failure.as_deref().is_some_and(|s| s.contains("GPU"));
                    if let Some(failure) = &report.failure {
                        failures.push(format!("{name}/{concurrency}: {failure}"));
                    }
                    reports.push(report);
                    if memory_failure {
                        break;
                    }
                }
                Err(e) => {
                    failures.push(format!("{name}/{concurrency}: {e}"));
                    break;
                }
            }
        }
    }
    let ideal_exists = reports.iter().any(|r| r.ideal);
    let candidates: Vec<_> = reports
        .iter()
        .filter(|r| r.qualified && (!ideal_exists || r.ideal))
        .collect();
    let avg = |r: &Report| r.trial_throughput.iter().sum::<f64>() / 3.0;
    let range = |r: &Report| {
        (
            r.trial_throughput
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min),
            r.trial_throughput
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max),
        )
    };
    let best = candidates
        .iter()
        .copied()
        .max_by(|a, b| avg(a).total_cmp(&avg(b)));
    let chosen = best
        .and_then(|best| {
            let (bmin, bmax) = range(best);
            candidates
                .iter()
                .copied()
                .filter(|r| {
                    let (min, max) = range(r);
                    (avg(best) - avg(r)).abs() < 0.05 * avg(best) || (min <= bmax && bmin <= max)
                })
                .min_by(|a, b| {
                    a.peak_gpu_bytes
                        .cmp(&b.peak_gpu_bytes)
                        .then(a.concurrency.cmp(&b.concurrency))
                        .then(a.quant.cmp(&b.quant))
                })
        })
        .map(|r| format!("{}-{}", r.quant, r.concurrency));
    let did_not_complete = reports.iter().all(|report| !report.completed) && !failures.is_empty();
    let summary = Summary {
        reason: if did_not_complete { "comparisons did not complete" } else if chosen.is_some() {
            if ideal_exists { "highest throughput in ideal pool; ties use lower observed VRAM then concurrency" } else { "highest throughput in qualified pool; ties use lower observed VRAM then concurrency" }
        } else { "no configuration met correctness, memory and latency requirements" }.into(),
        chosen,
        profiles: reports.iter().map(|r| format!("{}-{}",r.quant,r.concurrency)).collect(),
        failures,
    };
    fs::write(
        output.join("comparison-summary.json"),
        serde_json::to_vec_pretty(&summary).map_err(err)?,
    )?;
    if did_not_complete {
        return Err(err(
            "comparisons did not complete; see comparison-summary.json",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        has_visible_content, ms, percentile, scheduled_arrival, stagger_overlap, transport_overlap,
        verdict, warmup_cohorts, warmup_ready,
    };
    use std::time::{Duration, Instant};
    #[test]
    fn overlap_requires_full_actual_write_within_reasoner_execution() {
        let t = Instant::now();
        let at = |ms| Some(t + Duration::from_millis(ms));
        // A 40ms initial write shifts dispatch to 140ms, after completion at 120ms.
        assert!(!transport_overlap(at(40), at(140), at(141), at(120)));
        assert!(transport_overlap(at(40), at(100), at(119), at(120)));
        assert!(!transport_overlap(at(40), at(100), at(120), at(120)));
        assert!(!transport_overlap(at(40), at(39), at(100), at(120)));
        assert!(!transport_overlap(at(40), at(100), at(99), at(120)));
        assert!(!transport_overlap(at(40), at(100), None, at(120)));
    }
    #[test]
    fn snapshot_gate_requires_paired_reasoner() {
        let reasoner = metron_protocol::content_digest(b"reasoner");
        let other = metron_protocol::content_digest(b"other");
        assert!(stagger_overlap(
            &reasoner,
            Some(std::slice::from_ref(&reasoner))
        ));
        assert!(!stagger_overlap(&reasoner, Some(&[other])));
        assert!(!stagger_overlap(&reasoner, None));
    }
    #[test]
    fn warmup_loads_before_exercising_configured_concurrency() {
        for concurrency in [1, 2, 4, 8] {
            let cohorts = warmup_cohorts(concurrency);
            assert_eq!(cohorts[0], vec![(0, false)]);
            assert_eq!(cohorts[1].len(), usize::from(concurrency));
            assert!(cohorts[1].iter().any(|&(id, _)| id == 2));
            if concurrency > 1 {
                assert!(cohorts[1].iter().any(|&(id, _)| id == 0));
            }
        }
    }
    #[test]
    fn latency_starts_at_scheduled_arrival_even_after_delayed_flush() {
        let base = Instant::now();
        let stagger = Duration::from_millis(100);
        assert_eq!(
            scheduled_arrival(base, stagger, 1).duration_since(base),
            stagger
        );
        let delayed_flush = base + Duration::from_millis(800);
        let usable = delayed_flush + Duration::from_millis(300);
        assert_eq!(ms(scheduled_arrival(base, Duration::ZERO, 0), usable), 1100);
        assert_eq!(ms(scheduled_arrival(base, stagger, 1), usable), 1000);
    }
    use metron_protocol::{
        BatchProvenance, BoundedText, LocalObservation, LocalOutputSchema, LocalResponse,
        SchemaVersion, SubmissionCohort, content_digest,
    };
    #[test]
    fn warmup_accepts_verbose_text_but_scored_math_remains_strict() {
        let digest = content_digest(b"test");
        let observation = LocalObservation {
            schema_version: SchemaVersion::V2,
            profile_digest: digest.clone(),
            request_digest: digest.clone(),
            input_digest: digest,
            content: BoundedText::new("17 - 4 = 13, then 13 + 9 = 22").unwrap(),
            reasoning: None,
            prompt_tokens: 1,
            completion_tokens: 1,
            submission_cohort: SubmissionCohort {
                id: BoundedText::new("test").unwrap(),
                size: 1,
                position: 0,
            },
            backend_batch: BatchProvenance::Unknown,
            response: Some(LocalResponse::Text),
        };
        assert!(warmup_ready(LocalOutputSchema::Text, &observation));
        assert_eq!(verdict("math", &observation), "incorrect");
    }
    #[test]
    fn visible_content_excludes_control_and_format_characters() {
        assert!(!has_visible_content(
            " \n\t\u{0}\u{200b}\u{200c}\u{200d}\u{feff}"
        ));
        assert!(has_visible_content("\u{200b}a"));
    }
    #[test]
    fn percentile_uses_nearest_rank_and_handles_empty() {
        assert_eq!(percentile(vec![]), None);
        assert_eq!(percentile((1..=20).collect()), Some(19));
    }
}
