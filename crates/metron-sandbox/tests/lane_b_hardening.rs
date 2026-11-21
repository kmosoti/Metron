//! Negative-control hardening tests for Lane B direct supervisor (`use_systemd = false`),
//! `SandboxReceipt`, `RLIMIT_FSIZE`, `RLIMIT_AS`, wall-clock timeout, scratch space reserve,
//! and `/proc/self/fd` hygiene.

use metron_sandbox::{
    CgroupLimits, ExecSource, NamespacePolicy, NativeJailConfig, SandboxError, probe_kernel_abi,
    resolve_manager_tool, run_direct_jail, sha256_regular_file, userns_supported,
    verify_fd_hygiene,
};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

fn main() -> ExitCode {
    if let Ok(mode) = std::env::var("METRON_HARDEN_PROBE") {
        return run_probe(&mode);
    }

    // If invoked from an IDE/terminal that leaked non-CLOEXEC FDs > 2 into `cargo test`,
    // re-exec this test runner once with FDs > 2 closed so `metron-jail`'s strict
    // `/proc/self/fd` fail-closed check can be exercised cleanly.
    if std::env::var_os("METRON_TEST_CLEAN_FD").is_none() && verify_fd_hygiene(&[]).is_err() {
        let exe = std::env::current_exe().expect("current_exe");
        let args: Vec<String> = std::env::args().skip(1).collect();
        let err = Command::new("bash")
            .args([
                "-c",
                "for p in /proc/$$/fd/*; do f=${p##*/}; case \"$f\" in *[!0-9]*|\"\") continue;; esac; [ \"$f\" -gt 2 ] && eval \"exec ${f}>&-\" 2>/dev/null || true; done; exec \"$@\"",
                "_",
            ])
            .arg(exe)
            .args(&args)
            .env("METRON_TEST_CLEAN_FD", "1")
            .exec();
        panic!("clean-fd re-exec failed: {err}");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let tests: &[(&str, fn())] = &[
        (
            "direct_jail_timeout_kills_runaway_child",
            direct_jail_timeout_kills_runaway_child,
        ),
        (
            "direct_jail_enforces_rlimit_fsize_and_rlimit_as",
            direct_jail_enforces_rlimit_fsize_and_rlimit_as,
        ),
        (
            "low_scratch_space_returns_insufficient_scratch_not_timeout",
            low_scratch_space_returns_insufficient_scratch_not_timeout,
        ),
        (
            "receipt_and_fd_hygiene_and_path_tool_resolution",
            receipt_and_fd_hygiene_and_path_tool_resolution,
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

fn run_probe(mode: &str) -> ExitCode {
    match mode {
        "ok" => {
            println!("harden-ok");
            ExitCode::SUCCESS
        }
        "runaway_sleep" => {
            std::thread::sleep(Duration::from_secs(30));
            ExitCode::SUCCESS
        }
        "exceed_fsize" => {
            let scratch = PathBuf::from(std::env::var("HOME").unwrap());
            let mut file = fs::File::create(scratch.join("big.bin")).unwrap();
            let chunk = [0x42u8; 64 * 1024];
            for _ in 0..8 {
                if file.write_all(&chunk).is_err() {
                    return ExitCode::from(42);
                }
            }
            ExitCode::SUCCESS
        }
        "exceed_as" => {
            let mut blocks = Vec::new();
            for _ in 0..64 {
                let mut v = Vec::new();
                if v.try_reserve_exact(16 * 1024 * 1024).is_err() {
                    return ExitCode::from(43);
                }
                v.resize(16 * 1024 * 1024, 0x5Au8);
                blocks.push(v);
            }
            ExitCode::SUCCESS
        }
        other => panic!("unknown probe mode: {other}"),
    }
}

fn make_disk_dir(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp-sandbox-tests");
    let _ = fs::create_dir_all(&base);
    let root = base.join(format!("harden-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("scratch")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    root
}

fn direct_cfg(root: &Path, mode: &str) -> NativeJailConfig {
    let exe = std::env::current_exe().unwrap();
    let sha = sha256_regular_file(&exe).unwrap();
    let scratch = root.join("scratch");
    NativeJailConfig {
        cgroup: CgroupLimits {
            unit_name: format!("metron-harden-{mode}-{}", std::process::id()),
            scratch_reserve_bytes: 0,
            ..CgroupLimits::default()
        },
        primary: ExecSource::Path {
            path: exe,
            sha256: Some(sha),
            label: format!("harden-{mode}"),
        },
        inherited: Vec::new(),
        args: Vec::new(),
        workspace: None,
        writable_workspace: false,
        scratch: scratch.clone(),
        ro_inputs: Vec::new(),
        workdir: scratch,
        env: vec![("METRON_HARDEN_PROBE".to_owned(), mode.to_owned())],
        trampoline_bin: Some(PathBuf::from(env!("CARGO_BIN_EXE_metron-jail"))),
        namespace_policy: if userns_supported() {
            NamespacePolicy::Require
        } else {
            NamespacePolicy::AllowDegraded
        },
        allow_unix_sockets: false,
    }
}

fn direct_jail_timeout_kills_runaway_child() {
    let root = make_disk_dir("timeout");
    let mut cfg = direct_cfg(&root, "runaway_sleep");
    cfg.cgroup.timeout_ms = 300;
    let started = Instant::now();
    let out = run_direct_jail(&cfg, None).expect("direct timeout run");
    let elapsed = started.elapsed();
    assert!(
        out.timed_out,
        "direct supervisor must report timed_out; exit={:?} stderr={}",
        out.exit_code,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.success);
    assert_eq!(out.exit_code, None);
    assert!(
        elapsed < Duration::from_secs(5),
        "runaway child was not killed promptly: {elapsed:?}"
    );
    fs::remove_dir_all(&root).unwrap();
}

fn direct_jail_enforces_rlimit_fsize_and_rlimit_as() {
    let root = make_disk_dir("rlimits");

    let mut fsize_cfg = direct_cfg(&root, "exceed_fsize");
    fsize_cfg.cgroup.limit_fsize_bytes = 32 * 1024;
    let fsize_out = run_direct_jail(&fsize_cfg, None).expect("fsize run");
    assert!(
        !fsize_out.success,
        "child writing past RLIMIT_FSIZE must fail"
    );

    let mut as_cfg = direct_cfg(&root, "exceed_as");
    as_cfg.cgroup.memory_max_bytes = 128 * 1024 * 1024;
    let as_out = run_direct_jail(&as_cfg, None).expect("as run");
    assert!(!as_out.success, "child allocating past RLIMIT_AS must fail");

    fs::remove_dir_all(&root).unwrap();
}

fn low_scratch_space_returns_insufficient_scratch_not_timeout() {
    let root = make_disk_dir("disk-space");
    let mut cfg = direct_cfg(&root, "ok");
    cfg.cgroup.scratch_reserve_bytes = u64::MAX;
    let err = run_direct_jail(&cfg, None).expect_err("must reject insufficient scratch space");
    assert!(
        matches!(err, SandboxError::InsufficientScratchSpace { .. }),
        "expected InsufficientScratchSpace, got {err:?}"
    );
    fs::remove_dir_all(&root).unwrap();
}

fn receipt_and_fd_hygiene_and_path_tool_resolution() {
    let root = make_disk_dir("receipt");
    let cfg = direct_cfg(&root, "ok");
    let out = run_direct_jail(&cfg, None).expect("direct ok run");
    assert!(
        out.success,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"harden-ok\n");

    let receipt = out.receipt.expect("metron-jail must emit SandboxReceipt");
    let expected_abi = probe_kernel_abi().expect("host kernel Landlock ABI");
    assert_eq!(receipt.landlock.abi, expected_abi);
    assert!(receipt.landlock.fs_enforced);
    assert!(receipt.seccomp_active);
    assert!(receipt.rlimits_applied);
    assert!(receipt.fd_hygiene_verified);
    if userns_supported() {
        assert!(receipt.namespaces_active);
        assert!(receipt.namespaces_degraded_reason.is_none());
    } else {
        assert!(!receipt.namespaces_active);
        assert!(receipt.namespaces_degraded_reason.is_some());
    }

    // Open an unexpected extra file descriptor and prove BOTH verify_fd_hygiene AND
    // `metron-jail` itself fail closed when a non-CLOEXEC FD leaks across exec.
    let leaked = fs::File::open("/dev/null").expect("open /dev/null");
    let err = verify_fd_hygiene(&[]).expect_err("leaked fd must fail closed");
    assert!(
        matches!(err, SandboxError::Denied(ref msg) if msg.contains("unexpected open file descriptor")),
        "expected unexpected FD error, got {err:?}"
    );
    rustix::io::fcntl_setfd(&leaked, rustix::io::FdFlags::empty()).expect("clear cloexec");
    let leak_out = run_direct_jail(&cfg, None).expect("run with leaked fd");
    assert!(
        !leak_out.success && leak_out.exit_code == Some(126),
        "metron-jail must fail closed on leaked non-CLOEXEC fd; exit={:?} stderr={}",
        leak_out.exit_code,
        String::from_utf8_lossy(&leak_out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&leak_out.stderr).contains("unexpected open file descriptor"),
        "expected FD hygiene denial in stderr: {}",
        String::from_utf8_lossy(&leak_out.stderr)
    );
    drop(leaked);

    // PATH-based resolution must resolve standard tools without hardcoding /usr/bin.
    assert!(resolve_manager_tool("METRON_NONEXISTENT_TOOL_OVERRIDE", "sh").is_some());

    fs::remove_dir_all(&root).unwrap();
}
