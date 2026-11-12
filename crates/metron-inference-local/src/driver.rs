//! Consumed-authority local effect driver and lazy worker supervisor.
use crate::{artifact, comparison::ComparisonPermit, framing, qualification::QualificationPermit};
use metron_harness::{
    driver::DriverOutcome,
    policy::{AuthorizedEffect, EffectDriver},
};
use metron_protocol::coding_runtime::CodingStep;
use metron_protocol::{
    ActualUsage, ApprovedRequest, BoundedText, DecimalUsage, Digest, LocalDevice, LocalObservation,
    LocalOutputSchema, LocalProfile, LocalRole, MAX_ENVELOPE, MAX_LOCAL_CONTEXT, MAX_MODEL_OUTPUT,
    MAX_TEXT, PrincipalId, ProducerReceiptId, Receipt, ReceiptData, ReceiptOutput,
    SubmissionCohort, TaskId, UsageVector, WorkerReply, WorkerRequest, canonical_bytes,
    is_local_coding_recipe, parse_worker_reply, worker_request_bytes,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

type WriteQueue = mpsc::SyncSender<(Vec<u8>, mpsc::Sender<io::Result<()>>)>;
#[derive(Clone)]
pub struct WorkerControl {
    child: Arc<Mutex<Child>>,
    writes: WriteQueue,
    ready: Arc<AtomicBool>,
}
impl WorkerControl {
    /// Termination makes submitted effects uncertain until durable evidence resolves them.
    pub fn terminate(&self) -> io::Result<()> {
        self.child
            .lock()
            .map_err(|_| io::Error::other("worker lock poisoned"))?
            .kill()
    }
    /// Ask the worker to cancel a queued request without disturbing other requests.
    /// Cancellation after SDK submission has no guaranteed effect and remains uncertain.
    pub fn cancel(&self, dispatch_id: Digest) -> io::Result<()> {
        if !self.ready.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "first request not sent",
            ));
        }
        let frame = canonical_bytes(&WorkerRequest::Cancel { dispatch_id }, MAX_ENVELOPE)
            .map_err(io::Error::other)?;
        let (tx, _) = mpsc::channel();
        self.writes.try_send((frame, tx)).map_err(io::Error::other)
    }
}
#[derive(Clone)]
pub enum WorkerEvent {
    Spawned {
        pid: u32,
        control: WorkerControl,
    },
    Dispatching {
        dispatch_id: Digest,
        began_at: Instant,
    },
    Arrived {
        dispatch_id: Digest,
        in_flight: Vec<Digest>,
    },
    Sent {
        dispatch_id: Digest,
    },
    Loading {
        dispatch_id: Digest,
    },
    Loaded {
        dispatch_id: Digest,
    },
    Tokenized {
        dispatch_id: Digest,
        token_count: u32,
        token_ids_digest: Digest,
        rendered_digest: Digest,
    },
    Started {
        dispatch_id: Digest,
    },
    Delta {
        dispatch_id: Digest,
        sequence: u32,
        text: BoundedText<2048>,
        reasoning: bool,
        received_at: Instant,
    },
    Completed {
        dispatch_id: Digest,
        prompt_time_us: Option<u64>,
        completion_time_us: Option<u64>,
        received_at: Instant,
    },
    Exited {
        pid: u32,
    },
}
pub type EventSink = Arc<dyn Fn(WorkerEvent) + Send + Sync>;
pub type AdmissionCheck = Arc<dyn Fn() -> io::Result<()> + Send + Sync>;
struct Process {
    control: WorkerControl,
    writes: WriteQueue,
    replies: mpsc::Receiver<io::Result<(WorkerReply, Instant)>>,
    pid: u32,
    sink: Option<EventSink>,
}
impl Drop for Process {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let exited = self
                .control
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.try_wait().ok().flatten())
                .is_some();
            if exited {
                break;
            }
            if Instant::now() >= deadline {
                let _ = self.control.terminate();
                if let Ok(mut child) = self.control.child.lock() {
                    let _ = child.wait();
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(sink) = &self.sink {
            sink(WorkerEvent::Exited { pid: self.pid });
        }
    }
}

#[derive(Clone)]
struct TransportRequest {
    dispatch_id: Digest,
    request_digest: Digest,
    task_id: TaskId,
    principal_id: PrincipalId,
    action_digest: Digest,
    input_digest: Digest,
    role: LocalRole,
    output_schema: LocalOutputSchema,
    context: BoundedText<MAX_LOCAL_CONTEXT>,
    profile_digest: Digest,
    native_tools: Option<Vec<metron_protocol::coding::CodingTool>>,
    previous_tool_turn: Option<metron_protocol::coding_runtime::CompletedToolTurn>,
    cohort: Option<SubmissionCohort>,
    require_stream: bool,
}

/// Result of an individually admitted qualification case.
pub enum QualificationOutcome {
    Observed(Box<LocalObservation>),
    Rejected(BoundedText<MAX_TEXT>),
    Uncertain,
}
fn rejected(message: &str) -> QualificationOutcome {
    BoundedText::new(message).map_or(
        QualificationOutcome::Uncertain,
        QualificationOutcome::Rejected,
    )
}

/// Construction validates immutable config but never starts or loads a worker.
pub struct LocalInferenceDriver {
    worker_path: PathBuf,
    artifact_path: PathBuf,
    profile: LocalProfile,
    sink: Option<EventSink>,
    process: Option<Process>,
    admission_check: Option<AdmissionCheck>,
}
impl LocalInferenceDriver {
    pub fn new(
        worker_path: PathBuf,
        artifact_path: PathBuf,
        profile: LocalProfile,
        sink: Option<EventSink>,
    ) -> io::Result<Self> {
        profile.validate().map_err(io::Error::other)?;
        if !worker_path.is_absolute() || !artifact_path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "paths must be absolute",
            ));
        }
        Ok(Self {
            worker_path,
            artifact_path,
            profile,
            sink,
            process: None,
            admission_check: None,
        })
    }

    /// Recheck mutable host capacity immediately before a new worker is spawned.
    pub fn with_admission_check(mut self, check: AdmissionCheck) -> Self {
        self.admission_check = Some(check);
        self
    }
    fn start(&mut self) -> io::Result<()> {
        if self.process.is_some() {
            return Ok(());
        }
        if artifact::sha256_file(&self.worker_path)? != self.profile.backend_build_digest.as_str() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "worker build digest mismatch",
            ));
        }
        let mut command = Command::new(&self.worker_path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for name in [
            "RAYON_NUM_THREADS",
            "OMP_NUM_THREADS",
            "OPENBLAS_NUM_THREADS",
            "MKL_NUM_THREADS",
        ] {
            command.env(name, self.profile.cpu_threads.to_string());
        }
        command
            .env("HF_HUB_OFFLINE", "1")
            .env("MISTRALRS_CUDA_GRAPHS", "0")
            .env("HF_HUB_DISABLE_TELEMETRY", "1");
        match &self.profile.device {
            LocalDevice::Cpu => {
                command.env("CUDA_VISIBLE_DEVICES", "");
            }
            LocalDevice::Cuda { uuid, .. } => {
                command.env("CUDA_VISIBLE_DEVICES", uuid.as_str());
            }
        }
        if let Some(check) = &self.admission_check {
            check()?;
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("missing worker stdin"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing worker stdout"))?;
        let (writes, write_rx) = mpsc::sync_channel::<(Vec<u8>, mpsc::Sender<io::Result<()>>)>(1);
        let control = WorkerControl {
            child: Arc::new(Mutex::new(child)),
            writes: writes.clone(),
            ready: Arc::new(AtomicBool::new(false)),
        };
        if let Some(sink) = &self.sink {
            sink(WorkerEvent::Spawned {
                pid,
                control: control.clone(),
            });
        }
        std::thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok((frame, response)) = write_rx.recv() {
                let result = framing::write_request_frame(&mut stdin, &frame);
                let failed = result.is_err();
                let _ = response.send(result);
                if failed {
                    break;
                }
            }
        });
        let (tx, rx) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            loop {
                let result = match framing::read_frame(&mut stdout) {
                    Ok(Some(bytes)) => {
                        let received_at = Instant::now();
                        parse_worker_reply(&bytes)
                            .map(|reply| (reply, received_at))
                            .map_err(io::Error::other)
                    }
                    Ok(None) => Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "worker exited",
                    )),
                    Err(error) => Err(error),
                };
                let terminal = result.is_err();
                if tx.send(result).is_err() || terminal {
                    break;
                }
            }
        });
        self.process = Some(Process {
            control,
            writes,
            replies: rx,
            pid,
            sink: self.sink.clone(),
        });
        Ok(())
    }
    /// Explicitly stop the active task's worker and reap its process.
    pub fn shutdown(&mut self) {
        if let Some(process) = self.process.take() {
            if let Ok(bytes) = canonical_bytes(&WorkerRequest::Shutdown, MAX_ENVELOPE) {
                let (reply, _) = mpsc::channel();
                let _ = process.writes.try_send((bytes, reply));
            }
            drop(process);
        }
    }
    /// Consume independently admitted effects and submit one cohort to the native scheduler.
    pub fn execute_cohort(&mut self, effects: Vec<AuthorizedEffect>) -> Vec<DriverOutcome> {
        let requests = effects
            .iter()
            .map(|effect| {
                let prepared = effect.request();
                let (
                    input_digest,
                    context,
                    profile_digest,
                    role,
                    output_schema,
                    native_tools,
                    previous_tool_turn,
                ) = match &prepared.request {
                    ApprovedRequest::RequestLocalModel {
                        input_digest,
                        context,
                        profile_digest,
                        role,
                        output_schema,
                        ..
                    } => (
                        input_digest,
                        BoundedText::new(context.as_str()).ok()?,
                        profile_digest,
                        *role,
                        *output_schema,
                        None,
                        None,
                    ),
                    ApprovedRequest::Coding { step, .. }
                        if is_local_coding_recipe(self.profile.prompt_recipe_digest.as_str()) =>
                    {
                        let CodingStep::Model {
                            input_digest,
                            context,
                            profile_digest,
                            native_tools,
                            previous_tool_turn,
                            ..
                        } = step.as_ref()
                        else {
                            return None;
                        };
                        (
                            input_digest,
                            context.clone(),
                            profile_digest,
                            LocalRole::Reasoner,
                            LocalOutputSchema::Text,
                            native_tools.clone(),
                            previous_tool_turn.as_deref().cloned(),
                        )
                    }
                    _ => return None,
                };
                Some(TransportRequest {
                    dispatch_id: prepared.action.dispatch_id.clone(),
                    request_digest: prepared.request_digest.clone(),
                    task_id: prepared.action.task_id.clone(),
                    principal_id: prepared.action.principal_id.clone(),
                    action_digest: prepared.action_digest.clone(),
                    input_digest: input_digest.clone(),
                    role,
                    output_schema,
                    context,
                    profile_digest: profile_digest.clone(),
                    native_tools,
                    previous_tool_turn,
                    cohort: None,
                    require_stream: false,
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(requests) = requests else {
            return effects
                .iter()
                .map(|effect| failure(effect, "not a local model request"))
                .collect();
        };
        self.transport(requests)
            .into_iter()
            .zip(effects.iter())
            .map(|(outcome, effect)| match outcome {
                QualificationOutcome::Observed(observation) => {
                    receipt(effect, ReceiptOutput::LocalModel { observation }, "1")
                }
                QualificationOutcome::Rejected(diagnostic) => {
                    receipt(effect, ReceiptOutput::Failure { diagnostic }, "0")
                }
                QualificationOutcome::Uncertain => DriverOutcome::Uncertain,
            })
            .collect()
    }
    /// Consume one-shot, durably admitted corpus-case authorities for a qualification cohort.
    pub fn execute_qualification_cohort(
        &mut self,
        permits: Vec<QualificationPermit>,
    ) -> Vec<QualificationOutcome> {
        let requests = permits
            .iter()
            .map(|permit| {
                let request = permit.request();
                TransportRequest {
                    dispatch_id: request.dispatch_id.clone(),
                    request_digest: request.request_digest.clone(),
                    task_id: request.task_id.clone(),
                    principal_id: request.principal_id.clone(),
                    action_digest: request.action_digest.clone(),
                    input_digest: request.input_digest.clone(),
                    role: request.role,
                    output_schema: request.output_schema,
                    context: BoundedText::new(request.context.as_str())
                        .expect("legacy context bound is stricter than the local bound"),
                    profile_digest: request.profile_digest.clone(),
                    native_tools: None,
                    previous_tool_turn: None,
                    cohort: Some(request.cohort.clone()),
                    require_stream: false,
                }
            })
            .collect();
        self.transport(requests)
    }
    fn transport(&mut self, requests: Vec<TransportRequest>) -> Vec<QualificationOutcome> {
        self.transport_with_gap(requests, Duration::ZERO)
    }

    /// Consume one-shot model-only comparison authorities, without production promotion.
    pub fn execute_comparison_cohort(
        &mut self,
        permits: Vec<ComparisonPermit>,
        stagger: Duration,
    ) -> Vec<QualificationOutcome> {
        let requests = permits
            .iter()
            .map(|permit| {
                let request = permit.request();
                TransportRequest {
                    dispatch_id: request.dispatch_id.clone(),
                    request_digest: request.request_digest.clone(),
                    task_id: request.task_id.clone(),
                    principal_id: request.principal_id.clone(),
                    action_digest: request.action_digest.clone(),
                    input_digest: request.input_digest.clone(),
                    role: request.role,
                    output_schema: request.output_schema,
                    context: request.context.clone(),
                    profile_digest: request.profile_digest.clone(),
                    native_tools: None,
                    previous_tool_turn: None,
                    cohort: Some(request.cohort.clone()),
                    require_stream: true,
                }
            })
            .collect();
        self.transport_with_gap(requests, stagger)
    }

    fn transport_with_gap(
        &mut self,
        requests: Vec<TransportRequest>,
        stagger: Duration,
    ) -> Vec<QualificationOutcome> {
        if requests.is_empty() {
            return Vec::new();
        }
        let count = requests.len();
        if stagger > Duration::from_secs(1) {
            return (0..count)
                .map(|_| rejected("stagger exceeds limit"))
                .collect();
        }
        if count > usize::from(self.profile.max_sequences) {
            return (0..count)
                .map(|_| rejected("cohort exceeds profile"))
                .collect();
        }
        let mut pending = BTreeMap::new();
        let cohort_id = requests[0]
            .cohort
            .as_ref()
            .map_or_else(|| requests[0].dispatch_id.as_str(), |c| c.id.as_str())
            .to_owned();
        let admitted_cohort = requests[0].cohort.is_some();
        if requests
            .iter()
            .enumerate()
            .any(|(position, request)| match &request.cohort {
                Some(cohort) => {
                    !admitted_cohort
                        || cohort.id.as_str() != cohort_id
                        || usize::from(cohort.size) != count
                        || usize::from(cohort.position) != position
                }
                None => admitted_cohort,
            })
        {
            return (0..count)
                .map(|_| rejected("qualification cohort mismatch"))
                .collect();
        }
        let mut frames = Vec::new();
        for (position, request) in requests.iter().enumerate() {
            if self.profile.digest().ok().as_ref() != Some(&request.profile_digest) {
                return (0..count).map(|_| rejected("profile mismatch")).collect();
            }
            let Ok(cohort_id) = BoundedText::new(&cohort_id) else {
                return (0..count).map(|_| rejected("invalid cohort")).collect();
            };
            let Some(path_text) = self.artifact_path.to_str() else {
                return (0..count)
                    .map(|_| rejected("invalid artifact path"))
                    .collect();
            };
            let Ok(path) = BoundedText::new(path_text) else {
                return (0..count)
                    .map(|_| rejected("invalid artifact path"))
                    .collect();
            };
            let frame = WorkerRequest::Generate {
                dispatch_id: request.dispatch_id.clone(),
                request_digest: request.request_digest.clone(),
                task_id: request.task_id.clone(),
                principal_id: request.principal_id.clone(),
                action_digest: request.action_digest.clone(),
                input_digest: request.input_digest.clone(),
                role: request.role,
                output_schema: request.output_schema,
                context: request.context.clone(),
                native_tools: request.native_tools.clone().map(Box::new),
                previous_tool_turn: request.previous_tool_turn.clone().map(Box::new),
                profile: Box::new(self.profile.clone()),
                artifact_path: Box::new(path),
                cohort: Box::new(request.cohort.clone().unwrap_or(SubmissionCohort {
                    id: cohort_id,
                    size: count as u16,
                    position: position as u16,
                })),
            };
            let Ok(bytes) = worker_request_bytes(&frame) else {
                return (0..count)
                    .map(|_| rejected("request exceeds frame limit"))
                    .collect();
            };
            if frame.validate().is_err()
                || pending
                    .insert(request.dispatch_id.as_str().to_owned(), position)
                    .is_some()
            {
                return (0..count)
                    .map(|_| rejected("invalid request binding"))
                    .collect();
            }
            frames.push(bytes);
        }
        if let Err(error) = self.start() {
            let diagnostic = format!("worker could not start: {error}");
            return (0..count).map(|_| rejected(&diagnostic)).collect();
        }
        let process = self.process.as_mut().expect("started process");
        let deadline = Instant::now() + Duration::from_millis(u64::from(self.profile.deadline_ms));
        // Dispatch concurrently with receiving: a large later request must not wait
        // behind a full stdout queue produced by an earlier streaming request.
        // 0 = never queued, 1 = possibly written, 2 = flushed and Sent notified.
        let delivery = Arc::new((0..count).map(|_| AtomicU8::new(0)).collect::<Vec<_>>());
        let dispatch_done = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let writes = process.writes.clone();
        let control = process.control.clone();
        let sink = self.sink.clone();
        let ids = requests
            .iter()
            .map(|r| r.dispatch_id.clone())
            .collect::<Vec<_>>();
        let sender_delivery = delivery.clone();
        let sender_done = dispatch_done.clone();
        let sender_stop = stop.clone();
        let dispatcher = std::thread::spawn(move || {
            'dispatch: for (index, frame) in frames.into_iter().enumerate() {
                if index > 0 && !stagger.is_zero() {
                    let wake = Instant::now() + stagger;
                    while Instant::now() < wake {
                        if sender_stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                            break 'dispatch;
                        }
                        std::thread::sleep(
                            wake.saturating_duration_since(Instant::now())
                                .min(Duration::from_millis(10)),
                        );
                    }
                }
                if sender_stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                    break;
                }
                if let Some(sink) = &sink {
                    sink(WorkerEvent::Dispatching {
                        dispatch_id: ids[index].clone(),
                        began_at: Instant::now(),
                    });
                }
                let (tx, rx) = mpsc::channel();
                sender_delivery[index].store(1, Ordering::Release);
                if writes.try_send((frame, tx)).is_err() {
                    sender_delivery[index].store(0, Ordering::Release);
                    break;
                }
                loop {
                    if sender_stop.load(Ordering::Acquire) {
                        break 'dispatch;
                    }
                    let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                        break 'dispatch;
                    };
                    match rx.recv_timeout(wait.min(Duration::from_millis(10))) {
                        Ok(Ok(())) => {
                            control.ready.store(true, Ordering::Release);
                            if let Some(sink) = &sink {
                                sink(WorkerEvent::Sent {
                                    dispatch_id: ids[index].clone(),
                                });
                            }
                            sender_delivery[index].store(2, Ordering::Release);
                            break;
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => (),
                        _ => break 'dispatch,
                    }
                }
            }
            sender_done.store(true, Ordering::Release);
        });
        let mut results: Vec<Option<QualificationOutcome>> = (0..count).map(|_| None).collect();
        let mut started = BTreeSet::new();
        let mut arrived = BTreeSet::new();
        let mut tokenized = BTreeMap::new();
        let mut streamed: BTreeMap<usize, (String, String, u32)> = BTreeMap::new();
        let mut remaining = count;
        while remaining > 0 {
            if dispatch_done.load(Ordering::Acquire)
                && delivery
                    .iter()
                    .any(|state| state.load(Ordering::Acquire) != 2)
            {
                break;
            }
            let Some(wait) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            let (reply, received_at) = match process
                .replies
                .recv_timeout(wait.min(Duration::from_millis(10)))
            {
                Ok(Ok(reply)) => reply,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                _ => break,
            };
            let id = match &reply {
                WorkerReply::Arrived { dispatch_id, .. }
                | WorkerReply::Loading { dispatch_id }
                | WorkerReply::Loaded { dispatch_id }
                | WorkerReply::Tokenized { dispatch_id, .. }
                | WorkerReply::Started { dispatch_id }
                | WorkerReply::Delta { dispatch_id, .. }
                | WorkerReply::Completed { dispatch_id, .. }
                | WorkerReply::Rejected { dispatch_id, .. }
                | WorkerReply::CancelledBeforeStart { dispatch_id } => dispatch_id,
            };
            let Some(&index) = pending.get(id.as_str()) else {
                break;
            };
            while delivery[index].load(Ordering::Acquire) == 1
                && !dispatch_done.load(Ordering::Acquire)
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            if delivery[index].load(Ordering::Acquire) != 2 || results[index].is_some() {
                break;
            }
            match reply {
                WorkerReply::Arrived {
                    dispatch_id,
                    in_flight,
                } => {
                    if started.contains(&index)
                        || !arrived.insert(index)
                        || in_flight
                            .iter()
                            .any(|id| !pending.contains_key(id.as_str()))
                    {
                        break;
                    }
                    if let Some(sink) = &self.sink {
                        sink(WorkerEvent::Arrived {
                            dispatch_id,
                            in_flight,
                        });
                    }
                }
                WorkerReply::Loading { dispatch_id } => {
                    if let Some(sink) = &self.sink {
                        sink(WorkerEvent::Loading { dispatch_id });
                    }
                }
                WorkerReply::Loaded { dispatch_id } => {
                    if let Some(sink) = &self.sink {
                        sink(WorkerEvent::Loaded { dispatch_id });
                    }
                }
                WorkerReply::Tokenized {
                    dispatch_id,
                    token_count,
                    token_ids_digest,
                    rendered_digest,
                } => {
                    if started.contains(&index)
                        || token_count > self.profile.max_input_tokens
                        || tokenized.insert(index, token_count).is_some()
                    {
                        break;
                    }
                    if let Some(sink) = &self.sink {
                        sink(WorkerEvent::Tokenized {
                            dispatch_id,
                            token_count,
                            token_ids_digest,
                            rendered_digest,
                        });
                    }
                }
                WorkerReply::Started { dispatch_id } => {
                    if !tokenized.contains_key(&index) || !started.insert(index) {
                        break;
                    }
                    if let Some(sink) = &self.sink {
                        sink(WorkerEvent::Started { dispatch_id });
                    }
                }
                WorkerReply::Delta {
                    dispatch_id,
                    sequence,
                    text,
                    reasoning,
                } if started.contains(&index) => {
                    let (content, thoughts, next) = streamed.entry(index).or_default();
                    if sequence != *next || text.as_str().is_empty() {
                        break;
                    }
                    let field = if reasoning { thoughts } else { content };
                    if field.len().saturating_add(text.as_str().len()) > MAX_MODEL_OUTPUT {
                        break;
                    }
                    field.push_str(text.as_str());
                    let Some(sequence_after) = next.checked_add(1) else {
                        break;
                    };
                    *next = sequence_after;
                    if let Some(sink) = &self.sink {
                        sink(WorkerEvent::Delta {
                            dispatch_id,
                            sequence,
                            text,
                            reasoning,
                            received_at,
                        });
                    }
                }
                WorkerReply::Rejected { diagnostic, .. } if !started.contains(&index) => {
                    results[index] = Some(QualificationOutcome::Rejected(diagnostic));
                    remaining -= 1;
                }
                WorkerReply::CancelledBeforeStart { .. } if !started.contains(&index) => {
                    results[index] = Some(rejected("cancelled before submission"));
                    remaining -= 1;
                }
                WorkerReply::Completed {
                    dispatch_id,
                    observation,
                    prompt_time_us,
                    completion_time_us,
                } if started.contains(&index) => {
                    let request = &requests[index];
                    if (request.require_stream && !streamed.contains_key(&index))
                        || streamed.get(&index).is_some_and(|(content, reasoning, _)| {
                            content != observation.content.as_str()
                                || reasoning
                                    != observation.reasoning.as_ref().map_or("", |r| r.as_str())
                        })
                        || tokenized.get(&index) != Some(&observation.prompt_tokens)
                        || observation.input_digest != request.input_digest
                        || observation.submission_cohort.id.as_str() != cohort_id
                        || observation.submission_cohort.size as usize != count
                        || observation.submission_cohort.position as usize != index
                        || observation
                            .validate_with_context(
                                &self.profile,
                                &request.request_digest,
                                request.output_schema,
                                request.context.as_str(),
                                request.previous_tool_turn.as_ref(),
                            )
                            .is_err()
                    {
                        break;
                    }
                    if let Some(sink) = &self.sink {
                        sink(WorkerEvent::Completed {
                            dispatch_id,
                            prompt_time_us: prompt_time_us.map(|v| v.get()),
                            completion_time_us: completion_time_us.map(|v| v.get()),
                            received_at,
                        });
                    }
                    results[index] = Some(QualificationOutcome::Observed(observation));
                    remaining -= 1;
                }
                _ => break,
            }
        }
        stop.store(true, Ordering::Release);
        let _ = dispatcher.join();
        if remaining > 0
            || delivery
                .iter()
                .any(|state| state.load(Ordering::Acquire) != 2)
        {
            self.shutdown();
        }
        results
            .into_iter()
            .enumerate()
            .map(|(index, result)| {
                result.unwrap_or_else(|| {
                    if delivery[index].load(Ordering::Acquire) == 0 {
                        rejected("request was not sent")
                    } else {
                        QualificationOutcome::Uncertain
                    }
                })
            })
            .collect()
    }
}
impl Drop for LocalInferenceDriver {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl EffectDriver for LocalInferenceDriver {
    type Outcome = DriverOutcome;
    fn execute(&mut self, effect: AuthorizedEffect) -> Self::Outcome {
        self.execute_cohort(vec![effect])
            .pop()
            .unwrap_or(DriverOutcome::Uncertain)
    }
}
fn failure(effect: &AuthorizedEffect, text: &str) -> DriverOutcome {
    let Ok(diagnostic) = BoundedText::new(text) else {
        return DriverOutcome::Uncertain;
    };
    receipt(effect, ReceiptOutput::Failure { diagnostic }, "0")
}
fn receipt(effect: &AuthorizedEffect, output: ReceiptOutput, model: &str) -> DriverOutcome {
    let prepared = effect.request();
    let result = (|| {
        Receipt::new(ReceiptData {
            schema_version: effect.schema_version(),
            producer_receipt_id: ProducerReceiptId::new(prepared.action.dispatch_id.as_str())
                .ok()?,
            task_id: prepared.action.task_id.clone(),
            intent_id: prepared.intent_id.clone(),
            dispatch_id: prepared.action.dispatch_id.clone(),
            action_digest: prepared.action_digest.clone(),
            usage: ActualUsage::Known {
                usage: UsageVector {
                    model_requests: DecimalUsage::new(model).ok()?,
                    tool_calls: DecimalUsage::new("0").ok()?,
                    spend_micros_usd: DecimalUsage::new("0").ok()?,
                },
            },
            output,
        })
        .ok()
    })();
    result.map_or(DriverOutcome::Uncertain, |receipt| {
        DriverOutcome::Receipt(Box::new(receipt))
    })
}
