use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_metron-runtime")
}

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "metron-contract-cli-it-{tag}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    dir
}

fn write_file(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut f = fs::File::create(path).unwrap();
    f.write_all(bytes).unwrap();
    f.sync_all().unwrap();
}

fn run_cli(args: &[&str]) -> (i32, serde_json::Value) {
    let output: Output = Command::new(bin()).args(args).output().unwrap();
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "CLI exited {code} with non-JSON stdout: {e}\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (code, json)
}

#[test]
fn cli_subprocess_validate_freeze_audit_and_rejections() {
    let root = temp_root("e2e");
    let ws = root.join("workspace");
    fs::DirBuilder::new().mode(0o700).create(&ws).unwrap();

    let cargo_toml = b"[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
    let cargo_lock = b"# lock\nversion = 4\n";
    let main_rs = b"fn main() { println!(\"v1\"); }\n";
    write_file(&ws.join("Cargo.toml"), cargo_toml);
    write_file(&ws.join("Cargo.lock"), cargo_lock);
    write_file(&ws.join("src/main.rs"), main_rs);

    let req = "Fix addition output while preserving --version v1 and Cargo.lock unchanged.";
    let req_path = root.join("request.txt");
    write_file(&req_path, req.as_bytes());
    let clar = "empty args exit 2; empty or newline stdout allowed.";
    let clar_path = root.join("clarification.txt");
    write_file(&clar_path, clar.as_bytes());

    let draft_json = serde_json::json!({
        "schema_version": 1,
        "contract_id": "cli-existing-add",
        "workflow": "existing",
        "raw_request": req,
        "raw_request_sha256": metron_protocol::content_digest(req.as_bytes()),
        "sources": [
            {
                "id": "src-request",
                "claimed_kind": "operator_request",
                "label": "request.txt",
                "content_sha256": metron_protocol::content_digest(req.as_bytes())
            },
            {
                "id": "src-clarification",
                "claimed_kind": "clarification_answer",
                "label": "clarification.txt",
                "content_sha256": metron_protocol::content_digest(clar.as_bytes())
            }
        ],
        "baseline": {
            "workspace_label": "sample",
            "files": [
                {
                    "path": "Cargo.lock",
                    "sha256": metron_protocol::content_digest(cargo_lock),
                    "preserved": true
                },
                {
                    "path": "Cargo.toml",
                    "sha256": metron_protocol::content_digest(cargo_toml),
                    "preserved": true
                },
                {
                    "path": "src/main.rs",
                    "sha256": metron_protocol::content_digest(main_rs),
                    "preserved": false
                }
            ],
            "local_rust_package_count": 1,
            "selected_cli_binary": "sample",
            "selected_cli_target_count": 1
        },
        "compatibility": {
            "approved_compatibility": ["preserve --version v1"],
            "intended_delta": ["compute integer sum instead of difference"],
            "preserved_files": ["Cargo.lock", "Cargo.toml"],
            "allowed_edit_paths": ["src/main.rs"],
            "forbidden_files": []
        },
        "requirements": {
            "mandatory_obligations": [
                {
                    "id": "r1",
                    "clause": "sum two signed integers",
                    "checker": "selected-cli-vectors-v1",
                    "source_ref": "src-request"
                }
            ],
            "unknowns": [
                {
                    "id": "u1",
                    "question": "empty args exit code?",
                    "material": true,
                    "resolved_by": "res1",
                    "blocking": true
                }
            ],
            "resolutions": [
                {
                    "id": "res1",
                    "unknown_id": "u1",
                    "answer": "empty args exit 2",
                    "source_ref": "src-clarification",
                    "approved": false
                }
            ]
        },
        "checker_policy": {
            "metadata_checker": "snapshot-local-path-v1",
            "build_checker": "cargo-offline-locked-v1",
            "test_checker": "cargo-test-offline-locked-v1",
            "cli_checker": "selected-cli-vectors-v1",
            "max_local_packages": 2,
            "fixed_files": [
                {
                    "id": "lock",
                    "path": "Cargo.lock",
                    "expected_sha256": metron_protocol::content_digest(cargo_lock)
                }
            ],
            "interface_checks": [],
            "cli_vectors": [
                {
                    "args": ["4", "7"],
                    "expected_exit_code": 0,
                    "expected_stdout": "11\n"
                },
                {
                    "args": [""],
                    "expected_exit_code": 2,
                    "expected_stdout": ""
                },
                {
                    "args": [],
                    "expected_exit_code": 0,
                    "expected_stdout": "\n"
                }
            ]
        }
    });
    let draft_path = root.join("draft.json");
    write_file(&draft_path, &serde_json::to_vec(&draft_json).unwrap());

    // 1. validate without clarification -> blocked even if model approved=true!
    let mut no_clar = draft_json.clone();
    no_clar["requirements"]["resolutions"][0]["approved"] = serde_json::json!(true);
    let no_clar_path = root.join("no-clar-draft.json");
    write_file(&no_clar_path, &serde_json::to_vec(&no_clar).unwrap());
    let (nc_code, nc_val) = run_cli(&[
        "coding",
        "contract",
        "validate",
        "--draft",
        no_clar_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    assert_eq!(nc_code, 4);
    assert_eq!(nc_val["declared_ready"], false);

    // 2. validate with host-authenticated clarification -> exit 0 + prospective digest
    let (code, val) = run_cli(&[
        "coding",
        "contract",
        "validate",
        "--draft",
        draft_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--clarification-file",
        clar_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "validate failed: {val}");
    assert_eq!(val["declared_ready"], true);
    let digest = val["prospective_bundle_digest"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(digest.len(), 64);

    // 3. freeze with wrong digest fails exit 2
    let bundle_path = root.join("frozen-bundle.json");
    let (bad_code, _) = run_cli(&[
        "coding",
        "contract",
        "freeze",
        "--draft",
        draft_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--clarification-file",
        clar_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
        "--expected-digest",
        metron_protocol::content_digest(b"wrong").as_str(),
        "--output",
        bundle_path.to_str().unwrap(),
    ]);
    assert_eq!(bad_code, 2);
    assert!(!bundle_path.exists());

    // 4. freeze with exact digest exits 0
    let (f_code, frozen) = run_cli(&[
        "coding",
        "contract",
        "freeze",
        "--draft",
        draft_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--clarification-file",
        clar_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
        "--expected-digest",
        &digest,
        "--output",
        bundle_path.to_str().unwrap(),
    ]);
    assert_eq!(f_code, 0, "freeze failed: {frozen}");
    assert_eq!(frozen["bundle_digest"], digest);

    // 5. immutable no-overwrite freeze exits 2
    let (dup_code, _) = run_cli(&[
        "coding",
        "contract",
        "freeze",
        "--draft",
        draft_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--clarification-file",
        clar_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
        "--expected-digest",
        &digest,
        "--output",
        bundle_path.to_str().unwrap(),
    ]);
    assert_eq!(dup_code, 2);

    // 6. audit succeeds exit 0
    let (a_code, audit) = run_cli(&[
        "coding",
        "contract",
        "audit",
        "--bundle",
        bundle_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--clarification-file",
        clar_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    assert_eq!(a_code, 0, "audit failed: {audit}");
    assert_eq!(audit["sources_reauthenticated"], true);

    // 7. baseline tamper detected by audit (exit 4)
    write_file(
        &ws.join("src/main.rs"),
        b"fn main() { println!(\"tampered\"); }\n",
    );
    let (stale_code, stale_audit) = run_cli(&[
        "coding",
        "contract",
        "audit",
        "--bundle",
        bundle_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--clarification-file",
        clar_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    assert_eq!(stale_code, 4);
    assert_eq!(stale_audit["outcome"], "integrity_failure");

    // 8. ancestor symlink swap rejected exit 2
    let sub_real = root.join("sub_real");
    fs::DirBuilder::new().mode(0o700).create(&sub_real).unwrap();
    write_file(
        &sub_real.join("leaf.json"),
        &fs::read(&bundle_path).unwrap(),
    );
    let sub_sym = root.join("sub_sym");
    symlink(&sub_real, &sub_sym).unwrap();
    let (sym_code, _) = run_cli(&[
        "coding",
        "contract",
        "audit",
        "--bundle",
        sub_sym.join("leaf.json").to_str().unwrap(),
    ]);
    assert_eq!(sym_code, 2);

    // 9. duplicate/dangling ref + unknown checker + checker slot swap + objective overflow fail cleanly
    let mut bad = draft_json;
    bad["checker_policy"]["metadata_checker"] = serde_json::json!("cargo-offline-locked-v1");
    let mut obs = Vec::new();
    for idx in 0..5 {
        obs.push(serde_json::json!({
            "id": "dup",
            "clause": format!("{}-{idx}", "é".repeat(110)),
            "checker": format!("unknown-checker-{idx}"),
            "source_ref": "missing-src"
        }));
    }
    bad["requirements"]["mandatory_obligations"] = serde_json::Value::Array(obs);
    let bad_path = root.join("bad-draft.json");
    write_file(&bad_path, &serde_json::to_vec(&bad).unwrap());
    write_file(&ws.join("src/main.rs"), main_rs);
    let (bad_v_code, bad_v) = run_cli(&[
        "coding",
        "contract",
        "validate",
        "--draft",
        bad_path.to_str().unwrap(),
        "--request-file",
        req_path.to_str().unwrap(),
        "--clarification-file",
        clar_path.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
    ]);
    assert_eq!(bad_v_code, 4);
    assert_eq!(bad_v["structurally_valid"], false);
    assert_eq!(bad_v["declared_ready"], false);
    let codes: BTreeSet<&str> = bad_v["report"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains("checker_slot_mismatch"));
    assert!(codes.contains("duplicate_identifier"));
    assert!(codes.contains("dangling_source_ref"));
    assert!(codes.contains("unknown_checker"));
    assert!(codes.contains("objective_overflow"));

    // 10. Malformed/incomplete propose flags exit 2 cleanly
    let (m2_code, _) = run_cli(&["coding", "contract", "propose"]);
    assert_eq!(m2_code, 2);

    // 11. Production subprocesses cannot execute recorded/controlled effects.
    // Scripted staging and unknown outcomes are exercised in development-only tests.
    let prop_dir = root.join("cli-propose-ok");
    let prop_replies = root.join("prop-replies.json");
    write_file(
        &prop_replies,
        &serde_json::to_vec(&vec![
            serde_json::json!({
                "intended_delta": ["compute sum"],
                "approved_compatibility": ["preserve --version v1"],
                "allowed_edit_paths": ["src/main.rs"],
                "preserved_files": ["Cargo.lock"],
                "mandatory_obligations": [{"id": "ob1", "clause": "compute sum", "checker": "cargo-test-offline-locked-v1"}],
                "assumptions": [],
                "unknowns": [],
                "resolutions": []
            })
            .to_string(),
        ])
        .unwrap(),
    );
    let (p_code, p_val) = run_cli(&[
        "coding",
        "contract",
        "propose",
        "--request-file",
        req_path.to_str().unwrap(),
        "--contract-id",
        "cli-prop-1",
        "--output-dir",
        prop_dir.to_str().unwrap(),
        "--workspace",
        ws.to_str().unwrap(),
        "--workflow",
        "existing",
        "--recorded-replies",
        prop_replies.to_str().unwrap(),
    ]);
    assert_eq!(p_code, 2, "recorded execution was not rejected: {p_val}");
    assert_eq!(p_val["outcome"], "error");
    assert!(
        p_val["diagnostic"]
            .as_str()
            .unwrap()
            .contains("recorded model replies cannot execute")
    );
    assert!(!prop_dir.exists());

    let corp_path = root.join("corpus.json");
    let freeze_path = root.join("freeze.json");
    write_file(
        &corp_path,
        metron_testkit::coding::independent_corpus_json().as_bytes(),
    );
    write_file(
        &freeze_path,
        metron_testkit::coding::independent_corpus_freeze_json().as_bytes(),
    );
    let ctrl_path = root.join("ctrl-exp.json");
    write_file(
        &ctrl_path,
        &serde_json::to_vec(&serde_json::json!({
            "diagnostic_scripts": {
                "diagnostic-new-absolute": ["{\"tool\":\"plan\",\"steps\":[\"fail\"]}"],
                "diagnostic-new-uppercase": ["{\"tool\":\"plan\",\"steps\":[\"fail\"]}"],
                "diagnostic-existing-negate": ["{\"tool\":\"plan\",\"steps\":[\"fail\"]}"],
                "diagnostic-existing-colon": ["{\"tool\":\"plan\",\"steps\":[\"fail\"]}"],
            },
            "arm_scripts": {}
        }))
        .unwrap(),
    );
    let exp_dir = root.join("cli-exp-out");
    let (e_code, e_val) = run_cli(&[
        "coding",
        "contract",
        "experiment",
        "--corpus",
        corp_path.to_str().unwrap(),
        "--corpus-freeze",
        freeze_path.to_str().unwrap(),
        "--output-dir",
        exp_dir.to_str().unwrap(),
        "--controlled-replies",
        ctrl_path.to_str().unwrap(),
    ]);
    assert_eq!(e_code, 2, "controlled execution was not rejected: {e_val}");
    assert_eq!(e_val["outcome"], "error");
    assert!(e_val["diagnostic"].as_str().unwrap().contains("test-only"));
    assert!(!exp_dir.exists());

    let (run_block_code, run_block_val) = run_cli(&["coding", "contract", "run"]);
    assert_eq!(run_block_code, 4);
    assert_eq!(run_block_val["outcome"], "blocked");
    let (res_block_code, res_block_val) = run_cli(&["coding", "contract", "resume"]);
    assert_eq!(res_block_code, 4);
    assert_eq!(res_block_val["outcome"], "blocked");

    let _ = fs::remove_dir_all(&root);
}
