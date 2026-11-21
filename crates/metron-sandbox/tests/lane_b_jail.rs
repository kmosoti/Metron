//! Real kernel negative-control and positive integration tests for Lane B (`native_jail`).
//!
//! Zero `unsafe_code`: uses `harness = false` so this compiled Rust binary acts
//! directly as the sealed `memfd` probe child inside `metron-jail`.

use metron_sandbox::{
    CgroupLimits, ExecSource, NamespacePolicy, NativeJailConfig, run_direct_jail, run_native_jail,
    seal_bytes, sha256_regular_file, systemd_user_available, userns_supported, verify_fd_hygiene,
};
use rustix::net::{AddressFamily, SocketType, socket};
use std::{
    fs,
    io::ErrorKind,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

fn main() -> ExitCode {
    if let Ok(mode) = std::env::var("METRON_SANDBOX_PROBE") {
        return run_probe(&mode);
    }
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
            "lane_b_blocks_home_and_shadow_eacces",
            lane_b_blocks_home_and_shadow_eacces,
        ),
        (
            "lane_b_blocks_network_and_io_uring_eperm",
            lane_b_blocks_network_and_io_uring_eperm,
        ),
        (
            "lane_b_direct_memfd_runs_after_source_delete",
            lane_b_direct_memfd_runs_after_source_delete,
        ),
        (
            "lane_b_systemd_timeout_and_oom_cleanup",
            lane_b_systemd_timeout_and_oom_cleanup,
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
        if filter.is_some_and(|f| !name.contains(f)) {
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
        "check_landlock_eacces" => {
            let target_home = std::env::var("PROBE_REAL_HOME").expect("PROBE_REAL_HOME");
            let home_err = fs::read_dir(&target_home)
                .expect_err("real $HOME read_dir must fail under Landlock");
            assert_eq!(
                home_err.kind(),
                ErrorKind::PermissionDenied,
                "expected EACCES on $HOME, got {home_err:?}"
            );
            assert_eq!(home_err.raw_os_error(), Some(libc::EACCES));

            let shadow_err =
                fs::read("/etc/shadow").expect_err("/etc/shadow must be denied under Landlock");
            assert_eq!(shadow_err.kind(), ErrorKind::PermissionDenied);
            assert_eq!(shadow_err.raw_os_error(), Some(libc::EACCES));

            let ro_file = PathBuf::from(std::env::var("PROBE_RO_FILE").expect("PROBE_RO_FILE"));
            assert_eq!(fs::read_to_string(&ro_file).unwrap(), "ro-ok");
            let write_ro = fs::write(&ro_file, b"tamper").expect_err("ro write denied");
            assert_eq!(write_ro.raw_os_error(), Some(libc::EACCES));

            let scratch_file = PathBuf::from(std::env::var("HOME").unwrap()).join("out.txt");
            fs::write(&scratch_file, b"scratch-ok").unwrap();
            println!("landlock-eacces-ok");
            ExitCode::SUCCESS
        }
        "check_seccomp_eperm" => {
            for (label, family) in [
                ("AF_INET", AddressFamily::INET),
                ("AF_INET6", AddressFamily::INET6),
                ("AF_NETLINK", AddressFamily::NETLINK),
                ("AF_UNIX", AddressFamily::UNIX),
                ("AF_VSOCK", AddressFamily::from_raw(libc::AF_VSOCK as u16)),
                (
                    "AF_BLUETOOTH",
                    AddressFamily::from_raw(libc::AF_BLUETOOTH as u16),
                ),
                ("AF_ALG", AddressFamily::from_raw(libc::AF_ALG as u16)),
                ("AF_XDP", AddressFamily::from_raw(libc::AF_XDP as u16)),
            ] {
                let err = socket(family, SocketType::STREAM, None)
                    .err()
                    .unwrap_or_else(|| panic!("{label} socket() unexpectedly succeeded"));
                assert_eq!(
                    err,
                    rustix::io::Errno::PERM,
                    "{label} socket() expected EPERM, got {err:?}"
                );
            }

            #[allow(deprecated)]
            let unshare_err = rustix::thread::unshare(rustix::thread::UnshareFlags::NEWUSER)
                .expect_err("unshare(CLONE_NEWUSER) unexpectedly succeeded under seccomp");
            assert_eq!(unshare_err, rustix::io::Errno::PERM);

            let uring_err = io_uring::IoUring::new(8)
                .err()
                .expect("io_uring_setup unexpectedly succeeded");
            assert_eq!(
                uring_err.raw_os_error(),
                Some(libc::EPERM),
                "io_uring_setup expected EPERM, got {uring_err:?}"
            );

            if Path::new("/usr/bin/unshare").exists() {
                let status = Command::new("/usr/bin/unshare")
                    .args(["--user", "/bin/true"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .expect("spawn unshare");
                assert!(!status.success(), "unshare must fail under seccomp");
            }

            println!("seccomp-eperm-ok");
            ExitCode::SUCCESS
        }
        "check_memfd_echo" => {
            println!("memfd-direct-ok");
            ExitCode::SUCCESS
        }
        "check_timeout_spawn_child" => {
            // Spawn a grandchild via /proc/self/exe and wait; cgroup KillMode=control-group
            // must kill both parent and child on timeout.
            let mut child = Command::new("/proc/self/exe")
                .env("METRON_SANDBOX_PROBE", "sleep_write")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn child");
            std::thread::sleep(std::time::Duration::from_secs(4));
            let _ = child.wait();
            ExitCode::SUCCESS
        }
        "sleep_write" => {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let scratch = PathBuf::from(std::env::var("HOME").unwrap());
            let _ = fs::write(scratch.join("escaped"), b"bad");
            ExitCode::SUCCESS
        }
        "check_oom" => {
            let mut blocks: Vec<Vec<u8>> = Vec::new();
            loop {
                let mut v = vec![0xA5u8; 8 * 1024 * 1024];
                v[0] = 1;
                blocks.push(v);
            }
        }
        other => {
            eprintln!("unknown probe mode: {other}");
            ExitCode::from(2)
        }
    }
}

fn make_disk_dir(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp-sandbox-tests");
    let _ = fs::create_dir_all(&base);
    let root = base.join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let scratch = root.join("scratch");
    let ro = root.join("ro");
    fs::create_dir_all(&scratch).unwrap();
    fs::create_dir_all(&ro).unwrap();
    root
}

fn base_cfg(root: &Path, mode: &str, extra_env: Vec<(String, String)>) -> NativeJailConfig {
    let exe = std::env::current_exe().unwrap();
    let sha = sha256_regular_file(&exe).unwrap();
    let mut env = vec![("METRON_SANDBOX_PROBE".to_owned(), mode.to_owned())];
    env.extend(extra_env);
    let scratch = root.join("scratch");
    NativeJailConfig {
        cgroup: CgroupLimits {
            unit_name: format!("metron-test-{mode}-{}", std::process::id()),
            scratch_reserve_bytes: 0,
            ..CgroupLimits::default()
        },
        primary: ExecSource::Path {
            path: exe,
            sha256: Some(sha),
            label: format!("probe-{mode}"),
        },
        inherited: Vec::new(),
        args: Vec::new(),
        workspace: None,
        writable_workspace: false,
        scratch: scratch.clone(),
        ro_inputs: vec![root.join("ro")],
        workdir: scratch,
        env,
        trampoline_bin: Some(PathBuf::from(env!("CARGO_BIN_EXE_metron-jail"))),
        namespace_policy: if userns_supported() {
            NamespacePolicy::Require
        } else {
            NamespacePolicy::AllowDegraded
        },
        allow_unix_sockets: false,
    }
}

fn invoke(
    cfg: &NativeJailConfig,
    stdin_memfd: Option<&metron_sandbox::SealedMemfd>,
) -> metron_sandbox::NativeJailOutput {
    if systemd_user_available() {
        run_native_jail(cfg, stdin_memfd).expect("run_native_jail")
    } else {
        run_direct_jail(cfg, stdin_memfd).expect("run_direct_jail")
    }
}

fn lane_b_blocks_home_and_shadow_eacces() {
    let root = make_disk_dir("landlock-eacces");
    let real_home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_owned());
    let ro_file = root.join("ro/input.txt");
    fs::write(&ro_file, "ro-ok").unwrap();
    let cfg = base_cfg(
        &root,
        "check_landlock_eacces",
        vec![
            ("PROBE_REAL_HOME".to_owned(), real_home),
            (
                "PROBE_RO_FILE".to_owned(),
                ro_file.to_string_lossy().into_owned(),
            ),
        ],
    );
    let out = invoke(&cfg, None);
    assert!(
        out.success,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"landlock-eacces-ok\n");
    assert_eq!(
        fs::read_to_string(root.join("scratch/out.txt")).unwrap(),
        "scratch-ok"
    );
    fs::remove_dir_all(&root).unwrap();
}

fn lane_b_blocks_network_and_io_uring_eperm() {
    let root = make_disk_dir("seccomp-eperm");
    let cfg = base_cfg(&root, "check_seccomp_eperm", Vec::new());
    let out = invoke(&cfg, None);
    assert!(
        out.success,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"seccomp-eperm-ok\n");
    fs::remove_dir_all(&root).unwrap();
}

fn lane_b_direct_memfd_runs_after_source_delete() {
    let root = make_disk_dir("memfd-stdin");
    let exe_bytes = fs::read(std::env::current_exe().unwrap()).unwrap();
    let sealed = seal_bytes("probe-memfd", &exe_bytes, None, true).unwrap();
    let mut cfg = base_cfg(&root, "check_memfd_echo", Vec::new());
    cfg.primary = ExecSource::StdinSealed {
        sha256: sealed.sha256().to_owned(),
        label: "probe-memfd".to_owned(),
    };
    let out = invoke(&cfg, Some(&sealed));
    assert!(
        out.success,
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.stdout, b"memfd-direct-ok\n");
    fs::remove_dir_all(&root).unwrap();
}

fn lane_b_systemd_timeout_and_oom_cleanup() {
    if !systemd_user_available() {
        eprintln!("(skipped cgroup timeout/oom: user systemd unavailable)");
        return;
    }
    let root = make_disk_dir("systemd-cg");

    // 1. Timeout + spawned child must be killed cleanly by cgroup KillMode=control-group.
    let mut timeout_cfg = base_cfg(&root, "check_timeout_spawn_child", Vec::new());
    timeout_cfg.cgroup.unit_name = format!("metron-test-timeout-{}", std::process::id());
    timeout_cfg.cgroup.timeout_ms = 500;
    let out = run_native_jail(&timeout_cfg, None).expect("timeout run");
    assert!(out.timed_out, "expected timeout");
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(
        !root.join("scratch/escaped").exists(),
        "descendant escaped cgroup kill"
    );

    // 2. OOM kill under MemoryMax with MemorySwapMax=0 must terminate cleanly and leave cgroup gone.
    let mut oom_cfg = base_cfg(&root, "check_oom", Vec::new());
    oom_cfg.cgroup.unit_name = format!("metron-test-oom-{}", std::process::id());
    oom_cfg.cgroup.memory_max_bytes = 24 * 1024 * 1024;
    oom_cfg.cgroup.timeout_ms = 8000;
    let oom_out = run_native_jail(&oom_cfg, None).expect("oom run");
    assert!(!oom_out.success, "OOM probe must fail");

    fs::remove_dir_all(&root).unwrap();
}
