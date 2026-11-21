//! Lane A (`wasm_aot`) deterministic Wasmtime 51 runtime with `PoolingAllocationConfig`,
//! warm reusable `LaneARuntime` + shared epoch ticker thread, two-phase seccomp
//! (baseline pre-deserialize + stacked W^X `PROT_EXEC` post-deserialize), and `ResourceReceipt`.

pub use crate::wasm_aot_types::{
    AotEnvelopeMeta, EngineTunables, LaneALimits, PINNED_WASMTIME_VERSION, ResourceReceipt,
    decode_hex_32, encode_hex, host_target_triple,
};

use crate::{
    SealedMemfd,
    error::SandboxError,
    seccomp_bpf::{SeccompPolicy, install_prot_exec_seccomp, install_seccomp},
};
use metron_wasm_aot_loader::{AotSeal, deserialize_verified_memfd};
use sha2::{Digest as _, Sha256};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use wasmtime::{
    Engine, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder, Trap,
    component::{Linker, ResourceTable},
};
use wasmtime_wasi::{
    WasiCtx, WasiCtxView, WasiView,
    p2::{
        add_to_linker_sync,
        bindings::sync::CommandPre,
        pipe::{MemoryInputPipe, MemoryOutputPipe},
    },
};

struct MetronLimiter {
    inner: StoreLimits,
    peak_memory_bytes: u64,
}

impl ResourceLimiter for MetronLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let allowed = self.inner.memory_growing(current, desired, maximum)?;
        if allowed {
            self.peak_memory_bytes = self.peak_memory_bytes.max(desired as u64);
        }
        Ok(allowed)
    }

    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.inner.memory_grow_failed(error)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.inner.table_growing(current, desired, maximum)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

struct LaneAState {
    wasi: WasiCtx,
    table: ResourceTable,
    limiter: MetronLimiter,
}

impl WasiView for LaneAState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

/// Reusable Lane A runtime pool holding a warm pooling `wasmtime::Engine` and a single
/// shared background epoch ticker thread.
pub struct LaneARuntime {
    engine: Engine,
    tunables: EngineTunables,
    tunables_digest: [u8; 32],
    stop: Arc<AtomicBool>,
    ticker: Option<thread::JoinHandle<()>>,
}

impl LaneARuntime {
    /// Initializes a warm pooling Wasmtime 51 `Engine` and starts one shared 10ms epoch ticker.
    pub fn new(tunables: EngineTunables) -> Result<Self, SandboxError> {
        let cfg = tunables.to_wasmtime_config()?;
        let engine = Engine::new(&cfg)
            .map_err(|e| SandboxError::Denied(format!("wasmtime engine init failed: {e}")))?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_ticker = Arc::clone(&stop);
        let engine_ticker = engine.clone();
        let ticker = thread::spawn(move || {
            while !stop_ticker.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(10));
                engine_ticker.increment_epoch();
            }
        });
        Ok(Self {
            engine,
            tunables,
            tunables_digest: tunables.digest(),
            stop,
            ticker: Some(ticker),
        })
    }

    /// Returns the deterministic `EngineTunables` bound to this runtime pool.
    #[must_use]
    pub const fn tunables(&self) -> &EngineTunables {
        &self.tunables
    }

    /// Immediately advances the engine epoch by `ticks` to interrupt in-flight guest calls
    /// (for example on MCP `notifications/cancelled`).
    pub fn bump_epoch(&self, ticks: u64) {
        for _ in 0..ticks {
            self.engine.increment_epoch();
        }
    }

    /// Verifies a sealed `.cwasm` `memfd`, applies optional two-phase seccomp
    /// (baseline before deserialize, W^X `PROT_EXEC` immediately after deserialize),
    /// instantiates from the warm pooling allocator, and emits a deterministic `ResourceReceipt`.
    pub fn execute_sealed_cwasm(
        &self,
        cwasm_memfd: &SealedMemfd,
        meta: &AotEnvelopeMeta,
        key: &[u8; 32],
        limits: &LaneALimits,
        stdin_bytes: &[u8],
        apply_two_phase_seccomp: bool,
    ) -> Result<ResourceReceipt, SandboxError> {
        let target = host_target_triple()?;
        let (meta_tunables, cwasm_blake3, hmac_tag) = meta.decode_parts()?;
        let seal = AotSeal {
            wasmtime_version: &meta.wasmtime_version,
            target_triple: &meta.target_triple,
            engine_tunables_digest: meta_tunables,
            cwasm_blake3,
            hmac_tag,
        };

        if apply_two_phase_seccomp {
            // Phase 1: block network, exec, io_uring, ptrace, namespaces BEFORE deserialize.
            install_seccomp(SeccompPolicy {
                deny_prot_exec: false,
                allow_unix_sockets: false,
                deny_exec: true,
            })?;
        }

        let max_bytes = limits.max_cwasm_bytes.min(self.tunables.max_cwasm_bytes);
        let component = deserialize_verified_memfd(
            &self.engine,
            cwasm_memfd.as_file(),
            key,
            &seal,
            (
                PINNED_WASMTIME_VERSION,
                target,
                &self.tunables_digest,
                max_bytes,
            ),
        )
        .map_err(|msg| SandboxError::Denied(msg.into()))?;

        if apply_two_phase_seccomp {
            // Phase 2: stack W^X deny_prot_exec filter immediately after AOT deserialization.
            install_prot_exec_seccomp()?;
        }

        let stdout_pipe = MemoryOutputPipe::new(limits.max_output_bytes);
        let stderr_pipe = MemoryOutputPipe::new(limits.max_output_bytes);
        let mut wasi_builder = WasiCtx::builder();
        wasi_builder
            .stdin(MemoryInputPipe::new(stdin_bytes.to_vec()))
            .stdout(stdout_pipe.clone())
            .stderr(stderr_pipe.clone());

        let store_limits = StoreLimitsBuilder::new()
            .memory_size(limits.max_memory_bytes)
            .table_elements(limits.max_table_elements)
            .instances(8)
            .memories(4)
            .tables(8)
            .trap_on_grow_failure(true)
            .build();

        let mut store = Store::new(
            &self.engine,
            LaneAState {
                wasi: wasi_builder.build(),
                table: ResourceTable::new(),
                limiter: MetronLimiter {
                    inner: store_limits,
                    peak_memory_bytes: 0,
                },
            },
        );
        store.limiter(|s| &mut s.limiter);
        store
            .set_fuel(limits.fuel_budget)
            .map_err(|e| SandboxError::Denied(format!("set_fuel failed: {e}")))?;
        let ticks = u64::from(limits.timeout_ms).div_ceil(10).max(1);
        store.set_epoch_deadline(ticks);

        let mut linker = Linker::<LaneAState>::new(&self.engine);
        add_to_linker_sync(&mut linker)
            .map_err(|e| SandboxError::Denied(format!("wasi p2 link failed: {e}")))?;
        let pre = linker
            .instantiate_pre(&component)
            .map_err(|e| SandboxError::Denied(format!("component pre-instantiate failed: {e}")))?;

        let call_outcome = run_component_entry(&mut store, pre);

        let remaining_fuel = store.get_fuel().unwrap_or(0);
        let fuel_consumed = limits.fuel_budget.saturating_sub(remaining_fuel);
        let peak_memory_bytes = store.data().limiter.peak_memory_bytes;
        let stdout = stdout_pipe.contents().to_vec();
        let stderr = stderr_pipe.contents().to_vec();

        let (exit_code, trapped) = match call_outcome {
            Ok(code) => (code, None),
            Err(err) => (124, Some(classify_wasm_error(&err))),
        };

        Ok(ResourceReceipt {
            exit_code,
            fuel_consumed,
            peak_memory_bytes,
            stdout_sha256: encode_hex(&Sha256::digest(&stdout)),
            stderr_sha256: encode_hex(&Sha256::digest(&stderr)),
            stdout,
            stderr,
            cwasm_blake3,
            engine_tunables_digest: self.tunables_digest,
            trapped,
        })
    }
}

impl Drop for LaneARuntime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.ticker.take() {
            let _ = handle.join();
        }
    }
}

/// Convenience one-shot wrapper creating a [`LaneARuntime`] and executing a single `.cwasm`.
pub fn execute_sealed_cwasm(
    cwasm_memfd: &SealedMemfd,
    meta: &AotEnvelopeMeta,
    key: &[u8; 32],
    tunables: &EngineTunables,
    limits: &LaneALimits,
    stdin_bytes: &[u8],
    apply_two_phase_seccomp: bool,
) -> Result<ResourceReceipt, SandboxError> {
    let runtime = LaneARuntime::new(*tunables)?;
    runtime.execute_sealed_cwasm(
        cwasm_memfd,
        meta,
        key,
        limits,
        stdin_bytes,
        apply_two_phase_seccomp,
    )
}

fn run_component_entry(
    store: &mut Store<LaneAState>,
    pre: wasmtime::component::InstancePre<LaneAState>,
) -> wasmtime::Result<i32> {
    if let Ok(cmd_pre) = CommandPre::new(pre.clone()) {
        let cmd = cmd_pre.instantiate(&mut *store)?;
        return match cmd.wasi_cli_run().call_run(store)? {
            Ok(()) => Ok(0),
            Err(()) => Ok(1),
        };
    }
    let instance = pre.instantiate(&mut *store)?;
    if let Ok(func) = instance.get_typed_func::<(), (i32,)>(&mut *store, "run") {
        let (code,) = func.call(&mut *store, ())?;
        return Ok(code);
    }
    let func = instance.get_typed_func::<(), ()>(&mut *store, "run")?;
    func.call(&mut *store, ())?;
    Ok(0)
}

fn classify_wasm_error(err: &wasmtime::Error) -> String {
    match err.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => "trap:out_of_fuel".to_owned(),
        Some(Trap::Interrupt) => "trap:epoch_interrupt".to_owned(),
        Some(Trap::MemoryOutOfBounds) => "trap:memory_out_of_bounds".to_owned(),
        Some(Trap::UnreachableCodeReached) => "trap:unreachable".to_owned(),
        Some(Trap::StackOverflow) => "trap:stack_overflow".to_owned(),
        Some(other) => format!("trap:{other:?}"),
        None => "error:component_runtime".to_owned(),
    }
}
