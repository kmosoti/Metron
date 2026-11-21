//! Unprivileged Lane B (`systemd-run --user --collect` + `Landlock` + `seccomp-bpf` +
//! sealed `memfd`) confinement for host tool processes. Zero `bwrap`, zero `Image::stage`.

use metron_protocol::toolkit::ToolPin;
use metron_sandbox::{
    CgroupLimits, ExecSource, InheritedMemfd, NamespacePolicy, NativeJailConfig, SandboxError,
    SealedMemfd, open_and_seal_regular, require_disk_backed, run_native_jail,
};
use std::{
    fs::File,
    path::{Path, PathBuf},
};

/// Immutable executable held in memory (`F_SEAL_WRITE` `memfd`).
pub type Image = SealedMemfd;

/// Pins and seals a catalog executable into an in-memory `memfd` after verifying its SHA-256.
pub fn pin(pin: &ToolPin) -> Result<Image, &'static str> {
    open_and_seal_regular(
        Path::new(pin.executable.as_str()),
        "metron-coding-tool",
        Some(pin.sha256.as_str()),
        true,
    )
    .map_err(|err| match err {
        SandboxError::Denied(msg)
        | SandboxError::LandlockInsufficient(msg)
        | SandboxError::Uncertain(msg) => map_pin_err(&msg),
        SandboxError::InsufficientScratchSpace { .. } => "insufficient scratch space",
    })
}

/// Seals a regular file into an in-memory `memfd` (`F_SEAL_WRITE`).
pub fn seal(source: File) -> Result<Image, &'static str> {
    metron_sandbox::memfd::seal_from_reader("metron-coding-tool", source, None, true).map_err(|e| {
        match e {
            SandboxError::Denied(msg)
            | SandboxError::LandlockInsufficient(msg)
            | SandboxError::Uncertain(msg) => map_pin_err(&msg),
            SandboxError::InsufficientScratchSpace { .. } => "insufficient scratch space",
        }
    })
}

fn map_pin_err(msg: &str) -> &'static str {
    if msg.contains("digest mismatch") {
        "executable digest mismatch"
    } else if msg.contains("limit") {
        "executable exceeds limit"
    } else if msg.contains("seal") {
        "executable seal failed"
    } else {
        "pinned executable unavailable"
    }
}

/// Rejects RAM-backed scratch (`tmpfs` / `ramfs`); callers must provide disk-backed scratch.
pub fn require_disk(path: &Path) -> Result<(), &'static str> {
    require_disk_backed(path).map_err(|_| "RAM-backed scratch denied")
}

/// Target executable for a Lane B jailed invocation.
pub enum ExecTarget<'a> {
    /// Catalog-pinned host binary opened & sealed directly inside `metron-jail`.
    Pinned(&'a ToolPin),
    /// Pre-sealed in-memory executable streamed across stdin pipe into a sealed `memfd`.
    PreSealed(&'a Image),
}

/// Secondary pinned tool exposed to the child via an inherited `memfd` env var (e.g., `RUSTC`).
pub struct InheritedTool<'a> {
    pub env_var: &'a str,
    pub pin: &'a ToolPin,
}

/// Lane B confinement configuration.
pub struct Config<'a> {
    pub unit: &'a str,
    pub workspace: &'a Path,
    pub scratch: &'a Path,
    pub writable: bool,
    pub ro_inputs: Vec<PathBuf>,
    pub exec: ExecTarget<'a>,
    pub inherited: Vec<InheritedTool<'a>>,
    pub args: Vec<String>,
    pub workdir: PathBuf,
    pub extra_env: Vec<(String, String)>,
    pub timeout_ms: u32,
    pub memory: u64,
    pub processes: u16,
    pub output_limit: usize,
}

/// Completed output from a jailed process.
pub struct Completed {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub truncated: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub success: bool,
}

/// Process dispatch failure preserving Metron's Denied vs Uncertain distinction.
pub enum Failure {
    Denied(&'static str),
    Uncertain(&'static str),
}

/// Executes `cfg` inside `metron-sandbox` Lane B (`systemd-run --user --collect` +
/// `PR_SET_NO_NEW_PRIVS` + `Landlock` + `seccomp-bpf` + sealed `memfd`).
pub fn run(cfg: Config<'_>) -> Result<Completed, Failure> {
    let (primary, stdin_memfd) = match cfg.exec {
        ExecTarget::Pinned(tool_pin) => (
            ExecSource::Path {
                path: PathBuf::from(tool_pin.executable.as_str()),
                sha256: Some(tool_pin.sha256.as_str().to_owned()),
                label: "metron-pinned-tool".to_owned(),
            },
            None,
        ),
        ExecTarget::PreSealed(image) => (
            ExecSource::StdinSealed {
                sha256: image.sha256().to_owned(),
                label: "metron-sealed-candidate".to_owned(),
            },
            Some(image),
        ),
    };

    let inherited = cfg
        .inherited
        .into_iter()
        .map(|item| InheritedMemfd {
            env_var: item.env_var.to_owned(),
            path: PathBuf::from(item.pin.executable.as_str()),
            sha256: item.pin.sha256.as_str().to_owned(),
            label: format!("metron-inherited-{}", item.env_var.to_lowercase()),
        })
        .collect();

    let jail_cfg = NativeJailConfig {
        cgroup: CgroupLimits {
            unit_name: cfg.unit.to_owned(),
            memory_max_bytes: cfg.memory,
            tasks_max: cfg.processes,
            cpu_quota_percent: 200,
            limit_fsize_bytes: 67_108_864,
            timeout_ms: cfg.timeout_ms,
            output_limit: cfg.output_limit,
            scratch_reserve_bytes: 2 * 1024 * 1024 * 1024,
        },
        primary,
        inherited,
        args: cfg.args,
        workspace: Some(cfg.workspace.to_path_buf()),
        writable_workspace: cfg.writable,
        scratch: cfg.scratch.to_path_buf(),
        ro_inputs: cfg.ro_inputs,
        workdir: cfg.workdir,
        env: cfg.extra_env,
        trampoline_bin: None,
        namespace_policy: NamespacePolicy::Require,
        allow_unix_sockets: false,
    };

    match run_native_jail(&jail_cfg, stdin_memfd) {
        Ok(done) => Ok(Completed {
            stdout: done.stdout,
            stderr: done.stderr,
            truncated: done.truncated,
            exit_code: done.exit_code,
            timed_out: done.timed_out,
            success: done.success,
        }),
        Err(SandboxError::Denied(msg)) => Err(Failure::Denied(leak_or_map(&msg, false))),
        Err(SandboxError::LandlockInsufficient(_)) => {
            Err(Failure::Denied("landlock enforcement insufficient"))
        }
        Err(SandboxError::InsufficientScratchSpace { .. }) => {
            Err(Failure::Denied("insufficient scratch space"))
        }
        Err(SandboxError::Uncertain(msg)) => Err(Failure::Uncertain(leak_or_map(&msg, true))),
    }
}

fn leak_or_map(msg: &str, uncertain: bool) -> &'static str {
    if msg.contains("RAM-backed") {
        "RAM-backed scratch denied"
    } else if msg.contains("user runtime") {
        "user runtime unavailable"
    } else if msg.contains("cgroup") && uncertain {
        "service cgroup cleanup not established"
    } else if msg.contains("cgroup") {
        "user cgroup manager unavailable"
    } else if msg.contains("already exists") {
        "dispatch unit already exists"
    } else if msg.contains("pipes") {
        "output pipes remain open"
    } else if uncertain {
        "sandbox execution outcome uncertain"
    } else {
        "sandbox execution denied"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{
        BoundedText, parse_envelope,
        toolkit::{ToolCatalog, ToolId},
    };
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn pin_rejects_digest_mismatch_and_fifo() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp-host-process")
            .join(format!("pin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let bin = root.join("tool");
        fs::write(&bin, b"#!/bin/sh\necho ok\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();

        let wrong = ToolPin {
            tool: ToolId::Z3,
            executable: BoundedText::new(bin.to_str().unwrap()).unwrap(),
            sha256: metron_protocol::content_digest(b"wrong executable"),
            version_prefix: BoundedText::new("1.0").unwrap(),
        };
        assert!(pin(&wrong).is_err());

        let fifo = root.join("fifo");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .unwrap();
        let fifo_pin = ToolPin {
            executable: BoundedText::new(fifo.to_str().unwrap()).unwrap(),
            ..wrong
        };
        assert!(pin(&fifo_pin).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn require_disk_rejects_tmpfs_when_available() {
        let shm = Path::new("/dev/shm");
        if shm.exists() {
            assert_eq!(require_disk(shm), Err("RAM-backed scratch denied"));
        }
    }

    #[test]
    #[ignore = "requires pinned local tools, user systemd, and disk scratch"]
    fn confined_service_probes() {
        let catalog_file =
            std::env::var("METRON_CODING_PROBE_CATALOG").expect("pinned probe catalog");
        let catalog: ToolCatalog = parse_envelope(&fs::read(&catalog_file).unwrap()).unwrap();
        let get = |id| catalog.tools.iter().find(|p| p.tool == id).unwrap();
        let base = Path::new(&catalog_file)
            .parent()
            .expect("disk-backed catalog directory")
            .join(format!("metron-process-probe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let workspace = base.join("workspace");
        let scratch = base.join("scratch");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&scratch).unwrap();

        let p_smt = scratch.join("p.smt2");
        fs::write(&p_smt, "(assert false)\n(check-sat)\n").unwrap();
        let unit = format!("metron-probe-{}-smt", std::process::id());
        let result = run(Config {
            unit: &unit,
            workspace: &workspace,
            scratch: &scratch,
            writable: false,
            ro_inputs: vec![],
            exec: ExecTarget::Pinned(get(ToolId::Z3)),
            inherited: vec![],
            args: vec![p_smt.to_string_lossy().into_owned()],
            workdir: scratch.clone(),
            extra_env: vec![],
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
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, b"unsat\n");

        let jj_pin = get(ToolId::Jj);
        let rg_pin = get(ToolId::Ripgrep);
        fs::write(workspace.join(".gitignore"), "ignored\n").unwrap();
        fs::write(workspace.join("ignored"), "first\n").unwrap();
        let invoke = |suffix: &str, tool_pin: &ToolPin, args: Vec<&str>, writable| {
            let unit = format!("metron-probe-{}-{}", std::process::id(), suffix);
            match run(Config {
                unit: &unit,
                workspace: &workspace,
                scratch: &scratch,
                writable,
                ro_inputs: vec![],
                exec: ExecTarget::Pinned(tool_pin),
                inherited: vec![],
                args: args.into_iter().map(str::to_owned).collect(),
                workdir: workspace.clone(),
                extra_env: vec![],
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
            let out = invoke(suffix, jj_pin, args, true);
            assert!(out.success, "{}", String::from_utf8_lossy(&out.stderr));
        }
        assert!(!workspace.join(".git").exists());
        fs::write(workspace.join("ignored"), "second\n").unwrap();
        let diff = invoke(
            "jj-diff",
            jj_pin,
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
            rg_pin,
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
        fs::remove_dir_all(base).unwrap();
    }
}
