//! Host coding-tool boundary. Process execution remains fail-closed until
//! isolation and process-tree cleanup are established independently.
mod process;
mod verifier;
pub use verifier::VerifierRunner;

use metron_protocol::{
    BoundedText, Digest, MAX_INTERNAL, SqlU64, canonical_bytes,
    coding::{CodingAction, CodingPolicy, CodingTask, RustProgram, WorkspacePath},
    coding_runtime::{CodingArtifact, CodingFile, CodingToolOutput, ToolStatus},
    content_digest, structural_digest,
    toolkit::{ToolCatalog, ToolId},
};
use rustix::fs::{
    AtFlags, Dir, FileType, FlockOperation, Mode, OFlags, RenameFlags, ResolveFlags, flock,
    mkdirat, open, openat, openat2, renameat, renameat_with, unlinkat,
};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub enum ToolOutcome {
    Known(CodingToolOutput),
    Uncertain(String),
}

pub struct CodingTools {
    policy: CodingPolicy,
    root: File,
    run_dir: PathBuf,
    workspace: PathBuf,
    catalog: ToolCatalog,
    activity_marker: PathBuf,
    uncertain: bool,
}

impl CodingTools {
    pub fn new(
        task: &CodingTask,
        policy: &CodingPolicy,
        catalog: &ToolCatalog,
        workspace: &Path,
        run_dir: &Path,
    ) -> Result<Self, String> {
        policy
            .validate(task)
            .map_err(|_| "invalid task/policy binding")?;
        if catalog.digest().map_err(|_| "invalid tool catalog")? != task.data().tool_catalog_digest
        {
            return Err("tool catalog digest mismatch".into());
        }
        if workspace != Path::new(policy.workspace_root.as_str()) {
            return Err("workspace policy mismatch".into());
        }
        let root = File::from(
            open(
                workspace,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| "workspace unavailable")?,
        );
        // Every dispatcher must hold this advisory lock and own a private
        // workspace. Other same-user host processes are trusted not to mutate it.
        if root
            .metadata()
            .map_err(|_| "workspace metadata unavailable")?
            .mode()
            & 0o077
            != 0
        {
            return Err("workspace must be private to its owner".into());
        }
        flock(&root, FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| "workspace already in use")?;
        let canonical = fs::canonicalize(workspace).map_err(|_| "workspace unavailable")?;
        let activity_marker = canonical
            .parent()
            .ok_or("workspace has no private parent")?
            .join(format!(
                ".metron-active-{}",
                content_digest(canonical.as_os_str().as_encoded_bytes()).as_str()
            ));
        check_activity_marker(&activity_marker)?;
        Ok(Self {
            policy: policy.clone(),
            root,
            run_dir: run_dir.to_owned(),
            workspace: workspace.to_owned(),
            catalog: catalog.clone(),
            activity_marker,
            uncertain: false,
        })
    }

    /// Call only after the harness consumes the admission corresponding to dispatch.
    pub fn execute(&mut self, action: &CodingAction, dispatch: &Digest) -> ToolOutcome {
        if self.uncertain {
            return ToolOutcome::Uncertain("prior process cleanup was not established".into());
        }
        if !self.policy.permits(action) {
            return self.output(
                action,
                ToolStatus::Denied,
                "action denied by policy",
                false,
                vec![],
            );
        }
        match action {
            CodingAction::Run { .. } | CodingAction::Smt { .. } | CodingAction::Lean { .. } => {
                self.execute_process(action, dispatch)
            }
            CodingAction::List { directory } => {
                match list_directory(&self.root, directory.as_ref()) {
                    Ok((names, limited)) => {
                        self.output(action, ToolStatus::Succeeded, &names, limited, vec![])
                    }
                    Err(message) => self.output(action, ToolStatus::Denied, message, false, vec![]),
                }
            }
            CodingAction::Search { .. } | CodingAction::JjSnapshot | CodingAction::JjDiff => {
                self.execute_private(action, dispatch)
            }
            CodingAction::Edit {
                path,
                expected_digest,
                content,
            } => {
                match edit_file(
                    &self.root,
                    path,
                    expected_digest.as_ref(),
                    content.as_str().as_bytes(),
                    self.policy.max_file_bytes,
                    dispatch,
                ) {
                    Ok(file) => self.output(
                        action,
                        ToolStatus::Succeeded,
                        "edit applied",
                        false,
                        vec![file],
                    ),
                    Err(message) => self.output(action, ToolStatus::Denied, message, false, vec![]),
                }
            }
            CodingAction::Read {
                path,
                first_line,
                line_count,
            } => match read_file(&self.root, path, self.policy.max_file_bytes) {
                Ok(bytes) => {
                    let Ok(text) = std::str::from_utf8(&bytes) else {
                        return self.output(
                            action,
                            ToolStatus::Failed,
                            "file is not UTF-8",
                            false,
                            vec![],
                        );
                    };
                    let selected = text
                        .lines()
                        .skip((*first_line - 1) as usize)
                        .take(*line_count as usize)
                        .collect::<Vec<_>>()
                        .join("\n");
                    let max = self.policy.max_output_bytes as usize;
                    let end = selected.floor_char_boundary(max.min(selected.len()));
                    let file = CodingFile {
                        path: path.clone(),
                        digest: content_digest(&bytes),
                        bytes: SqlU64::new(bytes.len() as u64).expect("bounded file"),
                    };
                    self.output(
                        action,
                        ToolStatus::Succeeded,
                        &selected[..end],
                        end < selected.len(),
                        vec![file],
                    )
                }
                Err("file not found") => self.output(
                    action,
                    ToolStatus::Failed,
                    &format!("file not found: {}", path.as_str()),
                    false,
                    vec![],
                ),
                Err(message) => self.output(action, ToolStatus::Denied, message, false, vec![]),
            },
            _ => self.output(
                action,
                ToolStatus::Denied,
                "tool isolation or mutation safety is not yet qualified",
                false,
                vec![],
            ),
        }
    }

    fn execute_process(&mut self, action: &CodingAction, dispatch: &Digest) -> ToolOutcome {
        match self.prepare_process(action, dispatch) {
            Ok(done) => {
                let truncated = done.truncated
                    || done.stdout.len() + done.stderr.len()
                        > self.policy.max_output_bytes as usize;
                ToolOutcome::Known(CodingToolOutput {
                    tool: action.tool(),
                    status: if done.timed_out {
                        ToolStatus::TimedOut
                    } else if done.success {
                        ToolStatus::Succeeded
                    } else {
                        ToolStatus::Failed
                    },
                    body: {
                        let mut bytes = done.stdout;
                        bytes.extend_from_slice(&done.stderr);
                        let text = String::from_utf8_lossy(&bytes);
                        let end = text.floor_char_boundary(
                            text.len().min(self.policy.max_output_bytes as usize),
                        );
                        BoundedText::new(&text[..end]).expect("bounded output")
                    },
                    truncated,
                    exit_code: done.exit_code,
                    files: vec![],
                    process_tree_closed: true,
                })
            }
            Err(process::Failure::Denied(message)) => {
                self.output(action, ToolStatus::Denied, message, false, vec![])
            }
            Err(process::Failure::Uncertain(message)) => {
                self.uncertain = true;
                ToolOutcome::Uncertain(message.into())
            }
        }
    }

    fn prepare_process(
        &self,
        action: &CodingAction,
        dispatch: &Digest,
    ) -> Result<process::Completed, process::Failure> {
        use process::Failure::Denied;
        process::require_disk(&self.run_dir).map_err(Denied)?;
        for name in [".git", ".jj"] {
            if fs::symlink_metadata(self.workspace.join(name)).is_ok() {
                return Err(Denied("workspace contains repository metadata"));
            }
        }
        let get = |tool| {
            self.catalog
                .tools
                .iter()
                .find(|p| p.tool == tool)
                .ok_or(Denied("required executable missing from catalog"))
        };
        let bwrap = process::pin(get(ToolId::Bubblewrap)?).map_err(Denied)?;
        let scratch = self.run_dir.join(format!("scratch-{}", dispatch.as_str()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&scratch)
            .map_err(|_| Denied("scratch unavailable"))?;
        let staged = self.run_dir.join(format!("images-{}", dispatch.as_str()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&staged)
            .map_err(|_| Denied("image staging unavailable"))?;
        let mut images = Vec::new();
        let mut mounts = Vec::new();
        let mut args = Vec::new();
        let image_target: Option<(usize, PathBuf)>;
        let (program, writable, workdir) = match action {
            CodingAction::Smt { problem } => {
                fs::write(scratch.join("problem.smt2"), problem.as_str())
                    .map_err(|_| Denied("scratch write failed"))?;
                images.push(process::pin(get(ToolId::Z3)?).map_err(Denied)?);
                image_target = Some((0, PathBuf::from("/tool")));
                args.push("/scratch/problem.smt2".to_owned());
                ("/tool".to_owned(), false, "/scratch".to_owned())
            }
            CodingAction::Lean { statement, proof } => {
                fs::write(
                    scratch.join("proof.lean"),
                    format!(
                        "theorem metron_claim : {} := by\n{}\n",
                        statement.as_str(),
                        proof.as_str()
                    ),
                )
                .map_err(|_| Denied("scratch write failed"))?;
                let pin = get(ToolId::Lean)?;
                let base = Path::new(pin.executable.as_str())
                    .parent()
                    .and_then(Path::parent)
                    .ok_or(Denied("invalid Lean installation"))?;
                mounts.push((base.to_owned(), PathBuf::from("/lean")));
                images.push(process::pin(pin).map_err(Denied)?);
                image_target = Some((0, PathBuf::from("/lean/bin/lean")));
                args.push("/scratch/proof.lean".to_owned());
                ("/lean/bin/lean".to_owned(), false, "/scratch".to_owned())
            }
            CodingAction::Run {
                program,
                args: user_args,
                directory,
            } => {
                // Reject links and special files before exposing the writable source tree
                // to an untrusted build, including build scripts and proc macros.
                source_files(&self.root, None, self.policy.max_file_bytes, false)
                    .map_err(Denied)?;
                let tool = match program {
                    RustProgram::Cargo => ToolId::Cargo,
                    RustProgram::Rustc => ToolId::Rustc,
                    _ => return Err(Denied("program not pinned in catalog")),
                };
                let rustc = get(ToolId::Rustc)?;
                let cargo = get(ToolId::Cargo)?;
                let base = Path::new(rustc.executable.as_str())
                    .parent()
                    .and_then(Path::parent)
                    .ok_or(Denied("invalid Rust installation"))?;
                if Path::new(cargo.executable.as_str()).parent()
                    != Path::new(rustc.executable.as_str()).parent()
                {
                    return Err(Denied("Rust toolchain mismatch"));
                }
                mounts.push((base.to_owned(), PathBuf::from("/toolchain")));
                for (pin, target) in [
                    (rustc, "/toolchain/bin/rustc"),
                    (cargo, "/toolchain/bin/cargo"),
                ] {
                    images.push(process::pin(pin).map_err(Denied)?);
                    mounts.push((
                        images
                            .last()
                            .expect("image")
                            .stage(
                                &staged,
                                if target.ends_with("rustc") {
                                    "rustc"
                                } else {
                                    "cargo"
                                },
                            )
                            .map_err(Denied)?,
                        PathBuf::from(target),
                    ));
                }
                if tool == ToolId::Cargo {
                    mounts.pop();
                    image_target = Some((1, PathBuf::from("/toolchain/bin/cargo")));
                    args.push("--offline".into());
                } else {
                    mounts.remove(mounts.len() - 2);
                    image_target = Some((0, PathBuf::from("/toolchain/bin/rustc")));
                }
                args.extend(user_args.iter().map(|a| a.as_str().to_owned()));
                let dir = if let Some(path) = directory {
                    if path
                        .as_str()
                        .split('/')
                        .any(|p| matches!(p, ".git" | ".jj"))
                    {
                        return Err(Denied("repository metadata denied"));
                    }
                    openat2(
                        &self.root,
                        path.as_str(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                        Mode::empty(),
                        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
                    )
                    .map_err(|_| Denied("working directory unsafe"))?;
                    format!("/workspace/{}", path.as_str())
                } else {
                    "/workspace".to_owned()
                };
                (
                    if tool == ToolId::Cargo {
                        "/toolchain/bin/cargo"
                    } else {
                        "/toolchain/bin/rustc"
                    }
                    .to_owned(),
                    true,
                    dir,
                )
            }
            _ => return Err(Denied("unsupported process action")),
        };
        self.run_guarded(
            dispatch,
            process::Config {
                unit: "",
                bwrap: &bwrap,
                workspace: &self.workspace,
                scratch: &scratch,
                writable,
                mounts,
                image_stdin: image_target.map(|(index, target)| (&images[index], target)),
                program: &program,
                args,
                workdir,
                timeout_ms: self.policy.command_timeout_ms,
                memory: self
                    .policy
                    .command_memory_bytes
                    .get()
                    .min(2 * 1024 * 1024 * 1024),
                processes: self.policy.max_processes.min(64),
                output_limit: self.policy.max_output_bytes as usize,
            },
        )
    }

    fn run_guarded(
        &self,
        dispatch: &Digest,
        config: process::Config<'_>,
    ) -> Result<process::Completed, process::Failure> {
        use process::Failure::Denied;
        let unit = format!("metron-coding-{}", dispatch.as_str());
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.activity_marker)
            .map_err(|_| Denied("workspace process marker unavailable"))?;
        marker
            .write_all(unit.as_bytes())
            .and_then(|()| marker.sync_all())
            .map_err(|_| Denied("workspace process marker sync failed"))?;
        File::open(self.activity_marker.parent().expect("parent"))
            .and_then(|p| p.sync_all())
            .map_err(|_| Denied("workspace process marker sync failed"))?;
        let result = process::run(process::Config {
            unit: &unit,
            ..config
        });
        if !matches!(result, Err(process::Failure::Uncertain(_))) {
            fs::remove_file(&self.activity_marker).map_err(|_| {
                process::Failure::Uncertain("workspace process marker cleanup failed")
            })?;
            File::open(self.activity_marker.parent().expect("parent"))
                .and_then(|p| p.sync_all())
                .map_err(|_| {
                    process::Failure::Uncertain("workspace process marker cleanup sync failed")
                })?;
        }
        result
    }

    fn private_process(
        &self,
        dispatch: &Digest,
        workspace: &Path,
        tool: ToolId,
        args: Vec<String>,
        writable: bool,
    ) -> Result<process::Completed, process::Failure> {
        use process::Failure::Denied;
        process::require_disk(&self.run_dir).map_err(Denied)?;
        let get = |id| {
            self.catalog
                .tools
                .iter()
                .find(|p| p.tool == id)
                .ok_or(Denied("required executable missing from catalog"))
        };
        let bwrap = process::pin(get(ToolId::Bubblewrap)?).map_err(Denied)?;
        let image = process::pin(get(tool)?).map_err(Denied)?;
        let scratch = self
            .run_dir
            .join(format!("private-scratch-{}", dispatch.as_str()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&scratch)
            .map_err(|_| Denied("scratch unavailable"))?;
        self.run_guarded(
            dispatch,
            process::Config {
                unit: "",
                bwrap: &bwrap,
                workspace,
                scratch: &scratch,
                writable,
                mounts: vec![],
                image_stdin: Some((&image, PathBuf::from("/tool"))),
                program: "/tool",
                args,
                workdir: "/workspace".into(),
                timeout_ms: self.policy.command_timeout_ms,
                memory: self
                    .policy
                    .command_memory_bytes
                    .get()
                    .min(2 * 1024 * 1024 * 1024),
                processes: self.policy.max_processes.min(64),
                output_limit: self.policy.max_output_bytes as usize,
            },
        )
    }

    fn execute_private(&mut self, action: &CodingAction, dispatch: &Digest) -> ToolOutcome {
        match self.prepare_private(action, dispatch) {
            Ok((done, digest)) => {
                let ok = done.success
                    || (matches!(action, CodingAction::Search { .. }) && done.exit_code == Some(1));
                let body = if let Some(digest) = digest {
                    if matches!(action, CodingAction::JjDiff) {
                        format!(
                            "source_tree_digest: {}\n{}",
                            digest.as_str(),
                            String::from_utf8_lossy(&done.stdout)
                        )
                    } else {
                        format!("source_tree_digest: {}", digest.as_str())
                    }
                } else if matches!(action, CodingAction::JjSnapshot | CodingAction::JjDiff) {
                    "private repository operation failed".to_owned()
                } else {
                    String::from_utf8_lossy(&done.stdout).into_owned()
                };
                self.output(
                    action,
                    if done.timed_out {
                        ToolStatus::TimedOut
                    } else if ok {
                        ToolStatus::Succeeded
                    } else {
                        ToolStatus::Failed
                    },
                    &body,
                    done.truncated,
                    vec![],
                )
            }
            Err(process::Failure::Denied(message)) => {
                self.output(action, ToolStatus::Denied, message, false, vec![])
            }
            Err(process::Failure::Uncertain(message)) => {
                self.uncertain = true;
                ToolOutcome::Uncertain(message.into())
            }
        }
    }

    fn prepare_private(
        &self,
        action: &CodingAction,
        dispatch: &Digest,
    ) -> Result<(process::Completed, Option<Digest>), process::Failure> {
        use process::Failure::Denied;
        let repo = self.run_dir.join("jj-private");
        let copy = self
            .run_dir
            .join(format!("source-copy-{}", dispatch.as_str()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&copy)
            .map_err(|_| Denied("source copy unavailable"))?;
        let files = snapshot(&self.root, &copy, self.policy.max_file_bytes).map_err(Denied)?;
        let bytes =
            canonical_bytes(&files, MAX_INTERNAL).map_err(|_| Denied("source tree too large"))?;
        let digest = structural_digest("coding-source-tree/v1", &[&bytes])
            .map_err(|_| Denied("source tree digest failed"))?;
        match action {
            CodingAction::Search { query, directory } => {
                if query.as_str().contains('\n') || query.as_str().contains('\r') {
                    return Err(Denied("multiline search query denied"));
                }
                let target = directory
                    .as_ref()
                    .map_or(".".to_owned(), |p| p.as_str().to_owned());
                let args = vec![
                    "--fixed-strings".into(),
                    "--line-number".into(),
                    "--with-filename".into(),
                    "--no-follow".into(),
                    "--hidden".into(),
                    "--no-ignore".into(),
                    "--color=never".into(),
                    "--".into(),
                    query.as_str().into(),
                    target,
                ];
                Ok((
                    self.private_process(dispatch, &copy, ToolId::Ripgrep, args, false)?,
                    None,
                ))
            }
            CodingAction::JjSnapshot => {
                let baseline = self.run_dir.join("jj-baseline");
                if baseline.exists() {
                    fs::remove_file(&baseline).map_err(|_| Denied("baseline unavailable"))?;
                }
                if repo.exists() {
                    fs::remove_dir_all(&repo)
                        .map_err(|_| Denied("private repository unavailable"))?;
                }
                fs::rename(&copy, &repo).map_err(|_| Denied("private repository unavailable"))?;
                let init_id = content_digest(format!("{}:jj-init", dispatch.as_str()).as_bytes());
                let init = self.private_process(
                    &init_id,
                    &repo,
                    ToolId::Jj,
                    vec!["git".into(), "init".into(), "--no-colocate".into()],
                    true,
                )?;
                if !init.success {
                    return Ok((init, None));
                }
                let track_id = content_digest(format!("{}:jj-track", dispatch.as_str()).as_bytes());
                let track = self.private_process(
                    &track_id,
                    &repo,
                    ToolId::Jj,
                    vec![
                        "file".into(),
                        "track".into(),
                        "--include-ignored".into(),
                        "all()".into(),
                    ],
                    true,
                )?;
                if !track.success {
                    return Ok((track, None));
                }
                let commit = self.private_process(
                    dispatch,
                    &repo,
                    ToolId::Jj,
                    vec![
                        "--config=user.name=Metron".into(),
                        "--config=user.email=metron@invalid".into(),
                        "commit".into(),
                        "-m".into(),
                        "baseline".into(),
                    ],
                    true,
                )?;
                if !commit.success {
                    return Ok((commit, None));
                }
                fs::write(self.run_dir.join("jj-baseline"), digest.as_str())
                    .map_err(|_| Denied("baseline unavailable"))?;
                Ok((commit, Some(digest)))
            }
            CodingAction::JjDiff => {
                if !repo.join(".jj").is_dir() || !self.run_dir.join("jj-baseline").is_file() {
                    return Err(Denied("private baseline unavailable"));
                }
                for item in
                    fs::read_dir(&repo).map_err(|_| Denied("private repository unavailable"))?
                {
                    let item = item.map_err(|_| Denied("private repository unavailable"))?;
                    if item.file_name() == ".jj" {
                        continue;
                    }
                    let path = item.path();
                    if item
                        .file_type()
                        .map_err(|_| Denied("private repository unavailable"))?
                        .is_dir()
                    {
                        fs::remove_dir_all(path)
                    } else {
                        fs::remove_file(path)
                    }
                    .map_err(|_| Denied("private refresh failed"))?;
                }
                for item in fs::read_dir(&copy).map_err(|_| Denied("source copy unavailable"))? {
                    let item = item.map_err(|_| Denied("source copy unavailable"))?;
                    fs::rename(item.path(), repo.join(item.file_name()))
                        .map_err(|_| Denied("private refresh failed"))?;
                }
                let track_id = content_digest(format!("{}:jj-track", dispatch.as_str()).as_bytes());
                let track = self.private_process(
                    &track_id,
                    &repo,
                    ToolId::Jj,
                    vec![
                        "file".into(),
                        "track".into(),
                        "--include-ignored".into(),
                        "all()".into(),
                    ],
                    true,
                )?;
                if !track.success {
                    return Ok((track, None));
                }
                let done = self.private_process(
                    dispatch,
                    &repo,
                    ToolId::Jj,
                    vec![
                        "--color=never".into(),
                        "diff".into(),
                        "--git".into(),
                        "--from".into(),
                        "@-".into(),
                        "--to".into(),
                        "@".into(),
                    ],
                    true,
                )?;
                if done.success {
                    Ok((done, Some(digest)))
                } else {
                    Ok((done, None))
                }
            }
            _ => Err(Denied("unsupported private action")),
        }
    }

    pub fn freeze(&mut self, dispatch: &Digest) -> Result<CodingArtifact, String> {
        if self.uncertain {
            return Err("prior process cleanup was not established".into());
        }
        let name = format!("source-{}", dispatch.as_str());
        let destination = self.run_dir.join(&name);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&destination)
            .map_err(|_| "snapshot destination unavailable")?;
        let result =
            snapshot(&self.root, &destination, self.policy.max_file_bytes).and_then(|files| {
                if files.is_empty() {
                    return Err("empty source snapshot");
                }
                let bytes =
                    canonical_bytes(&files, MAX_INTERNAL).map_err(|_| "snapshot too large")?;
                let tree_digest = structural_digest("coding-source-tree/v1", &[&bytes])
                    .map_err(|_| "snapshot digest failed")?;
                Ok(CodingArtifact {
                    relative_root: WorkspacePath::new(name).map_err(|_| "invalid snapshot name")?,
                    files,
                    tree_digest,
                })
            });
        if result.is_err() {
            let _ = fs::remove_dir_all(&destination);
        }
        result.map_err(str::to_owned)
    }

    fn output(
        &self,
        action: &CodingAction,
        status: ToolStatus,
        body: &str,
        truncated: bool,
        files: Vec<CodingFile>,
    ) -> ToolOutcome {
        let end = body.floor_char_boundary(body.len().min(self.policy.max_output_bytes as usize));
        ToolOutcome::Known(CodingToolOutput {
            tool: action.tool(),
            status,
            body: BoundedText::new(&body[..end]).expect("bounded output"),
            truncated: truncated || end < body.len(),
            exit_code: None,
            files,
            process_tree_closed: true,
        })
    }
}

fn check_activity_marker(marker: &Path) -> Result<(), String> {
    match fs::symlink_metadata(marker) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(
            "workspace has an unresolved process dispatch; reconcile cleanup before reuse".into(),
        ),
    }
}

fn list_directory(
    root: &File,
    path: Option<&WorkspacePath>,
) -> Result<(String, bool), &'static str> {
    let directory = if let Some(path) = path {
        if path
            .as_str()
            .split('/')
            .any(|part| matches!(part, ".git" | ".jj"))
        {
            return Err("repository metadata denied");
        }
        File::from(
            openat2(
                root,
                path.as_str(),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
            )
            .map_err(|_| "directory unavailable or unsafe")?,
        )
    } else {
        root.try_clone().map_err(|_| "workspace unavailable")?
    };
    let mut entries = Dir::read_from(&directory).map_err(|_| "directory unreadable")?;
    let mut names = Vec::new();
    let mut limited = false;
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(|_| "directory unreadable")?;
        let Ok(name) = entry.file_name().to_str() else {
            continue;
        };
        if matches!(name, "." | ".." | ".git" | ".jj") {
            continue;
        }
        if !matches!(
            entry.file_type(),
            FileType::RegularFile | FileType::Directory
        ) {
            continue;
        }
        if names.len() == 1024 {
            limited = true;
            break;
        }
        names.push(name.to_owned());
    }
    names.sort_unstable();
    Ok((names.join("\n"), limited))
}

// The destination is a new, host-owned directory under the trusted run root.
pub fn inspect_artifact(run_dir: &Path, artifact: &CodingArtifact) -> Result<bool, String> {
    artifact
        .validate()
        .map_err(|_| "invalid artifact manifest")?;
    let parent = File::from(
        open(
            run_dir,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| "run directory unavailable")?,
    );
    let root = File::from(
        openat2(
            &parent,
            artifact.relative_root.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|_| "artifact root unsafe")?,
    );
    let (files, directories) =
        source_files(&root, None, 1024 * 1024, false).map_err(str::to_owned)?;
    let mut expected = BTreeSet::new();
    for file in &artifact.files {
        let mut path = file.path.as_str();
        while let Some((parent, _)) = path.rsplit_once('/') {
            expected.insert(parent.to_owned());
            path = parent;
        }
    }
    Ok(files == artifact.files && directories == expected)
}
fn snapshot(
    root: &File,
    destination: &Path,
    max_file: u32,
) -> Result<Vec<CodingFile>, &'static str> {
    source_files(root, Some(destination), max_file, true).map(|(files, _)| files)
}
fn source_files(
    root: &File,
    destination: Option<&Path>,
    max_file: u32,
    workspace: bool,
) -> Result<(Vec<CodingFile>, BTreeSet<String>), &'static str> {
    let mut pending = vec![String::new()];
    let mut files = Vec::new();
    let mut directories = BTreeSet::new();
    let mut total = 0usize;
    let mut entries_seen = 0usize;
    while let Some(directory) = pending.pop() {
        let dir = if directory.is_empty() {
            root.try_clone().map_err(|_| "workspace unavailable")?
        } else {
            File::from(
                openat2(
                    root,
                    directory.as_str(),
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    Mode::empty(),
                    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
                )
                .map_err(|_| "snapshot directory unsafe")?,
            )
        };
        let mut entries = Dir::read_from(&dir).map_err(|_| "snapshot directory unreadable")?;
        while let Some(entry) = entries.read() {
            let entry = entry.map_err(|_| "snapshot directory unreadable")?;
            let name = entry.file_name().to_str().map_err(|_| "non-UTF-8 path")?;
            if matches!(name, "." | "..") {
                continue;
            }
            if matches!(name, ".git" | ".jj") || (directory.is_empty() && name == "target") {
                if workspace {
                    continue;
                } else {
                    return Err("unexpected artifact metadata or build output");
                }
            }
            entries_seen += 1;
            if entries_seen > 1024 {
                return Err("snapshot entry limit");
            }
            let name = if directory.is_empty() {
                name.to_owned()
            } else {
                format!("{directory}/{name}")
            };
            let path = WorkspacePath::new(&name).map_err(|_| "invalid source path")?;
            let object = File::from(
                openat2(
                    root,
                    path.as_str(),
                    OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
                    Mode::empty(),
                    ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
                )
                .map_err(|_| "snapshot path unsafe")?,
            );
            let metadata = object
                .metadata()
                .map_err(|_| "snapshot metadata unavailable")?;
            if metadata.is_dir() {
                directories.insert(name.clone());
                pending.push(name);
            } else if metadata.is_file() && metadata.nlink() == 1 {
                if files.len() >= 128 {
                    return Err("snapshot file limit");
                }
                let bytes = read_file(root, &path, max_file)?;
                total += bytes.len();
                if total > 4 * 1024 * 1024 {
                    return Err("snapshot byte limit");
                }
                if let Some(destination) = destination {
                    let dest_path = destination.join(&name);
                    fs::create_dir_all(dest_path.parent().ok_or("snapshot parent unavailable")?)
                        .map_err(|_| "snapshot directory creation failed")?;
                    let mut dest = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(dest_path)
                        .map_err(|_| "snapshot file creation failed")?;
                    dest.write_all(&bytes)
                        .map_err(|_| "snapshot write failed")?;
                    dest.sync_all().map_err(|_| "snapshot sync failed")?;
                }
                files.push(CodingFile {
                    path,
                    digest: content_digest(&bytes),
                    bytes: SqlU64::new(bytes.len() as u64).expect("bounded file"),
                });
            } else {
                return Err("snapshot file type or link count denied");
            }
        }
    }
    files.sort_by(|a, b| a.path.as_str().cmp(b.path.as_str()));
    Ok((files, directories))
}

// Caller holds the workspace lock and guarantees there are no live sandbox
// descendants. The lock is cooperative with trusted same-user host processes.
fn edit_file(
    root: &File,
    path: &WorkspacePath,
    expected: Option<&Digest>,
    bytes: &[u8],
    max: u32,
    dispatch: &Digest,
) -> Result<CodingFile, &'static str> {
    if bytes.len() > max as usize {
        return Err("file exceeds size limit");
    }
    if path
        .as_str()
        .split('/')
        .any(|p| matches!(p, ".git" | ".jj"))
    {
        return Err("repository metadata denied");
    }
    let (parent_path, name) = path
        .as_str()
        .rsplit_once('/')
        .map_or((None, path.as_str()), |(p, n)| (Some(p), n));
    // Creating a new file includes creating its missing parent directories.
    // Existing-file CAS never creates directories on a failed lookup.
    if expected.is_none()
        && let Some(parent) = parent_path
    {
        let mut prefix = String::new();
        for component in parent.split('/') {
            let ancestor = if prefix.is_empty() {
                root.try_clone().map_err(|_| "workspace unavailable")?
            } else {
                File::from(
                    openat2(
                        root,
                        prefix.as_str(),
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                        Mode::empty(),
                        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
                    )
                    .map_err(|_| "parent unavailable or unsafe")?,
                )
            };
            match mkdirat(&ancestor, component, Mode::from_raw_mode(0o700)) {
                Ok(()) => ancestor.sync_all().map_err(|_| "directory sync failed")?,
                Err(error) if error == rustix::io::Errno::EXIST => {}
                Err(_) => return Err("parent creation failed"),
            }
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            openat2(
                root,
                prefix.as_str(),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
            )
            .map_err(|_| "parent unavailable or unsafe")?;
        }
    }
    let parent = match parent_path {
        Some(p) => File::from(
            openat2(
                root,
                p,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
            )
            .map_err(|_| "parent unavailable or unsafe")?,
        ),
        None => root.try_clone().map_err(|_| "workspace unavailable")?,
    };
    if let Some(expected) = expected {
        let current = read_file(root, path, max)?;
        if content_digest(&current) != *expected {
            return Err("expected digest mismatch");
        }
    }
    let temporary = format!(".metron-edit-{}", dispatch.as_str());
    let mut temp = File::from(
        openat(
            &parent,
            temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|_| "temporary file unavailable")?,
    );
    if temp.write_all(bytes).is_err() || temp.sync_all().is_err() {
        let _ = unlinkat(&parent, temporary.as_str(), AtFlags::empty());
        return Err("temporary write failed");
    }
    let result = if expected.is_some() {
        renameat(&parent, temporary.as_str(), &parent, name)
    } else {
        renameat_with(
            &parent,
            temporary.as_str(),
            &parent,
            name,
            RenameFlags::NOREPLACE,
        )
    };
    if let Err(error) = result {
        let _ = unlinkat(&parent, temporary.as_str(), AtFlags::empty());
        if expected.is_none() && error == rustix::io::Errno::EXIST {
            return Err("file exists; read its digest before replacing it");
        }
        return Err("atomic replacement failed");
    }
    parent
        .sync_all()
        .map_err(|_| "replacement directory sync failed")?;
    Ok(CodingFile {
        path: path.clone(),
        digest: content_digest(bytes),
        bytes: SqlU64::new(bytes.len() as u64).expect("bounded file"),
    })
}

fn read_file(root: &File, path: &WorkspacePath, max: u32) -> Result<Vec<u8>, &'static str> {
    if path
        .as_str()
        .split('/')
        .any(|part| matches!(part, ".git" | ".jj"))
    {
        return Err("repository metadata denied");
    }
    let file = File::from(
        openat2(
            root,
            path.as_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|e| {
            if e == rustix::io::Errno::NOENT {
                "file not found"
            } else {
                "file unavailable or unsafe"
            }
        })?,
    );
    let metadata = file.metadata().map_err(|_| "metadata unavailable")?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > u64::from(max) {
        return Err("file type, link count or size denied");
    }
    let mut bytes = Vec::new();
    file.take(u64::from(max) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "file unreadable")?;
    if bytes.len() > max as usize {
        return Err("file exceeds size limit");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    #[test]
    fn read_rejects_symlinks_and_repository_metadata() {
        let dir = std::env::temp_dir().join(format!("metron-coding-read-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("file"), b"hello").unwrap();
        symlink("file", dir.join("link")).unwrap();
        fs::create_dir(dir.join("nested")).unwrap();
        fs::write(dir.join("nested/file"), b"nested").unwrap();
        symlink("nested", dir.join("ancestor-link")).unwrap();
        fs::hard_link(dir.join("file"), dir.join("hardlink")).unwrap();
        fs::create_dir(dir.join(".git")).unwrap();
        fs::write(dir.join(".git/config"), b"secret").unwrap();
        let root = File::open(&dir).unwrap();
        assert_eq!(
            read_file(&root, &WorkspacePath::new("nested/file").unwrap(), 10).unwrap(),
            b"nested"
        );
        assert!(read_file(&root, &WorkspacePath::new("file").unwrap(), 10).is_err());
        assert!(
            read_file(
                &root,
                &WorkspacePath::new("ancestor-link/file").unwrap(),
                10
            )
            .is_err()
        );
        assert!(read_file(&root, &WorkspacePath::new("link").unwrap(), 10).is_err());
        assert!(read_file(&root, &WorkspacePath::new("file").unwrap(), 3).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn cas_and_exclusive_workspace_lock() {
        let dir = std::env::temp_dir().join(format!("metron-coding-cas-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let root = File::open(&dir).unwrap();
        let other = File::open(&dir).unwrap();
        flock(&root, FlockOperation::NonBlockingLockExclusive).unwrap();
        assert!(flock(&other, FlockOperation::NonBlockingLockExclusive).is_err());
        let path = WorkspacePath::new("source.rs").unwrap();
        let dispatch = content_digest(b"dispatch");
        edit_file(&root, &path, None, b"one", 100, &dispatch).unwrap();
        assert!(edit_file(&root, &path, None, b"two", 100, &dispatch).is_err());
        assert!(
            edit_file(
                &root,
                &path,
                Some(&content_digest(b"wrong")),
                b"two",
                100,
                &dispatch
            )
            .is_err()
        );
        edit_file(
            &root,
            &path,
            Some(&content_digest(b"one")),
            b"two",
            100,
            &dispatch,
        )
        .unwrap();
        assert_eq!(fs::read(dir.join("source.rs")).unwrap(), b"two");
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unresolved_process_marker_refuses_workspace_reuse() {
        let dir = std::env::temp_dir().join(format!("metron-marker-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let marker = dir.join("active");
        assert!(check_activity_marker(&marker).is_ok());
        fs::write(&marker, b"unknown unit").unwrap();
        assert!(check_activity_marker(&marker).is_err());
        fs::remove_file(&marker).unwrap();
        symlink("missing", &marker).unwrap();
        assert!(check_activity_marker(&marker).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nested_edit_refuses_symlink_parent() {
        let dir = std::env::temp_dir().join(format!("metron-edit-parent-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let outside = dir.join("outside");
        let workspace = dir.join("workspace");
        fs::create_dir(&outside).unwrap();
        fs::create_dir(&workspace).unwrap();
        symlink(&outside, workspace.join("link")).unwrap();
        let root = File::open(&workspace).unwrap();
        let dispatch = content_digest(b"nested");
        assert!(
            edit_file(
                &root,
                &WorkspacePath::new("link/nested/file").unwrap(),
                None,
                b"bad",
                100,
                &dispatch
            )
            .is_err()
        );
        assert!(!outside.join("nested").exists());
        edit_file(
            &root,
            &WorkspacePath::new("new/nested/file").unwrap(),
            None,
            b"ok",
            100,
            &dispatch,
        )
        .unwrap();
        assert_eq!(fs::read(workspace.join("new/nested/file")).unwrap(), b"ok");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn artifact_inspection_detects_tampering_and_extra_files() {
        let dir =
            std::env::temp_dir().join(format!("metron-artifact-inspect-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let source = dir.join("source");
        let dest = dir.join("artifact");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(source.join("file"), b"original").unwrap();
        fs::create_dir_all(source.join("nested/parent")).unwrap();
        fs::write(source.join("nested/parent/file"), b"nested").unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        let files = snapshot(&File::open(&source).unwrap(), &dest, 100).unwrap();
        let bytes = canonical_bytes(&files, MAX_INTERNAL).unwrap();
        let artifact = CodingArtifact {
            relative_root: WorkspacePath::new("artifact").unwrap(),
            files,
            tree_digest: structural_digest("coding-source-tree/v1", &[&bytes]).unwrap(),
        };
        assert!(!dest.join("empty").exists());
        assert!(dest.join("nested/parent/file").is_file());
        assert!(inspect_artifact(&dir, &artifact).unwrap());
        fs::create_dir(dest.join("extra-empty")).unwrap();
        assert!(!inspect_artifact(&dir, &artifact).unwrap());
        fs::remove_dir(dest.join("extra-empty")).unwrap();
        fs::write(dest.join("file"), b"tampered").unwrap();
        assert!(!inspect_artifact(&dir, &artifact).unwrap());
        fs::write(dest.join("file"), b"original").unwrap();
        fs::write(dest.join("extra"), b"extra").unwrap();
        assert!(!inspect_artifact(&dir, &artifact).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn snapshot_contains_sorted_verified_content_and_rejects_symlinks() {
        let dir =
            std::env::temp_dir().join(format!("metron-coding-snapshot-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let source = dir.join("workspace");
        let dest = dir.join("snapshot");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(source.join("z"), b"z").unwrap();
        fs::write(source.join("a"), b"a").unwrap();
        let root = File::open(&source).unwrap();
        let files = snapshot(&root, &dest, 100).unwrap();
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        assert_eq!(
            files[0].digest,
            content_digest(&fs::read(dest.join("a")).unwrap())
        );
        fs::remove_dir_all(&dest).unwrap();
        fs::create_dir(&dest).unwrap();
        symlink("a", source.join("link")).unwrap();
        assert!(snapshot(&root, &dest, 100).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
#[path = "coding_tools/security_tests.rs"]
mod security_tests;
