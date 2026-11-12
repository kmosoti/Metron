//! Observed worker timings and sampled resource windows for qualification.
use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use metron_inference_local::driver::{EventSink, WorkerControl, WorkerEvent};
use metron_protocol::{
    Digest, LocalDevice, LocalProfile, MeasurementPhase, MeasurementSource, PhaseMeasurement,
    SqlU64,
};

use crate::measurement::{
    CpuMonitor, GpuMonitor, Peak, ProcessSampler, clock_ticks_per_second, cpu_time_ms,
};

// One warm worker can see 144 scored cases and two warmups. Keep a little room
// while making forgotten timing retrieval a bounded failure.
const MAX_PENDING_TIMINGS: usize = 160;

#[derive(Clone, Debug)]
pub struct PerRequestTiming {
    // Legacy boundary: Sent callback to completed-frame receipt. It can be
    // unavailable when callbacks arrive in the opposite order.
    pub latency_ms: Option<u64>,
    pub first_visible_ms: Option<u64>,
    pub token_count: Option<u32>,
    pub token_ids_digest: Option<Digest>,
    pub rendered_digest: Option<Digest>,
    pub prompt_time_us: Option<u64>,
    pub completion_time_us: Option<u64>,
}
struct Pending {
    began: Instant,
    sent: Option<Instant>,
    first_visible: Option<Instant>,
    last_delta: Option<Instant>,
    next_sequence: u32,
    tokenized: Option<(u32, Digest, Digest)>,
    started: bool,
    completed: Option<PerRequestTiming>,
}
#[derive(Default)]
struct State {
    pid: Option<u32>,
    exited: bool,
    total: Peak,
    gpu_peak: Option<u64>,
    gpu_free: Option<u64>,
    control: Option<WorkerControl>,
    cpu: Option<CpuMonitor>,
    gpu: Option<GpuMonitor>,
    sampler: Option<ProcessSampler>,
    pending: BTreeMap<Digest, Pending>,
    failure: Option<String>,
}
impl State {
    fn fail(&mut self, reason: impl Into<String>) {
        if self.failure.is_none() {
            self.failure = Some(reason.into());
        }
        if let Some(control) = &self.control {
            let _ = control.terminate();
        }
    }
    fn check(&self) -> io::Result<()> {
        if let Some(reason) = &self.failure {
            return Err(io::Error::other(reason.clone()));
        }
        if self.cpu.as_ref().is_some_and(CpuMonitor::failed) {
            return Err(io::Error::other("CPU memory guard failed"));
        }
        Ok(())
    }
    fn event(&mut self, event: WorkerEvent, device: &LocalDevice) -> io::Result<()> {
        self.check()?;
        match event {
            WorkerEvent::Spawned { pid, control } => {
                if self.pid.is_some() {
                    return Err(io::Error::other("multiple workers in one capture"));
                }
                self.control = Some(control.clone());
                let cancel = control.clone();
                self.cpu = Some(CpuMonitor::start(move || {
                    let _ = cancel.terminate();
                })?);
                self.sampler = Some(ProcessSampler::start(pid));
                if let LocalDevice::Cuda { uuid, .. } = device {
                    let cancel = control.clone();
                    self.gpu = Some(GpuMonitor::start(uuid.as_str(), pid, move || {
                        let _ = cancel.terminate();
                    })?);
                }
                self.sampler
                    .as_ref()
                    .expect("sampler initialized")
                    .sample_now()?;
                if let Some(gpu) = &self.gpu {
                    gpu.sample_now()?;
                }
                self.pid = Some(pid);
            }
            WorkerEvent::Dispatching {
                dispatch_id,
                began_at,
            } => {
                if self.pid.is_none()
                    || self.pending.len() >= MAX_PENDING_TIMINGS
                    || self.pending.contains_key(&dispatch_id)
                {
                    return Err(io::Error::other("invalid or excessive dispatch event"));
                }
                self.pending.insert(
                    dispatch_id,
                    Pending {
                        began: began_at,
                        sent: None,
                        first_visible: None,
                        last_delta: None,
                        next_sequence: 0,
                        tokenized: None,
                        started: false,
                        completed: None,
                    },
                );
            }
            WorkerEvent::Sent { dispatch_id } => {
                let p = self
                    .pending
                    .get_mut(&dispatch_id)
                    .ok_or_else(|| io::Error::other("send without dispatch"))?;
                if p.sent.is_some() {
                    return Err(io::Error::other("invalid sent event"));
                }
                let sent = Instant::now();
                if sent < p.began {
                    return Err(io::Error::other("send before dispatch"));
                }
                p.sent = Some(sent);
            }
            WorkerEvent::Tokenized {
                dispatch_id,
                token_count,
                token_ids_digest,
                rendered_digest,
            } => {
                let p = self
                    .pending
                    .get_mut(&dispatch_id)
                    .ok_or_else(|| io::Error::other("tokenized without send"))?;
                if p.started || p.tokenized.is_some() || p.completed.is_some() {
                    return Err(io::Error::other("duplicate or late tokenization"));
                }
                p.tokenized = Some((token_count, token_ids_digest, rendered_digest));
            }
            WorkerEvent::Started { dispatch_id } => {
                let p = self
                    .pending
                    .get_mut(&dispatch_id)
                    .ok_or_else(|| io::Error::other("start without send"))?;
                if p.started || p.tokenized.is_none() {
                    return Err(io::Error::other("invalid start event"));
                }
                p.started = true;
            }
            WorkerEvent::Delta {
                dispatch_id,
                text,
                reasoning,
                received_at,
                sequence,
            } => {
                let p = self
                    .pending
                    .get_mut(&dispatch_id)
                    .ok_or_else(|| io::Error::other("delta without dispatch"))?;
                if !p.started
                    || p.completed.is_some()
                    || sequence != p.next_sequence
                    || received_at < p.began
                    || p.last_delta.is_some_and(|last| received_at < last)
                {
                    return Err(io::Error::other("invalid delta event"));
                }
                p.next_sequence = p
                    .next_sequence
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("delta sequence overflow"))?;
                p.last_delta = Some(received_at);
                if !reasoning && !text.as_str().is_empty() && p.first_visible.is_none() {
                    p.first_visible = Some(received_at);
                }
            }
            WorkerEvent::Completed {
                dispatch_id,
                prompt_time_us,
                completion_time_us,
                received_at,
            } => {
                let p = self
                    .pending
                    .get_mut(&dispatch_id)
                    .ok_or_else(|| io::Error::other("completion without send"))?;
                if !p.started
                    || p.completed.is_some()
                    || received_at < p.began
                    || p.last_delta.is_some_and(|last| received_at < last)
                {
                    return Err(io::Error::other("invalid completion event"));
                }
                let (count, ids, rendered) = p
                    .tokenized
                    .clone()
                    .ok_or_else(|| io::Error::other("completion without tokenization"))?;
                // Sent is dispatched by another thread and may arrive after any
                // reply, including completion. Preserve the legacy callback-side
                // boundary only when it precedes the frame receipt.
                let elapsed = p
                    .sent
                    .and_then(|sent| received_at.checked_duration_since(sent))
                    .map(|duration| u64::try_from(duration.as_millis()))
                    .transpose()
                    .map_err(io::Error::other)?;
                p.completed = Some(PerRequestTiming {
                    latency_ms: elapsed,
                    first_visible_ms: p
                        .first_visible
                        .map(|t| u64::try_from(t.duration_since(p.began).as_millis()))
                        .transpose()
                        .map_err(io::Error::other)?,
                    token_count: Some(count),
                    token_ids_digest: Some(ids),
                    rendered_digest: Some(rendered),
                    prompt_time_us,
                    completion_time_us,
                });
            }
            WorkerEvent::Loading { .. }
            | WorkerEvent::Loaded { .. }
            | WorkerEvent::Arrived { .. } => {}
            WorkerEvent::Exited { pid } => {
                if self.pid != Some(pid) || self.exited {
                    return Err(io::Error::other("unexpected worker exit"));
                }
                self.exited = true;
            }
        }
        Ok(())
    }
}
fn sql(value: u64) -> io::Result<SqlU64> {
    SqlU64::new(value).map_err(io::Error::other)
}
fn optional_sql(value: Option<u64>) -> io::Result<Option<SqlU64>> {
    value.map(sql).transpose()
}

pub struct Capture {
    state: Arc<Mutex<State>>,
    ticks_per_second: u64,
    session_id: Digest,
}
impl Capture {
    pub fn new(profile: &LocalProfile, session_id: &Digest) -> io::Result<(Self, EventSink)> {
        let ticks_per_second = clock_ticks_per_second()?;
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        let device = profile.device.clone();
        let sink: EventSink = Arc::new(move |event| {
            if let Ok(mut state) = shared.lock()
                && let Err(error) = state.event(event, &device)
            {
                state.fail(error.to_string());
            }
        });
        Ok((
            Self {
                state,
                ticks_per_second,
                session_id: session_id.clone(),
            },
            sink,
        ))
    }
    pub fn take_timing(&self, dispatch_id: &Digest) -> io::Result<PerRequestTiming> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("capture state poisoned"))?;
        state.check()?;
        // Call only after the driver has returned a terminal outcome for this
        // dispatch. Rejected/uncertain outcomes have no Completed event.
        let pending = state
            .pending
            .remove(dispatch_id)
            .ok_or_else(|| io::Error::other("dispatch timing unavailable"))?;
        let timing = pending
            .completed
            .ok_or_else(|| io::Error::other("completed timing unavailable"))?;
        Ok(timing)
    }
    pub fn finish_window(&self, repetition: u8) -> io::Result<PhaseMeasurement> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("capture state poisoned"))?;
        state.check()?;
        if state.pending.values().any(|p| p.completed.is_none()) {
            return Err(io::Error::other("requests still in flight"));
        }
        let sampler = state
            .sampler
            .as_ref()
            .ok_or_else(|| io::Error::other("worker sampler unavailable"))?;
        sampler.sample_now()?;
        if let Some(gpu) = &state.gpu {
            gpu.sample_now()?;
        }
        let peak = state
            .sampler
            .as_ref()
            .ok_or_else(|| io::Error::other("worker sampler unavailable"))?
            .take_window()?[6];
        let gpu = state
            .gpu
            .as_ref()
            .map(GpuMonitor::take_window)
            .transpose()?;
        if let Some(failure) = gpu.as_ref().and_then(|g| g.failure.as_ref()) {
            return Err(io::Error::other(failure.clone()));
        }
        state.total.samples = state.total.samples.saturating_add(peak.samples);
        state.total.cpu_samples = state.total.cpu_samples.saturating_add(peak.cpu_samples);
        state.total.rss_bytes = max_option(state.total.rss_bytes, peak.rss_bytes);
        state.total.pss_bytes = max_option(state.total.pss_bytes, peak.pss_bytes);
        if state.total.first_cpu_ticks.is_none() {
            state.total.first_cpu_ticks = peak.first_cpu_ticks;
        }
        if peak.last_cpu_ticks.is_some() {
            state.total.last_cpu_ticks = peak.last_cpu_ticks;
        }
        let gpu_peak = gpu.as_ref().and_then(|g| g.maximum_process_bytes);
        let gpu_free = gpu.as_ref().and_then(|g| g.minimum_free_bytes);
        state.gpu_peak = max_option(state.gpu_peak, gpu_peak);
        state.gpu_free = min_option(state.gpu_free, gpu_free);
        // Start the next window with actual endpoint observations too.
        state
            .sampler
            .as_ref()
            .expect("sampler initialized")
            .sample_now()?;
        if let Some(gpu) = &state.gpu {
            gpu.sample_now()?;
        }
        state.check()?;
        measurement(
            &self.session_id,
            repetition,
            &peak,
            gpu_peak,
            gpu_free,
            self.ticks_per_second,
        )
    }
    /// Snapshot all observed worker windows, including the final post-exit window.
    /// CPU time spans the first and last successful /proc counter observations.
    pub fn finish_worker(&self, repetition: u8) -> io::Result<PhaseMeasurement> {
        {
            let state = self
                .state
                .lock()
                .map_err(|_| io::Error::other("capture state poisoned"))?;
            if !state.exited {
                return Err(io::Error::other("worker has not exited"));
            }
        }
        self.finish_window(repetition)?;
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("capture state poisoned"))?;
        let mut result = measurement(
            &self.session_id,
            repetition,
            &state.total,
            state.gpu_peak,
            state.gpu_free,
            self.ticks_per_second,
        )?;
        result.phase = MeasurementPhase::WorkerLifetime;
        Ok(result)
    }
}
fn max_option(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}
fn min_option(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
fn measurement(
    session_id: &Digest,
    repetition: u8,
    peak: &Peak,
    gpu_peak: Option<u64>,
    gpu_free: Option<u64>,
    ticks_per_second: u64,
) -> io::Result<PhaseMeasurement> {
    let cpu_ms = if peak.cpu_samples >= 2 {
        cpu_time_ms(peak, ticks_per_second)
    } else {
        None
    };
    let cpu_present = peak.rss_bytes.is_some() || peak.pss_bytes.is_some() || cpu_ms.is_some();
    let gpu_present = gpu_free.is_some() || gpu_peak.is_some();
    Ok(PhaseMeasurement {
        session_id: session_id.clone(),
        phase: MeasurementPhase::WholeRun,
        repetition,
        cpu_source: cpu_present.then_some(MeasurementSource::Procfs),
        gpu_source: gpu_present.then_some(MeasurementSource::NvidiaSmi),
        cpu_sample_interval_ms: cpu_present.then_some(25),
        cpu_clock_ticks_per_second: if cpu_ms.is_some() {
            Some(u32::try_from(ticks_per_second).map_err(io::Error::other)?)
        } else {
            None
        },
        gpu_sample_interval_ms: gpu_present.then_some(250),
        peak_rss_bytes: optional_sql(peak.rss_bytes)?,
        peak_pss_bytes: optional_sql(peak.pss_bytes)?,
        cpu_time_ms: optional_sql(cpu_ms)?,
        peak_gpu_bytes: optional_sql(gpu_peak)?,
        min_gpu_free_bytes: optional_sql(gpu_free)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::content_digest;

    #[test]
    fn zero_tick_delta_is_measured_only_with_two_successful_samples() {
        let mut peak = Peak {
            cpu_samples: 1,
            first_cpu_ticks: Some(20),
            last_cpu_ticks: Some(20),
            ..Peak::default()
        };
        assert_eq!(
            measurement(&content_digest(b"session"), 0, &peak, None, None, 100)
                .unwrap()
                .cpu_time_ms,
            None
        );
        peak.cpu_samples = 2;
        assert_eq!(
            measurement(&content_digest(b"session"), 0, &peak, None, None, 100)
                .unwrap()
                .cpu_time_ms
                .unwrap()
                .get(),
            0
        );
    }
    #[test]
    fn timing_requires_order_and_preserves_worker_evidence() {
        let id = content_digest(b"dispatch");
        let ids = content_digest(b"ids");
        let rendered = content_digest(b"rendered");
        let mut state = State {
            pid: Some(42),
            ..State::default()
        };
        assert!(
            state
                .event(
                    WorkerEvent::Completed {
                        dispatch_id: id.clone(),
                        prompt_time_us: None,
                        completion_time_us: None,
                        received_at: Instant::now()
                    },
                    &LocalDevice::Cpu
                )
                .is_err()
        );
        state
            .event(
                WorkerEvent::Dispatching {
                    dispatch_id: id.clone(),
                    began_at: Instant::now(),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        state
            .event(
                WorkerEvent::Sent {
                    dispatch_id: id.clone(),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        assert!(
            state
                .event(
                    WorkerEvent::Started {
                        dispatch_id: id.clone()
                    },
                    &LocalDevice::Cpu
                )
                .is_err()
        );
        state
            .event(
                WorkerEvent::Tokenized {
                    dispatch_id: id.clone(),
                    token_count: 7,
                    token_ids_digest: ids.clone(),
                    rendered_digest: rendered.clone(),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        state
            .event(
                WorkerEvent::Started {
                    dispatch_id: id.clone(),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        state
            .event(
                WorkerEvent::Completed {
                    dispatch_id: id.clone(),
                    prompt_time_us: Some(12),
                    completion_time_us: None,
                    received_at: Instant::now(),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        let timing = state.pending[&id].completed.as_ref().unwrap();
        assert_eq!(timing.token_count, Some(7));
        assert_eq!(timing.token_ids_digest.as_ref(), Some(&ids));
        assert_eq!(timing.rendered_digest.as_ref(), Some(&rendered));
        assert_eq!(timing.prompt_time_us, Some(12));
        assert!(
            state
                .event(
                    WorkerEvent::Completed {
                        dispatch_id: id,
                        prompt_time_us: None,
                        completion_time_us: None,
                        received_at: Instant::now()
                    },
                    &LocalDevice::Cpu
                )
                .is_err()
        );
    }
    #[test]
    fn visible_timing_is_per_dispatch_and_never_invented() {
        use metron_protocol::BoundedText;
        use std::time::Duration;
        let mut state = State {
            pid: Some(1),
            ..State::default()
        };
        let a = content_digest(b"a");
        let b = content_digest(b"b");
        let base = Instant::now() - Duration::from_secs(10);
        for id in [&a, &b] {
            state
                .event(
                    WorkerEvent::Dispatching {
                        dispatch_id: id.clone(),
                        began_at: base,
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
            state
                .event(
                    WorkerEvent::Sent {
                        dispatch_id: id.clone(),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
            state.pending.get_mut(id).unwrap().sent = Some(base);
            state
                .event(
                    WorkerEvent::Tokenized {
                        dispatch_id: id.clone(),
                        token_count: 1,
                        token_ids_digest: content_digest(b"ids"),
                        rendered_digest: content_digest(b"rendered"),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
            state
                .event(
                    WorkerEvent::Started {
                        dispatch_id: id.clone(),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
        }
        let delta = |id: &Digest, ms, sequence, text: &str, reasoning| WorkerEvent::Delta {
            dispatch_id: id.clone(),
            sequence,
            text: BoundedText::new(text).unwrap(),
            reasoning,
            received_at: base + Duration::from_millis(ms),
        };
        state
            .event(delta(&a, 1000, 0, "", false), &LocalDevice::Cpu)
            .unwrap();
        state
            .event(delta(&b, 1500, 0, "thinking", true), &LocalDevice::Cpu)
            .unwrap();
        assert!(
            state
                .event(
                    delta(&a, 2000, 2, "out of sequence", false),
                    &LocalDevice::Cpu
                )
                .is_err()
        );
        state
            .event(delta(&a, 2000, 1, "visible", false), &LocalDevice::Cpu)
            .unwrap();
        state
            .event(delta(&a, 3000, 2, "later", false), &LocalDevice::Cpu)
            .unwrap();
        assert!(
            state
                .event(delta(&a, 2500, 3, "backwards", false), &LocalDevice::Cpu)
                .is_err()
        );
        assert!(
            state
                .event(
                    delta(&content_digest(b"missing"), 3000, 0, "x", false),
                    &LocalDevice::Cpu
                )
                .is_err()
        );
        // Completion must follow both the send and all received frames.
        assert!(
            state
                .event(
                    WorkerEvent::Completed {
                        dispatch_id: a.clone(),
                        prompt_time_us: None,
                        completion_time_us: None,
                        received_at: base + Duration::from_millis(2500)
                    },
                    &LocalDevice::Cpu
                )
                .is_err()
        );
        let end = Instant::now();
        for id in [&b, &a] {
            state
                .event(
                    WorkerEvent::Completed {
                        dispatch_id: id.clone(),
                        prompt_time_us: None,
                        completion_time_us: None,
                        received_at: end,
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
        }
        assert_eq!(
            state.pending[&a].completed.as_ref().unwrap().latency_ms,
            Some(u64::try_from(end.duration_since(base).as_millis()).unwrap())
        );
        assert_eq!(
            state.pending[&a]
                .completed
                .as_ref()
                .unwrap()
                .first_visible_ms,
            Some(2000)
        );
        assert_eq!(
            state.pending[&b]
                .completed
                .as_ref()
                .unwrap()
                .first_visible_ms,
            None
        );
        assert!(
            state
                .event(delta(&a, 4000, 3, "late", false), &LocalDevice::Cpu)
                .is_err()
        );
    }
    #[test]
    fn reversed_sent_callback_preserves_interleaved_terminal_results() {
        use metron_protocol::BoundedText;
        use std::time::Duration;
        let mut state = State {
            pid: Some(1),
            ..State::default()
        };
        let a = content_digest(b"reverse-a");
        let b = content_digest(b"reverse-b");
        let base = Instant::now() - Duration::from_secs(5);
        for id in [&a, &b] {
            state
                .event(
                    WorkerEvent::Dispatching {
                        dispatch_id: id.clone(),
                        began_at: base,
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
            state
                .event(
                    WorkerEvent::Tokenized {
                        dispatch_id: id.clone(),
                        token_count: 1,
                        token_ids_digest: content_digest(b"ids"),
                        rendered_digest: content_digest(b"rendered"),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
            state
                .event(
                    WorkerEvent::Started {
                        dispatch_id: id.clone(),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
        }
        for (id, ms) in [(&b, 1200), (&a, 2200)] {
            state
                .event(
                    WorkerEvent::Delta {
                        dispatch_id: id.clone(),
                        sequence: 0,
                        text: BoundedText::new("visible").unwrap(),
                        reasoning: false,
                        received_at: base + Duration::from_millis(ms),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
        }
        for (id, ms) in [(&a, 3000), (&b, 3500)] {
            state
                .event(
                    WorkerEvent::Completed {
                        dispatch_id: id.clone(),
                        prompt_time_us: None,
                        completion_time_us: None,
                        received_at: base + Duration::from_millis(ms),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
        }
        for (id, expected) in [(&a, 2200), (&b, 1200)] {
            let timing = state.pending[id].completed.as_ref().unwrap();
            assert_eq!(timing.first_visible_ms, Some(expected));
            assert_eq!(timing.latency_ms, None);
            state
                .event(
                    WorkerEvent::Sent {
                        dispatch_id: id.clone(),
                    },
                    &LocalDevice::Cpu,
                )
                .unwrap();
            assert_eq!(
                state.pending[id].completed.as_ref().unwrap().latency_ms,
                None
            );
            assert!(
                state
                    .event(
                        WorkerEvent::Sent {
                            dispatch_id: id.clone()
                        },
                        &LocalDevice::Cpu
                    )
                    .is_err()
            );
            assert!(
                state
                    .event(
                        WorkerEvent::Started {
                            dispatch_id: id.clone()
                        },
                        &LocalDevice::Cpu
                    )
                    .is_err()
            );
        }
    }
    #[test]
    fn callback_arrival_does_not_reorder_frame_timestamps() {
        use std::time::Duration;
        let mut state = State {
            pid: Some(1),
            ..State::default()
        };
        let id = content_digest(b"precallback-frame");
        let base = Instant::now() - Duration::from_secs(5);
        state
            .event(
                WorkerEvent::Dispatching {
                    dispatch_id: id.clone(),
                    began_at: base,
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        state
            .event(
                WorkerEvent::Sent {
                    dispatch_id: id.clone(),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        state
            .event(
                WorkerEvent::Tokenized {
                    dispatch_id: id.clone(),
                    token_count: 1,
                    token_ids_digest: content_digest(b"ids"),
                    rendered_digest: content_digest(b"rendered"),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        state
            .event(
                WorkerEvent::Started {
                    dispatch_id: id.clone(),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        state
            .event(
                WorkerEvent::Completed {
                    dispatch_id: id.clone(),
                    prompt_time_us: None,
                    completion_time_us: None,
                    received_at: base + Duration::from_secs(4),
                },
                &LocalDevice::Cpu,
            )
            .unwrap();
        assert_eq!(
            state.pending[&id].completed.as_ref().unwrap().latency_ms,
            None
        );
    }
}
