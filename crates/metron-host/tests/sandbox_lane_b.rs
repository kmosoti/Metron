//! End-to-end integration tests for `metron-runtime __metron-jail` Lane B confinement.
//!
//! Zero `unsafe_code`: `harness = false` lets this compiled binary serve directly as
//! the sealed `memfd` probe child inside `metron-runtime __metron-jail`.

use metron_protocol::toolkit::ToolId;
use metron_sandbox::{
    CgroupLimits, ExecSource, NamespacePolicy, NativeJailConfig, build_lane_b_spec, systemd_cg,
    systemd_user_available, userns_supported, verify_fd_hygiene,
};
use std::{
    fs,
    io::ErrorKind,
    net::{TcpListener, TcpStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Duration,
};

fn main() -> ExitCode {
    if let Ok(mode) = std::env::var("METRON_HOST_JAIL_PROBE") {
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
    if args.iter().any(|a| a == "--list") {
        for name in [
            "no_bubblewrap_in_protocol_toolkit",
            "metron_runtime_jail_blocks_home_shadow_and_network",
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
            "no_bubblewrap_in_protocol_toolkit",
            no_bubblewrap_in_protocol_toolkit,
        ),
        (
            "metron_runtime_jail_blocks_home_shadow_and_network",
            metron_runtime_jail_blocks_home_shadow_and_network,
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

fn run_probe(mode: &str) -> ExitCode {
    match mode {
        "deny_checks" => {
            let real_home = std::env::var("PROBE_REAL_HOME").expect("PROBE_REAL_HOME");
            let home_err = fs::read_dir(&real_home)
                .expect_err("real $HOME must be inaccessible inside metron-runtime __metron-jail");
            assert_eq!(home_err.kind(), ErrorKind::PermissionDenied);

            let shadow_err = fs::read("/etc/shadow")
                .expect_err("/etc/shadow must be denied inside metron-runtime __metron-jail");
            assert_eq!(shadow_err.kind(), ErrorKind::PermissionDenied);

            let bind_err = TcpListener::bind("127.0.0.1:0")
                .expect_err("AF_INET TCP bind must be blocked by seccomp/Landlock");
            assert_eq!(bind_err.kind(), ErrorKind::PermissionDenied);

            let addr = "1.1.1.1:443".parse().expect("socket addr");
            let conn_err = TcpStream::connect_timeout(&addr, Duration::from_millis(250))
                .expect_err("AF_INET TCP connect must be blocked");
            assert_eq!(conn_err.kind(), ErrorKind::PermissionDenied);

            let scratch_home = PathBuf::from(std::env::var("HOME").expect("jail HOME"));
            fs::write(scratch_home.join("ok.txt"), b"jailed-ok").expect("scratch write succeeds");
            println!("runtime-jail-ok");
            ExitCode::SUCCESS
        }
        other => panic!("unknown probe mode: {other}"),
    }
}

fn no_bubblewrap_in_protocol_toolkit() {
    let variants = [
        ToolId::Jj,
        ToolId::Ripgrep,
        ToolId::Z3,
        ToolId::Lean,
        ToolId::Cargo,
        ToolId::Rustc,
    ];
    for tool in variants {
        let name = serde_json::to_string(&tool).unwrap();
        assert!(
            !name.contains("bwrap"),
            "unexpected bubblewrap tool: {name}"
        );
        assert!(
            !name.contains("bubblewrap"),
            "unexpected bubblewrap tool: {name}"
        );
    }
    assert!(serde_json::from_str::<ToolId>("\"bubblewrap\"").is_err());
    assert!(serde_json::from_str::<ToolId>("\"bwrap\"").is_err());
}

fn metron_runtime_jail_blocks_home_shadow_and_network() {
    let dir = TempDir::new("metron-host-lane-b");
    let scratch = dir.path().join("scratch");
    let ro_dir = dir.path().join("ro");
    let fake_home = dir.path().join("fake-home");
    fs::create_dir_all(&scratch).unwrap();
    fs::create_dir_all(&ro_dir).unwrap();
    fs::create_dir_all(&fake_home).unwrap();
    fs::write(fake_home.join("secret.txt"), b"classified").unwrap();

    let self_bin = std::env::current_exe().unwrap();
    let runtime_bin = PathBuf::from(env!("CARGO_BIN_EXE_metron-runtime"));
    let cfg = NativeJailConfig {
        cgroup: CgroupLimits {
            unit_name: format!("metron-host-lane-b-{}", std::process::id()),
            timeout_ms: 10_000,
            ..CgroupLimits::default()
        },
        primary: ExecSource::Path {
            path: self_bin,
            sha256: None,
            label: "metron-host-lane-b-probe".to_owned(),
        },
        inherited: Vec::new(),
        args: Vec::new(),
        workspace: Some(ro_dir),
        writable_workspace: false,
        scratch: scratch.clone(),
        ro_inputs: Vec::new(),
        workdir: scratch.clone(),
        env: vec![
            (
                "METRON_HOST_JAIL_PROBE".to_owned(),
                "deny_checks".to_owned(),
            ),
            (
                "PROBE_REAL_HOME".to_owned(),
                fake_home.to_string_lossy().into_owned(),
            ),
        ],
        trampoline_bin: Some(runtime_bin.clone()),
        namespace_policy: if userns_supported() {
            NamespacePolicy::Require
        } else {
            NamespacePolicy::AllowDegraded
        },
        allow_unix_sockets: false,
    };

    let spec = build_lane_b_spec(&cfg);
    let spec_path = scratch.join("runtime-jail-spec.json");
    fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();

    let output = Command::new(&runtime_bin)
        .args(["__metron-jail", "--policy-file"])
        .arg(&spec_path)
        .output()
        .expect("spawn metron-runtime __metron-jail");
    assert!(
        output.status.success(),
        "metron-runtime __metron-jail failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("runtime-jail-ok"),
        "expected marker in stdout"
    );
    assert!(scratch.join("ok.txt").is_file());

    if systemd_user_available() && metron_sandbox::require_disk_backed(&scratch).is_ok() {
        let _ = fs::remove_file(scratch.join("ok.txt"));
        fs::write(&spec_path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let cg_out = systemd_cg::run_in_systemd_cgroup(
            &cfg.cgroup,
            &scratch,
            &runtime_bin,
            &[
                "__metron-jail".to_owned(),
                "--policy-file".to_owned(),
                spec_path.to_string_lossy().into_owned(),
            ],
            None,
        )
        .expect("run via systemd-run --user --collect");
        assert!(
            cg_out.success,
            "systemd-run metron-runtime __metron-jail failed: stderr={}",
            String::from_utf8_lossy(&cg_out.stderr)
        );
        assert!(scratch.join("ok.txt").is_file());
    }
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
