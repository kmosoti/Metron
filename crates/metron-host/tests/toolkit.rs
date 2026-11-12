//! Operator inventory is bounded and cannot manufacture runtime qualification.
use metron_inference_local::artifact::sha256_file;
use metron_protocol::{
    BoundedText, Digest, content_digest,
    toolkit::{ToolCatalog, ToolId, ToolPin},
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn inventory_checks_bytes_before_execution_and_rejects_bad_versions() {
    let root = std::env::temp_dir().join(format!("metron-toolkit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let executable = root.join("fixture-tool");
    let marker = root.join("executed");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nprintf 'fixture-tool 1.0\\n'\nprintf executed > '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let hash = Digest::new(sha256_file(&executable).unwrap()).unwrap();
    let file = root.join("catalog.json");
    let mut catalog = ToolCatalog {
        schema_version: 1,
        tools: vec![ToolPin {
            tool: ToolId::Ripgrep,
            executable: BoundedText::new(executable.to_str().unwrap()).unwrap(),
            sha256: content_digest(b"wrong binary"),
            version_prefix: BoundedText::new("fixture-tool 1.0").unwrap(),
        }],
    };
    let invoke = |catalog: &ToolCatalog| {
        fs::write(&file, serde_json::to_vec(catalog).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
            .args(["toolkit", "inspect"])
            .arg(&file)
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["scope"], "inventory_only_not_runtime_qualification");
        (output, report)
    };
    let (output, report) = invoke(&catalog);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["all_identified"], false);
    assert!(!marker.exists());
    catalog.tools[0].sha256 = hash;
    let (output, report) = invoke(&catalog);
    assert!(output.status.success());
    assert_eq!(report["all_identified"], true);
    assert_eq!(report["sqlite_fts5_available"], true);
    assert!(marker.exists());
    catalog.tools[0].version_prefix = BoundedText::new("fixture-tool 2.0").unwrap();
    let (output, report) = invoke(&catalog);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["entries"][0]["diagnostic"], "version mismatch");
    catalog.tools.push(catalog.tools[0].clone());
    let (output, _) = invoke(&catalog);
    assert_eq!(output.status.code(), Some(2));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inventory_rejects_a_fifo_catalog_without_waiting_for_a_writer() {
    let root = std::env::temp_dir().join(format!("metron-fifo-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    let fifo = root.join("catalog");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_metron-runtime"))
        .args(["toolkit", "inspect"])
        .arg(&fifo)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"], "catalog must be an available regular file");
    fs::remove_dir_all(root).unwrap();
}
