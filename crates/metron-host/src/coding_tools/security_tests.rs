use super::*;

#[test]
fn existing_file_create_is_denied_with_cas_guidance() {
    let dir = std::env::temp_dir().join(format!("metron-edit-guidance-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let root = File::open(&dir).unwrap();
    let path = WorkspacePath::new("source.rs").unwrap();
    fs::write(dir.join("source.rs"), "original").unwrap();
    let dispatch = content_digest(b"edit-guidance");
    assert_eq!(
        edit_file(&root, &path, None, b"replacement", 100, &dispatch).unwrap_err(),
        "file exists; read its digest before replacing it"
    );
    assert_eq!(fs::read(dir.join("source.rs")).unwrap(), b"original");
    let read = read_file(&root, &path, 100).unwrap();
    edit_file(
        &root,
        &path,
        Some(&content_digest(&read)),
        b"replacement",
        100,
        &dispatch,
    )
    .unwrap();
    assert_eq!(fs::read(dir.join("source.rs")).unwrap(), b"replacement");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn pin_rejects_tampered_digest_without_disk_staging() {
    let dir = std::env::temp_dir().join(format!("metron-pin-digest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("tool");
    fs::write(&exe, b"#!/bin/sh\necho ok\n").unwrap();
    let bad_pin = metron_protocol::toolkit::ToolPin {
        tool: metron_protocol::toolkit::ToolId::Cargo,
        executable: metron_protocol::BoundedText::new(exe.to_string_lossy().as_ref()).unwrap(),
        sha256: metron_protocol::Digest::new(
            "0000000000000000000000000000000000000000000000000000000000000000",
        )
        .unwrap(),
        version_prefix: metron_protocol::BoundedText::new("cargo 1.100").unwrap(),
    };
    assert_eq!(
        process::pin(&bad_pin).err(),
        Some("executable digest mismatch")
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn require_disk_rejects_tmpfs_when_mounted() {
    let tmp = Path::new("/tmp");
    if tmp.is_dir() && process::require_disk(tmp).is_err() {
        assert_eq!(process::require_disk(tmp), Err("RAM-backed scratch denied"));
    }
}
