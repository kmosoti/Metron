//! Bounded host-side observations; sampled peaks are not hard allocation caps.
use std::{
    fs::File,
    io::{self, Read},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
const MAX_PROC_BYTES: u64 = 64 * 1024;
const MIN_GPU_HEADROOM: u64 = 1024 * 1024 * 1024;
const SAMPLE_INTERVAL: Duration = Duration::from_millis(25);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Unattributed,
    WholeRun,
}
impl Phase {
    fn index(self) -> usize {
        match self {
            Self::Unattributed => 0,
            Self::WholeRun => 6,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Peak {
    pub samples: u64,
    pub cpu_samples: u64,
    pub rss_bytes: Option<u64>,
    pub pss_bytes: Option<u64>,
    pub first_cpu_ticks: Option<u64>,
    pub last_cpu_ticks: Option<u64>,
}
#[derive(Clone, Copy, Debug, Default)]
struct Sample {
    rss_bytes: Option<u64>,
    pss_bytes: Option<u64>,
    cpu_ticks: Option<u64>,
}
struct State {
    phase: Phase,
    peaks: [Peak; 7],
}
fn bounded_text(path: &str) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_PROC_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PROC_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "proc entry too large",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid proc text"))
}
fn kilobytes(text: &str, label: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with(label))?;
    let mut fields = line[label.len()..].split_whitespace();
    let value = fields.next()?.parse::<u64>().ok()?;
    if fields.next()? != "kB" || fields.next().is_some() {
        return None;
    }
    value.checked_mul(1024)
}
fn cpu_ticks(stat: &str) -> Option<u64> {
    let (_, fields) = stat.rsplit_once(") ")?;
    let mut fields = fields.split_whitespace();
    let user = fields.nth(11)?.parse::<u64>().ok()?;
    let system = fields.next()?.parse::<u64>().ok()?;
    user.checked_add(system)
}
/// Converts cumulative process ticks using the host's reported POSIX clock rate.
pub fn cpu_time_ms(peak: &Peak, ticks_per_second: u64) -> Option<u64> {
    if ticks_per_second == 0 {
        return None;
    }
    let ticks = peak.last_cpu_ticks?.checked_sub(peak.first_cpu_ticks?)?;
    u64::try_from(u128::from(ticks) * 1000 / u128::from(ticks_per_second)).ok()
}
pub fn clock_ticks_per_second() -> io::Result<u64> {
    let mut child = Command::new("getconf")
        .arg("CLK_TCK")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(result.err().unwrap_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "clock rate query timed out")
                }));
            }
        }
    };
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("clock rate stdout unavailable"))?
        .take(65)
        .read_to_end(&mut bytes)?;
    if !status.success() || bytes.len() > 64 {
        return Err(io::Error::other("clock rate unavailable"));
    }
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid clock rate"))
}
fn process_sample(pid: u32) -> Sample {
    let path = format!("/proc/{pid}");
    Sample {
        rss_bytes: bounded_text(&format!("{path}/status"))
            .ok()
            .and_then(|s| kilobytes(&s, "VmRSS:")),
        pss_bytes: bounded_text(&format!("{path}/smaps_rollup"))
            .ok()
            .and_then(|s| kilobytes(&s, "Pss:")),
        cpu_ticks: bounded_text(&format!("{path}/stat"))
            .ok()
            .and_then(|s| cpu_ticks(&s)),
    }
}
fn update(peak: &mut Peak, sample: Sample) {
    peak.samples = peak.samples.saturating_add(1);
    if let Some(value) = sample.rss_bytes {
        peak.rss_bytes = Some(peak.rss_bytes.map_or(value, |old| old.max(value)));
    }
    if let Some(value) = sample.pss_bytes {
        peak.pss_bytes = Some(peak.pss_bytes.map_or(value, |old| old.max(value)));
    }
    if let Some(value) = sample.cpu_ticks {
        peak.cpu_samples = peak.cpu_samples.saturating_add(1);
        peak.first_cpu_ticks.get_or_insert(value);
        peak.last_cpu_ticks = Some(value);
    }
}
/// Fixed-memory sampler. Phase labels require actual backend boundary events.
pub struct ProcessSampler {
    pid: u32,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl ProcessSampler {
    pub fn start(pid: u32) -> Self {
        let state = Arc::new(Mutex::new(State {
            phase: Phase::Unattributed,
            peaks: [Peak::default(); 7],
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, done) = (Arc::clone(&state), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                let sample = process_sample(pid);
                if let Ok(mut state) = s.lock() {
                    let index = state.phase.index();
                    update(&mut state.peaks[index], sample);
                    if index != Phase::WholeRun.index() {
                        update(&mut state.peaks[Phase::WholeRun.index()], sample);
                    }
                }
                thread::sleep(SAMPLE_INTERVAL);
            }
        });
        Self {
            pid,
            state,
            stop,
            thread: Some(thread),
        }
    }
    pub fn sample_now(&self) -> io::Result<()> {
        let sample = process_sample(self.pid);
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("measurement state poisoned"))?;
        let index = state.phase.index();
        update(&mut state.peaks[index], sample);
        if index != Phase::WholeRun.index() {
            update(&mut state.peaks[Phase::WholeRun.index()], sample);
        }
        Ok(())
    }
    /// Begin a new measurement window without stopping the worker.
    pub fn take_window(&self) -> io::Result<[Peak; 7]> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("measurement state poisoned"))?;
        Ok(std::mem::take(&mut state.peaks))
    }
}
impl Drop for ProcessSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
/// Read actual free GPU memory before launch and require at least 1 GiB.
pub fn cuda_headroom(uuid: &str) -> io::Result<u64> {
    let free = cuda_free(uuid)?;
    if free < MIN_GPU_HEADROOM {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "less than 1 GiB GPU headroom",
        ));
    }
    Ok(free)
}
fn cuda_free(uuid: &str) -> io::Result<u64> {
    let output = nvidia_query(uuid, "memory.free")?;
    let free = output
        .trim()
        .parse::<u64>()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid GPU free-memory observation",
            )
        })?
        .checked_mul(1024 * 1024)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "GPU memory overflow"))?;
    Ok(free)
}

#[derive(Clone, Debug, Default)]
pub struct GpuObservation {
    pub samples: u64,
    pub minimum_free_bytes: Option<u64>,
    pub maximum_process_bytes: Option<u64>,
    pub failure: Option<String>,
}

/// Polls physical GPU free memory. These samples are observations, not an allocation cap.
/// A failed query or insufficient headroom invokes the worker cancellation callback once.
pub struct GpuMonitor {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<GpuObservation>>,
    query: Arc<dyn Fn() -> io::Result<(u64, Option<u64>)> + Send + Sync>,
    cancel: Arc<dyn Fn() + Send + Sync>,
    thread: Option<JoinHandle<()>>,
}

/// Stops the worker if sampled system memory headroom is exhausted or unavailable.
pub struct CpuMonitor {
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl CpuMonitor {
    pub fn start(cancel: impl Fn() + Send + 'static) -> io::Result<Self> {
        if crate::hardware::available_cpu()? <= MIN_GPU_HEADROOM {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "less than 1 GiB system memory available",
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&stop);
        let failure = Arc::clone(&failed);
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(250));
                if done.load(Ordering::Acquire) {
                    break;
                }
                if !crate::hardware::available_cpu().is_ok_and(|free| free > MIN_GPU_HEADROOM) {
                    failure.store(true, Ordering::Release);
                    cancel();
                    break;
                }
            }
        });
        Ok(Self {
            stop,
            failed,
            thread: Some(thread),
        })
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}
impl Drop for CpuMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl GpuMonitor {
    pub fn start(
        uuid: &str,
        pid: u32,
        cancel: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let initial = cuda_headroom(uuid)?;
        let uuid = uuid.to_owned();
        Ok(Self::start_with(
            initial,
            move || Ok((cuda_free(&uuid)?, gpu_process_bytes(&uuid, pid)?)),
            cancel,
        ))
    }
    fn start_with(
        initial: u64,
        query: impl Fn() -> io::Result<(u64, Option<u64>)> + Send + Sync + 'static,
        cancel: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let state = Arc::new(Mutex::new(GpuObservation {
            samples: 1,
            minimum_free_bytes: Some(initial),
            maximum_process_bytes: None,
            failure: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let query: Arc<dyn Fn() -> io::Result<(u64, Option<u64>)> + Send + Sync> = Arc::new(query);
        let cancel: Arc<dyn Fn() + Send + Sync> = Arc::new(cancel);
        let thread_query = Arc::clone(&query);
        let thread_cancel = Arc::clone(&cancel);
        let (observations, done) = (Arc::clone(&state), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(250));
                if done.load(Ordering::Acquire) {
                    break;
                }
                let sample = thread_query();
                let failed = match observations.lock() {
                    Ok(mut observations) => match sample {
                        Ok((free, process)) => {
                            observations.samples = observations.samples.saturating_add(1);
                            observations.minimum_free_bytes = Some(
                                observations
                                    .minimum_free_bytes
                                    .map_or(free, |old| old.min(free)),
                            );
                            if let Some(process) = process {
                                observations.maximum_process_bytes = Some(
                                    observations
                                        .maximum_process_bytes
                                        .map_or(process, |old| old.max(process)),
                                );
                            }
                            if free < MIN_GPU_HEADROOM {
                                observations.failure = Some("GPU headroom below 1 GiB".into());
                                true
                            } else {
                                false
                            }
                        }
                        Err(_) => {
                            observations.failure = Some("GPU telemetry unavailable".into());
                            true
                        }
                    },
                    Err(_) => true,
                };
                if failed {
                    thread_cancel();
                    break;
                }
            }
        });
        Self {
            stop,
            state,
            query,
            cancel,
            thread: Some(thread),
        }
    }
    pub fn sample_now(&self) -> io::Result<()> {
        let result = (self.query)();
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("GPU monitor state poisoned"))?;
        match result {
            Ok((free, process)) if free >= MIN_GPU_HEADROOM => {
                state.samples = state.samples.saturating_add(1);
                state.minimum_free_bytes =
                    Some(state.minimum_free_bytes.map_or(free, |old| old.min(free)));
                if let Some(process) = process {
                    state.maximum_process_bytes = Some(
                        state
                            .maximum_process_bytes
                            .map_or(process, |old| old.max(process)),
                    );
                }
                Ok(())
            }
            _ => {
                state.failure = Some("GPU telemetry unavailable or headroom below 1 GiB".into());
                drop(state);
                (self.cancel)();
                Err(io::Error::other("GPU telemetry or headroom failure"))
            }
        }
    }
    #[cfg(test)]
    pub fn finish(mut self) -> io::Result<GpuObservation> {
        self.stop.store(true, Ordering::Release);
        if self
            .thread
            .take()
            .is_some_and(|thread| thread.join().is_err())
        {
            return Err(io::Error::other("GPU monitor failed"));
        }
        self.state
            .lock()
            .map(|state| state.clone())
            .map_err(|_| io::Error::other("GPU monitor state poisoned"))
    }
    pub fn take_window(&self) -> io::Result<GpuObservation> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("GPU monitor state poisoned"))?;
        Ok(std::mem::take(&mut *state))
    }
}
impl Drop for GpuMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub(crate) fn nvidia_query(selector: &str, field: &'static str) -> io::Result<String> {
    let output = nvidia_call(selector, &format!("--query-gpu={field}"))?;
    let output = output.trim();
    if output.is_empty() || output.len() > 512 || output.contains(['\n', '\r']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ambiguous GPU query output",
        ));
    }
    Ok(output.to_owned())
}
fn gpu_process_bytes(selector: &str, pid: u32) -> io::Result<Option<u64>> {
    let output = nvidia_call(selector, "--query-compute-apps=pid,used_memory")?;
    let mut result = None;
    for line in output.lines() {
        let (id, memory) = line
            .split_once(',')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid GPU process row"))?;
        if id.trim().parse::<u32>().ok() == Some(pid) {
            let bytes = memory
                .trim()
                .parse::<u64>()
                .ok()
                .and_then(|n| n.checked_mul(1024 * 1024))
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "GPU process memory unavailable")
                })?;
            if result.replace(bytes).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "duplicate GPU process row",
                ));
            }
        }
    }
    Ok(result)
}
fn nvidia_call(selector: &str, query: &str) -> io::Result<String> {
    if selector.is_empty()
        || !selector
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid GPU selector",
        ));
    }
    // Always reap the query, including when taking stdout or polling fails.
    struct Reap(Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Reap(
        Command::new("nvidia-smi")
            .arg(format!("--id={selector}"))
            .arg(query)
            .arg("--format=csv,noheader,nounits")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("GPU query stdout unavailable"))?;
    let reader = thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        stdout.take(4097).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "GPU query timed out",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    };
    let bytes = reader
        .join()
        .map_err(|_| io::Error::other("GPU query reader failed"))??;
    if !status.success() || bytes.len() > 4096 {
        return Err(io::Error::other("GPU free-memory query unavailable"));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid GPU query output"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_process_accounting_and_units() {
        assert_eq!(
            kilobytes("Name: x\nVmRSS: 123 kB\n", "VmRSS:"),
            Some(125_952)
        );
        assert_eq!(kilobytes("Pss: 3 MB\n", "Pss:"), None);
        assert_eq!(
            cpu_ticks("1 (worker with spaces) S 0 0 0 0 0 0 0 0 0 0 17 5 0"),
            Some(22)
        );
        assert!(process_sample(std::process::id()).rss_bytes.is_some());
    }
    #[test]
    fn headroom_failure_cancels_worker() {
        let (send, receive) = std::sync::mpsc::channel();
        let monitor = GpuMonitor::start_with(
            MIN_GPU_HEADROOM,
            || Ok((0, Some(10))),
            move || {
                let _ = send.send(());
            },
        );
        receive.recv_timeout(Duration::from_secs(3)).unwrap();
        let observation = monitor.finish().unwrap();
        assert_eq!(observation.minimum_free_bytes, Some(0));
        assert_eq!(observation.maximum_process_bytes, Some(10));
        assert!(observation.failure.is_some());
    }
}
