//! Transient-service and namespace confinement for untrusted tool processes.
use metron_inference_local::artifact::sha256_file;
use metron_protocol::toolkit::ToolPin;
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub struct Image {
    _file: File,
    pub path: PathBuf,
}
impl Image {
    // Bubblewrap cannot open another process's proc-fd after entering its user
    // namespace. The staging directory is private and never exposed in the
    // sandbox; its owner is part of the trusted-host-writer boundary.
    pub fn stage(&self, directory: &Path, name: &str) -> Result<PathBuf, &'static str> {
        let path = directory.join(name);
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o500)
            .open(&path)
            .map_err(|_| "executable staging unavailable")?;
        let mut source = File::open(&self.path).map_err(|_| "sealed image unavailable")?;
        std::io::copy(&mut source, &mut target).map_err(|_| "executable staging failed")?;
        target.flush().map_err(|_| "executable staging failed")?;
        Ok(path)
    }
}
pub fn pin(pin: &ToolPin) -> Result<Image, &'static str> {
    let source = File::from(
        rustix::fs::open(
            pin.executable.as_str(),
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| "pinned executable unavailable")?,
    );
    let image = seal(source)?;
    if sha256_file(&image.path).map_err(|_| "executable hash failed")? != pin.sha256.as_str() {
        return Err("executable digest mismatch");
    }
    Ok(image)
}

pub fn seal(source: File) -> Result<Image, &'static str> {
    let metadata = source
        .metadata()
        .map_err(|_| "pinned executable metadata unavailable")?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 * 1024 {
        return Err("invalid executable");
    }
    let mut image = File::from(
        memfd_create(
            "metron-coding-tool",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::EXEC,
        )
        .map_err(|_| "sealed executable unavailable")?,
    );
    if std::io::copy(&mut source.take(512 * 1024 * 1024 + 1), &mut image)
        .map_err(|_| "executable copy failed")?
        > 512 * 1024 * 1024
    {
        return Err("executable exceeds limit");
    }
    fcntl_add_seals(
        &image,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .map_err(|_| "executable seal failed")?;
    let path = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        image.as_raw_fd()
    ));
    Ok(Image { _file: image, path })
}

/// Reject RAM-backed scratch; the caller selects a trusted disk-backed path.
pub fn require_disk(path: &Path) -> Result<(), &'static str> {
    let fs = rustix::fs::statfs(path).map_err(|_| "scratch filesystem unavailable")?;
    let kind = fs.f_type as u64;
    if kind == 0x0102_1994 || kind == 0x8584_58f6 {
        return Err("RAM-backed scratch denied");
    }
    Ok(())
}

pub struct Config<'a> {
    pub unit: &'a str,
    pub bwrap: &'a Image,
    pub workspace: &'a Path,
    pub scratch: &'a Path,
    pub writable: bool,
    pub mounts: Vec<(PathBuf, PathBuf)>,
    pub image_stdin: Option<(&'a Image, PathBuf)>,
    pub program: &'a str,
    pub args: Vec<String>,
    pub workdir: String,
    pub timeout_ms: u32,
    pub memory: u64,
    pub processes: u16,
    pub output_limit: usize,
}
pub struct Completed {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub success: bool,
}
pub enum Failure {
    Denied(&'static str),
    Uncertain(&'static str),
}

fn manager_command(program: &str, runtime: &str) -> Command {
    let mut c = Command::new(program);
    c.env_clear()
        .env("XDG_RUNTIME_DIR", runtime)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={runtime}/bus"),
        )
        .env("PATH", "/usr/bin")
        .env("LANG", "C.UTF-8");
    c
}
fn systemctl(runtime: &str, args: &[&str]) -> Option<String> {
    let mut c = manager_command("/usr/bin/timeout", runtime);
    let out = c
        .args(["5s", "/usr/bin/systemctl", "--user"])
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}
fn capture(
    mut input: impl Read + Send + 'static,
    limit: usize,
) -> thread::JoinHandle<std::io::Result<(Vec<u8>, bool)>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let take = (limit - output.len()).min(n);
            output.extend_from_slice(&buf[..take]);
            truncated |= take < n;
        }
        Ok((output, truncated))
    })
}

pub fn run(cfg: Config<'_>) -> Result<Completed, Failure> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map_err(|_| Failure::Denied("user runtime unavailable"))?;
    if !runtime.starts_with('/') || runtime.contains('\n') {
        return Err(Failure::Denied("invalid user runtime"));
    }
    let manager = systemctl(&runtime, &["show", "-p", "ControlGroup", "--value"])
        .ok_or(Failure::Denied("user cgroup manager unavailable"))?;
    let manager = manager.trim();
    if !manager.starts_with('/') || manager.contains("..") || manager.contains('\n') {
        return Err(Failure::Denied("invalid manager cgroup"));
    }
    let manager_events = PathBuf::from("/sys/fs/cgroup")
        .join(manager.trim_start_matches('/'))
        .join("cgroup.events");
    let events = std::fs::read_to_string(manager_events)
        .map_err(|_| Failure::Denied("cgroup v2 state unavailable"))?;
    if !events.lines().any(|line| line.starts_with("populated ")) {
        return Err(Failure::Denied("invalid cgroup v2 state"));
    }
    let cgroup = PathBuf::from("/sys/fs/cgroup")
        .join(manager.trim_start_matches('/'))
        .join("app.slice")
        .join(format!("{}.service", cfg.unit));
    if cgroup.exists()
        || systemctl(&runtime, &["show", cfg.unit, "-p", "LoadState", "--value"])
            .is_some_and(|v| v.trim() != "not-found")
    {
        return Err(Failure::Denied("dispatch unit already exists"));
    }
    let secs = u64::from(cfg.timeout_ms).div_ceil(1000);
    let mut c = manager_command("/usr/bin/systemd-run", &runtime);
    c.args(["--user", "--quiet", "--wait", "--pipe", "--slice=app.slice"])
        .arg(format!("--unit={}", cfg.unit))
        .args([
            "--property=Type=exec",
            "--property=ExitType=cgroup",
            "--property=KillMode=control-group",
            "--property=TimeoutStopSec=2s",
            "--property=CPUQuota=200%",
            "--property=MemorySwapMax=0",
            "--property=LimitFSIZE=67108864",
        ])
        .arg(format!("--property=MemoryMax={}", cfg.memory))
        .arg(format!("--property=TasksMax={}", cfg.processes))
        .arg(format!("--property=RuntimeMaxSec={}s", secs.max(1)))
        .arg(&cfg.bwrap.path)
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--unshare-ipc",
            "--unshare-uts",
            "--new-session",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--ro-bind",
            "/usr",
            "/usr",
            "--symlink",
            "usr/lib",
            "/lib",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--dir",
            "/workspace",
        ])
        .arg(if cfg.writable { "--bind" } else { "--ro-bind" })
        .arg(cfg.workspace)
        .arg("/workspace")
        .args(["--bind"])
        .arg(cfg.scratch)
        .arg("/scratch");
    for (source, target) in &cfg.mounts {
        c.arg("--ro-bind").arg(source).arg(target);
    }
    if let Some((_, target)) = &cfg.image_stdin {
        c.args(["--perms", "0555", "--ro-bind-data", "0"])
            .arg(target);
    }
    c.args([
        "--setenv",
        "PATH",
        "/toolchain/bin:/usr/bin",
        "--setenv",
        "LANG",
        "C.UTF-8",
        "--setenv",
        "HOME",
        "/scratch",
        "--setenv",
        "TMPDIR",
        "/scratch",
        "--setenv",
        "CARGO_HOME",
        "/scratch/cargo-home",
        "--setenv",
        "CARGO_TARGET_DIR",
        "/scratch/target",
        "--setenv",
        "CARGO_NET_OFFLINE",
        "true",
        "--setenv",
        "RUSTUP_TOOLCHAIN",
        "1.100.0",
        "--chdir",
    ])
    .arg(&cfg.workdir)
    .arg("--")
    .arg(cfg.program)
    .args(&cfg.args)
    .stdin(match &cfg.image_stdin {
        Some((image, _)) => Stdio::from(
            File::open(&image.path)
                .map_err(|_| Failure::Denied("sealed image input unavailable"))?,
        ),
        None => Stdio::null(),
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = c
        .spawn()
        .map_err(|_| Failure::Denied("transient service launch unavailable"))?;
    let stdout = capture(
        child
            .stdout
            .take()
            .ok_or(Failure::Uncertain("stdout unavailable"))?,
        cfg.output_limit,
    );
    let stderr = capture(
        child
            .stderr
            .take()
            .ok_or(Failure::Uncertain("stderr unavailable"))?,
        cfg.output_limit,
    );
    let start = Instant::now();
    let mut timed_out = false;
    let mut launcher_status = None;
    loop {
        // Sampled reserve protection complements per-file/memory/process caps.
        // It is not a filesystem quota or a claim of a strict disk-byte cap.
        let disk_ok = rustix::fs::statvfs(cfg.scratch)
            .ok()
            .is_some_and(|s| s.f_bavail.saturating_mul(s.f_frsize) > 2 * 1024 * 1024 * 1024);
        if !disk_ok {
            timed_out = true;
            let _ = systemctl(
                &runtime,
                &["kill", "--kill-whom=all", "--signal=KILL", cfg.unit],
            );
            let _ = systemctl(&runtime, &["stop", cfg.unit]);
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                launcher_status = status.code();
                break;
            }
            Ok(None)
                if start.elapsed() < Duration::from_millis(u64::from(cfg.timeout_ms) + 3000) =>
            {
                thread::sleep(Duration::from_millis(10))
            }
            _ => {
                timed_out = true;
                let _ = systemctl(
                    &runtime,
                    &["kill", "--kill-whom=all", "--signal=KILL", cfg.unit],
                );
                let _ = systemctl(&runtime, &["stop", cfg.unit]);
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
    let info = systemctl(
        &runtime,
        &[
            "show",
            cfg.unit,
            "-p",
            "ActiveState",
            "-p",
            "SubState",
            "-p",
            "Result",
            "-p",
            "ExecMainStatus",
            "-p",
            "ExecMainCode",
        ],
    )
    .ok_or(Failure::Uncertain("service state unavailable"))?;
    let closed = match std::fs::read_to_string(cgroup.join("cgroup.events")) {
        Ok(events) => events.lines().any(|line| line == "populated 0"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    if !info
        .lines()
        .any(|s| s == "ActiveState=inactive" || s == "ActiveState=failed")
        || !closed
    {
        return Err(Failure::Uncertain("service cgroup cleanup not established"));
    }
    let finish = Instant::now();
    while !(stdout.is_finished() && stderr.is_finished()) {
        if finish.elapsed() > Duration::from_millis(500) {
            return Err(Failure::Uncertain("output pipes remain open"));
        }
        thread::sleep(Duration::from_millis(5));
    }
    let (out, a) = stdout
        .join()
        .map_err(|_| Failure::Uncertain("output capture failed"))?
        .map_err(|_| Failure::Uncertain("output capture failed"))?;
    let (err, b) = stderr
        .join()
        .map_err(|_| Failure::Uncertain("output capture failed"))?
        .map_err(|_| Failure::Uncertain("output capture failed"))?;
    let exit = info
        .lines()
        .any(|line| line == "ExecMainCode=1")
        .then(|| {
            info.lines().find_map(|line| {
                line.strip_prefix("ExecMainStatus=")
                    .and_then(|s| s.parse::<i32>().ok())
            })
        })
        .flatten()
        .or_else(|| {
            (info.lines().any(|s| s == "Result=success") && launcher_status == Some(0)).then_some(0)
        });
    let timed_out = timed_out || info.lines().any(|s| s == "Result=timeout");
    Ok(Completed {
        stdout: out,
        stderr: err,
        truncated: a || b,
        exit_code: exit,
        timed_out,
        success: info.lines().any(|s| s == "Result=success") && exit == Some(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{
        parse_envelope,
        toolkit::{ToolCatalog, ToolId},
    };
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    #[ignore = "requires pinned local tools, user systemd, bubblewrap and disk scratch"]
    fn confined_service_probes() {
        let catalog_file =
            std::env::var("METRON_CODING_PROBE_CATALOG").expect("pinned probe catalog");
        let catalog: ToolCatalog = parse_envelope(&fs::read(&catalog_file).unwrap()).unwrap();
        let get = |id| catalog.tools.iter().find(|p| p.tool == id).unwrap();
        let bwrap = pin(get(ToolId::Bubblewrap)).unwrap();
        let z3 = pin(get(ToolId::Z3)).unwrap();
        let base = Path::new(&catalog_file)
            .parent()
            .expect("disk-backed catalog directory")
            .join(format!("metron-process-probe-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let workspace = base.join("workspace");
        let scratch = base.join("scratch");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&scratch).unwrap();

        fs::write(scratch.join("p.smt2"), "(assert false)\n(check-sat)\n").unwrap();
        let unit = format!("metron-probe-{}-smt", std::process::id());
        let result = run(Config {
            unit: &unit,
            bwrap: &bwrap,
            workspace: &workspace,
            scratch: &scratch,
            writable: false,
            mounts: vec![],
            image_stdin: Some((&z3, PathBuf::from("/tool"))),
            program: "/tool",
            args: vec!["/scratch/p.smt2".into()],
            workdir: "/scratch".into(),
            timeout_ms: 3000,
            memory: 256 * 1024 * 1024,
            processes: 32,
            output_limit: 4096,
        });
        let result = match result {
            Ok(v) => v,
            Err(Failure::Denied(e)) => panic!("denied: {e}"),
            Err(Failure::Uncertain(e)) => panic!("uncertain: {e}"),
        };
        assert_eq!(
            result.exit_code,
            Some(0),
            "stdout: {} stderr: {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"unsat\n");
        let unit = format!("metron-probe-{}-net", std::process::id());
        let result=run(Config { unit:&unit,bwrap:&bwrap,workspace:&workspace,scratch:&scratch,writable:false,mounts:vec![],image_stdin:None,program:"/usr/bin/python3",args:vec!["-c".into(),"import os,socket; s=socket.socket(); s.settimeout(1); print(os.path.exists('/home'),s.connect_ex(('1.1.1.1',443)))".into()],workdir:"/scratch".into(),timeout_ms:3000,memory:256*1024*1024,processes:32,output_limit:4096 });
        let result = match result {
            Ok(v) => v,
            Err(Failure::Denied(e)) => panic!("denied: {e}"),
            Err(Failure::Uncertain(e)) => panic!("uncertain: {e}"),
        };
        assert_eq!(
            result.exit_code,
            Some(0),
            "stdout: {} stderr: {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(result.stdout, b"False 101\n");
        let mut wrong = get(ToolId::Z3).clone();
        wrong.sha256 = metron_protocol::content_digest(b"wrong executable");
        assert!(pin(&wrong).is_err());
        fs::write(scratch.join("sat.smt2"), "(check-sat)\n").unwrap();

        let unit = format!("metron-probe-{}-sat", std::process::id());
        let sat = run(Config {
            unit: &unit,
            bwrap: &bwrap,
            workspace: &workspace,
            scratch: &scratch,
            writable: false,
            mounts: vec![],
            image_stdin: Some((&z3, PathBuf::from("/tool"))),
            program: "/tool",
            args: vec!["/scratch/sat.smt2".into()],
            workdir: "/scratch".into(),
            timeout_ms: 3000,
            memory: 256 * 1024 * 1024,
            processes: 32,
            output_limit: 4096,
        });
        let sat = match sat {
            Ok(v) => v,
            Err(Failure::Denied(e)) => panic!("denied: {e}"),
            Err(Failure::Uncertain(e)) => panic!("uncertain: {e}"),
        };
        assert!(sat.success);
        assert_eq!(sat.stdout, b"sat\n");
        let lean_pin = get(ToolId::Lean);
        let lean = pin(lean_pin).unwrap();
        let lean_root = Path::new(lean_pin.executable.as_str())
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        for (suffix, source, valid) in [
            ("lean-valid", "theorem t : True := by trivial\n", true),
            ("lean-invalid", "theorem t : False := by trivial\n", false),
        ] {
            fs::write(scratch.join("proof.lean"), source).unwrap();
            let unit = format!("metron-probe-{}-{}", std::process::id(), suffix);
            let result = run(Config {
                unit: &unit,
                bwrap: &bwrap,
                workspace: &workspace,
                scratch: &scratch,
                writable: false,
                mounts: vec![(lean_root.to_owned(), PathBuf::from("/lean"))],
                image_stdin: Some((&lean, PathBuf::from("/lean/bin/lean"))),
                program: "/lean/bin/lean",
                args: vec!["/scratch/proof.lean".into()],
                workdir: "/scratch".into(),
                timeout_ms: 3000,
                memory: 512 * 1024 * 1024,
                processes: 32,
                output_limit: 4096,
            });
            let result = match result {
                Ok(v) => v,
                Err(Failure::Denied(e)) => panic!("denied: {e}"),
                Err(Failure::Uncertain(e)) => panic!("uncertain: {e}"),
            };
            assert_eq!(
                result.success,
                valid,
                "{}",
                String::from_utf8_lossy(&result.stdout)
            );
        }
        let unit = format!("metron-probe-{}-timeout", std::process::id());
        let timeout = run(Config {
            unit: &unit,
            bwrap: &bwrap,
            workspace: &workspace,
            scratch: &scratch,
            writable: false,
            mounts: vec![],
            image_stdin: None,
            program: "/usr/bin/sleep",
            args: vec!["10".into()],
            workdir: "/scratch".into(),
            timeout_ms: 1000,
            memory: 256 * 1024 * 1024,
            processes: 32,
            output_limit: 4096,
        });
        let timeout = match timeout {
            Ok(v) => v,
            Err(Failure::Denied(e)) => panic!("denied: {e}"),
            Err(Failure::Uncertain(e)) => panic!("uncertain: {e}"),
        };
        assert!(timeout.timed_out, "expected service timeout");
        let unit = format!("metron-probe-{}-child", std::process::id());
        let child=run(Config { unit:&unit,bwrap:&bwrap,workspace:&workspace,scratch:&scratch,writable:false,mounts:vec![],image_stdin:None,program:"/usr/bin/python3",args:vec!["-c".into(),"import os,time; p=os.fork(); (os._exit(0) if p else None); os.setsid(); p=os.fork(); (os._exit(0) if p else None); time.sleep(3); open('/scratch/escaped','w').write('bad')".into()],workdir:"/scratch".into(),timeout_ms:1000,memory:256*1024*1024,processes:32,output_limit:4096 });
        let child = match child {
            Ok(v) => v,
            Err(Failure::Denied(e)) => panic!("denied: {e}"),
            Err(Failure::Uncertain(e)) => panic!("uncertain: {e}"),
        };
        assert!(child.timed_out || child.success);
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert!(
            !scratch.join("escaped").exists(),
            "background descendant survived"
        );
        // Exercise the actual pinned JJ and ripgrep in the same namespace boundary.
        let jj = pin(get(ToolId::Jj)).unwrap();
        let rg = pin(get(ToolId::Ripgrep)).unwrap();
        fs::write(workspace.join(".gitignore"), "ignored\n").unwrap();
        fs::write(workspace.join("ignored"), "first\n").unwrap();
        let invoke = |suffix: &str, image: &Image, args: Vec<&str>, writable| {
            let unit = format!("metron-probe-{}-{}", std::process::id(), suffix);
            match run(Config {
                unit: &unit,
                bwrap: &bwrap,
                workspace: &workspace,
                scratch: &scratch,
                writable,
                mounts: vec![],
                image_stdin: Some((image, PathBuf::from("/tool"))),
                program: "/tool",
                args: args.into_iter().map(str::to_owned).collect(),
                workdir: "/workspace".into(),
                timeout_ms: 5000,
                memory: 512 * 1024 * 1024,
                processes: 32,
                output_limit: 4096,
            }) {
                Ok(result) => result,
                Err(Failure::Denied(e)) | Err(Failure::Uncertain(e)) => panic!("probe failed: {e}"),
            }
        };
        for (suffix, args) in [
            ("jj-init", vec!["git", "init", "--no-colocate"]),
            (
                "jj-track",
                vec!["file", "track", "--include-ignored", "all()"],
            ),
            (
                "jj-commit",
                vec![
                    "--config=user.name=Metron",
                    "--config=user.email=metron@invalid",
                    "commit",
                    "-m",
                    "baseline",
                ],
            ),
        ] {
            let out = invoke(suffix, &jj, args, true);
            assert!(out.success, "{}", String::from_utf8_lossy(&out.stderr));
        }
        assert!(!workspace.join(".git").exists());
        fs::write(workspace.join("ignored"), "second\n").unwrap();
        let diff = invoke(
            "jj-diff",
            &jj,
            vec![
                "--color=never",
                "diff",
                "--git",
                "--from",
                "@-",
                "--to",
                "@",
            ],
            true,
        );
        assert!(diff.success, "{}", String::from_utf8_lossy(&diff.stderr));
        assert!(String::from_utf8_lossy(&diff.stdout).contains("+second"));
        let search = invoke(
            "rg",
            &rg,
            vec![
                "--fixed-strings",
                "--line-number",
                "--with-filename",
                "--no-follow",
                "--hidden",
                "--no-ignore",
                "--color=never",
                "--",
                "second",
                "ignored",
            ],
            false,
        );
        assert!(
            search.success,
            "{}",
            String::from_utf8_lossy(&search.stderr)
        );
        assert_eq!(search.stdout, b"ignored:1:second\n");
        let fifo = base.join("fifo");
        assert!(
            std::process::Command::new("/usr/bin/mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let mut invalid = get(ToolId::Z3).clone();
        invalid.executable = metron_protocol::BoundedText::new(fifo.to_str().unwrap()).unwrap();
        assert!(pin(&invalid).is_err());
        fs::remove_dir_all(base).unwrap();
    }
}
