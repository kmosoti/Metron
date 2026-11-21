//! Wave 2 hardening tests for Lane A (`wasm_aot`):
//! - Zero HMAC key bytes written to `LaneASpec` JSON on scratch disk
//! - `EngineTunables::digest()` includes `max_cwasm_bytes`
//! - `LaneARuntime` warm pool reuse + instant external `bump_epoch` cancellation
//! - Hardened `run_lane_a_jail(use_systemd: false)` wall-clock timeout + `SIGKILL`

use metron_sandbox::{
    CgroupLimits, EngineTunables, LandlockPolicy, LaneALimits, LaneARequest, LaneARuntime,
    PINNED_WASMTIME_VERSION, host_target_triple, run_lane_a_jail, seal_bytes,
    wasm_aot::{AotEnvelopeMeta, encode_hex},
    wasm_aot_jail::LaneASpec,
};
use metron_wasm_aot_loader::{AotSeal, compute_seal_tag};
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use wasmtime::Engine;

const SPIN_WAT: &str = r#"
(component
  (type $res (result))
  (type $run-sig (func (result $res)))
  (core module $m
    (memory (export "mem") 1 2)
    (func (export "run") (result i32)
      (loop $spin (br $spin))
      i32.const 0))
  (core instance $i (instantiate $m))
  (func $run-fn (type $run-sig) (canon lift (core func $i "run")))
  (instance $wasi-run (export "run" (func $run-fn)))
  (export "wasi:cli/run@0.2.0" (instance $wasi-run)))
"#;

const FAST_WAT: &str = r#"
(component
  (type $res (result))
  (type $run-sig (func (result $res)))
  (core module $m
    (memory (export "mem") 1 2)
    (func (export "run") (result i32)
      (i32.store (i32.const 0) (i32.const 7))
      i32.const 0))
  (core instance $i (instantiate $m))
  (func $run-fn (type $run-sig) (canon lift (core func $i "run")))
  (instance $wasi-run (export "run" (func $run-fn)))
  (export "wasi:cli/run@0.2.0" (instance $wasi-run)))
"#;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tests: &[(&str, fn())] = &[
        (
            "lane_a_spec_has_zero_hmac_key_and_digest_binds_max_cwasm_bytes",
            lane_a_spec_has_zero_hmac_key_and_digest_binds_max_cwasm_bytes,
        ),
        (
            "lane_a_warm_runtime_pool_and_bump_epoch_cancellation",
            lane_a_warm_runtime_pool_and_bump_epoch_cancellation,
        ),
        (
            "lane_a_direct_child_enforces_real_wall_clock_timeout",
            lane_a_direct_child_enforces_real_wall_clock_timeout,
        ),
    ];
    if args.iter().any(|a| a == "--list") {
        for &(name, _) in tests {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }
    let filter = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    let mut ran = 0usize;
    for &(name, func) in tests {
        if let Some(f) = filter
            && !name.contains(f)
        {
            continue;
        }
        print!("test {name} ... ");
        func();
        println!("ok");
        ran += 1;
    }
    println!("test result: ok. {ran} passed; 0 failed; 0 ignored");
    ExitCode::SUCCESS
}

fn compile_seal(
    wat: &str,
    tunables: &EngineTunables,
    key: &[u8; 32],
) -> (Vec<u8>, AotEnvelopeMeta) {
    let bytes = wat::parse_bytes(wat.as_bytes()).unwrap().into_owned();
    let engine = Engine::new(&tunables.to_wasmtime_config().unwrap()).unwrap();
    let cwasm = engine.precompile_component(&bytes).unwrap();
    let target = host_target_triple().unwrap();
    let td = tunables.digest();
    let cb = *blake3::hash(&cwasm).as_bytes();
    let seal = AotSeal {
        wasmtime_version: PINNED_WASMTIME_VERSION,
        target_triple: target,
        engine_tunables_digest: td,
        cwasm_blake3: cb,
        hmac_tag: [0u8; 32],
    };
    let tag = compute_seal_tag(key, &seal).unwrap();
    let meta = AotEnvelopeMeta {
        wasmtime_version: PINNED_WASMTIME_VERSION.to_owned(),
        target_triple: target.to_owned(),
        engine_tunables_digest_hex: encode_hex(&td),
        cwasm_blake3_hex: encode_hex(&cb),
        hmac_tag_hex: encode_hex(&tag),
    };
    (cwasm, meta)
}

fn lane_a_spec_has_zero_hmac_key_and_digest_binds_max_cwasm_bytes() {
    let key = [0xABu8; 32];
    let key_hex = encode_hex(&key);
    let t1 = EngineTunables::default();
    let t2 = EngineTunables {
        max_cwasm_bytes: 16 * 1024 * 1024,
        ..t1
    };
    assert_ne!(
        t1.digest(),
        t2.digest(),
        "EngineTunables::digest must include max_cwasm_bytes"
    );

    let (_, meta) = compile_seal(FAST_WAT, &t1, &key);
    let spec = LaneASpec {
        meta,
        tunables: t1,
        fuel_budget: 100_000,
        max_memory_bytes: 1024 * 1024,
        max_table_elements: 1024,
        timeout_ms: 1000,
        max_cwasm_bytes: t1.max_cwasm_bytes,
        max_output_bytes: 4096,
        stdin_bytes: Vec::new(),
        landlock: LandlockPolicy::lane_b(Vec::new(), Vec::new()),
        rlimits: None,
    };
    let json = serde_json::to_string(&spec).unwrap();
    assert!(
        !json.contains(&key_hex) && !json.contains("hmac_key"),
        "LaneASpec JSON must never contain secret key material: {json}"
    );
}

fn lane_a_warm_runtime_pool_and_bump_epoch_cancellation() {
    let key = [0x22u8; 32];
    let tunables = EngineTunables::default();
    let runtime = Arc::new(LaneARuntime::new(tunables).unwrap());

    // 1. Reuse the same warm pooling Engine + shared ticker across multiple invocations.
    let (fast_cwasm, fast_meta) = compile_seal(FAST_WAT, &tunables, &key);
    let fast_memfd = seal_bytes("fast", &fast_cwasm, None, true).unwrap();
    let limits = LaneALimits::default();
    let r1 = runtime
        .execute_sealed_cwasm(&fast_memfd, &fast_meta, &key, &limits, &[], false)
        .unwrap();
    let r2 = runtime
        .execute_sealed_cwasm(&fast_memfd, &fast_meta, &key, &limits, &[], false)
        .unwrap();
    assert_eq!(r1.exit_code, 0);
    assert_eq!(r1, r2);

    // 2. External cancellation via bump_epoch interrupts a long-deadline guest promptly.
    let (spin_cwasm, spin_meta) = compile_seal(SPIN_WAT, &tunables, &key);
    let spin_memfd = seal_bytes("spin", &spin_cwasm, None, true).unwrap();
    let cancel_rt = Arc::clone(&runtime);
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        cancel_rt.bump_epoch(5_000);
    });
    let started = Instant::now();
    let long_limits = LaneALimits {
        fuel_budget: 100_000_000_000,
        timeout_ms: 30_000,
        ..LaneALimits::default()
    };
    let r_cancel = runtime
        .execute_sealed_cwasm(&spin_memfd, &spin_meta, &key, &long_limits, &[], false)
        .unwrap();
    canceller.join().unwrap();
    assert_eq!(r_cancel.trapped.as_deref(), Some("trap:epoch_interrupt"));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "bump_epoch did not interrupt promptly: {:?}",
        started.elapsed()
    );
}

fn lane_a_direct_child_enforces_real_wall_clock_timeout() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp-sandbox-tests");
    let scratch = base.join(format!("lane-a-timeout-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();

    let key = [0x44u8; 32];
    let tunables = EngineTunables::default();
    let (spin_cwasm, spin_meta) = compile_seal(SPIN_WAT, &tunables, &key);
    let spin_memfd = seal_bytes("spin-timeout", &spin_cwasm, None, true).unwrap();
    let jail_bin = PathBuf::from(env!("CARGO_BIN_EXE_metron-jail"));

    // Set huge guest fuel & Wasm epoch timeout (30s) but tight direct-supervisor deadline (250ms)
    // to prove run_direct_supervised kills the runaway Lane A child with SIGKILL.
    let limits = LaneALimits {
        fuel_budget: 100_000_000_000,
        timeout_ms: 30_000,
        cgroup: CgroupLimits {
            timeout_ms: 250,
            scratch_reserve_bytes: 0,
            ..CgroupLimits::default()
        },
        ..LaneALimits::default()
    };
    let started = Instant::now();
    let err = run_lane_a_jail(&LaneARequest {
        cwasm_memfd: &spin_memfd,
        meta: &spin_meta,
        key: &key,
        tunables: &tunables,
        limits: &limits,
        scratch: &scratch,
        stdin_bytes: &[],
        trampoline_bin: Some(&jail_bin),
        use_systemd: false,
    })
    .expect_err("direct supervisor must kill runaway Lane A child");
    assert!(
        err.to_string().contains("timed out"),
        "expected timeout error, got: {err}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    let _ = fs::remove_dir_all(&scratch);
}
