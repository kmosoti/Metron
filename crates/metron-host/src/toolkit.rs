//! Operator-only inventory. This command does not admit runtime tool effects.
use metron_inference_local::artifact::sha256_file;
use metron_protocol::{
    Digest, MAX_ENVELOPE, parse_envelope,
    toolkit::{ToolCatalog, ToolId, ToolPin},
};
use rustix::fs::{MemfdFlags, Mode, OFlags, SealFlags, fcntl_add_seals, memfd_create, open};
use serde::Serialize;
use std::{
    fs::File,
    io::Read,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Serialize)]
struct Entry {
    tool: ToolId,
    expected_sha256: Digest,
    observed_sha256: Option<String>,
    version: Option<String>,
    identified: bool,
    diagnostic: Option<&'static str>,
}
#[derive(Serialize)]
struct Report {
    schema_version: u16,
    scope: &'static str,
    catalog_digest: Digest,
    all_identified: bool,
    sqlite_version: Option<String>,
    sqlite_fts5_available: bool,
    entries: Vec<Entry>,
}

struct PinnedImage {
    // RAII keeps the sealed object available to the child/interpreter.
    _file: File,
    path: PathBuf,
}

fn regular_file(path: &str) -> Result<File, &'static str> {
    // NONBLOCK lets us reject a FIFO without waiting for a writer. Operate on
    // the opened object rather than checking one path and reading another.
    let file = File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| "file unavailable")?,
    );
    if !file
        .metadata()
        .map_err(|_| "metadata unavailable")?
        .is_file()
    {
        return Err("non-regular file rejected");
    }
    Ok(file)
}

fn pin_image(pin: &ToolPin) -> Result<PinnedImage, &'static str> {
    let source = regular_file(pin.executable.as_str())?;
    let metadata = source.metadata().map_err(|_| "metadata unavailable")?;
    if metadata.len() > 512 * 1024 * 1024 || metadata.permissions().mode() & 0o111 == 0 {
        return Err("executable exceeds inventory bound or is not executable");
    }
    let mut image = File::from(
        memfd_create(
            "metron-tool-inventory",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::EXEC,
        )
        .map_err(|_| "sealed executable images unavailable")?,
    );
    let copied = std::io::copy(&mut source.take(512 * 1024 * 1024 + 1), &mut image)
        .map_err(|_| "image copy failed")?;
    if copied > 512 * 1024 * 1024 {
        return Err("executable exceeds inventory bound");
    }
    fcntl_add_seals(
        &image,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .map_err(|_| "executable sealing failed")?;
    let path = PathBuf::from(format!(
        "/proc/{}/fd/{}",
        std::process::id(),
        image.as_raw_fd()
    ));
    Ok(PinnedImage { _file: image, path })
}

fn version(pin: &ToolPin, image: &PinnedImage) -> Result<String, &'static str> {
    let arg = if pin.tool == ToolId::Z3 {
        "-version"
    } else {
        "--version"
    };
    // The kept-open, sealed image is the object hashed by inspect. Path swaps
    // or in-place source writes cannot substitute another executable here.
    // Interpreters and shared libraries remain operator-trusted dependencies;
    // this inventory is not distribution or sandbox qualification.
    let parent = std::path::Path::new(pin.executable.as_str())
        .parent()
        .ok_or("invalid executable path")?;
    let libraries = std::env::join_paths([parent.join("../lib"), parent.join("../lib/lean")])
        .map_err(|_| "invalid library path")?;
    let mut child = Command::new(&image.path)
        .arg(arg)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("LD_LIBRARY_PATH", libraries)
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "version process could not start")?;
    fn capture(reader: impl Read + Send + 'static) -> thread::JoinHandle<std::io::Result<Vec<u8>>> {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            reader.take(8193).read_to_end(&mut bytes)?;
            Ok(bytes)
        })
    }
    let stdout = capture(child.stdout.take().ok_or("stdout unavailable")?);
    let stderr = capture(child.stderr.take().ok_or("stderr unavailable")?);
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < Duration::from_secs(5) => {
                thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                // Arbitrary executable descendants are not qualified by this
                // metadata probe. Never turn a timeout into identified=true.
                return Err("version probe uncertain; process-tree cleanup not established");
            }
        }
    };
    // Never wait indefinitely for descendants holding pipe handles. These are
    // trusted operator-selected executables, but a failed probe stays failed.
    let drain = Instant::now();
    while !(stdout.is_finished() && stderr.is_finished()) {
        if drain.elapsed() >= Duration::from_millis(250) {
            return Err("version descendants retained output; cleanup not established");
        }
        thread::sleep(Duration::from_millis(5));
    }
    let out = stdout
        .join()
        .map_err(|_| "capture failed")?
        .map_err(|_| "capture failed")?;
    let err = stderr
        .join()
        .map_err(|_| "capture failed")?
        .map_err(|_| "capture failed")?;
    if !status.success() || out.len() > 8192 || err.len() > 8192 {
        return Err("version probe failed or exceeded output bound");
    }
    let out = String::from_utf8(out).map_err(|_| "non-UTF-8 version")?;
    let err = String::from_utf8(err).map_err(|_| "non-UTF-8 version")?;
    let line = out
        .lines()
        .next()
        .or_else(|| err.lines().next())
        .ok_or("missing version")?;
    if line.len() > 1024 {
        return Err("version line exceeded bound");
    }
    Ok(line.to_owned())
}

fn inspect(pin: &ToolPin) -> Entry {
    let mut entry = Entry {
        tool: pin.tool,
        expected_sha256: pin.sha256.clone(),
        observed_sha256: None,
        version: None,
        identified: false,
        diagnostic: None,
    };
    let image = match pin_image(pin) {
        Ok(image) => image,
        Err(error) => {
            entry.diagnostic = Some(error);
            return entry;
        }
    };
    let hash = match sha256_file(&image.path) {
        Ok(hash) => hash,
        Err(_) => {
            entry.diagnostic = Some("executable missing or unreadable");
            return entry;
        }
    };
    entry.observed_sha256 = Some(hash.clone());
    if hash != pin.sha256.as_str() {
        entry.diagnostic = Some("executable digest mismatch; not executed");
        return entry;
    }
    match version(pin, &image) {
        Ok(version) => {
            let matched = version.starts_with(pin.version_prefix.as_str());
            entry.version = Some(version);
            if !matched {
                entry.diagnostic = Some("version mismatch");
            } else {
                entry.identified = true;
            }
        }
        Err(error) => entry.diagnostic = Some(error),
    }
    entry
}

pub fn execute(args: &[String]) -> ExitCode {
    fn run(args: &[String]) -> Result<Report, &'static str> {
        if args.len() != 2 || args[0] != "inspect" {
            return Err("usage: toolkit inspect CATALOG.json");
        }
        let file =
            regular_file(&args[1]).map_err(|_| "catalog must be an available regular file")?;
        let mut bytes = Vec::new();
        file.take((MAX_ENVELOPE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "catalog unreadable")?;
        let catalog: ToolCatalog = parse_envelope(&bytes).map_err(|_| "invalid catalog")?;
        let digest = catalog.digest().map_err(|_| "invalid catalog")?;
        let entries: Vec<_> = catalog.tools.iter().map(inspect).collect();
        let storage = metron_store_sqlite::search_capabilities().ok();
        Ok(Report {
            schema_version: 1,
            scope: "inventory_only_not_runtime_qualification",
            catalog_digest: digest,
            all_identified: entries.iter().all(|e| e.identified),
            sqlite_version: storage.as_ref().map(|(version, _)| version.clone()),
            sqlite_fts5_available: storage.is_some_and(|(_, available)| available),
            entries,
        })
    }
    match run(args) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string(&report).expect("bounded inventory")
            );
            ExitCode::from(if report.all_identified { 0 } else { 1 })
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({"schema_version":1,"scope":"inventory_only_not_runtime_qualification","error":error})
            );
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metron_protocol::BoundedText;
    use std::{fs, io::Write};

    #[test]
    fn sealed_probe_executes_verified_image_after_source_replacement() {
        let root = std::env::temp_dir().join(format!("metron-image-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let source = root.join("tool");
        fs::write(&source, "#!/bin/sh\nprintf 'pinned 1\\n'\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
        let pin = ToolPin {
            tool: ToolId::Ripgrep,
            executable: BoundedText::new(source.to_str().unwrap()).unwrap(),
            sha256: Digest::new(sha256_file(&source).unwrap()).unwrap(),
            version_prefix: BoundedText::new("pinned 1").unwrap(),
        };
        let image = pin_image(&pin).unwrap();
        assert_eq!(sha256_file(&image.path).unwrap(), pin.sha256.as_str());
        fs::write(&source, "#!/bin/sh\nprintf 'substituted 2\\n'\n").unwrap();
        assert_eq!(version(&pin, &image).unwrap(), "pinned 1");
        assert!(
            image
                ._file
                .try_clone()
                .unwrap()
                .write_all(b"replacement")
                .is_err()
        );
        assert_eq!(sha256_file(&image.path).unwrap(), pin.sha256.as_str());
        fs::remove_dir_all(root).unwrap();
    }
}
