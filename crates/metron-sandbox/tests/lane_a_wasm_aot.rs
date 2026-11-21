//! Real WAT/Wasm Lane A (`wasm_aot`) integration tests:
//! (a) AOT compile -> HMAC/BLAKE3 seal -> Landlock + seccomp(PROT_EXEC) jail -> deterministic ResourceReceipt
//! (b) Tampered .cwasm, wrong HMAC, wrong version/triple/tunables, and unsealed memfd rejection
//! (c) Deterministic fuel exhaustion trap, StoreLimits memory cap, and epoch interruption

use metron_sandbox::{
    CgroupLimits, EngineTunables, LandlockPolicy, LaneALimits, LaneARequest,
    PINNED_WASMTIME_VERSION, SeccompPolicy, apply_landlock, host_target_triple, install_seccomp,
    run_lane_a_jail, seal_bytes, systemd_user_available,
    wasm_aot::{AotEnvelopeMeta, ResourceReceipt, execute_sealed_cwasm},
};
use metron_wasm_aot_loader::{AotSeal, compute_seal_tag, deserialize_verified_memfd};
use rustix::fs::{MemfdFlags, memfd_create};
use std::{
    fs::{self, File},
    io::{ErrorKind, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};
use wasmtime::Engine;

fn compile_and_seal_component(
    wat_src: &[u8],
    tunables: &EngineTunables,
    key: &[u8; 32],
) -> Result<(Vec<u8>, AotEnvelopeMeta), metron_sandbox::SandboxError> {
    let component_bytes = wat::parse_bytes(wat_src).unwrap().into_owned();
    let cfg = tunables.to_wasmtime_config()?;
    let engine = Engine::new(&cfg).unwrap();
    let cwasm = engine.precompile_component(&component_bytes).unwrap();
    let target = host_target_triple()?;
    let tunables_digest = tunables.digest();
    let cwasm_blake3 = *blake3::hash(&cwasm).as_bytes();
    let seal = AotSeal {
        wasmtime_version: PINNED_WASMTIME_VERSION,
        target_triple: target,
        engine_tunables_digest: tunables_digest,
        cwasm_blake3,
        hmac_tag: [0u8; 32],
    };
    let tag = compute_seal_tag(key, &seal).unwrap();
    Ok((
        cwasm,
        AotEnvelopeMeta {
            wasmtime_version: PINNED_WASMTIME_VERSION.to_owned(),
            target_triple: target.to_owned(),
            engine_tunables_digest_hex: hex(&tunables_digest),
            cwasm_blake3_hex: hex(&cwasm_blake3),
            hmac_tag_hex: hex(&tag),
        },
    ))
}

fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&b| [H[(b >> 4) as usize] as char, H[(b & 0x0f) as usize] as char])
        .collect()
}

const DETERMINISTIC_WORK_WAT: &str = r#"
(component
  (type $res (result))
  (type $run-sig (func (result $res)))
  (core module $m
    (memory (export "mem") 1 4)
    (func (export "run") (result i32)
      (local $i i32)
      ;; Grow linear memory deterministically by 1 page (64 KiB -> 128 KiB total).
      (drop (memory.grow (i32.const 1)))
      (i32.store (i32.const 65536) (i32.const 42))
      (loop $work
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br_if $work (i32.lt_s (local.get $i) (i32.const 600)))
      )
      i32.const 0
    )
  )
  (core instance $i (instantiate $m))
  (func $run-fn (type $run-sig) (canon lift (core func $i "run")))
  (instance $wasi-run (export "run" (func $run-fn)))
  (export "wasi:cli/run@0.2.0" (instance $wasi-run))
)
"#;

const INFINITE_FUEL_WAT: &str = r#"
(component
  (type $res (result))
  (type $run-sig (func (result $res)))
  (core module $m
    (memory (export "mem") 1 2)
    (func (export "run") (result i32)
      (loop $spin
        (br $spin)
      )
      i32.const 0
    )
  )
  (core instance $i (instantiate $m))
  (func $run-fn (type $run-sig) (canon lift (core func $i "run")))
  (instance $wasi-run (export "run" (func $run-fn)))
  (export "wasi:cli/run@0.2.0" (instance $wasi-run))
)
"#;

const MEMORY_GROW_CAP_WAT: &str = r#"
(component
  (type $res (result))
  (type $run-sig (func (result $res)))
  (core module $m
    (memory (export "mem") 1 64)
    (func (export "run") (result i32)
      ;; Attempt to grow by 16 pages (total 17 pages > 4-page 256 KiB cap);
      ;; returns -1 when StoreLimits denies growth cleanly.
      (drop (memory.grow (i32.const 16)))
      i32.const 0
    )
  )
  (core instance $i (instantiate $m))
  (func $run-fn (type $run-sig) (canon lift (core func $i "run")))
  (instance $wasi-run (export "run" (func $run-fn)))
  (export "wasi:cli/run@0.2.0" (instance $wasi-run))
)
"#;

fn main() -> ExitCode {
    if std::env::var_os("METRON_LANE_A_POSTLOAD_PROBE").is_some() {
        return run_postload_probe();
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        for name in [
            "lane_a_compile_seal_jail_exec_and_receipt_determinism",
            "lane_a_postload_jail_blocks_home_socket_and_prot_exec",
            "lane_a_rejects_tampered_cwasm_and_unsealed_memfd_before_deserialize",
            "lane_a_deterministic_fuel_exhaustion_trap",
            "lane_a_store_limits_and_epoch_interruption",
        ] {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }

    let filter = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    let tests: &[(&str, fn())] = &[
        (
            "lane_a_compile_seal_jail_exec_and_receipt_determinism",
            lane_a_compile_seal_jail_exec_and_receipt_determinism,
        ),
        (
            "lane_a_postload_jail_blocks_home_socket_and_prot_exec",
            lane_a_postload_jail_blocks_home_socket_and_prot_exec,
        ),
        (
            "lane_a_rejects_tampered_cwasm_and_unsealed_memfd_before_deserialize",
            lane_a_rejects_tampered_cwasm_and_unsealed_memfd_before_deserialize,
        ),
        (
            "lane_a_deterministic_fuel_exhaustion_trap",
            lane_a_deterministic_fuel_exhaustion_trap,
        ),
        (
            "lane_a_store_limits_and_epoch_interruption",
            lane_a_store_limits_and_epoch_interruption,
        ),
    ];

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

fn run_postload_probe() -> ExitCode {
    let dir = PathBuf::from(std::env::var("PROBE_SCRATCH").expect("PROBE_SCRATCH"));
    let real_home = std::env::var("PROBE_REAL_HOME").expect("PROBE_REAL_HOME");
    let cwasm_path = dir.join("probe.cwasm");
    let meta_path = dir.join("probe.seal.json");
    let cwasm_bytes = fs::read(&cwasm_path).expect("read probe.cwasm");
    let meta: AotEnvelopeMeta =
        serde_json::from_slice(&fs::read(&meta_path).expect("read meta")).expect("decode meta");
    let cwasm_memfd = seal_bytes("probe-cwasm", &cwasm_bytes, None, true).expect("seal memfd");

    let key = [0x42u8; 32];
    let tunables = EngineTunables::default();
    let limits = LaneALimits::default();

    rustix::thread::set_no_new_privs(true).expect("PR_SET_NO_NEW_PRIVS");
    apply_landlock(&LandlockPolicy::lane_b(Vec::new(), vec![dir.clone()])).expect("apply_landlock");

    // Deserializes .cwasm, installs seccomp(deny_prot_exec: true) POST-LOAD, then runs guest.
    let receipt = execute_sealed_cwasm(&cwasm_memfd, &meta, &key, &tunables, &limits, &[], true)
        .expect("wasm execution inside Landlock + post-load PROT_EXEC seccomp");
    assert_eq!(receipt.exit_code, 0);
    assert!(receipt.fuel_consumed > 0);
    assert!(receipt.peak_memory_bytes >= 128 * 1024);

    // Prove Landlock, seccomp network deny, and post-load PROT_EXEC deny are ALL enforced now.
    assert_eq!(
        fs::read_dir(&real_home).unwrap_err().kind(),
        ErrorKind::PermissionDenied
    );
    assert_eq!(
        TcpListener::bind("127.0.0.1:0").unwrap_err().kind(),
        ErrorKind::PermissionDenied
    );
    let exec_err = Command::new("/usr/bin/true").status().map(|s| s.success());
    assert!(
        !matches!(exec_err, Ok(true)),
        "dynamically-linked exec must fail when PROT_EXEC is blocked post-load: {exec_err:?}"
    );
    let status = fs::read_to_string("/proc/self/status").expect("proc status");
    assert!(
        status.lines().any(|l| l.starts_with("Seccomp:\t2")),
        "expected Seccomp filter mode 2"
    );

    println!("{}", serde_json::to_string(&receipt).unwrap());
    ExitCode::SUCCESS
}

fn lane_a_compile_seal_jail_exec_and_receipt_determinism() {
    let dir = TempDir::new("metron-lane-a-det");
    let scratch = dir.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();

    let key = [0x11u8; 32];
    let tunables = EngineTunables::default();
    let (cwasm, meta) =
        compile_and_seal_component(DETERMINISTIC_WORK_WAT.as_bytes(), &tunables, &key).unwrap();
    let memfd = seal_bytes("det-cwasm", &cwasm, None, true).unwrap();

    let limits = LaneALimits {
        fuel_budget: 500_000,
        max_memory_bytes: 16 * 1024 * 1024,
        cgroup: CgroupLimits {
            unit_name: format!("metron-lane-a-det-{}", std::process::id()),
            ..CgroupLimits::default()
        },
        ..LaneALimits::default()
    };
    let jail_bin = PathBuf::from(env!("CARGO_BIN_EXE_metron-jail"));
    let use_systemd = systemd_user_available();

    let r1 = run_lane_a_jail(&LaneARequest {
        cwasm_memfd: &memfd,
        meta: &meta,
        key: &key,
        tunables: &tunables,
        limits: &limits,
        scratch: &scratch,
        stdin_bytes: &[],
        trampoline_bin: Some(&jail_bin),
        use_systemd,
    })
    .unwrap();

    let limits2 = LaneALimits {
        cgroup: CgroupLimits {
            unit_name: format!("metron-lane-a-det2-{}", std::process::id()),
            ..limits.cgroup.clone()
        },
        ..limits
    };
    let r2 = run_lane_a_jail(&LaneARequest {
        cwasm_memfd: &memfd,
        meta: &meta,
        key: &key,
        tunables: &tunables,
        limits: &limits2,
        scratch: &scratch,
        stdin_bytes: &[],
        trampoline_bin: Some(&jail_bin),
        use_systemd: false,
    })
    .unwrap();

    assert_eq!(r1.exit_code, 0);
    assert!(r1.trapped.is_none());
    assert!(r1.fuel_consumed > 0);
    assert!(r1.peak_memory_bytes >= 128 * 1024);
    assert_eq!(r1, r2, "ResourceReceipt must be deterministic across runs");
}

fn lane_a_postload_jail_blocks_home_socket_and_prot_exec() {
    let dir = TempDir::new("metron-lane-a-postload");
    let scratch = dir.path().join("scratch");
    let fake_home = dir.path().join("fake-home");
    fs::create_dir_all(&scratch).unwrap();
    fs::create_dir_all(&fake_home).unwrap();
    fs::write(fake_home.join("vault.key"), b"secret").unwrap();

    let key = [0x42u8; 32];
    let tunables = EngineTunables::default();
    let (cwasm, meta) =
        compile_and_seal_component(DETERMINISTIC_WORK_WAT.as_bytes(), &tunables, &key).unwrap();
    fs::write(scratch.join("probe.cwasm"), &cwasm).unwrap();
    fs::write(
        scratch.join("probe.seal.json"),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();

    let self_bin = std::env::current_exe().unwrap();
    let out = Command::new(self_bin)
        .env("METRON_LANE_A_POSTLOAD_PROBE", "1")
        .env("PROBE_SCRATCH", &scratch)
        .env("PROBE_REAL_HOME", &fake_home)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "postload probe failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let receipt: ResourceReceipt = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt.exit_code, 0);
    assert!(receipt.fuel_consumed > 0);
}

fn lane_a_rejects_tampered_cwasm_and_unsealed_memfd_before_deserialize() {
    let key = [0x77u8; 32];
    let tunables = EngineTunables::default();
    let (mut cwasm, meta) =
        compile_and_seal_component(DETERMINISTIC_WORK_WAT.as_bytes(), &tunables, &key).unwrap();
    let limits = LaneALimits::default();

    // 1. Tampered .cwasm byte rejected by BLAKE3 before Component::deserialize.
    let mid = cwasm.len() / 2;
    cwasm[mid] ^= 0xff;
    let tampered_memfd = seal_bytes("tampered", &cwasm, None, true).unwrap();
    let err = execute_sealed_cwasm(&tampered_memfd, &meta, &key, &tunables, &limits, &[], false)
        .unwrap_err();
    assert!(
        err.to_string().contains("blake3"),
        "expected blake3 rejection, got: {err}"
    );
    cwasm[mid] ^= 0xff;

    // 2. Wrong HMAC key rejected before Component::deserialize.
    let good_memfd = seal_bytes("good", &cwasm, None, true).unwrap();
    let err = execute_sealed_cwasm(
        &good_memfd,
        &meta,
        &[0x99u8; 32],
        &tunables,
        &limits,
        &[],
        false,
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("hmac"),
        "expected hmac rejection, got: {err}"
    );

    // 3. Mismatched version / target / tunables rejected before deserialize.
    let mut bad_ver = meta.clone();
    bad_ver.wasmtime_version = "50.0.0".into();
    assert!(
        execute_sealed_cwasm(&good_memfd, &bad_ver, &key, &tunables, &limits, &[], false).is_err()
    );

    let bad_tunables = EngineTunables {
        max_wasm_stack_bytes: 256 * 1024,
        ..tunables
    };
    assert!(
        execute_sealed_cwasm(&good_memfd, &meta, &key, &bad_tunables, &limits, &[], false).is_err()
    );

    // 4. Unsealed memfd (missing F_SEAL_WRITE) rejected directly by loader before deserialize.
    let unsealed_fd =
        File::from(memfd_create("unsealed-cwasm", MemfdFlags::ALLOW_SEALING).unwrap());
    (&unsealed_fd).write_all(&cwasm).unwrap();
    let engine = Engine::new(&tunables.to_wasmtime_config().unwrap()).unwrap();
    let (td, cb, tag) = meta.decode_parts().unwrap();
    let seal = AotSeal {
        wasmtime_version: &meta.wasmtime_version,
        target_triple: &meta.target_triple,
        engine_tunables_digest: td,
        cwasm_blake3: cb,
        hmac_tag: tag,
    };
    let res = deserialize_verified_memfd(
        &engine,
        &unsealed_fd,
        &key,
        &seal,
        (
            PINNED_WASMTIME_VERSION,
            host_target_triple().unwrap(),
            &tunables.digest(),
            limits.max_cwasm_bytes,
        ),
    );
    assert!(
        res.unwrap_err().contains("seals"),
        "expected missing F_SEAL_WRITE seal rejection"
    );
}

fn lane_a_deterministic_fuel_exhaustion_trap() {
    let dir = TempDir::new("metron-lane-a-fuel");
    let scratch = dir.path().join("scratch");
    fs::create_dir_all(&scratch).unwrap();

    let key = [0x33u8; 32];
    let tunables = EngineTunables::default();
    let (cwasm, meta) =
        compile_and_seal_component(INFINITE_FUEL_WAT.as_bytes(), &tunables, &key).unwrap();
    let memfd = seal_bytes("fuel-trap", &cwasm, None, true).unwrap();
    let limits = LaneALimits {
        fuel_budget: 25_000,
        timeout_ms: 5_000,
        ..LaneALimits::default()
    };
    let jail_bin = PathBuf::from(env!("CARGO_BIN_EXE_metron-jail"));

    let r1 = run_lane_a_jail(&LaneARequest {
        cwasm_memfd: &memfd,
        meta: &meta,
        key: &key,
        tunables: &tunables,
        limits: &limits,
        scratch: &scratch,
        stdin_bytes: &[],
        trampoline_bin: Some(&jail_bin),
        use_systemd: false,
    })
    .unwrap();
    let r2 = run_lane_a_jail(&LaneARequest {
        cwasm_memfd: &memfd,
        meta: &meta,
        key: &key,
        tunables: &tunables,
        limits: &limits,
        scratch: &scratch,
        stdin_bytes: &[],
        trampoline_bin: Some(&jail_bin),
        use_systemd: false,
    })
    .unwrap();

    assert_eq!(r1.exit_code, 124);
    assert_eq!(r1.trapped.as_deref(), Some("trap:out_of_fuel"));
    assert_eq!(r1, r2, "fuel exhaustion trap must be deterministic");
}

fn lane_a_store_limits_and_epoch_interruption() {
    let key = [0x55u8; 32];
    let tunables = EngineTunables::default();

    // 1. StoreLimits enforces hard ceiling on memory growth.
    let (cwasm_mem, meta_mem) =
        compile_and_seal_component(MEMORY_GROW_CAP_WAT.as_bytes(), &tunables, &key).unwrap();
    let memfd_mem = seal_bytes("mem-cap", &cwasm_mem, None, true).unwrap();
    let limits_mem = LaneALimits {
        max_memory_bytes: 256 * 1024,
        ..LaneALimits::default()
    };
    let r_mem = execute_sealed_cwasm(
        &memfd_mem,
        &meta_mem,
        &key,
        &tunables,
        &limits_mem,
        &[],
        false,
    )
    .unwrap();
    assert!(
        r_mem.peak_memory_bytes <= 256 * 1024,
        "peak memory {} exceeded StoreLimits 256 KiB",
        r_mem.peak_memory_bytes
    );

    // 2. Epoch interruption aborts long-running loop when fuel budget is still ample.
    let (cwasm_spin, meta_spin) =
        compile_and_seal_component(INFINITE_FUEL_WAT.as_bytes(), &tunables, &key).unwrap();
    let memfd_spin = seal_bytes("epoch-spin", &cwasm_spin, None, true).unwrap();
    let limits_epoch = LaneALimits {
        fuel_budget: 100_000_000_000,
        timeout_ms: 50,
        ..LaneALimits::default()
    };
    let r_epoch = execute_sealed_cwasm(
        &memfd_spin,
        &meta_spin,
        &key,
        &tunables,
        &limits_epoch,
        &[],
        false,
    )
    .unwrap();
    assert_eq!(r_epoch.trapped.as_deref(), Some("trap:epoch_interrupt"));
    assert_eq!(r_epoch.exit_code, 124);
    let _ = (
        install_seccomp as fn(SeccompPolicy) -> _,
        apply_landlock as fn(&LandlockPolicy) -> _,
    );
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let base = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        Self(base)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
