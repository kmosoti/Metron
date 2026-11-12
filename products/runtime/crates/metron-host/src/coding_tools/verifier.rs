//! Independent acceptance runner over a previously frozen source tree.
use super::{process, snapshot};
use crate::coding_acceptance::{CargoStage, EvaluatorRunner, OUTPUT_LIMIT, RunOutput, RunnerError};
use metron_protocol::{
    Digest,
    coding_runtime::CodingArtifact,
    toolkit::{ToolCatalog, ToolId},
};
use rustix::fs::{FlockOperation, Mode, OFlags, ResolveFlags, flock, open, openat2};
use std::{
    fs::{self, File},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

pub struct VerifierRunner {
    source: PathBuf,
    _source_lock: File,
    scratch: PathBuf,
    base: PathBuf,
    candidate_image: Option<(String, process::Image)>,
    built: bool,
    bwrap: process::Image,
    cargo: process::Image,
    rustc_stage: PathBuf,
    toolchain: PathBuf,
    unit_prefix: String,
    next: u16,
    uncertain: bool,
}
impl VerifierRunner {
    /// The caller must bind the catalog to the task and provide an exclusive,
    /// trusted run directory on disk. No model process may still be alive.
    pub fn new(
        catalog: &ToolCatalog,
        artifact: &CodingArtifact,
        run_dir: &Path,
        dispatch: &Digest,
    ) -> Result<Self, RunnerError> {
        artifact.validate().map_err(|_| RunnerError)?;
        if !super::inspect_artifact(run_dir, artifact).map_err(|_| RunnerError)? {
            return Err(RunnerError);
        }
        process::require_disk(run_dir).map_err(|_| RunnerError)?;
        let source = run_dir.join(artifact.relative_root.as_str());
        let root = File::from(
            open(
                &source,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| RunnerError)?,
        );
        flock(&root, FlockOperation::NonBlockingLockExclusive).map_err(|_| RunnerError)?;
        let base = run_dir.join(format!("verifier-{}", dispatch.as_str()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&base)
            .map_err(|_| RunnerError)?;
        let verify = base.join("verify");
        fs::create_dir(&verify).map_err(|_| RunnerError)?;
        let files = snapshot(&root, &verify, 1024 * 1024).map_err(|_| RunnerError)?;
        if files != artifact.files {
            return Err(RunnerError);
        }
        fs::remove_dir_all(&verify).map_err(|_| RunnerError)?;
        let scratch = base.join("scratch");
        let images = base.join("images");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&scratch)
            .map_err(|_| RunnerError)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&images)
            .map_err(|_| RunnerError)?;
        let get = |id| {
            catalog
                .tools
                .iter()
                .find(|p| p.tool == id)
                .ok_or(RunnerError)
        };
        let bwrap = process::pin(get(ToolId::Bubblewrap)?).map_err(|_| RunnerError)?;
        let cargo_pin = get(ToolId::Cargo)?;
        let rustc_pin = get(ToolId::Rustc)?;
        let cargo = process::pin(cargo_pin).map_err(|_| RunnerError)?;
        let rustc = process::pin(rustc_pin).map_err(|_| RunnerError)?;
        let toolchain = Path::new(rustc_pin.executable.as_str())
            .parent()
            .and_then(Path::parent)
            .ok_or(RunnerError)?
            .to_owned();
        if Path::new(cargo_pin.executable.as_str()).parent()
            != Path::new(rustc_pin.executable.as_str()).parent()
        {
            return Err(RunnerError);
        }
        let rustc_stage = rustc.stage(&images, "rustc").map_err(|_| RunnerError)?;
        Ok(Self {
            source,
            _source_lock: root,
            scratch,
            base,
            candidate_image: None,
            built: false,
            bwrap,
            cargo,
            rustc_stage,
            toolchain,
            unit_prefix: format!("metron-verify-{}", dispatch.as_str()),
            next: 0,
            uncertain: false,
        })
    }
    fn run(
        &mut self,
        image: &process::Image,
        scratch: &Path,
        target: &str,
        program: &str,
        args: Vec<String>,
        mounts: Vec<(PathBuf, PathBuf)>,
    ) -> Result<RunOutput, RunnerError> {
        if self.uncertain {
            return Err(RunnerError);
        }
        let unit = format!("{}-{}", self.unit_prefix, self.next);
        self.next = self.next.checked_add(1).ok_or(RunnerError)?;
        let result = process::run(process::Config {
            unit: &unit,
            bwrap: &self.bwrap,
            workspace: &self.source,
            scratch,
            writable: false,
            mounts,
            image_stdin: Some((image, PathBuf::from(target))),
            program,
            args,
            workdir: "/workspace".into(),
            timeout_ms: 120_000,
            memory: 2 * 1024 * 1024 * 1024,
            processes: 64,
            output_limit: OUTPUT_LIMIT,
        });
        let done = match result {
            Ok(done) => done,
            Err(_) => {
                self.uncertain = true;
                return Err(RunnerError);
            }
        };
        if done.timed_out || done.truncated {
            return Err(RunnerError);
        }
        Ok(RunOutput {
            exit_code: if !done.success && done.exit_code == Some(0) {
                None
            } else {
                done.exit_code
            },
            stdout: done.stdout,
            stderr: done.stderr,
            process_tree_closed: true,
        })
    }
}
impl EvaluatorRunner for VerifierRunner {
    fn cargo(&mut self, stage: CargoStage) -> Result<RunOutput, RunnerError> {
        if self.candidate_image.is_some() {
            return Err(RunnerError);
        }
        if matches!(stage, CargoStage::Build) {
            self.built = false;
        }
        // Temporarily move the pinned image so the immutable image and mutable
        // runner can be borrowed together without duplicating its descriptor.
        let image = process::seal(File::open(&self.cargo.path).map_err(|_| RunnerError)?)
            .map_err(|_| RunnerError)?;
        let args = match stage {
            CargoStage::Metadata => vec![
                "--offline",
                "--locked",
                "metadata",
                "--format-version=1",
                "--no-deps",
            ],
            CargoStage::Test => vec!["--offline", "--locked", "test"],
            CargoStage::Build => vec!["--offline", "--locked", "build"],
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mounts = vec![
            (self.toolchain.clone(), PathBuf::from("/toolchain")),
            (
                self.rustc_stage.clone(),
                PathBuf::from("/toolchain/bin/rustc"),
            ),
        ];
        let scratch = self.scratch.clone();
        let result = self.run(
            &image,
            &scratch,
            "/toolchain/bin/cargo",
            "/toolchain/bin/cargo",
            args,
            mounts,
        );
        if matches!(stage, CargoStage::Build) {
            self.built = result.as_ref().is_ok_and(|out| out.exit_code == Some(0));
        }
        result
    }
    fn candidate(&mut self, bin_name: &str, args: &[&str]) -> Result<RunOutput, RunnerError> {
        if bin_name.is_empty()
            || !bin_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(RunnerError);
        }
        if !self.built {
            return Err(RunnerError);
        }
        if self.candidate_image.is_none() {
            let root = File::from(
                open(
                    &self.scratch,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|_| RunnerError)?,
            );
            let path = format!("target/debug/{bin_name}");
            let file = File::from(
                openat2(
                    &root,
                    path.as_str(),
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
                )
                .map_err(|_| RunnerError)?,
            );
            let image = process::seal(file).map_err(|_| RunnerError)?;
            self.candidate_image = Some((bin_name.to_owned(), image));
        }
        let (name, image) = self.candidate_image.take().ok_or(RunnerError)?;
        if name != bin_name {
            self.candidate_image = Some((name, image));
            return Err(RunnerError);
        }
        let scratch = self.base.join(format!("case-{}", self.next));
        if fs::DirBuilder::new().mode(0o700).create(&scratch).is_err() {
            self.candidate_image = Some((name, image));
            return Err(RunnerError);
        }
        let result = self.run(
            &image,
            &scratch,
            "/candidate",
            "/candidate",
            args.iter().map(|a| (*a).to_owned()).collect(),
            vec![],
        );
        self.candidate_image = Some((name, image));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::{
        MAX_INTERNAL, canonical_bytes, coding::WorkspacePath, content_digest, parse_envelope,
        structural_digest,
    };
    use std::os::unix::fs::PermissionsExt;
    #[test]
    #[ignore = "requires pinned local tools, user systemd, bubblewrap and disk scratch"]
    fn offline_frozen_build_and_candidate() {
        let catalog_file =
            std::env::var("METRON_CODING_PROBE_CATALOG").expect("pinned probe catalog");
        let catalog: ToolCatalog = parse_envelope(&fs::read(&catalog_file).unwrap()).unwrap();
        let base = Path::new(&catalog_file)
            .parent()
            .expect("disk-backed catalog directory")
            .join(format!("metron-verifier-probe-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
        let source = base.join("input");
        let frozen = base.join("frozen");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&frozen).unwrap();
        fs::create_dir(source.join("src")).unwrap();
        fs::write(
            source.join("Cargo.toml"),
            "[package]\nname=\"probe\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        )
        .unwrap();
        fs::write(
            source.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"probe\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            source.join("src/main.rs"),
            "fn main() { let p = std::path::Path::new(\"/scratch/state\"); println!(\"{} {}\", p.exists(), std::path::Path::new(\"/scratch/target\").exists()); std::fs::write(p, b\"persist\").unwrap(); }\n",
        )
        .unwrap();
        let files = snapshot(&File::open(&source).unwrap(), &frozen, 1024 * 1024).unwrap();
        let tree_digest = structural_digest(
            "coding-source-tree/v1",
            &[&canonical_bytes(&files, MAX_INTERNAL).unwrap()],
        )
        .unwrap();
        let artifact = CodingArtifact {
            relative_root: WorkspacePath::new("frozen").unwrap(),
            files,
            tree_digest,
        };
        let mut runner = VerifierRunner::new(
            &catalog,
            &artifact,
            &base,
            &content_digest(b"test verifier"),
        )
        .unwrap();
        for stage in [CargoStage::Metadata, CargoStage::Test, CargoStage::Build] {
            let out = runner.cargo(stage).unwrap();
            assert_eq!(
                out.exit_code,
                Some(0),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let out = runner.candidate("probe", &["hello"]).unwrap();
        assert_eq!(
            out.exit_code,
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.stdout, b"false false\n");
        // Replacing the build output after the first case cannot change the
        // sealed image; each case also starts with an empty private scratch.
        fs::write(
            runner.scratch.join("target/debug/probe"),
            b"not an executable",
        )
        .unwrap();
        let second = runner.candidate("probe", &["second"]).unwrap();
        assert_eq!(second.exit_code, Some(0));
        assert_eq!(second.stdout, b"false false\n");
        assert!(runner.candidate("other", &[]).is_err());
        fs::remove_dir_all(base).unwrap();
    }
}
