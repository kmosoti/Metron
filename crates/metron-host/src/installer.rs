//! Explicit installer for the two pinned qualification artifacts.
use metron_inference_local::artifact::{AUXILIARY_FILES, Artifact, ArtifactFile, verify_auxiliary};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
};

const MODEL_REPOSITORY: &str = "https://huggingface.co/LiquidAI/LFM3-1.7B-GGUF/resolve/cd0c74c575b5c416f3011b5b4c298077f26ca8c2/";
const AUX_REPOSITORY: &str =
    "https://huggingface.co/LiquidAI/LFM3-1.7B/resolve/1da06bec6a819da10b54c67e5c90972b46d81871/";
const MAX_DIAGNOSTIC: u64 = 4096;
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Fetch a pinned artifact and its pinned tokenizer, configuration and license files.
pub fn install(artifact: Artifact, cache: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(cache)?;
    match fs::symlink_metadata(cache.join("tokenizer.json")) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "root tokenizer.json makes embedded tokenizer ambiguous",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let auxiliary = cache
        .join("auxiliary")
        .join("1da06bec6a819da10b54c67e5c90972b46d81871");
    fs::create_dir_all(&auxiliary)?;
    for file in AUXILIARY_FILES {
        install_one(*file, AUX_REPOSITORY, &auxiliary)?;
    }
    let model = ArtifactFile {
        filename: artifact.filename(),
        size: artifact.size(),
        sha256: artifact.sha256(),
    };
    install_one(model, MODEL_REPOSITORY, cache)
}

fn install_one(
    artifact: ArtifactFile,
    repository: &'static str,
    cache: &Path,
) -> io::Result<PathBuf> {
    fs::create_dir_all(cache)?;
    let destination = cache.join(artifact.filename);
    if destination.try_exists()? {
        verify_auxiliary(&destination, artifact)?;
        return Ok(destination);
    }
    let temporary_path = cache.join(format!(
        ".{}.{}.part",
        artifact.filename,
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)?;
    // Only remove a temporary file after this invocation successfully created it.
    let temporary = TempFile(temporary_path);
    let url = format!("{repository}{}", artifact.filename);
    let child = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-redirs",
            "5",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "20",
            "--max-time",
            "1800",
            "--",
            &url,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut child = ChildGuard(child);
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("curl stdout unavailable"))?;
    let stderr = child
        .0
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("curl stderr unavailable"))?;
    let diagnostic = thread::spawn(move || -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        stderr.take(MAX_DIAGNOSTIC).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    let mut received = 0_u64;
    let transfer = (|| -> io::Result<()> {
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let n = stdout.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            received = received
                .checked_add(n as u64)
                .ok_or_else(|| io::Error::other("artifact length overflow"))?;
            if received > artifact.size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "artifact exceeds pinned size",
                ));
            }
            file.write_all(&buffer[..n])?;
        }
        Ok(())
    })();
    if transfer.is_err() && child.0.try_wait()?.is_none() {
        child.0.kill()?;
    }
    drop(stdout);
    let status = child.0.wait()?;
    let stderr = diagnostic
        .join()
        .map_err(|_| io::Error::other("curl diagnostic reader failed"))??;
    transfer?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    if received != artifact.size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "artifact size mismatch",
        ));
    }
    file.sync_all()?;
    drop(file);
    verify_auxiliary(&temporary.0, artifact)?;
    match fs::hard_link(&temporary.0, &destination) {
        Ok(()) => File::open(cache)?.sync_all()?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            verify_auxiliary(&destination, artifact)?
        }
        Err(e) => return Err(e),
    }
    fs::remove_file(&temporary.0)?;
    File::open(cache)?.sync_all()?;
    Ok(destination)
}
