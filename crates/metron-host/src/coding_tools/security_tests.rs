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
