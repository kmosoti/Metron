//! Closed host contract verifier (`ContractVerifier`) implementing `CodingVerifier`
//! over persisted immutable `CodingArtifact` snapshots via `VerifierRunner` / `EvaluatorRunner`.

use super::io::{
    open_external_nofollow, read_external_json, read_open_file_bounded, write_external_new_json,
};
use super::types::*;
use super::validation::{
    build_contract_report, expected_check_specs_for_bundle, validate_contract_settings,
    verify_report_structural_only,
};
use crate::{
    coding_acceptance::{CargoStage, EvaluatorRunner, OUTPUT_LIMIT, RunOutput, RunnerError},
    coding_tools::{VerifierRunner, inspect_artifact},
};
use metron_harness::orchestrator::{CodingVerifier, DriveError};
use metron_protocol::{
    BoundedText, CodingVerificationSelection, Digest,
    coding::WorkspacePath,
    coding_runtime::{CodingArtifact, CodingObservation},
    content_digest,
    domain::ReceiptOutput,
    store::Projection,
    structural_digest,
    toolkit::ToolCatalog,
};
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Component, Path, PathBuf},
    time::Instant,
};

#[derive(Debug, Deserialize)]
struct CargoMetadataDoc {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,
    source: Option<serde_json::Value>,
    manifest_path: String,
    #[serde(default)]
    targets: Vec<CargoMetadataTarget>,
    #[serde(default)]
    dependencies: Vec<CargoMetadataDep>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataTarget {
    name: String,
    src_path: String,
    #[serde(default)]
    kind: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDep {
    name: String,
    source: Option<serde_json::Value>,
    registry: Option<serde_json::Value>,
    path: Option<String>,
}

fn normalize_lexical_abs(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("expected absolute path, got '{}'", path.display()));
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::RootDir => {}
            Component::CurDir => {}
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(format!(
                        "path escapes filesystem root: '{}'",
                        path.display()
                    ));
                }
            }
            Component::Prefix(_) => {
                return Err(format!("unsupported path prefix: '{}'", path.display()));
            }
        }
    }
    let mut out = PathBuf::from("/");
    for part in parts {
        out.push(part);
    }
    Ok(out)
}

fn resolve_inside_snapshot_roots<'a>(
    raw: &Path,
    base_dir: Option<&Path>,
    roots: &'a [&'a Path],
) -> Result<PathBuf, String> {
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else if let Some(base) = base_dir {
        base.join(raw)
    } else {
        return Err(format!("relative path '{}' has no base", raw.display()));
    };
    let norm = normalize_lexical_abs(&joined)?;
    for root in roots {
        let norm_root = normalize_lexical_abs(root)?;
        if let Ok(rel) = norm.strip_prefix(&norm_root) {
            return Ok(rel.to_path_buf());
        }
    }
    Err(format!(
        "path '{}' resolves outside frozen workspace snapshot",
        raw.display()
    ))
}

/// Validate `cargo metadata --format-version=1 --offline --locked --no-deps` strictly:
/// - `1..=max_local_packages` (`<= 2`) local packages with `source == null`
/// - every package `manifest_path` strictly inside snapshot root and present in `artifact.files`
/// - every dependency has `source == null`, `registry == null`, and snapshot-internal local `path`
///   resolving to an inspected package manifest
/// - every target source (including custom build scripts) is present in the same snapshot
/// - exact count (`== 1`) of `bin` targets across the workspace matching `selected_bin`.
#[cfg(test)]
pub(crate) fn validate_cargo_metadata_json(
    metadata_bytes: &[u8],
    artifact: &CodingArtifact,
    selected_bin: &str,
    max_local_packages: u8,
    allowed_roots: &[&Path],
) -> Result<(), String> {
    validate_cargo_metadata_closure(
        metadata_bytes,
        artifact,
        selected_bin,
        max_local_packages,
        allowed_roots,
        None,
    )
}

pub(crate) fn validate_cargo_metadata_closure(
    metadata_bytes: &[u8],
    artifact: &CodingArtifact,
    selected_bin: &str,
    max_local_packages: u8,
    allowed_roots: &[&Path],
    snapshot_bytes: Option<&BTreeMap<String, Vec<u8>>>,
) -> Result<(), String> {
    let doc: CargoMetadataDoc = serde_json::from_slice(metadata_bytes)
        .map_err(|e| format!("invalid cargo metadata JSON: {e}"))?;
    let pkg_cap = usize::from(max_local_packages).min(MAX_LOCAL_RUST_PACKAGES);
    if doc.packages.is_empty() || doc.packages.len() > pkg_cap {
        return Err(format!(
            "cargo metadata package count {} must be 1..={pkg_cap}",
            doc.packages.len()
        ));
    }

    let snapshot_paths: BTreeSet<&str> = artifact.files.iter().map(|f| f.path.as_str()).collect();
    let mut inspected_manifests: BTreeSet<String> = BTreeSet::new();
    let mut pkg_dep_manifests: Vec<(String, PathBuf, String)> = Vec::new();
    let mut bin_names: Vec<String> = Vec::new();

    for pkg in &doc.packages {
        if pkg.source.is_some() {
            return Err(format!(
                "non-local package '{}' has non-null source {:?}",
                pkg.name, pkg.source
            ));
        }
        let manifest_abs = normalize_lexical_abs(Path::new(&pkg.manifest_path))?;
        let rel_manifest = resolve_inside_snapshot_roots(&manifest_abs, None, allowed_roots)?;
        let rel_manifest_str = rel_manifest
            .to_str()
            .ok_or_else(|| "non-UTF8 manifest path in metadata".to_string())?;
        if rel_manifest.file_name().and_then(|s| s.to_str()) != Some("Cargo.toml")
            || !snapshot_paths.contains(rel_manifest_str)
        {
            return Err(format!(
                "package manifest '{}' is missing from frozen snapshot",
                rel_manifest.display()
            ));
        }
        if !inspected_manifests.insert(rel_manifest_str.to_string()) {
            return Err(format!("duplicate package manifest '{rel_manifest_str}'"));
        }
        if snapshot_bytes.is_some_and(|bytes| !bytes.contains_key(rel_manifest_str)) {
            return Err(format!(
                "package manifest '{rel_manifest_str}' was not loaded from snapshot"
            ));
        }
        let pkg_abs_dir = manifest_abs
            .parent()
            .ok_or_else(|| "manifest path has no parent directory".to_string())?
            .to_path_buf();

        for dep in &pkg.dependencies {
            if dep.source.is_some() || dep.registry.is_some() {
                return Err(format!(
                    "dependency '{}' uses non-local source/registry (forbidden)",
                    dep.name
                ));
            }
            let Some(dep_path) = &dep.path else {
                return Err(format!(
                    "dependency '{}' lacks an explicit snapshot-local path",
                    dep.name
                ));
            };
            pkg_dep_manifests.push((dep.name.clone(), pkg_abs_dir.clone(), dep_path.clone()));
        }

        for target in &pkg.targets {
            // Cargo emits absolute source paths. Resolve using the same trusted snapshot
            // aliases as manifests, then require a file actually bound into this artifact.
            // Runtime bytes were opened beneath the snapshot with NO_SYMLINKS and hashed.
            let source_abs = normalize_lexical_abs(Path::new(&target.src_path))?;
            let source_rel = resolve_inside_snapshot_roots(&source_abs, None, allowed_roots)?;
            let source_path = source_rel
                .to_str()
                .ok_or_else(|| "non-UTF8 target source path in metadata".to_string())?;
            if !snapshot_paths.contains(source_path)
                || snapshot_bytes.is_some_and(|bytes| !bytes.contains_key(source_path))
            {
                return Err(format!(
                    "target '{}' source '{}' is missing from frozen snapshot",
                    target.name, target.src_path
                ));
            }
            if target.kind.iter().any(|k| k == "bin") {
                bin_names.push(target.name.clone());
            }
        }
    }

    // All dependency kinds and target configurations are included by Cargo metadata.
    // Requiring every edge to lead to an inspected package closes the graph inductively,
    // including excluded path dependencies. A virtual workspace Cargo.toml need not
    // represent a package and does not count toward the package cap.
    for (dep_name, pkg_abs_dir, dep_path) in pkg_dep_manifests {
        let rel_dep_dir =
            resolve_inside_snapshot_roots(Path::new(&dep_path), Some(&pkg_abs_dir), allowed_roots)?;
        let dep_manifest = if rel_dep_dir.as_os_str().is_empty() {
            "Cargo.toml".to_string()
        } else {
            format!("{}/Cargo.toml", rel_dep_dir.display())
        };
        if !inspected_manifests.contains(&dep_manifest) {
            return Err(format!(
                "dependency '{dep_name}' resolved to '{dep_manifest}' which is not an inspected local package manifest"
            ));
        }
    }

    if bin_names.len() != MAX_SELECTED_CLI_TARGETS {
        return Err(format!(
            "expected exactly {MAX_SELECTED_CLI_TARGETS} binary target matching '{selected_bin}', found {}: {:?}",
            bin_names.len(),
            bin_names
        ));
    }
    if bin_names[0] != selected_bin {
        return Err(format!(
            "selected CLI binary '{selected_bin}' mismatches single built binary target '{}'",
            bin_names[0]
        ));
    }
    Ok(())
}

/// Strip Rust `//` comments, nested `/* ... */` comments, normal/raw string literals,
/// and character literals before lexical interface checks so comments and string constants
/// cannot spoof code declarations.
pub(crate) fn strip_rust_comments_and_literals(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;
    while i < bytes.len() {
        // Line comment `// ...`
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            out.push(' ');
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Nested block comment `/* ... */`
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            out.push(' ');
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Raw string literal: `r#"..."#` / `br#"..."#`
        let raw_prefix = if bytes[i] == b'r' {
            Some(i + 1)
        } else if i + 1 < bytes.len() && bytes[i] == b'b' && bytes[i + 1] == b'r' {
            Some(i + 2)
        } else {
            None
        };
        if let Some(mut j) = raw_prefix {
            let mut hashes = 0usize;
            while j < bytes.len() && bytes[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'"' {
                out.push(' ');
                i = j + 1;
                while i < bytes.len() {
                    if bytes[i] == b'"'
                        && i + 1 + hashes <= bytes.len()
                        && bytes[i + 1..i + 1 + hashes].iter().all(|&b| b == b'#')
                    {
                        i += 1 + hashes;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }
        // Normal / byte string literal `"..."`
        if bytes[i] == b'"' || (i + 1 < bytes.len() && bytes[i] == b'b' && bytes[i + 1] == b'"') {
            out.push(' ');
            if bytes[i] == b'b' {
                i += 2;
            } else {
                i += 1;
            }
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Char literal `'x'` / `'\n'` (avoid eating lifetimes like `'a`)
        if bytes[i] == b'\'' {
            if i + 2 < bytes.len() && bytes[i + 1] != b'\\' && bytes[i + 2] == b'\'' {
                out.push(' ');
                i += 3;
                continue;
            }
            if i + 3 < bytes.len() && bytes[i + 1] == b'\\' {
                let mut k = i + 2;
                while k < bytes.len() && k <= i + 6 && bytes[k] != b'\'' && bytes[k] != b'\n' {
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b'\'' {
                    out.push(' ');
                    i = k + 1;
                    continue;
                }
            }
        }
        let ch = src[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

pub(crate) fn check_interface_lexical_tokens(
    src_utf8: &str,
    check: &InterfaceCheckV1,
) -> Result<(), String> {
    if check.mode == InterfaceCheckModeV1::SemanticRustApi {
        return Err(
            "unsupported semantic_rust_api interface check (lexical token check cannot prove Rust API semantics)".into(),
        );
    }
    let stripped = strip_rust_comments_and_literals(src_utf8);
    for req in &check.required_substrings {
        if !stripped.contains(req.as_str()) {
            return Err(format!(
                "missing required non-comment source token '{}'",
                req.as_str()
            ));
        }
    }
    for forb in &check.forbidden_substrings {
        if stripped.contains(forb.as_str()) {
            return Err(format!(
                "forbidden non-comment source token '{}' present",
                forb.as_str()
            ));
        }
    }
    Ok(())
}

type RunnerFactory =
    Box<dyn FnMut(&CodingArtifact, &Digest) -> Result<Box<dyn EvaluatorRunner>, RunnerError>>;

/// Bounded closed-registry host verifier bound to a frozen `ContractBundleV1`,
/// pinned `ToolCatalog` + checker implementation digest, and `EvaluatorRunner`.
pub(crate) struct ContractVerifier {
    bundle: ContractBundleV1,
    bundle_file_sha256: Digest,
    checker_impl_digest: Digest,
    run_dir: PathBuf,
    allowed_metadata_roots: Vec<PathBuf>,
    runner_factory: RunnerFactory,
}

impl ContractVerifier {
    pub(crate) fn with_verifier_runner(
        bundle: ContractBundleV1,
        bundle_file_sha256: Digest,
        catalog: ToolCatalog,
        run_dir: PathBuf,
    ) -> Result<Self, String> {
        let checker_impl_digest = contract_verifier_implementation_digest(&catalog)?;
        let run_dir_clone = run_dir.clone();
        Ok(Self {
            bundle,
            bundle_file_sha256,
            checker_impl_digest,
            run_dir,
            allowed_metadata_roots: vec![PathBuf::from("/workspace")],
            runner_factory: Box::new(move |artifact, dispatch| {
                let runner = VerifierRunner::new(&catalog, artifact, &run_dir_clone, dispatch)?;
                Ok(Box::new(runner))
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_custom_runner<F>(
        bundle: ContractBundleV1,
        bundle_file_sha256: Digest,
        checker_impl_digest: Digest,
        run_dir: PathBuf,
        allowed_metadata_roots: Vec<PathBuf>,
        mut make_runner: F,
    ) -> Self
    where
        F: FnMut(&CodingArtifact, &Digest) -> Result<Box<dyn EvaluatorRunner>, RunnerError>
            + 'static,
    {
        Self {
            bundle,
            bundle_file_sha256,
            checker_impl_digest,
            run_dir,
            allowed_metadata_roots,
            runner_factory: Box::new(move |artifact, dispatch| make_runner(artifact, dispatch)),
        }
    }

    fn read_snapshot_file(
        &self,
        snapshot_dir: &File,
        rel_path: &WorkspacePath,
    ) -> Result<Vec<u8>, String> {
        let fd = openat2(
            snapshot_dir,
            rel_path.as_str(),
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|e| format!("open snapshot file '{}': {e}", rel_path.as_str()))?;
        read_open_file_bounded(File::from(fd), MAX_BASELINE_TOTAL_BYTES)
    }

    fn verify_snapshot_bounds(
        &self,
        snapshot_dir: &File,
        artifact: &CodingArtifact,
    ) -> Result<BTreeMap<String, Vec<u8>>, String> {
        if artifact.files.is_empty() || artifact.files.len() > MAX_BASELINE_FILES {
            return Err(format!(
                "frozen snapshot file count {} out of 1..={MAX_BASELINE_FILES} bounds",
                artifact.files.len()
            ));
        }
        let mut total_bytes = 0usize;
        let mut loaded = BTreeMap::new();
        for entry in &artifact.files {
            let bytes = self.read_snapshot_file(snapshot_dir, &entry.path)?;
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| "snapshot total bytes overflow".to_string())?;
            if total_bytes > MAX_BASELINE_TOTAL_BYTES {
                return Err(format!(
                    "frozen snapshot exceeds {MAX_BASELINE_TOTAL_BYTES} byte limit"
                ));
            }
            if content_digest(&bytes) != entry.digest {
                return Err(format!(
                    "frozen snapshot file '{}' content digest mismatch",
                    entry.path.as_str()
                ));
            }
            loaded.insert(entry.path.as_str().to_string(), bytes);
        }
        Ok(loaded)
    }
}

fn push_check_record(
    checks: &mut Vec<ContractCheckResultV1>,
    id: &str,
    checker: &str,
    status: ContractCheckStatus,
    output: &[u8],
) -> Result<(), DriveError> {
    checks.push(ContractCheckResultV1 {
        id: BoundedText::new(id).map_err(|_| DriveError::Integrity)?,
        checker: BoundedText::new(checker).map_err(|_| DriveError::Integrity)?,
        status,
        output_digest: Some(content_digest(output)),
    });
    Ok(())
}

impl CodingVerifier for ContractVerifier {
    fn verify(
        &mut self,
        projection: &Projection,
    ) -> Result<(CodingVerificationSelection, bool), DriveError> {
        let started = Instant::now();
        let task = projection.task.coding().ok_or(DriveError::Integrity)?;
        if task.data().acceptance_digest != self.bundle.bundle_digest {
            return Err(DriveError::Integrity);
        }
        let task_digest = task.digest().map_err(|_| DriveError::Integrity)?;
        let publication = projection
            .verification
            .publication
            .as_ref()
            .ok_or(DriveError::Integrity)?;
        let ReceiptOutput::Coding { observation } = &publication.data().output else {
            return Err(DriveError::Integrity);
        };
        let CodingObservation::Artifact { artifact } = observation.as_ref() else {
            return Err(DriveError::Integrity);
        };
        if !inspect_artifact(&self.run_dir, artifact).unwrap_or(false) {
            return Err(DriveError::Integrity);
        }

        // Verify persisted contract-settings.json matches current host executable + bundle + task
        // BEFORE reusing any retained report or invoking fresh verification runners.
        let (_, settings, _) = read_external_json::<ContractSettingsV1>(
            &self.run_dir.join("contract-settings.json"),
            None,
        )
        .map_err(|_| DriveError::Integrity)?;
        validate_contract_settings(&settings).map_err(|_| DriveError::Integrity)?;
        let expected_eval =
            expected_contract_evaluator_digest(&self.bundle, &self.checker_impl_digest)
                .map_err(|_| DriveError::Integrity)?;
        if settings.bundle_digest != self.bundle.bundle_digest
            || settings.bundle_file_sha256 != self.bundle_file_sha256
            || settings.checker_implementation_digest != self.checker_impl_digest
            || settings.expected_evaluator_digest != expected_eval
            || settings.task != *task
        {
            return Err(DriveError::Integrity);
        }

        // Crash-recovery reuse: if immutable contract-report.json already exists before
        // RecordCodingVerification committed, verify it against task/artifact/settings/check-set
        // and reuse it without re-running or overwriting; reject conflicts or Uncertain acceptance.
        let report_path = self.run_dir.join("contract-report.json");
        match std::fs::symlink_metadata(&report_path) {
            Ok(_) => {
                let (_, existing, _) = read_external_json::<ContractReportV1>(&report_path, None)
                    .map_err(|_| DriveError::Integrity)?;
                verify_report_structural_only(&existing, &self.bundle)
                    .map_err(|_| DriveError::Integrity)?;
                if existing.body.task_digest != task_digest
                    || existing.body.tree_digest != artifact.tree_digest
                    || existing.body.checker_implementation_digest != self.checker_impl_digest
                    || existing.body.evaluator_digest != expected_eval
                {
                    return Err(DriveError::Integrity);
                }
                let has_uncertain = existing
                    .body
                    .checks
                    .iter()
                    .any(|c| c.status == ContractCheckStatus::Uncertain);
                if has_uncertain && existing.body.accepted {
                    return Err(DriveError::Integrity);
                }
                let selection = CodingVerificationSelection {
                    evidence_revision: projection.verification.evidence_revision,
                    plan_evidence: projection
                        .state
                        .coding_context
                        .as_ref()
                        .and_then(|c| c.plan_evidence.clone())
                        .ok_or(DriveError::Integrity)?,
                    model_evidence: projection
                        .verification
                        .model_evidence
                        .clone()
                        .ok_or(DriveError::Integrity)?,
                    artifact_evidence: projection
                        .verification
                        .publication_evidence
                        .clone()
                        .ok_or(DriveError::Integrity)?,
                    artifact_digest: artifact.tree_digest.clone(),
                    acceptance_digest: self.bundle.bundle_digest.clone(),
                    evaluator_digest: existing.body.evaluator_digest.clone(),
                    report_digest: existing.report_digest,
                };
                return Ok((selection, !has_uncertain && existing.body.accepted));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(DriveError::Integrity),
        }

        let snapshot_path = self.run_dir.join(artifact.relative_root.as_str());
        let (snapshot_dir, _) =
            open_external_nofollow(&snapshot_path, true).map_err(|_| DriveError::Integrity)?;
        let snapshot_bytes = self
            .verify_snapshot_bounds(&snapshot_dir, artifact)
            .map_err(|_| DriveError::Integrity)?;

        let dispatch = structural_digest(
            "contract-verifier-dispatch/v1",
            &[
                &self.bundle.bundle_digest.bytes(),
                &self.bundle_file_sha256.bytes(),
                &artifact.tree_digest.bytes(),
                &self.checker_impl_digest.bytes(),
            ],
        )
        .map_err(|_| DriveError::Integrity)?;

        let mut runner =
            (self.runner_factory)(artifact, &dispatch).map_err(|_| DriveError::Integrity)?;

        let mut checks = Vec::new();
        let mut cargo_invocations = 0u32;
        let mut cli_invocations = 0u32;
        let mut stdout_bytes = 0u64;

        let cap_add = |acc: &mut u64, out: &RunOutput| {
            let add = (out.stdout.len().min(OUTPUT_LIMIT))
                .saturating_add(out.stderr.len().min(OUTPUT_LIMIT)) as u64;
            *acc = acc.saturating_add(add);
        };

        // 1. Metadata: must execute BEFORE build/test and validate local-only snapshot graph.
        let root_refs: Vec<&Path> = self
            .allowed_metadata_roots
            .iter()
            .map(PathBuf::as_path)
            .collect();
        let meta_out = runner
            .cargo(CargoStage::Metadata)
            .map_err(|_| DriveError::Invalid)?;
        cargo_invocations += 1;
        cap_add(&mut stdout_bytes, &meta_out);
        let metadata_ok = meta_out.process_tree_closed
            && meta_out.exit_code == Some(0)
            && validate_cargo_metadata_closure(
                &meta_out.stdout,
                artifact,
                self.bundle.body.baseline.selected_cli_binary.as_str(),
                self.bundle.body.checker_policy.max_local_packages,
                &root_refs,
                Some(&snapshot_bytes),
            )
            .is_ok();
        push_check_record(
            &mut checks,
            "metadata",
            CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1,
            if metadata_ok {
                ContractCheckStatus::Passed
            } else {
                ContractCheckStatus::Failed
            },
            &meta_out.stdout,
        )?;

        // 2. Build (only invoked once metadata proves snapshot isolation).
        let build_ok = if metadata_ok {
            let out = runner
                .cargo(CargoStage::Build)
                .map_err(|_| DriveError::Invalid)?;
            cargo_invocations += 1;
            cap_add(&mut stdout_bytes, &out);
            let passed = out.process_tree_closed && out.exit_code == Some(0);
            push_check_record(
                &mut checks,
                "build",
                CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1,
                if passed {
                    ContractCheckStatus::Passed
                } else {
                    ContractCheckStatus::Failed
                },
                &out.stderr,
            )?;
            passed
        } else {
            push_check_record(
                &mut checks,
                "build",
                CHECKER_BUILD_CARGO_OFFLINE_LOCKED_V1,
                ContractCheckStatus::Failed,
                b"skipped due to failed snapshot cargo metadata check",
            )?;
            false
        };

        // 3. Cargo test (only invoked once metadata & build pass).
        if metadata_ok && build_ok {
            let out = runner
                .cargo(CargoStage::Test)
                .map_err(|_| DriveError::Invalid)?;
            cargo_invocations += 1;
            cap_add(&mut stdout_bytes, &out);
            let passed = out.process_tree_closed && out.exit_code == Some(0);
            push_check_record(
                &mut checks,
                "test",
                CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1,
                if passed {
                    ContractCheckStatus::Passed
                } else {
                    ContractCheckStatus::Failed
                },
                &out.stdout,
            )?;
        } else {
            push_check_record(
                &mut checks,
                "test",
                CHECKER_TEST_CARGO_TEST_OFFLINE_LOCKED_V1,
                ContractCheckStatus::Failed,
                b"skipped due to failed metadata or build check",
            )?;
        }

        // 4. Fixed-file SHA-256 checks.
        for fixed in &self.bundle.body.checker_policy.fixed_files {
            let actual = snapshot_bytes.get(fixed.path.as_str());
            let passed = actual.is_some_and(|b| content_digest(b) == fixed.expected_sha256);
            push_check_record(
                &mut checks,
                &format!("fixed:{}", fixed.id.as_str()),
                CHECKER_FIXED_FILE_V1,
                if passed {
                    ContractCheckStatus::Passed
                } else {
                    ContractCheckStatus::Failed
                },
                fixed.path.as_str().as_bytes(),
            )?;
        }

        // 5. Forbidden paths absent.
        for path in &self.bundle.body.compatibility.forbidden_files {
            let passed = !snapshot_bytes.contains_key(path.as_str());
            push_check_record(
                &mut checks,
                &format!("forbidden:{}", path.as_str()),
                CHECKER_FORBIDDEN_FILE_V1,
                if passed {
                    ContractCheckStatus::Passed
                } else {
                    ContractCheckStatus::Failed
                },
                path.as_str().as_bytes(),
            )?;
        }

        // 6. Preserved baseline file SHA-256 identity.
        let baseline_by_path: BTreeMap<&str, &Digest> = self
            .bundle
            .body
            .baseline
            .files
            .iter()
            .map(|f| (f.path.as_str(), &f.sha256))
            .collect();
        for path in &self.bundle.body.compatibility.preserved_files {
            let passed = match (
                baseline_by_path.get(path.as_str()),
                snapshot_bytes.get(path.as_str()),
            ) {
                (Some(expected_sha), Some(bytes)) => content_digest(bytes) == **expected_sha,
                _ => false,
            };
            push_check_record(
                &mut checks,
                &format!("preserved:{}", path.as_str()),
                CHECKER_PRESERVED_FILE_V1,
                if passed {
                    ContractCheckStatus::Passed
                } else {
                    ContractCheckStatus::Failed
                },
                path.as_str().as_bytes(),
            )?;
        }

        // 6b. Complete baseline -> artifact diff check against allowed_edit_paths
        // (catches CargoRun build.rs/test mutations outside declared scope).
        if !self.bundle.body.compatibility.allowed_edit_paths.is_empty() {
            let allowed_set: BTreeSet<&str> = self
                .bundle
                .body
                .compatibility
                .allowed_edit_paths
                .iter()
                .map(|p| p.as_str())
                .collect();
            let mut violations = Vec::new();
            for (path, bytes) in &snapshot_bytes {
                if !allowed_set.contains(path.as_str()) {
                    match baseline_by_path.get(path.as_str()) {
                        None => violations.push(format!("added:{path}")),
                        Some(base_sha) if content_digest(bytes) != **base_sha => {
                            violations.push(format!("modified:{path}"));
                        }
                        _ => {}
                    }
                }
            }
            for base_path in baseline_by_path.keys() {
                if !snapshot_bytes.contains_key(*base_path) && !allowed_set.contains(*base_path) {
                    violations.push(format!("removed:{base_path}"));
                }
            }
            let summary = if violations.is_empty() {
                b"scope-clean".to_vec()
            } else {
                violations.join(",").into_bytes()
            };
            push_check_record(
                &mut checks,
                "edit_scope",
                CHECKER_METADATA_SNAPSHOT_LOCAL_PATH_V1,
                if violations.is_empty() {
                    ContractCheckStatus::Passed
                } else {
                    ContractCheckStatus::Failed
                },
                &summary,
            )?;
        }

        // 7. Lexical non-comment source token checks (SemanticRustApi explicitly fails).
        for iface in &self.bundle.body.checker_policy.interface_checks {
            let passed = snapshot_bytes
                .get(iface.path.as_str())
                .and_then(|b| std::str::from_utf8(b).ok())
                .is_some_and(|txt| check_interface_lexical_tokens(txt, iface).is_ok());
            push_check_record(
                &mut checks,
                &format!("interface:{}", iface.id.as_str()),
                CHECKER_SOURCE_LITERAL_V1,
                if passed {
                    ContractCheckStatus::Passed
                } else {
                    ContractCheckStatus::Failed
                },
                iface.path.as_str().as_bytes(),
            )?;
        }

        // 8. Sealed CLI vector checks against immutable sealed binary.
        if self.bundle.body.checker_policy.cli_checker.is_some() {
            if self.bundle.body.checker_policy.cli_vectors.len() > MAX_CLI_VECTORS {
                return Err(DriveError::Integrity);
            }
            for (idx, vec) in self
                .bundle
                .body
                .checker_policy
                .cli_vectors
                .iter()
                .enumerate()
            {
                if !metadata_ok || !build_ok {
                    push_check_record(
                        &mut checks,
                        &format!("cli:{idx}"),
                        CHECKER_CLI_SELECTED_VECTORS_V1,
                        ContractCheckStatus::Failed,
                        b"skipped due to failed metadata or build",
                    )?;
                    continue;
                }
                let args: Vec<&str> = vec.args.iter().map(|a| a.as_str()).collect();
                let out = runner
                    .candidate(
                        self.bundle.body.baseline.selected_cli_binary.as_str(),
                        &args,
                    )
                    .map_err(|_| DriveError::Invalid)?;
                cli_invocations += 1;
                cap_add(&mut stdout_bytes, &out);
                let passed = out.process_tree_closed
                    && out.exit_code == Some(vec.expected_exit_code)
                    && out.stdout.as_slice() == vec.expected_stdout.as_str().as_bytes();
                push_check_record(
                    &mut checks,
                    &format!("cli:{idx}"),
                    CHECKER_CLI_SELECTED_VECTORS_V1,
                    if passed {
                        ContractCheckStatus::Passed
                    } else {
                        ContractCheckStatus::Failed
                    },
                    &out.stdout,
                )?;
            }
        }

        // Confirm exact alignment against expected check specs.
        let expected_specs = expected_check_specs_for_bundle(&self.bundle);
        if checks.len() != expected_specs.len() {
            return Err(DriveError::Integrity);
        }

        let verifier_cost = ContractVerifierCostV1 {
            cargo_invocations,
            cli_invocations,
            stdout_bytes,
        };
        let report = build_contract_report(
            &self.bundle,
            task_digest,
            artifact.tree_digest.clone(),
            self.checker_impl_digest.clone(),
            checks,
            verifier_cost,
        )
        .map_err(|_| DriveError::Integrity)?;

        write_external_new_json(
            &self.run_dir.join("contract-report.json"),
            None,
            &[],
            &report,
        )
        .map_err(|_| DriveError::Integrity)?;

        let telemetry = ContractExecutionTelemetryV1 {
            wall_ms: u64::try_from(
                Instant::now()
                    .saturating_duration_since(started)
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
        };
        let _ = write_external_new_json(
            &self.run_dir.join("contract-telemetry.json"),
            None,
            &[],
            &telemetry,
        );

        let selection = CodingVerificationSelection {
            evidence_revision: projection.verification.evidence_revision,
            plan_evidence: projection
                .state
                .coding_context
                .as_ref()
                .and_then(|c| c.plan_evidence.clone())
                .ok_or(DriveError::Integrity)?,
            model_evidence: projection
                .verification
                .model_evidence
                .clone()
                .ok_or(DriveError::Integrity)?,
            artifact_evidence: projection
                .verification
                .publication_evidence
                .clone()
                .ok_or(DriveError::Integrity)?,
            artifact_digest: artifact.tree_digest.clone(),
            acceptance_digest: self.bundle.bundle_digest.clone(),
            evaluator_digest: report.body.evaluator_digest.clone(),
            report_digest: report.report_digest,
        };
        Ok((selection, report.body.accepted))
    }
}

#[cfg(test)]
mod metadata_containment_tests {
    use super::*;
    use metron_protocol::{SqlU64, coding_runtime::CodingFile};
    use serde_json::{Value, json};

    fn fixture() -> (CodingArtifact, BTreeMap<String, Vec<u8>>, Value) {
        // A virtual root is not a package: three manifests describe two packages.
        let bytes: BTreeMap<String, Vec<u8>> = [
            ("Cargo.toml", "[workspace]\nmembers = ['cli', 'helper']\n"),
            ("cli/Cargo.toml", "[package]\nname = 'cli'\n"),
            ("cli/src/main.rs", "fn main() {}\n"),
            ("helper/Cargo.toml", "[package]\nname = 'helper'\n"),
            ("helper/src/lib.rs", "pub fn helper() {}\n"),
            ("helper/build.rs", "fn main() {}\n"),
        ]
        .into_iter()
        .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
        .collect();
        let artifact = CodingArtifact {
            tree_digest: content_digest(b"test-tree"),
            relative_root: WorkspacePath::new("snapshot").unwrap(),
            files: bytes
                .iter()
                .map(|(path, bytes)| CodingFile {
                    path: WorkspacePath::new(path.clone()).unwrap(),
                    digest: content_digest(bytes),
                    bytes: SqlU64::new(bytes.len() as u64).unwrap(),
                })
                .collect(),
        };
        let metadata = json!({"packages": [
            {"name": "cli", "manifest_path": "/workspace/cli/Cargo.toml",
             "targets": [{"name": "cli", "kind": ["bin"], "src_path": "/workspace/cli/src/main.rs"}],
             "dependencies": [{"name": "helper", "path": "../helper", "kind": "build", "target": "cfg(unix)"}]},
            {"name": "helper", "manifest_path": "/workspace/helper/Cargo.toml",
             "targets": [
                {"name": "helper", "kind": ["lib"], "src_path": "/workspace/helper/src/lib.rs"},
                {"name": "build-script-build", "kind": ["custom-build"], "src_path": "/workspace/helper/build.rs"}],
             "dependencies": []}
        ]});
        (artifact, bytes, metadata)
    }

    fn validate(
        artifact: &CodingArtifact,
        bytes: &BTreeMap<String, Vec<u8>>,
        metadata: &Value,
    ) -> Result<(), String> {
        validate_cargo_metadata_closure(
            &serde_json::to_vec(metadata).unwrap(),
            artifact,
            "cli",
            2,
            &[Path::new("/workspace")],
            Some(bytes),
        )
    }

    #[test]
    fn accepts_virtual_root_with_two_inspected_packages_and_internal_build_script() {
        let (artifact, bytes, metadata) = fixture();
        validate(&artifact, &bytes, &metadata).unwrap();
    }

    #[test]
    fn rejects_excluded_dependency_chain_for_every_dependency_kind_and_configuration() {
        let (mut artifact, mut bytes, metadata) = fixture();
        // cli -> helper -> excluded b must fail even when b's manifest is frozen.
        bytes.insert("b/Cargo.toml".into(), b"[package]\nname='b'\n".to_vec());
        artifact.files.push(CodingFile {
            path: WorkspacePath::new("b/Cargo.toml").unwrap(),
            digest: content_digest(&bytes["b/Cargo.toml"]),
            bytes: SqlU64::new(bytes["b/Cargo.toml"].len() as u64).unwrap(),
        });
        for kind in [Value::Null, json!("dev"), json!("build")] {
            for target in [Value::Null, json!("cfg(target_os = \"never\")")] {
                let mut metadata = metadata.clone();
                metadata["packages"][1]["dependencies"] = json!([
                    {"name": "b", "path": "../b", "kind": kind, "target": target}
                ]);
                assert!(
                    validate(&artifact, &bytes, &metadata)
                        .unwrap_err()
                        .contains("not an inspected local package")
                );
            }
        }
    }

    #[test]
    fn rejects_escaping_binary_library_and_build_script_sources() {
        let (artifact, bytes, metadata) = fixture();
        for (package, target) in [(0, 0), (1, 0), (1, 1)] {
            for path in [
                "/host/source.rs",
                "/workspace/../host/source.rs",
                "src/main.rs",
            ] {
                let mut metadata = metadata.clone();
                metadata["packages"][package]["targets"][target]["src_path"] = json!(path);
                assert!(validate(&artifact, &bytes, &metadata).is_err(), "{path}");
            }
        }
    }

    #[test]
    fn requires_target_source_in_artifact_and_loaded_snapshot() {
        let (artifact, bytes, metadata) = fixture();
        let mut missing_artifact = artifact.clone();
        missing_artifact
            .files
            .retain(|f| f.path.as_str() != "cli/src/main.rs");
        assert!(validate(&missing_artifact, &bytes, &metadata).is_err());
        let mut missing_bytes = bytes.clone();
        missing_bytes.remove("helper/build.rs");
        assert!(validate(&artifact, &missing_bytes, &metadata).is_err());
        let mut missing_source = metadata.clone();
        missing_source["packages"][0]["targets"][0]
            .as_object_mut()
            .unwrap()
            .remove("src_path");
        assert!(validate(&artifact, &bytes, &missing_source).is_err());
    }

    #[test]
    fn rejects_duplicate_package_manifest_and_unselected_binary() {
        let (artifact, bytes, mut metadata) = fixture();
        metadata["packages"][1]["manifest_path"] = json!("/workspace/cli/Cargo.toml");
        assert!(
            validate(&artifact, &bytes, &metadata)
                .unwrap_err()
                .contains("duplicate")
        );
        let (_, _, mut metadata) = fixture();
        metadata["packages"][0]["targets"][0]["name"] = json!("other");
        assert!(
            validate(&artifact, &bytes, &metadata)
                .unwrap_err()
                .contains("mismatches")
        );
    }
}
